//! Splitting a session into the two halves a phone actually has.
//!
//! The audio callback and the app's UI thread cannot share one object behind
//! one lock. If they did, a volume change arriving while the callback held the
//! lock would block the UI — and far worse, a UI thread holding it while the
//! callback asked for frames would produce a gap in somebody's monitor feed.
//!
//! So the session is split:
//!
//! * [`SessionRender`] is owned by the audio callback and nothing else. It
//!   holds the `AudioOutput` and renders frames.
//! * [`SessionControl`] is owned by the app. It pumps the control channel,
//!   reports telemetry, and *posts* settings changes.
//!
//! Settings cross between them through [`SettingsInbox`], which the render
//! half drains with `try_lock` — never a blocking lock. If the app is
//! mid-write when a callback arrives, the change applies one block later,
//! which is five milliseconds and inaudible. The alternative, waiting, is a
//! dropout.

use std::sync::Arc;

use parking_lot::Mutex;
use receiver_core::dsp::MonitorSettings;
use receiver_core::receiver::{AudioOutput, Preset, Telemetry, TelemetryHandle};

use crate::audio::{AudioSocket, SocketStats};
use crate::control::{ControlClient, ControlEvent, ControlStatus, TelemetryReport};

/// Settings waiting to be applied by the render half.
///
/// Only the latest matters: a performer dragging a volume fader produces
/// dozens of changes a second and only the one they stopped at is interesting,
/// so this overwrites rather than queueing.
#[derive(Default)]
pub struct SettingsInbox {
    monitor: Mutex<Option<MonitorSettings>>,
    preset: Mutex<Option<Preset>>,
}

impl SettingsInbox {
    /// Posts new settings. Called from the app thread.
    pub fn post(&self, settings: MonitorSettings) {
        *self.monitor.lock() = Some(settings);
    }

    /// Posts a new buffer preset.
    pub fn post_preset(&self, preset: Preset) {
        *self.preset.lock() = Some(preset);
    }

    /// Takes whatever is pending, without ever waiting.
    ///
    /// `try_lock` rather than `lock`: this runs on the audio thread, and the
    /// one thing it must never do is block. A missed poll costs one block of
    /// delay in applying a change.
    fn take(&self) -> (Option<MonitorSettings>, Option<Preset>) {
        let monitor = self.monitor.try_lock().and_then(|mut slot| slot.take());
        let preset = self.preset.try_lock().and_then(|mut slot| slot.take());
        (monitor, preset)
    }
}

/// The audio callback's half.
///
/// `Send` but not `Sync`: it is moved onto the audio thread and belongs to
/// that thread alone. The type system says so, which is the point.
pub struct SessionRender {
    audio: AudioOutput,
    settings: Arc<SettingsInbox>,
    /// Scratch for stereo interleaving, allocated once.
    mono: Vec<f32>,
}

impl SessionRender {
    /// Fills `out` with mono frames. **This is the audio callback.**
    ///
    /// Allocation-free and lock-free in the blocking sense. The only locks are
    /// `try_lock` on the settings inbox, which cannot wait.
    pub fn render(&mut self, out: &mut [f32]) {
        self.drain_settings();
        self.audio.render(out);
    }

    /// Fills an interleaved stereo buffer by duplicating the mono mix.
    ///
    /// Both ears get the same signal — **never synthesised stereo.** The
    /// desktop sends one pre-mixed mono feed, and inventing a stereo image
    /// from it would place instruments where they are not.
    pub fn render_stereo(&mut self, interleaved: &mut [f32]) {
        self.drain_settings();
        let frames = interleaved.len() / 2;
        if self.mono.len() < frames {
            // Grows once, on the first callback or after a buffer-size change.
            // Oboe can change its burst size on a route change, and refusing
            // to grow would mean silence on the new route.
            self.mono.resize(frames, 0.0);
        }
        self.audio.render(&mut self.mono[..frames]);
        for (frame, pair) in interleaved.chunks_exact_mut(2).enumerate() {
            pair[0] = self.mono[frame];
            pair[1] = self.mono[frame];
        }
    }

    fn drain_settings(&mut self) {
        let (monitor, preset) = self.settings.take();
        if let Some(monitor) = monitor {
            self.audio.apply_settings(monitor);
        }
        if let Some(preset) = preset {
            self.audio.set_preset(preset);
        }
    }

    #[must_use]
    pub fn telemetry(&self) -> Telemetry {
        self.audio.telemetry()
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.audio.is_playing()
    }
}

/// The app's half.
pub struct SessionControl {
    control: ControlClient,
    socket: AudioSocket,
    telemetry: TelemetryHandle,
    settings: Arc<SettingsInbox>,
    status: Arc<ControlStatus>,
    confirmed_sample_rate: Option<u32>,
    session_id: Option<u32>,
}

impl SessionControl {
    /// Applies the performer's settings.
    ///
    /// Posted rather than applied: the audio thread owns the chain, and it
    /// picks this up on its next callback.
    pub fn apply_settings(&self, settings: MonitorSettings) {
        self.settings.post(settings);
    }

    pub fn set_preset(&self, preset: Preset) {
        self.settings.post_preset(preset);
    }

    /// Pumps the control channel and refreshes the telemetry it reports.
    ///
    /// Called from the app's own loop — it takes locks and allocates, and the
    /// audio callback must do neither.
    pub fn poll(&mut self) -> Vec<ControlEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.control.try_next_event() {
            if let ControlEvent::StreamParams(params) = &event {
                self.confirmed_sample_rate = Some(params.sample_rate);
                self.session_id = Some(params.session_id);
            }
            events.push(event);
        }

        let telemetry = self.telemetry.read();
        self.control.set_telemetry(TelemetryReport {
            packets_received: telemetry.packets_received,
            packets_lost: telemetry.packets_lost,
            loss_percent: telemetry.loss_percent,
            // Not measured yet. Reported as zero rather than invented; the
            // limitation is recorded in docs/mobile-plan.md.
            jitter_ms: 0.0,
            buffer_health_ms: telemetry.buffer_health_ms,
            underruns: telemetry.concealed_runs,
            playback_state: if telemetry.playing { "playing" } else { "buffering" },
            // Only the platform sink knows this, and it is not wired through
            // yet. A receiver that cannot measure something says so rather
            // than reporting zero.
            output_latency_ms: None,
            resample_ratio_ppm: Some(telemetry.resample_ratio_ppm),
        });

        events
    }

    #[must_use]
    pub fn telemetry(&self) -> Telemetry {
        self.telemetry.read()
    }

    #[must_use]
    pub fn control_status(&self) -> Arc<ControlStatus> {
        Arc::clone(&self.status)
    }

    #[must_use]
    pub fn socket_stats(&self) -> Arc<SocketStats> {
        self.socket.stats()
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    /// Whether the desktop has told us the audio parameters. Until it has, no
    /// audio is expected — the operator has not started the stream.
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

/// Builds the two halves. Internal; `ReceiverSession::into_halves` is the way
/// in.
pub(crate) fn split(
    control: ControlClient,
    socket: AudioSocket,
    audio: AudioOutput,
    status: Arc<ControlStatus>,
    confirmed_sample_rate: Option<u32>,
    session_id: Option<u32>,
) -> (SessionControl, SessionRender) {
    let settings = Arc::new(SettingsInbox::default());
    let telemetry = audio.telemetry_handle();
    (
        SessionControl {
            control,
            socket,
            telemetry,
            settings: Arc::clone(&settings),
            status,
            confirmed_sample_rate,
            session_id,
        },
        SessionRender {
            audio,
            settings,
            mono: Vec::new(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use receiver_core::dsp::SafetyProfile;

    #[test]
    fn the_inbox_keeps_only_the_latest_settings() {
        let inbox = SettingsInbox::default();
        for gain in [-6.0, -3.0, 0.0] {
            inbox.post(MonitorSettings { gain_db: gain, ..MonitorSettings::default() });
        }
        let (monitor, _) = inbox.take();
        assert_eq!(
            monitor.unwrap().gain_db,
            0.0,
            "a fader drag produces dozens of changes; only where it stopped matters"
        );
        assert!(inbox.take().0.is_none(), "taking twice yields nothing");
    }

    #[test]
    fn settings_and_presets_travel_independently() {
        let inbox = SettingsInbox::default();
        inbox.post_preset(Preset::Safe);
        let (monitor, preset) = inbox.take();
        assert!(monitor.is_none());
        assert_eq!(preset, Some(Preset::Safe));
    }

    /// The audio thread must never wait. Taking from the inbox while the app
    /// thread holds it yields nothing rather than blocking, and the change
    /// applies on the next callback.
    #[test]
    fn taking_while_the_app_holds_the_lock_does_not_block() {
        let inbox = SettingsInbox::default();
        inbox.post(MonitorSettings {
            safety: SafetyProfile::Standard,
            ..MonitorSettings::default()
        });

        let held = inbox.monitor.lock();
        let (monitor, _) = inbox.take();
        assert!(monitor.is_none(), "it should have given up rather than waited");
        drop(held);

        // And the change is still there for the next callback.
        assert!(inbox.take().0.is_some());
    }

    #[test]
    fn the_render_half_is_send_so_it_can_be_moved_to_the_audio_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<SessionRender>();
        assert_send::<SessionControl>();
    }
}
