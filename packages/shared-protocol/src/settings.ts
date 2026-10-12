import { z } from 'zod'
import {
  DEFAULT_CONTROL_PORT,
  DEFAULT_FRAME_DURATION_MS,
  DEFAULT_MAX_RECEIVERS,
  DEFAULT_SAMPLE_RATE,
  DEFAULT_STREAM_PORT,
  frameDurationSchema,
  payloadFormatSchema,
  streamTransportSchema,
} from './stream'
import { wasapiShareModeSchema } from './audio'

/**
 * Persisted application settings.
 *
 * These live in SQLite and are validated on read *and* write — a hand-edited or
 * migrated-forward database must never be able to put the service into an
 * invalid configuration.
 */

export const audioSettingsSchema = z.object({
  /** Remembered endpoint id. Null until the operator picks one. */
  deviceId: z.string().nullable().default(null),
  /** Remembered friendly name, used to explain "your device is missing". */
  deviceName: z.string().nullable().default(null),
  channelIndex: z.number().int().min(0).default(0),
  sampleRate: z.number().int().positive().default(DEFAULT_SAMPLE_RATE),
  shareMode: wasapiShareModeSchema.default('shared'),
  /** Reopen the capture device automatically when it reappears. */
  autoReconnect: z.boolean().default(true),
  /** dBFS threshold above which the dashboard shows a "hot signal" warning. */
  hotSignalWarningDb: z.number().min(-24).max(0).default(-3),
})
export type AudioSettings = z.infer<typeof audioSettingsSchema>

export const networkSettingsSchema = z.object({
  adapterId: z.string().nullable().default(null),
  bindAddress: z.string().nullable().default(null),
  streamPort: z.number().int().min(1024).max(65_535).default(DEFAULT_STREAM_PORT),
  controlPort: z.number().int().min(1024).max(65_535).default(DEFAULT_CONTROL_PORT),
  /** Advertise the service over mDNS/DNS-SD. */
  discoveryEnabled: z.boolean().default(true),
  /** Instance name used in the DNS-SD record. */
  discoveryInstanceName: z.string().min(1).max(63).default('StageLink Core'),
})
export type NetworkSettings = z.infer<typeof networkSettingsSchema>

export const streamingSettingsSchema = z.object({
  frameDurationMs: frameDurationSchema.default(DEFAULT_FRAME_DURATION_MS),
  payloadFormat: payloadFormatSchema.default('pcm_s16le'),
  transport: streamTransportSchema.default('udp-encrypted'),
  maxReceivers: z.number().int().min(1).max(64).default(DEFAULT_MAX_RECEIVERS),
  perReceiverQueuePackets: z.number().int().min(2).max(512).default(64),
  /** Begin streaming as soon as capture is healthy at app start. */
  autoStartStreaming: z.boolean().default(false),
  /** Require operator approval for every new receiver, even trusted ones. */
  requireApprovalEveryJoin: z.boolean().default(false),
})
export type StreamingSettings = z.infer<typeof streamingSettingsSchema>

export const appearanceSettingsSchema = z.object({
  /** StageLink ships dark-first; light is offered for bright-stage laptops. */
  theme: z.enum(['dark', 'light', 'system']).default('dark'),
  /** Meter ballistics preset. */
  meterStyle: z.enum(['peak', 'rms', 'peak+rms']).default('peak+rms'),
  reduceMotion: z.boolean().default(false),
  /** Renderer meter redraw rate; independent of the service's meter cadence. */
  meterRefreshHz: z.number().int().min(10).max(60).default(25),
})
export type AppearanceSettings = z.infer<typeof appearanceSettingsSchema>

export const startupSettingsSchema = z.object({
  launchOnLogin: z.boolean().default(false),
  startMinimisedToTray: z.boolean().default(false),
  /** Show the first-run wizard. Cleared once setup completes. */
  showSetupWizard: z.boolean().default(true),
  /** Warn before quitting while a session is live. */
  confirmQuitDuringSession: z.boolean().default(true),
})
export type StartupSettings = z.infer<typeof startupSettingsSchema>

export const loggingSettingsSchema = z.object({
  level: z.enum(['error', 'warn', 'info', 'debug', 'trace']).default('info'),
  /** Keep this many rotated service log files. */
  retainFiles: z.number().int().min(1).max(60).default(7),
  /** Persist diagnostics snapshots to disk for later analysis. */
  persistDiagnostics: z.boolean().default(false),
})
export type LoggingSettings = z.infer<typeof loggingSettingsSchema>

export const diagnosticsSettingsSchema = z.object({
  /** Subscribe to diagnostics pushes while the Diagnostics page is open. */
  streamWhileVisible: z.boolean().default(true),
  intervalMs: z.number().int().min(200).max(10_000).default(500),
  /** Points retained per chart series in the renderer. */
  chartHistoryPoints: z.number().int().min(30).max(3_600).default(300),
})
export type DiagnosticsSettings = z.infer<typeof diagnosticsSettingsSchema>

export const appSettingsSchema = z.object({
  audio: audioSettingsSchema.default(() => audioSettingsSchema.parse({})),
  network: networkSettingsSchema.default(() => networkSettingsSchema.parse({})),
  streaming: streamingSettingsSchema.default(() => streamingSettingsSchema.parse({})),
  appearance: appearanceSettingsSchema.default(() => appearanceSettingsSchema.parse({})),
  startup: startupSettingsSchema.default(() => startupSettingsSchema.parse({})),
  logging: loggingSettingsSchema.default(() => loggingSettingsSchema.parse({})),
  diagnostics: diagnosticsSettingsSchema.default(() => diagnosticsSettingsSchema.parse({})),
})
export type AppSettings = z.infer<typeof appSettingsSchema>

/** A fully-defaulted settings object. */
export function defaultSettings(): AppSettings {
  return appSettingsSchema.parse({})
}

/** Deep-partial patch accepted by `settings.update`. */
export const settingsPatchSchema = z.object({
  audio: audioSettingsSchema.partial().optional(),
  network: networkSettingsSchema.partial().optional(),
  streaming: streamingSettingsSchema.partial().optional(),
  appearance: appearanceSettingsSchema.partial().optional(),
  startup: startupSettingsSchema.partial().optional(),
  logging: loggingSettingsSchema.partial().optional(),
  diagnostics: diagnosticsSettingsSchema.partial().optional(),
})
export type SettingsPatch = z.infer<typeof settingsPatchSchema>

/**
 * Settings whose change cannot take effect until the stream is restarted. The
 * UI uses this list to show "restart required" instead of silently applying a
 * value that the running pipeline is ignoring.
 */
export const RESTART_REQUIRED_PATHS = [
  'audio.deviceId',
  'audio.channelIndex',
  'audio.sampleRate',
  'audio.shareMode',
  'network.adapterId',
  'network.bindAddress',
  'network.streamPort',
  'network.controlPort',
  'streaming.frameDurationMs',
  'streaming.payloadFormat',
  'streaming.transport',
] as const

export type RestartRequiredPath = (typeof RESTART_REQUIRED_PATHS)[number]

/** Returns the restart-requiring paths touched by a patch. */
export function restartRequiredFor(patch: SettingsPatch): RestartRequiredPath[] {
  const touched: RestartRequiredPath[] = []
  for (const path of RESTART_REQUIRED_PATHS) {
    const [group, key] = path.split('.') as [keyof SettingsPatch, string]
    const section = patch[group]
    if (section && Object.prototype.hasOwnProperty.call(section, key)) {
      touched.push(path)
    }
  }
  return touched
}

export const settingsUpdateResultSchema = z.object({
  settings: appSettingsSchema,
  /** Paths that will only take effect after a stream restart. */
  restartRequired: z.array(z.string()),
})
export type SettingsUpdateResult = z.infer<typeof settingsUpdateResultSchema>
