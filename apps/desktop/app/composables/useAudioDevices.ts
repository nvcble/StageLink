/**
 * Audio device selection.
 *
 * Wraps the audio store with the lifecycle and presentation logic the Audio
 * page and the setup wizard both need, so neither has to re-derive it.
 */
export function useAudioDevices(options: { refreshOnMount?: boolean } = {}) {
  const audio = useAudioStore()
  const { refreshOnMount = true } = options

  if (refreshOnMount) {
    onMounted(() => {
      void audio.refreshDevices(false)
      void audio.refreshStatus()
    })
  }

  /** Device options for a select control, with unavailable ones disabled. */
  const deviceOptions = computed(() =>
    audio.devices.map((device) => ({
      value: device.id,
      label: device.name,
      disabled: device.state !== 'active' || device.inputChannels === 0,
      // Shown as help text so the operator can see *why* their interface from
      // last Sunday is not selectable.
      hint: describeDeviceState(device.state, device.inputChannels),
      channels: device.inputChannels,
      sampleRate: device.defaultSampleRate,
      isDefault: device.isDefault,
    })),
  )

  /** True when the machine has nothing that could be used as an input. */
  const hasNoUsableDevice = computed(
    () => !audio.isLoadingDevices && audio.usableDevices.length === 0,
  )

  /**
   * True when the device the operator configured is currently missing.
   *
   * Worth calling out explicitly: "your Focusrite is not plugged in" is a very
   * different message from "pick an input".
   */
  const configuredDeviceMissing = computed(() => {
    const settings = useSettingsStore()
    const configuredId = settings.settings.audio.deviceId
    if (!configuredId) return false
    const found = audio.devices.find((device) => device.id === configuredId)
    return found === undefined || found.state !== 'active'
  })

  /** Re-probe sample rates, which requires activating each device. */
  async function probeFormats() {
    await audio.refreshDevices(true)
  }

  async function selectChannel(channelIndex: number) {
    const device = audio.selectedDevice
    if (!device) return false
    return audio.selectDevice(device.id, channelIndex)
  }

  return {
    audio,
    deviceOptions,
    hasNoUsableDevice,
    configuredDeviceMissing,
    probeFormats,
    selectChannel,
    describeDeviceState,
  }
}

/** Plain-language explanation of a Windows endpoint state. */
export function describeDeviceState(state: string, inputChannels: number): string {
  switch (state) {
    case 'active':
      return inputChannels > 0
        ? `${inputChannels} input channel${inputChannels === 1 ? '' : 's'}`
        : 'No input channels'
    case 'unplugged':
      return 'Unplugged — connect the interface'
    case 'disabled':
      return 'Disabled in Windows Sound settings'
    case 'notPresent':
      return 'Not present — the driver is installed but the hardware is absent'
    default:
      return state
  }
}
