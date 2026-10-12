<script setup lang="ts">
/**
 * Connected Receivers.
 *
 * With nothing connected this page shows pairing instructions, not a sample
 * device. Every quality figure shown for a receiver is labelled as coming from
 * that receiver, because the desktop cannot verify any of it.
 */
useHead({ title: 'Receivers · StageLink Core' })

const {
  receivers,
  receiverLabel,
  connectionLabel,
  connectionTone,
  pairingLabel,
  reportedHealth,
  telemetryAgeSeconds,
  formatPercent,
  formatMs,
} = useReceivers()

const network = useNetworkStore()

const pairingSteps = computed(() => [
  'Join the phone to the same Wi-Fi network as this machine’s streaming adapter.',
  'Open StageLink Mobile and choose Find a server, or scan the pairing QR code.',
  network.serverAddress
    ? `If discovery does not find it, enter the address manually: ${network.serverAddress}`
    : 'Select a streaming network adapter on the Network page so an address can be shown here.',
  'Approve the device when it appears here. Approved devices reconnect automatically next time.',
])

const renamingId = ref<string | null>(null)
const renameValue = ref('')

function beginRename(receiverId: string, current: string) {
  renamingId.value = receiverId
  renameValue.value = current
}

async function commitRename() {
  if (!renamingId.value || !renameValue.value.trim()) {
    renamingId.value = null
    return
  }
  await receivers.rename(renamingId.value, renameValue.value.trim())
  renamingId.value = null
}
</script>

<template>
  <div class="mx-auto max-w-6xl space-y-5">
    <UAlert
      v-if="receivers.lastError"
      color="error"
      variant="subtle"
      icon="i-lucide-circle-x"
      :title="receivers.lastError.message"
      :description="receivers.lastError.detail ?? undefined"
      close
      @update:open="receivers.clearError()"
    />

    <!-- Pending approvals come first: they need a decision. -->
    <SharedStagePanel
      v-if="receivers.hasPendingApprovals"
      title="Awaiting your approval"
      icon="i-lucide-user-plus"
      subtitle="Only the source address is verified by the desktop. Everything else is self-reported by the device."
    >
      <ul class="space-y-3">
        <li
          v-for="request in receivers.pendingRequests"
          :key="request.requestId"
          class="stage-panel-elevated flex flex-wrap items-center gap-4 px-4 py-3"
        >
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="truncate font-medium">{{ request.deviceName }}</span>
              <UBadge size="sm" variant="subtle" color="neutral">{{ request.platform }}</UBadge>
              <UBadge
                v-if="request.presentedValidCode"
                size="sm"
                variant="subtle"
                color="success"
              >
                valid code
              </UBadge>
              <UBadge v-else size="sm" variant="subtle" color="warning">no code</UBadge>
            </div>
            <div class="selectable mt-1 flex flex-wrap gap-x-4 text-xs text-stage-muted">
              <span>from {{ request.ipAddress }}</span>
              <span class="stage-numeric">key {{ request.keyFingerprint }}</span>
            </div>
            <p class="mt-1 text-[11px] text-stage-faint">
              Check the key fingerprint matches the one shown on the device before approving.
            </p>
          </div>
          <div class="flex shrink-0 gap-2">
            <UButton
              color="primary"
              size="sm"
              icon="i-lucide-check"
              @click="receivers.approve(request.requestId)"
            >
              Approve
            </UButton>
            <UButton
              color="neutral"
              variant="outline"
              size="sm"
              icon="i-lucide-x"
              @click="receivers.reject(request.requestId)"
            >
              Reject
            </UButton>
          </div>
        </li>
      </ul>
    </SharedStagePanel>

    <!-- Roster -->
    <SharedStagePanel
      title="Receivers"
      icon="i-lucide-smartphone"
      :subtitle="`${receivers.connectedCount} connected of a ${receivers.maxReceivers} receiver limit`"
      flush
    >
      <template #actions>
        <UButton
          size="xs"
          variant="ghost"
          icon="i-lucide-refresh-cw"
          :loading="receivers.isLoading"
          @click="receivers.refresh()"
        >
          Refresh
        </UButton>
      </template>

      <SharedEmptyState
        v-if="receivers.receivers.length === 0"
        icon="i-lucide-smartphone"
        title="No receivers connected"
        description="When a phone running StageLink Mobile pairs with this desktop it will appear here."
        :steps="pairingSteps"
      />

      <ul v-else class="divide-y divide-stage-border">
        <li v-for="receiver in receivers.sorted" :key="receiver.id" class="px-4 py-4">
          <div class="flex flex-wrap items-start gap-4">
            <!-- Identity -->
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <SharedStatusDot
                  :tone="connectionTone(receiver)"
                  :active="receiver.connectionState === 'streaming'"
                />
                <template v-if="renamingId === receiver.id">
                  <UInput
                    v-model="renameValue"
                    size="sm"
                    class="max-w-48"
                    autofocus
                    @keyup.enter="commitRename"
                    @keyup.esc="renamingId = null"
                  />
                  <UButton size="xs" icon="i-lucide-check" @click="commitRename" />
                </template>
                <template v-else>
                  <span class="truncate font-medium">{{ receiverLabel(receiver) }}</span>
                  <UButton
                    size="xs"
                    variant="ghost"
                    icon="i-lucide-pencil"
                    :title="`Rename ${receiverLabel(receiver)}`"
                    @click="beginRename(receiver.id, receiverLabel(receiver))"
                  />
                </template>
              </div>

              <div class="selectable mt-1.5 flex flex-wrap gap-x-4 gap-y-1 text-xs text-stage-muted">
                <span>{{ connectionLabel(receiver) }}</span>
                <span>{{ pairingLabel(receiver) }}</span>
                <span v-if="receiver.ipAddress" class="stage-numeric">{{ receiver.ipAddress }}</span>
                <span>{{ receiver.platform }}{{ receiver.platformVersion ? ` ${receiver.platformVersion}` : '' }}</span>
                <span v-if="receiver.appVersion">app {{ receiver.appVersion }}</span>
                <span class="stage-numeric text-stage-faint">{{ receiver.id.slice(0, 8) }}</span>
              </div>
            </div>

            <!-- Desktop-measured -->
            <dl class="grid shrink-0 grid-cols-2 gap-x-6 gap-y-1 text-xs">
              <dt class="text-stage-faint">Packets sent</dt>
              <dd class="stage-numeric text-right">
                {{ receiver.packetsSentToReceiver.toLocaleString() }}
              </dd>
              <dt class="text-stage-faint">Dropped locally</dt>
              <dd
                class="stage-numeric text-right"
                :class="receiver.packetsDroppedLocally > 0 ? 'text-stage-warning' : ''"
              >
                {{ receiver.packetsDroppedLocally.toLocaleString() }}
              </dd>
            </dl>

            <!-- Actions -->
            <div class="flex shrink-0 gap-2">
              <UButton
                size="xs"
                variant="outline"
                color="neutral"
                icon="i-lucide-unplug"
                @click="receivers.disconnect(receiver.id)"
              >
                Disconnect
              </UButton>
              <UButton
                size="xs"
                variant="outline"
                color="error"
                icon="i-lucide-shield-off"
                title="Withdraw this device's authorisation permanently"
                @click="receivers.revoke(receiver.id)"
              >
                Revoke
              </UButton>
            </div>
          </div>

          <!-- Receiver-reported telemetry, clearly fenced off -->
          <div class="mt-3 rounded-lg border border-stage-border bg-stage-bg/60 px-3 py-2.5">
            <div class="mb-2 flex items-center gap-2">
              <UIcon name="i-lucide-smartphone" class="size-3.5 text-stage-warning" />
              <span class="text-[11px] font-medium uppercase tracking-wide text-stage-warning">
                Reported by this receiver
              </span>
              <span v-if="telemetryAgeSeconds(receiver) !== null" class="text-[11px] text-stage-faint">
                · {{ telemetryAgeSeconds(receiver) }}s ago
              </span>
              <UBadge
                v-if="reportedHealth(receiver)"
                size="sm"
                variant="subtle"
                :color="reportedHealth(receiver)!.tone"
                class="ml-auto"
              >
                {{ reportedHealth(receiver)!.label }}
              </UBadge>
            </div>

            <p v-if="!receiver.telemetry" class="text-xs text-stage-faint">
              This receiver has not sent a telemetry report yet.
            </p>

            <dl v-else class="grid grid-cols-2 gap-x-6 gap-y-1.5 text-xs sm:grid-cols-5">
              <div>
                <dt class="text-stage-faint">Loss</dt>
                <dd class="stage-numeric">{{ formatPercent(receiver.telemetry.lossPercent) }}</dd>
              </div>
              <div>
                <dt class="text-stage-faint">Jitter</dt>
                <dd class="stage-numeric">{{ formatMs(receiver.telemetry.jitterMs) }}</dd>
              </div>
              <div>
                <dt class="text-stage-faint">Buffer</dt>
                <dd class="stage-numeric">{{ formatMs(receiver.telemetry.bufferHealthMs, 0) }}</dd>
              </div>
              <div>
                <dt class="text-stage-faint">Underruns</dt>
                <dd
                  class="stage-numeric"
                  :class="receiver.telemetry.underruns > 0 ? 'text-stage-warning' : ''"
                >
                  {{ receiver.telemetry.underruns }}
                </dd>
              </div>
              <div>
                <dt class="text-stage-faint">Playback</dt>
                <dd class="capitalize">{{ receiver.telemetry.playbackState }}</dd>
              </div>
            </dl>
          </div>
        </li>
      </ul>
    </SharedStagePanel>
  </div>
</template>
