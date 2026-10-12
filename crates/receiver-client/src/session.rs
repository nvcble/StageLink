//! A whole receiver session: join, receive, render, report.
//!
//! The piece the mobile app's C ABI wraps, and the piece the headless binary
//! runs. Keeping it here rather than in either consumer means the app and the
//! conformance receiver cannot drift apart in how they join or what they
//! report — which is the only reason testing the headless one proves anything
//! about the app.

use std::sync::Arc;

use anyhow::Context;

use receiver_core::dsp::MonitorSettings;
use receiver_core::receiver::{AudioOutput, Preset, ReceiverConfig, Telemetry};
use tracing::{info, warn};

use crate::audio::AudioSocket;
use crate::control::{
    ControlClient, ControlEvent, ControlStatus, DeviceIdentity, TelemetryReport,
};
use crate::join_payload::JoinPayload;

/// How a session is set up.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub identity: DeviceIdentity,
    pub preset: Preset,
    /// Nominal sample rate, used until the desktop says otherwise.
    ///
    /// The desktop's `stream.sampleRate` wins. This exists because the audio
    /// path has to be built before the welcome arrives, and 48 kHz is what
    /// every supported configuration uses.
    pub assumed_sample_rate: u32,
    pub assumed_frame_duration_ms: f64,
    /// Bind the audio socket to an OS-chosen port instead of the one in the
    /// QR code.
    ///
    /// On a phone the QR's port is right and expected. Set this to run more
    /// than one receiver on a single machine — or for any receiver behind a
    /// shared address — since two processes cannot both hold one UDP port.
    /// The bound port is declared in the handshake either way, so the desktop
    /// does not have to guess.
    pub ephemeral_audio_port: bool,
}

impl SessionConfig {
    #[must_use]
    pub fn new(identity: DeviceIdentity) -> Self {
        Self {
            identity,
            preset: Preset::Normal,
            assumed_sample_rate: 48_000,
            assumed_frame_duration_ms: 5.0,
            ephemeral_audio_port: false,
        }
    }

    /// Frames per packet implied by the assumed configuration.
    ///
    /// Used before the desktop's `welcome` arrives, since the audio path has
    /// to be sized before the handshake completes.
    #[must_use]
    pub fn frames_per_packet(&self) -> usize {
        audio_core::frames_per_packet(self.assumed_sample_rate, self.assumed_frame_duration_ms)
    }
}

/// A live receiver session.
pub struct ReceiverSession {
    control: ControlClient,
    socket: AudioSocket,
    audio: AudioOutput,
    status: Arc<ControlStatus>,
    /// Sample rate the desktop told us, once it has.
    confirmed_sample_rate: Option<u32>,
    session_id: Option<u32>,
}

impl ReceiverSession {
    /// Joins a desktop and starts receiving.
    ///
    /// The audio socket is bound **before** the handshake, for two reasons: the
    /// hello declares the port it actually got, so the desktop needs no
    /// assumption about where to send; and a port that cannot be opened is
    /// discovered before the desktop has a roster entry for a receiver that
    /// could never have heard anything.
    pub async fn join(payload: &JoinPayload, config: SessionConfig) -> anyhow::Result<Self> {
        // Built on the assumed geometry, then rebuilt if the desktop says
        // otherwise. The alternative — wait for the welcome, then bind — gives
        // up the ability to declare the port.
        let frames = config.frames_per_packet();
        let mut receiver_config = ReceiverConfig::new(config.assumed_sample_rate, frames);
        receiver_config.preset = config.preset;
        let (net, provisional_audio) = receiver_core::receiver::new(receiver_config);

        let bind = if config.ephemeral_audio_port {
            "0.0.0.0:0".to_owned()
        } else {
            payload.audio_bind()
        };
        let socket = AudioSocket::start(
            &bind,
            payload.desktop_audio_address().map(|address| address.ip()),
            net,
        )
        .with_context(|| format!("could not bind the audio socket at {bind}"))?;
        let audio_port = socket.local_address().port();

        let mut identity = config.identity.clone();
        identity.audio_port = Some(audio_port);

        let control = ControlClient::join(payload, identity).await?;
        let status = control.status();
        let welcome = status.welcome();

        // Prefer what the desktop said. The packet header does not carry the
        // sample rate, so this is the only place it comes from — and a
        // receiver that assumed 48 kHz against a 44.1 kHz stream would play
        // the mix a semitone and a half sharp, live, in somebody's ears.
        let (sample_rate, frame_ms, session_id) = match welcome
            .as_ref()
            .and_then(|welcome| welcome.stream.as_ref())
        {
            Some(stream) => (
                stream.sample_rate,
                stream.frame_duration_ms,
                Some(stream.session_id),
            ),
            None => (
                config.assumed_sample_rate,
                config.assumed_frame_duration_ms,
                None,
            ),
        };

        // The desktop's geometry differs from what was assumed, so the audio
        // path has to be rebuilt around it: the resampler's table, the jitter
        // buffer's slot size and the concealer's fade length all depend on the
        // sample rate, and none can change in place.
        let confirmed_frames = audio_core::frames_per_packet(sample_rate, frame_ms);
        let mut audio = if confirmed_frames == frames && sample_rate == config.assumed_sample_rate
        {
            provisional_audio
        } else {
            warn!(
                assumed = config.assumed_sample_rate,
                actual = sample_rate,
                "the desktop's audio geometry differs from the assumed one; rebuilding"
            );
            let mut rebuilt = ReceiverConfig::new(sample_rate, confirmed_frames);
            rebuilt.preset = config.preset;
            let (net, audio) = receiver_core::receiver::new(rebuilt);
            // The socket's input half has to be replaced too, which means a
            // new socket on the same port. Dropping the old one first, because
            // two sockets cannot hold one port.
            drop(socket);
            let replacement = AudioSocket::start(
                &format!("0.0.0.0:{audio_port}"),
                payload.desktop_audio_address().map(|address| address.ip()),
                net,
            )?;
            return Ok(Self {
                control,
                socket: replacement,
                audio,
                status,
                confirmed_sample_rate: Some(sample_rate),
                session_id,
            });
        };
        audio.apply_settings(MonitorSettings::default());

        info!(
            sample_rate,
            frames_per_packet = confirmed_frames,
            audio_port,
            session = ?session_id,
            "receiver session ready"
        );

        Ok(Self {
            control,
            socket,
            audio,
            status,
            confirmed_sample_rate: session_id.map(|_| sample_rate),
            session_id,
        })
    }

    /// Renders audio. This is the sink's callback.
    pub fn render(&mut self, out: &mut [f32]) {
        self.audio.render(out);
    }

    /// Duplicates the mono mix to both ears.
    pub fn render_stereo(&mut self, interleaved: &mut [f32], scratch: &mut Vec<f32>) {
        self.audio.render_stereo(interleaved, scratch);
    }

    /// Applies the performer's own settings.
    pub fn apply_settings(&mut self, settings: MonitorSettings) {
        self.audio.apply_settings(settings);
    }

    pub fn set_preset(&mut self, preset: Preset) {
        self.audio.set_preset(preset);
    }

    /// Pumps control events and refreshes the telemetry the client sends.
    ///
    /// Called from the app's own loop, not from the audio callback: it takes
    /// locks and allocates, and the audio callback must do neither.
    pub fn poll(&mut self) -> Vec<ControlEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.control.try_next_event() {
            if let ControlEvent::StreamParams(params) = &event {
                self.on_stream_params(params);
            }
            events.push(event);
        }

        let telemetry = self.audio.telemetry();
        self.control.set_telemetry(TelemetryReport {
            packets_received: telemetry.packets_received,
            packets_lost: telemetry.packets_lost,
            loss_percent: telemetry.loss_percent,
            // Not measured yet: reported as zero rather than invented. See the
            // limitation noted in the plan.
            jitter_ms: 0.0,
            buffer_health_ms: telemetry.buffer_health_ms,
            underruns: telemetry.concealed_runs,
            playback_state: if telemetry.playing { "playing" } else { "buffering" },
            // Unknown without a real audio sink, and a receiver that cannot
            // measure something must say so rather than report zero.
            output_latency_ms: None,
            resample_ratio_ppm: Some(telemetry.resample_ratio_ppm),
        });

        events
    }

    /// The desktop changed the audio parameters.
    fn on_stream_params(&mut self, params: &desktop_ipc::codec::StreamParams) {
        if self
            .confirmed_sample_rate
            .is_some_and(|rate| rate != params.sample_rate)
        {
            // The audio path is built around a sample rate and cannot change
            // it in place: the resampler's coefficient table, the jitter
            // buffer's slot size and the concealer's fade length all depend on
            // it. Refused loudly rather than played at the wrong rate.
            warn!(
                was = self.confirmed_sample_rate,
                now = params.sample_rate,
                "the desktop changed sample rate; this session must be rejoined"
            );
            return;
        }
        self.confirmed_sample_rate = Some(params.sample_rate);
        self.session_id = Some(params.session_id);
    }

    #[must_use]
    pub fn telemetry(&self) -> Telemetry {
        self.audio.telemetry()
    }

    #[must_use]
    pub fn control_status(&self) -> Arc<ControlStatus> {
        Arc::clone(&self.status)
    }

    #[must_use]
    pub fn socket_stats(&self) -> Arc<crate::audio::SocketStats> {
        self.socket.stats()
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    /// Whether the desktop has told us the audio parameters.
    ///
    /// Until it has, no audio is expected: the operator has not started the
    /// stream. Worth distinguishing from "joined but silent", which is a
    /// fault.
    #[must_use]
    pub const fn has_stream(&self) -> bool {
        self.confirmed_sample_rate.is_some()
    }

    /// Leaves cleanly, so the desktop drops the roster entry at once rather
    /// than waiting five heartbeats to notice.
    pub fn leave(&mut self) {
        self.control.leave();
        self.socket.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            device_name: "Test Receiver".to_owned(),
            platform: "headless-test".to_owned(),
            platform_version: None,
            app_version: "0.1.0".to_owned(),
            public_key: "ab".repeat(32),
            audio_port: Some(45_872),
        }
    }

    #[test]
    fn the_default_configuration_matches_the_protocol_defaults() {
        let config = SessionConfig::new(identity());
        assert_eq!(config.assumed_sample_rate, 48_000);
        assert_eq!(config.frames_per_packet(), 240);
        assert_eq!(config.preset, Preset::Normal);
    }

    #[test]
    fn a_different_packet_duration_changes_the_frame_count() {
        let mut config = SessionConfig::new(identity());
        config.assumed_frame_duration_ms = 10.0;
        assert_eq!(config.frames_per_packet(), 480);
        config.assumed_frame_duration_ms = 2.5;
        assert_eq!(config.frames_per_packet(), 120);
    }

    #[test]
    fn the_headless_platform_is_declared_honestly() {
        // Not "android": the desktop's roster shows what this really is, so an
        // operator debugging a session is not told a test harness is a phone.
        assert_eq!(identity().platform, "headless-test");
    }
}
