import type { ServiceError, StreamStatus } from '@stagelink/shared-protocol'

/**
 * Streaming state.
 *
 * The counters here are desktop-measured: packets we handed to the socket,
 * bytes we wrote, sends the OS refused. None of it says anything about what a
 * receiver actually heard — that lives in the receiver store and is labelled
 * as receiver-reported.
 */
export const useStreamingStore = defineStore('streaming', () => {
  const status = ref<StreamStatus | null>(null)
  const isStarting = ref(false)
  const isStopping = ref(false)
  const lastError = ref<ServiceError | null>(null)

  const state = computed(() => status.value?.state ?? 'stopped')
  const isStreaming = computed(() => state.value === 'running')
  const isBusy = computed(() => isStarting.value || isStopping.value)

  /** Guard against a double-press on Start. */
  const canStart = computed(() => !isBusy.value && state.value === 'stopped')
  const canStop = computed(() => !isBusy.value && state.value === 'running')

  const packetsSent = computed(() => status.value?.packetsSent ?? 0)
  const bytesSent = computed(() => status.value?.bytesSent ?? 0)
  const sendFailures = computed(() => status.value?.sendFailures ?? 0)
  const packetsDropped = computed(() => status.value?.packetsDropped ?? 0)
  const activeReceivers = computed(() => status.value?.activeReceivers ?? 0)
  const queueDepth = computed(() => status.value?.queueDepth ?? 0)

  /** True when the configured transport is the encrypted one. */
  const isEncrypted = computed(() => status.value?.config?.transport === 'udp-encrypted')

  function applyStatus(next: StreamStatus) {
    status.value = next
    isStarting.value = next.state === 'starting'
    isStopping.value = next.state === 'stopping'
  }

  async function refreshStatus() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('stream.getStatus')
    if (result.ok) {
      applyStatus(result.value)
      lastError.value = null
    } else {
      lastError.value = result.error
    }
  }

  async function start(sessionLabel?: string) {
    if (!canStart.value) return false
    const { tryInvoke } = useStageLinkBridge()
    isStarting.value = true
    try {
      const result = await tryInvoke('stream.start', {
        ...(sessionLabel ? { sessionLabel } : {}),
      })
      if (result.ok) {
        applyStatus(result.value)
        lastError.value = null
        return true
      }
      lastError.value = result.error
      return false
    } finally {
      isStarting.value = false
    }
  }

  /**
   * Stops the stream, draining queued packets by default so receivers hear the
   * tail of the mix rather than a hard cut.
   */
  async function stop(immediate = false) {
    if (!canStop.value) return false
    const { tryInvoke } = useStageLinkBridge()
    isStopping.value = true
    try {
      const result = await tryInvoke('stream.stop', { immediate })
      if (result.ok) {
        applyStatus(result.value)
        lastError.value = null
        return true
      }
      lastError.value = result.error
      return false
    } finally {
      isStopping.value = false
    }
  }

  function clearError() {
    lastError.value = null
  }

  return {
    status,
    isStarting,
    isStopping,
    lastError,
    state,
    isStreaming,
    isBusy,
    canStart,
    canStop,
    packetsSent,
    bytesSent,
    sendFailures,
    packetsDropped,
    activeReceivers,
    queueDepth,
    isEncrypted,
    applyStatus,
    refreshStatus,
    start,
    stop,
    clearError,
  }
})
