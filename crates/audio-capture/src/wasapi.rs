//! WASAPI endpoint enumeration and format negotiation.
//!
//! Everything in this module talks to the real Windows audio stack. If a call
//! fails, the failure is reported — nothing here substitutes a plausible
//! default for a value the driver declined to give us.

use audio_core::{AudioFormat, SampleFormat, ShareMode};
use tracing::{debug, warn};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_OK};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE, AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED, DEVICE_STATE,
    DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
    DEVICE_STATEMASK_ALL, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eCapture, eConsole,
};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::Media::KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::core::PCWSTR;

use crate::device::{AudioApi, DeviceInfo, DeviceState, PROBE_SAMPLE_RATES};
use crate::error::{CaptureError, Result};

/// `WAVE_FORMAT_EXTENSIBLE`. Not exposed as a constant by the `windows` crate.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `WAVE_FORMAT_IEEE_FLOAT`.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// `WAVE_FORMAT_PCM`.
const WAVE_FORMAT_PCM: u16 = 0x0001;

/// 100-nanosecond units per millisecond, the unit WASAPI uses for durations.
pub const REFTIMES_PER_MILLISEC: i64 = 10_000;

/// RAII guard for a COM apartment on the current thread.
///
/// Every thread that touches WASAPI needs its own apartment. The capture thread
/// creates one for its whole lifetime; enumeration creates one per call.
pub struct ComApartment {
    /// False when the thread was already in an apartment, in which case we must
    /// not call `CoUninitialize` and unbalance somebody else's bookkeeping.
    owns: bool,
}

impl ComApartment {
    /// Enters a multithreaded apartment.
    pub fn enter() -> Result<Self> {
        // SAFETY: CoInitializeEx is safe to call on any thread; we inspect the
        // HRESULT rather than assuming success.
        let hresult = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hresult.is_ok() {
            Ok(Self { owns: true })
        } else if hresult == RPC_E_CHANGED_MODE {
            // The thread is already in a single-threaded apartment. WASAPI
            // still works; we simply do not own the apartment.
            debug!("thread already in a COM apartment with a different mode");
            Ok(Self { owns: false })
        } else {
            Err(CaptureError::ComInit(format!(
                "CoInitializeEx returned 0x{:08X}",
                hresult.0
            )))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: balanced against the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

/// Creates the endpoint enumerator. Requires an active apartment.
fn device_enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: standard COM activation of a documented system class.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(Into::into)
}

/// Reads an endpoint's stable id.
fn endpoint_id(device: &IMMDevice) -> Result<String> {
    // SAFETY: GetId returns a COM-allocated wide string that we must free.
    unsafe {
        let raw = device.GetId()?;
        let text = raw
            .to_string()
            .map_err(|error| CaptureError::Wasapi(format!("endpoint id is not valid UTF-16: {error}")))?;
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(text)
    }
}

/// Reads an endpoint's friendly name, falling back to its id.
fn endpoint_name(device: &IMMDevice, fallback: &str) -> String {
    // SAFETY: the property store is released when the interface drops, and
    // PROPVARIANT's Drop impl calls PropVariantClear.
    let name = unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .map(|value| value.to_string())
    };
    match name {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) => fallback.to_owned(),
        Err(error) => {
            // A nameless device is still usable, so warn rather than fail.
            warn!(%error, "could not read device friendly name");
            fallback.to_owned()
        }
    }
}

fn map_device_state(state: DEVICE_STATE) -> DeviceState {
    match state {
        DEVICE_STATE_ACTIVE => DeviceState::Active,
        DEVICE_STATE_DISABLED => DeviceState::Disabled,
        DEVICE_STATE_UNPLUGGED => DeviceState::Unplugged,
        DEVICE_STATE_NOTPRESENT => DeviceState::NotPresent,
        // Windows documents exactly four states; anything else is treated as
        // unusable rather than optimistically assumed active.
        _ => DeviceState::NotPresent,
    }
}

/// Interprets a `WAVEFORMATEX` (or `WAVEFORMATEXTENSIBLE`) as an [`AudioFormat`].
///
/// # Safety
/// `format` must point to a valid `WAVEFORMATEX`, and when `wFormatTag` is
/// `WAVE_FORMAT_EXTENSIBLE` it must actually be a `WAVEFORMATEXTENSIBLE`.
unsafe fn read_format(format: *const WAVEFORMATEX) -> Result<AudioFormat> {
    // `WAVEFORMATEX` and `WAVEFORMATEXTENSIBLE` are `#[repr(packed)]`, so every
    // field is copied out by value before use. Borrowing a packed field would
    // produce a misaligned reference.
    let header = unsafe { core::ptr::read_unaligned(format) };
    let format_tag = header.wFormatTag;
    let bits_per_sample = header.wBitsPerSample;
    let sample_rate = header.nSamplesPerSec;
    let channels = header.nChannels;

    let is_float = match format_tag {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_EXTENSIBLE => {
            let extensible = unsafe { core::ptr::read_unaligned(format.cast::<WAVEFORMATEXTENSIBLE>()) };
            let subformat = extensible.SubFormat;
            if subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                true
            } else if subformat == KSDATAFORMAT_SUBTYPE_PCM {
                false
            } else {
                return Err(CaptureError::FormatUnsupported(format!(
                    "unrecognised WAVEFORMATEXTENSIBLE subformat {subformat:?}"
                )));
            }
        }
        other => {
            return Err(CaptureError::FormatUnsupported(format!(
                "unrecognised wFormatTag 0x{other:04X}"
            )));
        }
    };

    let sample_format = SampleFormat::from_bits(bits_per_sample, is_float).map_err(|_| {
        CaptureError::FormatUnsupported(format!(
            "{bits_per_sample} bit {} is not supported",
            if is_float { "float" } else { "integer" }
        ))
    })?;

    Ok(AudioFormat::new(sample_rate, channels, sample_format))
}

/// Builds a `WAVEFORMATEXTENSIBLE` describing the format we want to capture.
///
/// Always extensible: multi-channel interfaces need the channel mask, and the
/// explicit subformat GUID avoids ambiguity about float vs integer.
#[must_use]
pub fn build_format(format: &AudioFormat) -> WAVEFORMATEXTENSIBLE {
    let bits = (format.sample_format.bytes_per_sample() * 8) as u16;
    let block_align = format.bytes_per_frame() as u16;

    let mut extensible = WAVEFORMATEXTENSIBLE::default();
    extensible.Format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_EXTENSIBLE,
        nChannels: format.channels,
        nSamplesPerSec: format.sample_rate,
        nAvgBytesPerSec: format.sample_rate * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: bits,
        cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
    };
    extensible.Samples.wValidBitsPerSample = bits;
    // A zero channel mask means "driver decides", which is what we want: we are
    // extracting a single physical input, not laying out a speaker arrangement.
    extensible.dwChannelMask = 0;
    extensible.SubFormat = match format.sample_format {
        SampleFormat::F32 => KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        SampleFormat::I16 | SampleFormat::I24 | SampleFormat::I32 => KSDATAFORMAT_SUBTYPE_PCM,
    };
    extensible
}

#[must_use]
pub const fn share_mode_flag(mode: ShareMode) -> AUDCLNT_SHAREMODE {
    match mode {
        ShareMode::Shared => AUDCLNT_SHAREMODE_SHARED,
        ShareMode::Exclusive => AUDCLNT_SHAREMODE_EXCLUSIVE,
    }
}

/// Activates the audio client for an endpoint.
pub fn activate_client(device: &IMMDevice) -> Result<IAudioClient> {
    // SAFETY: Activate is the documented way to obtain IAudioClient.
    unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }.map_err(Into::into)
}

/// Reads the endpoint's current mix format.
pub fn mix_format(client: &IAudioClient) -> Result<AudioFormat> {
    // SAFETY: GetMixFormat returns a COM-allocated WAVEFORMATEX we must free.
    unsafe {
        let raw = client.GetMixFormat()?;
        let result = read_format(raw);
        CoTaskMemFree(Some(raw.cast()));
        result
    }
}

/// Reads the driver's default and minimum periods, converted to frames.
pub fn device_periods(client: &IAudioClient, sample_rate: u32) -> (Option<u32>, Option<u32>) {
    let mut default_hns = 0_i64;
    let mut minimum_hns = 0_i64;
    // SAFETY: both out-pointers are valid for the duration of the call.
    let ok = unsafe {
        client
            .GetDevicePeriod(Some(&mut default_hns), Some(&mut minimum_hns))
            .is_ok()
    };
    if !ok {
        return (None, None);
    }
    let to_frames = |hns: i64| -> Option<u32> {
        if hns <= 0 {
            return None;
        }
        // 1 hns == 100 ns, so frames = hns * rate / 10_000_000.
        u32::try_from((hns as i128 * i128::from(sample_rate) / 10_000_000) as i64).ok()
    };
    (to_frames(default_hns), to_frames(minimum_hns))
}

/// Asks the driver whether it will accept an exact format.
///
/// Two subtleties that are easy to get wrong, and both cost real debugging time:
///
/// 1. In **shared** mode `ppClosestMatch` must not be NULL. Passing NULL makes
///    the call fail with `E_POINTER` for every format, which looks exactly like
///    "this driver supports nothing".
/// 2. The call returns `S_FALSE` — which is *not* an error HRESULT — when the
///    format is unsupported but a closest match was produced. Only `S_OK` means
///    "yes, exactly this".
pub fn is_format_supported(client: &IAudioClient, format: &AudioFormat, mode: ShareMode) -> bool {
    let wave = build_format(format);
    let mut closest: *mut WAVEFORMATEX = core::ptr::null_mut();

    // SAFETY: `wave` outlives the call; `closest` is a valid out-pointer whose
    // allocation, if any, we free immediately below.
    let hresult = unsafe {
        client.IsFormatSupported(
            share_mode_flag(mode),
            core::ptr::from_ref(&wave).cast::<WAVEFORMATEX>(),
            Some(&mut closest),
        )
    };

    if !closest.is_null() {
        // SAFETY: WASAPI allocated this with CoTaskMemAlloc.
        unsafe { CoTaskMemFree(Some(closest.cast())) };
    }

    hresult == S_OK
}

/// Asks the driver which of [`PROBE_SAMPLE_RATES`] it will accept.
///
/// In shared mode WASAPI answers for the audio engine rather than the hardware,
/// which is still the right answer for us: it is what we will actually be able
/// to open.
fn probe_sample_rates(client: &IAudioClient, template: &AudioFormat, mode: ShareMode) -> Vec<u32> {
    PROBE_SAMPLE_RATES
        .iter()
        .copied()
        .filter(|&rate| {
            let candidate = AudioFormat::new(rate, template.channels, template.sample_format);
            is_format_supported(client, &candidate, mode)
        })
        .collect()
}

/// Describes one endpoint.
fn describe_device(
    device: &IMMDevice,
    default_id: Option<&str>,
    probe_formats: bool,
) -> Result<DeviceInfo> {
    let id = endpoint_id(device)?;
    let name = endpoint_name(device, &id);
    // SAFETY: GetState is a simple accessor on a live interface.
    let state = map_device_state(unsafe { device.GetState() }?);

    // A device that is not active cannot be activated, so report what we know
    // from the property store and leave the format fields at the OS defaults
    // rather than failing the whole enumeration.
    if !state.is_usable() {
        return Ok(DeviceInfo {
            is_default: default_id == Some(id.as_str()),
            id,
            name,
            api: AudioApi::Wasapi,
            state,
            input_channels: 0,
            default_sample_rate: 0,
            default_sample_format: SampleFormat::F32,
            supported_sample_rates: Vec::new(),
            default_period_frames: None,
            minimum_period_frames: None,
        });
    }

    let client = activate_client(device)?;
    let format = mix_format(&client)?;
    let (default_period_frames, minimum_period_frames) = device_periods(&client, format.sample_rate);
    let supported_sample_rates = if probe_formats {
        probe_sample_rates(&client, &format, ShareMode::Shared)
    } else {
        Vec::new()
    };

    Ok(DeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
        api: AudioApi::Wasapi,
        state,
        input_channels: format.channels,
        default_sample_rate: format.sample_rate,
        default_sample_format: format.sample_format,
        supported_sample_rates,
        default_period_frames,
        minimum_period_frames,
    })
}

/// Enumerates every capture endpoint Windows knows about, in any state.
///
/// Disabled and unplugged devices are included deliberately: the operator needs
/// to see that the interface they configured last Sunday is currently unplugged,
/// rather than having it silently vanish from the list.
pub fn enumerate_capture_devices(probe_formats: bool) -> Result<Vec<DeviceInfo>> {
    let _apartment = ComApartment::enter()?;
    let enumerator = device_enumerator()?;

    // The default endpoint is informational; a machine with no capture device
    // at all legitimately has none.
    let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
        .ok()
        .and_then(|device| endpoint_id(&device).ok());

    // SAFETY: DEVICE_STATEMASK_ALL is the documented "every state" mask.
    let collection =
        unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE(DEVICE_STATEMASK_ALL)) }?;
    let count = unsafe { collection.GetCount() }?;

    let mut devices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = match unsafe { collection.Item(index) } {
            Ok(device) => device,
            Err(error) => {
                // One bad endpoint must not hide every other interface.
                warn!(index, %error, "skipping an endpoint that could not be opened");
                continue;
            }
        };
        match describe_device(&device, default_id.as_deref(), probe_formats) {
            Ok(info) => devices.push(info),
            Err(error) => warn!(index, %error, "skipping an endpoint that could not be described"),
        }
    }

    // Stable, operator-friendly ordering: default first, then active devices,
    // then by name. Unplugged hardware sinks to the bottom of the list.
    devices.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| b.state.is_usable().cmp(&a.state.is_usable()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    debug!(count = devices.len(), "enumerated WASAPI capture endpoints");
    Ok(devices)
}

/// Looks up a single endpoint by its stable id.
pub fn open_device(device_id: &str) -> Result<IMMDevice> {
    let enumerator = device_enumerator()?;
    let wide: Vec<u16> = device_id.encode_utf16().chain(core::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }
        .map_err(|_| CaptureError::DeviceNotFound(device_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copies the packed fields we assert on into an ordinary struct, so tests
    /// never take a reference into a `#[repr(packed)]` layout.
    struct FormatFields {
        tag: u16,
        channels: u16,
        sample_rate: u32,
        bits: u16,
        block_align: u16,
        avg_bytes: u32,
        cb_size: u16,
        subformat: windows::core::GUID,
    }

    fn fields(wave: &WAVEFORMATEXTENSIBLE) -> FormatFields {
        FormatFields {
            tag: wave.Format.wFormatTag,
            channels: wave.Format.nChannels,
            sample_rate: wave.Format.nSamplesPerSec,
            bits: wave.Format.wBitsPerSample,
            block_align: wave.Format.nBlockAlign,
            avg_bytes: wave.Format.nAvgBytesPerSec,
            cb_size: wave.Format.cbSize,
            subformat: wave.SubFormat,
        }
    }

    #[test]
    fn built_format_describes_the_requested_audio() {
        let wave = build_format(&AudioFormat::new(48_000, 2, SampleFormat::F32));
        let f = fields(&wave);
        assert_eq!(f.tag, WAVE_FORMAT_EXTENSIBLE);
        assert_eq!(f.channels, 2);
        assert_eq!(f.sample_rate, 48_000);
        assert_eq!(f.bits, 32);
        // 2 channels * 4 bytes.
        assert_eq!(f.block_align, 8);
        assert_eq!(f.avg_bytes, 48_000 * 8);
        assert_eq!(f.subformat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
    }

    #[test]
    fn integer_formats_use_the_pcm_subformat() {
        let wave = build_format(&AudioFormat::new(44_100, 1, SampleFormat::I24));
        let f = fields(&wave);
        assert_eq!(f.subformat, KSDATAFORMAT_SUBTYPE_PCM);
        assert_eq!(f.bits, 24);
        assert_eq!(f.block_align, 3);
    }

    #[test]
    fn cbsize_covers_the_extensible_tail() {
        let wave = build_format(&AudioFormat::new(48_000, 2, SampleFormat::F32));
        // WAVEFORMATEXTENSIBLE adds 22 bytes beyond WAVEFORMATEX.
        assert_eq!(fields(&wave).cb_size, 22);
    }

    #[test]
    fn a_round_trip_through_waveformatex_preserves_the_format() {
        for format in [
            AudioFormat::new(48_000, 2, SampleFormat::F32),
            AudioFormat::new(44_100, 4, SampleFormat::I16),
            AudioFormat::new(96_000, 8, SampleFormat::I32),
        ] {
            let wave = build_format(&format);
            // SAFETY: `wave` is a valid WAVEFORMATEXTENSIBLE built just above.
            let parsed =
                unsafe { read_format(core::ptr::from_ref(&wave).cast::<WAVEFORMATEX>()) }.unwrap();
            assert_eq!(parsed, format);
        }
    }

    #[test]
    fn share_modes_map_to_the_wasapi_constants() {
        assert_eq!(share_mode_flag(ShareMode::Shared), AUDCLNT_SHAREMODE_SHARED);
        assert_eq!(
            share_mode_flag(ShareMode::Exclusive),
            AUDCLNT_SHAREMODE_EXCLUSIVE
        );
    }

    #[test]
    fn every_documented_device_state_is_mapped() {
        assert_eq!(map_device_state(DEVICE_STATE_ACTIVE), DeviceState::Active);
        assert_eq!(map_device_state(DEVICE_STATE_DISABLED), DeviceState::Disabled);
        assert_eq!(
            map_device_state(DEVICE_STATE_UNPLUGGED),
            DeviceState::Unplugged
        );
        assert_eq!(
            map_device_state(DEVICE_STATE_NOTPRESENT),
            DeviceState::NotPresent
        );
        // An undocumented value must not be treated as usable.
        assert!(!map_device_state(DEVICE_STATE(0xFF)).is_usable());
    }

    #[test]
    fn com_apartment_can_be_entered_and_left_repeatedly() {
        for _ in 0..3 {
            let apartment = ComApartment::enter().expect("COM should initialise");
            drop(apartment);
        }
    }

    /// Hardware-dependent: this machine may legitimately have no capture
    /// device, so the assertion is on the *shape* of the result, not on finding
    /// a particular interface. See docs/testing.md for the manual procedure
    /// that verifies a real interface is listed.
    #[test]
    fn enumeration_returns_consistent_device_records() {
        let devices = enumerate_capture_devices(false).expect("enumeration must not fail");
        for device in &devices {
            assert!(!device.id.is_empty(), "every endpoint must have an id");
            assert!(!device.name.is_empty(), "every endpoint must have a name");
            assert_eq!(device.api, AudioApi::Wasapi);
            if device.state.is_usable() {
                assert!(
                    device.input_channels > 0,
                    "an active capture endpoint must report at least one channel"
                );
                assert!(
                    device.default_sample_rate >= 8_000,
                    "an active endpoint must report a plausible mix-format rate"
                );
            }
        }
        // At most one endpoint may be flagged as the default.
        assert!(devices.iter().filter(|d| d.is_default).count() <= 1);
    }
}
