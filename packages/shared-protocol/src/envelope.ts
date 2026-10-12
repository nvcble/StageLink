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

/** Client -> service: authenticate and agree a protocol version. */
export const helloEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('hello'),
  /** One-time secret handed to the service at spawn time. */
  token: z.string().min(16),
  /** Informational client identity, e.g. "stagelink-desktop/0.1.0". */
  client: z.string().min(1),
  controlProtocolVersion: z.string().min(1),
})
export type HelloEnvelope = z.infer<typeof helloEnvelopeSchema>

/** Service -> client: handshake accepted. */
export const welcomeEnvelopeSchema = z.object({
  v: z.literal(IPC_ENVELOPE_VERSION),
  kind: z.literal('welcome'),
  serviceVersion: z.string(),
  controlProtocolVersion: z.string(),
  audioProtocolVersion: z.number().int(),
  availableAudioApis: z.array(z.enum(['wasapi', 'asio'])),
  pid: z.number().int(),
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
