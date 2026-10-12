import { receiverLabel, type Receiver } from '@stagelink/shared-protocol'

/**
 * Receiver presentation.
 *
 * The important job here is labelling provenance. Every quality figure a
 * receiver shows — loss, jitter, buffer health, underruns — was sent to us by
 * that device and cannot be verified. The desktop's own figures (packets we
 * sent, packets we dropped locally) are separate and are marked as such.
 * Mixing them would let an operator trust a number they should not.
 */
export function useReceivers(options: { refreshOnMount?: boolean } = {}) {
  const receivers = useReceiverStore()
  const { refreshOnMount = true } = options

  if (refreshOnMount) {
    onMounted(() => void receivers.refresh())
  }

  /** Short, operator-facing description of a connection state. */
  function connectionLabel(receiver: Receiver): string {
    switch (receiver.connectionState) {
      case 'streaming':
        return 'Receiving audio'
      case 'connected':
        return 'Connected'
      case 'connecting':
        return 'Connecting…'
      case 'timedOut':
        return 'Not responding'
      case 'offline':
        return 'Offline'
    }
  }

  function connectionTone(receiver: Receiver): 'success' | 'warning' | 'error' | 'neutral' {
    switch (receiver.connectionState) {
      case 'streaming':
        return 'success'
      case 'connected':
        return 'success'
      case 'connecting':
        return 'neutral'
      case 'timedOut':
        return 'error'
      case 'offline':
        return 'neutral'
    }
  }

  function pairingLabel(receiver: Receiver): string {
    // Two states only: a device in the roster scanned a valid code, or the
    // operator has since withdrawn that.
    return receiver.pairingState === 'approved' ? 'Joined by QR' : 'Authorisation revoked'
  }

  /**
   * Health verdict from receiver-reported telemetry.
   *
   * Returns `null` when the receiver has not reported, which the UI renders as
   * "no telemetry yet" rather than as a clean bill of health.
   */
  function reportedHealth(
    receiver: Receiver,
  ): { tone: 'success' | 'warning' | 'error'; label: string } | null {
    const telemetry = receiver.telemetry
    if (!telemetry) return null

    if (telemetry.lossPercent >= 5 || telemetry.underruns > 10) {
      return { tone: 'error', label: 'Poor' }
    }
    if (telemetry.lossPercent >= 1 || telemetry.jitterMs >= 20 || telemetry.underruns > 0) {
      return { tone: 'warning', label: 'Marginal' }
    }
    return { tone: 'success', label: 'Good' }
  }

  /** How stale a receiver's last telemetry report is, in seconds. */
  function telemetryAgeSeconds(receiver: Receiver): number | null {
    if (!receiver.telemetry) return null
    return Math.max(0, Math.round((Date.now() - receiver.telemetry.receivedAt) / 1_000))
  }

  function formatPercent(value: number | null | undefined, digits = 2): string {
    if (value === null || value === undefined) return '—'
    return `${value.toFixed(digits)}%`
  }

  function formatMs(value: number | null | undefined, digits = 1): string {
    if (value === null || value === undefined) return '—'
    return `${value.toFixed(digits)} ms`
  }

  return {
    receivers,
    receiverLabel,
    connectionLabel,
    connectionTone,
    pairingLabel,
    reportedHealth,
    telemetryAgeSeconds,
    formatPercent,
    formatMs,
  }
}
