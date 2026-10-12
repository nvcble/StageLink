<script setup lang="ts">
/**
 * The input level meter.
 *
 * Shows peak, RMS and a decaying peak-hold, all computed in the Rust service
 * from every captured sample. Three deliberate decisions:
 *
 * 1. **No animation between frames.** A CSS transition on the bar would show a
 *    level that was never measured. The bar steps at the service's 25 Hz, which
 *    reads as smooth to the eye and is honest.
 * 2. **Silence draws nothing.** An empty bar with an explanation, not a
 *    minimum-width stub that could be mistaken for signal.
 * 3. **Green through amber to red**, not the brand gradient. A meter is an
 *    instrument; it uses the convention every engineer already reads.
 */
const props = withDefaults(
  defineProps<{
    orientation?: 'horizontal' | 'vertical'
    /** Show the dB scale ticks. */
    showScale?: boolean
    /** Show the numeric peak/RMS readout. */
    showReadout?: boolean
    height?: number
  }>(),
  { orientation: 'horizontal', showScale: true, showReadout: true, height: 28 },
)

const meters = useAudioMeters()

const barColour = computed(() => {
  switch (meters.band.value) {
    case 'clip':
      return 'var(--color-meter-clip)'
    case 'high':
      return 'var(--color-meter-high)'
    case 'mid':
      return 'var(--color-meter-mid)'
    case 'low':
      return 'var(--color-meter-low)'
    default:
      return 'transparent'
  }
})

const isVertical = computed(() => props.orientation === 'vertical')

/** Peak bar geometry. */
const peakStyle = computed(() => {
  const extent = meters.fractionToPercent(meters.peakFraction.value)
  return isVertical.value
    ? { height: extent, width: '100%', backgroundColor: barColour.value }
    : { width: extent, height: '100%', backgroundColor: barColour.value }
})

/** RMS bar, drawn over the peak bar as a brighter core. */
const rmsStyle = computed(() => {
  const extent = meters.fractionToPercent(meters.rmsFraction.value)
  return isVertical.value ? { height: extent } : { width: extent }
})

/** Peak-hold marker. */
const holdStyle = computed(() => {
  if (meters.peakHoldDb.value === null) return { display: 'none' }
  const offset = meters.fractionToPercent(meters.peakHoldFraction.value)
  return isVertical.value ? { bottom: offset } : { left: offset }
})

function tickStyle(db: number) {
  const offset = meters.fractionToPercent(meters.dbToFraction(db))
  return isVertical.value ? { bottom: offset } : { left: offset }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <!-- Readout -->
    <div v-if="showReadout" class="flex items-center justify-between gap-3 text-xs">
      <div class="flex items-center gap-4">
        <div class="flex items-baseline gap-1">
          <span class="text-stage-faint">Peak</span>
          <span class="stage-numeric text-sm text-stage-text">
            {{ meters.formatDb(meters.peakDb.value) }}
          </span>
          <span class="text-stage-faint">dBFS</span>
        </div>
        <div class="flex items-baseline gap-1">
          <span class="text-stage-faint">RMS</span>
          <span class="stage-numeric text-sm text-stage-muted">
            {{ meters.formatDb(meters.rmsDb.value) }}
          </span>
          <span class="text-stage-faint">dBFS</span>
        </div>
      </div>

      <div
        v-if="meters.isClipping.value"
        class="flex items-center gap-1.5 rounded bg-stage-error/15 px-2 py-0.5 font-semibold text-stage-error"
      >
        <UIcon name="i-lucide-triangle-alert" class="size-3.5" />
        CLIP
      </div>
      <div v-else-if="meters.clipCount.value > 0" class="text-stage-warning">
        {{ meters.clipCount.value }} clipped sample{{ meters.clipCount.value === 1 ? '' : 's' }}
        this session
      </div>
    </div>

    <!-- Bar -->
    <div
      class="relative overflow-hidden rounded border border-stage-border bg-stage-bg"
      :class="isVertical ? 'w-6' : 'w-full'"
      :style="isVertical ? { height: `${height * 4}px` } : { height: `${height}px` }"
      role="meter"
      :aria-valuenow="meters.peakDb.value ?? -60"
      aria-valuemin="-60"
      aria-valuemax="0"
      aria-label="Input level in dBFS"
    >
      <!-- Peak -->
      <div
        class="absolute"
        :class="isVertical ? 'bottom-0 left-0' : 'left-0 top-0'"
        :style="peakStyle"
      >
        <!-- RMS core -->
        <div
          class="absolute bg-white/25"
          :class="isVertical ? 'bottom-0 left-0 w-full' : 'left-0 top-0 h-full'"
          :style="rmsStyle"
        />
      </div>

      <!-- Peak hold -->
      <div
        class="absolute bg-stage-text/80"
        :class="isVertical ? 'left-0 h-px w-full' : 'top-0 h-full w-px'"
        :style="holdStyle"
      />

      <!-- Empty-state message, drawn inside the bar so the layout never jumps -->
      <div
        v-if="meters.emptyMessage.value"
        class="absolute inset-0 flex items-center justify-center px-2"
      >
        <span class="truncate text-[11px] text-stage-faint">{{ meters.emptyMessage.value }}</span>
      </div>
    </div>

    <!-- Scale -->
    <div v-if="showScale && !isVertical" class="relative h-3">
      <div
        v-for="tick in meters.ticks"
        :key="tick"
        class="absolute -translate-x-1/2 text-[10px] text-stage-faint"
        :style="tickStyle(tick)"
      >
        {{ tick }}
      </div>
    </div>
  </div>
</template>
