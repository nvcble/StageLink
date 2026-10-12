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

/// Capture ring depth. Generous: a Windows scheduling stall should cost
/// nothing, and 250 ms of mono `f32` at 48 kHz is only 48 kB.
const RING_MILLIS: f64 = 250.0;

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
