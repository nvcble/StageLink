import { existsSync } from 'node:fs'
import path from 'node:path'
import { app } from 'electron'

/**
 * Filesystem locations, resolved once.
 *
 * Development and packaged builds put things in different places, and getting
 * this wrong is the classic way an Electron app works perfectly until it is
 * installed. Every path the application uses is derived here so there is one
 * place to check.
 */

/**
 * True for a packaged build.
 *
 * A function rather than a module-level constant: `app.isPackaged` is only
 * meaningful once Electron has initialised, so reading it at import time
 * couples correctness to module evaluation order.
 */
export function isPackaged(): boolean {
  return app.isPackaged
}

/** True when the renderer should be loaded from the Nuxt dev server. */
export function isDevelopment(): boolean {
  return !isPackaged() && process.env.NODE_ENV !== 'production'
}

/** Nuxt dev server origin. Must match `dev:nuxt` in package.json. */
export const DEV_SERVER_URL = process.env.STAGELINK_DEV_SERVER ?? 'http://127.0.0.1:3579'

/**
 * Custom protocol the packaged renderer is served from.
 *
 * A real origin rather than `file://`: it gives the renderer a normal security
 * origin, makes absolute asset paths (`/_nuxt/...`) resolve, and lets Nuxt's
 * history-mode router work on refresh and on direct navigation. `file://`
 * breaks all three in ways that only show up after packaging.
 */
export const APP_PROTOCOL = 'stagelink'
export const APP_ORIGIN = `${APP_PROTOCOL}://app`

/** Directory holding the built Nuxt output in a packaged build. */
export function rendererRoot(): string {
  // electron-builder unpacks `.output/public` next to the app bundle.
  return path.join(process.resourcesPath, 'renderer')
}

/** Directory holding the bundled Rust service in a packaged build. */
function packagedServiceDirectory(): string {
  return path.join(process.resourcesPath, 'service')
}

/**
 * Locates the `stagelink-service` executable.
 *
 * Returns `null` when it cannot be found, so the caller can surface a clear
 * "the audio service is missing" message rather than failing to spawn with an
 * ENOENT nobody can interpret.
 */
export function resolveServiceBinary(): string | null {
  const executable =
    process.platform === 'win32' ? 'stagelink-service.exe' : 'stagelink-service'

  const candidates = isPackaged()
    ? [path.join(packagedServiceDirectory(), executable)]
    : [
        // An override is useful when working on the service itself.
        process.env.STAGELINK_SERVICE_PATH,
        path.join(repoRoot(), 'crates', 'target', 'debug', executable),
        path.join(repoRoot(), 'crates', 'target', 'release', executable),
      ]

  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) return candidate
  }
  return null
}

/** Repository root, used only in development. */
function repoRoot(): string {
  // app.getAppPath() is apps/desktop during `nuxt dev`.
  return path.resolve(app.getAppPath(), '..', '..')
}

/** Where the service keeps its database, secrets and logs. */
export function dataDirectory(): string {
  // The service defaults to %LOCALAPPDATA%\StageLink; pass the same path
  // explicitly so development and packaged builds cannot diverge.
  return path.join(app.getPath('appData'), '..', 'Local', 'StageLink')
}

/** Where Electron's own logs go. */
export function logDirectory(): string {
  return path.join(dataDirectory(), 'logs')
}

/** Paths the renderer may ask to reveal in Explorer. */
export const REVEALABLE_PATHS = {
  logs: logDirectory,
  database: () => path.join(dataDirectory(), 'stagelink.db'),
  userData: dataDirectory,
} as const

export type RevealTarget = keyof typeof REVEALABLE_PATHS
