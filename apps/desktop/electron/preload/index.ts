import { contextBridge, ipcRenderer } from 'electron'

import {
  AUDIO_PROTOCOL_VERSION,
  BRIDGE_CHANNELS,
  CONTROL_PROTOCOL_VERSION,
  isBridgeErrorPayload,
  StageLinkServiceError,
  type BridgeRuntimeInfo,
  type EventName,
  type StageLinkBridge,
} from '@stagelink/shared-protocol'

/**
 * The preload bridge.
 *
 * The entire surface the renderer gets is the object below. Note what is *not*
 * exposed: no `ipcRenderer`, no `require`, no `process`, no channel names, no
 * generic `send`. A component cannot reach a main-process capability that is
 * not a named method here, which means the attack surface is a list you can
 * read in one screen rather than "all of Electron".
 *
 * This file runs in a sandboxed context, so it is CommonJS and must not import
 * anything that needs Node at runtime. `@stagelink/shared-protocol` is bundled
 * into it at build time.
 */

const runtime: BridgeRuntimeInfo = {
  appVersion: process.env.STAGELINK_APP_VERSION ?? '0.1.0',
  controlProtocolVersion: CONTROL_PROTOCOL_VERSION,
  audioProtocolVersion: AUDIO_PROTOCOL_VERSION,
  electronVersion: process.versions.electron ?? 'unknown',
  chromeVersion: process.versions.chrome ?? 'unknown',
  nodeVersion: process.versions.node ?? 'unknown',
  platform: process.platform,
  arch: process.arch,
  isPackaged: process.env.STAGELINK_PACKAGED === '1',
  channel: process.env.STAGELINK_PACKAGED === '1' ? 'production' : 'development',
}

/** Listeners registered per event name, so one channel serves all events. */
const listeners = new Map<EventName, Set<(payload: unknown) => void>>()

ipcRenderer.on(BRIDGE_CHANNELS.event, (_event, name: EventName, payload: unknown) => {
  const registered = listeners.get(name)
  if (!registered) return
  // Iterate a copy: a listener that unsubscribes itself must not disturb the
  // iteration, which would silently skip the next subscriber.
  for (const listener of [...registered]) {
    try {
      listener(payload)
    } catch (error) {
      // A throwing component listener must not stop the others from running,
      // and must not kill the event channel for the rest of the session.
      console.error(`[stagelink] a ${name} listener threw:`, error)
    }
  }
})

const bridge: StageLinkBridge = {
  runtime,

  async invoke(method, ...args) {
    const result = await ipcRenderer.invoke(BRIDGE_CHANNELS.invoke, method, args[0])
    // Errors cross the boundary as a tagged value and are rethrown here, so
    // callers get a real Error with the structured code intact.
    if (isBridgeErrorPayload(result)) {
      throw new StageLinkServiceError(result.error)
    }
    return result as never
  },

  on(event, listener) {
    const registered = listeners.get(event) ?? new Set()
    const wrapped = listener as (payload: unknown) => void
    registered.add(wrapped)
    listeners.set(event, registered)

    return () => {
      const current = listeners.get(event)
      if (!current) return
      current.delete(wrapped)
      if (current.size === 0) listeners.delete(event)
    }
  },

  async getServiceInfo() {
    return ipcRenderer.invoke(BRIDGE_CHANNELS.serviceInfo)
  },

  async restartService() {
    const result = await ipcRenderer.invoke(BRIDGE_CHANNELS.restartService)
    if (isBridgeErrorPayload(result)) {
      throw new StageLinkServiceError(result.error)
    }
    return result
  },

  window: {
    minimise() {
      ipcRenderer.send(BRIDGE_CHANNELS.windowCommand, 'minimise')
    },
    toggleMaximise() {
      ipcRenderer.send(BRIDGE_CHANNELS.windowCommand, 'toggleMaximise')
    },
    close() {
      ipcRenderer.send(BRIDGE_CHANNELS.windowCommand, 'close')
    },
    async isMaximised() {
      return ipcRenderer.invoke(BRIDGE_CHANNELS.windowIsMaximised)
    },
  },

  async openExternal(url: string) {
    return ipcRenderer.invoke(BRIDGE_CHANNELS.openExternal, url)
  },

  async revealPath(target) {
    return ipcRenderer.invoke(BRIDGE_CHANNELS.revealPath, target)
  },
}

contextBridge.exposeInMainWorld('stagelink', bridge)
