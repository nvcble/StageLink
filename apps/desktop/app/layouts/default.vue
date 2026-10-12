<script setup lang="ts">
/**
 * The desktop shell: sidebar navigation, a status bar, and the content region.
 *
 * The status bar is always visible on every page by design. During a service
 * the operator may be on the Receivers page when the audio interface drops out,
 * and they must see that immediately rather than only on the dashboard.
 */
const service = useServiceStore()
const audio = useAudioStore()
const streaming = useStreamingStore()
const receivers = useReceiverStore()
const network = useNetworkStore()
const { session, stateLabel, stateTone } = useSession()
const meters = useAudioMeters()

const navigation = [
  { to: '/', label: 'Dashboard', icon: 'i-lucide-layout-dashboard' },
  { to: '/audio', label: 'Audio Input', icon: 'i-lucide-audio-waveform' },
  { to: '/receivers', label: 'Receivers', icon: 'i-lucide-smartphone' },
  { to: '/network', label: 'Network', icon: 'i-lucide-network' },
  { to: '/sessions', label: 'Sessions', icon: 'i-lucide-history' },
  { to: '/diagnostics', label: 'Diagnostics', icon: 'i-lucide-activity' },
  { to: '/settings', label: 'Settings', icon: 'i-lucide-settings' },
  { to: '/about', label: 'About', icon: 'i-lucide-info' },
] as const

/** The audio-input indicator in the status bar. */
const audioTone = computed<'success' | 'warning' | 'error' | 'neutral'>(() => {
  if (audio.deviceLost) return 'error'
  if (audio.isClipping) return 'error'
  if (audio.captureState === 'running') return 'success'
  if (audio.captureState === 'error') return 'error'
  if (audio.captureState === 'reconnecting' || audio.captureState === 'opening') return 'warning'
  return 'neutral'
})

const audioLabel = computed(() => {
  if (audio.deviceLost) return 'Input disconnected'
  if (!audio.selectedDevice) return 'No input selected'
  if (audio.captureState !== 'running') return `Input ${audio.captureState}`
  const channel = audio.selectedChannel
  return `${audio.selectedDevice.name}${channel !== null ? ` · In ${channel + 1}` : ''}`
})
</script>

<template>
  <div class="flex h-screen overflow-hidden bg-stage-bg text-stage-text">
    <!-- Sidebar -->
    <aside class="flex w-60 shrink-0 flex-col border-r border-stage-border bg-stage-surface">
      <!-- Brand -->
      <div class="flex items-center gap-3 border-b border-stage-border px-4 py-4">
        <SharedStageLogo :size="36" with-wordmark with-tagline />
      </div>

      <!-- Navigation -->
      <nav class="flex-1 space-y-0.5 overflow-y-auto p-2" aria-label="Main">
        <NuxtLink
          v-for="item in navigation"
          :key="item.to"
          :to="item.to"
          class="group relative flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-stage-muted transition-colors hover:bg-stage-elevated hover:text-stage-text"
          active-class="bg-stage-elevated text-stage-text"
        >
          <!-- The one place the brand gradient appears as a solid fill. -->
          <span
            class="stage-gradient absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-r opacity-0 transition-opacity group-[.router-link-active]:opacity-100"
          />
          <UIcon :name="item.icon" class="size-4 shrink-0" />
          <span class="truncate">{{ item.label }}</span>

          <!-- Attention badges -->
          <UBadge
            v-if="item.to === '/receivers' && receivers.hasPendingApprovals"
            color="warning"
            variant="subtle"
            size="sm"
            class="ml-auto"
          >
            {{ receivers.pendingRequests.length }}
          </UBadge>
          <UBadge
            v-else-if="item.to === '/receivers' && receivers.connectedCount > 0"
            color="neutral"
            variant="subtle"
            size="sm"
            class="ml-auto"
          >
            {{ receivers.connectedCount }}
          </UBadge>
          <UIcon
            v-else-if="item.to === '/network' && network.hasBlockingWarning"
            name="i-lucide-triangle-alert"
            class="ml-auto size-3.5 text-stage-error"
          />
        </NuxtLink>
      </nav>

      <!-- Service state -->
      <div class="border-t border-stage-border p-3">
        <div class="flex items-center gap-2 text-xs">
          <SharedStatusDot :tone="service.statusTone" :active="service.isStarting" />
          <span class="truncate text-stage-muted">{{ service.statusLabel }}</span>
        </div>
        <div
          v-if="service.info.version"
          class="selectable mt-1 pl-4 text-[11px] text-stage-faint"
        >
          service v{{ service.info.version }} · protocol
          {{ service.info.controlProtocolVersion }}
        </div>
        <UButton
          v-if="service.hasFailed"
          size="xs"
          color="error"
          variant="subtle"
          class="mt-2 w-full"
          icon="i-lucide-rotate-ccw"
          @click="service.restart()"
        >
          Restart service
        </UButton>
      </div>
    </aside>

    <!-- Main -->
    <div class="flex min-w-0 flex-1 flex-col">
      <!-- Status bar -->
      <header
        class="flex h-14 shrink-0 items-center gap-5 border-b border-stage-border bg-stage-surface px-5"
      >
        <!-- Streaming -->
        <div class="flex items-center gap-2">
          <SharedStatusDot
            :tone="streaming.isStreaming ? 'success' : 'neutral'"
            :active="streaming.isStreaming"
          />
          <span class="text-sm font-medium">
            {{ streaming.isStreaming ? 'Streaming' : 'Not streaming' }}
          </span>
        </div>

        <div class="h-5 w-px bg-stage-border" />

        <!-- Session -->
        <div class="flex items-center gap-2 text-sm">
          <SharedStatusDot :tone="stateTone(session.state)" size="sm" />
          <span class="text-stage-muted">{{ stateLabel(session.state) }}</span>
          <span v-if="session.isActive" class="stage-numeric text-stage-text">
            {{ session.uptimeLabel }}
          </span>
        </div>

        <div class="h-5 w-px bg-stage-border" />

        <!-- Audio input -->
        <div class="flex min-w-0 items-center gap-2 text-sm">
          <SharedStatusDot :tone="audioTone" size="sm" />
          <span class="truncate text-stage-muted">{{ audioLabel }}</span>
          <span
            v-if="meters.hasSignal.value"
            class="stage-numeric shrink-0 text-xs text-stage-text"
          >
            {{ meters.formatDb(meters.peakDb.value) }} dBFS
          </span>
        </div>

        <div class="ml-auto flex items-center gap-5">
          <!-- Receivers -->
          <div class="flex items-center gap-2 text-sm">
            <UIcon name="i-lucide-smartphone" class="size-4 text-stage-muted" />
            <span class="stage-numeric">{{ receivers.connectedCount }}</span>
            <span class="text-stage-muted">/ {{ receivers.maxReceivers }}</span>
          </div>

          <!-- Server address -->
          <div v-if="network.serverAddress" class="flex items-center gap-2 text-sm">
            <UIcon name="i-lucide-network" class="size-4 text-stage-muted" />
            <span class="selectable stage-numeric text-xs">{{ network.serverAddress }}</span>
          </div>
        </div>
      </header>

      <!-- Alerts that must be visible on every page -->
      <div v-if="audio.deviceLost || service.hasFailed" class="shrink-0 px-5 pt-4">
        <UAlert
          v-if="service.hasFailed"
          color="error"
          variant="subtle"
          icon="i-lucide-server-off"
          :title="service.info.lastError?.message ?? 'The audio service is unavailable.'"
          :description="service.info.lastError?.detail ?? undefined"
        />
        <UAlert
          v-else-if="audio.deviceLost"
          color="error"
          variant="subtle"
          icon="i-lucide-unplug"
          title="The audio interface has been disconnected"
          description="Reconnect it, then reopen the input on the Audio Input page. Streaming has no signal to send until then."
        />
      </div>

      <!-- Content -->
      <main class="min-h-0 flex-1 overflow-y-auto p-5">
        <slot />
      </main>
    </div>
  </div>
</template>
