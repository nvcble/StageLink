import { existsSync } from 'node:fs'
import path from 'node:path'
import { BrowserWindow, app, dialog } from 'electron'

import { forwardServiceEvents, registerIpcHandlers, removeIpcHandlers } from '../ipc/register'
import { initLogging, logger } from './logger'
import { isDevelopment, isPackaged } from './paths'
import { registerProtocolHandler, registerProtocolScheme } from './protocol'
import { ServiceSupervisor } from './service-supervisor'
import { createTray, destroyTray } from './tray'
import { createMainWindow, loadRenderer, preloadPath } from './window'

/**
 * StageLink Core main process.
 *
 * Owns the desktop lifecycle: the window, the tray, the custom protocol and the
 * Rust service. It does not touch audio and it does not render anything.
 *
 * ## Shutdown policy
 *
 * The audio service is a child of this process and stops with it. A monitor
 * feed running with no UI attached is a hazard — nobody can see clipping and
 * nobody can stop it — so there is no "keep streaming in the background after
 * quit" mode. Quitting is therefore gated: if a session is live, the operator
 * is asked to confirm, and then the service is given time to notify receivers
 * and release the capture device before the process exits.
 *
 * Reloading the renderer is entirely separate and never stops audio.
 */

// Must happen before `app.whenReady()`.
registerProtocolScheme()

const supervisor = new ServiceSupervisor()
let mainWindow: BrowserWindow | null = null
let quitConfirmed = false

/** A second instance would fight over the audio device and the database. */
if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', () => {
    if (!mainWindow) return
    if (mainWindow.isMinimized()) mainWindow.restore()
    mainWindow.show()
    mainWindow.focus()
  })

  void bootstrap()
}

async function bootstrap(): Promise<void> {
  initLogging(isDevelopment() ? 'debug' : 'info')

  // Surface anything unexpected in the log rather than letting Electron show a
  // bare dialog with no context.
  process.on('uncaughtException', (error) => {
    logger.error(`uncaught exception in the main process: ${error.stack ?? error.message}`)
  })
  process.on('unhandledRejection', (reason) => {
    logger.error(`unhandled rejection in the main process: ${String(reason)}`)
  })

  // Available to the sandboxed preload, which cannot read `app`.
  process.env.STAGELINK_PACKAGED = isPackaged() ? '1' : '0'
  process.env.STAGELINK_APP_VERSION = app.getVersion()

  await app.whenReady()

  // Production serves the renderer over stagelink://; development loads it from
  // the Nuxt dev server, which needs no handler.
  if (!isDevelopment()) {
    registerProtocolHandler()
  }

  registerIpcHandlers(supervisor)
  forwardServiceEvents(supervisor, () => mainWindow)

  await createWindow()

  createTray(
    () => mainWindow,
    () => {
      quitConfirmed = true
      app.quit()
    },
  )

  // Start the service after the window exists, so its first state events have
  // somewhere to go and a failure is visible immediately rather than silently.
  const info = await supervisor.start()
  if (info.state === 'failed') {
    logger.error('the audio service could not be started')
    // The UI shows this too, but a dialog is warranted: without the service
    // there is no application, and an operator setting up before a service
    // needs to know at once.
    dialog.showErrorBox(
      'StageLink audio service unavailable',
      `${info.lastError?.message ?? 'The audio service could not be started.'}\n\n${
        info.lastError?.detail ?? ''
      }`.trim(),
    )
  }

  app.on('window-all-closed', () => {
    // Windows convention: closing the last window quits. The service must not
    // be left running without a UI.
    if (process.platform !== 'darwin') app.quit()
  })

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) void createWindow()
  })

  app.on('before-quit', (event) => {
    if (quitConfirmed) return
    // Give the service a chance to stop cleanly. `before-quit` cannot await, so
    // the quit is cancelled, the shutdown runs, and then quit is re-issued.
    event.preventDefault()
    void shutdown()
  })
}

async function createWindow(): Promise<void> {
  const preload = preloadPath(app.getAppPath())
  if (!existsSync(preload)) {
    // Almost always a missing build step; say which one.
    const message = `The preload bundle is missing at ${preload}.`
    logger.error(message)
    dialog.showErrorBox(
      'StageLink Core cannot start',
      `${message}\n\nRun "npm run build:electron" in apps/desktop.`,
    )
    app.exit(1)
    return
  }

  mainWindow = createMainWindow(preload)
  mainWindow.on('closed', () => {
    mainWindow = null
  })

  // A renderer reload drops the event listeners but changes nothing about the
  // service: the audio keeps flowing and the new page resynchronises from the
  // snapshot the service pushes on reconnect.
  mainWindow.webContents.on('did-finish-load', () => {
    logger.debug('renderer loaded')
  })
  mainWindow.webContents.on('render-process-gone', (_event, details) => {
    logger.error(`the renderer process went away: ${details.reason}`)
  })

  await loadRenderer(mainWindow)
}

/** Orderly shutdown: stop the service, then let the quit proceed. */
async function shutdown(): Promise<void> {
  if (quitConfirmed) return
  quitConfirmed = true
  logger.info('shutting down')

  try {
    await supervisor.stop('app-quit')
  } catch (error) {
    logger.error(`the audio service did not shut down cleanly: ${String(error)}`)
  }

  removeIpcHandlers()
  destroyTray()
  app.quit()
}

export { supervisor }
