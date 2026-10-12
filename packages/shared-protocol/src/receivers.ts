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

/** Authorisation lifecycle, persisted in SQLite. */
export const pairingStateSchema = z.enum([
  /** Discovered but has never requested pairing. */
  'unpaired',
  /** Awaiting operator approval. */
  'pending',
  /** Trusted; may join sessions without re-approval. */
  'approved',
  /** Explicitly refused this pairing attempt. */
  'rejected',
  /** Was approved, authorisation withdrawn. */
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
// Pairing
// ---------------------------------------------------------------------------

/**
 * A pending pairing request surfaced to the operator. Contains only
 * self-reported data plus the observed source address, which is the one field
 * the desktop can vouch for.
 */
export const pairingRequestSchema = z.object({
  requestId: z.string().min(1),
  receiverId: z.string().min(1),
  deviceName: z.string(),
  platform: receiverPlatformSchema,
  platformVersion: z.string().nullable(),
  appVersion: z.string().nullable(),
  /** Observed source address of the request — desktop-measured. */
  ipAddress: z.string(),
  requestedAt: z.number().int(),
  /** Request expiry; stale requests are discarded. */
  expiresAt: z.number().int(),
  /**
   * Short fingerprint of the receiver's long-term public key, rendered for
   * out-of-band comparison on the device screen.
   */
  keyFingerprint: z.string(),
  /** True when the receiver presented a currently valid pairing code. */
  presentedValidCode: z.boolean(),
})

export type PairingRequest = z.infer<typeof pairingRequestSchema>

export const PAIRING_CODE_TTL_MS = 120_000 as const
export const PAIRING_CODE_LENGTH = 6 as const

export const pairingCodeSchema = z.object({
  /** Numeric code the operator reads out, e.g. "418 902". */
  code: z.string().length(PAIRING_CODE_LENGTH),
  expiresAt: z.number().int(),
  /**
   * Everything a receiver needs to connect, encoded for a QR code. See
   * docs/protocol/pairing.md for the payload grammar.
   */
  qrPayload: z.string(),
})

export type PairingCodeInfo = z.infer<typeof pairingCodeSchema>

// ---------------------------------------------------------------------------
// Method params
// ---------------------------------------------------------------------------

export const receiversListResultSchema = z.object({
  receivers: z.array(receiverSchema),
  pendingRequests: z.array(pairingRequestSchema),
  maxReceivers: z.number().int().min(1),
})
export type ReceiversListResult = z.infer<typeof receiversListResultSchema>

export const receiverIdParamsSchema = z.object({
  receiverId: z.string().min(1),
})
export type ReceiverIdParams = z.infer<typeof receiverIdParamsSchema>

export const approvePairingParamsSchema = z.object({
  requestId: z.string().min(1),
  /** Optional label applied at approval time. */
  displayName: z.string().max(60).optional(),
  /** Persist as trusted so the device reconnects without re-approval. */
  trustPermanently: z.boolean().default(true),
})
export type ApprovePairingParams = z.infer<typeof approvePairingParamsSchema>

export const rejectPairingParamsSchema = z.object({
  requestId: z.string().min(1),
  reason: z.string().max(200).optional(),
})
export type RejectPairingParams = z.infer<typeof rejectPairingParamsSchema>

export const renameReceiverParamsSchema = z.object({
  receiverId: z.string().min(1),
  displayName: z.string().min(1).max(60),
})
export type RenameReceiverParams = z.infer<typeof renameReceiverParamsSchema>
