import { z } from 'zod'
import { serviceErrorSchema } from './errors'
import { CONTROL_PROTOCOL_VERSION, IPC_ENVELOPE_VERSION } from './version'

/**
 * Wire framing for the Electron-main <-> Rust-service control channel.
 *
 * Transport: a Windows named pipe, newline-delimited JSON (NDJSON). One JSON
 * object per line, UTF-8, no embedded newlines. The pipe is created by the
 * service with a DACL restricted to the current user, and the first line the
 * client sends must be a `hello` carrying the one-time token that Electron
 * passed to the service on spawn. Audio never travels over this channel.
 */

export const MAX_ENVELOPE_BYTES = 1_048_576 as const

/**
 * Self-description a receiver sends when it joins.
 *
 * Everything here is self-reported and untrusted; the only field the desktop
 * can vouch for is the source address it observed.
 */
export const receiverIdentitySchema = z.object({
  deviceName: z.string().min(1).max(120),
  platform: z.enum(['android', 'ios', 'windows', 'macos', 'linux', 'headless-test', 'unknown']),
  platformVersion: z.string().max(60).nullable(),
  appVersion: z.string().max(60).nullable(),
  /** Long-term public key, hex. Lets the device reconnect without a new code. */
  publicKey: z.string().min(1).max(256),
  /**
   * UDP port this receiver has **actually bound** for audio.
   *
   * Optional, and absent means "use the configured stream port" — which is
   * what an older receiver implies. Sending it is better: the desktop then
   * needs no assumption about where a receiver is listening, and two
   * receivers sharing one IP address (two devices behind the same NAT, or two
   * processes on one machine) do not collide on a single port.
   */
  audioPort: z.number().int().min(1).max(65_535).optional(),
})
export type ReceiverIdentity = z.infer<typeof receiverIdentitySchema>

/**
 * Client -> service: authenticate and agree a protocol version.
 *
 * A receiver joins with this same message, putting its join token in `token`
 * and its self-description in `receiver`. There is deliberately no separate
 * `join.request` method: the envelope contract requires `hello` to be the first
 * message and to carry the credential, so a second credential-bearing frame
 * could never be first. One round trip, and the token is already in the field
 * that is compared in constant time.
 */
export const helloEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('hello'),
  /**
   * For the local pipe, the one-time secret handed to the service at spawn.
   * For a receiver, the join token from the QR code.
   */
  token: z.string().min(16),
  /** Informational client identity, e.g. "stagelink-desktop/0.1.0". */
  client: z.string().min(1),
  controlProtocolVersion: z.string().min(1),
  /** Present only when a receiver is joining. */
  receiver: receiverIdentitySchema.optional(),
})
export type HelloEnvelope = z.infer<typeof helloEnvelopeSchema>

/**
 * Audio parameters an admitted receiver needs in order to play correctly.
 *
 * **This is the only place the sample rate is communicated.** The packet header
 * carries `channels` and `payloadFormat` but no rate, while the sender permits
 * 8–192 kHz — so a receiver that assumed 48 kHz and got 44.1 kHz would play a
 * semitone and a half sharp, in a performer's ears, live.
 *
 * A conforming receiver must *also* infer the rate independently, as
 * `framesPerPacket / (Δ timestampUs / 1e6)` across two consecutive in-order
 * packets, and refuse to play if the two disagree. Trusting one source for
 * something this audible is not enough.
 *
 * Re-sent on any `sessionId` change, because the rate may change with it.
 */
export const streamParamsSchema = z.object({
  /** Matches the `sessionId` in the audio packet headers. */
  sessionId: z.number().int(),
  sampleRate: z.number().int().positive(),
  frameDurationMs: z.number().positive(),
  payloadFormat: z.literal('pcm_s16le'),
  transport: z.enum(['udp-plaintext', 'udp-encrypted']),
  /** UDP port audio arrives on. */
  streamPort: z.number().int().min(1),
  /** Frames per packet, so the receiver can size its slots before any audio. */
  framesPerPacket: z.number().int().positive(),
})
export type StreamParams = z.infer<typeof streamParamsSchema>

/** Service -> client: handshake accepted. */
export const welcomeEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('welcome'),
  serviceVersion: z.string(),
  controlProtocolVersion: z.string(),
  audioProtocolVersion: z.number().int(),
  availableAudioApis: z.array(z.enum(['wasapi', 'asio'])),
  pid: z.number().int(),
  /** Present only when a receiver was admitted. */
  stream: streamParamsSchema.optional(),
  /**
   * Fingerprint of the desktop's long-term key. A receiver pins this; a change
   * means a different machine and must force a fresh join code.
   */
  keyFingerprint: z.string().optional(),
})
export type WelcomeEnvelope = z.infer<typeof welcomeEnvelopeSchema>

export const requestEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('request'),
  /** Correlation id, unique per connection. */
  id: z.string().min(1).max(64),
  method: z.string().min(1).max(64),
  /** Validated against the method's own schema once `method` is known. */
  params: z.unknown(),
})
export type RequestEnvelope = z.infer<typeof requestEnvelopeSchema>

export const responseEnvelopeSchema = z.discriminatedUnion('ok', [
  z.object({
    v: z.literal(IPC_ENVELOPE_VERSION),
    kind: z.literal('response'),
    id: z.string().min(1),
    ok: z.literal(true),
    result: z.unknown(),
  }),
  z.object({
    v: z.literal(IPC_ENVELOPE_VERSION),
    kind: z.literal('response'),
    id: z.string().min(1),
    ok: z.literal(false),
    error: serviceErrorSchema,
  }),
])
export type ResponseEnvelope = z.infer<typeof responseEnvelopeSchema>

export const eventEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('event'),
  event: z.string().min(1).max(64),
  payload: z.unknown(),
  at: z.number().int(),
})
export type EventEnvelope = z.infer<typeof eventEnvelopeSchema>

/**
 * A plain union rather than a `discriminatedUnion` on `kind`: the two response
 * variants share `kind: 'response'` and are themselves discriminated on `ok`,
 * which a single-level discriminated union cannot express.
 */
export const envelopeSchema = z.union([
  helloEnvelopeSchema,
  welcomeEnvelopeSchema,
  requestEnvelopeSchema,
  responseEnvelopeSchema,
  eventEnvelopeSchema,
])
export type Envelope = z.infer<typeof envelopeSchema>

// ---------------------------------------------------------------------------
// Constructors — keep the version literal in exactly one place
// ---------------------------------------------------------------------------

export function makeHello(token: string, client: string): HelloEnvelope {
  return {
    v: IPC_ENVELOPE_VERSION,
    kind: 'hello',
    token,
    client,
    controlProtocolVersion: CONTROL_PROTOCOL_VERSION,
  }
}

export function makeRequest(id: string, method: string, params: unknown): RequestEnvelope {
  return { v: IPC_ENVELOPE_VERSION, kind: 'request', id, method, params }
}

export function makeOkResponse(id: string, result: unknown): ResponseEnvelope {
  return { v: IPC_ENVELOPE_VERSION, kind: 'response', id, ok: true, result }
}

export function makeErrorResponse(
  id: string,
  error: z.infer<typeof serviceErrorSchema>,
): ResponseEnvelope {
  return { v: IPC_ENVELOPE_VERSION, kind: 'response', id, ok: false, error }
}

export function makeEvent(event: string, payload: unknown, at = Date.now()): EventEnvelope {
  return { v: IPC_ENVELOPE_VERSION, kind: 'event', event, payload, at }
}

/** Serialises an envelope to a single NDJSON line, including the terminator. */
export function encodeEnvelope(envelope: Envelope): string {
  return `${JSON.stringify(envelope)}\n`
}

export type DecodeResult =
  | { ok: true; envelope: Envelope }
  | { ok: false; reason: string }

/** Parses one NDJSON line. Never throws. */
export function decodeEnvelope(line: string): DecodeResult {
  if (line.length > MAX_ENVELOPE_BYTES) {
    return { ok: false, reason: `Envelope exceeds ${MAX_ENVELOPE_BYTES} bytes.` }
  }
  let json: unknown
  try {
    json = JSON.parse(line)
  } catch (cause) {
    return { ok: false, reason: `Malformed JSON: ${(cause as Error).message}` }
  }
  const parsed = envelopeSchema.safeParse(json)
  if (!parsed.success) {
    return { ok: false, reason: parsed.error.issues.map((i) => i.message).join('; ') }
  }
  return { ok: true, envelope: parsed.data }
}
