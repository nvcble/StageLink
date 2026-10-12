import { z } from 'zod'
import {
  audioStatusSchema,
  audioTestParamsSchema,
  audioTestResultSchema,
  listDevicesParamsSchema,
  listDevicesResultSchema,
  selectDeviceParamsSchema,
} from './audio'
import {
  diagnosticsSnapshotSchema,
  diagnosticsSubscribeParamsSchema,
} from './diagnostics'
import {
  firewallStatusSchema,
  networkStatusSchema,
  selectAdapterParamsSchema,
} from './network'
import {
  joinCodeSchema,
  receiverHeartbeatParamsSchema,
  receiverHeartbeatResultSchema,
  receiverIdParamsSchema,
  receiverTelemetryParamsSchema,
  receiverTelemetryResultSchema,
  receiverSchema,
  receiversListResultSchema,
  renameReceiverParamsSchema,
} from './receivers'
import {
  sessionEventSchema,
  sessionEventsParamsSchema,
  sessionHistoryParamsSchema,
  sessionHistoryResultSchema,
  sessionSchema,
} from './session'
import { appSettingsSchema, settingsPatchSchema, settingsUpdateResultSchema } from './settings'
import {
  DEFAULT_REQUEST_TIMEOUT_MS,
  HARDWARE_REQUEST_TIMEOUT_MS,
  serviceInfoSchema,
  serviceShutdownParamsSchema,
} from './service'
import { streamStartParamsSchema, streamStatusSchema, streamStopParamsSchema } from './stream'

/**
 * The control-method registry.
 *
 * This object is the single authority for what the renderer may ask the native
 * service to do. The preload bridge exposes exactly these names and nothing
 * else — there is no generic "invoke(command, args)" escape hatch, so a
 * compromised renderer cannot reach an operation that is not listed here.
 *
 * Each entry carries both schemas so the same definition drives:
 *   - compile-time types in the renderer,
 *   - runtime validation of params in the Electron main process (before the
 *     request reaches the service),
 *   - runtime validation of results (so a misbehaving service cannot inject
 *     unexpected shapes into Pinia),
 *   - the generated protocol documentation for StageLink Mobile.
 */
export interface MethodDefinition<
  P extends z.ZodType = z.ZodType,
  R extends z.ZodType = z.ZodType,
> {
  params: P
  result: R
  /** Request deadline in milliseconds. */
  timeoutMs: number
  /** Used by the generated protocol docs. */
  description: string
  /** False for pure reads; used to gate requests while the service is degraded. */
  mutates: boolean
}

/** Params schema for methods that take no arguments. */
const noParams = z.object({}).optional()

export const METHODS = {
  // -- audio ---------------------------------------------------------------
  'audio.listDevices': {
    params: listDevicesParamsSchema.optional(),
    result: listDevicesResultSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Enumerate Windows audio capture endpoints.',
    mutates: false,
  },
  'audio.selectDevice': {
    params: selectDeviceParamsSchema,
    result: audioStatusSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Open a capture device and begin monitoring one mono input channel.',
    mutates: true,
  },
  'audio.releaseDevice': {
    params: noParams,
    result: audioStatusSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Close the capture device and stop metering.',
    mutates: true,
  },
  'audio.getStatus': {
    params: noParams,
    result: audioStatusSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Current capture state, negotiated format and device health.',
    mutates: false,
  },
  'audio.test': {
    params: audioTestParamsSchema,
    result: audioTestResultSchema,
    timeoutMs: 45_000,
    description: 'Listen to the selected input and report whether real signal is present.',
    mutates: false,
  },

  // -- stream --------------------------------------------------------------
  'stream.start': {
    params: streamStartParamsSchema,
    result: streamStatusSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Start packetising and sending audio to authorised receivers.',
    mutates: true,
  },
  'stream.stop': {
    params: streamStopParamsSchema,
    result: streamStatusSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Stop sending audio, draining queued packets unless immediate.',
    mutates: true,
  },
  'stream.getStatus': {
    params: noParams,
    result: streamStatusSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Current stream state and desktop-measured transmission counters.',
    mutates: false,
  },

  // -- receivers -----------------------------------------------------------
  'receivers.list': {
    params: noParams,
    result: receiversListResultSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'All known receivers and the current join code.',
    mutates: false,
  },
  'receivers.rename': {
    params: renameReceiverParamsSchema,
    result: receiverSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Set the operator-facing label for a receiver.',
    mutates: true,
  },
  'receivers.disconnect': {
    params: receiverIdParamsSchema,
    result: receiverSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Drop a receiver from the current session; it may reconnect if trusted.',
    mutates: true,
  },
  'receivers.revoke': {
    params: receiverIdParamsSchema,
    result: receiverSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Withdraw a receiver authorisation permanently and disconnect it.',
    mutates: true,
  },
  // -- receiver -> desktop -------------------------------------------------
  // The only two methods a receiver calls. Requests, not events: events are
  // server-to-client only and the control server discards client-sent events.
  'receiver.heartbeat': {
    params: receiverHeartbeatParamsSchema,
    result: receiverHeartbeatResultSchema,
    timeoutMs: 2_000,
    description:
      'Receiver liveness. Missing RECEIVER_HEARTBEAT_MISS_LIMIT of these stops audio to it.',
    mutates: true,
  },
  'receiver.telemetry': {
    params: receiverTelemetryParamsSchema,
    result: receiverTelemetryResultSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description:
      'Receiver-reported quality figures. Unverifiable by the desktop and labelled as such.',
    mutates: true,
  },

  'receivers.createJoinCode': {
    params: noParams,
    result: joinCodeSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description:
      'Mint a short-lived join code and its QR payload, replacing any previous one. ' +
      'A receiver that scans it is authorised without further approval.',
    mutates: true,
  },

  // -- network -------------------------------------------------------------
  'network.getStatus': {
    params: noParams,
    result: networkStatusSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Adapters, bound address, discovery state and network warnings.',
    mutates: false,
  },
  'network.selectAdapter': {
    params: selectAdapterParamsSchema,
    result: networkStatusSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Choose the adapter and address used for streaming and discovery.',
    mutates: true,
  },
  'network.checkFirewall': {
    params: noParams,
    result: firewallStatusSchema,
    timeoutMs: HARDWARE_REQUEST_TIMEOUT_MS,
    description: 'Report whether inbound rules exist for the StageLink ports. Read-only.',
    mutates: false,
  },

  // -- session -------------------------------------------------------------
  'session.getCurrent': {
    params: noParams,
    result: sessionSchema.nullable(),
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'The active session, or null when idle.',
    mutates: false,
  },
  'session.history': {
    params: sessionHistoryParamsSchema.optional(),
    result: sessionHistoryResultSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Paginated past sessions from the local database.',
    mutates: false,
  },
  'session.events': {
    params: sessionEventsParamsSchema,
    result: z.object({ events: z.array(sessionEventSchema) }),
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Operational event log for one session.',
    mutates: false,
  },

  // -- diagnostics ---------------------------------------------------------
  'diagnostics.getSnapshot': {
    params: noParams,
    result: diagnosticsSnapshotSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'One-shot diagnostics snapshot with per-metric provenance.',
    mutates: false,
  },
  'diagnostics.subscribe': {
    params: diagnosticsSubscribeParamsSchema,
    result: z.object({ enabled: z.boolean(), intervalMs: z.number().int() }),
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Turn periodic diagnostics pushes on or off.',
    mutates: true,
  },

  // -- settings ------------------------------------------------------------
  'settings.get': {
    params: noParams,
    result: appSettingsSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Read the persisted, fully-defaulted application settings.',
    mutates: false,
  },
  'settings.update': {
    params: settingsPatchSchema,
    result: settingsUpdateResultSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Validate and persist a settings patch; reports restart-required paths.',
    mutates: true,
  },

  // -- service -------------------------------------------------------------
  'service.getInfo': {
    params: noParams,
    result: serviceInfoSchema,
    timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
    description: 'Service version, protocol versions and supervision state.',
    mutates: false,
  },
  'service.ping': {
    params: noParams,
    result: z.object({ at: z.number().int(), uptimeMs: z.number().int() }),
    timeoutMs: 2_000,
    description: 'Liveness probe used by the supervisor.',
    mutates: false,
  },
  'service.shutdown': {
    params: serviceShutdownParamsSchema,
    result: z.object({ accepted: z.boolean() }),
    timeoutMs: 15_000,
    description: 'Request an orderly service shutdown with a drain grace period.',
    mutates: true,
  },
} as const satisfies Record<string, MethodDefinition>

export type MethodRegistry = typeof METHODS
export type MethodName = keyof MethodRegistry

/** Params accepted by a method, as written by a caller (pre-defaults). */
export type MethodParams<M extends MethodName> = z.input<MethodRegistry[M]['params']>

/** Params as the service sees them, with defaults applied. */
export type MethodParsedParams<M extends MethodName> = z.output<MethodRegistry[M]['params']>

/** The value a method resolves to. */
export type MethodResult<M extends MethodName> = z.output<MethodRegistry[M]['result']>

export const METHOD_NAMES = Object.keys(METHODS) as MethodName[]

export function isMethodName(value: string): value is MethodName {
  return Object.prototype.hasOwnProperty.call(METHODS, value)
}

export function methodDefinition<M extends MethodName>(method: M): MethodRegistry[M] {
  return METHODS[method]
}
