import type { DiagnosticsSnapshot, Measurement, ServiceError } from '@stagelink/shared-protocol'

/** One point in a charted series. */
export interface SeriesPoint {
  at: number
  value: number | null
}

/**
 * Diagnostics.
 *
 * Holds the latest snapshot plus a short rolling history for the charts. The
 * history is capped and kept in the renderer; nothing is charted by polling the
 * service faster than it pushes, and the subscription is only active while the
 * Diagnostics page is mounted — an idle dashboard must not cost the audio
 * engine anything.
 *
 * Provenance is preserved end to end. `Measurement` carries its own `source`,
 * and the UI renders that badge rather than the store flattening it away.
 */
export const useDiagnosticsStore = defineStore('diagnostics', () => {
  const snapshot = ref<DiagnosticsSnapshot | null>(null)
  const isSubscribed = ref(false)
  const lastError = ref<ServiceError | null>(null)

  const historyLimit = ref(300)
  const cpuHistory = ref<SeriesPoint[]>([])
  const egressHistory = ref<SeriesPoint[]>([])
  const xrunHistory = ref<SeriesPoint[]>([])

  const audio = computed(() => snapshot.value?.audio ?? null)
  const stream = computed(() => snapshot.value?.stream ?? null)
  const receivers = computed(() => snapshot.value?.receivers ?? null)
  const system = computed(() => snapshot.value?.system ?? null)
  const latency = computed(() => snapshot.value?.latency ?? null)
  const recentErrors = computed(() => snapshot.value?.recentErrors ?? [])
  const uptimeMs = computed(() => snapshot.value?.serviceUptimeMs ?? 0)

  /** True when the capture thread got the real-time priority it asked for. */
  const realtimeGranted = computed(() => system.value?.realtimePriorityGranted ?? false)

  /** Figures that indicate a problem worth the operator's attention. */
  const healthWarnings = computed(() => {
    const warnings: string[] = []
    const audioDiagnostics = audio.value
    if (audioDiagnostics) {
      if (audioDiagnostics.xruns > 0) {
        warnings.push(`${audioDiagnostics.xruns} audio glitch(es) since capture started`)
      }
      if (audioDiagnostics.bufferOverruns > 0) {
        warnings.push(
          `${audioDiagnostics.bufferOverruns} capture buffer overrun(s): the packetiser fell behind`,
        )
      }
      if (audioDiagnostics.clipCount > 0) {
        warnings.push(`${audioDiagnostics.clipCount} clipped sample(s): reduce the monitor send`)
      }
    }
    if (system.value && !system.value.realtimePriorityGranted && audioDiagnostics?.captureState === 'running') {
      warnings.push(
        'The capture thread did not get Pro Audio priority; glitches are more likely under load',
      )
    }
    const streamDiagnostics = stream.value
    if (streamDiagnostics) {
      if (streamDiagnostics.sendFailures > 0) {
        warnings.push(`${streamDiagnostics.sendFailures} failed packet send(s)`)
      }
      if (streamDiagnostics.packetsDropped > 0) {
        warnings.push(
          `${streamDiagnostics.packetsDropped} packet(s) dropped from a full receiver queue`,
        )
      }
    }
    return warnings
  })

  function applySnapshot(next: DiagnosticsSnapshot) {
    snapshot.value = next
    push(cpuHistory, next.at, next.system.processCpuPercent)
    push(egressHistory, next.at, next.stream.egressBitsPerSecond)
    push(xrunHistory, next.at, next.audio.xruns)
  }

  function push(series: Ref<SeriesPoint[]>, at: number, value: number | null) {
    series.value = [...series.value, { at, value }].slice(-historyLimit.value)
  }

  async function refresh() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('diagnostics.getSnapshot')
    if (result.ok) {
      applySnapshot(result.value)
      lastError.value = null
    } else {
      lastError.value = result.error
    }
  }

  /** Turns the service's periodic pushes on. */
  async function subscribe(intervalMs = 500) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('diagnostics.subscribe', { enabled: true, intervalMs })
    if (result.ok) {
      isSubscribed.value = true
      lastError.value = null
      return true
    }
    lastError.value = result.error
    return false
  }

  /** Turns them off again. Called when the page unmounts. */
  async function unsubscribe() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('diagnostics.subscribe', { enabled: false, intervalMs: 500 })
    isSubscribed.value = false
    if (!result.ok) lastError.value = result.error
    return result.ok
  }

  function setHistoryLimit(limit: number) {
    historyLimit.value = Math.max(30, Math.min(3_600, limit))
  }

  function clearHistory() {
    cpuHistory.value = []
    egressHistory.value = []
    xrunHistory.value = []
  }

  function clearError() {
    lastError.value = null
  }

  return {
    snapshot,
    isSubscribed,
    lastError,
    historyLimit,
    cpuHistory,
    egressHistory,
    xrunHistory,
    audio,
    stream,
    receivers,
    system,
    latency,
    recentErrors,
    uptimeMs,
    realtimeGranted,
    healthWarnings,
    applySnapshot,
    refresh,
    subscribe,
    unsubscribe,
    setHistoryLimit,
    clearHistory,
    clearError,
  }
})

/** Formats a `Measurement` for display, with "—" for an unknown value. */
export function formatMeasurement(measurement: Measurement | null, digits = 1): string {
  if (!measurement || measurement.value === null) return '—'
  return `${measurement.value.toFixed(digits)} ${measurement.unit}`
}
