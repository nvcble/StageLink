import type { SessionEvent, SessionState } from '@stagelink/shared-protocol'

/**
 * Session presentation.
 *
 * Owns the one-second clock that drives the uptime display, and the mapping
 * from state machine states to something an operator reads at a glance.
 */
export function useSession(options: { runClock?: boolean } = {}) {
  const session = useSessionStore()
  const { runClock = true } = options

  if (runClock) {
    onMounted(() => session.startClock())
    onBeforeUnmount(() => session.stopClock())
  }

  function stateLabel(state: SessionState): string {
    switch (state) {
      case 'idle':
        return 'Idle'
      case 'preparing':
        return 'Preparing'
      case 'ready':
        return 'Ready'
      case 'streaming':
        return 'Streaming'
      case 'stopping':
        return 'Stopping'
      case 'recovering':
        return 'Recovering'
      case 'error':
        return 'Error'
    }
  }

  function stateTone(state: SessionState): 'success' | 'warning' | 'error' | 'neutral' {
    switch (state) {
      case 'streaming':
        return 'success'
      case 'ready':
        return 'success'
      case 'preparing':
      case 'stopping':
      case 'recovering':
        return 'warning'
      case 'error':
        return 'error'
      case 'idle':
        return 'neutral'
    }
  }

  /** Plain-language description of what the system is doing right now. */
  function stateDescription(state: SessionState): string {
    switch (state) {
      case 'idle':
        return 'Nothing is running.'
      case 'preparing':
        return 'Opening the audio input and binding the network sockets.'
      case 'ready':
        return 'Audio input is open. Press Start Streaming to send it to receivers.'
      case 'streaming':
        return 'Audio is being sent to connected receivers.'
      case 'stopping':
        return 'Draining queued audio and closing down.'
      case 'recovering':
        return 'A fault occurred and StageLink is trying to recover automatically.'
      case 'error':
        return 'The session stopped with an error and needs attention.'
    }
  }

  function eventIcon(event: SessionEvent): string {
    if (event.kind.startsWith('receiver.')) return 'i-lucide-smartphone'
    if (event.kind.startsWith('audio.')) return 'i-lucide-audio-waveform'
    if (event.kind.startsWith('stream.')) return 'i-lucide-radio'
    if (event.kind.startsWith('network.')) return 'i-lucide-network'
    if (event.kind === 'error.raised') return 'i-lucide-triangle-alert'
    return 'i-lucide-circle-dot'
  }

  function severityTone(severity: SessionEvent['severity']): 'success' | 'warning' | 'error' | 'neutral' {
    switch (severity) {
      case 'info':
        return 'neutral'
      case 'warning':
        return 'warning'
      case 'error':
      case 'critical':
        return 'error'
    }
  }

  /** Formats an epoch-millisecond timestamp as a local clock time. */
  function formatTime(at: number): string {
    return new Date(at).toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    })
  }

  /** Formats an epoch-millisecond timestamp as a local date and time. */
  function formatDateTime(at: number | null): string {
    if (at === null) return '—'
    return new Date(at).toLocaleString(undefined, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
  }

  return {
    session,
    stateLabel,
    stateTone,
    stateDescription,
    eventIcon,
    severityTone,
    formatTime,
    formatDateTime,
    formatDuration,
  }
}
