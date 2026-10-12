import {
  defaultSettings,
  restartRequiredFor,
  type AppSettings,
  type ServiceError,
  type SettingsPatch,
} from '@stagelink/shared-protocol'

/**
 * Application settings.
 *
 * Settings live in the service's SQLite database, not in the renderer, so they
 * survive a reinstall of the UI and are available to the service at startup
 * before any window exists. This store is a cache with write-through.
 *
 * The `restartRequired` tracking matters: a setting that cannot take effect
 * until the stream restarts must say so rather than appearing to apply. An
 * operator who changes the input channel mid-service and sees no error would
 * reasonably assume it worked.
 */
export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<AppSettings>(defaultSettings())
  const isLoading = ref(false)
  const isSaving = ref(false)
  const lastError = ref<ServiceError | null>(null)
  const loaded = ref(false)

  /** Paths changed since the last stream restart. */
  const pendingRestartPaths = ref<string[]>([])

  const needsRestart = computed(() => pendingRestartPaths.value.length > 0)
  const showSetupWizard = computed(() => settings.value.startup.showSetupWizard)
  const theme = computed(() => settings.value.appearance.theme)
  const reduceMotion = computed(() => settings.value.appearance.reduceMotion)
  const meterRefreshHz = computed(() => settings.value.appearance.meterRefreshHz)
  const transport = computed(() => settings.value.streaming.transport)

  /** True when the configured transport would be refused in a release build. */
  const usingInsecureTransport = computed(
    () => settings.value.streaming.transport === 'udp-plaintext',
  )

  async function load() {
    const { tryInvoke } = useStageLinkBridge()
    isLoading.value = true
    try {
      const result = await tryInvoke('settings.get')
      if (result.ok) {
        settings.value = result.value
        loaded.value = true
        lastError.value = null
      } else {
        lastError.value = result.error
      }
    } finally {
      isLoading.value = false
    }
  }

  /**
   * Writes a patch.
   *
   * Returns the restart-required paths the service reported, so the caller can
   * show the right message next to the control that was just changed.
   */
  async function update(patch: SettingsPatch) {
    const { tryInvoke } = useStageLinkBridge()
    isSaving.value = true
    try {
      const result = await tryInvoke('settings.update', patch)
      if (result.ok) {
        settings.value = result.value.settings
        lastError.value = null
        const touched = result.value.restartRequired
        if (touched.length > 0) {
          pendingRestartPaths.value = [
            ...new Set([...pendingRestartPaths.value, ...touched]),
          ]
        }
        return { ok: true as const, restartRequired: touched }
      }
      lastError.value = result.error
      return { ok: false as const, error: result.error }
    } finally {
      isSaving.value = false
    }
  }

  /**
   * Predicts whether a patch will need a restart, before it is sent.
   *
   * Lets a form warn as the operator types rather than only after saving.
   */
  function willNeedRestart(patch: SettingsPatch) {
    return restartRequiredFor(patch)
  }

  /** Called once the stream has actually been restarted. */
  function clearRestartFlags() {
    pendingRestartPaths.value = []
  }

  function clearError() {
    lastError.value = null
  }

  /** Marks first-run setup as complete. */
  async function completeSetup() {
    return update({ startup: { showSetupWizard: false } })
  }

  return {
    settings,
    isLoading,
    isSaving,
    lastError,
    loaded,
    pendingRestartPaths,
    needsRestart,
    showSetupWizard,
    theme,
    reduceMotion,
    meterRefreshHz,
    transport,
    usingInsecureTransport,
    load,
    update,
    willNeedRestart,
    clearRestartFlags,
    clearError,
    completeSetup,
  }
})
