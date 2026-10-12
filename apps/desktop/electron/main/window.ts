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

/** How long to give the app to render before treating the window as blank. */
const FIRST_PAINT_MS = 10_000
/** How long to give it after a recovery reload, which compiles nothing new. */
const RELOAD_PAINT_MS = 20_000

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

/**
 * Whether the Vue app actually appeared in the DOM.
 *
 * Asked rather than awaited as an event, because there is no event for this: a
 * failed dynamic import emits no `did-fail-load` and throws nothing the main
 * process can see. The only reliable signal is the thing that matters — is
 * there a rendered app?
 */
async function hasRendered(window: BrowserWindow): Promise<boolean> {
  if (window.isDestroyed()) return false
  const rendered = await window.webContents
    .executeJavaScript(`!!document.getElementById('__nuxt')?.childElementCount`)
    .catch(() => false)
  return rendered === true
}

/**
 * What to put on screen when the interface will not come up.
 *
 * Written into the empty document rather than loaded as a page, so it cannot
 * itself fail on a navigation rule or the content security policy. Unstyled on
 * purpose: the stylesheet is one of the things that may not have loaded.
 */
const MOUNT_FAILURE_NOTICE = [
  '<h2>StageLink Core could not start its interface.</h2>',
  '<p>The window loaded but the application never rendered. Nothing is streaming.</p>',
  '<p>If this is a development build, the Nuxt dev server may still have been',
  'compiling &mdash; close this window and reopen it.</p>',
  '<p>The reason, if there was one, is in the log in the StageLink data folder.</p>',
].join(' ')

/**
 * Recovers a window that loaded but never rendered anything.
 *
 * `loadURL` resolving means the HTML arrived — not that anything rendered, and
 * the gap between those two is where every blank-window bug in this project has
 * lived: a dev server answering `/` with the shell before Vite has compiled the
 * client module graph, a content security policy blocking Nuxt's bootstrap
 * script, a protocol handler that was never registered. All of them present
 * identically, as black glass with nothing in any log.
 *
 * **Never awaited by startup.** What follows the load — spawning the audio
 * service and the store load that depends on it — is timed against the page
 * having loaded, not having painted, and making it wait on a watchdog delays
 * the service handshake enough to fail it. This runs alongside instead.
 *
 * Stands down if anything navigated in the meantime: the operator reloading, or
 * the renderer navigating itself, means something is alive and this must not
 * reload a page out from under it.
 */
async function recoverIfBlank(window: BrowserWindow): Promise<void> {
  let navigated = false
  const onNavigate = () => {
    navigated = true
  }
  window.webContents.on('did-start-navigation', onNavigate)

  await delay(FIRST_PAINT_MS)
  if (navigated || (await hasRendered(window))) {
    if (!window.isDestroyed()) window.webContents.off('did-start-navigation', onNavigate)
    return
  }

  logger.warn('the renderer loaded but never rendered; reloading once')
  window.webContents.off('did-start-navigation', onNavigate)
  window.webContents.reload()

  await delay(RELOAD_PAINT_MS)
  if (await hasRendered(window)) return

  // Say so rather than leaving an operator looking at a black window and
  // guessing, which is how a service starts ten minutes late.
  logger.error('the renderer never rendered')
  await window.webContents
    .executeJavaScript(`document.body.innerHTML = ${JSON.stringify(MOUNT_FAILURE_NOTICE)}`)
    .catch(() => undefined)
}

/** Loads the renderer: the dev server in development, the bundle in production. */
export async function loadRenderer(window: BrowserWindow): Promise<void> {
  const url = isDevelopment() ? DEV_SERVER_URL : `${APP_ORIGIN}/`
  logger.info(`loading the renderer from ${url}`)
  await window.loadURL(url)
  if (isDevelopment()) window.webContents.openDevTools({ mode: 'detach' })
  void recoverIfBlank(window)
}

/** Resolves the preload script path for the current build layout. */
export function preloadPath(appPath: string): string {
  return path.join(appPath, 'dist-electron', 'preload', 'index.cjs')
}
