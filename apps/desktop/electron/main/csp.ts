import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'

import { APP_ORIGIN, DEV_SERVER_URL } from './paths'

/**
 * Content-Security-Policy construction.
 *
 * Nuxt emits inline `<script>` blocks into every generated HTML file: a
 * colour-mode bootstrap that runs before first paint (so the app never flashes
 * light), the runtime config, and an import map. A CSP without `unsafe-inline`
 * blocks them, and the symptom is brutal — `window.__NUXT__.config` is never
 * assigned, `createNuxtApp` tries to proxy `undefined`, and the window renders
 * completely blank with only a CSP violation in a console nobody is watching.
 *
 * Rather than granting `unsafe-inline` to scripts, the production policy pins
 * the SHA-256 hash of each inline script actually present in the build. The
 * hashes are computed at startup from the files the protocol handler will
 * serve, so they cannot drift from what is on disk, and a tampered HTML file
 * fails to execute instead of running.
 *
 * Development is different and deliberately weaker: Vite's HMR client needs
 * `unsafe-eval`, and the dev server regenerates inline scripts per request with
 * a changing build id, so hashing them is not possible. Development builds are
 * not what ships, and `isDevelopment()` gates the difference.
 */

/** Finds every inline `<script>` body in an HTML document. */
export function extractInlineScripts(html: string): string[] {
  // Matches a <script> with no `src` attribute. Deliberately includes
  // `type="importmap"` and JSON data blocks: CSP applies to the import map, and
  // hashing the data blocks costs nothing and avoids a surprise if a future
  // Nuxt version starts executing one.
  const pattern = /<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g
  const bodies: string[] = []
  let match: RegExpExecArray | null
  while ((match = pattern.exec(html)) !== null) {
    const body = match[1]
    if (body !== undefined && body.length > 0) bodies.push(body)
  }
  return bodies
}

/** SHA-256 CSP source token for a script body. */
export function scriptHash(body: string): string {
  return `'sha256-${createHash('sha256').update(body, 'utf8').digest('base64')}'`
}

/**
 * Collects the hashes of every inline script in the built renderer.
 *
 * Walks all HTML files, not just `index.html`: each prerendered route carries
 * its own config and payload blocks, so their hashes differ.
 */
export function collectInlineScriptHashes(rendererRoot: string): string[] {
  const hashes = new Set<string>()

  const walk = (directory: string, depth = 0) => {
    // The renderer tree is shallow; the bound stops a symlink loop from
    // becoming an infinite walk.
    if (depth > 8) return
    let entries: string[]
    try {
      entries = readdirSync(directory)
    } catch {
      return
    }
    for (const entry of entries) {
      const candidate = path.join(directory, entry)
      let stats: ReturnType<typeof statSync>
      try {
        stats = statSync(candidate)
      } catch {
        continue
      }
      if (stats.isDirectory()) {
        walk(candidate, depth + 1)
      } else if (entry.endsWith('.html')) {
        try {
          for (const body of extractInlineScripts(readFileSync(candidate, 'utf8'))) {
            hashes.add(scriptHash(body))
          }
        } catch {
          // An unreadable HTML file is the protocol handler's problem, not
          // the policy's.
        }
      }
    }
  }

  walk(rendererRoot)
  return [...hashes]
}

/**
 * Builds the policy.
 *
 * `connect-src` is the one worth reading twice: in production it is `'self'`
 * only. StageLink needs no internet access for live audio, so the renderer is
 * given no way to reach the network at all. Every piece of data it displays
 * arrives over the Electron IPC bridge from the local Rust service.
 */
export function buildContentSecurityPolicy(options: {
  development: boolean
  inlineScriptHashes?: readonly string[]
}): string {
  const { development, inlineScriptHashes = [] } = options

  const scriptSources = development
    ? // Vite's HMR client evaluates code, and dev-server inline scripts change
      // per request so they cannot be hashed.
      [`'self'`, `'unsafe-inline'`, `'unsafe-eval'`, DEV_SERVER_URL]
    : [`'self'`, APP_ORIGIN, ...inlineScriptHashes]

  const origin = development ? DEV_SERVER_URL : APP_ORIGIN

  return [
    `default-src 'none'`,
    `script-src ${scriptSources.join(' ')}`,
    // Styles do get 'unsafe-inline': Tailwind and Nuxt UI inject style
    // attributes and <style> blocks at runtime. Inline *styles* cannot execute
    // code, so this is a far smaller concession than the script equivalent.
    `style-src 'self' 'unsafe-inline' ${origin}`,
    `img-src 'self' data: ${origin}`,
    `font-src 'self' data: ${origin}`,
    development
      ? `connect-src 'self' ${DEV_SERVER_URL} ws://127.0.0.1:3579`
      : `connect-src 'self'`,
    `object-src 'none'`,
    `base-uri 'none'`,
    `form-action 'none'`,
    `frame-ancestors 'none'`,
    `worker-src 'self'`,
  ].join('; ')
}
