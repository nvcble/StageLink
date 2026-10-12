<script setup lang="ts">
import type { FrameDurationMs, SettingsPatch } from '@stagelink/shared-protocol'

/**
 * Application Settings.
 *
 * Every control writes through to the service, which validates before
 * persisting — so an invalid value is refused with the field named rather than
 * silently stored. Settings that cannot take effect until the stream restarts
 * say so, next to the control that was changed.
 */
useHead({ title: 'Settings · StageLink Core' })

const settings = useSettingsStore()
const service = useServiceStore()
const { bridge } = useStageLinkBridge()

/** Writes a patch and reports what happened. */
const savingPath = ref<string | null>(null)
const lastSaved = ref<string | null>(null)

async function save(patch: SettingsPatch, label: string) {
  savingPath.value = label
  const result = await settings.update(patch)
  savingPath.value = null
  if (result.ok) {
    lastSaved.value = label
    setTimeout(() => {
      if (lastSaved.value === label) lastSaved.value = null
    }, 2_500)
  }
}

const frameDurationOptions = [
  { value: 2.5, label: '2.5 ms — lowest latency, least tolerant of loss' },
  { value: 5, label: '5 ms — recommended' },
  { value: 10, label: '10 ms — most tolerant, slightly more latency' },
]

const transportOptions = [
  { value: 'udp-encrypted', label: 'Encrypted (required for release builds)' },
  { value: 'udp-plaintext', label: 'Plaintext — development and test receiver only' },
]

const themeOptions = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
  { value: 'system', label: 'Match Windows' },
]

const logLevelOptions = ['error', 'warn', 'info', 'debug', 'trace'].map((value) => ({
  value,
  label: value,
}))

async function reveal(target: 'logs' | 'database' | 'userData') {
  await bridge?.revealPath(target)
}
</script>

<template>
  <div class="mx-auto max-w-3xl space-y-5">
    <UAlert
      v-if="settings.lastError"
      color="error"
      variant="subtle"
      icon="i-lucide-circle-x"
      :title="settings.lastError.message"
      :description="settings.lastError.field ? `Field: ${settings.lastError.field}` : undefined"
      close
      @update:open="settings.clearError()"
    />

    <UAlert
      v-if="settings.needsRestart"
      color="warning"
      variant="subtle"
      icon="i-lucide-rotate-ccw"
      title="Some changes need the stream restarted"
      :description="`Restart streaming to apply: ${settings.pendingRestartPaths.join(', ')}`"
    >
      <template #actions>
        <UButton size="sm" color="warning" variant="subtle" @click="settings.clearRestartFlags()">
          Dismiss
        </UButton>
      </template>
    </UAlert>

    <!-- Streaming -->
    <SharedStagePanel title="Streaming" icon="i-lucide-radio">
      <div class="space-y-5">
        <UFormField
          label="Packet frame duration"
          help="Smaller packets mean lower latency but less tolerance of a congested network. Changing this restarts the stream."
        >
          <USelectMenu
            :model-value="settings.settings.streaming.frameDurationMs"
            :items="frameDurationOptions"
            value-key="value"
            label-key="label"
            class="w-full"
            @update:model-value="
              (value: number) =>
                save(
                  { streaming: { frameDurationMs: value as FrameDurationMs } },
                  'frameDuration',
                )
            "
          />
        </UFormField>

        <UFormField
          label="Audio transport"
          :help="
            settings.usingInsecureTransport
              ? 'The plaintext transport sends the monitor mix unencrypted. It exists for development and for the headless test receiver, and a release build refuses it.'
              : 'Audio payloads are encrypted and authenticated with per-session keys.'
          "
        >
          <USelectMenu
            :model-value="settings.settings.streaming.transport"
            :items="transportOptions"
            value-key="value"
            label-key="label"
            class="w-full"
            @update:model-value="
              (value: string) => save({ streaming: { transport: value as never } }, 'transport')
            "
          />
        </UFormField>

        <UFormField label="Receiver limit" help="Between 1 and 64.">
          <UInput
            :model-value="settings.settings.streaming.maxReceivers"
            type="number"
            :min="1"
            :max="64"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  { streaming: { maxReceivers: Number((event.target as HTMLInputElement).value) } },
                  'maxReceivers',
                )
            "
          />
        </UFormField>

        <UFormField
          label="Per-receiver send queue"
          help="Packets buffered per receiver before the oldest is dropped. Deeper tolerates a worse network at the cost of latency on that receiver."
        >
          <UInput
            :model-value="settings.settings.streaming.perReceiverQueuePackets"
            type="number"
            :min="2"
            :max="512"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  {
                    streaming: {
                      perReceiverQueuePackets: Number((event.target as HTMLInputElement).value),
                    },
                  },
                  'queueDepth',
                )
            "
          />
        </UFormField>

        <USwitch
          :model-value="settings.settings.streaming.autoStartStreaming"
          label="Start streaming automatically when the audio input is healthy"
          description="Off by default: starting a monitor feed is a decision, not something that should happen on its own."
          @update:model-value="
            (value: boolean) => save({ streaming: { autoStartStreaming: value } }, 'autoStart')
          "
        />

        <USwitch
          :model-value="settings.settings.streaming.requireApprovalEveryJoin"
          label="Require approval every time a receiver joins"
          description="On: even trusted devices need approving each session. Off: approved devices reconnect automatically."
          @update:model-value="
            (value: boolean) =>
              save({ streaming: { requireApprovalEveryJoin: value } }, 'requireApproval')
          "
        />
      </div>
    </SharedStagePanel>

    <!-- Audio -->
    <SharedStagePanel title="Audio input" icon="i-lucide-audio-waveform">
      <div class="space-y-5">
        <USwitch
          :model-value="settings.settings.audio.autoReconnect"
          label="Reopen the audio input automatically when it reappears"
          description="If the interface is unplugged and reconnected, StageLink will reopen it without being asked."
          @update:model-value="
            (value: boolean) => save({ audio: { autoReconnect: value } }, 'autoReconnect')
          "
        />

        <UFormField
          label="Hot-signal warning threshold"
          help="The dashboard flags the input as hot above this level. In dBFS."
        >
          <UInput
            :model-value="settings.settings.audio.hotSignalWarningDb"
            type="number"
            :min="-24"
            :max="0"
            :step="0.5"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  {
                    audio: {
                      hotSignalWarningDb: Number((event.target as HTMLInputElement).value),
                    },
                  },
                  'hotSignal',
                )
            "
          />
        </UFormField>

        <p class="text-xs text-stage-muted">
          The interface and input channel are chosen on the
          <NuxtLink to="/audio" class="text-stage-cyan-soft underline">Audio Input</NuxtLink> page.
        </p>
      </div>
    </SharedStagePanel>

    <!-- Startup -->
    <SharedStagePanel title="Startup" icon="i-lucide-power">
      <div class="space-y-5">
        <USwitch
          :model-value="settings.settings.startup.launchOnLogin"
          label="Launch StageLink when Windows starts"
          @update:model-value="
            (value: boolean) => save({ startup: { launchOnLogin: value } }, 'launchOnLogin')
          "
        />
        <USwitch
          :model-value="settings.settings.startup.startMinimisedToTray"
          label="Start minimised to the system tray"
          @update:model-value="
            (value: boolean) =>
              save({ startup: { startMinimisedToTray: value } }, 'startMinimised')
          "
        />
        <USwitch
          :model-value="settings.settings.startup.confirmQuitDuringSession"
          label="Ask before quitting during a live session"
          description="Quitting stops the audio service, which silences every receiver. Leave this on."
          @update:model-value="
            (value: boolean) =>
              save({ startup: { confirmQuitDuringSession: value } }, 'confirmQuit')
          "
        />
        <USwitch
          :model-value="settings.settings.startup.showSetupWizard"
          label="Show the setup guide on the dashboard"
          @update:model-value="
            (value: boolean) => save({ startup: { showSetupWizard: value } }, 'setupWizard')
          "
        />
      </div>
    </SharedStagePanel>

    <!-- Appearance -->
    <SharedStagePanel title="Appearance" icon="i-lucide-palette">
      <div class="space-y-5">
        <UFormField label="Theme" help="StageLink is designed dark-first.">
          <USelectMenu
            :model-value="settings.settings.appearance.theme"
            :items="themeOptions"
            value-key="value"
            label-key="label"
            class="w-full max-w-xs"
            @update:model-value="
              (value: string) => save({ appearance: { theme: value as never } }, 'theme')
            "
          />
        </UFormField>

        <UFormField
          label="Meter redraw rate"
          help="How often the meter is repainted. Independent of how often the audio service measures the level."
        >
          <UInput
            :model-value="settings.settings.appearance.meterRefreshHz"
            type="number"
            :min="10"
            :max="60"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  {
                    appearance: {
                      meterRefreshHz: Number((event.target as HTMLInputElement).value),
                    },
                  },
                  'meterHz',
                )
            "
          />
        </UFormField>

        <USwitch
          :model-value="settings.settings.appearance.reduceMotion"
          label="Reduce motion"
          description="Useful on a stage laptop that somebody else configured."
          @update:model-value="
            (value: boolean) => save({ appearance: { reduceMotion: value } }, 'reduceMotion')
          "
        />
      </div>
    </SharedStagePanel>

    <!-- Diagnostics -->
    <SharedStagePanel title="Diagnostics" icon="i-lucide-activity">
      <div class="space-y-5">
        <USwitch
          :model-value="settings.settings.diagnostics.streamWhileVisible"
          label="Stream diagnostics while the Diagnostics page is open"
          description="Off means the page only updates when you refresh it. The service does no diagnostics work either way unless something is watching."
          @update:model-value="
            (value: boolean) =>
              save({ diagnostics: { streamWhileVisible: value } }, 'diagnosticsStream')
          "
        />
        <UFormField label="Update interval" help="Milliseconds between diagnostics pushes.">
          <UInput
            :model-value="settings.settings.diagnostics.intervalMs"
            type="number"
            :min="200"
            :max="10000"
            :step="100"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  {
                    diagnostics: { intervalMs: Number((event.target as HTMLInputElement).value) },
                  },
                  'diagnosticsInterval',
                )
            "
          />
        </UFormField>
      </div>
    </SharedStagePanel>

    <!-- Logging -->
    <SharedStagePanel title="Logging" icon="i-lucide-file-text">
      <div class="space-y-5">
        <UFormField label="Log level">
          <USelectMenu
            :model-value="settings.settings.logging.level"
            :items="logLevelOptions"
            value-key="value"
            label-key="label"
            class="w-full max-w-xs"
            @update:model-value="
              (value: string) => save({ logging: { level: value as never } }, 'logLevel')
            "
          />
        </UFormField>
        <UFormField label="Log files to keep">
          <UInput
            :model-value="settings.settings.logging.retainFiles"
            type="number"
            :min="1"
            :max="60"
            class="w-32"
            @change="
              (event: Event) =>
                save(
                  { logging: { retainFiles: Number((event.target as HTMLInputElement).value) } },
                  'retainFiles',
                )
            "
          />
        </UFormField>

        <div class="flex flex-wrap gap-2">
          <UButton size="sm" variant="outline" color="neutral" icon="i-lucide-folder-open" @click="reveal('logs')">
            Open log folder
          </UButton>
          <UButton size="sm" variant="outline" color="neutral" icon="i-lucide-database" @click="reveal('database')">
            Show database
          </UButton>
        </div>
      </div>
    </SharedStagePanel>

    <!-- Service -->
    <SharedStagePanel title="Audio service" icon="i-lucide-server">
      <dl class="space-y-3">
        <div class="flex items-baseline justify-between gap-3">
          <dt class="text-xs text-stage-muted">State</dt>
          <dd class="flex items-center gap-2 text-sm">
            <SharedStatusDot :tone="service.statusTone" size="sm" />
            {{ service.statusLabel }}
          </dd>
        </div>
        <div class="flex items-baseline justify-between gap-3">
          <dt class="text-xs text-stage-muted">Version</dt>
          <dd class="selectable stage-numeric text-sm">{{ service.info.version ?? '—' }}</dd>
        </div>
        <div class="flex items-baseline justify-between gap-3">
          <dt class="text-xs text-stage-muted">Process ID</dt>
          <dd class="stage-numeric text-sm">{{ service.info.pid ?? '—' }}</dd>
        </div>
        <div class="flex items-baseline justify-between gap-3">
          <dt class="text-xs text-stage-muted">Automatic restarts</dt>
          <dd class="stage-numeric text-sm">{{ service.info.restartCount }}</dd>
        </div>
      </dl>

      <UButton
        class="mt-4"
        size="sm"
        variant="outline"
        color="warning"
        icon="i-lucide-rotate-ccw"
        @click="service.restart()"
      >
        Restart audio service
      </UButton>
      <p class="mt-2 text-xs text-stage-muted">
        Restarting interrupts audio. Do not do this during a service unless you have to.
      </p>
    </SharedStagePanel>

    <div v-if="lastSaved" class="pb-2 text-center text-xs text-stage-success">Saved</div>
    <div v-else-if="savingPath" class="pb-2 text-center text-xs text-stage-muted">Saving…</div>
  </div>
</template>
