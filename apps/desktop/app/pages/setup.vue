<script setup lang="ts">
/**
 * First-run setup.
 *
 * Every step validates the real operation it describes. Nothing is ticked off
 * because the operator clicked Next — the audio step is only complete when a
 * device is genuinely open and delivering frames, and the network step only
 * when an adapter with a usable address has actually been selected.
 *
 * Steps belonging to a later phase say so rather than presenting a control that
 * would silently do nothing.
 */
useHead({ title: 'Setup · StageLink Core' })
definePageMeta({ layout: 'default' })

const audio = useAudioStore()
const network = useNetworkStore()
const settings = useSettingsStore()
const service = useServiceStore()
const { deviceOptions, hasNoUsableDevice } = useAudioDevices()
const { recommendedAdapter, recommendationReason, primaryAddress } = useNetworkStatus()
const meters = useAudioMeters()

const stepIndex = ref(0)

const pendingDeviceId = ref<string | undefined>(undefined)
const pendingChannel = ref(0)

watchEffect(() => {
  if (audio.selectedDevice && pendingDeviceId.value === undefined) {
    pendingDeviceId.value = audio.selectedDevice.id
    pendingChannel.value = audio.selectedChannel ?? 0
  }
})

const pendingDevice = computed(
  () => audio.devices.find((device) => device.id === pendingDeviceId.value) ?? null,
)

const channelOptions = computed(() =>
  Array.from({ length: pendingDevice.value?.inputChannels ?? 0 }, (_, index) => ({
    value: index,
    label: `Input ${index + 1}`,
  })),
)

/**
 * Each step's completion is derived from real state, never from a local flag.
 */
const steps = computed(() => [
  {
    key: 'welcome',
    title: 'Welcome to StageLink',
    complete: service.isUsable,
    blocked: !service.isUsable,
  },
  {
    key: 'interface',
    title: 'Select your audio interface',
    complete: audio.selectedDevice !== null,
    blocked: hasNoUsableDevice.value,
  },
  {
    key: 'channel',
    title: 'Choose the mono monitor input',
    complete: audio.isCapturing && audio.selectedChannel !== null,
    blocked: audio.selectedDevice === null,
  },
  {
    key: 'verify',
    title: 'Verify incoming audio',
    complete: audio.testResult?.signalDetected === true,
    blocked: !audio.isCapturing,
  },
  {
    key: 'network',
    title: 'Select the network adapter',
    complete: network.isConfigured && !network.hasBlockingWarning,
    blocked: network.adapters.length === 0,
  },
  {
    key: 'stream',
    title: 'Configure streaming',
    complete: false,
    blocked: true,
    phase: 'Phase 3',
  },
  {
    key: 'receiver',
    title: 'Connect a test receiver',
    complete: false,
    blocked: true,
    phase: 'Phase 3',
  },
])

const currentStep = computed(() => steps.value[stepIndex.value])
const completedCount = computed(() => steps.value.filter((step) => step.complete).length)

function next() {
  if (stepIndex.value < steps.value.length - 1) stepIndex.value += 1
}
function back() {
  if (stepIndex.value > 0) stepIndex.value -= 1
}

async function openSelectedInput() {
  if (!pendingDeviceId.value) return
  await audio.selectDevice(pendingDeviceId.value, pendingChannel.value)
}

async function useRecommendedAdapter() {
  if (!recommendedAdapter.value) return
  await network.selectAdapter(recommendedAdapter.value.id)
}

async function finish() {
  await settings.completeSetup()
  await navigateTo('/')
}
</script>

<template>
  <div class="mx-auto max-w-3xl space-y-5">
    <!-- Progress -->
    <SharedStagePanel>
      <div class="flex items-center gap-4">
        <SharedStageLogo :size="44" />
        <div class="min-w-0 flex-1">
          <h1 class="text-lg font-semibold">Set up StageLink</h1>
          <p class="text-sm text-stage-muted">
            {{ completedCount }} of {{ steps.length }} steps complete
          </p>
        </div>
        <UButton variant="ghost" color="neutral" size="sm" to="/">Skip for now</UButton>
      </div>

      <ol class="mt-5 flex flex-wrap gap-2">
        <li v-for="(step, index) in steps" :key="step.key">
          <button
            type="button"
            class="flex items-center gap-2 rounded-full border px-3 py-1 text-xs transition-colors"
            :class="[
              index === stepIndex
                ? 'border-stage-purple bg-stage-purple/15 text-stage-text'
                : 'border-stage-border text-stage-muted hover:text-stage-text',
            ]"
            @click="stepIndex = index"
          >
            <UIcon
              :name="step.complete ? 'i-lucide-circle-check' : 'i-lucide-circle'"
              class="size-3.5"
              :class="step.complete ? 'text-stage-success' : ''"
            />
            {{ index + 1 }}
          </button>
        </li>
      </ol>
    </SharedStagePanel>

    <!-- Step body -->
    <SharedStagePanel :title="currentStep?.title">
      <!-- 1. Welcome -->
      <template v-if="currentStep?.key === 'welcome'">
        <p class="text-sm leading-relaxed text-stage-muted">
          StageLink takes one combined monitor mix from your FOH console and sends it to every
          performer's phone over a local network. This guide walks through the four things it needs:
          an audio interface, the input carrying your monitor send, a network adapter, and a
          receiver to listen on.
        </p>
        <UAlert
          v-if="!service.isUsable"
          class="mt-4"
          color="error"
          variant="subtle"
          icon="i-lucide-server-off"
          :title="service.statusLabel"
          :description="service.info.lastError?.detail ?? 'The audio service must be running before setup can continue.'"
        />
        <UAlert
          v-else
          class="mt-4"
          color="success"
          variant="subtle"
          icon="i-lucide-circle-check"
          title="The audio service is running"
          :description="`Service v${service.info.version}, control protocol ${service.info.controlProtocolVersion}, using ${service.info.availableAudioApis.join(', ').toUpperCase()}.`"
        />
      </template>

      <!-- 2. Interface -->
      <template v-else-if="currentStep?.key === 'interface'">
        <SharedEmptyState
          v-if="hasNoUsableDevice"
          icon="i-lucide-mic-off"
          title="No usable audio input found"
          :steps="[
            'Connect your USB audio interface and install its driver if needed.',
            'Confirm it appears under Input in Windows Sound settings and is enabled.',
            'Press Rescan.',
          ]"
        >
          <UButton icon="i-lucide-refresh-cw" :loading="audio.isLoadingDevices" @click="audio.refreshDevices(false)">
            Rescan
          </UButton>
        </SharedEmptyState>

        <div v-else class="space-y-4">
          <p class="text-sm text-stage-muted">
            Choose the interface receiving the AUX/monitor send from your mixer.
          </p>
          <USelectMenu
            v-model="pendingDeviceId"
            :items="deviceOptions"
            value-key="value"
            label-key="label"
            placeholder="Choose an audio interface"
            class="w-full"
          />
          <p v-if="pendingDevice" class="text-xs text-stage-muted">
            {{ pendingDevice.inputChannels }} input channel{{
              pendingDevice.inputChannels === 1 ? '' : 's'
            }}<template v-if="pendingDevice.defaultSampleRate !== null">, {{ (pendingDevice.defaultSampleRate / 1000).toFixed(1) }} kHz</template>
          </p>
        </div>
      </template>

      <!-- 3. Channel -->
      <template v-else-if="currentStep?.key === 'channel'">
        <p class="text-sm leading-relaxed text-stage-muted">
          Pick the single physical input carrying the monitor mix. StageLink captures exactly one
          channel — there is no mixing here, because the signal is already mixed.
        </p>
        <div class="mt-4 space-y-4">
          <USelectMenu
            v-model="pendingChannel"
            :items="channelOptions"
            value-key="value"
            label-key="label"
            :disabled="channelOptions.length === 0"
            class="w-full max-w-xs"
          />
          <UButton
            color="primary"
            icon="i-lucide-check"
            :disabled="!pendingDeviceId || audio.isSelecting"
            :loading="audio.isSelecting"
            @click="openSelectedInput"
          >
            Open this input
          </UButton>
          <UAlert
            v-if="audio.lastError"
            color="error"
            variant="subtle"
            icon="i-lucide-circle-x"
            :title="audio.lastError.message"
          />
          <UAlert
            v-else-if="audio.isCapturing"
            color="success"
            variant="subtle"
            icon="i-lucide-circle-check"
            title="Input open and delivering audio"
            :description="`${audio.status?.framesCaptured?.toLocaleString() ?? 0} frames captured so far.`"
          />
        </div>
      </template>

      <!-- 4. Verify -->
      <template v-else-if="currentStep?.key === 'verify'">
        <p class="text-sm leading-relaxed text-stage-muted">
          Play something through the monitor send on your mixer and watch the meter. The level
          below is measured from every captured sample.
        </p>

        <div class="mt-5">
          <AudioLevelMeter :height="36" />
        </div>

        <div class="mt-5 flex flex-wrap items-center gap-3">
          <UButton
            icon="i-lucide-play-circle"
            :disabled="!audio.isCapturing || audio.isTesting"
            :loading="audio.isTesting"
            @click="audio.testInput(3_000)"
          >
            Test for 3 seconds
          </UButton>
          <span v-if="meters.hasSignal.value" class="stage-numeric text-sm">
            {{ meters.formatDb(meters.peakDb.value) }} dBFS
          </span>
        </div>

        <UAlert
          v-if="audio.testResult"
          class="mt-4"
          :color="audio.testResult.signalDetected && !audio.testResult.clipping ? 'success' : 'warning'"
          variant="subtle"
          :icon="audio.testResult.signalDetected ? 'i-lucide-circle-check' : 'i-lucide-volume-x'"
          :title="audio.testResult.verdict"
        />
      </template>

      <!-- 5. Network -->
      <template v-else-if="currentStep?.key === 'network'">
        <p class="text-sm leading-relaxed text-stage-muted">
          Choose the adapter audio will be sent from. The recommended setup is a dedicated Wi-Fi
          access point cabled to this machine over Ethernet. It does not need an internet
          connection.
        </p>

        <div v-if="recommendedAdapter" class="stage-panel-elevated mt-4 px-4 py-3">
          <div class="flex items-center gap-2">
            <UIcon name="i-lucide-sparkles" class="size-4 text-stage-cyan-soft" />
            <span class="font-medium">{{ recommendedAdapter.name }}</span>
            <span class="stage-numeric text-xs text-stage-muted">
              {{ primaryAddress(recommendedAdapter) }}
            </span>
          </div>
          <p class="mt-1 text-xs text-stage-muted">{{ recommendationReason }}</p>
          <UButton
            class="mt-3"
            size="sm"
            color="primary"
            :disabled="recommendedAdapter.id === network.selectedAdapterId || network.isSelecting"
            :loading="network.isSelecting"
            @click="useRecommendedAdapter"
          >
            {{ recommendedAdapter.id === network.selectedAdapterId ? 'Already selected' : 'Use this adapter' }}
          </UButton>
        </div>

        <UButton class="mt-4" variant="ghost" color="neutral" size="sm" to="/network">
          Choose a different adapter
        </UButton>

        <div v-if="network.warnings.length > 0" class="mt-4 space-y-2">
          <UAlert
            v-for="warning in network.warnings"
            :key="warning.code"
            :color="warning.severity === 'error' ? 'error' : warning.severity === 'warning' ? 'warning' : 'neutral'"
            variant="subtle"
            icon="i-lucide-triangle-alert"
            :title="warning.message"
            :description="warning.remediation ?? undefined"
          />
        </div>
      </template>

      <!-- 6 & 7. Streaming and receivers -->
      <template v-else>
        <UAlert
          color="neutral"
          variant="subtle"
          icon="i-lucide-construction"
          :title="`This step needs the streaming engine (${currentStep?.phase})`"
          description="The packet format, the packetiser and the per-receiver send queues are implemented and tested, but the UDP sender and the pairing handshake are not. Rather than showing a control that would do nothing, this step is marked as unavailable."
        />
        <p class="mt-4 text-sm leading-relaxed text-stage-muted">
          Everything up to this point is real and working: you can select an interface, capture
          your monitor mix and watch live levels. Streaming to phones becomes available in the
          next phase.
        </p>
      </template>

      <template #footer>
        <div class="flex items-center justify-between gap-3">
          <UButton variant="ghost" color="neutral" :disabled="stepIndex === 0" @click="back">
            Back
          </UButton>
          <UButton
            v-if="stepIndex < steps.length - 1"
            color="primary"
            trailing-icon="i-lucide-arrow-right"
            @click="next"
          >
            Next
          </UButton>
          <UButton v-else color="primary" icon="i-lucide-check" @click="finish">
            Finish setup
          </UButton>
        </div>
      </template>
    </SharedStagePanel>
  </div>
</template>
