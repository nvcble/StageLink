import type { EventName, EventPayload } from './events'
import type { MethodName, MethodParams, MethodResult } from './methods'
import type { ServiceError } from './errors'
import type { ServiceInfo } from './service'

/**
 * Argument tuple for `invoke`.
 *
 * Methods whose params schema accepts `undefined` (the no-argument methods,
 * declared as `z.object({}).optional()`) may be called with no second argument.
 * The test is `undefined extends MethodParams<M>` rather than
 * `MethodParams<M> extends undefined`, because an optional object schema infers
 * as `{} | undefined` — which does not extend `undefined`, but which `undefined`
 * does extend.
 */
export type InvokeArgs<M extends MethodName> = undefined extends MethodParams<M>
  ? [params?: MethodParams<M>]
  : [params: MethodParams<M>]

/**
 * The exact surface the preload script exposes on `window.stagelink`.
 *
 * Deliberately narrow: a method-name-keyed `invoke`, a typed event subscription
 * and a handful of desktop affordances. No Node primitives, no `require`, no
 * raw `ipcRenderer`, no arbitrary channel names.
 */
export interface StageLinkBridge {
  /** Build/runtime identification, available synchronously at page load. */
  readonly runtime: BridgeRuntimeInfo

  /**
   * Issue a typed control request. Rejects with a `StageLinkServiceError`-shaped
   * object on failure; never throws synchronously.
   */
  invoke<M extends MethodName>(
    method: M,
    ...params: InvokeArgs<M>
  ): Promise<MethodResult<M>>

  /**
   * Subscribe to a service event. Returns an unsubscribe function; callers must
   * invoke it on scope teardown (the Nuxt plugin wires this to `onScopeDispose`).
   */
  on<E extends EventName>(event: E, listener: (payload: EventPayload<E>) => void): () => void

  /** Current native-service supervision state, without a round trip. */
  getServiceInfo(): Promise<ServiceInfo>

  /** Ask the main process to restart the native service. */
  restartService(): Promise<ServiceInfo>

  /** Desktop window affordances that must not require Node in the renderer. */
  readonly window: {
    minimise(): void
    toggleMaximise(): void
    close(): void
    isMaximised(): Promise<boolean>
  }

  /** Open an external URL in the system browser after main-process validation. */
  openExternal(url: string): Promise<boolean>

  /** Reveal a log or database file in Explorer. Paths come from the main process. */
  revealPath(target: 'logs' | 'database' | 'userData'): Promise<boolean>
}

export interface BridgeRuntimeInfo {
  appVersion: string
  controlProtocolVersion: string
  audioProtocolVersion: number
  electronVersion: string
  chromeVersion: string
  nodeVersion: string
  platform: string
  arch: string
  /** True for a packaged build; false under `nuxt dev`. */
  isPackaged: boolean
  /** Locale-independent build channel. */
  channel: 'development' | 'production'
}

/** Shape of a rejected `invoke`, as it crosses the preload boundary. */
export interface BridgeErrorPayload {
  readonly __stagelinkError: true
  readonly error: ServiceError
}

export function isBridgeErrorPayload(value: unknown): value is BridgeErrorPayload {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { __stagelinkError?: unknown }).__stagelinkError === true
  )
}

/** The single Electron IPC channel names used by the bridge. */
export const BRIDGE_CHANNELS = {
  invoke: 'stagelink:invoke',
  event: 'stagelink:event',
  serviceInfo: 'stagelink:service-info',
  restartService: 'stagelink:service-restart',
  windowCommand: 'stagelink:window-command',
  windowIsMaximised: 'stagelink:window-is-maximised',
  openExternal: 'stagelink:open-external',
  revealPath: 'stagelink:reveal-path',
} as const

export type BridgeChannel = (typeof BRIDGE_CHANNELS)[keyof typeof BRIDGE_CHANNELS]

/** Global augmentation so renderer code gets `window.stagelink` typed. */
declare global {
  interface Window {
    stagelink?: StageLinkBridge
  }
}
