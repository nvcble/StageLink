import type { NetworkStatus, ServiceError } from '@stagelink/shared-protocol'

/**
 * Network configuration.
 *
 * The recommended deployment is a dedicated Wi-Fi access point cabled to this
 * machine over Ethernet, with no internet uplink. The warnings surfaced here
 * nudge towards that without ever treating a missing uplink as a fault —
 * StageLink needs no internet access for live audio.
 */
export const useNetworkStore = defineStore('network', () => {
  const status = ref<NetworkStatus | null>(null)
  const isLoading = ref(false)
  const isSelecting = ref(false)
  const lastError = ref<ServiceError | null>(null)

  const adapters = computed(() => status.value?.adapters ?? [])
  const selectedAdapterId = computed(() => status.value?.selectedAdapterId ?? null)
  const selectedAddress = computed(() => status.value?.selectedAddress ?? null)
  const warnings = computed(() => status.value?.warnings ?? [])

  const selectedAdapter = computed(
    () => adapters.value.find((adapter) => adapter.id === selectedAdapterId.value) ?? null,
  )

  /** Adapters that could actually carry a stream. */
  const usableAdapters = computed(() =>
    adapters.value.filter(
      (adapter) =>
        adapter.isUp &&
        adapter.kind !== 'loopback' &&
        adapter.addresses.some((address) => address.family === 'ipv4'),
    ),
  )

  /** The Ethernet adapters, which is what the operator should normally pick. */
  const ethernetAdapters = computed(() =>
    usableAdapters.value.filter((adapter) => adapter.kind === 'ethernet'),
  )

  const hasBlockingWarning = computed(() =>
    warnings.value.some((warning) => warning.severity === 'error' || warning.severity === 'critical'),
  )

  /** The address receivers should be pointed at. */
  const serverAddress = computed(() => {
    if (!selectedAddress.value || !status.value?.controlPort) return null
    return `${selectedAddress.value}:${status.value.controlPort}`
  })

  const isConfigured = computed(
    () => selectedAdapterId.value !== null && selectedAddress.value !== null,
  )

  const egressBitsPerSecond = computed(() => status.value?.egressBitsPerSecond ?? 0)
  const sendErrors = computed(() => status.value?.sendErrors ?? 0)
  const discoveryActive = computed(() => status.value?.discoveryActive ?? false)

  function applyStatus(next: NetworkStatus) {
    status.value = next
  }

  async function refresh() {
    const { tryInvoke } = useStageLinkBridge()
    isLoading.value = true
    try {
      const result = await tryInvoke('network.getStatus')
      if (result.ok) {
        applyStatus(result.value)
        lastError.value = null
      } else {
        lastError.value = result.error
      }
    } finally {
      isLoading.value = false
    }
  }

  async function selectAdapter(adapterId: string, address?: string) {
    const { tryInvoke } = useStageLinkBridge()
    isSelecting.value = true
    try {
      const result = await tryInvoke('network.selectAdapter', {
        adapterId,
        ...(address ? { address } : {}),
      })
      if (result.ok) {
        applyStatus(result.value)
        lastError.value = null
        return true
      }
      lastError.value = result.error
      return false
    } finally {
      isSelecting.value = false
    }
  }

  /**
   * Reports on Windows Firewall. Read-only by design: StageLink never edits
   * firewall rules without the operator doing it themselves.
   */
  async function checkFirewall() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('network.checkFirewall')
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
    status,
    isLoading,
    isSelecting,
    lastError,
    adapters,
    selectedAdapterId,
    selectedAddress,
    selectedAdapter,
    usableAdapters,
    ethernetAdapters,
    warnings,
    hasBlockingWarning,
    serverAddress,
    isConfigured,
    egressBitsPerSecond,
    sendErrors,
    discoveryActive,
    applyStatus,
    refresh,
    selectAdapter,
    checkFirewall,
    clearError,
  }
})
