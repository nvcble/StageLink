<script setup lang="ts">
import QRCode from 'qrcode'

/**
 * Renders a join payload as a QR code.
 *
 * Generated locally with `qrcode` — never from a web service. StageLink is
 * designed for an access point with no internet uplink, so a remotely-rendered
 * QR would be a blank square exactly when it is needed.
 *
 * Rendered as inline SVG rather than a canvas so it stays crisp at any size and
 * prints cleanly, and on white rather than the dark surface because scanners
 * expect a light quiet zone.
 */
const props = withDefaults(
  defineProps<{
    /** The string to encode. Nothing is drawn when null. */
    payload: string | null
    /** Rendered edge length in pixels. */
    size?: number
    /** Dim the code to show it is no longer valid. */
    expired?: boolean
  }>(),
  { size: 220, expired: false },
)

const svg = ref<string | null>(null)
const error = ref<string | null>(null)

watch(
  () => props.payload,
  async (payload) => {
    if (!payload) {
      svg.value = null
      return
    }
    try {
      svg.value = await QRCode.toString(payload, {
        type: 'svg',
        // Medium correction is the usual trade: a laptop screen is a clean
        // target, so capacity is better spent keeping the code low-version.
        errorCorrectionLevel: 'M',
        margin: 2,
        width: props.size,
        color: { dark: '#080B16', light: '#FFFFFF' },
      })
      error.value = null
    } catch (cause) {
      svg.value = null
      error.value = cause instanceof Error ? cause.message : String(cause)
    }
  },
  { immediate: true },
)
</script>

<template>
  <div
    class="inline-flex items-center justify-center rounded-lg bg-white p-3 transition-opacity"
    :class="expired ? 'opacity-25' : ''"
    :style="{ width: `${size}px`, height: `${size}px` }"
  >
    <!-- eslint-disable-next-line vue/no-v-html -- generated locally by qrcode, never user input -->
    <div v-if="svg" class="size-full [&>svg]:size-full" v-html="svg" />
    <p v-else-if="error" class="px-2 text-center text-xs text-stage-error">{{ error }}</p>
    <UIcon v-else name="i-lucide-qr-code" class="size-10 text-stage-faint" />
  </div>
</template>
