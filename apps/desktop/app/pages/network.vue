<script setup lang="ts">
/**
 * Network Management.
 *
 * Two jobs: let the operator pick the adapter audio goes out of, and explain
 * the deployment StageLink is designed around. Note what this page never does:
 * change a firewall rule. It reports and it instructs; the operator acts.
 */
useHead({ title: 'Network · StageLink Core' })

const {
  network,
  recommendedAdapter,
  recommendationReason,
  adapterKindLabel,
  primaryAddress,
  speedLabel,
  setupSteps,
} = useNetworkStatus()

async function select(adapterId: string) {
  await network.selectAdapter(adapterId)
}

const warningColour = (severity: string) =>
  severity === 'error' || severity === 'critical'
    ? 'error'
    : severity === 'warning'
      ? 'warning'
      : 'neutral'
</script>

<template>
  <div class="mx-auto max-w-5xl space-y-5">
    <UAlert
      v-if="network.lastError"
      color="error"
      variant="subtle"
      icon="i-lucide-circle-x"
      :title="network.lastError.message"
      close
      @update:open="network.clearError()"
    />

    <!-- Warnings -->
    <div v-if="network.warnings.length > 0" class="space-y-3">
      <UAlert
        v-for="warning in network.warnings"
        :key="warning.code"
        :color="warningColour(warning.severity)"
        variant="subtle"
        icon="i-lucide-triangle-alert"
        :title="warning.message"
        :description="warning.remediation ?? undefined"
      />
    </div>

    <!-- Adapter selection -->
    <SharedStagePanel
      title="Streaming adapter"
      icon="i-lucide-ethernet-port"
      subtitle="The network interface audio is sent from"
      flush
    >
      <template #actions>
        <UButton
          size="xs"
          variant="ghost"
          icon="i-lucide-refresh-cw"
          :loading="network.isLoading"
          @click="network.refresh()"
        >
          Refresh
        </UButton>
      </template>

      <SharedEmptyState
        v-if="network.adapters.length === 0"
        icon="i-lucide-network"
        title="No network adapters found"
        description="This is unusual — even a machine with no network should report a loopback interface. Try refreshing."
      />

      <ul v-else class="divide-y divide-stage-border">
        <li
          v-for="adapter in network.adapters"
          :key="adapter.id"
          class="flex flex-wrap items-center gap-4 px-4 py-3"
          :class="adapter.id === network.selectedAdapterId ? 'bg-stage-elevated/60' : ''"
        >
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <SharedStatusDot :tone="adapter.isUp ? 'success' : 'neutral'" size="sm" />
              <span class="truncate font-medium">{{ adapter.name }}</span>
              <UBadge size="sm" variant="subtle" color="neutral">
                {{ adapterKindLabel(adapter) }}
              </UBadge>
              <UBadge
                v-if="adapter.id === recommendedAdapter?.id && adapter.id !== network.selectedAdapterId"
                size="sm"
                variant="subtle"
                color="primary"
              >
                recommended
              </UBadge>
              <UBadge
                v-if="adapter.id === network.selectedAdapterId"
                size="sm"
                variant="subtle"
                color="success"
              >
                selected
              </UBadge>
            </div>

            <div class="selectable mt-1 flex flex-wrap gap-x-4 gap-y-1 text-xs text-stage-muted">
              <span class="stage-numeric">{{ primaryAddress(adapter) ?? 'no IPv4 address' }}</span>
              <span>{{ speedLabel(adapter) }}</span>
              <span v-if="adapter.macAddress" class="stage-numeric">{{ adapter.macAddress }}</span>
              <span v-if="adapter.hasDefaultRoute === true">has internet route</span>
              <span v-else-if="adapter.hasDefaultRoute === false">no internet route</span>
            </div>

            <p
              v-if="adapter.id === recommendedAdapter?.id && recommendationReason"
              class="mt-1 text-[11px] text-stage-cyan-soft"
            >
              {{ recommendationReason }}
            </p>
            <p class="mt-1 truncate text-[11px] text-stage-faint">{{ adapter.description }}</p>
          </div>

          <UButton
            size="sm"
            :variant="adapter.id === network.selectedAdapterId ? 'subtle' : 'outline'"
            :color="adapter.id === network.selectedAdapterId ? 'success' : 'neutral'"
            :disabled="
              adapter.id === network.selectedAdapterId ||
              !adapter.isUp ||
              primaryAddress(adapter) === null ||
              adapter.kind === 'loopback' ||
              network.isSelecting
            "
            :loading="network.isSelecting"
            @click="select(adapter.id)"
          >
            {{ adapter.id === network.selectedAdapterId ? 'In use' : 'Use this adapter' }}
          </UButton>
        </li>
      </ul>
    </SharedStagePanel>

    <!-- Session detail -->
    <div class="grid gap-5 md:grid-cols-2">
      <SharedStagePanel title="Active network session" icon="i-lucide-activity">
        <dl class="space-y-3">
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Bound address</dt>
            <dd class="selectable stage-numeric text-sm">{{ network.selectedAddress ?? '—' }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Control port</dt>
            <dd class="stage-numeric text-sm">{{ network.status?.controlPort ?? '—' }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Audio port</dt>
            <dd class="stage-numeric text-sm">{{ network.status?.streamPort ?? '—' }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Discovery</dt>
            <dd class="text-sm">
              {{ network.discoveryActive ? 'Advertising over mDNS' : 'Not advertising' }}
            </dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Egress (measured)</dt>
            <dd class="stage-numeric text-sm">{{ formatBitrate(network.egressBitsPerSecond) }}</dd>
          </div>
          <div class="flex items-baseline justify-between gap-3">
            <dt class="text-xs text-stage-muted">Send errors (measured)</dt>
            <dd
              class="stage-numeric text-sm"
              :class="network.sendErrors > 0 ? 'text-stage-error' : ''"
            >
              {{ network.sendErrors }}
            </dd>
          </div>
        </dl>
      </SharedStagePanel>

      <SharedStagePanel title="Recommended setup" icon="i-lucide-book-open">
        <ol class="space-y-2.5">
          <li
            v-for="(step, index) in setupSteps"
            :key="index"
            class="flex gap-3 text-sm leading-relaxed text-stage-muted"
          >
            <span
              class="stage-numeric mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border border-stage-border text-[11px] text-stage-cyan-soft"
            >
              {{ index + 1 }}
            </span>
            <span>{{ step }}</span>
          </li>
        </ol>

        <UAlert
          class="mt-5"
          color="neutral"
          variant="subtle"
          icon="i-lucide-shield"
          title="StageLink will not change your firewall"
          description="If receivers can pair but hear nothing, a firewall rule is the usual cause. StageLink reports on it but never edits rules on your behalf."
        />
      </SharedStagePanel>
    </div>
  </div>
</template>
