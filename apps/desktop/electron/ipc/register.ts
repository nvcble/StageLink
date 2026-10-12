import { BrowserWindow, ipcMain, shell } from 'electron'

import {
  BRIDGE_CHANNELS,
  EVENTS,
  ServiceErrorCode,
  isEventName,
  serviceError,
  toServiceError,
  type ServiceError,
} from '@stagelink/shared-protocol'

import { logger } from '../main/logger'
import { REVEALABLE_PATHS, type RevealTarget } from '../main/paths'
import type { ServiceSupervisor } from '../main/service-supervisor'
import { openExternalUrl } from '../main/window'

/**
 * The renderer-facing IPC surface.
 *
 * Eight channels, all of them named and fixed. The renderer cannot invent a
 * channel, cannot pass a channel name as data, and cannot reach anything in the
 * main process that is not handled here.
 *
 * Errors are returned as a tagged value rather than thrown. An exception thrown
 * out of an `ipcMain.handle` arrives in the renderer as a string with the stack
 * mangled into the message, which loses the structured error code the UI needs
 * to react to.
 */

interface BridgeErrorEnvelope {
  readonly __stagelinkError: true
  readonly error: ServiceError
}

function errorEnvelope(error: ServiceError): BridgeErrorEnvelope {
  return { __stagelinkError: true, error }
}

/** Rejects a payload that is not a plain object or undefined. */
function sanitiseParams(value: unknown): unknown {
  if (value === undefined || value === null) return {}
  if (typeof value !== 'object' || Array.isArray(value)) {
    throw serviceError(ServiceErrorCode.InvalidRequest, 'Parameters must be an object.')
  }
  return value
}

export function registerIpcHandlers(supervisor: ServiceSupervisor): void {
  // -- control requests ----------------------------------------------------
  ipcMain.handle(BRIDGE_CHANNELS.invoke, async (_event, method: unknown, params: unknown) => {
    try {
      if (typeof method !== 'string') {
        throw serviceError(ServiceErrorCode.InvalidRequest, 'The method name must be a string.')
      }
      // The supervisor validates `method` against the registry and `params`
      // against that method's schema, so nothing further is needed here.
      return await supervisor.invoke(method, sanitiseParams(params))
    } catch (error) {
      return errorEnvelope(toServiceError(error))
    }
  })

  // -- supervision ---------------------------------------------------------
  ipcMain.handle(BRIDGE_CHANNELS.serviceInfo, () => supervisor.info)

  ipcMain.handle(BRIDGE_CHANNELS.restartService, async () => {
    try {
      return await supervisor.restart()
    } catch (error) {
      return errorEnvelope(toServiceError(error))
    }
  })

  // -- window controls -----------------------------------------------------
  ipcMain.on(BRIDGE_CHANNELS.windowCommand, (event, command: unknown) => {
    const window = BrowserWindow.fromWebContents(event.sender)
    if (!window) return
    switch (command) {
      case 'minimise':
        window.minimize()
        return
      case 'toggleMaximise':
        if (window.isMaximized()) window.unmaximize()
        else window.maximize()
        return
      case 'close':
        window.close()
        return
      default:
        logger.warn(`ignoring an unknown window command: ${String(command)}`)
    }
  })

  ipcMain.handle(BRIDGE_CHANNELS.windowIsMaximised, (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isMaximized() ?? false
  })

  // -- shell integration ---------------------------------------------------
  ipcMain.handle(BRIDGE_CHANNELS.openExternal, async (_event, url: unknown) => {
    if (typeof url !== 'string') return false
    return openExternalUrl(url)
  })

  ipcMain.handle(BRIDGE_CHANNELS.revealPath, async (_event, target: unknown) => {
    // Only the three named locations, resolved in the main process. The
    // renderer never supplies a path.
    if (typeof target !== 'string' || !(target in REVEALABLE_PATHS)) {
      logger.warn(`refused to reveal an unknown target: ${String(target)}`)
      return false
    }
    const resolved = REVEALABLE_PATHS[target as RevealTarget]()
    const error = await shell.openPath(resolved)
    if (error) {
      logger.warn(`could not reveal ${resolved}: ${error}`)
      return false
    }
    return true
  })
}

/**
 * Forwards validated service events to a window.
 *
 * Payloads are checked against the event registry here rather than in the
 * renderer: a store should never have to defend itself against a shape the
 * protocol does not allow, and a validation failure is a protocol bug worth
 * logging in the main process where the log lives.
 */
export function forwardServiceEvents(
  supervisor: ServiceSupervisor,
  getWindow: () => BrowserWindow | null,
): void {
  supervisor.on('service-event', (event: string, payload: unknown) => {
    if (!isEventName(event)) {
      logger.warn(`dropping an unknown service event: ${event}`)
      return
    }

    const parsed = EVENTS[event].safeParse(payload)
    if (!parsed.success) {
      logger.warn(
        `dropping a malformed ${event} payload: ${parsed.error.issues
          .map((issue) => `${issue.path.join('.')}: ${issue.message}`)
          .join('; ')}`,
      )
      return
    }

    const window = getWindow()
    // No window (closed to tray, or mid-reload) is normal, not an error. The
    // service keeps running and the renderer resynchronises on reconnect.
    if (!window || window.isDestroyed()) return
    window.webContents.send(BRIDGE_CHANNELS.event, event, parsed.data)
  })
}

/** Removes every handler, so a reload cannot register them twice. */
export function removeIpcHandlers(): void {
  for (const channel of Object.values(BRIDGE_CHANNELS)) {
    ipcMain.removeHandler(channel)
    ipcMain.removeAllListeners(channel)
  }
}
