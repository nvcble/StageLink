import { createReadStream, existsSync, statSync } from 'node:fs'
import path from 'node:path'
import { Readable } from 'node:stream'
import { protocol } from 'electron'

import { APP_PROTOCOL, rendererRoot } from './paths'
import { logger } from './logger'

/**
 * Serves the built Nuxt output over a custom `stagelink://` scheme.
 *
 * Why not `file://`: Nuxt generates absolute asset URLs (`/_nuxt/...`) and uses
 * HTML history routing. Under `file://` the absolute URLs resolve against the
 * drive root, a refresh on `/receivers` looks for a directory that does not
 * exist, and the page has an opaque origin that breaks normal web security
 * assumptions. A custom scheme registered as standard and secure behaves like
 * a real origin and fixes all three.
 */

/** Must be called before `app.whenReady()`. */
export function registerProtocolScheme(): void {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: APP_PROTOCOL,
      privileges: {
        standard: true,
        secure: true,
        supportFetchAPI: true,
        corsEnabled: true,
        // No stream protocol needed and no service workers: the renderer is
        // local, so there is nothing to cache offline.
        stream: true,
      },
    },
  ])
}

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.map': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.txt': 'text/plain; charset=utf-8',
}

/** Installs the request handler. Call after `app.whenReady()`. */
export function registerProtocolHandler(): void {
  const root = rendererRoot()
  logger.info(`serving the renderer from ${root}`)

  protocol.handle(APP_PROTOCOL, async (request) => {
    const url = new URL(request.url)
    const resolved = resolveRequestPath(root, url.pathname)

    if (!resolved) {
      return new Response('Not found', { status: 404, headers: { 'content-type': 'text/plain' } })
    }

    try {
      const stats = statSync(resolved)
      const body = Readable.toWeb(createReadStream(resolved)) as ReadableStream<Uint8Array>
      return new Response(body, {
        status: 200,
        headers: {
          'content-type': MIME_TYPES[path.extname(resolved).toLowerCase()] ?? 'application/octet-stream',
          'content-length': String(stats.size),
          // The renderer is immutable per build, but index.html must not be
          // cached across an update or the app would load stale script URLs.
          'cache-control': resolved.endsWith('index.html')
            ? 'no-cache'
            : 'public, max-age=31536000, immutable',
        },
      })
    } catch (error) {
      logger.error(`failed to serve ${url.pathname}: ${String(error)}`)
      return new Response('Internal error', { status: 500 })
    }
  })
}

/**
 * Maps a request path to a file on disk.
 *
 * Two jobs: refuse to escape the renderer directory, and fall back to
 * `index.html` for client-side routes so a refresh on `/diagnostics` works.
 * Returns `null` when the request cannot be satisfied.
 */
export function resolveRequestPath(root: string, pathname: string): string | null {
  // Decode once, then normalise. A path that escapes the root after
  // normalisation is refused outright rather than clamped.
  let decoded: string
  try {
    decoded = decodeURIComponent(pathname)
  } catch {
    return null
  }
  // A NUL byte would truncate the path at the OS layer.
  if (decoded.includes('\0')) return null

  const relative = decoded.replace(/^\/+/, '')
  const candidate = path.resolve(root, relative)

  const rootWithSep = path.resolve(root) + path.sep
  if (candidate !== path.resolve(root) && !candidate.startsWith(rootWithSep)) {
    logger.warn(`refused a renderer request that escaped the root: ${pathname}`)
    return null
  }

  if (existsSync(candidate) && statSync(candidate).isFile()) {
    return candidate
  }

  // A prerendered route directory, e.g. /receivers -> receivers/index.html.
  const directoryIndex = path.join(candidate, 'index.html')
  if (existsSync(directoryIndex)) {
    return directoryIndex
  }

  // Anything that looks like an asset must 404 rather than silently returning
  // HTML — an HTML response to a `.js` request produces a baffling syntax
  // error instead of a clear missing-file error.
  if (path.extname(relative) !== '') {
    return null
  }

  // Otherwise it is a client-side route: hand back the SPA entry point.
  const fallback = path.join(root, 'index.html')
  return existsSync(fallback) ? fallback : null
}
