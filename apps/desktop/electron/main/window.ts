import path from 'node:path'
import { BrowserWindow, shell } from 'electron'

import { buildContentSecurityPolicy, collectInlineScriptHashes } from './csp'
import { logger } from './logger'
import { APP_ORIGIN, DEV_SERVER_URL, isDevelopment, rendererRoot } from './paths'

/**
 * The main window, and the security policy that goes with it.
 *
 * Every renderer capability is denied by default and only the few StageLink
 * actually needs are granted. The renderer is a UI: it draws meters and sends
 * control commands. It has no business touching Node, opening windows,
 * navigating off-origin, or reaching the filesystem.
 */

/**
 * The policy, computed once.
 *
 * In production this hashes the inline scripts Nuxt generated, so the policy is
 * derived from the files actually on disk. Doing it once at first use rather
 * than per request keeps it off the header hot path.
 */
let cachedPolicy: string | null = null

function contentSecurityPolicy(): string {
  if (cachedPolicy !== null) return cachedPolicy

  const development = isDevelopment()
  const inlineScriptHashes = development ? [] : collectInlineScriptHashes(rendererRoot())

  if (!development) {
    logger.info(`content security policy pins ${inlineScriptHashes.length} inline script hash(es)`)
    if (inlineScriptHashes.length === 0) {
      // Without hashes the renderer's bootstrap script is blocked and the
      // window renders blank. Loud, because it is invisible otherwise.
      logger.error(
        'no inline script hashes were found in the renderer build; the window will render blank. ' +
          'Check that .output/public was bundled into resources/renderer.',
      )
    }
  }

  cachedPolicy = buildContentSecurityPolicy({ development, inlineScriptHashes })
  return cachedPolicy
}

export function createMainWindow(preloadPath: string): BrowserWindow {
  const window = new BrowserWindow({
    width: 1_360,
    height: 900,
    minWidth: 1_040,
    minHeight: 680,
    show: false,
    backgroundColor: '#080B16',
    title: 'StageLink Core',
    autoHideMenuBar: true,
    webPreferences: {
      preload: preloadPath,
      // The renderer gets no Node, no remote module and no access to the
      // Electron API beyond what the preload explicitly exposes.
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      // Sandboxing the renderer is compatible with a contextBridge-only
      // preload, which is all StageLink uses.
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      experimentalFeatures: false,
      // Nothing in StageLink uses WebGL, plugins or spellcheck.
      webgl: false,
      plugins: false,
      spellcheck: false,
      devTools: isDevelopment(),
    },
  })

  applySecurityPolicy(window)

  // Show only once the first paint is ready, so the operator never sees a white
  // flash against the dark theme.
  window.once('ready-to-show', () => window.show())

  return window
}

/** Applies navigation, permission and CSP restrictions to a window. */
export function applySecurityPolicy(window: BrowserWindow): void {
  const { webContents } = window

  // Inject the CSP as a response header rather than a meta tag: a header
  // cannot be removed by anything running in the page.
  webContents.session.webRequest.onHeadersReceived((details, callback) => {
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [contentSecurityPolicy()],
        'X-Content-Type-Options': ['nosniff'],
      },
    })
  })

  /**
   * Deny every permission. StageLink's audio capture happens in the Rust
   * service, so the renderer never needs a microphone, a camera, a
   * notification, clipboard access or a geolocation fix.
   *
   * `openExternal` is the sole exception, and it is not really an exception:
   * Electron raises it for `shell.openExternal` calls that originate in the
   * main process, which in StageLink means `openExternalUrl` — where the URL
   * has already been checked against an https/mailto allowlist. Denying it
   * would break the one button that uses it while protecting nothing.
   */
  const ALLOWED_PERMISSIONS = new Set(['openExternal'])

  webContents.session.setPermissionRequestHandler((_contents, permission, callback) => {
    if (ALLOWED_PERMISSIONS.has(permission)) {
      callback(true)
      return
    }
    logger.warn(`denied a renderer permission request: ${permission}`)
    callback(false)
  })
  webContents.session.setPermissionCheckHandler((_contents, permission) =>
    ALLOWED_PERMISSIONS.has(permission),
  )

  // Refuse to load any code from outside the application.
  webContents.session.webRequest.onBeforeRequest((details, callback) => {
    if (isPermittedUrl(details.url)) {
      callback({ cancel: false })
      return
    }
    logger.warn(`blocked a renderer request to ${details.url}`)
    callback({ cancel: true })
  })

  // In-app navigation stays in-app; anything else goes to the system browser
  // after validation, so a compromised renderer cannot open a window it
  // controls with node privileges.
  webContents.on('will-navigate', (event, url) => {
    if (!isPermittedUrl(url)) {
      event.preventDefault()
      logger.warn(`blocked navigation to ${url}`)
    }
  })

  webContents.setWindowOpenHandler(({ url }) => {
    void openExternalUrl(url)
    return { action: 'deny' }
  })

  // A renderer should never attach a webview; if one appears, strip its
  // privileges and refuse it.
  webContents.on('will-attach-webview', (event, webPreferences) => {
    delete webPreferences.preload
    webPreferences.nodeIntegration = false
    webPreferences.contextIsolation = true
    event.preventDefault()
    logger.warn('blocked an attempt to attach a webview')
  })
}

/** Whether a URL may be loaded inside the application. */
export function isPermittedUrl(url: string): boolean {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return false
  }

  if (parsed.protocol === 'stagelink:') return true
  if (parsed.protocol === 'devtools:') return isDevelopment()

  if (isDevelopment()) {
    const dev = new URL(DEV_SERVER_URL)
    if (
      (parsed.protocol === 'http:' || parsed.protocol === 'ws:') &&
      parsed.hostname === dev.hostname &&
      parsed.port === dev.port
    ) {
      return true
    }
  }

  return false
}

/**
 * Opens a URL in the system browser, after checking it is something we are
 * willing to hand over.
 *
 * Only `https:` and `mailto:` are allowed. A `file:` or `smb:` URL from the
 * renderer would be a way to launch arbitrary local content.
 */
export async function openExternalUrl(url: string): Promise<boolean> {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    logger.warn(`refused to open an unparseable URL: ${url}`)
    return false
  }
  if (parsed.protocol !== 'https:' && parsed.protocol !== 'mailto:') {
    logger.warn(`refused to open a ${parsed.protocol} URL externally`)
    return false
  }
  await shell.openExternal(parsed.toString())
  return true
}

/** Loads the renderer: the dev server in development, the bundle in production. */
export async function loadRenderer(window: BrowserWindow): Promise<void> {
  if (isDevelopment()) {
    logger.info(`loading the renderer from ${DEV_SERVER_URL}`)
    await window.loadURL(DEV_SERVER_URL)
    window.webContents.openDevTools({ mode: 'detach' })
    return
  }
  logger.info(`loading the renderer from ${APP_ORIGIN}`)
  await window.loadURL(`${APP_ORIGIN}/`)
}

/** Resolves the preload script path for the current build layout. */
export function preloadPath(appPath: string): string {
  return path.join(appPath, 'dist-electron', 'preload', 'index.cjs')
}
