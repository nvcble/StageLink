import { readFileSync, readdirSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Keeps `database/migrations/*.sql` honest.
 *
 * The authoritative migrations are compiled into the Rust service; the SQL
 * files exist so the schema can be read and diffed without reading Rust. That
 * is only useful if they match, so this test compares them.
 */

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const migrationsDir = path.join(repoRoot, 'database', 'migrations')
const migrationsRs = path.join(repoRoot, 'crates', 'persistence', 'src', 'migrations.rs')

/** Extracts every `sql: r#"…"#` block from the Rust source, in order. */
function rustMigrationSql(): string[] {
  const source = readFileSync(migrationsRs, 'utf8')
  const blocks: string[] = []
  const pattern = /sql: r#"([\s\S]*?)"#,/g
  let match: RegExpExecArray | null
  while ((match = pattern.exec(source)) !== null) {
    if (match[1] !== undefined) blocks.push(match[1])
  }
  return blocks
}

/** Normalises whitespace so formatting differences do not fail the test. */
function normalise(sql: string): string {
  return sql
    .split('\n')
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0)
    .join('\n')
    .trim()
}

/** Strips the reference-copy header comment block. */
function stripHeader(sql: string): string {
  const lines = sql.split('\n')
  // The header is the leading run of `--` lines before the first blank line or
  // first non-comment line that is part of the migration itself.
  let index = 0
  while (index < lines.length && lines[index]?.startsWith('--')) index += 1
  // The migration's own leading comment is re-attached: only the generated
  // header (which mentions "REFERENCE COPY") is removed.
  const header = lines.slice(0, index).join('\n')
  if (!header.includes('REFERENCE COPY')) return sql
  // Find where the generated header ends — at the line after the last one that
  // is part of it. The generated header always ends with the "known version."
  // sentence.
  const endIndex = lines.findIndex((line) => line.includes('known version.'))
  return lines.slice(endIndex + 1).join('\n')
}

describe('SQL migration reference copies', () => {
  const rustBlocks = rustMigrationSql()
  const files = readdirSync(migrationsDir)
    .filter((name) => name.endsWith('.sql'))
    .sort()

  it('finds the Rust migrations', () => {
    expect(rustBlocks.length).toBeGreaterThan(0)
  })

  it('has one SQL file per Rust migration', () => {
    expect(files).toHaveLength(rustBlocks.length)
  })

  it('names the files in migration order', () => {
    files.forEach((name, index) => {
      expect(name).toMatch(new RegExp(`^0*${index + 1}_`))
    })
  })

  files.forEach((name, index) => {
    it(`${name} matches the compiled migration`, () => {
      const onDisk = normalise(stripHeader(readFileSync(path.join(migrationsDir, name), 'utf8')))
      const inRust = normalise(rustBlocks[index] ?? '')
      expect(onDisk).toBe(inRust)
    })
  })

  it('every migration creates STRICT tables', () => {
    // STRICT catches a type mismatch at write time rather than silently
    // coercing it, which matters for a schema that stores validated JSON.
    for (const block of rustBlocks) {
      const creates = block.match(/CREATE TABLE/g) ?? []
      const stricts = block.match(/\) STRICT;/g) ?? []
      expect(stricts.length, 'every CREATE TABLE should be STRICT').toBe(creates.length)
    }
  })

  it('never stores a secret in an ordinary column', () => {
    // Private keys and session keys belong in the OS-protected store, not in a
    // SQLite row. `public_key` is explicitly fine — it is not a secret.
    //
    // Comments are stripped first: the schema's own comments discuss which
    // material is and is not secret, and matching those would be checking the
    // prose rather than the columns.
    for (const block of rustBlocks) {
      const columns = block
        .split('\n')
        .map((line) => line.replace(/--.*$/, ''))
        .join('\n')
        .toLowerCase()

      for (const forbidden of [
        /\bprivate_key\b/,
        /\bsecret\b/,
        /\bpassword\b/,
        /\bsession_key\b/,
        /\bpairing_code\b/,
      ]) {
        expect(columns, `${forbidden} must not be a column`).not.toMatch(forbidden)
      }
      // And the one that is deliberately present.
      expect(block).toContain('public_key')
    }
  })
})
