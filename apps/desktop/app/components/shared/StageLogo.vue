<script setup lang="ts">
/**
 * The official StageLink logo.
 *
 * Renders `assets/branding/StageLink-Logo.png` as supplied — a purple-to-cyan
 * waveform on a dark rounded square. It is never recoloured, never
 * reconstructed in SVG and never substituted. The only thing that changes is
 * its size.
 */
withDefaults(
  defineProps<{
    /** Rendered edge length in pixels. */
    size?: number
    /** Show the wordmark beside the logo. */
    withWordmark?: boolean
    /** Show the tagline under the wordmark. */
    withTagline?: boolean
  }>(),
  {
    size: 32,
    withWordmark: false,
    withTagline: false,
  },
)
</script>

<template>
  <div class="flex items-center gap-3">
    <img
      src="/stagelink-logo.png"
      alt="StageLink"
      :width="size"
      :height="size"
      :style="{ width: `${size}px`, height: `${size}px` }"
      class="shrink-0 rounded-[22%] object-contain"
      draggable="false"
    >
    <div v-if="withWordmark" class="min-w-0 leading-tight">
      <div class="truncate font-semibold tracking-tight text-stage-text">StageLink</div>
      <div v-if="withTagline" class="truncate text-xs text-stage-muted">
        Your mix. Your stage. Your sound.
      </div>
    </div>
  </div>
</template>
