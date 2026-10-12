/**
 * Meter display logic.
 *
 * The service pushes level frames at 25 Hz and this turns them into bar
 * geometry. It deliberately does *not* animate between frames: a meter that
 * interpolates is showing the operator a level that was never measured, and in
 * a monitoring system that is worse than a slightly steppy bar.
 *
 * The one piece of smoothing is the peak-hold, and even that is computed in the
 * service from real sample peaks rather than invented here.
 */

/** Meter floor. Below this there is nothing useful to show. */
export const METER_FLOOR_DB = -60

/** Top of the scale. Full scale, where clipping begins. */
export const METER_CEILING_DB = 0

/** dBFS above which the bar turns amber. */
export const METER_HOT_DB = -6

/** dBFS above which the bar turns red. */
export const METER_CLIP_WARNING_DB = -1

/** Scale ticks, chosen to match the markings on a typical console meter. */
export const METER_TICKS_DB = [0, -6, -12, -18, -24, -36, -48, -60] as const

export function useAudioMeters() {
  const audio = useAudioStore()

  /**
   * Maps dBFS to a 0–1 bar fraction.
   *
   * A straight linear dB mapping wastes most of the bar on levels nobody looks
   * at, so the scale is expanded towards the top where the operator actually
   * works. `null` (silence or no reading) is an empty bar, never a minimum-width
   * stub that could be mistaken for signal.
   */
  function dbToFraction(db: number | null): number {
    if (db === null) return 0
    const clamped = Math.max(METER_FLOOR_DB, Math.min(METER_CEILING_DB, db))
    const linear = (clamped - METER_FLOOR_DB) / (METER_CEILING_DB - METER_FLOOR_DB)
    // Mild expansion towards the top of the scale.
    return linear ** 0.7
  }

  function fractionToPercent(fraction: number): string {
    return `${(fraction * 100).toFixed(2)}%`
  }

  /** The colour band a level falls into. */
  function bandFor(db: number | null): 'silent' | 'low' | 'mid' | 'high' | 'clip' {
    if (db === null) return 'silent'
    if (db >= METER_CLIP_WARNING_DB) return 'clip'
    if (db >= METER_HOT_DB) return 'high'
    if (db >= -24) return 'mid'
    return 'low'
  }

  const peakDb = computed(() => audio.levels?.peakDb ?? null)
  const rmsDb = computed(() => audio.levels?.rmsDb ?? null)
  const peakHoldDb = computed(() => audio.levels?.peakHoldDb ?? null)

  const peakFraction = computed(() => dbToFraction(peakDb.value))
  const rmsFraction = computed(() => dbToFraction(rmsDb.value))
  const peakHoldFraction = computed(() => dbToFraction(peakHoldDb.value))

  const band = computed(() => bandFor(peakDb.value))

  /** True while the service is reporting frames. */
  const hasSignal = computed(() => peakDb.value !== null)

  /**
   * What the meter should say when it has no level to draw.
   *
   * Three genuinely different situations, and conflating them is how an
   * operator ends up debugging the wrong thing.
   */
  const emptyReason = computed<null | 'no-device' | 'not-capturing' | 'stalled' | 'silent'>(() => {
    if (!audio.selectedDevice) return 'no-device'
    if (!audio.isCapturing) return 'not-capturing'
    if (audio.meterStalled) return 'stalled'
    if (!hasSignal.value) return 'silent'
    return null
  })

  const emptyMessage = computed(() => {
    switch (emptyReason.value) {
      case 'no-device':
        return 'No audio input selected'
      case 'not-capturing':
        return 'Input not open'
      case 'stalled':
        return 'No audio arriving from the interface'
      case 'silent':
        return 'Silent — check the mixer monitor send'
      default:
        return null
    }
  })

  /** Formats a dB value for a numeric readout. */
  function formatDb(db: number | null, digits = 1): string {
    if (db === null) return '—'
    return `${db > -0.05 && db < 0.05 ? '0.0' : db.toFixed(digits)}`
  }

  return {
    peakDb,
    rmsDb,
    peakHoldDb,
    peakFraction,
    rmsFraction,
    peakHoldFraction,
    band,
    hasSignal,
    emptyReason,
    emptyMessage,
    isClipping: computed(() => audio.isClipping),
    clipCount: computed(() => audio.clipCount),
    dbToFraction,
    fractionToPercent,
    bandFor,
    formatDb,
    ticks: METER_TICKS_DB,
  }
}
