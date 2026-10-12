<script setup lang="ts">
/**
 * Session Management.
 *
 * The current session's state machine, plus the history of past ones with their
 * operational event logs — which is what you actually want on the Monday after
 * something went wrong on the Sunday.
 */
useHead({ title: 'Sessions · StageLink Core' })

const { session, stateLabel, stateTone, stateDescription, eventIcon, severityTone, formatTime, formatDateTime } =
  useSession()

const expandedId = ref<string | null>(null)

onMounted(() => {
  void session.refreshHistory()
})

async function toggle(sessionId: string) {
  if (expandedId.value === sessionId) {
    expandedId.value = null
    return
  }
  expandedId.value = sessionId
  await session.loadEvents(sessionId)
}
</script>

<template>
  <div class="mx-auto max-w-5xl space-y-5">
    <!-- Current -->
    <SharedStagePanel title="Current session" icon="i-lucide-circle-play">
      <div v-if="session.current" class="space-y-4">
        <div class="flex flex-wrap items-center gap-4">
          <SharedStatusDot :tone="stateTone(session.state)" :active="session.isActive" />
          <div>
            <div class="font-medium">{{ stateLabel(session.state) }}</div>
            <div class="text-xs text-stage-muted">{{ stateDescription(session.state) }}</div>
          </div>
          <div class="ml-auto text-right">
            <div class="stage-numeric text-2xl">{{ session.uptimeLabel }}</div>
            <div class="text-xs text-stage-muted">Duration</div>
          </div>
        </div>

        <dl class="grid grid-cols-2 gap-4 border-t border-stage-border pt-4 sm:grid-cols-4">
          <div>
            <dt class="text-xs text-stage-muted">Label</dt>
            <dd class="mt-0.5 truncate text-sm">{{ session.current.label ?? '—' }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Started</dt>
            <dd class="mt-0.5 text-sm">{{ formatDateTime(session.current.startedAt) }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Peak receivers</dt>
            <dd class="stage-numeric mt-0.5 text-sm">{{ session.current.peakReceivers }}</dd>
          </div>
          <div>
            <dt class="text-xs text-stage-muted">Total receivers</dt>
            <dd class="stage-numeric mt-0.5 text-sm">{{ session.current.totalReceivers }}</dd>
          </div>
        </dl>

        <div class="selectable border-t border-stage-border pt-3 text-xs text-stage-faint">
          Session ID <span class="stage-numeric">{{ session.current.id }}</span>
        </div>

        <UAlert
          v-if="session.current.lastError"
          color="error"
          variant="subtle"
          icon="i-lucide-circle-x"
          :title="session.current.lastError"
        />
      </div>

      <SharedEmptyState
        v-else
        icon="i-lucide-circle-pause"
        title="No session is running"
        description="A session starts when you press Start Streaming on the dashboard."
      />
    </SharedStagePanel>

    <!-- Live event log -->
    <SharedStagePanel
      v-if="session.events.length > 0 && session.isActive"
      title="Live event log"
      icon="i-lucide-scroll-text"
      flush
    >
      <ul class="max-h-80 divide-y divide-stage-border overflow-y-auto">
        <li
          v-for="event in session.events"
          :key="event.id"
          class="flex items-start gap-3 px-4 py-2.5"
        >
          <UIcon :name="eventIcon(event)" class="mt-0.5 size-4 shrink-0 text-stage-muted" />
          <div class="min-w-0 flex-1">
            <div class="text-sm">{{ event.message }}</div>
            <div class="mt-0.5 text-xs text-stage-faint">{{ event.kind }}</div>
          </div>
          <span class="stage-numeric shrink-0 text-xs text-stage-faint">
            {{ formatTime(event.at) }}
          </span>
        </li>
      </ul>
    </SharedStagePanel>

    <!-- History -->
    <SharedStagePanel
      title="Session history"
      icon="i-lucide-history"
      :subtitle="`${session.historyTotal} session${session.historyTotal === 1 ? '' : 's'} recorded`"
      flush
    >
      <template #actions>
        <UButton
          size="xs"
          variant="ghost"
          icon="i-lucide-refresh-cw"
          :loading="session.isLoading"
          @click="session.refreshHistory()"
        >
          Refresh
        </UButton>
      </template>

      <SharedEmptyState
        v-if="session.history.length === 0"
        icon="i-lucide-history"
        title="No past sessions"
        description="Completed sessions are recorded here with their event logs."
      />

      <ul v-else class="divide-y divide-stage-border">
        <li v-for="past in session.history" :key="past.id">
          <button
            type="button"
            class="flex w-full items-center gap-4 px-4 py-3 text-left transition-colors hover:bg-stage-elevated/60"
            @click="toggle(past.id)"
          >
            <SharedStatusDot :tone="past.lastError ? 'error' : 'neutral'" size="sm" />
            <div class="min-w-0 flex-1">
              <div class="truncate text-sm">
                {{ past.label ?? 'Untitled session' }}
              </div>
              <div class="mt-0.5 text-xs text-stage-muted">
                {{ formatDateTime(past.startedAt ?? past.createdAt) }}
              </div>
            </div>
            <dl class="hidden shrink-0 gap-6 text-xs sm:flex">
              <div class="text-right">
                <dt class="text-stage-faint">Duration</dt>
                <dd class="stage-numeric">{{ formatDuration(past.durationMs) }}</dd>
              </div>
              <div class="text-right">
                <dt class="text-stage-faint">Peak</dt>
                <dd class="stage-numeric">{{ past.peakReceivers }}</dd>
              </div>
              <div class="text-right">
                <dt class="text-stage-faint">Total</dt>
                <dd class="stage-numeric">{{ past.totalReceivers }}</dd>
              </div>
            </dl>
            <UIcon
              :name="expandedId === past.id ? 'i-lucide-chevron-up' : 'i-lucide-chevron-down'"
              class="size-4 shrink-0 text-stage-muted"
            />
          </button>

          <div v-if="expandedId === past.id" class="border-t border-stage-border bg-stage-bg/50">
            <p v-if="past.lastError" class="px-4 pt-3 text-sm text-stage-error">
              {{ past.lastError }}
            </p>
            <ul v-if="session.events.length > 0" class="divide-y divide-stage-border">
              <li
                v-for="event in session.events"
                :key="event.id"
                class="flex items-start gap-3 px-4 py-2"
              >
                <SharedStatusDot :tone="severityTone(event.severity)" size="sm" class="mt-1.5" />
                <div class="min-w-0 flex-1">
                  <div class="text-sm">{{ event.message }}</div>
                  <div class="mt-0.5 text-xs text-stage-faint">{{ event.kind }}</div>
                </div>
                <span class="stage-numeric shrink-0 text-xs text-stage-faint">
                  {{ formatTime(event.at) }}
                </span>
              </li>
            </ul>
            <p v-else class="px-4 py-3 text-sm text-stage-muted">
              No events were recorded for this session.
            </p>
          </div>
        </li>
      </ul>
    </SharedStagePanel>
  </div>
</template>
