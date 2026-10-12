<script setup lang="ts">
/**
 * About StageLink.
 *
 * Versions, build information and — importantly — an honest statement of what
 * this build can and cannot do. A page that implies more capability than exists
 * is worse than no page at all.
 */
useHead({ title: 'About · StageLink Core' })

const service = useServiceStore()
const { runtime, bridge } = useStageLinkBridge()

/**
 * Capability status.
 *
 * Kept here, next to the version numbers, rather than buried in a changelog:
 * somebody evaluating whether StageLink can run their service on Sunday needs
 * to know exactly where it stands.
 */
const capabilities = [
  {
    label: 'Windows audio device enumeration',
    state: 'done' as const,
    note: 'Real WASAPI endpoint enumeration, including devices that are unplugged or disabled.',
  },
  {
    label: 'Mono monitor capture with live metering',
    state: 'done' as const,
    note: 'Real-time capture thread with Pro Audio scheduling, lock-free hand-off and true peak/RMS metering.',
  },
  {
    label: 'Audio packet protocol',
    state: 'done' as const,
    note: 'Binary framing, sequencing and loss accounting, verified against a headless receiver.',
  },
  {
    label: 'Audio streaming to receivers',
    state: 'pending' as const,
    note: 'Phase 3. The packetiser and per-receiver queues are implemented; the UDP sender is not.',
  },
  {
    label: 'Join codes and QR generation',
    state: 'done' as const,
    note: 'Real short-lived tokens and scannable QR codes, generated locally. Scanning authorises a device; no approval step.',
  },
  {
    label: 'Accepting receiver connections',
    state: 'pending' as const,
    note: 'Phase 3. The join code is honoured by the control server, which is not built yet.',
  },
  {
    label: 'Encrypted audio transport',
    state: 'pending' as const,
    note: 'Phase 5. The transport abstraction and packet format are in place; key exchange is not.',
  },
]

async function openRepository() {
  await bridge?.openExternal('https://github.com/stagelink/stagelink')
}
</script>

<template>
  <div class="mx-auto max-w-3xl space-y-5">
    <!-- Identity -->
    <SharedStagePanel>
      <div class="flex flex-col items-center py-6 text-center">
        <SharedStageLogo :size="88" />
        <h1 class="mt-5 text-2xl font-semibold tracking-tight">StageLink Core</h1>
        <p class="stage-gradient-text mt-1 text-sm font-medium">
          Your mix. Your stage. Your sound.
        </p>
        <p class="mt-4 max-w-md text-sm leading-relaxed text-stage-muted">
          A wireless in-ear monitoring system. StageLink takes one combined monitor mix from your
          FOH console and delivers it to every performer's phone over a local network, with
          personal volume, EQ and limiting handled on each device.
        </p>
      </div>
    </SharedStagePanel>

    <!-- Versions -->
    <SharedStagePanel title="Build" icon="i-lucide-package">
      <dl class="grid grid-cols-2 gap-4 text-sm">
        <div>
          <dt class="text-xs text-stage-muted">Application</dt>
          <dd class="selectable stage-numeric mt-0.5">{{ runtime?.appVersion ?? '—' }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Audio service</dt>
          <dd class="selectable stage-numeric mt-0.5">{{ service.info.version ?? '—' }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Control protocol</dt>
          <dd class="selectable stage-numeric mt-0.5">
            {{ service.info.controlProtocolVersion ?? runtime?.controlProtocolVersion ?? '—' }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Audio protocol</dt>
          <dd class="selectable stage-numeric mt-0.5">
            v{{ service.info.audioProtocolVersion ?? runtime?.audioProtocolVersion ?? '—' }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Electron</dt>
          <dd class="selectable stage-numeric mt-0.5">{{ runtime?.electronVersion ?? '—' }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Chromium</dt>
          <dd class="selectable stage-numeric mt-0.5">{{ runtime?.chromeVersion ?? '—' }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Node</dt>
          <dd class="selectable stage-numeric mt-0.5">{{ runtime?.nodeVersion ?? '—' }}</dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Platform</dt>
          <dd class="selectable stage-numeric mt-0.5">
            {{ runtime ? `${runtime.platform} ${runtime.arch}` : '—' }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Audio backend</dt>
          <dd class="mt-0.5 uppercase">
            {{ service.info.availableAudioApis.join(', ') || '—' }}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-stage-muted">Build channel</dt>
          <dd class="mt-0.5 capitalize">{{ runtime?.channel ?? '—' }}</dd>
        </div>
      </dl>
    </SharedStagePanel>

    <!-- Capabilities -->
    <SharedStagePanel
      title="What this build does"
      icon="i-lucide-list-checks"
      subtitle="An honest account, so you can tell what is ready for a live service"
      flush
    >
      <ul class="divide-y divide-stage-border">
        <li
          v-for="capability in capabilities"
          :key="capability.label"
          class="flex items-start gap-3 px-4 py-3"
        >
          <UIcon
            :name="capability.state === 'done' ? 'i-lucide-circle-check' : 'i-lucide-circle-dashed'"
            class="mt-0.5 size-4 shrink-0"
            :class="capability.state === 'done' ? 'text-stage-success' : 'text-stage-muted'"
          />
          <div class="min-w-0">
            <div class="text-sm">{{ capability.label }}</div>
            <p class="mt-0.5 text-xs leading-snug text-stage-muted">{{ capability.note }}</p>
          </div>
        </li>
      </ul>
    </SharedStagePanel>

    <!-- Latency note -->
    <SharedStagePanel title="About latency" icon="i-lucide-timer">
      <p class="text-sm leading-relaxed text-stage-muted">
        StageLink targets roughly 15–30 ms from this desktop to a wired earphone under favourable
        conditions. That is a design target, not a measured guarantee. Real end-to-end latency
        depends on your audio interface, its driver, Windows scheduling, Wi-Fi conditions, the
        receiver's jitter buffer and the mobile audio output path — several of which the desktop
        cannot measure at all.
      </p>
      <p class="mt-3 text-sm leading-relaxed text-stage-muted">
        The Diagnostics page shows the desktop-side contributions it can genuinely measure, each
        labelled with how it was obtained. For a real end-to-end figure, follow the physical
        loopback procedure in the documentation.
      </p>
    </SharedStagePanel>

    <div class="flex justify-center pb-4">
      <UButton variant="ghost" color="neutral" size="sm" icon="i-lucide-external-link" @click="openRepository">
        Project repository
      </UButton>
    </div>
  </div>
</template>
