<script setup lang="ts">
/**
 * An honest empty state.
 *
 * Used wherever StageLink has nothing to show — no receivers, no sessions, no
 * audio input. The alternative, filling a panel with plausible-looking sample
 * data, would be the single most dangerous thing this application could do:
 * an operator must never be able to mistake a demo row for a connected device.
 */
withDefaults(
  defineProps<{
    icon?: string
    title: string
    description?: string
    /** Numbered steps telling the operator how to make something appear here. */
    steps?: readonly string[]
  }>(),
  { icon: 'i-lucide-inbox' },
)
</script>

<template>
  <div class="flex flex-col items-center px-6 py-10 text-center">
    <div
      class="mb-4 flex size-12 items-center justify-center rounded-full border border-stage-border bg-stage-elevated"
    >
      <UIcon :name="icon" class="size-5 text-stage-muted" />
    </div>

    <h3 class="text-sm font-semibold text-stage-text">{{ title }}</h3>
    <p v-if="description" class="mt-1.5 max-w-md text-sm leading-relaxed text-stage-muted">
      {{ description }}
    </p>

    <ol v-if="steps?.length" class="mt-5 max-w-lg space-y-2 text-left">
      <li
        v-for="(step, index) in steps"
        :key="index"
        class="flex gap-3 text-sm leading-relaxed text-stage-muted"
      >
        <span
          class="stage-numeric mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border border-stage-border text-[11px] text-stage-cyan-soft"
        >
          {{ index + 1 }}
        </span>
        <span>{{ step }}</span>
      </li>
    </ol>

    <div v-if="$slots.default" class="mt-5">
      <slot />
    </div>
  </div>
</template>
