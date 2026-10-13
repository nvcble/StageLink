<script setup lang="ts">
/**
 * Audio Input.
 *
 * Where the operator picks the interface and the one physical input carrying
 * the FOH monitor send. There is deliberately no mixer here: the signal is a
 * single pre-mixed mono feed, so there is nothing to mix.
 */
useHead({ title: 'Audio Input · StageLink Core' })

const { audio, deviceOptions, hasNoUsableDevice, configuredDeviceMissing, probeFormats } =
  useAudioDevices()
const settings = useSettingsStore()

const selectedDeviceId = ref<string | undefined>(undefined)
const selectedChannel = ref(0)

// Keep the form in step with what the service reports, including after a
// restore on service startup.
watchEffect(() => {
  if (audio.selectedDevice) {
    selectedDeviceId.value = audio.selectedDevice.id
    selectedChannel.value = audio.selectedChannel ?? 0
  } else if (selectedDeviceId.value === undefined) {
    selectedDeviceId.value = settings.settings.audio.deviceId ?? undefined
    selectedChannel.value = settings.settings.audio.channelIndex
  }
})

const pendingDevice = computed(
  () => audio.devices.find((device) => device.id === selectedDeviceId.value) ?? null,
)

const pendingChannelOptions = computed(() => {
  const count = pendingDevice.value?.inputChannels ?? 0
  return Array.from({ length: count }, (_, index) => ({
    value: index,
    label: `Input ${index + 1}`,
  }))
})

const isDirty = computed(
  () =>
    selectedDeviceId.value !== (audio.selectedDevice?.id ?? undefined) ||
    selectedChannel.value !== (audio.selectedChannel ?? -1),
)

const canApply = computed(
  () =>
    selectedDeviceId.value !== undefined &&
    pendingDevice.value?.state === 'active' &&
    (pendingDevice.value?.inputChannels ?? 0) > selectedChannel.value &&
    !audio.isSelecting,
)

async function apply() {
  if (!selectedDeviceId.value) return
  await audio.selectDevice(selectedDeviceId.value, selectedChannel.value)
}

async function release() {
  await audio.releaseDevice()
}

const testDurationMs = ref(3_000)
async function runTest() {
  await audio.testInput(testDurationMs.value)
}
</script>

<template>
  <div class="mx-auto max-w-5xl space-y-5">
    <UAlert
      color="neutral"
      variant="subtle"
      icon="i-lucide-info"
      title="StageLink carries one mono monitor mix"
      description="Select the single physical input that receives the AUX/monitor send from your FOH mixer. Every receiver gets that same signal; personal volume, EQ and limiting happen on each phone."
    />

    <UAlert
      v-if="configuredDeviceMissing"
      color="warning"
      variant="subtle"
      icon="i-lucide-unplug"
      :title="`${settings.settings.audio.deviceName ?? 'The configured interface'} is not available`"
      description="It is either unplugged or disabled in Windows. Reconnect it, or choose a different input below."
    />

    <UAlert
      v-if="audio.lastError"
      color="error"
      variant="subtle"
      icon="i-lucide-circle-x"
      :title="audio.lastError.message"
      :description="audio.lastError.detail ?? undefined"
      close
      @update:open="audio.clearError()"
    />

    <!-- Live meter -->
    <SharedStagePanel
      title="Input level"
      icon="i-lucide-audio-waveform"
      subtitle="Measured from every captured sample by the audio service"
    >
      <AudioLevelMeter :height="36" />
    </SharedStagePanel>

    <!-- Device selection -->
    <SharedStagePanel title="Audio interface" icon="i-lucide-sliders-horizontal">
      <template #actions>
        <div class="flex gap-2">
          <UButton
            size="xs"
            variant="ghost"
            icon="i-lucide-refresh-cw"
            :loading="audio.isLoadingDevices"
            @click="audio.refreshDevices(false)"
          >
            Rescan
          </UButton>
          <UButton
            size="xs"
            variant="ghost"
            icon="i-lucide-search-check"
            :loading="audio.isLoadingDevices"
            title="Asks each interface which sample rates it accepts. Briefly opens every device."
            @click="probeFormats"
          >
            Probe rates
          </UButton>
        </div>
      </template>

      <SharedEmptyState
        v-if="hasNoUsableDevice"
        icon="i-lucide-mic-off"
        title="No usable audio input found"
        description="Windows reports no active capture device with input channels."
        :steps="[
          'Connect your USB audio interface and install its driver if needed.',
          'Open Windows Sound settings and confirm the interface appears under Input and is enabled.',
          'Press Rescan above.',
        ]"
      />

      <div v-else class="space-y-5">
        <UFormField
          label="Interface"
          help="Interfaces that are unplugged or disabled are listed but cannot be selected."
        >
          <USelectMenu
            v-model="selectedDeviceId"
            :items="deviceOptions"
            value-key="value"
            label-key="label"
            placeholder="Choose an audio interface"
            class="w-full"
          >
            <template #item-label="{ item }">
              <div class="flex min-w-0 flex-col">
                <span class="truncate">{{ item.label }}</span>
                <span class="text-xs text-stage-muted">{{ item.hint }}</span>
              </div>
            </template>
          </USelectMenu>
        </UFormField>

        <UFormField
          label="Monitor input channel"
          :help="
            pendingDevice
              ? `This interface exposes ${pendingDevice.inputChannels} physical input${pendingDevice.inputChannels === 1 ? '' : 's'}.`
              : 'Choose an interface first.'
          "
        >
          <USelectMenu
            v-model="selectedChannel"
            :items="pendingChannelOptions"
            value-key="value"
            label-key="label"
            :disabled="pendingChannelOptions.length === 0"
            class="w-full"
          />
        </UFormField>

        <div class="flex flex-wrap items-center gap-3">
          <UButton
            color="primary"
            icon="i-lucide-check"
            :disabled="!canApply || !isDirty"
            :loading="audio.isSelecting"
            @click="apply"
          >
            {{ audio.isCapturing ? 'Switch input' : 'Open input' }}
          </UButton>
          <UButton
            v-if="audio.isCapturing"
            variant="outline"
            color="neutral"
            icon="i-lucide-square"
            @click="release"
          >
            Close input
          </UButton>
          <span v-if="isDirty && audio.isCapturing" class="text-xs text-stage-warning">
            Switching the input restarts capture and interrupts audio briefly.
          </span>
        </div>
      </div>
    </SharedStagePanel>

    <!-- Negotiated format -->
    <SharedStagePanel
      v-if="audio.format"
      title="Negotiated capture format"
      icon="i-lucide-file-audio"
      subtitle="What the driver actually agreed to, not what was requested"
    >
      <dl class="grid grid-cols-2 gap-4 sm:grid-cols-3">
        <div>
          <dt class="text-xs text-stage-muted">Sample rate</dt>
          <dd class="stage-numeric mt-0.5">{{ audio.format.sampleRate.toLocaleString() }} Hz</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Device channels</dt>
          <dd class="stage-numeric mt-0.5">{{ audio.format.deviceChannels }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Device format</dt>
          <dd class="mt-0.5 uppercase">{{ audio.format.deviceSampleFormat }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Capture period</dt>
          <dd class="stage-numeric mt-0.5">
            {{ audio.format.periodFrames }} frames ({{
              ((audio.format.periodFrames / audio.format.sampleRate) * 1000).toFixed(2)
            }}
            ms)
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Sharing mode</dt>
          <dd class="mt-0.5 capitalize">
            {{ audio.format.shareMode
            }}<span v-if="audio.lowLatency" class="text-stage-muted"> (low latency)</span>
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Engine resampling</dt>
          <dd class="mt-0.5" :class="audio.engineConverting ? 'text-stage-warning' : ''">
            {{ audio.engineConverting ? 'Yes' : 'No' }}
          </dd>
        </div>
      </dl>
    </SharedStagePanel>

    <!-- Input test -->
    <SharedStagePanel
      title="Test the input"
      icon="i-lucide-stethoscope"
      subtitle="Listens to the selected input and reports what is actually arriving"
    >
      <div class="flex flex-wrap items-end gap-3">
        <UFormField label="Listen for" class="w-40">
          <USelectMenu
            v-model="testDurationMs"
            :items="[
              { value: 1_000, label: '1 second' },
              { value: 3_000, label: '3 seconds' },
              { value: 10_000, label: '10 seconds' },
            ]"
            value-key="value"
            label-key="label"
          />
        </UFormField>
        <UButton
          icon="i-lucide-play-circle"
          :disabled="!audio.isCapturing || audio.isTesting"
          :loading="audio.isTesting"
          @click="runTest"
        >
          Run test
        </UButton>
        <span v-if="!audio.isCapturing" class="text-xs text-stage-muted">
          Open an input first.
        </span>
      </div>

      <div v-if="audio.testResult" class="mt-5">
        <UAlert
          :color="
            audio.testResult.clipping
              ? 'error'
              : audio.testResult.signalDetected
                ? 'success'
                : 'warning'
          "
          variant="subtle"
          :icon="
            audio.testResult.clipping
              ? 'i-lucide-triangle-alert'
              : audio.testResult.signalDetected
                ? 'i-lucide-circle-check'
                : 'i-lucide-volume-x'
          "
          :title="audio.testResult.verdict"
        />
        <dl class="mt-4 grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div>
            <dt class="text-xs text-stage-muted">Peak</dt>
            <dd class="stage-numeric mt-0.5">
              {{ audio.testResult.peakDb === null ? '—' : `${audio.testResult.peakDb.toFixed(1)} dBFS` }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">RMS</dt>
            <dd class="stage-numeric mt-0.5">
              {{ audio.testResult.rmsDb === null ? '—' : `${audio.testResult.rmsDb.toFixed(1)} dBFS` }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Frames analysed</dt>
            <dd class="stage-numeric mt-0.5">
              {{ audio.testResult.framesAnalysed.toLocaleString() }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Clipping</dt>
            <dd class="mt-0.5" :class="audio.testResult.clipping ? 'text-stage-error' : ''">
              {{ audio.testResult.clipping ? 'Yes' : 'No' }}
            </dd>
          </div>
        </dl>
      </div>
    </SharedStagePanel>

    <!-- Unavailable devices, for transparency -->
    <SharedStagePanel
      v-if="audio.unavailableDevices.length > 0"
      title="Unavailable inputs"
      icon="i-lucide-eye-off"
      subtitle="Listed so a device you configured previously does not simply vanish"
    >
      <ul class="divide-y divide-stage-border">
        <li
          v-for="device in audio.unavailableDevices"
          :key="device.id"
          class="flex items-center justify-between gap-4 py-2.5"
        >
          <span class="min-w-0 truncate text-sm text-stage-muted">{{ device.name }}</span>
          <span class="shrink-0 text-xs text-stage-faint">
            {{ describeDeviceState(device.state, device.inputChannels) }}
          </span>
        </li>
      </ul>
    </SharedStagePanel>
  </div>
</template>
