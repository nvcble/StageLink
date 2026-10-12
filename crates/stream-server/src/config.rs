use audio_protocol::{TransportKind, fits_in_mtu};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Frame durations the packetiser offers, in milliseconds.
///
/// Capped at 10 ms so one packet always fits a single 1500-byte datagram: 20 ms
/// of mono S16 at 48 kHz is 1920 payload bytes, which would be IP-fragmented,
/// and a fragmented audio packet is a packet that loses all of its audio when
/// any fragment is dropped.
pub const SUPPORTED_FRAME_DURATIONS_MS: [f64; 3] = [2.5, 5.0, 10.0];

pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_FRAME_DURATION_MS: f64 = 5.0;
pub const DEFAULT_STREAM_PORT: u16 = 45_872;
pub const DEFAULT_CONTROL_PORT: u16 = 45_871;
pub const DEFAULT_MAX_RECEIVERS: u16 = 16;
pub const DEFAULT_QUEUE_PACKETS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StreamConfigError {
    #[error("frame duration {0} ms is not supported (choose one of 2.5, 5 or 10)")]
    UnsupportedFrameDuration(String),

    #[error("sample rate {0} Hz is not supported")]
    UnsupportedSampleRate(u32),

    #[error("a packet of {frames} frames would be fragmented at a {mtu}-byte MTU")]
    WouldFragment { frames: usize, mtu: usize },

    #[error("ports below 1024 are privileged; {0} cannot be used")]
    PrivilegedPort(u16),

    #[error("the stream and control ports must differ (both are {0})")]
    DuplicatePort(u16),

    #[error("receiver limit must be between 1 and 64, got {0}")]
    ReceiverLimit(u16),

    #[error(
        "transport {0} is not permitted in a release build: a live monitor mix must not be sent \
         in the clear over a venue network"
    )]
    InsecureTransport(&'static str),
}

/// Validated stream configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamConfig {
    pub sample_rate: u32,
    pub frame_duration_ms: f64,
    pub transport: TransportKind,
    pub bind_address: String,
    pub stream_port: u16,
    pub control_port: u16,
    pub max_receivers: u16,
    pub per_receiver_queue_packets: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            frame_duration_ms: DEFAULT_FRAME_DURATION_MS,
            transport: TransportKind::UdpEncrypted,
            bind_address: "0.0.0.0".to_owned(),
            stream_port: DEFAULT_STREAM_PORT,
            control_port: DEFAULT_CONTROL_PORT,
            max_receivers: DEFAULT_MAX_RECEIVERS,
            per_receiver_queue_packets: DEFAULT_QUEUE_PACKETS,
        }
    }
}

impl StreamConfig {
    /// Frames of mono audio per packet.
    #[must_use]
    pub fn frames_per_packet(&self) -> usize {
        audio_core::frames_per_packet(self.sample_rate, self.frame_duration_ms)
    }

    /// Validates the configuration.
    ///
    /// `release_build` makes the transport check strict: development builds may
    /// use the plaintext transport so the headless test receiver can validate
    /// framing without a key exchange, but a packaged build must not.
    pub fn validate(&self, release_build: bool) -> Result<(), StreamConfigError> {
        if !SUPPORTED_FRAME_DURATIONS_MS.contains(&self.frame_duration_ms) {
            return Err(StreamConfigError::UnsupportedFrameDuration(
                self.frame_duration_ms.to_string(),
            ));
        }
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err(StreamConfigError::UnsupportedSampleRate(self.sample_rate));
        }

        let frames = self.frames_per_packet();
        if !fits_in_mtu(frames, self.transport.is_encrypted(), 1_500) {
            return Err(StreamConfigError::WouldFragment { frames, mtu: 1_500 });
        }

        for port in [self.stream_port, self.control_port] {
            if port < 1_024 {
                return Err(StreamConfigError::PrivilegedPort(port));
            }
        }
        if self.stream_port == self.control_port {
            return Err(StreamConfigError::DuplicatePort(self.stream_port));
        }
        if !(1..=64).contains(&self.max_receivers) {
            return Err(StreamConfigError::ReceiverLimit(self.max_receivers));
        }
        if release_build && !self.transport.allowed_in_production() {
            return Err(StreamConfigError::InsecureTransport(self.transport.as_str()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_configuration_is_valid_in_a_release_build() {
        StreamConfig::default().validate(true).unwrap();
    }

    #[test]
    fn the_default_transport_is_the_encrypted_one() {
        assert_eq!(StreamConfig::default().transport, TransportKind::UdpEncrypted);
    }

    #[test]
    fn plaintext_is_allowed_in_development_and_refused_in_a_release_build() {
        let config = StreamConfig { transport: TransportKind::UdpPlaintext, ..Default::default() };
        assert!(
            config.validate(false).is_ok(),
            "the test receiver needs plaintext during development"
        );
        assert!(matches!(
            config.validate(true),
            Err(StreamConfigError::InsecureTransport(_))
        ));
    }

    #[test]
    fn frame_geometry_matches_the_protocol() {
        let config = StreamConfig::default();
        assert_eq!(config.frames_per_packet(), 240);

        let ten_ms = StreamConfig { frame_duration_ms: 10.0, ..Default::default() };
        assert_eq!(ten_ms.frames_per_packet(), 480);
    }

    #[test]
    fn every_supported_frame_duration_validates_and_fits_one_datagram() {
        for duration in SUPPORTED_FRAME_DURATIONS_MS {
            let config = StreamConfig { frame_duration_ms: duration, ..Default::default() };
            config.validate(true).unwrap_or_else(|error| {
                panic!("{duration} ms should be valid: {error}");
            });
        }
    }

    #[test]
    fn an_unsupported_frame_duration_is_refused() {
        let config = StreamConfig { frame_duration_ms: 20.0, ..Default::default() };
        assert!(matches!(
            config.validate(true),
            Err(StreamConfigError::UnsupportedFrameDuration(_))
        ));
    }

    #[test]
    fn a_configuration_that_would_fragment_is_refused_even_at_a_legal_duration() {
        // 10 ms at 192 kHz is 1920 frames = 3840 payload bytes.
        let config = StreamConfig {
            sample_rate: 192_000,
            frame_duration_ms: 10.0,
            ..Default::default()
        };
        assert!(matches!(
            config.validate(true),
            Err(StreamConfigError::WouldFragment { .. })
        ));
    }

    #[test]
    fn privileged_and_duplicate_ports_are_refused() {
        let privileged = StreamConfig { stream_port: 80, ..Default::default() };
        assert!(matches!(
            privileged.validate(true),
            Err(StreamConfigError::PrivilegedPort(80))
        ));

        let duplicate = StreamConfig {
            stream_port: 45_871,
            control_port: 45_871,
            ..Default::default()
        };
        assert!(matches!(
            duplicate.validate(true),
            Err(StreamConfigError::DuplicatePort(45_871))
        ));
    }

    #[test]
    fn the_receiver_limit_is_bounded() {
        for limit in [0_u16, 65] {
            let config = StreamConfig { max_receivers: limit, ..Default::default() };
            assert!(
                matches!(config.validate(true), Err(StreamConfigError::ReceiverLimit(_))),
                "{limit} should be refused"
            );
        }
        let config = StreamConfig { max_receivers: 64, ..Default::default() };
        assert!(config.validate(true).is_ok());
    }

    #[test]
    fn an_implausible_sample_rate_is_refused() {
        for rate in [0_u32, 4_000, 384_000] {
            let config = StreamConfig { sample_rate: rate, ..Default::default() };
            assert!(config.validate(true).is_err(), "{rate} Hz should be refused");
        }
    }
}
