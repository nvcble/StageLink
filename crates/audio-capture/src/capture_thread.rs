//! The WASAPI capture thread.
//!
//! This is the only genuinely real-time part of StageLink Core. The rules it
//! obeys, and why:
//!
//! * **No allocation in the loop.** Every buffer is allocated during start-up
//!   and reused. A heap allocation can block on a lock inside the allocator,
//!   and a blocked capture thread is a dropout in every performer's ears.
//! * **No locks.** The hand-off to the rest of the service is a lock-free ring
//!   buffer plus atomic counters. Nothing the UI, the database or the network
//!   does can make this thread wait.
//! * **No logging in the loop.** `tracing` can allocate and can take a writer
//!   lock. Faults are recorded in atomics and reported by the thread that reads
//!   them, or logged once on the way out.
//! * **No database or socket calls.** Packetisation and transmission happen on
//!   other threads, fed by the ring.
//! * **Never block.** If the ring is full the samples are dropped and counted.
//!   Dropping audio is bad; stalling the driver is worse.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use audio_core::{
    AudioFormat, LevelAccumulator, RingProducer, SampleFormat, ShareMode, convert,
};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient, IAudioClient, WAVEFORMATEX,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};
use windows::core::w;

use crate::error::{CaptureError, Result};
use crate::wasapi::{
    ComApartment, REFTIMES_PER_MILLISEC, activate_client, build_format, device_periods, mix_format,
    open_device, share_mode_flag,
};

/// How long the capture loop waits for a driver buffer before deciding the
/// device has stopped producing. Generous enough not to trip on a scheduling
/// hiccup, short enough that an unplugged interface is noticed quickly.
const BUFFER_WAIT_TIMEOUT_MS: u32 = 500;

/// Consecutive wait timeouts before the device is declared lost.
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 4;

/// Capture ring size. 250 ms is far more than the packetiser needs, but it is
/// only ~1 MB of `f32` and it absorbs a Windows scheduling stall without a
/// dropout.
const DEFAULT_RING_MILLIS: f64 = 250.0;

/// What to open and how.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Stable WASAPI endpoint id.
    pub device_id: String,
    /// Zero-based physical input channel carrying the monitor send.
    pub channel_index: u16,
    /// Rate we would like. The driver may refuse; the negotiated rate is
    /// reported back and is the one that counts.
    pub requested_sample_rate: u32,
    pub share_mode: ShareMode,
    /// Ring capacity in milliseconds. Defaults to [`DEFAULT_RING_MILLIS`].
    pub ring_millis: Option<f64>,
}

/// Where captured audio and meter readings go.
pub struct CaptureSink {
    /// Mono `f32` ring consumed by the packetiser.
    pub ring: RingProducer,
    /// Meter accumulator read by the telemetry thread.
    pub levels: Arc<LevelAccumulator>,
}

/// Counters written by the capture thread, read by everyone else.
#[derive(Debug, Default)]
pub struct CaptureStats {
    frames_captured: AtomicU64,
    /// Driver-reported discontinuities plus our own ring overruns.
    xruns: AtomicU64,
    /// Buffers the driver flagged as digitally silent.
    silent_buffers: AtomicU64,
    callback_max_us: AtomicU64,
    callback_total_us: AtomicU64,
    callback_count: AtomicU64,
    /// Set when the thread exits because the endpoint went away.
    device_lost: AtomicBool,
    /// True once the thread has successfully entered its loop.
    running: AtomicBool,
    /// Whether the "Pro Audio" scheduling class was actually granted.
    realtime_granted: AtomicBool,
}

impl CaptureStats {
    #[must_use]
    pub fn frames_captured(&self) -> u64 {
        self.frames_captured.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn xruns(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn silent_buffers(&self) -> u64 {
        self.silent_buffers.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn callback_max_us(&self) -> u64 {
        self.callback_max_us.load(Ordering::Relaxed)
    }
    /// Rolling mean callback duration in microseconds, or 0 before the first.
    #[must_use]
    pub fn callback_avg_us(&self) -> u64 {
        let count = self.callback_count.load(Ordering::Relaxed);
        if count == 0 {
            0
        } else {
            self.callback_total_us.load(Ordering::Relaxed) / count
        }
    }
    #[must_use]
    pub fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
    #[must_use]
    pub fn realtime_granted(&self) -> bool {
        self.realtime_granted.load(Ordering::Relaxed)
    }

    fn record_callback(&self, micros: u64) {
        self.callback_total_us.fetch_add(micros, Ordering::Relaxed);
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        let mut current = self.callback_max_us.load(Ordering::Relaxed);
        while micros > current {
            match self.callback_max_us.compare_exchange_weak(
                current,
                micros,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }
}

/// What the driver actually agreed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedCapture {
    /// Format of the stream the driver is handing us.
    pub format: AudioFormat,
    /// Capture period in frames, as granted.
    pub period_frames: u32,
    pub share_mode: ShareMode,
    /// True when the Windows audio engine is resampling for us because the
    /// endpoint's own rate differs from the one we asked for. Worth surfacing:
    /// it costs a little latency and a little quality.
    pub engine_converted: bool,
}

/// Owns a running capture thread. Dropping it stops capture.
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    stats: Arc<CaptureStats>,
    negotiated: NegotiatedCapture,
    channel_index: u16,
    device_id: String,
}

impl CaptureHandle {
    #[must_use]
    pub fn negotiated(&self) -> NegotiatedCapture {
        self.negotiated
    }

    #[must_use]
    pub fn stats(&self) -> Arc<CaptureStats> {
        Arc::clone(&self.stats)
    }

    #[must_use]
    pub fn channel_index(&self) -> u16 {
        self.channel_index
    }

    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Whether the thread is still capturing.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.stats.running() && !self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Signals the thread and waits for it to finish.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            // The loop wakes at least every BUFFER_WAIT_TIMEOUT_MS, so this
            // cannot hang for longer than that plus the drain of one buffer.
            if let Err(panic) = join.join() {
                error!(?panic, "capture thread panicked");
            }
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts capture, returning once the device is open and the loop is running.
///
/// Opening a device is a blocking, failure-prone operation (drivers, exclusive
/// mode, unplugged hardware), so it happens on the capture thread and the
/// result is handed back through a channel. That way the caller gets a real
/// error — not a thread that quietly died.
pub fn start_capture(config: CaptureConfig, sink: CaptureSink) -> Result<CaptureHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(CaptureStats::default());
    let (ready_tx, ready_rx) = mpsc::channel::<Result<NegotiatedCapture>>();

    let thread_stop = Arc::clone(&stop);
    let thread_stats = Arc::clone(&stats);
    let thread_config = config.clone();

    let join = thread::Builder::new()
        .name("stagelink-capture".to_owned())
        .spawn(move || {
            capture_main(thread_config, sink, thread_stop, thread_stats, ready_tx);
        })
        .map_err(|error| CaptureError::StartFailed(format!("could not spawn capture thread: {error}")))?;

    // The thread reports success or failure as soon as it has tried to open the
    // device. Opening can legitimately take a second or two on USB hardware.
    let negotiated = match ready_rx.recv_timeout(core::time::Duration::from_secs(15)) {
        Ok(Ok(negotiated)) => negotiated,
        Ok(Err(error)) => {
            stop.store(true, Ordering::Release);
            let _ = join.join();
            return Err(error);
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            let _ = join.join();
            return Err(CaptureError::StartFailed(
                "the capture thread did not report readiness within 15 s".to_owned(),
            ));
        }
    };

    Ok(CaptureHandle {
        stop,
        join: Some(join),
        stats,
        negotiated,
        channel_index: config.channel_index,
        device_id: config.device_id,
    })
}

/// Thread body: set up, report readiness, then run the loop.
fn capture_main(
    config: CaptureConfig,
    mut sink: CaptureSink,
    stop: Arc<AtomicBool>,
    stats: Arc<CaptureStats>,
    ready: mpsc::Sender<Result<NegotiatedCapture>>,
) {
    // The apartment must live for as long as any COM interface we hold.
    let _apartment = match ComApartment::enter() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let prepared = match prepare(&config) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    if ready.send(Ok(prepared.negotiated)).is_err() {
        // The caller gave up on us; do not open a stream nobody will read.
        return;
    }

    info!(
        device = %config.device_id,
        channel = config.channel_index,
        sample_rate = prepared.negotiated.format.sample_rate,
        channels = prepared.negotiated.format.channels,
        format = prepared.negotiated.format.sample_format.as_str(),
        period_frames = prepared.negotiated.period_frames,
        engine_converted = prepared.negotiated.engine_converted,
        "capture started"
    );

    run_capture_loop(prepared, &config, &mut sink, &stop, &stats);

    stats.running.store(false, Ordering::Release);
    info!(
        frames = stats.frames_captured(),
        xruns = stats.xruns(),
        "capture stopped"
    );
}

/// Everything the loop needs, built before the loop starts.
struct Prepared {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
    negotiated: NegotiatedCapture,
    /// Scratch buffer for the extracted mono channel. Sized once, reused.
    mono: Vec<f32>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        // SAFETY: `event` was created by CreateEventW and is owned by us.
        if !self.event.is_invalid() {
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }
}

/// Opens the endpoint, negotiates a format and initialises the stream.
fn prepare(config: &CaptureConfig) -> Result<Prepared> {
    let device = open_device(&config.device_id)?;
    let client = activate_client(&device)?;

    let endpoint_format = mix_format(&client)?;
    if config.channel_index >= endpoint_format.channels {
        return Err(CaptureError::ChannelOutOfRange {
            requested: config.channel_index,
            available: endpoint_format.channels,
        });
    }

    let (desired, engine_converted) =
        choose_format(&client, &endpoint_format, config.requested_sample_rate, config.share_mode)?;

    let mut stream_flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    if engine_converted {
        // Let the Windows audio engine resample for us rather than shipping our
        // own resampler. This is a documented shared-mode capability and keeps
        // the sample-rate conversion in one well-tested place.
        stream_flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    }

    let wave = build_format(&desired);
    let wave_ptr = core::ptr::from_ref(&wave).cast::<WAVEFORMATEX>();

    // Ask for a buffer three capture periods deep: enough that a single late
    // wake-up does not lose audio, without adding meaningful latency.
    let buffer_duration = 30 * REFTIMES_PER_MILLISEC;

    // SAFETY: `wave` outlives the Initialize call. Periodicity must be 0 when
    // AUTOCONVERTPCM is set, and 0 is also "driver default" otherwise.
    unsafe {
        client.Initialize(
            share_mode_flag(config.share_mode),
            stream_flags,
            buffer_duration,
            0,
            wave_ptr,
            None,
        )
    }
    .map_err(|error| {
        if error.code() == windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_INVALIDATED {
            CaptureError::DeviceDisconnected
        } else {
            CaptureError::FormatUnsupported(format!(
                "{} Hz, {} ch, {} in {} mode was refused: {} (0x{:08X})",
                desired.sample_rate,
                desired.channels,
                desired.sample_format.as_str(),
                config.share_mode.as_str(),
                error.message(),
                error.code().0
            ))
        }
    })?;

    // SAFETY: a manual-reset=false, initially-unsignalled, unnamed event.
    let event = unsafe { CreateEventW(None, false, false, w!("")) }
        .map_err(|error| CaptureError::StartFailed(format!("CreateEventW failed: {error}")))?;
    // SAFETY: the client owns a reference to the handle until Stop/release.
    unsafe { client.SetEventHandle(event) }.map_err(|error| {
        // Close the handle we just created before giving up on it.
        unsafe { CloseHandle(event) }.ok();
        CaptureError::StartFailed(format!("SetEventHandle failed: {error}"))
    })?;

    let capture = unsafe { client.GetService::<IAudioCaptureClient>() }?;
    let buffer_frames = unsafe { client.GetBufferSize() }?;
    let (default_period, _minimum_period) = device_periods(&client, desired.sample_rate);
    let period_frames = default_period.unwrap_or(buffer_frames);

    // The driver can hand over up to a whole buffer in one go, so the scratch
    // space is sized for the worst case and never grows afterwards.
    let mono = vec![0.0_f32; buffer_frames as usize];

    unsafe { client.Start() }
        .map_err(|error| CaptureError::StartFailed(format!("IAudioClient::Start failed: {error}")))?;

    Ok(Prepared {
        client,
        capture,
        event,
        negotiated: NegotiatedCapture {
            format: desired,
            period_frames,
            share_mode: config.share_mode,
            engine_converted,
        },
        mono,
    })
}

/// Decides what format to request.
///
/// Preference order:
/// 1. The requested rate, if the driver accepts it outright.
/// 2. The requested rate with the audio engine converting (shared mode only).
/// 3. The endpoint's own mix format, reported honestly as what we got.
fn choose_format(
    client: &IAudioClient,
    endpoint_format: &AudioFormat,
    requested_rate: u32,
    share_mode: ShareMode,
) -> Result<(AudioFormat, bool)> {
    // Capture in f32 where the endpoint already does; otherwise keep the
    // endpoint's integer depth and convert in `extract_mono_channel`.
    let sample_format = match endpoint_format.sample_format {
        SampleFormat::F32 => SampleFormat::F32,
        other => other,
    };

    let exact = AudioFormat::new(requested_rate, endpoint_format.channels, sample_format);
    if is_supported(client, &exact, share_mode) {
        return Ok((exact, false));
    }

    if share_mode == ShareMode::Shared && requested_rate != endpoint_format.sample_rate {
        // Shared mode can convert, and IsFormatSupported does not account for
        // AUTOCONVERTPCM, so this path is validated by Initialize succeeding.
        debug!(
            requested_rate,
            endpoint_rate = endpoint_format.sample_rate,
            "requesting audio-engine sample-rate conversion"
        );
        return Ok((exact, true));
    }

    if is_supported(client, endpoint_format, share_mode) {
        warn!(
            requested_rate,
            actual_rate = endpoint_format.sample_rate,
            "driver refused the requested rate; capturing at the endpoint rate"
        );
        return Ok((*endpoint_format, false));
    }

    Err(CaptureError::FormatUnsupported(format!(
        "the driver accepted neither {} Hz nor its own mix format of {} Hz in {} mode",
        requested_rate,
        endpoint_format.sample_rate,
        share_mode.as_str()
    )))
}

fn is_supported(client: &IAudioClient, format: &AudioFormat, share_mode: ShareMode) -> bool {
    crate::wasapi::is_format_supported(client, format, share_mode)
}

/// The real-time loop. See the module docs for the constraints it obeys.
fn run_capture_loop(
    mut prepared: Prepared,
    config: &CaptureConfig,
    sink: &mut CaptureSink,
    stop: &AtomicBool,
    stats: &CaptureStats,
) {
    // Ask Windows for the "Pro Audio" scheduling class. If it is refused we
    // carry on at normal priority and say so, rather than pretending we have
    // real-time scheduling we do not have.
    let mut task_index = 0_u32;
    let avrt = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) };
    match &avrt {
        Ok(_) => stats.realtime_granted.store(true, Ordering::Release),
        Err(error) => {
            stats.realtime_granted.store(false, Ordering::Release);
            warn!(%error, "Pro Audio thread priority was refused; capture runs at normal priority");
        }
    }

    let format = prepared.negotiated.format;
    let channel = config.channel_index;
    let bytes_per_frame = format.bytes_per_frame();
    let mut consecutive_timeouts = 0_u32;

    stats.running.store(true, Ordering::Release);

    while !stop.load(Ordering::Acquire) {
        // SAFETY: `event` is a valid, owned event handle.
        let wait = unsafe { WaitForSingleObject(prepared.event, BUFFER_WAIT_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            consecutive_timeouts += 1;
            if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                // Either the interface was unplugged or the driver stalled. In
                // both cases the supervisor needs to know, so stop and flag it.
                stats.device_lost.store(true, Ordering::Release);
                warn!(
                    timeouts = consecutive_timeouts,
                    "capture device stopped delivering buffers"
                );
                break;
            }
            continue;
        }
        consecutive_timeouts = 0;

        let started = std::time::Instant::now();

        // Drain every packet the driver has queued, not just one: after a
        // scheduling stall there may be several waiting.
        loop {
            let available = match unsafe { prepared.capture.GetNextPacketSize() } {
                Ok(frames) => frames,
                Err(error) => {
                    if error.code() == windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_INVALIDATED {
                        stats.device_lost.store(true, Ordering::Release);
                    }
                    // Logging here is a deliberate exception: we are leaving the
                    // loop, so the cost no longer matters.
                    error!(%error, "GetNextPacketSize failed");
                    stop.store(true, Ordering::Release);
                    break;
                }
            };
            if available == 0 {
                break;
            }

            let mut data: *mut u8 = core::ptr::null_mut();
            let mut frames = 0_u32;
            let mut flags = 0_u32;

            // SAFETY: all three out-pointers are valid; the returned buffer is
            // owned by WASAPI until the matching ReleaseBuffer.
            if let Err(error) = unsafe {
                prepared
                    .capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            } {
                if error.code() == windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_INVALIDATED {
                    stats.device_lost.store(true, Ordering::Release);
                }
                error!(%error, "IAudioCaptureClient::GetBuffer failed");
                stop.store(true, Ordering::Release);
                break;
            }

            if frames > 0 {
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let discontinuity =
                    flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
                if discontinuity {
                    stats.xruns.fetch_add(1, Ordering::Relaxed);
                }

                // The driver must never hand over more frames than the buffer
                // size we sized the scratch space from, but clamp rather than
                // trust it.
                let scratch_len = prepared.mono.len();
                let wanted = (frames as usize).min(scratch_len);
                let mono = &mut prepared.mono[..wanted];

                if silent {
                    // WASAPI may hand over an uninitialised buffer when it sets
                    // the SILENT flag, so we must write zeros rather than read
                    // whatever happens to be there.
                    stats.silent_buffers.fetch_add(1, Ordering::Relaxed);
                    mono.fill(0.0);
                    sink.levels.push(mono);
                    sink.ring.push(mono);
                    stats.frames_captured.fetch_add(u64::from(frames), Ordering::Relaxed);
                } else if !data.is_null() {
                    // SAFETY: WASAPI guarantees `frames * bytes_per_frame`
                    // readable bytes at `data` until ReleaseBuffer is called.
                    let raw =
                        unsafe { core::slice::from_raw_parts(data, wanted * bytes_per_frame) };
                    match convert::extract_mono_channel(raw, &format, channel, mono) {
                        Ok(written) => {
                            let captured = &mono[..written];
                            sink.levels.push(captured);
                            // A full ring drops samples and counts an overrun;
                            // it never blocks here.
                            let accepted = sink.ring.push(captured);
                            if accepted < written {
                                stats.xruns.fetch_add(1, Ordering::Relaxed);
                            }
                            stats
                                .frames_captured
                                .fetch_add(written as u64, Ordering::Relaxed);
                        }
                        Err(_) => {
                            // A ragged buffer means the negotiated format and
                            // the delivered data disagree: a driver fault we
                            // cannot recover from mid-stream.
                            stats.xruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }

            // SAFETY: matches the GetBuffer above exactly once.
            if let Err(error) = unsafe { prepared.capture.ReleaseBuffer(frames) } {
                error!(%error, "IAudioCaptureClient::ReleaseBuffer failed");
                stop.store(true, Ordering::Release);
                break;
            }
        }

        stats.record_callback(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
    }

    if let Ok(handle) = avrt {
        // SAFETY: reverts the characteristics we set on this thread.
        unsafe { AvRevertMmThreadCharacteristics(handle) }.ok();
    }
    // `prepared` drops here, which stops the client and closes the event.
}

/// Default ring capacity in milliseconds.
#[must_use]
pub const fn default_ring_millis() -> f64 {
    DEFAULT_RING_MILLIS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_start_at_zero_and_are_safe_to_read_before_capture() {
        let stats = CaptureStats::default();
        assert_eq!(stats.frames_captured(), 0);
        assert_eq!(stats.xruns(), 0);
        assert_eq!(stats.callback_avg_us(), 0, "must not divide by zero");
        assert_eq!(stats.callback_max_us(), 0);
        assert!(!stats.running());
        assert!(!stats.device_lost());
    }

    #[test]
    fn callback_timing_tracks_max_and_mean() {
        let stats = CaptureStats::default();
        stats.record_callback(100);
        stats.record_callback(300);
        stats.record_callback(200);
        assert_eq!(stats.callback_max_us(), 300);
        assert_eq!(stats.callback_avg_us(), 200);
    }

    #[test]
    fn opening_an_unknown_device_fails_without_spawning_a_live_thread() {
        let levels = Arc::new(LevelAccumulator::new());
        let (ring, _consumer) = audio_core::AudioRing::with_duration(48_000, 100.0).unwrap();
        let result = start_capture(
            CaptureConfig {
                device_id: "{not-a-real-endpoint}".to_owned(),
                channel_index: 0,
                requested_sample_rate: 48_000,
                share_mode: ShareMode::Shared,
                ring_millis: None,
            },
            CaptureSink { ring, levels },
        );
        match result {
            Err(CaptureError::DeviceNotFound(id)) => {
                assert_eq!(id, "{not-a-real-endpoint}");
            }
            Err(other) => panic!("expected DeviceNotFound, got {other:?}"),
            Ok(_) => panic!("a made-up endpoint id must not open a capture stream"),
        }
    }

    /// Hardware-dependent. Runs only when an active capture endpoint exists;
    /// skips cleanly otherwise so CI on a machine with no audio input passes.
    /// See docs/testing.md for the physical-input verification procedure.
    #[test]
    fn captures_real_audio_from_the_default_device_when_one_exists() {
        let devices = match crate::enumerate_devices(false) {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("skipping: enumeration unavailable ({error})");
                return;
            }
        };
        let Some(device) = devices
            .iter()
            .find(|d| d.state.is_usable() && d.input_channels > 0)
        else {
            eprintln!("skipping: no active capture endpoint on this machine");
            return;
        };

        let levels = Arc::new(LevelAccumulator::new());
        let (ring, mut consumer) =
            audio_core::AudioRing::with_duration(device.default_sample_rate, 500.0).unwrap();

        let handle = match start_capture(
            CaptureConfig {
                device_id: device.id.clone(),
                channel_index: 0,
                requested_sample_rate: 48_000,
                share_mode: ShareMode::Shared,
                ring_millis: None,
            },
            CaptureSink { ring, levels: Arc::clone(&levels) },
        ) {
            Ok(handle) => handle,
            Err(error) => {
                // A device can be present but held exclusively by something
                // else. That is an environment fact, not a test failure.
                eprintln!("skipping: could not open {}: {error}", device.name);
                return;
            }
        };

        let negotiated = handle.negotiated();
        assert!(negotiated.format.sample_rate >= 8_000);
        assert!(negotiated.format.channels >= 1);

        // Give the driver time to deliver several periods.
        thread::sleep(core::time::Duration::from_millis(400));

        let stats = handle.stats();
        assert!(
            stats.frames_captured() > 0,
            "an open capture stream must deliver frames"
        );
        assert!(consumer.occupied() > 0, "captured audio must reach the ring");

        // The meter must have seen the same audio. Note this asserts the meter
        // ran, not that there was signal: an interface with nothing patched in
        // correctly reads silence.
        assert!(levels.drain().is_some(), "the meter must observe the frames");

        let mut scratch = vec![0.0_f32; 256];
        assert!(consumer.pop(&mut scratch) > 0);

        handle.stop();
    }
}
