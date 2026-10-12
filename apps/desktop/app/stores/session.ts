import {
  canTransition,
  isSessionActive,
  type ServiceError,
  type Session,
  type SessionEvent,
  type SessionState,
} from '@stagelink/shared-protocol'

/**
 * Session lifecycle.
 *
 * The service owns the state machine; this store mirrors it so the UI can
 * disable actions that would be refused anyway. That is a better experience
 * than letting an operator press Start twice and watch an error appear — but
 * the service remains the authority, and a transition is never assumed locally.
 */
export const useSessionStore = defineStore('session', () => {
  const current = ref<Session | null>(null)
  const history = ref<Session[]>([])
  const historyTotal = ref(0)
  const events = ref<SessionEvent[]>([])
  const isLoading = ref(false)
  const lastError = ref<ServiceError | null>(null)

  /** Ticks every second so the duration display advances. */
  const now = ref(Date.now())
  let ticker: ReturnType<typeof setInterval> | null = null

  const state = computed<SessionState>(() => current.value?.state ?? 'idle')
  const isActive = computed(() => isSessionActive(state.value))
  const isStreaming = computed(() => state.value === 'streaming')
  const hasError = computed(() => state.value === 'error')

  /** Live duration in milliseconds, recomputed as `now` advances. */
  const durationMs = computed(() => {
    const session = current.value
    if (!session) return 0
    if (session.startedAt && !session.endedAt) {
      return Math.max(0, now.value - session.startedAt)
    }
    return session.durationMs
  })

  const uptimeLabel = computed(() => formatDuration(durationMs.value))

  /** Whether the UI should enable a transition to `next`. */
  function canMoveTo(next: SessionState) {
    return canTransition(state.value, next)
  }

  const peakReceivers = computed(() => current.value?.peakReceivers ?? 0)
  const totalReceivers = computed(() => current.value?.totalReceivers ?? 0)

  function applySession(next: Session) {
    current.value = next
    // A session that has ended belongs in history, not in the live slot.
    if (next.state === 'idle' && next.endedAt !== null) {
      void refreshHistory()
    }
  }

  function appendEvent(event: SessionEvent) {
    events.value = [event, ...events.value].slice(0, 500)
  }

  async function refreshCurrent() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('session.getCurrent')
    if (result.ok) {
      current.value = result.value
      lastError.value = null
    } else {
      lastError.value = result.error
    }
  }

  async function refreshHistory(limit = 50, offset = 0) {
    const { tryInvoke } = useStageLinkBridge()
    isLoading.value = true
    try {
      const result = await tryInvoke('session.history', { limit, offset })
      if (result.ok) {
        history.value = result.value.sessions
        historyTotal.value = result.value.total
        lastError.value = null
      } else {
        lastError.value = result.error
      }
    } finally {
      isLoading.value = false
    }
  }

  async function loadEvents(sessionId: string, limit = 200) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('session.events', { sessionId, limit })
    if (result.ok) {
      events.value = result.value.events
      lastError.value = null
      return result.value.events
    }
    lastError.value = result.error
    return []
  }

  /** Starts the one-second clock that drives the duration display. */
  function startClock() {
    if (ticker !== null) return
    ticker = setInterval(() => {
      now.value = Date.now()
    }, 1_000)
  }

  function stopClock() {
    if (ticker === null) return
    clearInterval(ticker)
    ticker = null
  }

  function clearError() {
    lastError.value = null
  }

  return {
    current,
    history,
    historyTotal,
    events,
    isLoading,
    lastError,
    state,
    isActive,
    isStreaming,
    hasError,
    durationMs,
    uptimeLabel,
    peakReceivers,
    totalReceivers,
    canMoveTo,
    applySession,
    appendEvent,
    refreshCurrent,
    refreshHistory,
    loadEvents,
    startClock,
    stopClock,
    clearError,
  }
})

/** Formats milliseconds as `h:mm:ss` or `mm:ss`. */
export function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1_000))
  const hours = Math.floor(totalSeconds / 3_600)
  const minutes = Math.floor((totalSeconds % 3_600) / 60)
  const seconds = totalSeconds % 60
  const pad = (value: number) => String(value).padStart(2, '0')
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`
}
