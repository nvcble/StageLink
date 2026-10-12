import type { NetworkAdapter } from '@stagelink/shared-protocol'

/**
 * Network presentation and guidance.
 *
 * StageLink has an opinion about deployment — a dedicated access point, cabled
 * to this machine over Ethernet, with no internet uplink — and this is where
 * that opinion is turned into concrete, actionable guidance rather than a wall
 * of adapter rows.
 */
export function useNetworkStatus(options: { refreshOnMount?: boolean } = {}) {
  const network = useNetworkStore()
  const { refreshOnMount = true } = options

  if (refreshOnMount) {
    onMounted(() => void network.refresh())
  }

  /** The adapter StageLink would pick, if the operator has not chosen. */
  const recommendedAdapter = computed<NetworkAdapter | null>(() => {
    // An Ethernet adapter with no internet route is almost certainly the
    // dedicated access point link.
    const isolatedEthernet = network.ethernetAdapters.find(
      (adapter) => adapter.hasDefaultRoute === false,
    )
    if (isolatedEthernet) return isolatedEthernet
    return network.ethernetAdapters[0] ?? network.usableAdapters[0] ?? null
  })

  /** Why that adapter is recommended, so the suggestion is not a black box. */
  const recommendationReason = computed(() => {
    const adapter = recommendedAdapter.value
    if (!adapter) return null
    if (adapter.kind === 'ethernet' && adapter.hasDefaultRoute === false) {
      return 'Ethernet with no internet route — this looks like a dedicated access point link.'
    }
    if (adapter.kind === 'ethernet') {
      return 'Ethernet gives the most predictable audio, even if it also carries internet traffic.'
    }
    return 'The only usable adapter. Cabling this machine to the access point would be better.'
  })

  function adapterLabel(adapter: NetworkAdapter): string {
    const address = adapter.addresses.find((info) => info.family === 'ipv4')?.address
    return address ? `${adapter.name} — ${address}` : adapter.name
  }

  function adapterKindLabel(adapter: NetworkAdapter): string {
    switch (adapter.kind) {
      case 'ethernet':
        return 'Ethernet'
      case 'wifi':
        return 'Wi-Fi'
      case 'loopback':
        return 'Loopback'
      case 'virtual':
        return 'Virtual'
      default:
        return 'Other'
    }
  }

  function primaryAddress(adapter: NetworkAdapter): string | null {
    return adapter.addresses.find((info) => info.family === 'ipv4')?.address ?? null
  }

  function speedLabel(adapter: NetworkAdapter): string {
    if (adapter.speedMbps === null) return '—'
    if (adapter.speedMbps >= 1_000) return `${(adapter.speedMbps / 1_000).toFixed(0)} Gbit/s`
    return `${adapter.speedMbps} Mbit/s`
  }

  /**
   * The setup instructions. Deliberately prose rather than a diagram: the
   * person reading this is often not the person who built the network.
   */
  const setupSteps = [
    'Connect a dedicated Wi-Fi access point to this machine with an Ethernet cable.',
    'Give the access point its own subnet. It does not need an internet connection — StageLink never requires one for live audio.',
    'Select that Ethernet adapter below as the streaming adapter.',
    'Join every receiver phone to the access point\'s Wi-Fi network, and nothing else.',
    'Allow StageLink Core through Windows Firewall on that network when prompted. StageLink will not change firewall rules on your behalf.',
  ] as const

  return {
    network,
    recommendedAdapter,
    recommendationReason,
    adapterLabel,
    adapterKindLabel,
    primaryAddress,
    speedLabel,
    setupSteps,
  }
}
