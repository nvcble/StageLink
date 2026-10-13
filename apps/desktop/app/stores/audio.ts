import type {
  AudioDevice,
  AudioLevels,
  AudioStatus,
  AudioTestResult,
  ServiceError,
} from '@stagelink/shared-protocol'

/**
 * Audio input state.
 *
 * Two things worth knowing about this store:
 *
 * 1. **Levels come from the service, never from here.** `levels` is whatever
 *    the Rust meter last reported. There is no interpolation, no decay applied
 *    in the renderer and no synthetic motion — if the service stops reporting,
 *    the meter goes quiet, which is the truth.
 * 2. **`null` is a real value.** A dB field of `null` means digital silence or
 *    "no reading", and is rendered as "—". It is never coerced to a number,
 *    because 0 dBFS is full scale and -∞ is silence; confusing the two would be
 *    dangerous in a monitoring system.
 */
export const useAudioStore = defineStore('audio', () => {
  const status = ref<AudioStatus | null>(null)
  const devices = ref<AudioDevice[]>([])
  const availableApis = ref<string[]>([])
  const levels = ref<AudioLevels | null>(null)

  const isLoadingDevices = ref(false)
  const isSelecting = ref(false)
  const isTesting = ref(false)
  const testResult = ref<AudioTestResult | null>(null)
  const lastError = ref<ServiceError | null>(null)

  /** Timestamp of the last meter frame, used to detect a stalled meter. */
  const lastLevelsAt = ref(0)

  const captureState = computed(() => status.value?.state ?? 'idle')
  const isCapturing = computed(() => captureState.value === 'running')
  const selectedDevice = computed(() => status.value?.device ?? null)
  const selectedChannel = computed(() => status.value?.channelIndex ?? null)
  const format = computed(() => status.value?.format ?? null)

  /** Devices Windows reports as present and openable. */
  const usableDevices = computed(() =>
    devices.value.filter((device) => device.state === 'active' && device.inputChannels > 0),
  )

  /** Devices the operator can see but not use, with the reason. */
  const unavailableDevices = computed(() =>
    devices.value.filter((device) => device.state !== 'active'),
  )

  /** Input channels on the selected device, as pickable options. */
  const channelOptions = computed(() => {
    const count = selectedDevice.value?.inputChannels ?? 0
    return Array.from({ length: count }, (_, index) => ({
      value: index,
      label: `Input ${index + 1}`,
    }))
  })

  /** True when the capture device has gone away mid-session. */
  const deviceLost = computed(
    () => status.value !== null && status.value.device !== null && !status.value.deviceConnected,
  )

  /**
   * True when the Windows audio engine is resampling for us.
   *
   * Surfaced because it costs a little latency and a little quality, and an
   * operator chasing latency should know the interface is not running at the
   * requested rate natively.
   */
  const engineConverting = computed(() => format.value?.engineConverted === true)

  /**
   * True when capture is on the low-latency shared path.
   *
   * Worth distinguishing from a small `periodFrames` alone: it tells an
   * operator the period is small *because* the engine granted it, rather than
   * because this driver happens to have a short default.
   */
  const lowLatency = computed(() => format.value?.lowLatency === true)

  /** True when the meter has not updated recently despite capture running. */
  const meterStalled = computed(() => {
    if (!isCapturing.value || lastLevelsAt.value === 0) return false
    return Date.now() - lastLevelsAt.value > 1_000
  })

  const isClipping = computed(() => levels.value?.clipping ?? false)
  const clipCount = computed(() => levels.value?.clipCount ?? 0)

  function applyStatus(next: AudioStatus) {
    status.value = next
    if (next.state !== 'running') {
      // Stop showing a level for a device that is no longer capturing.
      levels.value = null
      lastLevelsAt.value = 0
    }
  }

  function applyLevels(next: AudioLevels) {
    levels.value = next
    lastLevelsAt.value = Date.now()
  }

  async function refreshStatus() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('audio.getStatus')
    if (result.ok) {
      applyStatus(result.value)
      lastError.value = null
    } else {
      lastError.value = result.error
    }
  }

  async function refreshDevices(probeFormats = false) {
    const { tryInvoke } = useStageLinkBridge()
    isLoadingDevices.value = true
    try {
      const result = await tryInvoke('audio.listDevices', { probeFormats })
      if (result.ok) {
        devices.value = result.value.devices
        availableApis.value = result.value.availableApis
        lastError.value = null
      } else {
        lastError.value = result.error
      }
    } finally {
      isLoadingDevices.value = false
    }
  }

  /** Opens a device and starts metering one mono channel. */
  async function selectDevice(deviceId: string, channelIndex: number, sampleRate?: number) {
    const { tryInvoke } = useStageLinkBridge()
    isSelecting.value = true
    testResult.value = null
    try {
      const result = await tryInvoke('audio.selectDevice', {
        deviceId,
        channelIndex,
        ...(sampleRate !== undefined ? { sampleRate } : {}),
      })
      if (result.ok) {
        applyStatus(result.value)
        lastError.value = null
        return true
      }
      lastError.value = result.error
      return false
    } finally {
      isSelecting.value = false
    }
  }

  async function releaseDevice() {
    const { tryInvoke } = useStageLinkBridge()
    const result = await tryInvoke('audio.releaseDevice')
    if (result.ok) {
      applyStatus(result.value)
      lastError.value = null
      return true
    }
    lastError.value = result.error
    return false
  }

  /** Listens to the selected input and reports what is actually arriving. */
  async function testInput(durationMs = 3_000) {
    const { tryInvoke } = useStageLinkBridge()
    isTesting.value = true
    testResult.value = null
    try {
      const result = await tryInvoke('audio.test', { durationMs })
      if (result.ok) {
        testResult.value = result.value
        lastError.value = null
        return result.value
      }
      lastError.value = result.error
      return null
    } finally {
      isTesting.value = false
    }
  }

  function clearError() {
    lastError.value = null
  }

  return {
    status,
    devices,
    availableApis,
    levels,
    isLoadingDevices,
    isSelecting,
    isTesting,
    testResult,
    lastError,
    captureState,
    isCapturing,
    selectedDevice,
    selectedChannel,
    format,
    usableDevices,
    unavailableDevices,
    channelOptions,
    deviceLost,
    engineConverting,
    lowLatency,
    meterStalled,
    isClipping,
    clipCount,
    applyStatus,
    applyLevels,
    refreshStatus,
    refreshDevices,
    selectDevice,
    releaseDevice,
    testInput,
    clearError,
  }
})
