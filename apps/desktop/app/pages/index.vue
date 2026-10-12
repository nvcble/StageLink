<script setup lang="ts">
/**
 * Dashboard.
 *
 * The one screen an operator looks at during a service. Everything here is real
 * state from the Rust service; nothing is simulated, and figures that are not
 * known read "—" rather than zero.
 */
useHead({ title: 'Dashboard · StageLink Core' })

const service = useServiceStore()
const audio = useAudioStore()
const network = useNetworkStore()
const receivers = useReceiverStore()
const { session, stateLabel, stateTone, stateDescription } = useSession()
const { streaming, blockers, primaryBlocker, canStart, canStop, start, stop, bandwidthLabel } =
  useStreaming()
const settings = useSettingsStore()

const confirmStop = ref(false)

async function onStart() {
  await start()
}

async function onStop() {
  // Stopping mid-service silences every performer's monitors at once, so it is
  // confirmed rather than a single click.
  if (!confirmStop.value) {
    confirmStop.value = true
    setTimeout(() => {
      confirmStop.value = false
    }, 4_000)
    return
  }
  confirmStop.value = false
  await stop()
}
</script>

<template>
  <div class="mx-auto max-w-7xl space-y-5">
    <!-- First-run prompt -->
    <UAlert
      v-if="settings.showSetupWizard"
      color="primary"
      variant="subtle"
      icon="i-lucide-wand-sparkles"
      title="Finish setting up StageLink"
      description="Choose your audio interface, pick the mono monitor input, and select the network adapter for your access point."
    >
      <template #actions>
        <UButton to="/setup" color="primary" size="sm">Open setup</UButton>
      </template>
    </UAlert>

    <!-- Transport warning: never let an insecure default pass unnoticed -->
    <UAlert
      v-if="settings.usingInsecureTransport"
      color="warning"
      variant="subtle"
      icon="i-lucide-shield-alert"
      title="Audio is configured to stream unencrypted"
      description="The plaintext transport exists for development and for the headless test receiver. A release build refuses it. Switch to the encrypted transport in Settings before using this on a venue network."
    />

    <!-- Transport controls -->
    <SharedStagePanel>
      <div class="flex flex-wrap items-center gap-5">
        <div class="flex items-center gap-3">
          <UButton
            v-if="!streaming.isStreaming"
            size="lg"
            color="primary"
            icon="i-lucide-play"
            :disabled="!canStart"
            :loading="streaming.isStarting"
            :title="primaryBlocker ?? undefined"
            @click="onStart"
          >
            Start Streaming
          </UButton>
          <UButton
            v-else
            size="lg"
            :color="confirmStop ? 'error' : 'neutral'"
            :variant="confirmStop ? 'solid' : 'outline'"
            :icon="confirmStop ? 'i-lucide-octagon-alert' : 'i-lucide-square'"
            :disabled="!canStop"
            :loading="streaming.isStopping"
            @click="onStop"
          >
            {{ confirmStop ? 'Confirm stop' : 'Stop Streaming' }}
          </UButton>
        </div>

        <div class="flex items-center gap-3">
          <SharedStatusDot :tone="stateTone(session.state)" :active="session.isActive" />
          <div>
            <div class="text-sm font-medium">{{ stateLabel(session.state) }}</div>
            <div class="text-xs text-stage-muted">{{ stateDescription(session.state) }}</div>
          </div>
        </div>

        <div v-if="session.isActive" class="ml-auto text-right">
          <div class="stage-numeric text-2xl">{{ session.uptimeLabel }}</div>
          <div class="text-xs text-stage-muted">Session uptime</div>
        </div>
      </div>

      <!-- Why Start is unavailable. A disabled button with no reason is useless
           ten minutes before a service. -->
      <div v-if="blockers.length > 0 && !streaming.isStreaming" class="mt-4">
        <ul class="space-y-1.5">
          <li
            v-for="blocker in blockers"
            :key="blocker"
            class="flex items-start gap-2 text-sm text-stage-muted"
          >
            <UIcon name="i-lucide-circle-alert" class="mt-0.5 size-4 shrink-0 text-stage-warning" />
            <span>{{ blocker }}</span>
          </li>
        </ul>
      </div>
    </SharedStagePanel>

    <div class="grid gap-5 lg:grid-cols-3">
      <!-- Input level -->
      <SharedStagePanel
        class="lg:col-span-2"
        title="Monitor input"
        icon="i-lucide-audio-waveform"
        :subtitle="
          audio.selectedDevice
            ? `${audio.selectedDevice.name} · input ${(audio.selectedChannel ?? 0) + 1} of ${audio.selectedDevice.inputChannels}`
            : 'No audio input selected'
        "
      >
        <template #actions>
          <UButton to="/audio" size="xs" variant="ghost" icon="i-lucide-settings-2">
            Configure
          </UButton>
        </template>

        <AudioLevelMeter :height="32" />

        <dl class="mt-5 grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div>
            <dt class="text-xs text-stage-muted">Sample rate</dt>
            <dd class="stage-numeric mt-0.5 text-sm">
              {{ audio.format ? `${(audio.format.sampleRate / 1000).toFixed(1)} kHz` : '—' }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Capture period</dt>
            <dd class="stage-numeric mt-0.5 text-sm">
              {{
                audio.format
                  ? `${((audio.format.periodFrames / audio.format.sampleRate) * 1000).toFixed(1)} ms`
                  : '—'
              }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Frames captured</dt>
            <dd class="stage-numeric mt-0.5 text-sm">
              {{ audio.status?.framesCaptured?.toLocaleString() ?? '—' }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Glitches</dt>
            <dd
              class="stage-numeric mt-0.5 text-sm"
              :class="(audio.status?.xruns ?? 0) > 0 ? 'text-stage-warning' : ''"
            >
              {{ audio.status?.xruns ?? '—' }}
            </dd>
          </div>
        </dl>

        <UAlert
          v-if="audio.engineConverting"
          class="mt-4"
          color="warning"
          variant="subtle"
          icon="i-lucide-shuffle"
          title="Windows is resampling this input"
          description="The interface is not running at the requested rate natively. This adds a little latency. Set the interface to 48 kHz in its control panel for the best result."
        />
      </SharedStagePanel>

      <!-- Receivers -->
      <SharedStagePanel title="Receivers" icon="i-lucide-smartphone">
        <template #actions>
          <UButton to="/receivers" size="xs" variant="ghost" icon="i-lucide-arrow-right">
            Manage
          </UButton>
        </template>

        <div class="flex items-baseline gap-2">
          <span class="stage-numeric text-4xl">{{ receivers.connectedCount }}</span>
          <span class="text-sm text-stage-muted">of {{ receivers.maxReceivers }} connected</span>
        </div>

        <div v-if="receivers.hasPendingApprovals" class="mt-4">
          <UAlert
            color="warning"
            variant="subtle"
            icon="i-lucide-user-plus"
            :title="`${receivers.pendingRequests.length} device awaiting approval`"
          />
        </div>

        <p v-else-if="receivers.isEmpty" class="mt-4 text-sm leading-relaxed text-stage-muted">
          No receivers have connected. Open the Receivers page for pairing instructions.
        </p>

        <dl v-else class="mt-4 space-y-2.5">
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Worst reported loss</dt>
            <dd class="stage-numeric text-sm">
              {{
                receivers.worstReportedLoss === null
                  ? '—'
                  : `${receivers.worstReportedLoss.toFixed(2)}%`
              }}
            </dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Worst reported jitter</dt>
            <dd class="stage-numeric text-sm">
              {{
                receivers.worstReportedJitter === null
                  ? '—'
                  : `${receivers.worstReportedJitter.toFixed(1)} ms`
              }}
            </dd>
          </div>
          <p class="pt-1 text-[11px] leading-snug text-stage-faint">
            Loss and jitter are reported by the receivers themselves and cannot be verified by
            the desktop.
          </p>
        </dl>
      </SharedStagePanel>
    </div>

    <div class="grid gap-5 lg:grid-cols-3">
      <!-- Transmission -->
      <SharedStagePanel title="Transmission" icon="i-lucide-radio" subtitle="Measured on this machine">
        <dl class="space-y-3">
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Packets sent</dt>
            <dd class="stage-numeric text-sm">{{ streaming.packetsSent.toLocaleString() }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Data sent</dt>
            <dd class="stage-numeric text-sm">{{ formatBytes(streaming.bytesSent) }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Egress</dt>
            <dd class="stage-numeric text-sm">{{ bandwidthLabel }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Send failures</dt>
            <dd
              class="stage-numeric text-sm"
              :class="streaming.sendFailures > 0 ? 'text-stage-error' : ''"
            >
              {{ streaming.sendFailures }}
            </dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Dropped (full queue)</dt>
            <dd
              class="stage-numeric text-sm"
              :class="streaming.packetsDropped > 0 ? 'text-stage-warning' : ''"
            >
              {{ streaming.packetsDropped }}
            </dd>
          </div>
        </dl>
      </SharedStagePanel>

      <!-- Network -->
      <SharedStagePanel title="Network" icon="i-lucide-network">
        <template #actions>
          <UButton to="/network" size="xs" variant="ghost" icon="i-lucide-arrow-right">
            Configure
          </UButton>
        </template>

        <dl class="space-y-3">
          <div>
            <dt class="text-xs text-stage-muted">Streaming adapter</dt>
            <dd class="mt-0.5 truncate text-sm">
              {{ network.selectedAdapter?.name ?? 'Not selected' }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Server address</dt>
            <dd class="selectable stage-numeric mt-0.5 text-sm">
              {{ network.serverAddress ?? '—' }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Discovery</dt>
            <dd class="mt-0.5 text-sm">
              {{ network.discoveryActive ? 'Advertising' : 'Not advertising' }}
            </dd>
          </div>
        </dl>

        <div v-if="network.warnings.length > 0" class="mt-4 space-y-2">
          <div
            v-for="warning in network.warnings.slice(0, 2)"
            :key="warning.code"
            class="flex items-start gap-2 text-xs leading-snug"
            :class="
              warning.severity === 'error' || warning.severity === 'critical'
                ? 'text-stage-error'
                : warning.severity === 'warning'
                  ? 'text-stage-warning'
                  : 'text-stage-muted'
            "
          >
            <UIcon name="i-lucide-info" class="mt-0.5 size-3.5 shrink-0" />
            <span>{{ warning.message }}</span>
          </div>
        </div>
      </SharedStagePanel>

      <!-- System -->
      <SharedStagePanel title="System" icon="i-lucide-cpu">
        <template #actions>
          <UButton to="/diagnostics" size="xs" variant="ghost" icon="i-lucide-arrow-right">
            Diagnostics
          </UButton>
        </template>

        <dl class="space-y-3">
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Audio service</dt>
            <dd class="flex items-center gap-2 text-sm">
              <SharedStatusDot :tone="service.statusTone" size="sm" />
              {{ service.statusLabel }}
            </dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Audio API</dt>
            <dd class="text-sm uppercase">
              {{ service.info.availableAudioApis.join(', ') || '—' }}
            </dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Peak receivers</dt>
            <dd class="stage-numeric text-sm">{{ session.peakReceivers }}</dd>
          </div>
        </dl>

        <div v-if="service.errors.length > 0" class="mt-4">
          <div class="mb-1.5 text-xs font-medium text-stage-muted">Recent faults</div>
          <ul class="space-y-1.5">
            <li
              v-for="error in service.errors.slice(0, 3)"
              :key="error.id"
              class="text-xs leading-snug text-stage-error"
            >
              {{ error.message }}
            </li>
          </ul>
        </div>
      </SharedStagePanel>
    </div>
  </div>
</template>
