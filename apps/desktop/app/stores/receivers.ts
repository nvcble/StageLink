import {
  isJoinCodeValid,
  joinCodeTimeRemaining,
  receiverLabel,
  type JoinCode,
  type Receiver,
  type ServiceError,
} from '@stagelink/shared-protocol'

/**
 * Connected receivers, and the join code that lets devices in.
 *
 * Devices authorise themselves by scanning the QR — there is no approval queue.
 * The store therefore holds the current code and tracks its expiry, so the UI
 * can never show a QR that has stopped working.
 *
 * The one rule this store exists to enforce: **nothing in here is invented.**
 * An empty roster stays empty. There is no demo device and no optimistic entry
 * added before the service confirms it. An operator looking at this screen
 * thirty seconds before a service has to be able to trust it.
 *
 * Likewise, every quality figure under `telemetry` is the receiver's own claim.
 * The store never promotes it to a measurement, and the UI labels it.
 */
export const useReceiverStore = defineStore('receivers', () => {
  const receivers = ref<Receiver[]>([])
  const joinCode = ref<JoinCode | null>(null)
  const maxReceivers = ref(16)
  const isLoading = ref(false)
  const isMintingCode = ref(false)
  const lastError = ref<ServiceError | null>(null)

  /**
   * Ticks every second so the countdown advances and the code retires itself
   * in the UI at the same moment the service stops honouring it.
   */
  const now = ref(Date.now())
  let ticker: ReturnType<typeof setInterval> | null = null

  /** Receivers currently getting audio. */
  const connected = computed(() =>
    receivers.value.filter(
      (receiver) =>
        receiver.connectionState === 'connected' || receiver.connectionState === 'streaming',
    ),
  )

  const connectedCount = computed(() => connected.value.length)

  const timedOut = computed(() =>
    receivers.value.filter((receiver) => receiver.connectionState === 'timedOut'),
  )

  /** Trusted receivers that are not currently attached. */
  const offline = computed(() =>
    receivers.value.filter(
      (receiver) => receiver.connectionState === 'offline' && receiver.pairingState === 'approved',
    ),
  )

  const isEmpty = computed(() => receivers.value.length === 0)
  const atCapacity = computed(() => connectedCount.value >= maxReceivers.value)

  /** True while the displayed QR will actually admit a device. */
  const hasValidJoinCode = computed(() => isJoinCodeValid(joinCode.value, now.value))

  /** Seconds left on the current code, for the countdown. */
  const joinCodeSecondsRemaining = computed(() =>
    Math.ceil(joinCodeTimeRemaining(joinCode.value, now.value) / 1_000),
  )

  /** Receivers sorted for display: attention first, then by label. */
  const sorted = computed(() => {
    const rank = (receiver: Receiver) => {
      if (receiver.connectionState === 'timedOut') return 0
      if (receiver.connectionState === 'streaming') return 1
      if (receiver.connectionState === 'connected') return 2
      return 3
    }
    return [...receivers.value].sort(
      (a, b) => rank(a) - rank(b) || receiverLabel(a).localeCompare(receiverLabel(b)),
    )
  })

  /**
   * The worst loss any receiver has *reported*, or `null` if none has reported.
   *
   * Null rather than zero: "no receiver has told us anything" must not read as
   * "everything is perfect".
   */
  const worstReportedLoss = computed(() => {
    const values = receivers.value
      .map((receiver) => receiver.telemetry?.lossPercent)
      .filter((value): value is number => typeof value === 'number')
    return values.length > 0 ? Math.max(...values) : null
  })

  const worstReportedJitter = computed(() => {
    const values = receivers.value
      .map((receiver) => receiver.telemetry?.jitterMs)
      .filter((value): value is number => typeof value === 'number')
    return values.length > 0 ? Math.max(...values) : null
  })

  function applyReceivers(next: Receiver[]) {
    receivers.value = next
  }

  function applyJoinCode(next: JoinCode | null) {
    joinCode.value = next
  }

  async function refresh() {
    const { tryInvoke } = useStageLinkBridge()
    isLoading.value = true
    try {
      const result = await tryInvoke('receivers.list')
      if (result.ok) {
        receivers.value = result.value.receivers
        joinCode.value = result.value.joinCode
        maxReceivers.value = result.value.maxReceivers
        lastError.value = null
      } else {
        lastError.value = result.error
      }
    } finally {
      isLoading.value = false
    }
  }

  /**
   * Mints a join code, replacing any previous one.
   *
   * Rotation is the operator's answer to a QR that got photographed or shared:
   * the old token stops working the moment this resolves.
   */
  async function createJoinCode() {
    const { tryInvoke } = useStageLinkBridge()
    isMintingCode.value = true
    try {
      const result = await tryInvoke('receivers.createJoinCode')
      if (!result.ok) {
        lastError.value = result.error
        return null
      }
      joinCode.value = result.value
      lastError.value = null
      return result.value
    } finally {
      isMintingCode.value = false
    }
  }

  /**
   * Administrative actions.
   *
   * Each refreshes from the service rather than mutating locally, so the list
   * always reflects what the service actually did.
   */
  async function rename(receiverId: string, displayName: string) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.rename', { receiverId, displayName })
    if (!result.ok) {
      lastError.value = result.error
      return false
    }
    await refresh()
    return true
  }

  async function disconnect(receiverId: string) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.disconnect', { receiverId })
    if (!result.ok) {
      lastError.value = result.error
      return false
    }
    await refresh()
    return true
  }

  /** Withdraws authorisation permanently. */
  async function revoke(receiverId: string) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.revoke', { receiverId })
    if (!result.ok) {
      lastError.value = result.error
      return false
    }
    await refresh()
    return true
  }

  /** Starts the one-second clock driving the join-code countdown. */
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
    receivers,
    joinCode,
    maxReceivers,
    isLoading,
    isMintingCode,
    lastError,
    connected,
    connectedCount,
    timedOut,
    offline,
    isEmpty,
    atCapacity,
    hasValidJoinCode,
    joinCodeSecondsRemaining,
    sorted,
    worstReportedLoss,
    worstReportedJitter,
    applyReceivers,
    applyJoinCode,
    refresh,
    createJoinCode,
    rename,
    disconnect,
    revoke,
    startClock,
    stopClock,
    clearError,
  }
})
