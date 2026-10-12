import {
  receiverLabel,
  type PairingRequest,
  type Receiver,
  type ServiceError,
} from '@stagelink/shared-protocol'

/**
 * Connected receivers.
 *
 * The one rule this store exists to enforce: **nothing in here is invented.**
 * An empty roster stays empty. There is no demo device, no placeholder row and
 * no optimistic entry added before the service confirms it. An operator looking
 * at this screen thirty seconds before a service starts has to be able to trust
 * that what they see is what is connected.
 *
 * Likewise, every quality figure under `telemetry` is the receiver's own claim.
 * The store never promotes it to a measurement, and the UI labels it.
 */
export const useReceiverStore = defineStore('receivers', () => {
  const receivers = ref<Receiver[]>([])
  const pendingRequests = ref<PairingRequest[]>([])
  const maxReceivers = ref(16)
  const isLoading = ref(false)
  const lastError = ref<ServiceError | null>(null)

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

  const isEmpty = computed(() => receivers.value.length === 0 && pendingRequests.value.length === 0)
  const hasPendingApprovals = computed(() => pendingRequests.value.length > 0)
  const atCapacity = computed(() => connectedCount.value >= maxReceivers.value)

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

  function applyPairingRequest(request: PairingRequest) {
    const existing = pendingRequests.value.findIndex(
      (candidate) => candidate.requestId === request.requestId,
    )
    if (existing >= 0) pendingRequests.value.splice(existing, 1, request)
    else pendingRequests.value = [...pendingRequests.value, request]
  }

  function resolvePairingRequest(requestId: string) {
    pendingRequests.value = pendingRequests.value.filter(
      (request) => request.requestId !== requestId,
    )
  }

  async function refresh() {
    const { tryInvoke } = useStageLinkBridge()
    isLoading.value = true
    try {
      const result = await tryInvoke('receivers.list')
      if (result.ok) {
        receivers.value = result.value.receivers
        pendingRequests.value = result.value.pendingRequests
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
   * Administrative actions.
   *
   * Each refreshes from the service rather than mutating locally, so the list
   * always reflects what the service actually did.
   */
  async function approve(requestId: string, displayName?: string) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.approve', {
      requestId,
      ...(displayName ? { displayName } : {}),
      trustPermanently: true,
    })
    if (!result.ok) {
      lastError.value = result.error
      return false
    }
    resolvePairingRequest(requestId)
    await refresh()
    return true
  }

  async function reject(requestId: string, reason?: string) {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.reject', {
      requestId,
      ...(reason ? { reason } : {}),
    })
    if (!result.ok) {
      lastError.value = result.error
      return false
    }
    resolvePairingRequest(requestId)
    return true
  }

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

  async function createPairingCode() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('receivers.createPairingCode')
    if (!result.ok) {
      lastError.value = result.error
      return null
    }
    return result.value
  }

  function clearError() {
    lastError.value = null
  }

  return {
    receivers,
    pendingRequests,
    maxReceivers,
    isLoading,
    lastError,
    connected,
    connectedCount,
    timedOut,
    offline,
    isEmpty,
    hasPendingApprovals,
    atCapacity,
    sorted,
    worstReportedLoss,
    worstReportedJitter,
    applyReceivers,
    applyPairingRequest,
    resolvePairingRequest,
    refresh,
    approve,
    reject,
    rename,
    disconnect,
    revoke,
    createPairingCode,
    clearError,
  }
})
