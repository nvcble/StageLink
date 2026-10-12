import {
  StageLinkServiceError,
  type InvokeArgs,
  type MethodName,
  type MethodResult,
  type ServiceError,
  type StageLinkBridge,
} from '@stagelink/shared-protocol'

/**
 * Access to the typed Electron bridge.
 *
 * Everything in the renderer goes through here rather than touching
 * `window.stagelink` directly, which gives one place to handle the
 * "not running inside Electron" case and one place that knows how to turn a
 * rejection into something a component can show.
 */
export function useStageLinkBridge() {
  const { $stagelink } = useNuxtApp()
  const bridge = $stagelink as StageLinkBridge | null

  /** True when the desktop bridge is present. */
  const isAvailable = computed(() => bridge !== null)

  /**
   * Calls a control method.
   *
   * Rejects with a `StageLinkServiceError` carrying the structured code, so a
   * caller can branch on `AUDIO_DEVICE_NOT_FOUND` rather than parsing a string.
   */
  async function invoke<M extends MethodName>(
    method: M,
    ...params: InvokeArgs<M>
  ): Promise<MethodResult<M>> {
    if (!bridge) {
      throw new StageLinkServiceError({
        code: 'SERVICE_UNAVAILABLE',
        message: 'StageLink is not running inside the desktop application.',
        retryable: false,
      })
    }
    return bridge.invoke(method, ...params)
  }

  /**
   * Calls a control method and returns `null` instead of throwing.
   *
   * For the many places where a failed refresh should leave the previous value
   * on screen and surface an error banner, rather than blanking a panel in the
   * middle of a service.
   */
  async function tryInvoke<M extends MethodName>(
    method: M,
    ...params: InvokeArgs<M>
  ): Promise<{ ok: true; value: MethodResult<M> } | { ok: false; error: ServiceError }> {
    try {
      return { ok: true, value: await invoke(method, ...params) }
    } catch (cause) {
      return { ok: false, error: describeError(cause) }
    }
  }

  return {
    bridge,
    isAvailable,
    runtime: bridge?.runtime ?? null,
    invoke,
    tryInvoke,
  }
}

/** Normalises anything thrown by `invoke` into a `ServiceError`. */
export function describeError(cause: unknown): ServiceError {
  if (cause instanceof StageLinkServiceError) {
    return {
      code: cause.code,
      message: cause.message,
      detail: cause.detail,
      retryable: cause.retryable,
      field: cause.field,
    }
  }
  if (cause instanceof Error) {
    return { code: 'INTERNAL', message: cause.message, retryable: false }
  }
  return { code: 'INTERNAL', message: String(cause), retryable: false }
}
