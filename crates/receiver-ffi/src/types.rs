//! The structs that cross the boundary.
//!
//! All `repr(C)`, all plain data, no pointers out. Every string the caller
//! passes in is borrowed for the duration of the call and copied if it is
//! kept, so the app owns its own memory and there is nothing to free.

use std::ffi::c_char;

use receiver_client::halves::SessionControl;
use receiver_core::receiver::Preset;

/// What the app tells us about itself when joining.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlrConfig {
    /// Shown in the operator's roster, e.g. "Pixel 8".
    pub device_name: *const c_char,
    /// One of `android`, `ios`, `windows`, `macos`, `linux`.
    pub platform: *const c_char,
    pub platform_version: *const c_char,
    pub app_version: *const c_char,
    /// Long-term public key, hex. Not a secret: it is how the desktop
    /// recognises this device across reconnects, so a label and a revocation
    /// both stick to it.
    pub public_key: *const c_char,
    /// 0 tight, 1 normal, 2 safe.
    pub preset: u32,
    /// Bind an OS-chosen audio port instead of the one in the QR code.
    ///
    /// Wrong on a phone, where the QR's port is expected. Needed when several
    /// receivers share one address.
    pub ephemeral_audio_port: bool,
}

impl Default for SlrConfig {
    fn default() -> Self {
        Self {
            device_name: std::ptr::null(),
            platform: std::ptr::null(),
            platform_version: std::ptr::null(),
            app_version: std::ptr::null(),
            public_key: std::ptr::null(),
            preset: 1,
            ephemeral_audio_port: false,
        }
    }
}

/// Three-band EQ, as the performer set it.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlrEq {
    pub low_hz: f32,
    pub low_db: f32,
    pub low_q: f32,
    pub mid_hz: f32,
    pub mid_db: f32,
    pub mid_q: f32,
    pub high_hz: f32,
    pub high_db: f32,
    pub high_q: f32,
}

impl Default for SlrEq {
    fn default() -> Self {
        Self {
            low_hz: 150.0,
            low_db: 0.0,
            low_q: 0.707,
            mid_hz: 1_000.0,
            mid_db: 0.0,
            mid_q: 0.9,
            high_hz: 4_000.0,
            high_db: 0.0,
            high_q: 0.707,
        }
    }
}

/// Everything the app's UI needs, in one poll.
///
/// A struct copy rather than a call per field, because the app polls this at
/// about 10 Hz and nine FFI calls to draw one screen would be wasteful for no
/// benefit. Every field is either desktop-measured by us or measured here;
/// none is guessed.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SlrStats {
    /// 0 disconnected, 1 connecting, 2 connected, 3 failed.
    pub connection_state: u32,
    /// True once the desktop has sent audio parameters. Before that, silence
    /// is expected rather than a fault — the operator has not started yet.
    pub has_stream: bool,
    pub playing: bool,

    pub packets_received: u64,
    pub packets_lost: u64,
    pub loss_percent: f32,
    pub duplicates: u64,
    pub reordered: u64,
    /// Datagrams that were not valid StageLink packets.
    pub rejected: u64,
    /// Datagrams from an address other than the desktop we joined.
    pub foreign_datagrams: u64,

    /// Jitter-buffer depth, in milliseconds.
    pub buffer_ms: f32,
    /// Separate gaps concealed. More telling than a sample count: fifty short
    /// gaps and one long one sound nothing alike.
    pub concealed_runs: u64,
    pub concealed_ms: f32,
    pub resyncs: u64,
    /// Clock difference the drift controller settled on, in parts per million.
    pub drift_ppm: f32,
    /// Samples the safety clip caught. Should always be zero; a non-zero
    /// value means the limiter is misbehaving.
    pub clipped: u64,

    pub heartbeats: u64,
    pub missed_heartbeats: u64,
    /// Control-channel round-trip time, or −1 if none has been measured.
    ///
    /// **Not audio latency.** It is the control path, over TCP, and reporting
    /// it as latency would be a measurement of the wrong thing. The app must
    /// not label it as such.
    pub control_rtt_ms: f32,
}

/// Maps a preset index, defaulting to normal.
#[must_use]
pub fn preset_from(value: u32) -> Preset {
    match value {
        0 => Preset::Tight,
        2 => Preset::Safe,
        _ => Preset::Normal,
    }
}

/// Builds the stats struct from a control half.
#[must_use]
pub fn stats_from(control: &SessionControl) -> SlrStats {
    let telemetry = control.telemetry();
    let status = control.control_status();
    let socket = control.socket_stats();

    SlrStats {
        connection_state: match status.state() {
            receiver_client::ConnectionState::Disconnected => 0,
            receiver_client::ConnectionState::Connecting => 1,
            receiver_client::ConnectionState::Connected => 2,
            receiver_client::ConnectionState::Failed(_) => 3,
        },
        has_stream: control.has_stream(),
        playing: telemetry.playing,

        packets_received: telemetry.packets_received,
        packets_lost: telemetry.packets_lost,
        loss_percent: telemetry.loss_percent as f32,
        duplicates: telemetry.duplicates,
        reordered: telemetry.reordered,
        rejected: telemetry.rejected,
        foreign_datagrams: socket.foreign(),

        buffer_ms: telemetry.buffer_health_ms as f32,
        concealed_runs: telemetry.concealed_runs,
        concealed_ms: telemetry.concealed_ms as f32,
        resyncs: telemetry.resyncs,
        drift_ppm: telemetry.resample_ratio_ppm as f32,
        clipped: telemetry.clipped,

        heartbeats: status.heartbeats(),
        missed_heartbeats: status.missed_heartbeats(),
        // −1 rather than 0: zero would read as "instant", and a receiver that
        // cannot measure something must say so.
        control_rtt_ms: status.rtt_ms().map_or(-1.0, |rtt| rtt as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_preset_falls_back_to_normal() {
        assert_eq!(preset_from(0), Preset::Tight);
        assert_eq!(preset_from(1), Preset::Normal);
        assert_eq!(preset_from(2), Preset::Safe);
        assert_eq!(preset_from(99), Preset::Normal);
        assert_eq!(preset_from(u32::MAX), Preset::Normal);
    }

    #[test]
    fn a_fresh_stats_struct_claims_nothing() {
        let stats = SlrStats::default();
        assert_eq!(stats.connection_state, 0);
        assert!(!stats.has_stream);
        assert!(!stats.playing);
        assert_eq!(stats.packets_received, 0);
        // Default is zero here rather than −1, so the app must check
        // `connection_state` before showing a round-trip time at all.
        assert_eq!(stats.control_rtt_ms, 0.0);
    }

    /// The layouts are the ABI. A field added in the middle, or a type
    /// changed, silently shifts every field after it — and the symptom is
    /// nonsense on screen rather than a compile error, because the app is in
    /// another language.
    #[test]
    fn the_struct_layouts_are_pinned() {
        use std::mem::{align_of, size_of};

        // Five pointers, a u32 and a bool.
        assert_eq!(size_of::<SlrConfig>(), 5 * size_of::<usize>() + 8);
        assert_eq!(align_of::<SlrConfig>(), align_of::<usize>());

        // Nine floats.
        assert_eq!(size_of::<SlrEq>(), 36);
        assert_eq!(align_of::<SlrEq>(), 4);

        // Pinned by value: if this changes, SLR_ABI_VERSION must change too.
        assert_eq!(size_of::<SlrStats>(), 136);
        assert_eq!(align_of::<SlrStats>(), 8);
    }

    #[test]
    fn the_eq_defaults_are_flat() {
        let eq = SlrEq::default();
        assert_eq!(eq.low_db, 0.0);
        assert_eq!(eq.mid_db, 0.0);
        assert_eq!(eq.high_db, 0.0);
        // And the frequencies are the ones receiver-core defaults to, so an
        // app that sends the defaults changes nothing.
        let settings = receiver_core::dsp::MonitorSettings::default();
        assert_eq!(eq.low_hz, settings.low.frequency_hz);
        assert_eq!(eq.mid_hz, settings.mid.frequency_hz);
        assert_eq!(eq.high_hz, settings.high.frequency_hz);
    }
}
