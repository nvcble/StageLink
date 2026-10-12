import { z } from 'zod'
import { serviceErrorSchema } from './errors'

/**
 * Native-service supervision contracts.
 *
 * The Rust service runs as a child process of the Electron main process and
 * communicates over a Windows named pipe. It is deliberately decoupled from the
 * renderer: reloading the Nuxt window (or an HMR update in development) must
 * never interrupt a live session.
 */

export const serviceStateSchema = z.enum([
  /** Not spawned. */
  'stopped',
  /** Process spawned, pipe not yet connected. */
  'starting',
  /** Pipe connected, authenticating and exchanging versions. */
  'handshaking',
  /** Authenticated and answering requests. */
  'ready',
  /** Connected but reporting a fault (e.g. capture device lost). */
  'degraded',
  /** Backing off before a restart attempt. */
  'restarting',
  /** Gave up after exhausting restart attempts. */
  'failed',
])
export type ServiceState = z.infer<typeof serviceStateSchema>

export const serviceInfoSchema = z.object({
  state: serviceStateSchema,
  /** Service crate version, once the handshake has completed. */
  version: z.string().nullable(),
  controlProtocolVersion: z.string().nullable(),
  audioProtocolVersion: z.number().int().nullable(),
  pid: z.number().int().nullable(),
  startedAt: z.number().int().nullable(),
  /** Consecutive automatic restarts. Reset on a clean, long-lived run. */
  restartCount: z.number().int().min(0),
  lastError: serviceErrorSchema.nullable(),
  /** Audio backends the running binary actually has available. */
  availableAudioApis: z.array(z.enum(['wasapi', 'asio'])).default([]),
})
export type ServiceInfo = z.infer<typeof serviceInfoSchema>

/** True when control requests may be issued. */
export function isServiceUsable(state: ServiceState): boolean {
  return state === 'ready' || state === 'degraded'
}

export const logRecordSchema = z.object({
  at: z.number().int(),
  level: z.enum(['error', 'warn', 'info', 'debug', 'trace']),
  target: z.string(),
  message: z.string(),
})
export type LogRecord = z.infer<typeof logRecordSchema>

/**
 * Shutdown policy, answering the question posed in the product spec: should the
 * audio service outlive the desktop UI?
 *
 * Decision: NO. Quitting StageLink Core performs an orderly shutdown of the
 * service. A monitor feed with no operator UI attached is a hazard — nobody can
 * see clipping, nobody can stop it, and performers wearing IEMs have no way to
 * intervene. The renderer reloading is a different matter and never stops audio.
 *
 * When a session is live, quitting is gated on explicit confirmation and then
 * follows the drain sequence below so receivers get a clean stop rather than a
 * sudden silence.
 */
export const shutdownReasonSchema = z.enum([
  /** Operator chose Quit. */
  'app-quit',
  /** Windows is shutting down or logging off. */
  'os-shutdown',
  /** Supervisor is cycling the service after a fault. */
  'restart',
  /** Operator asked to stop the service from Settings. */
  'operator-request',
])
export type ShutdownReason = z.infer<typeof shutdownReasonSchema>

export const serviceShutdownParamsSchema = z.object({
  reason: shutdownReasonSchema,
  /**
   * Milliseconds allowed to notify receivers, drain send queues and close the
   * capture device before the process is terminated.
   */
  graceMs: z.number().int().min(0).max(10_000).default(1_500),
})
export type ServiceShutdownParams = z.infer<typeof serviceShutdownParamsSchema>

/** Supervisor restart backoff, in milliseconds, indexed by attempt number. */
export const SERVICE_RESTART_BACKOFF_MS = [250, 1_000, 2_500, 5_000, 10_000] as const

/** Attempts before the supervisor gives up and reports `failed`. */
export const SERVICE_MAX_RESTART_ATTEMPTS = SERVICE_RESTART_BACKOFF_MS.length

/** A run lasting at least this long resets `restartCount`. */
export const SERVICE_HEALTHY_RUN_MS = 60_000 as const

/** Default deadline for a control request before `TIMEOUT` is returned. */
export const DEFAULT_REQUEST_TIMEOUT_MS = 5_000 as const

/** Longer deadline for requests that touch audio hardware. */
export const HARDWARE_REQUEST_TIMEOUT_MS = 20_000 as const
