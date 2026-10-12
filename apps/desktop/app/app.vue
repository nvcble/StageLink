<script setup lang="ts">
/**
 * Application root.
 *
 * Applies the two presentation preferences that have to be set on `<html>`
 * rather than inside a component — the theme class and reduce-motion — and then
 * hands off to the layout.
 */
const settings = useSettingsStore()

useHead({
  htmlAttrs: {
    class: computed(() => (settings.reduceMotion ? 'reduce-motion' : '')),
  },
})
</script>

<template>
  <UApp>
    <NuxtLayout>
      <NuxtPage />
    </NuxtLayout>
  </UApp>
</template>
