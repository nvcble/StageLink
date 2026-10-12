<script setup lang="ts">
/**
 * Performance Diagnostics.
 *
 * The rule this page exists to honour: an operator must be able to tell a
 * measurement from a guess, and a desktop measurement from something a phone
 * claimed. Every figure carries a provenance badge, and the latency breakdown
 * deliberately refuses to present a total.
 */
useHead({ title: 'Diagnostics · StageLink Core' })

const { diagnostics, formatMemory, formatCpu, formatMicros } = useDiagnostics()
const receivers = useReceiverStore()
</script>

<template>
  <div class="mx-auto max-w-6xl space-y-5">
    <UAlert
      v-if="diagnostics.healthWarnings.length > 0"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      title="Issues worth looking at"
    >
      <template #description>
        <ul class="mt-1 space-y-1">
          <li v-for="warning in diagnostics.healthWarnings" :key="warning">{{ warning }}</li>
        </ul>
      </template>
    </UAlert>

    <SharedStagePanel
      title="Audio capture health"
      icon="i-lucide-audio-waveform"
      subtitle="Counted by the audio service on this machine"
    >
      <template #actions>
        <div class="flex items-center gap-2 text-xs text-stage-muted">
          <SharedStatusDot :tone="diagnostics.isSubscribed ? 'success' : 'neutral'" size="sm" />
          {{ diagnostics.isSubscribed ? 'Live' : 'Paused' }}
        </div>
      </template>

      <dl v-if="diagnostics.audio" class="grid grid-cols-2 gap-5 sm:grid-cols-4">
        <div>
          <dt class="text-xs text-stage-muted">Capture state</dt>
          <dd class="mt-0.5 capitalize">{{ diagnostics.audio.captureState }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Frames captured</dt>
          <dd class="stage-numeric mt-0.5">
            {{ diagnostics.audio.framesCaptured.toLocaleString() }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Driver glitches</dt>
          <dd
            class="stage-numeric mt-0.5"
            :class="diagnostics.audio.xruns > 0 ? 'text-stage-warning' : ''"
          >
            {{ diagnostics.audio.xruns }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Clipped samples</dt>
          <dd
            class="stage-numeric mt-0.5"
            :class="diagnostics.audio.clipCount > 0 ? 'text-stage-error' : ''"
          >
            {{ diagnostics.audio.clipCount }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Buffer overruns</dt>
          <dd
            class="stage-numeric mt-0.5"
            :class="diagnostics.audio.bufferOverruns > 0 ? 'text-stage-warning' : ''"
          >
            {{ diagnostics.audio.bufferOverruns }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Buffer underruns</dt>
          <dd class="stage-numeric mt-0.5">{{ diagnostics.audio.bufferUnderruns }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Callback worst case</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatMicros(diagnostics.audio.callbackMaxUs) }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Callback average</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatMicros(diagnostics.audio.callbackAvgUs) }}
          </dd>
        </div>
      </dl>
      <p v-else class="text-sm text-stage-muted">Waiting for the first snapshot…</p>
    </SharedStagePanel>

    <!-- Latency -->
    <SharedStagePanel
      title="Latency breakdown"
      icon="i-lucide-timer"
      subtitle="Desktop-side contributions only"
    >
      <div v-if="diagnostics.latency" class="space-y-5">
        <div class="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
          <SharedMetricValue
            label="Capture period"
            :measurement="diagnostics.latency.captureBufferMs"
            :digits="2"
          />
          <SharedMetricValue
            label="Packet frame"
            :measurement="diagnostics.latency.packetFrameMs"
            :digits="1"
          />
          <SharedMetricValue
            label="Send queue"
            :measurement="diagnostics.latency.sendQueueMs"
            :digits="2"
          />
          <SharedMetricValue
            label="Desktop subtotal"
            :measurement="diagnostics.latency.desktopSubtotalMs"
            :digits="2"
          />
          <SharedMetricValue
            label="Control channel round trip"
            :measurement="diagnostics.latency.controlChannelRttMs"
            :digits="1"
          />
          <SharedMetricValue
            label="Receiver output path"
            :measurement="diagnostics.latency.receiverReportedOutputMs"
            :digits="1"
          />
        </div>

        <UAlert color="neutral" variant="subtle" icon="i-lucide-info" title="About these figures">
          <template #description>
            <p class="leading-relaxed">{{ diagnostics.latency.disclaimer }}</p>
          </template>
        </UAlert>
      </div>
    </SharedStagePanel>

    <div class="grid gap-5 lg:grid-cols-2">
      <!-- Transmission -->
      <SharedStagePanel
        title="Transmission"
        icon="i-lucide-radio"
        subtitle="Counted on this machine"
      >
        <dl v-if="diagnostics.stream" class="grid grid-cols-2 gap-5">
          <div>
            <dt class="text-xs text-stage-muted">Packets sent</dt>
            <dd class="stage-numeric mt-0.5">
              {{ diagnostics.stream.packetsSent.toLocaleString() }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Data sent</dt>
            <dd class="stage-numeric mt-0.5">{{ formatBytes(diagnostics.stream.bytesSent) }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Send failures</dt>
            <dd
              class="stage-numeric mt-0.5"
              :class="diagnostics.stream.sendFailures > 0 ? 'text-stage-error' : ''"
            >
              {{ diagnostics.stream.sendFailures }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Dropped from full queues</dt>
            <dd
              class="stage-numeric mt-0.5"
              :class="diagnostics.stream.packetsDropped > 0 ? 'text-stage-warning' : ''"
            >
              {{ diagnostics.stream.packetsDropped }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Deepest queue seen</dt>
            <dd class="stage-numeric mt-0.5">{{ diagnostics.stream.queueDepthMax }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Egress</dt>
            <dd class="stage-numeric mt-0.5">
              {{ formatBitrate(diagnostics.stream.egressBitsPerSecond) }}
            </dd>
          </div>
        </dl>
      </SharedStagePanel>

      <!-- Receiver-reported aggregates -->
      <SharedStagePanel
        title="Receiver-reported quality"
        icon="i-lucide-smartphone"
        subtitle="Sent to us by receivers; unverified"
      >
        <p
          v-if="receivers.connectedCount === 0"
          class="text-sm leading-relaxed text-stage-muted"
        >
          No receivers are connected, so there is nothing reported. These figures stay blank
          rather than showing zeroes that could be mistaken for a clean link.
        </p>

        <dl v-else-if="diagnostics.receivers" class="grid grid-cols-2 gap-5">
          <div>
            <dt class="text-xs text-stage-muted">Connected</dt>
            <dd class="stage-numeric mt-0.5">{{ diagnostics.receivers.connectedCount }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Receiving audio</dt>
            <dd class="stage-numeric mt-0.5">{{ diagnostics.receivers.streamingCount }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Worst loss</dt>
            <dd class="stage-numeric mt-0.5">
              {{
                diagnostics.receivers.worstLossPercent === null
                  ? '—'
                  : `${diagnostics.receivers.worstLossPercent.toFixed(2)}%`
              }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Worst jitter</dt>
            <dd class="stage-numeric mt-0.5">
              {{
                diagnostics.receivers.worstJitterMs === null
                  ? '—'
                  : `${diagnostics.receivers.worstJitterMs.toFixed(1)} ms`
              }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Total underruns</dt>
            <dd class="stage-numeric mt-0.5">{{ diagnostics.receivers.totalUnderruns }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Lowest buffer</dt>
            <dd class="stage-numeric mt-0.5">
              {{
                diagnostics.receivers.minBufferHealthMs === null
                  ? '—'
                  : `${diagnostics.receivers.minBufferHealthMs.toFixed(0)} ms`
              }}
            </dd>
          </div>
        </dl>
      </SharedStagePanel>
    </div>

    <!-- System -->
    <SharedStagePanel title="System" icon="i-lucide-cpu" subtitle="Measured on this machine">
      <dl v-if="diagnostics.system" class="grid grid-cols-2 gap-5 sm:grid-cols-3 lg:grid-cols-5">
        <div>
          <dt class="text-xs text-stage-muted">Service CPU</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatCpu(diagnostics.system.processCpuPercent) }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">System CPU</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatCpu(diagnostics.system.systemCpuPercent) }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Service memory</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatMemory(diagnostics.system.processMemoryBytes) }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Memory available</dt>
          <dd class="stage-numeric mt-0.5">
            {{ formatMemory(diagnostics.system.systemMemoryAvailableBytes) }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Real-time priority</dt>
          <dd
            class="mt-0.5"
            :class="diagnostics.system.realtimePriorityGranted ? '' : 'text-stage-warning'"
          >
            {{ diagnostics.system.realtimePriorityGranted ? 'Granted' : 'Not granted' }}
          </dd>
        </div>
      </dl>

      <div class="mt-5 flex items-baseline justify-between gap-3 border-t border-stage-border pt-4">
        <span class="text-xs text-stage-muted">Service uptime</span>
        <span class="stage-numeric text-sm">{{ formatDuration(diagnostics.uptimeMs) }}</span>
      </div>
    </SharedStagePanel>

    <!-- Errors -->
    <SharedStagePanel title="Recent operational errors" icon="i-lucide-list-x" flush>
      <SharedEmptyState
        v-if="diagnostics.recentErrors.length === 0"
        icon="i-lucide-circle-check"
        title="No errors recorded"
        description="The audio service has not reported any faults."
      />
      <ul v-else class="divide-y divide-stage-border">
        <li
          v-for="error in diagnostics.recentErrors"
          :key="error.id"
          class="flex items-start gap-3 px-4 py-3"
        >
          <SharedStatusDot
            :tone="error.severity === 'warning' ? 'warning' : 'error'"
            size="sm"
            class="mt-1.5"
          />
          <div class="min-w-0">
            <div class="text-sm">{{ error.message }}</div>
            <div class="mt-0.5 flex flex-wrap gap-x-3 text-xs text-stage-faint">
              <span class="stage-numeric">{{ new Date(error.at).toLocaleTimeString() }}</span>
              <span>{{ error.source }}</span>
              <span class="stage-numeric">{{ error.code }}</span>
            </div>
            <p v-if="error.detail" class="mt-1 text-xs text-stage-muted">{{ error.detail }}</p>
          </div>
        </li>
      </ul>
    </SharedStagePanel>
  </div>
</template>
