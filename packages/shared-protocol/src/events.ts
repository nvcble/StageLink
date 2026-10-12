import { z } from 'zod'
import { audioLevelsSchema, audioStatusSchema } from './audio'
import { diagnosticsSnapshotSchema } from './diagnostics'
import { networkStatusSchema } from './network'
import { pairingRequestSchema, receiverSchema } from './receivers'
import { logRecordSchema, serviceInfoSchema } from './service'
import { sessionEventSchema, sessionSchema } from './session'
import { streamStatusSchema } from './stream'
import { operationalErrorSchema } from './errors'

/**
 * Service -> renderer push events.
 *
 * Cadence matters here. `audio.levels` is the only high-rate event and it is
 * throttled *in Rust* (METER_EVENT_HZ) rather than in the renderer, so the
 * Electron IPC channel never carries per-buffer traffic. Audio samples
 * themselves are never sent over this channel at any rate.
 */
export const EVENTS = {
  /** Throttled metering frame. ~25 Hz. */
  'audio.levels': audioLevelsSchema,
  /** Capture state change: device opened, lost, reconnected, errored. */
  'audio.status': audioStatusSchema,
  /** Stream state change plus refreshed transmission counters. */
  'stream.status': streamStatusSchema,
  /** The receiver roster changed (join, leave, rename, state change). */
  'receivers.changed': z.object({ receivers: z.array(receiverSchema) }),
  /** A device is asking to pair and needs operator attention. */
  'receivers.pairingRequest': pairingRequestSchema,
  /** A pending request expired or was handled elsewhere. */
  'receivers.pairingResolved': z.object({
    requestId: z.string(),
    outcome: z.enum(['approved', 'rejected', 'expired']),
  }),
  /** Adapter list, bound address, discovery state or warnings changed. */
  'network.status': networkStatusSchema,
  /** Session state machine transitioned. */
  'session.state': sessionSchema,
  /** A new entry was appended to the session event log. */
  'session.event': sessionEventSchema,
  /** Periodic diagnostics push while subscribed. ~2 Hz. */
  'diagnostics.snapshot': diagnosticsSnapshotSchema,
  /** Supervision state of the native service changed. */
  'service.state': serviceInfoSchema,
  /** Forwarded service log line (respecting the configured level). */
  'service.log': logRecordSchema,
  /** An operational fault was raised and should surface in the UI. */
  'service.error': operationalErrorSchema,
} as const satisfies Record<string, z.ZodType>

export type EventRegistry = typeof EVENTS
export type EventName = keyof EventRegistry
export type EventPayload<E extends EventName> = z.output<EventRegistry[E]>

export const EVENT_NAMES = Object.keys(EVENTS) as EventName[]

export function isEventName(value: string): value is EventName {
  return Object.prototype.hasOwnProperty.call(EVENTS, value)
}

export function eventSchema<E extends EventName>(event: E): EventRegistry[E] {
  return EVENTS[event]
}

/**
 * Events whose payload fully replaces prior state (as opposed to being an
 * append-only notification). Stores use this to decide between `set` and
 * `push` semantics.
 */
export const SNAPSHOT_EVENTS = [
  'audio.status',
  'stream.status',
  'receivers.changed',
  'network.status',
  'session.state',
  'diagnostics.snapshot',
  'service.state',
] as const satisfies readonly EventName[]

export type SnapshotEventName = (typeof SNAPSHOT_EVENTS)[number]
