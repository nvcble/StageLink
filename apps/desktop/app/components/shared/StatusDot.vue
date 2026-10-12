<script setup lang="ts">
/**
 * A small coloured state indicator.
 *
 * Deliberately paired with a text label by every caller: colour alone is not
 * an accessible way to convey state, and a stage is often lit in a way that
 * makes subtle hue differences hard to read.
 */
const props = withDefaults(
  defineProps<{
    tone: 'success' | 'warning' | 'error' | 'neutral'
    /** Pulse to indicate ongoing activity. */
    active?: boolean
    size?: 'sm' | 'md'
  }>(),
  { active: false, size: 'md' },
)

const toneClass = computed(() => {
  switch (props.tone) {
    case 'success':
      return 'bg-stage-success shadow-[0_0_8px_rgba(34,197,94,0.6)]'
    case 'warning':
      return 'bg-stage-warning shadow-[0_0_8px_rgba(245,158,11,0.6)]'
    case 'error':
      return 'bg-stage-error shadow-[0_0_8px_rgba(239,68,68,0.6)]'
    default:
      return 'bg-stage-faint'
  }
})

const sizeClass = computed(() => (props.size === 'sm' ? 'size-1.5' : 'size-2'))
</script>

<template>
  <span
    class="inline-block shrink-0 rounded-full"
    :class="[toneClass, sizeClass, active ? 'stage-pulse' : '']"
    aria-hidden="true"
  />
</template>
