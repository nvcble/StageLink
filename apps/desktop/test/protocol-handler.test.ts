import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'

// Electron is not available in a plain Node test process.
vi.mock('electron', () => ({
  app: { isPackaged: false, getAppPath: () => 'C:/stagelink/apps/desktop', getPath: () => 'C:/tmp' },
  protocol: { registerSchemesAsPrivileged: () => {}, handle: () => {} },
}))

vi.mock('../electron/main/logger', () => ({
  logger: { info: () => {}, warn: () => {}, error: () => {}, debug: () => {} },
  logServiceLine: () => {},
  initLogging: () => {},
}))

import { resolveRequestPath } from '../electron/main/protocol'

/**
 * Tests for the `stagelink://` request resolver.
 *
 * This function decides which file a renderer request maps to, which makes it
 * both the thing that makes production routing work and the thing that would
 * leak the filesystem if it were wrong. Both halves are covered here.
 */

let root: string
let outside: string

beforeAll(() => {
  const base = mkdtempSync(path.join(os.tmpdir(), 'stagelink-protocol-'))
  root = path.join(base, 'renderer')
  outside = path.join(base, 'secret')

  mkdirSync(path.join(root, '_nuxt'), { recursive: true })
  mkdirSync(path.join(root, 'receivers'), { recursive: true })
  mkdirSync(outside, { recursive: true })

  writeFileSync(path.join(root, 'index.html'), '<!doctype html><title>app</title>')
  writeFileSync(path.join(root, '_nuxt', 'entry.js'), 'export default 1')
  writeFileSync(path.join(root, 'receivers', 'index.html'), '<!doctype html><title>receivers</title>')
  writeFileSync(path.join(outside, 'passwords.txt'), 'do not serve me')
})

afterAll(() => {
  rmSync(path.dirname(root), { recursive: true, force: true })
})

describe('serving real files', () => {
  it('serves the entry point for the root path', () => {
    expect(resolveRequestPath(root, '/')).toBe(path.join(root, 'index.html'))
  })

  it('serves a build asset by its absolute path', () => {
    // Nuxt emits absolute asset URLs. This is the reason the app is served from
    // a custom protocol rather than file://.
    expect(resolveRequestPath(root, '/_nuxt/entry.js')).toBe(
      path.join(root, '_nuxt', 'entry.js'),
    )
  })

  it('serves a prerendered route directory as its index.html', () => {
    expect(resolveRequestPath(root, '/receivers')).toBe(
      path.join(root, 'receivers', 'index.html'),
    )
  })

  it('serves a trailing-slash route the same way', () => {
    expect(resolveRequestPath(root, '/receivers/')).toBe(
      path.join(root, 'receivers', 'index.html'),
    )
  })

  it('decodes a percent-encoded path', () => {
    expect(resolveRequestPath(root, '/_nuxt%2Fentry.js')).toBe(
      path.join(root, '_nuxt', 'entry.js'),
    )
  })
})

describe('client-side routing', () => {
  it('falls back to the SPA entry for a route with no prerendered directory', () => {
    // A refresh on a client-only route must not 404.
    expect(resolveRequestPath(root, '/sessions/123')).toBe(path.join(root, 'index.html'))
  })

  it('404s a missing asset rather than returning HTML', () => {
    // Returning index.html for a .js request produces a baffling syntax error
    // instead of a clear missing-file error.
    expect(resolveRequestPath(root, '/_nuxt/does-not-exist.js')).toBeNull()
    expect(resolveRequestPath(root, '/missing.css')).toBeNull()
  })
})

describe('refusing to escape the renderer directory', () => {
  it('refuses a traversal with ..', () => {
    expect(resolveRequestPath(root, '/../secret/passwords.txt')).toBeNull()
    expect(resolveRequestPath(root, '/../../etc/passwd')).toBeNull()
    expect(resolveRequestPath(root, '/_nuxt/../../secret/passwords.txt')).toBeNull()
  })

  it('refuses a percent-encoded traversal', () => {
    expect(resolveRequestPath(root, '/%2e%2e/secret/passwords.txt')).toBeNull()
    expect(resolveRequestPath(root, '/..%2fsecret%2fpasswords.txt')).toBeNull()
  })

  it('refuses a path containing a NUL byte', () => {
    // A NUL would truncate the path at the OS layer, turning
    // "index.html\0.js" into "index.html".
    expect(resolveRequestPath(root, '/index.html%00.js')).toBeNull()
  })

  it('refuses an undecodable path rather than guessing', () => {
    expect(resolveRequestPath(root, '/%E0%A4%A')).toBeNull()
  })

  it('refuses a Windows absolute path', () => {
    expect(resolveRequestPath(root, '/C:/Windows/System32/drivers/etc/hosts')).toBeNull()
  })

  it('does not mistake a sibling directory with a shared prefix for the root', () => {
    // `renderer-evil` starts with `renderer` as a string but is a different
    // directory; a naive `startsWith(root)` check would let it through.
    const sibling = `${root}-evil`
    mkdirSync(sibling, { recursive: true })
    writeFileSync(path.join(sibling, 'evil.js'), 'pwned')
    expect(resolveRequestPath(root, '/../renderer-evil/evil.js')).toBeNull()
    rmSync(sibling, { recursive: true, force: true })
  })
})

describe('a missing renderer build', () => {
  it('returns null rather than throwing', () => {
    const empty = mkdtempSync(path.join(os.tmpdir(), 'stagelink-empty-'))
    expect(resolveRequestPath(empty, '/')).toBeNull()
    expect(resolveRequestPath(empty, '/anything')).toBeNull()
    rmSync(empty, { recursive: true, force: true })
  })
})
