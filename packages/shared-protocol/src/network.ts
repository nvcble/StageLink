import { z } from 'zod'
import { severitySchema } from './errors'

/**
 * Network contracts.
 *
 * The recommended deployment is a dedicated Wi-Fi access point cabled to this
 * machine over Ethernet, with no internet uplink. Nothing in StageLink requires
 * internet access for live audio.
 */

export const adapterKindSchema = z.enum(['ethernet', 'wifi', 'loopback', 'virtual', 'other'])
export type AdapterKind = z.infer<typeof adapterKindSchema>

export const ipAddressInfoSchema = z.object({
  family: z.enum(['ipv4', 'ipv6']),
  address: z.string(),
  netmask: z.string().nullable(),
  /** CIDR prefix length. */
  prefixLength: z.number().int().min(0).max(128),
  /** Broadcast address for IPv4, used for discovery fallback. */
  broadcast: z.string().nullable(),
})
export type IpAddressInfo = z.infer<typeof ipAddressInfoSchema>

export const networkAdapterSchema = z.object({
  /** OS adapter id (GUID on Windows). Stable across reboots. */
  id: z.string().min(1),
  name: z.string(),
  description: z.string(),
  kind: adapterKindSchema,
  macAddress: z.string().nullable(),
  isUp: z.boolean(),
  /** Negotiated link speed, when the driver reports it. */
  speedMbps: z.number().int().positive().nullable(),
  addresses: z.array(ipAddressInfoSchema),
  /**
   * Whether the adapter has a default route to the internet. `null` means not
   * determined. StageLink never requires this to be true — it is shown so the
   * operator can confirm they picked the isolated AP adapter.
   */
  hasDefaultRoute: z.boolean().nullable(),
})
export type NetworkAdapter = z.infer<typeof networkAdapterSchema>

export const networkWarningCodeSchema = z.enum([
  /** No adapter has been chosen for streaming yet. */
  'NO_ADAPTER_SELECTED',
  /** The persisted adapter is missing or has no address. */
  'SELECTED_ADAPTER_MISSING',
  /** Streaming over Wi-Fi from the desktop; Ethernet to the AP is advised. */
  'STREAMING_OVER_WIFI',
  /** Selected adapter is a loopback/virtual interface. */
  'NON_PHYSICAL_ADAPTER',
  /** The adapter carries a default route; AP is probably not isolated. */
  'SHARED_WITH_INTERNET',
  /** Windows Firewall appears to block the stream or control port. */
  'FIREWALL_MAY_BLOCK',
  /** Estimated egress exceeds a comfortable share of the link rate. */
  'BANDWIDTH_NEAR_LINK_CAPACITY',
  /** Multiple adapters share a subnet; routing may be ambiguous. */
  'SUBNET_CONFLICT',
])
export type NetworkWarningCode = z.infer<typeof networkWarningCodeSchema>

export const networkWarningSchema = z.object({
  code: networkWarningCodeSchema,
  severity: severitySchema,
  message: z.string(),
  detail: z.string().optional(),
  /** Concrete operator action, e.g. "Select the Ethernet adapter". */
  remediation: z.string().optional(),
})
export type NetworkWarning = z.infer<typeof networkWarningSchema>

export const networkStatusSchema = z.object({
  adapters: z.array(networkAdapterSchema),
  selectedAdapterId: z.string().nullable(),
  /** The bound IPv4 address audio is sent from. */
  selectedAddress: z.string().nullable(),
  controlPort: z.number().int().nullable(),
  streamPort: z.number().int().nullable(),
  /** True while mDNS/DNS-SD advertisement is active. */
  discoveryActive: z.boolean(),
  discoveryServiceName: z.string().nullable(),
  /** Desktop-measured egress over the last sampling window, bits/sec. */
  egressBitsPerSecond: z.number().min(0),
  /** Desktop-measured socket send errors since the stream started. */
  sendErrors: z.number().int().min(0),
  warnings: z.array(networkWarningSchema),
})
export type NetworkStatus = z.infer<typeof networkStatusSchema>

/**
 * Firewall probe result. StageLink only ever *reports* on firewall state — it
 * never edits rules without explicit operator consent.
 */
export const firewallStatusSchema = z.object({
  /** `null` when the state could not be determined without elevation. */
  inboundAllowed: z.boolean().nullable(),
  profile: z.string().nullable(),
  /** The exact command an operator can run to add the rule themselves. */
  suggestedCommand: z.string().nullable(),
  detail: z.string().nullable(),
})
export type FirewallStatus = z.infer<typeof firewallStatusSchema>

export const selectAdapterParamsSchema = z.object({
  adapterId: z.string().min(1),
  /** Pick a specific address when the adapter has several. */
  address: z.string().optional(),
})
export type SelectAdapterParams = z.infer<typeof selectAdapterParamsSchema>
