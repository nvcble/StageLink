import {
  isServiceUsable,
  type BridgeRuntimeInfo,
  type OperationalError,
  type ServiceInfo,
} from '@stagelink/shared-protocol'

/**
 * Native-service supervision state.
 *
 * The single source of truth for "is the backend reachable". Every other store
 * assumes the service may be absent and leans on this one for the answer, so
 * the UI degrades to read-only in one consistent way instead of each panel
 * inventing its own failure mode.
 */
export const useServiceStore = defineStore('service', () => {
  const info = ref<ServiceInfo>({
    state: 'stopped',
    version: null,
    controlProtocolVersion: null,
    audioProtocolVersion: null,
    pid: null,
    startedAt: null,
    restartCount: 0,
    lastError: null,
    availableAudioApis: [],
  })

  const runtime = ref<BridgeRuntimeInfo | null>(null)

  /** Recent faults, newest first. Capped so a long service does not grow it. */
  const errors = ref<OperationalError[]>([])
  const MAX_ERRORS = 50

  const state = computed(() => info.value.state)

  /** True when control requests can be issued. */
  const isUsable = computed(() => isServiceUsable(info.value.state))

  /** True when the service is in the middle of coming up. */
  const isStarting = computed(() =>
    ['starting', 'handshaking', 'restarting'].includes(info.value.state),
  )

  /** True when the supervisor has given up. */
  const hasFailed = computed(() => info.value.state === 'failed')

  /** Operator-facing one-liner for the status bar. */
  const statusLabel = computed(() => {
    switch (info.value.state) {
      case 'stopped':
        return 'Audio service stopped'
      case 'starting':
        return 'Starting audio service…'
      case 'handshaking':
        return 'Connecting to audio service…'
      case 'ready':
        return 'Audio service ready'
      case 'degraded':
        return 'Audio service reporting a fault'
      case 'restarting':
        return `Restarting audio service (attempt ${info.value.restartCount})…`
      case 'failed':
        return 'Audio service unavailable'
    }
  })

  const statusTone = computed<'success' | 'warning' | 'error' | 'neutral'>(() => {
    switch (info.value.state) {
      case 'ready':
        return 'success'
      case 'degraded':
      case 'restarting':
        return 'warning'
      case 'failed':
        return 'error'
      default:
        return 'neutral'
    }
  })

  /** Whether the running service reports a usable ASIO backend. */
  const hasAsio = computed(() => info.value.availableAudioApis.includes('asio'))

  function applyInfo(next: ServiceInfo) {
    info.value = next
  }

  function setRuntime(next: BridgeRuntimeInfo) {
    runtime.value = next
  }

  function appendError(error: OperationalError) {
    errors.value = [error, ...errors.value].slice(0, MAX_ERRORS)
  }

  function clearErrors() {
    errors.value = []
  }

  /** Asks the main process to cycle the service. */
  async function restart() {
    const { bridge } = useStageLinkBridge()
    if (!bridge) return
    info.value = await bridge.restartService()
  }

  return {
    info,
    runtime,
    errors,
    state,
    isUsable,
    isStarting,
    hasFailed,
    statusLabel,
    statusTone,
    hasAsio,
    applyInfo,
    setRuntime,
    appendError,
    clearErrors,
    restart,
  }
})
