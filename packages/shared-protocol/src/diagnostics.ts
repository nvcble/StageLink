import { z } from 'zod'
import { operationalErrorSchema } from './errors'
import { audioCaptureStateSchema } from './audio'
import { streamStateSchema } from './stream'

/**
 * Diagnostics contracts.
 *
 * Every numeric figure the diagnostics page shows is tagged with how it was
 * obtained. This is a hard product requirement: an operator debugging a live
 * service must be able to tell a measurement from a guess, and a desktop-side
 * measurement from something a phone claimed.
 */
export const metricSourceSchema = z.enum([
  /** Counted or timed directly by the Rust service on this machine. */
  'measured',
  /** Derived arithmetically from configuration and measured values. */
  'estimated',
  /** Sent to us by a receiver; unverified. */
  'receiver-reported',
])
export type MetricSource = z.infer<typeof metricSourceSchema>

/** A single tagged scalar. */
export const measurementSchema = z.object({
  value: z.number().nullable(),
  unit: z.string(),
  source: metricSourceSchema,
  /** Shown as a tooltip; explains how the figure was produced. */
  note: z.string().optional(),
})
export type Measurement = z.infer<typeof measurementSchema>

export function measured(value: number | null, unit: string, note?: string): Measurement {
  return { value, unit, source: 'measured', ...(note ? { note } : {}) }
}
export function estimated(value: number | null, unit: string, note?: string): Measurement {
  return { value, unit, source: 'estimated', ...(note ? { note } : {}) }
}
export function receiverReported(value: number | null, unit: string, note?: string): Measurement {
  return { value, unit, source: 'receiver-reported', ...(note ? { note } : {}) }
}

export const audioDiagnosticsSchema = z.object({
  captureState: audioCaptureStateSchema,
  framesCaptured: z.number().int().min(0),
  /** Capture ring buffer overflowed: the consumer could not keep up. */
  bufferOverruns: z.number().int().min(0),
  /** Capture ring buffer ran dry: the producer fell behind. */
  bufferUnderruns: z.number().int().min(0),
  /**
   * Times the capture backlog was trimmed to hold latency down.
   *
   * Not a fault in itself — it is latency being paid back rather than carried
   * for the rest of the session — but it only happens when this machine failed
   * to keep up with the send cadence, and the audio either side of a trim is
   * not continuous. A figure that keeps climbing is the one to act on.
   */
  latencyTrims: z.number().int().min(0),
  /** Audio those trims discarded, in milliseconds. */
  latencyTrimmedMs: z.number().min(0),
  /** Driver-reported discontinuities. */
  xruns: z.number().int().min(0),
  /** Worst observed capture-callback duration, microseconds. */
  callbackMaxUs: z.number().min(0),
  /** Rolling mean capture-callback duration, microseconds. */
  callbackAvgUs: z.number().min(0),
  clipCount: z.number().int().min(0),
})
export type AudioDiagnostics = z.infer<typeof audioDiagnosticsSchema>

export const streamDiagnosticsSchema = z.object({
  state: streamStateSchema,
  packetsSent: z.number().int().min(0),
  bytesSent: z.number().int().min(0),
  sendFailures: z.number().int().min(0),
  packetsDropped: z.number().int().min(0),
  /** Deepest per-receiver queue seen since the stream started. */
  queueDepthMax: z.number().int().min(0),
  queueDepthNow: z.number().int().min(0),
  /** Desktop-measured egress over the sampling window. */
  egressBitsPerSecond: z.number().min(0),
})
export type StreamDiagnostics = z.infer<typeof streamDiagnosticsSchema>

/** Aggregates over receiver-reported telemetry. Null when no receiver reported. */
export const receiverAggregateSchema = z.object({
  connectedCount: z.number().int().min(0),
  streamingCount: z.number().int().min(0),
  worstLossPercent: z.number().min(0).max(100).nullable(),
  meanLossPercent: z.number().min(0).max(100).nullable(),
  worstJitterMs: z.number().min(0).nullable(),
  meanJitterMs: z.number().min(0).nullable(),
  totalUnderruns: z.number().int().min(0),
  minBufferHealthMs: z.number().min(0).nullable(),
})
export type ReceiverAggregate = z.infer<typeof receiverAggregateSchema>

export const systemDiagnosticsSchema = z.object({
  /** StageLink service process CPU, percent of one core. */
  processCpuPercent: z.number().min(0).nullable(),
  /** Whole-machine CPU, percent. */
  systemCpuPercent: z.number().min(0).max(100).nullable(),
  /** Service resident set size, bytes. */
  processMemoryBytes: z.number().int().min(0).nullable(),
  systemMemoryTotalBytes: z.number().int().min(0).nullable(),
  systemMemoryAvailableBytes: z.number().int().min(0).nullable(),
  /** Whether the capture thread actually got a real-time priority bump. */
  realtimePriorityGranted: z.boolean(),
})
export type SystemDiagnostics = z.infer<typeof systemDiagnosticsSchema>

/**
 * Latency breakdown.
 *
 * Only the desktop-side contributions can be known here. The figure an operator
 * actually cares about — mixer input to earphone — can only be obtained with a
 * physical loopback measurement (see docs/latency-measurement.md). We therefore
 * publish the parts we know, clearly marked, and refuse to sum them into a
 * headline "total latency" number.
 */
export const latencyBreakdownSchema = z.object({
  /** Capture period granted by the driver. */
  captureBufferMs: measurementSchema,
  /** Packetiser frame duration. */
  packetFrameMs: measurementSchema,
  /** Time audio spends in the send queue, measured. */
  sendQueueMs: measurementSchema,
  /** Sum of the desktop-side contributions above. */
  desktopSubtotalMs: measurementSchema,
  /**
   * Network one-way delay, if the control channel has estimated it. This is
   * NOT audio latency and must never be displayed as such.
   */
  controlChannelRttMs: measurementSchema,
  /** Receiver's own reported jitter-buffer + output latency, if available. */
  receiverReportedOutputMs: measurementSchema,
  /** Plain-language caveat rendered beside the breakdown. */
  disclaimer: z.string(),
})
export type LatencyBreakdown = z.infer<typeof latencyBreakdownSchema>

export const diagnosticsSnapshotSchema = z.object({
  at: z.number().int(),
  serviceUptimeMs: z.number().int().min(0),
  serviceVersion: z.string(),
  audio: audioDiagnosticsSchema,
  stream: streamDiagnosticsSchema,
  receivers: receiverAggregateSchema,
  system: systemDiagnosticsSchema,
  latency: latencyBreakdownSchema,
  /** Most recent operational errors, newest first. */
  recentErrors: z.array(operationalErrorSchema),
})
export type DiagnosticsSnapshot = z.infer<typeof diagnosticsSnapshotSchema>

/** Default diagnostics push cadence. Low enough to stay off the hot path. */
export const DIAGNOSTICS_EVENT_HZ = 2 as const

export const diagnosticsSubscribeParamsSchema = z.object({
  /** Push snapshots automatically while true. */
  enabled: z.boolean(),
  intervalMs: z.number().int().min(200).max(10_000).default(500),
})
export type DiagnosticsSubscribeParams = z.infer<typeof diagnosticsSubscribeParamsSchema>
