import { z } from 'zod'
import { severitySchema } from './errors'

/**
 * Session state machine.
 *
 * A "session" is one continuous run of the monitor system — typically one
 * service or rehearsal. It owns the stream lifecycle and the receiver roster.
 */

export const sessionStateSchema = z.enum([
  /** Nothing running. */
  'idle',
  /** Opening the capture device and binding sockets. */
  'preparing',
  /** Capture and sockets are up, audio not yet being sent. */
  'ready',
  /** Audio flowing to receivers. */
  'streaming',
  /** Draining queues and closing down. */
  'stopping',
  /** A fault occurred; attempting automatic recovery. */
  'recovering',
  /** Unrecoverable without operator action. */
  'error',
])
export type SessionState = z.infer<typeof sessionStateSchema>

/**
 * Legal transitions. Enforced in Rust (authoritative) and mirrored here so the
 * UI can disable impossible actions instead of letting the user fire a request
 * that is guaranteed to fail.
 */
export const SESSION_TRANSITIONS: Readonly<Record<SessionState, readonly SessionState[]>> = {
  idle: ['preparing'],
  preparing: ['ready', 'error', 'stopping'],
  ready: ['streaming', 'stopping', 'error'],
  streaming: ['stopping', 'recovering', 'error'],
  stopping: ['idle', 'error'],
  recovering: ['streaming', 'ready', 'error', 'stopping'],
  error: ['idle', 'preparing'],
} as const

export function canTransition(from: SessionState, to: SessionState): boolean {
  return SESSION_TRANSITIONS[from].includes(to)
}

/** True when a session is doing work and a second start must be refused. */
export function isSessionActive(state: SessionState): boolean {
  return state !== 'idle' && state !== 'error'
}

export const sessionEventKindSchema = z.enum([
  'session.created',
  'session.state-changed',
  'audio.device-opened',
  'audio.device-lost',
  'audio.device-recovered',
  'stream.started',
  'stream.stopped',
  'receiver.joined',
  'receiver.left',
  'receiver.approved',
  'receiver.rejected',
  'receiver.revoked',
  'receiver.timed-out',
  'network.adapter-changed',
  'error.raised',
])
export type SessionEventKind = z.infer<typeof sessionEventKindSchema>

export const sessionEventSchema = z.object({
  id: z.string(),
  sessionId: z.string(),
  at: z.number().int(),
  kind: sessionEventKindSchema,
  severity: severitySchema,
  message: z.string(),
  /** Structured extras, e.g. `{ receiverId }`. Kept small; persisted as JSON. */
  data: z.record(z.string(), z.unknown()).nullable(),
})
export type SessionEvent = z.infer<typeof sessionEventSchema>

export const sessionSchema = z.object({
  id: z.string().min(1),
  label: z.string().nullable(),
  state: sessionStateSchema,
  createdAt: z.number().int(),
  startedAt: z.number().int().nullable(),
  endedAt: z.number().int().nullable(),
  /** Wall-clock duration so far, or final duration once ended. */
  durationMs: z.number().int().min(0),
  /** Highest simultaneous receiver count observed. */
  peakReceivers: z.number().int().min(0),
  /** Distinct receivers that joined at any point. */
  totalReceivers: z.number().int().min(0),
  lastError: z.string().nullable(),
})
export type Session = z.infer<typeof sessionSchema>

export const sessionHistoryParamsSchema = z.object({
  limit: z.number().int().min(1).max(200).default(50),
  offset: z.number().int().min(0).default(0),
})
export type SessionHistoryParams = z.infer<typeof sessionHistoryParamsSchema>

export const sessionHistoryResultSchema = z.object({
  sessions: z.array(sessionSchema),
  total: z.number().int().min(0),
})
export type SessionHistoryResult = z.infer<typeof sessionHistoryResultSchema>

export const sessionEventsParamsSchema = z.object({
  sessionId: z.string().min(1),
  limit: z.number().int().min(1).max(1_000).default(200),
})
export type SessionEventsParams = z.infer<typeof sessionEventsParamsSchema>
