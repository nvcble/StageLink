/**
 * Diagnostics subscription lifecycle.
 *
 * Subscribes on mount and unsubscribes on unmount, so the service only does
 * diagnostics work while somebody is looking at it. An idle dashboard, or a
 * minimised window, costs the audio engine nothing.
 */
export function useDiagnostics(options: { autoSubscribe?: boolean } = {}) {
  const diagnostics = useDiagnosticsStore()
  const settings = useSettingsStore()
  const { autoSubscribe = true } = options

  onMounted(async () => {
    diagnostics.setHistoryLimit(settings.settings.diagnostics.chartHistoryPoints)
    // One immediate snapshot so the page is never blank while waiting for the
    // first push.
    await diagnostics.refresh()
    if (autoSubscribe && settings.settings.diagnostics.streamWhileVisible) {
      await diagnostics.subscribe(settings.settings.diagnostics.intervalMs)
    }
  })

  onBeforeUnmount(() => {
    if (diagnostics.isSubscribed) {
      // Fire and forget: the page is going away and the service tolerates the
      // request arriving after the component is gone.
      void diagnostics.unsubscribe()
    }
  })

  /** Formats a byte count as a memory figure. */
  function formatMemory(bytes: number | null): string {
    if (bytes === null) return '—'
    return `${(bytes / (1_024 * 1_024)).toFixed(0)} MB`
  }

  /** Formats a CPU percentage. */
  function formatCpu(percent: number | null): string {
    if (percent === null) return '—'
    return `${percent.toFixed(1)}%`
  }

  /** Formats microseconds for the callback timing readouts. */
  function formatMicros(micros: number | null | undefined): string {
    if (micros === null || micros === undefined) return '—'
    if (micros < 1_000) return `${micros} µs`
    return `${(micros / 1_000).toFixed(2)} ms`
  }

  return {
    diagnostics,
    formatMemory,
    formatCpu,
    formatMicros,
    formatMeasurement,
  }
}
