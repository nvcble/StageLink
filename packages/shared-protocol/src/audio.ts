import { z } from 'zod'

/**
 * Audio domain contracts.
 *
 * StageLink captures exactly ONE mono channel from a multi-channel USB audio
 * interface: the FOH monitor/AUX send. Everything downstream is mono. There are
 * no per-instrument channels in the signal, so no per-instrument contracts
 * exist here by design.
 */

/** Host audio API used for capture. */
export const audioApiSchema = z.enum(['wasapi', 'asio'])
export type AudioApi = z.infer<typeof audioApiSchema>

/** WASAPI sharing mode. Exclusive gives lower latency but locks the device. */
export const wasapiShareModeSchema = z.enum(['shared', 'exclusive'])
export type WasapiShareMode = z.infer<typeof wasapiShareModeSchema>

/** Windows device presence, as reported by the OS endpoint enumerator. */
export const audioDeviceStateSchema = z.enum(['active', 'disabled', 'unplugged', 'notPresent'])
export type AudioDeviceState = z.infer<typeof audioDeviceStateSchema>

/** Internal DSP sample format. StageLink always processes in f32. */
export const sampleFormatSchema = z.enum(['f32', 'i32', 'i24', 'i16'])
export type SampleFormat = z.infer<typeof sampleFormatSchema>

export const audioDeviceSchema = z.object({
  /** Stable OS endpoint id. Persisted in settings, survives reboots. */
  id: z.string().min(1),
  /** Friendly name as shown by Windows, e.g. "Line 1/2 (Focusrite USB)". */
  name: z.string().min(1),
  api: audioApiSchema,
  isDefault: z.boolean(),
  state: audioDeviceStateSchema,
  /**
   * Physical input channels the endpoint exposes.
   *
   * Zero for a device that is present but not active — an unplugged interface
   * cannot be activated, so its channel count is genuinely unknown.
   */
  inputChannels: z.number().int().min(0),
  /**
   * Device mix-format sample rate reported by the driver.
   *
   * `null` when the endpoint could not be activated to ask: a disabled,
   * unplugged or absent device has no mix format. Null rather than 0, because
   * "0 Hz" is not a sample rate and a UI that divides by it produces nonsense.
   */
  defaultSampleRate: z.number().int().positive().nullable(),
  /** Rates the driver accepted during probing. Empty means "not probed". */
  supportedSampleRates: z.array(z.number().int().positive()).default([]),
  /** Sample format of the device mix format. `null` when not activatable. */
  defaultSampleFormat: sampleFormatSchema.nullable(),
})

export type AudioDevice = z.infer<typeof audioDeviceSchema>

/** The negotiated capture format actually in use. */
export const captureFormatSchema = z.object({
  sampleRate: z.number().int().positive(),
  /** Channel count the device stream delivers (we extract one of these). */
  deviceChannels: z.number().int().positive(),
  /** Sample format the device delivers before conversion to f32. */
  deviceSampleFormat: sampleFormatSchema,
  /** Capture buffer period in frames, as granted by the driver. */
  periodFrames: z.number().int().positive(),
  shareMode: wasapiShareModeSchema,
})

export type CaptureFormat = z.infer<typeof captureFormatSchema>

export const audioCaptureStateSchema = z.enum([
  'idle',
  'opening',
  'running',
  'reconnecting',
  'error',
])
export type AudioCaptureState = z.infer<typeof audioCaptureStateSchema>

export const audioStatusSchema = z.object({
  state: audioCaptureStateSchema,
  device: audioDeviceSchema.nullable(),
  /** Zero-based index of the physical input channel being captured. */
  channelIndex: z.number().int().min(0).nullable(),
  format: captureFormatSchema.nullable(),
  /** Total frames pulled from the driver since capture started. */
  framesCaptured: z.number().int().min(0),
  /** Driver glitch count (dropped/discontinuous buffers) since start. */
  xruns: z.number().int().min(0),
  /** True while the OS still reports the endpoint as present. */
  deviceConnected: z.boolean(),
  startedAt: z.number().int().nullable(),
  lastError: z.string().nullable(),
})

export type AudioStatus = z.infer<typeof audioStatusSchema>

/**
 * Metering frame. Emitted by the Rust service on a throttled cadence
 * (`METER_EVENT_HZ`), computed from every captured sample — not sampled from
 * the UI and never synthesised.
 *
 * All level fields are dBFS; negative, with 0.0 meaning full scale.
 * `-Infinity` is serialised as `null` for digital silence.
 */
export const audioLevelsSchema = z.object({
  at: z.number().int(),
  /** RMS over the reporting window. */
  rmsDb: z.number().nullable(),
  /** True peak over the reporting window. */
  peakDb: z.number().nullable(),
  /** Decaying peak-hold value for the meter's hold indicator. */
  peakHoldDb: z.number().nullable(),
  /** True if any sample in the window reached or exceeded full scale. */
  clipping: z.boolean(),
  /** Cumulative count of clipped samples since capture started. */
  clipCount: z.number().int().min(0),
  /** Frames represented by this window — lets the UI detect gaps. */
  windowFrames: z.number().int().min(0),
})

export type AudioLevels = z.infer<typeof audioLevelsSchema>

/** How often the service emits `audio.levels`. Decoupled from capture rate. */
export const METER_EVENT_HZ = 25 as const

/** Peak-hold decay applied by the service, in dB per second. */
export const METER_PEAK_DECAY_DB_PER_SEC = 20 as const

// ---------------------------------------------------------------------------
// Method params / results
// ---------------------------------------------------------------------------

export const listDevicesParamsSchema = z.object({
  /** Re-probe supported sample rates. Slower; off by default. */
  probeFormats: z.boolean().default(false),
  api: audioApiSchema.optional(),
})
export type ListDevicesParams = z.infer<typeof listDevicesParamsSchema>

export const listDevicesResultSchema = z.object({
  devices: z.array(audioDeviceSchema),
  /** APIs this build was compiled with and found usable at runtime. */
  availableApis: z.array(audioApiSchema),
})
export type ListDevicesResult = z.infer<typeof listDevicesResultSchema>

export const selectDeviceParamsSchema = z.object({
  deviceId: z.string().min(1),
  /** Physical input channel to capture as the mono monitor mix. */
  channelIndex: z.number().int().min(0),
  /** Preferred rate. The service negotiates and reports what it got. */
  sampleRate: z.number().int().positive().optional(),
  shareMode: wasapiShareModeSchema.default('shared'),
})
export type SelectDeviceParams = z.infer<typeof selectDeviceParamsSchema>

export const audioTestParamsSchema = z.object({
  /** How long to listen while evaluating the input, in milliseconds. */
  durationMs: z.number().int().min(250).max(30_000).default(3_000),
})
export type AudioTestParams = z.infer<typeof audioTestParamsSchema>

export const audioTestResultSchema = z.object({
  /** True when a signal above the noise floor was detected. */
  signalDetected: z.boolean(),
  peakDb: z.number().nullable(),
  rmsDb: z.number().nullable(),
  clipping: z.boolean(),
  framesAnalysed: z.number().int().min(0),
  /** Operator-facing verdict, e.g. "Signal present at a healthy level." */
  verdict: z.string(),
})
export type AudioTestResult = z.infer<typeof audioTestResultSchema>
