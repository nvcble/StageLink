/**
 * Streaming controls.
 *
 * Centralises the question "may streaming start right now, and if not, why
 * not". A disabled Start button with no explanation is the worst possible
 * experience ten minutes before a service, so every blocker produces a reason.
 */
export function useStreaming() {
  const streaming = useStreamingStore()
  const audio = useAudioStore()
  const network = useNetworkStore()
  const service = useServiceStore()
  const session = useSessionStore()

  /** Every reason streaming cannot start, in the order worth fixing them. */
  const blockers = computed<string[]>(() => {
    const reasons: string[] = []
    if (!service.isUsable) {
      reasons.push('The audio service is not running.')
    }
    if (!audio.selectedDevice) {
      reasons.push('No audio input has been selected.')
    } else if (!audio.isCapturing) {
      reasons.push('The audio input is not open.')
    } else if (audio.deviceLost) {
      reasons.push('The audio interface has been disconnected.')
    }
    if (!network.isConfigured) {
      reasons.push('No network adapter has been chosen for streaming.')
    }
    if (network.hasBlockingWarning) {
      reasons.push('The selected network adapter cannot carry a stream.')
    }
    return reasons
  })

  const canStart = computed(() => streaming.canStart && blockers.value.length === 0)
  const canStop = computed(() => streaming.canStop)

  /** The single most important reason, for a tooltip. */
  const primaryBlocker = computed(() => blockers.value[0] ?? null)

  async function start(sessionLabel?: string) {
    if (!canStart.value) return false
    const started = await streaming.start(sessionLabel)
    if (started) await session.refreshCurrent()
    return started
  }

  async function stop(immediate = false) {
    if (!canStop.value) return false
    const stopped = await streaming.stop(immediate)
    if (stopped) await session.refreshCurrent()
    return stopped
  }

  /** Human-readable egress rate. */
  const bandwidthLabel = computed(() => formatBitrate(network.egressBitsPerSecond))

  return {
    streaming,
    blockers,
    primaryBlocker,
    canStart,
    canStop,
    isStreaming: computed(() => streaming.isStreaming),
    isBusy: computed(() => streaming.isBusy),
    bandwidthLabel,
    start,
    stop,
  }
}

/** Formats bits per second with a sensible unit. */
export function formatBitrate(bitsPerSecond: number): string {
  if (bitsPerSecond <= 0) return '0 kbit/s'
  if (bitsPerSecond < 1_000_000) return `${(bitsPerSecond / 1_000).toFixed(0)} kbit/s`
  return `${(bitsPerSecond / 1_000_000).toFixed(2)} Mbit/s`
}

/** Formats a byte count with a sensible unit. */
export function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`
  if (bytes < 1_024 * 1_024) return `${(bytes / 1_024).toFixed(1)} KiB`
  if (bytes < 1_024 * 1_024 * 1_024) return `${(bytes / (1_024 * 1_024)).toFixed(1)} MiB`
  return `${(bytes / (1_024 * 1_024 * 1_024)).toFixed(2)} GiB`
}
