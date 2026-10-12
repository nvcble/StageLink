import { z } from 'zod'

/**
 * Receiver (StageLink Mobile) contracts.
 *
 * IMPORTANT: every field under `telemetry` is *reported by the receiver* and is
 * therefore unverified by the desktop. The `source` discriminator is carried on
 * the wire so the UI can label it honestly and never present it as a desktop
 * measurement. See docs/protocol/receiver-telemetry.md.
 */

export const receiverPlatformSchema = z.enum([
  'android',
  'ios',
  'windows',
  'macos',
  'linux',
  'headless-test',
  'unknown',
])
export type ReceiverPlatform = z.infer<typeof receiverPlatformSchema>

/**
 * Authorisation lifecycle, persisted in SQLite.
 *
 * Only two states, because authorisation is granted by scanning the join QR
 * rather than by an operator approving a request. A device that presented a
 * valid code is `approved`; the operator can later withdraw that with
 * `receivers.revoke`.
 */
export const pairingStateSchema = z.enum([
  /** Presented a valid join code. Trusted until revoked. */
  'approved',
  /** Authorisation withdrawn by the operator. */
  'revoked',
])
export type PairingState = z.infer<typeof pairingStateSchema>

/** Live connectivity, in-memory only. */
export const connectionStateSchema = z.enum([
  'offline',
  'connecting',
  /** Control channel up, not receiving audio. */
  'connected',
  /** Control channel up and audio flowing. */
  'streaming',
  /** Missed the heartbeat deadline. */
  'timedOut',
])
export type ConnectionState = z.infer<typeof connectionStateSchema>

export const playbackStateSchema = z.enum(['stopped', 'playing', 'muted', 'buffering', 'unknown'])
export type PlaybackState = z.infer<typeof playbackStateSchema>

/**
 * Receiver-reported quality telemetry. Arrives over the control channel at
 * `RECEIVER_TELEMETRY_INTERVAL_MS`.
 */
export const receiverTelemetrySchema = z.object({
  /** Literal marker so this data can never be mistaken for a local measurement. */
  source: z.literal('receiver-reported'),
  /** Desktop clock time at which the report was accepted. */
  receivedAt: z.number().int(),
  /** Receiver's own clock time when it built the report. */
  reportedAt: z.number().int(),
  packetsReceived: z.number().int().min(0),
  /** Derived from sequence-number gaps observed by the receiver. */
  packetsLost: z.number().int().min(0),
  lossPercent: z.number().min(0).max(100),
  /** Inter-arrival jitter as measured on the receiver. */
  jitterMs: z.number().min(0),
  /** Current jitter-buffer fill, in milliseconds of audio. */
  bufferHealthMs: z.number().min(0),
  /** Audio-output underruns since the receiver connected. */
  underruns: z.number().int().min(0),
  playbackState: playbackStateSchema,
  /** Receiver's own estimate of its output-path latency, if it can measure it. */
  outputLatencyMs: z.number().min(0).nullable(),
})

export type ReceiverTelemetry = z.infer<typeof receiverTelemetrySchema>

export const RECEIVER_TELEMETRY_INTERVAL_MS = 2_000 as const
export const RECEIVER_HEARTBEAT_INTERVAL_MS = 1_000 as const
/** Missing this many heartbeats moves a receiver to `timedOut`. */
export const RECEIVER_HEARTBEAT_MISS_LIMIT = 5 as const

export const receiverSchema = z.object({
  /** Desktop-assigned stable id (UUID), persisted for trusted receivers. */
  id: z.string().min(1),
  /** Self-reported device name, e.g. "Pixel 8". Untrusted input. */
  deviceName: z.string(),
  /** Operator-assigned label, e.g. "Lead Vocal". Overrides deviceName in the UI. */
  displayName: z.string().nullable(),
  platform: receiverPlatformSchema,
  platformVersion: z.string().nullable(),
  appVersion: z.string().nullable(),
  ipAddress: z.string().nullable(),
  connectionState: connectionStateSchema,
  pairingState: pairingStateSchema,
  firstSeenAt: z.number().int(),
  lastSeenAt: z.number().int(),
  lastHeartbeatAt: z.number().int().nullable(),
  /** Desktop-measured: packets we attempted to send to this receiver. */
  packetsSentToReceiver: z.number().int().min(0),
  /** Desktop-measured: packets dropped locally because its queue was full. */
  packetsDroppedLocally: z.number().int().min(0),
  telemetry: receiverTelemetrySchema.nullable(),
})

export type Receiver = z.infer<typeof receiverSchema>

/** Returns the name the UI should display for a receiver. */
export function receiverLabel(receiver: Receiver): string {
  return receiver.displayName?.trim() || receiver.deviceName.trim() || receiver.id.slice(0, 8)
}

// ---------------------------------------------------------------------------
// Joining
// ---------------------------------------------------------------------------

/**
 * How long a join code stays valid. The operator can rotate it at any time.
 *
 * Short on purpose: the code in the QR is a bearer credential, so anyone who
 * photographs the operator's screen can join while it lives. Five minutes is
 * long enough to walk a band through scanning and short enough that a
 * screenshot is not lasting access.
 */
export const JOIN_CODE_TTL_MS = 300_000 as const

/** Bytes of randomness in a join token. */
export const JOIN_TOKEN_BYTES = 32 as const

/**
 * The current join code, and everything a receiver needs to connect.
 *
 * `qrPayload` is what the QR encodes; see `docs/protocol/pairing.md` for the
 * grammar. The other fields are broken out so the desktop can also display the
 * address for manual entry when a camera is not an option.
 */
export const joinCodeSchema = z.object({
  /** Bearer credential. Presenting it is what authorises a receiver. */
  token: z.string().min(16),
  qrPayload: z.string().min(1),
  host: z.string().min(1),
  controlPort: z.number().int().min(1),
  streamPort: z.number().int().min(1),
  issuedAt: z.number().int(),
  expiresAt: z.number().int(),
})

export type JoinCode = z.infer<typeof joinCodeSchema>

/** Whether a join code is still usable at `now`. */
export function isJoinCodeValid(code: JoinCode | null, now = Date.now()): boolean {
  return code !== null && code.expiresAt > now
}

/** Milliseconds until a join code expires, clamped at zero. */
export function joinCodeTimeRemaining(code: JoinCode | null, now = Date.now()): number {
  return code === null ? 0 : Math.max(0, code.expiresAt - now)
}

// ---------------------------------------------------------------------------
// Method params
// ---------------------------------------------------------------------------

export const receiversListResultSchema = z.object({
  receivers: z.array(receiverSchema),
  /** The current join code, or null if none has been minted or it expired. */
  joinCode: joinCodeSchema.nullable(),
  maxReceivers: z.number().int().min(1),
})
export type ReceiversListResult = z.infer<typeof receiversListResultSchema>

export const receiverIdParamsSchema = z.object({
  receiverId: z.string().min(1),
})
export type ReceiverIdParams = z.infer<typeof receiverIdParamsSchema>

export const renameReceiverParamsSchema = z.object({
  receiverId: z.string().min(1),
  displayName: z.string().min(1).max(60),
})
export type RenameReceiverParams = z.infer<typeof renameReceiverParamsSchema>
