import { readFileSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Guards against a bug class TypeScript cannot see.
 *
 * `isDevelopment` and `isPackaged` are functions. Writing `if (!isDevelopment)`
 * instead of `if (!isDevelopment())` is perfectly valid TypeScript — a function
 * reference is always truthy — and it silently inverts a build-mode branch.
 *
 * This actually happened: a missing pair of parentheses meant the production
 * protocol handler was never registered, and the packaged application opened a
 * blank window with `ERR_FAILED` as the only clue. Unit tests passed, the
 * typechecker was happy, and the development build was unaffected.
 */

const electronDir = fileURLToPath(new URL('../electron', import.meta.url))

/** Every `.ts` file under electron/. */
function sourceFiles(directory: string): string[] {
  const files: string[] = []
  for (const entry of readdirSync(directory)) {
    const candidate = path.join(directory, entry)
    if (statSync(candidate).isDirectory()) files.push(...sourceFiles(candidate))
    else if (entry.endsWith('.ts')) files.push(candidate)
  }
  return files
}

describe('build-mode predicates are always called, never referenced', () => {
  const files = sourceFiles(electronDir)

  it('finds the Electron sources', () => {
    expect(files.length).toBeGreaterThan(5)
  })

  for (const predicate of ['isDevelopment', 'isPackaged']) {
    it(`never uses \`${predicate}\` as a value`, () => {
      const offences: string[] = []

      for (const file of files) {
        const relative = path.relative(electronDir, file)
        const lines = readFileSync(file, 'utf8').split('\n')

        lines.forEach((line, index) => {
          // Skip the declarations themselves, imports, comments and the
          // same-named preload field (which is a plain boolean property).
          const trimmed = line.trim()
          if (
            trimmed.startsWith('//') ||
            trimmed.startsWith('*') ||
            trimmed.startsWith('/*') ||
            trimmed.startsWith('import ') ||
            trimmed.startsWith('export function') ||
            /^\s*isPackaged:\s/.test(line)
          ) {
            return
          }

          // A bare occurrence not followed by `(` is being used as a value.
          const pattern = new RegExp(`(^|[^.\\w])${predicate}(?!\\s*\\()`, 'g')
          if (pattern.test(line)) {
            offences.push(`${relative}:${index + 1}: ${trimmed}`)
          }
        })
      }

      expect(offences, `\`${predicate}\` must be called, not referenced`).toEqual([])
    })
  }
})

describe('the production renderer path is wired up', () => {
  it('registers the protocol handler when not in development', () => {
    const source = readFileSync(path.join(electronDir, 'main', 'index.ts'), 'utf8')
    // The exact shape that was broken.
    expect(source).toContain('if (!isDevelopment()) {')
    expect(source).toContain('registerProtocolHandler()')
  })

  it('registers the privileged scheme before the app is ready', () => {
    // Comments mention both calls, so they are stripped first — otherwise this
    // compares the positions of prose rather than of code.
    const source = readFileSync(path.join(electronDir, 'main', 'index.ts'), 'utf8')
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/^\s*\/\/.*$/gm, '')

    const schemeIndex = source.indexOf('registerProtocolScheme()')
    const readyIndex = source.indexOf('await app.whenReady()')
    expect(schemeIndex, 'registerProtocolScheme() must be called').toBeGreaterThan(-1)
    expect(readyIndex, 'app.whenReady() must be awaited').toBeGreaterThan(-1)
    // Electron requires registerSchemesAsPrivileged before the ready event.
    expect(schemeIndex).toBeLessThan(readyIndex)
  })
})
