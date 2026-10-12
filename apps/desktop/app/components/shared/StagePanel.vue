<script setup lang="ts">
/**
 * The standard panel container.
 *
 * One component so spacing, borders and header treatment are consistent across
 * every page without each one re-deciding.
 */
withDefaults(
  defineProps<{
    title?: string
    subtitle?: string
    icon?: string
    /** Use the raised surface colour. */
    elevated?: boolean
    /** Remove body padding, for panels whose content manages its own. */
    flush?: boolean
  }>(),
  { elevated: false, flush: false },
)
</script>

<template>
  <section :class="elevated ? 'stage-panel-elevated' : 'stage-panel'">
    <header
      v-if="title || $slots.header || $slots.actions"
      class="flex items-start justify-between gap-4 border-b border-stage-border px-4 py-3"
    >
      <div class="flex min-w-0 items-start gap-2.5">
        <UIcon v-if="icon" :name="icon" class="mt-0.5 size-4 shrink-0 text-stage-cyan-soft" />
        <div class="min-w-0">
          <h2 v-if="title" class="truncate text-sm font-semibold text-stage-text">{{ title }}</h2>
          <p v-if="subtitle" class="mt-0.5 text-xs leading-snug text-stage-muted">
            {{ subtitle }}
          </p>
          <slot name="header" />
        </div>
      </div>
      <div v-if="$slots.actions" class="shrink-0">
        <slot name="actions" />
      </div>
    </header>

    <div :class="flush ? '' : 'px-4 py-4'">
      <slot />
    </div>

    <footer v-if="$slots.footer" class="border-t border-stage-border px-4 py-3">
      <slot name="footer" />
    </footer>
  </section>
</template>
