<script setup lang="ts">
/**
 * The join panel: a QR code devices scan to admit themselves.
 *
 * Its own component because both the Receivers page and the setup wizard need
 * it, and because the expiry/rotation behaviour is the part worth having in one
 * place. Scanning is the authorisation — there is no approval step — so the
 * panel is explicit about what the code grants and how long for.
 */
withDefaults(defineProps<{ size?: number; showSteps?: boolean }>(), {
  size: 220,
  showSteps: true,
})

const receivers = useReceiverStore()
const network = useNetworkStore()

// The panel drives its own countdown so it behaves identically wherever it is
// mounted, and costs nothing when it is not.
onMounted(() => receivers.startClock())
onBeforeUnmount(() => receivers.stopClock())

/** mm:ss for the countdown. */
const countdown = computed(() => {
  const total = receivers.joinCodeSecondsRemaining
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`
})

const steps = computed(() => [
  'Join the phone to the same Wi-Fi network as this machine’s streaming adapter.',
  'Open StageLink Mobile and scan the code. The device joins immediately — no approval needed.',
  receivers.joinCode
    ? `If the camera will not cooperate, enter ${receivers.joinCode.host}:${receivers.joinCode.controlPort} manually.`
    : 'Create a join code to get an address for manual entry.',
])
</script>

<template>
  <SharedStagePanel
    title="Add a receiver"
    icon="i-lucide-qr-code"
    subtitle="Scanning this code admits a device. Treat it like a key."
  >
    <template #actions>
      <UButton
        size="xs"
        :variant="receivers.hasValidJoinCode ? 'ghost' : 'solid'"
        :color="receivers.hasValidJoinCode ? 'neutral' : 'primary'"
        icon="i-lucide-refresh-cw"
        :loading="receivers.isMintingCode"
        @click="receivers.createJoinCode()"
      >
        {{ receivers.hasValidJoinCode ? 'New code' : 'Create code' }}
      </UButton>
    </template>

    <!-- No adapter means no address to put in the code. -->
    <UAlert
      v-if="!network.isConfigured"
      color="warning"
      variant="subtle"
      icon="i-lucide-network"
      title="Choose a streaming network adapter first"
      description="Receivers need an address to connect to."
    >
      <template #actions>
        <UButton to="/network" size="sm" color="warning" variant="subtle">Open Network</UButton>
      </template>
    </UAlert>

    <div v-else class="flex flex-wrap items-start gap-6">
      <div class="flex flex-col items-center gap-2">
        <ReceiversJoinQrCode
          :payload="receivers.joinCode?.qrPayload ?? null"
          :size="size"
          :expired="!receivers.hasValidJoinCode"
        />
        <div v-if="receivers.joinCode" class="flex items-center gap-1.5 text-xs">
          <SharedStatusDot
            :tone="receivers.hasValidJoinCode ? 'success' : 'neutral'"
            size="sm"
          />
          <span v-if="receivers.hasValidJoinCode" class="text-stage-muted">
            Expires in <span class="stage-numeric text-stage-text">{{ countdown }}</span>
          </span>
          <span v-else class="text-stage-muted">Expired — create a new code</span>
        </div>
      </div>

      <div class="min-w-56 flex-1 space-y-4">
        <dl class="space-y-2.5">
          <div>
            <dt class="text-xs text-stage-muted">Server address</dt>
            <dd class="selectable stage-numeric mt-0.5 text-sm">
              {{
                receivers.joinCode
                  ? `${receivers.joinCode.host}:${receivers.joinCode.controlPort}`
                  : '—'
              }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Capacity</dt>
            <dd class="stage-numeric mt-0.5 text-sm">
              {{ receivers.connectedCount }} of {{ receivers.maxReceivers }} connected
            </dd>
          </div>
        </dl>

        <ol v-if="showSteps" class="space-y-2">
          <li
            v-for="(step, index) in steps"
            :key="index"
            class="flex gap-2.5 text-xs leading-relaxed text-stage-muted"
          >
            <span
              class="stage-numeric mt-px flex size-4 shrink-0 items-center justify-center rounded-full border border-stage-border text-[10px] text-stage-cyan-soft"
            >
              {{ index + 1 }}
            </span>
            <span>{{ step }}</span>
          </li>
        </ol>

        <p class="text-[11px] leading-snug text-stage-faint">
          Anyone who photographs this code can join until it expires. Press
          <strong class="font-medium text-stage-muted">New code</strong> to invalidate it
          immediately, or revoke an individual device below.
        </p>
      </div>
    </div>
  </SharedStagePanel>
</template>
