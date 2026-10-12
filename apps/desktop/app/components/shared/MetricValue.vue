<script setup lang="ts">
import type { Measurement } from '@stagelink/shared-protocol'

/**
 * A single diagnostics figure with its provenance.
 *
 * The provenance badge is the point of this component. An operator debugging a
 * dropout mid-service needs to know instantly whether a number was counted on
 * this machine, derived arithmetically, or claimed by a phone. Rendering all
 * three identically would be actively misleading, so the badge is not optional.
 */
const props = withDefaults(
  defineProps<{
    label: string
    measurement: Measurement | null
    digits?: number
    /** Hide the provenance badge. Only for measured values in dense tables. */
    compact?: boolean
  }>(),
  { digits: 1, compact: false },
)

const display = computed(() => {
  const measurement = props.measurement
  if (!measurement || measurement.value === null) return '—'
  return `${measurement.value.toFixed(props.digits)}`
})

const unit = computed(() => props.measurement?.unit ?? '')

const source = computed(() => props.measurement?.source ?? null)

const sourceBadge = computed(() => {
  switch (source.value) {
    case 'measured':
      return { label: 'measured', class: 'text-stage-success/90 bg-stage-success/10' }
    case 'estimated':
      return { label: 'derived', class: 'text-stage-cyan-soft bg-stage-cyan/10' }
    case 'receiver-reported':
      return { label: 'reported by receiver', class: 'text-stage-warning bg-stage-warning/10' }
    default:
      return null
  }
})
</script>

<template>
  <div class="flex flex-col gap-1">
    <div class="flex items-baseline gap-1.5">
      <span class="stage-numeric text-xl text-stage-text">{{ display }}</span>
      <span v-if="unit && display !== '—'" class="text-xs text-stage-muted">{{ unit }}</span>
    </div>

    <div class="flex flex-wrap items-center gap-1.5">
      <span class="text-xs text-stage-muted">{{ label }}</span>
      <span
        v-if="!compact && sourceBadge"
        class="rounded px-1.5 py-px text-[10px] font-medium tracking-wide"
        :class="sourceBadge.class"
        :title="measurement?.note ?? undefined"
      >
        {{ sourceBadge.label }}
      </span>
    </div>

    <p v-if="measurement?.note && !compact" class="text-[11px] leading-snug text-stage-faint">
      {{ measurement.note }}
    </p>
  </div>
</template>
