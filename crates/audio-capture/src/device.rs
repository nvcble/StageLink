use audio_core::{AudioFormat, SampleFormat};
use serde::{Deserialize, Serialize};

/// Host audio API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioApi {
    Wasapi,
    Asio,
}

impl AudioApi {
    /// APIs this binary can actually use right now.
    ///
    /// ASIO requires both a compile-time backend and a vendor driver on the
    /// machine, so it is reported only when both are present. The UI uses this
    /// to avoid offering a choice that would immediately fail.
    #[must_use]
    pub fn available() -> Vec<Self> {
        let mut apis = Vec::new();
        if cfg!(windows) {
            apis.push(Self::Wasapi);
        }
        // ASIO is not compiled into this build. When an ASIO backend is added it
        // must probe for a registered driver here before advertising itself.
        apis
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wasapi => "wasapi",
            Self::Asio => "asio",
        }
    }
}

/// Windows endpoint presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceState {
    Active,
    Disabled,
    Unplugged,
    NotPresent,
}

impl DeviceState {
    /// Whether capture could be opened on a device in this state.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Active)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Unplugged => "unplugged",
            Self::NotPresent => "notPresent",
        }
    }
}

/// A capture endpoint as reported by the OS.
///
/// Every field here comes from the driver or the endpoint property store. None
/// of it is invented: if Windows will not tell us the supported sample rates,
/// `supported_sample_rates` is empty rather than filled with plausible guesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Stable OS endpoint id. Safe to persist across reboots.
    pub id: String,
    /// Friendly name from the endpoint property store.
    pub name: String,
    pub api: AudioApi,
    pub is_default: bool,
    pub state: DeviceState,
    /// Physical input channels the endpoint exposes.
    pub input_channels: u16,
    /// Sample rate of the device mix format.
    pub default_sample_rate: u32,
    /// Sample format of the device mix format.
    pub default_sample_format: SampleFormat,
    /// Rates the driver accepted when probed. Empty means "not probed".
    pub supported_sample_rates: Vec<u32>,
    /// Driver's default period in frames, if it reported one.
    pub default_period_frames: Option<u32>,
    /// Driver's minimum period in frames, if it reported one.
    pub minimum_period_frames: Option<u32>,
}

impl DeviceInfo {
    /// The device's native format.
    #[must_use]
    pub const fn native_format(&self) -> AudioFormat {
        AudioFormat::new(
            self.default_sample_rate,
            self.input_channels,
            self.default_sample_format,
        )
    }

    /// Whether `channel` is a real input on this device.
    #[must_use]
    pub const fn has_channel(&self, channel: u16) -> bool {
        channel < self.input_channels
    }
}

/// Sample rates worth probing. Covers the rates used in practice by USB
/// interfaces; anything else is reported via the device's own mix format.
pub const PROBE_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "{0.0.1.00000000}.{abc}".into(),
            name: "Line 1/2 (Focusrite USB)".into(),
            api: AudioApi::Wasapi,
            is_default: true,
            state: DeviceState::Active,
            input_channels: 2,
            default_sample_rate: 48_000,
            default_sample_format: SampleFormat::F32,
            supported_sample_rates: vec![44_100, 48_000],
            default_period_frames: Some(480),
            minimum_period_frames: Some(144),
        }
    }

    #[test]
    fn only_active_devices_are_usable() {
        assert!(DeviceState::Active.is_usable());
        assert!(!DeviceState::Unplugged.is_usable());
        assert!(!DeviceState::Disabled.is_usable());
        assert!(!DeviceState::NotPresent.is_usable());
    }

    #[test]
    fn channel_bounds_follow_the_reported_channel_count() {
        let device = device();
        assert!(device.has_channel(0));
        assert!(device.has_channel(1));
        assert!(!device.has_channel(2));
    }

    #[test]
    fn native_format_mirrors_the_mix_format() {
        let format = device().native_format();
        assert_eq!(format.sample_rate, 48_000);
        assert_eq!(format.channels, 2);
        assert_eq!(format.sample_format, SampleFormat::F32);
    }

    #[test]
    fn asio_is_not_advertised_unless_a_backend_exists() {
        // This build has no ASIO backend compiled in, so it must not be offered.
        assert!(!AudioApi::available().contains(&AudioApi::Asio));
    }

    #[cfg(windows)]
    #[test]
    fn wasapi_is_available_on_windows() {
        assert!(AudioApi::available().contains(&AudioApi::Wasapi));
    }
}
