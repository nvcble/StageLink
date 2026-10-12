import { z } from 'zod'

/**
 * Audio streaming contracts.
 *
 * One mono PCM stream is multicast-by-unicast to every authorised receiver.
 * Every receiver gets identical audio; all personal adjustment (volume, EQ,
 * limiter, mute) happens on the receiver.
 */

/**
 * Wire transport for audio payloads.
 *
 * - `udp-plaintext` is the development / proof-of-concept transport. It is
 *   refused when the build is a production release (see
 *   `isTransportAllowedInProduction`).
 * - `udp-srtp-like` is the authenticated+encrypted production transport: an
 *   AEAD (ChaCha20-Poly1305) over the audio payload with per-session keys
 *   derived from the paired control-channel handshake. The header stays in the
 *   clear so receivers can reorder and detect loss before decryption.
 */
export const streamTransportSchema = z.enum(['udp-plaintext', 'udp-encrypted'])
export type StreamTransport = z.infer<typeof streamTransportSchema>

/** Only PCM S16LE is defined for v1 of the audio protocol. */
export const payloadFormatSchema = z.enum(['pcm_s16le'])
export type PayloadFormat = z.infer<typeof payloadFormatSchema>

/**
 * Frame durations the packetiser supports, in milliseconds.
 *
 * Capped at 10 ms deliberately: one packet must fit in a single 1500-byte
 * datagram so nothing is ever IP-fragmented, and 20 ms of mono S16 at 48 kHz is
 * 1920 payload bytes — over the limit. 10 ms is 960 bytes and fits with room
 * for the header and the AEAD tag. `tests/protocol` asserts this holds for
 * every entry here.
 */
export const SUPPORTED_FRAME_DURATIONS_MS = [2.5, 5, 10] as const
export type FrameDurationMs = (typeof SUPPORTED_FRAME_DURATIONS_MS)[number]

export const frameDurationSchema = z
  .number()
  .refine(
    (value): value is FrameDurationMs =>
      (SUPPORTED_FRAME_DURATIONS_MS as readonly number[]).includes(value),
    { message: `Frame duration must be one of ${SUPPORTED_FRAME_DURATIONS_MS.join(', ')} ms.` },
  )

export const DEFAULT_SAMPLE_RATE = 48_000 as const
export const DEFAULT_FRAME_DURATION_MS = 5 as const
export const DEFAULT_STREAM_PORT = 45_872 as const
export const DEFAULT_CONTROL_PORT = 45_871 as const
export const DEFAULT_MAX_RECEIVERS = 16 as const

export const streamConfigSchema = z.object({
  sampleRate: z.number().int().positive().default(DEFAULT_SAMPLE_RATE),
  /** Packet frame duration. Smaller = lower latency, less loss tolerance. */
  frameDurationMs: frameDurationSchema.default(DEFAULT_FRAME_DURATION_MS),
  payloadFormat: payloadFormatSchema.default('pcm_s16le'),
  transport: streamTransportSchema,
  /** Local address to bind the audio socket to. Chosen on the Network page. */
  bindAddress: z.string().min(1),
  streamPort: z.number().int().min(1024).max(65_535).default(DEFAULT_STREAM_PORT),
  controlPort: z.number().int().min(1024).max(65_535).default(DEFAULT_CONTROL_PORT),
  maxReceivers: z.number().int().min(1).max(64).default(DEFAULT_MAX_RECEIVERS),
  /**
   * Per-receiver send queue capacity in packets. A receiver that fills its
   * queue gets packets dropped — it must never stall capture or peers.
   */
  perReceiverQueuePackets: z.number().int().min(2).max(512).default(64),
})

export type StreamConfig = z.infer<typeof streamConfigSchema>

export const streamStateSchema = z.enum(['stopped', 'starting', 'running', 'stopping', 'error'])
export type StreamState = z.infer<typeof streamStateSchema>

export const streamStatusSchema = z.object({
  state: streamStateSchema,
  config: streamConfigSchema.nullable(),
  sessionId: z.string().nullable(),
  startedAt: z.number().int().nullable(),
  /** Audio datagrams handed to the OS socket since the stream started. */
  packetsSent: z.number().int().min(0),
  bytesSent: z.number().int().min(0),
  /** Datagrams the OS refused (EWOULDBLOCK, unreachable, ...). */
  sendFailures: z.number().int().min(0),
  /** Packets dropped because a receiver's queue was full. */
  packetsDropped: z.number().int().min(0),
  /** Deepest per-receiver queue right now, in packets. */
  queueDepth: z.number().int().min(0),
  activeReceivers: z.number().int().min(0),
  lastError: z.string().nullable(),
})

export type StreamStatus = z.infer<typeof streamStatusSchema>

// ---------------------------------------------------------------------------
// Derived helpers — shared so Rust, Electron and the UI agree on arithmetic
// ---------------------------------------------------------------------------

/** Samples per packet for a given rate and frame duration. */
export function framesPerPacket(sampleRate: number, frameDurationMs: number): number {
  return Math.round((sampleRate * frameDurationMs) / 1000)
}

/** Audio payload bytes per packet (mono S16LE). */
export function payloadBytesPerPacket(sampleRate: number, frameDurationMs: number): number {
  return framesPerPacket(sampleRate, frameDurationMs) * 2
}

/** Packets per second on the wire. */
export function packetsPerSecond(frameDurationMs: number): number {
  return 1000 / frameDurationMs
}

/**
 * Egress bitrate in bits/sec for `receivers` unicast copies, including the
 * StageLink header plus UDP/IP/Ethernet overhead.
 */
export function estimatedBitrateBps(
  sampleRate: number,
  frameDurationMs: number,
  receivers: number,
  headerBytes: number,
): number {
  const perPacket = payloadBytesPerPacket(sampleRate, frameDurationMs) + headerBytes + 42
  return perPacket * 8 * packetsPerSecond(frameDurationMs) * receivers
}

/**
 * Production builds must not stream audio in the clear. Development builds may,
 * so the headless test receiver can validate framing without key exchange.
 */
export function isTransportAllowedInProduction(transport: StreamTransport): boolean {
  return transport === 'udp-encrypted'
}

// ---------------------------------------------------------------------------
// Method params
// ---------------------------------------------------------------------------

export const streamStartParamsSchema = z.object({
  /** Partial overrides merged over the persisted stream configuration. */
  config: streamConfigSchema.partial().optional(),
  /** Optional operator label for the session, e.g. "Sunday 1st service". */
  sessionLabel: z.string().max(120).optional(),
})
export type StreamStartParams = z.infer<typeof streamStartParamsSchema>

export const streamStopParamsSchema = z.object({
  /**
   * Stop immediately instead of letting queued packets drain. Draining is the
   * default so receivers hear the tail of the mix rather than a hard cut.
   */
  immediate: z.boolean().default(false),
})
export type StreamStopParams = z.infer<typeof streamStopParamsSchema>
