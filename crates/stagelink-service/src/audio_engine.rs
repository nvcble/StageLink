//! Capture lifecycle, owned by the service.
//!
//! Sits between the control channel (which asks for devices to be opened and
//! closed) and `audio-capture` (which does it). The important property is that
//! nothing here is ever called from the capture thread: the engine holds the
//! `CaptureHandle`, the meter accumulator and the ring consumer, and the audio
//! thread only ever touches the lock-free structures it was handed at start-up.

use std::sync::Arc;

use audio_capture::{
    CaptureConfig, CaptureHandle, CaptureSink, CaptureStats, DeviceInfo, NegotiatedCapture,
    enumerate_devices,
};
use audio_core::{AudioRing, LevelAccumulator, RingConsumer, RingStats, ShareMode};
use parking_lot::Mutex;
use tracing::{info, warn};

/// Capture ring capacity.
///
/// Deliberately modest. Capacity here is not free headroom: audio waiting in
/// the ring is audio nobody has heard yet, so anything the ring is actually
/// *using* is latency every listener pays. 60 ms still absorbs a Windows
/// scheduling stall of several capture periods, and is a bound on how far
/// behind live a stall can ever put us.
///
/// This was 250 ms, which was sized as though a deep buffer were purely
/// insurance. It was not: with the sender consuming at exactly real time and
/// nothing discarding the excess, a full ring meant a permanent quarter-second
/// delay. See [`AudioEngine::reset_ring`] and `RingConsumer::trim_to`.
const RING_MILLIS: f64 = 60.0;

/// Packets of backlog tolerated in the capture ring before the oldest audio is
/// discarded.
///
/// Two is enough to ride out the jitter of a 10 ms capture period feeding a
/// 5 ms packet cadence without ever trimming in normal operation; a third
/// gives the sender thread one late wake-up of slack. Beyond that the audio is
/// not late, it is *stale*, and skipping forward is the cheaper of the two
/// evils.
const MAX_BACKLOG_PACKETS: usize = 3;

/// What the engine is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "Reconnecting is set by the Phase 2 reconnection handler")]
pub enum CaptureState {
    Idle,
    Opening,
    Running,
    Reconnecting,
    Error,
}

impl CaptureState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Opening => "opening",
            Self::Running => "running",
            Self::Reconnecting => "reconnecting",
            Self::Error => "error",
        }
    }
}

/// A full picture of the capture path, as reported over the control channel.
#[derive(Debug, Clone)]
pub struct AudioStatus {
    pub state: CaptureState,
    pub device: Option<DeviceInfo>,
    pub channel_index: Option<u16>,
    pub negotiated: Option<NegotiatedCapture>,
    pub frames_captured: u64,
    pub xruns: u64,
    pub device_connected: bool,
    pub started_at: Option<i64>,
    pub last_error: Option<String>,
    pub realtime_granted: bool,
}

/// Owns the capture device and the buffers feeding the packetiser.
pub struct AudioEngine {
    inner: Mutex<EngineInner>,
    /// Shared with the capture thread; read by the telemetry task.
    levels: Arc<LevelAccumulator>,
}

struct EngineInner {
    state: CaptureState,
    handle: Option<CaptureHandle>,
    /// Consumed by the packetiser once streaming is wired up (Phase 3).
    ring: Option<RingConsumer>,
    ring_stats: Option<Arc<RingStats>>,
    stats: Option<Arc<CaptureStats>>,
    device: Option<DeviceInfo>,
    channel_index: Option<u16>,
    negotiated: Option<NegotiatedCapture>,
    started_at: Option<i64>,
    last_error: Option<String>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                state: CaptureState::Idle,
                handle: None,
                ring: None,
                ring_stats: None,
                stats: None,
                device: None,
                channel_index: None,
                negotiated: None,
                started_at: None,
                last_error: None,
            }),
            levels: Arc::new(LevelAccumulator::new()),
        }
    }

    /// The meter accumulator the telemetry task drains.
    #[must_use]
    pub fn levels(&self) -> Arc<LevelAccumulator> {
        Arc::clone(&self.levels)
    }

    /// Enumerates capture endpoints.
    ///
    /// Blocking (it talks to COM and may activate devices), so callers run it
    /// on a blocking task rather than the async runtime.
    pub fn list_devices(probe_formats: bool) -> audio_capture::Result<Vec<DeviceInfo>> {
        enumerate_devices(probe_formats)
    }

    /// Opens a device and starts capturing one mono channel.
    ///
    /// Closes any previous capture first, so repeated calls are safe and the
    /// operator can switch inputs without restarting the service.
    pub fn open(
        &self,
        device_id: &str,
        channel_index: u16,
        sample_rate: u32,
        share_mode: ShareMode,
        now_ms: i64,
    ) -> audio_capture::Result<AudioStatus> {
        // Find the device first so "no such device" and "wrong channel" are
        // reported before anything is torn down.
        let devices = enumerate_devices(false)?;
        let device = devices
            .iter()
            .find(|candidate| candidate.id == device_id)
            .cloned()
            .ok_or_else(|| audio_capture::CaptureError::DeviceNotFound(device_id.to_owned()))?;

        if !device.state.is_usable() {
            return Err(audio_capture::CaptureError::DeviceUnavailable(
                match device.state {
                    audio_capture::DeviceState::Disabled => "disabled",
                    audio_capture::DeviceState::Unplugged => "unplugged",
                    audio_capture::DeviceState::NotPresent => "not present",
                    audio_capture::DeviceState::Active => "active",
                },
            ));
        }
        if !device.has_channel(channel_index) {
            return Err(audio_capture::CaptureError::ChannelOutOfRange {
                requested: channel_index,
                available: device.input_channels,
            });
        }

        self.close(now_ms);

        {
            let mut inner = self.inner.lock();
            inner.state = CaptureState::Opening;
            inner.last_error = None;
        }

        self.levels.reset();
        let (producer, consumer) = AudioRing::with_duration(sample_rate.max(8_000), RING_MILLIS)
            .map_err(audio_capture::CaptureError::Core)?;
        let ring_stats = producer.stats();

        let sink = CaptureSink { ring: producer, levels: Arc::clone(&self.levels) };
        let config = CaptureConfig {
            device_id: device_id.to_owned(),
            channel_index,
            requested_sample_rate: sample_rate,
            share_mode,
            ring_millis: Some(RING_MILLIS),
        };

        match audio_capture::start_capture(config, sink) {
            Ok(handle) => {
                let negotiated = handle.negotiated();
                let stats = handle.stats();
                let mut inner = self.inner.lock();
                inner.state = CaptureState::Running;
                inner.handle = Some(handle);
                inner.ring = Some(consumer);
                inner.ring_stats = Some(ring_stats);
                inner.stats = Some(stats);
                inner.device = Some(device);
                inner.channel_index = Some(channel_index);
                inner.negotiated = Some(negotiated);
                inner.started_at = Some(now_ms);
                drop(inner);

                info!(
                    device_id,
                    channel_index,
                    sample_rate = negotiated.format.sample_rate,
                    engine_converted = negotiated.engine_converted,
                    "capture device opened"
                );
                Ok(self.status())
            }
            Err(error) => {
                let mut inner = self.inner.lock();
                inner.state = CaptureState::Error;
                inner.last_error = Some(error.to_string());
                drop(inner);
                warn!(device_id, %error, "could not open the capture device");
                Err(error)
            }
        }
    }

    /// Stops capture and releases the device. Safe to call when idle.
    pub fn close(&self, _now_ms: i64) {
        // The handle is taken out under the lock and dropped *outside* it:
        // dropping joins the capture thread, and holding a lock across a thread
        // join is how deadlocks happen.
        let handle = {
            let mut inner = self.inner.lock();
            inner.ring = None;
            inner.ring_stats = None;
            inner.stats = None;
            inner.negotiated = None;
            inner.started_at = None;
            inner.state = CaptureState::Idle;
            inner.handle.take()
        };
        if let Some(handle) = handle {
            handle.stop();
            info!("capture device released");
        }
    }

    /// Current capture status.
    pub fn status(&self) -> AudioStatus {
        let inner = self.inner.lock();
        let (frames, xruns, realtime, lost) = match &inner.stats {
            Some(stats) => (
                stats.frames_captured(),
                // The engine reports driver glitches and ring overruns together:
                // from the operator's point of view both are a gap in the audio.
                stats.xruns()
                    + inner
                        .ring_stats
                        .as_ref()
                        .map_or(0, |ring| ring.overrun_events()),
                stats.realtime_granted(),
                stats.device_lost(),
            ),
            None => (0, 0, false, false),
        };

        // A capture thread that exited because the interface vanished must be
        // reported as an error, not as healthily running.
        let state = if lost && inner.state == CaptureState::Running {
            CaptureState::Error
        } else {
            inner.state
        };

        AudioStatus {
            state,
            device: inner.device.clone(),
            channel_index: inner.channel_index,
            negotiated: inner.negotiated,
            frames_captured: frames,
            xruns,
            device_connected: !lost && inner.device.is_some(),
            started_at: inner.started_at,
            last_error: if lost {
                Some("the capture device was disconnected".to_owned())
            } else {
                inner.last_error.clone()
            },
            realtime_granted: realtime,
        }
    }

    /// Whether capture is delivering frames right now.
    pub fn is_running(&self) -> bool {
        let inner = self.inner.lock();
        inner.state == CaptureState::Running
            && inner.stats.as_ref().is_some_and(|stats| !stats.device_lost())
    }

    /// Buffered capture audio in milliseconds, for the latency breakdown.
    pub fn ring_millis(&self) -> Option<f64> {
        let inner = self.inner.lock();
        let rate = inner.negotiated?.format.sample_rate;
        inner.ring.as_ref().map(|ring| ring.occupied_millis(rate))
    }

    /// Ring buffer health, for diagnostics.
    pub fn ring_stats(&self) -> Option<Arc<RingStats>> {
        self.inner.lock().ring_stats.clone()
    }

    /// Pops exactly one packet of captured frames for the sender thread.
    ///
    /// Returns false when the ring does not hold a whole packet, which the
    /// sender records as an underrun rather than sending a short one.
    ///
    /// The ring stays owned by the engine rather than being handed to the
    /// sender, because the diagnostics page reads its occupancy for the
    /// latency breakdown. The mutex is taken 200 times a second on the sender
    /// thread and held for a memcpy; the *capture* thread holds the ring's
    /// producer and never touches this lock, so the real-time path is
    /// unaffected either way.
    ///
    /// Trims before reading, so the backlog is bounded on every packet rather
    /// than only when a stream starts. The sender never bursts to catch up —
    /// see `stream_server::sender`'s pacing notes — so without this, every
    /// scheduling stall would add its own delay permanently, and a long enough
    /// session on a loaded machine would drift back towards the old behaviour.
    pub fn pop_frames(&self, out: &mut [f32]) -> bool {
        let mut inner = self.inner.lock();
        let Some(ring) = inner.ring.as_mut() else {
            return false;
        };
        // The bound is on the backlog *before* this read, so the ring is left
        // holding at most MAX_BACKLOG_PACKETS - 1 once the packet comes out.
        ring.trim_to(out.len() * MAX_BACKLOG_PACKETS);
        ring.pop_exact(out)
    }

    /// Drops audio captured before now, so a starting stream begins at live.
    ///
    /// Called when a stream starts. A device is usually opened minutes before
    /// anyone presses Start — the operator is setting levels — and nothing
    /// reads the ring in the meantime, so by then it holds a full ring of audio
    /// from whenever the device was opened. Starting the sender on that means
    /// every performer hears the whole ring's worth of delay for the entire
    /// session, with nothing downstream able to detect it: the packets are
    /// perfectly paced and perfectly sequenced, just late.
    pub fn reset_ring(&self) {
        let mut inner = self.inner.lock();
        if let Some(ring) = inner.ring.as_mut() {
            let discarded = ring.occupied();
            ring.clear();
            if discarded > 0 {
                let rate = inner
                    .negotiated
                    .map_or(48_000, |negotiated| negotiated.format.sample_rate);
                info!(
                    discarded_ms = (discarded as f64 * 1_000.0) / f64::from(rate.max(1)),
                    "discarded buffered capture audio so the stream starts at live"
                );
            }
        }
    }

    /// Negotiated sample rate, if a device is open.
    pub fn sample_rate(&self) -> Option<u32> {
        self.inner
            .lock()
            .negotiated
            .as_ref()
            .map(|negotiated| negotiated.format.sample_rate)
    }

    /// Capture-callback timing, for diagnostics.
    pub fn capture_stats(&self) -> Option<Arc<CaptureStats>> {
        self.inner.lock().stats.clone()
    }

    /// Marks the engine as reconnecting after a device loss.
    ///
    /// Not yet called: automatic reconnection is driven by the
    /// `audio.autoReconnect` setting and is wired up with the device-change
    /// notification handler. Kept because the state it sets is already part of
    /// the published `AudioStatus` contract.
    #[allow(dead_code, reason = "reconnection handler is Phase 2 follow-up work")]
    pub fn mark_reconnecting(&self) {
        let mut inner = self.inner.lock();
        if inner.state != CaptureState::Idle {
            inner.state = CaptureState::Reconnecting;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_engine_is_idle_and_reports_no_device() {
        let engine = AudioEngine::new();
        let status = engine.status();
        assert_eq!(status.state, CaptureState::Idle);
        assert!(status.device.is_none());
        assert_eq!(status.frames_captured, 0);
        assert!(!status.device_connected);
        assert!(status.last_error.is_none());
        assert!(!engine.is_running());
    }

    #[test]
    fn an_idle_engine_reports_no_buffered_audio_rather_than_zero() {
        let engine = AudioEngine::new();
        assert_eq!(
            engine.ring_millis(),
            None,
            "with no device open there is no buffer depth to report"
        );
    }

    #[test]
    fn closing_an_idle_engine_is_harmless() {
        let engine = AudioEngine::new();
        engine.close(0);
        engine.close(0);
        assert_eq!(engine.status().state, CaptureState::Idle);
    }

    #[test]
    fn resetting_the_ring_with_no_device_open_is_harmless() {
        // Stream start calls this unconditionally, and a stream can be asked
        // for in states where no ring exists yet.
        let engine = AudioEngine::new();
        engine.reset_ring();
        engine.reset_ring();
        assert_eq!(engine.status().state, CaptureState::Idle);
    }

    #[test]
    fn popping_from_an_engine_with_no_device_reports_no_audio() {
        let engine = AudioEngine::new();
        let mut packet = [0.0_f32; 240];
        assert!(!engine.pop_frames(&mut packet));
    }

    /// The latency regression, and the reason any of this exists.
    ///
    /// A capture device is opened long before anyone starts a stream, and
    /// nothing reads the ring until then, so it saturates. The bug this guards
    /// against is a sender that then begins reading at the *oldest* sample and
    /// stays a full ring behind live for the whole session — inaudible as a
    /// glitch, obvious as a delay, and invisible to every controller
    /// downstream because the packets are perfectly paced and sequenced.
    ///
    /// Hardware-dependent: skips cleanly where there is no capture endpoint.
    #[cfg(windows)]
    #[test]
    fn a_stream_starting_on_a_saturated_ring_still_begins_near_live() {
        let Ok(devices) = AudioEngine::list_devices(false) else {
            eprintln!("skipping: enumeration unavailable");
            return;
        };
        let Some(device) = devices
            .iter()
            .find(|d| d.state.is_usable() && d.input_channels > 0)
        else {
            eprintln!("skipping: no active capture endpoint on this machine");
            return;
        };

        let engine = AudioEngine::new();
        if engine.open(&device.id, 0, 48_000, ShareMode::Shared, 0).is_err() {
            eprintln!("skipping: could not open {}", device.name);
            return;
        }

        // Nobody is reading: this is the operator setting levels before the
        // show. Well past RING_MILLIS, so the ring is full.
        std::thread::sleep(core::time::Duration::from_millis(300));

        let saturated = engine.ring_millis().expect("a device is open");
        assert!(
            saturated > RING_MILLIS / 2.0,
            "the ring should have filled with nothing reading it, got {saturated:.1} ms — \
             this test proves nothing if it did not"
        );

        // Pressing Start. The capture thread keeps pushing while we measure,
        // so the assertion is that the backlog was *discarded* rather than
        // that the ring is empty at some exact instant — a live producer makes
        // any tight absolute bound here a race.
        engine.reset_ring();
        let after_reset = engine.ring_millis().expect("still open");
        assert!(
            after_reset < saturated / 2.0,
            "a starting stream must not inherit the backlog: {after_reset:.1} ms \
             of the {saturated:.1} ms survived the reset"
        );

        // And the bound holds against a ring that saturates again mid-session,
        // which is what a scheduling stall on a loaded machine looks like.
        std::thread::sleep(core::time::Duration::from_millis(300));
        assert!(
            engine.ring_millis().is_some_and(|millis| millis > RING_MILLIS / 2.0),
            "precondition: the ring should be full again"
        );

        // 240 frames is one 5 ms packet at 48 kHz.
        let mut packet = [0.0_f32; 240];
        assert!(engine.pop_frames(&mut packet), "a full ring holds a whole packet");

        // Again a live producer is refilling as we measure, so the bound is
        // "nowhere near saturated" rather than an exact packet count. The real
        // target is MAX_BACKLOG_PACKETS - 1 packets, about 10 ms.
        let bounded = engine.ring_millis().expect("still open");
        assert!(
            bounded < RING_MILLIS / 2.0,
            "reading must trim the backlog, not carry it: {bounded:.1} ms of a \
             {RING_MILLIS:.0} ms ring"
        );
        assert!(bounded < saturated, "the backlog must shrink, not persist");

        // The trim discarded audio, so the receiver has to be told.
        let trims = engine
            .ring_stats()
            .expect("a device is open")
            .trim_events();
        assert!(trims > 0, "a trim that is not counted cannot be reported as a glitch");

        engine.close(0);
    }

    #[test]
    fn the_meter_starts_with_nothing_to_report() {
        let engine = AudioEngine::new();
        assert!(
            engine.levels().drain().is_none(),
            "an engine with no capture must not produce a meter reading"
        );
    }

    #[cfg(windows)]
    #[test]
    fn opening_an_unknown_device_reports_device_not_found_and_stays_idle() {
        let engine = AudioEngine::new();
        let result = engine.open("{no-such-endpoint}", 0, 48_000, ShareMode::Shared, 0);
        assert!(matches!(
            result,
            Err(audio_capture::CaptureError::DeviceNotFound(_))
        ));
        assert_eq!(engine.status().state, CaptureState::Idle);
    }

    #[cfg(windows)]
    #[test]
    fn opening_an_out_of_range_channel_is_refused_before_any_teardown() {
        let Ok(devices) = AudioEngine::list_devices(false) else {
            eprintln!("skipping: enumeration unavailable");
            return;
        };
        let Some(device) = devices.iter().find(|d| d.state.is_usable()) else {
            eprintln!("skipping: no active capture endpoint");
            return;
        };

        let engine = AudioEngine::new();
        let result = engine.open(
            &device.id,
            device.input_channels + 10,
            48_000,
            ShareMode::Shared,
            0,
        );
        match result {
            Err(audio_capture::CaptureError::ChannelOutOfRange { requested, available }) => {
                assert_eq!(requested, device.input_channels + 10);
                assert_eq!(available, device.input_channels);
            }
            other => panic!("expected ChannelOutOfRange, got {other:?}"),
        }
    }

    /// Hardware-dependent end-to-end check: open a real device, confirm the
    /// meter and the ring both receive the real signal, then release it.
    #[cfg(windows)]
    #[test]
    fn a_real_device_feeds_both_the_meter_and_the_ring() {
        let Ok(devices) = AudioEngine::list_devices(false) else {
            eprintln!("skipping: enumeration unavailable");
            return;
        };
        let Some(device) = devices
            .iter()
            .find(|d| d.state.is_usable() && d.input_channels > 0)
        else {
            eprintln!("skipping: no active capture endpoint on this machine");
            return;
        };

        let engine = AudioEngine::new();
        let status = match engine.open(&device.id, 0, 48_000, ShareMode::Shared, 1_000) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("skipping: could not open {}: {error}", device.name);
                return;
            }
        };

        assert_eq!(status.state, CaptureState::Running);
        assert_eq!(status.channel_index, Some(0));
        assert!(status.negotiated.is_some());
        assert_eq!(status.started_at, Some(1_000));
        assert!(engine.is_running());

        std::thread::sleep(core::time::Duration::from_millis(300));

        let running = engine.status();
        assert!(
            running.frames_captured > 0,
            "a running capture must report frames"
        );
        assert!(running.device_connected);
        assert!(
            engine.levels().drain().is_some(),
            "the meter must see the captured audio"
        );
        assert!(
            engine.ring_millis().is_some_and(|millis| millis >= 0.0),
            "the ring must report a buffer depth while running"
        );

        engine.close(2_000);
        let closed = engine.status();
        assert_eq!(closed.state, CaptureState::Idle);
        assert!(!engine.is_running());
    }
}
