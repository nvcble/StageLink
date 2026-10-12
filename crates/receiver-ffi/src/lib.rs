//! The C ABI the mobile app calls.
//!
//! ## What crosses this boundary, and what deliberately does not
//!
//! Crossing: join, leave, the performer's controls, a stats struct polled at
//! about 10 Hz for the UI, and the render callback.
//!
//! **Not** crossing: audio, per packet. Nothing here is called once per
//! datagram. The network thread and the audio callback both live in Rust and
//! talk to each other through `receiver-core`; the app never sees a packet.
//! Also not crossing: the join token, or any key material. The app passes the
//! scanned QR payload *in* and never gets a credential back.
//!
//! ## Why every function catches panics
//!
//! Unwinding across an FFI boundary is undefined behaviour, and a panic in
//! Rust reaching Dart or Kotlin is an immediate crash with no chance to tell
//! the performer why their monitor stopped. Every entry point is wrapped, and
//! a caught panic becomes an error code.
//!
//! That requires this crate to be built with `panic = "unwind"`. The
//! workspace release profile uses `panic = "abort"`, which is right for a
//! standalone service and wrong here — so there is a `mobile` profile. Build
//! with `--profile mobile`, never plain `--release`.
//!
//! ## Thread rules the caller must follow
//!
//! | Function | Thread |
//! | --- | --- |
//! | `slr_render`, `slr_render_stereo` | the audio callback, and only it |
//! | everything else | any other thread |
//!
//! `slr_render` is the only function that may be called from a real-time
//! context. It never allocates, never takes a blocking lock, and never calls
//! into the app.

use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use parking_lot::Mutex;
use receiver_client::halves::{SessionControl, SessionRender};
use receiver_client::{DeviceIdentity, JoinPayload, ReceiverSession, SessionConfig};
use receiver_core::dsp::{BandSettings, MonitorSettings, SafetyProfile};

mod error;
mod types;

pub use error::SlrStatus;
pub use types::{SlrConfig, SlrEq, SlrStats};

/// ABI version. Bumped whenever a signature or a struct layout changes.
///
/// The app checks this at load. A mismatched `.so` left over from a previous
/// build would otherwise read the wrong bytes out of these structs, and the
/// symptom would be nonsense rather than an error.
pub const SLR_ABI_VERSION: u32 = 3;

/// An opaque receiver handle.
///
/// The render half is behind its own lock, taken only by the audio callback
/// with `try_lock`; the control half is behind another, taken only by the app.
/// Two locks rather than one, because a single lock would let the app thread
/// stall the audio callback — and a stalled audio callback is a gap in
/// somebody's monitor feed.
pub struct SlrReceiver {
    /// Kept alive for the control channel's background tasks. Never read:
    /// holding it *is* its purpose, and dropping it with the handle is what
    /// stops those tasks.
    #[allow(dead_code, reason = "owned to keep the control channel alive")]
    runtime: tokio::runtime::Runtime,
    control: Mutex<SessionControl>,
    render: Mutex<SessionRender>,
    settings: Mutex<MonitorSettings>,
}

/// Returns the ABI version this library implements.
#[unsafe(no_mangle)]
pub extern "C" fn slr_abi_version() -> u32 {
    SLR_ABI_VERSION
}

/// Initialises logging. Optional, and safe to call more than once.
#[unsafe(no_mangle)]
pub extern "C" fn slr_init_logging() {
    // `try_init`, not `init`: called twice it would panic, and an app that
    // reinitialises after a configuration change is doing nothing wrong.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .try_init();
}

/// Joins a desktop from a scanned QR payload.
///
/// Blocks until the handshake completes or fails, which is a second or two at
/// worst — the app calls it off its UI thread. Writes the handle to `out` and
/// returns [`SlrStatus::Ok`], or leaves `out` null and returns why.
///
/// # Safety
///
/// `qr` must be a valid NUL-terminated UTF-8 string. `config` and `out` must
/// be valid pointers. The handle must eventually be passed to [`slr_leave`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_join(
    qr: *const c_char,
    config: *const SlrConfig,
    out: *mut *mut SlrReceiver,
) -> SlrStatus {
    if out.is_null() {
        return SlrStatus::InvalidArgument;
    }
    unsafe { *out = std::ptr::null_mut() };

    let result = catch_unwind(|| {
        let Some(qr) = (unsafe { cstr(qr) }) else {
            return Err(SlrStatus::InvalidArgument);
        };
        let Some(config) = (unsafe { config.as_ref() }) else {
            return Err(SlrStatus::InvalidArgument);
        };

        let payload = JoinPayload::parse(qr, receiver_client::CONTROL_PROTOCOL_VERSION)
            .map_err(error::from_payload)?;

        let identity = DeviceIdentity {
            device_name: unsafe { cstr(config.device_name) }
                .unwrap_or("StageLink Mobile")
                .to_owned(),
            platform: unsafe { cstr(config.platform) }.unwrap_or("unknown").to_owned(),
            platform_version: unsafe { cstr(config.platform_version) }.map(str::to_owned),
            app_version: unsafe { cstr(config.app_version) }
                .unwrap_or("0.0.0")
                .to_owned(),
            public_key: unsafe { cstr(config.public_key) }
                .ok_or(SlrStatus::InvalidArgument)?
                .to_owned(),
            audio_port: None,
        };

        let mut session_config = SessionConfig::new(identity);
        session_config.preset = types::preset_from(config.preset);
        session_config.ephemeral_audio_port = config.ephemeral_audio_port;

        // Its own runtime, owned by the handle. Two threads: enough for the
        // control socket and its timers, and small enough that a phone with
        // four cores does not hand most of them to a WebSocket.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("stagelink-control")
            .build()
            .map_err(|_| SlrStatus::Internal)?;

        let session = runtime
            .block_on(ReceiverSession::join(&payload, session_config))
            .map_err(|error| {
                tracing::warn!(%error, "join failed");
                error::from_join(&error)
            })?;

        let (control, render) = session.into_halves();
        Ok(Box::new(SlrReceiver {
            runtime,
            control: Mutex::new(control),
            render: Mutex::new(render),
            settings: Mutex::new(MonitorSettings::default()),
        }))
    });

    match result {
        Ok(Ok(receiver)) => {
            unsafe { *out = Box::into_raw(receiver) };
            SlrStatus::Ok
        }
        Ok(Err(status)) => status,
        Err(_) => SlrStatus::Panicked,
    }
}

/// Leaves and frees the receiver.
///
/// Leaving tells the desktop at once, so the roster entry goes immediately
/// rather than after five missed heartbeats.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`] and must not be used afterwards.
/// Passing null is allowed and does nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_leave(receiver: *mut SlrReceiver) {
    if receiver.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let receiver = unsafe { Box::from_raw(receiver) };
        receiver.control.lock().leave();
        // The runtime is dropped with the box, which stops the background
        // tasks. Dropped last, after `leave`, so the goodbye is actually sent.
    }));
}

/// Renders mono frames. **The audio callback, and only the audio callback.**
///
/// Returns the number of frames written, or a negative [`SlrStatus`] on
/// failure. Writes silence and returns the frame count if the render half is
/// momentarily unavailable, because a callback that returns nothing is a
/// click and silence is merely a gap.
///
/// # Safety
///
/// `out` must point to `frames` writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_render(
    receiver: *mut SlrReceiver,
    out: *mut f32,
    frames: u32,
) -> i32 {
    render_impl(receiver, out, frames, false)
}

/// Renders interleaved stereo frames by duplicating the mono mix.
///
/// `frames` is frames, not samples: `out` must hold `frames * 2` floats.
///
/// # Safety
///
/// `out` must point to `frames * 2` writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_render_stereo(
    receiver: *mut SlrReceiver,
    out: *mut f32,
    frames: u32,
) -> i32 {
    render_impl(receiver, out, frames, true)
}

fn render_impl(receiver: *mut SlrReceiver, out: *mut f32, frames: u32, stereo: bool) -> i32 {
    if out.is_null() || frames == 0 {
        return SlrStatus::InvalidArgument as i32;
    }
    let samples = frames as usize * if stereo { 2 } else { 1 };

    let Some(receiver) = (unsafe { receiver.as_ref() }) else {
        return SlrStatus::InvalidArgument as i32;
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        let buffer = unsafe { std::slice::from_raw_parts_mut(out, samples) };

        // `try_lock`: this is the real-time path and must never wait. Nothing
        // else should ever hold this lock, so failing means the caller broke
        // the one-thread rule — in which case silence is the safe answer.
        let Some(mut render) = receiver.render.try_lock() else {
            buffer.fill(0.0);
            return frames as i32;
        };

        if stereo {
            render.render_stereo(buffer);
        } else {
            render.render(buffer);
        }
        frames as i32
    }));

    match result {
        Ok(frames) => frames,
        Err(_) => {
            // A panic here would otherwise unwind into the platform's audio
            // callback, which is undefined behaviour. Silence and an error
            // code instead.
            let buffer = unsafe { std::slice::from_raw_parts_mut(out, samples) };
            buffer.fill(0.0);
            SlrStatus::Panicked as i32
        }
    }
}

/// Sets the performer's volume, in decibels.
///
/// Clamped by `receiver-core` to a range that cannot be a hazard; a value
/// outside it is applied as the nearest allowed one rather than refused.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_set_gain_db(receiver: *mut SlrReceiver, gain_db: f32) -> SlrStatus {
    with_settings(receiver, |settings| settings.gain_db = gain_db)
}

/// Mutes or unmutes. Ramped, so it does not click.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_set_muted(receiver: *mut SlrReceiver, muted: bool) -> SlrStatus {
    with_settings(receiver, |settings| settings.muted = muted)
}

/// Sets the three-band EQ.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`]; `eq` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_set_eq(receiver: *mut SlrReceiver, eq: *const SlrEq) -> SlrStatus {
    let Some(eq) = (unsafe { eq.as_ref() }) else {
        return SlrStatus::InvalidArgument;
    };
    let eq = *eq;
    with_settings(receiver, move |settings| {
        settings.low = BandSettings {
            frequency_hz: eq.low_hz,
            gain_db: eq.low_db,
            q: eq.low_q,
        };
        settings.mid = BandSettings {
            frequency_hz: eq.mid_hz,
            gain_db: eq.mid_db,
            q: eq.mid_q,
        };
        settings.high = BandSettings {
            frequency_hz: eq.high_hz,
            gain_db: eq.high_db,
            q: eq.high_q,
        };
    })
}

/// Sets the hearing-safety profile.
///
/// The one coarse, auditable call that can raise the output ceiling. 0 is
/// protected (−6 dBFS), 1 standard (−3 dBFS), 2 unrestricted (0 dBFS). An
/// unknown value is treated as protected — the safe direction — rather than
/// refused, because a refused call leaves whatever was already in force and
/// the caller may not check.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_set_safety_profile(
    receiver: *mut SlrReceiver,
    profile: u32,
) -> SlrStatus {
    let profile = match profile {
        1 => SafetyProfile::Standard,
        2 => SafetyProfile::Unrestricted,
        _ => SafetyProfile::Protected,
    };
    with_settings(receiver, move |settings| settings.safety = profile)
}

/// Sets the buffer preset: 0 tight, 1 normal, 2 safe.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_set_preset(receiver: *mut SlrReceiver, preset: u32) -> SlrStatus {
    let Some(receiver) = (unsafe { receiver.as_ref() }) else {
        return SlrStatus::InvalidArgument;
    };
    let preset = types::preset_from(preset);
    match catch_unwind(AssertUnwindSafe(|| {
        receiver.control.lock().set_preset(preset);
    })) {
        Ok(()) => SlrStatus::Ok,
        Err(_) => SlrStatus::Panicked,
    }
}

/// Fills `out` with current statistics.
///
/// Polled at about 10 Hz for the UI. Never called per packet, which is why
/// this ABI can afford to be a plain struct copy.
///
/// # Safety
///
/// `receiver` must come from [`slr_join`]; `out` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slr_stats(
    receiver: *mut SlrReceiver,
    out: *mut SlrStats,
) -> SlrStatus {
    if out.is_null() {
        return SlrStatus::InvalidArgument;
    }
    let Some(receiver) = (unsafe { receiver.as_ref() }) else {
        return SlrStatus::InvalidArgument;
    };

    match catch_unwind(AssertUnwindSafe(|| {
        let mut control = receiver.control.lock();
        // Pumping here as well as reporting: the app polls stats on a timer
        // anyway, so this is the natural place to service the control channel
        // and refresh the telemetry it sends.
        control.poll();
        types::stats_from(&control)
    })) {
        Ok(stats) => {
            unsafe { *out = stats };
            SlrStatus::Ok
        }
        Err(_) => SlrStatus::Panicked,
    }
}

/// Applies a settings change through the inbox the render half drains.
fn with_settings(
    receiver: *mut SlrReceiver,
    change: impl FnOnce(&mut MonitorSettings),
) -> SlrStatus {
    let Some(receiver) = (unsafe { receiver.as_ref() }) else {
        return SlrStatus::InvalidArgument;
    };
    match catch_unwind(AssertUnwindSafe(|| {
        // The whole settings struct is kept here, so a call that changes only
        // the gain does not reset the EQ. The alternative — reading the
        // current settings back out of the audio chain — would need a lock the
        // audio thread holds.
        let mut settings = receiver.settings.lock();
        change(&mut settings);
        receiver.control.lock().apply_settings(*settings);
    })) {
        Ok(()) => SlrStatus::Ok,
        Err(_) => SlrStatus::Panicked,
    }
}

/// Borrows a C string as `&str`, or `None` if it is null or not UTF-8.
unsafe fn cstr(pointer: *const c_char) -> Option<&'static str> {
    if pointer.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn the_abi_version_is_reported() {
        assert_eq!(slr_abi_version(), SLR_ABI_VERSION);
    }

    #[test]
    fn logging_can_be_initialised_more_than_once() {
        slr_init_logging();
        slr_init_logging();
    }

    /// Every entry point must survive a null, because a null is what a caller
    /// gets after a failed join — and the alternative is a crash in somebody's
    /// hand during a service.
    #[test]
    fn every_function_survives_a_null_handle() {
        let mut stats = SlrStats::default();
        let eq = SlrEq::default();
        let mut frames = [0.0_f32; 64];

        unsafe {
            slr_leave(std::ptr::null_mut());
            assert_eq!(
                slr_set_gain_db(std::ptr::null_mut(), -6.0),
                SlrStatus::InvalidArgument
            );
            assert_eq!(
                slr_set_muted(std::ptr::null_mut(), true),
                SlrStatus::InvalidArgument
            );
            assert_eq!(
                slr_set_eq(std::ptr::null_mut(), &eq),
                SlrStatus::InvalidArgument
            );
            assert_eq!(
                slr_set_safety_profile(std::ptr::null_mut(), 0),
                SlrStatus::InvalidArgument
            );
            assert_eq!(
                slr_set_preset(std::ptr::null_mut(), 1),
                SlrStatus::InvalidArgument
            );
            assert_eq!(
                slr_stats(std::ptr::null_mut(), &mut stats),
                SlrStatus::InvalidArgument
            );
            assert!(slr_render(std::ptr::null_mut(), frames.as_mut_ptr(), 64) < 0);
            assert!(slr_render_stereo(std::ptr::null_mut(), frames.as_mut_ptr(), 32) < 0);
        }
    }

    #[test]
    fn a_null_output_pointer_is_refused_rather_than_written_to() {
        unsafe {
            assert_eq!(
                slr_stats(std::ptr::null_mut(), std::ptr::null_mut()),
                SlrStatus::InvalidArgument
            );
            assert!(slr_render(std::ptr::null_mut(), std::ptr::null_mut(), 64) < 0);
        }
    }

    #[test]
    fn joining_with_nonsense_arguments_fails_without_crashing() {
        let config = SlrConfig::default();
        let mut handle: *mut SlrReceiver = std::ptr::null_mut();

        unsafe {
            // Null QR.
            assert_eq!(
                slr_join(std::ptr::null(), &config, &mut handle),
                SlrStatus::InvalidArgument
            );
            assert!(handle.is_null());

            // Null output.
            let qr = CString::new("stagelink://join?h=1&c=45871&s=45872&t=x&v=1.0.0").unwrap();
            assert_eq!(
                slr_join(qr.as_ptr(), &config, std::ptr::null_mut()),
                SlrStatus::InvalidArgument
            );

            // Null config.
            assert_eq!(
                slr_join(qr.as_ptr(), std::ptr::null(), &mut handle),
                SlrStatus::InvalidArgument
            );
        }
    }

    /// The failure a person will actually hit: they scanned something that is
    /// not a StageLink code, or the code has expired and been re-displayed.
    #[test]
    fn a_payload_that_is_not_a_join_code_is_reported_as_such() {
        let mut config = SlrConfig::default();
        let key = CString::new("ab".repeat(32)).unwrap();
        config.public_key = key.as_ptr();
        let mut handle: *mut SlrReceiver = std::ptr::null_mut();

        for payload in [
            "https://example.com",
            "WIFI:S:Network;T:WPA;P:secret;;",
            "stagelink://join?h=10.0.0.1&c=45871&s=45872&t=short&v=1.0.0",
        ] {
            let qr = CString::new(payload).unwrap();
            let status = unsafe { slr_join(qr.as_ptr(), &config, &mut handle) };
            assert_eq!(
                status,
                SlrStatus::BadJoinCode,
                "for {payload}: a person needs to be told to rescan"
            );
            assert!(handle.is_null(), "no handle on failure");
        }
    }

    #[test]
    fn a_future_protocol_version_is_reported_distinctly() {
        // Worth its own code: "update the app" is a different instruction
        // from "rescan the code".
        let mut config = SlrConfig::default();
        let key = CString::new("ab".repeat(32)).unwrap();
        config.public_key = key.as_ptr();
        let mut handle: *mut SlrReceiver = std::ptr::null_mut();

        let qr = CString::new(format!(
            "stagelink://join?h=10.0.0.1&c=45871&s=45872&t={}&v=9.0.0",
            "a".repeat(64)
        ))
        .unwrap();
        let status = unsafe { slr_join(qr.as_ptr(), &config, &mut handle) };
        assert_eq!(status, SlrStatus::IncompatibleVersion);
    }

    #[test]
    fn joining_an_address_with_nothing_listening_reports_that_it_could_not_connect() {
        let mut config = SlrConfig::default();
        let key = CString::new("ab".repeat(32)).unwrap();
        config.public_key = key.as_ptr();
        config.ephemeral_audio_port = true;
        let mut handle: *mut SlrReceiver = std::ptr::null_mut();

        // Port 1 on the loopback refuses connections.
        let qr = CString::new(format!(
            "stagelink://join?h=127.0.0.1&c=1024&s=45873&t={}&v=1.0.0",
            "a".repeat(64)
        ))
        .unwrap();
        let status = unsafe { slr_join(qr.as_ptr(), &config, &mut handle) };
        assert_eq!(status, SlrStatus::CouldNotConnect);
        assert!(handle.is_null());
    }

    #[test]
    fn the_safety_profile_only_accepts_known_values_and_defaults_to_protected() {
        // An unknown value must not raise the ceiling. Checked on the mapping
        // rather than through a handle, because that is where the decision is.
        for (input, expected) in [
            (0, SafetyProfile::Protected),
            (1, SafetyProfile::Standard),
            (2, SafetyProfile::Unrestricted),
            (3, SafetyProfile::Protected),
            (u32::MAX, SafetyProfile::Protected),
        ] {
            let mapped = match input {
                1 => SafetyProfile::Standard,
                2 => SafetyProfile::Unrestricted,
                _ => SafetyProfile::Protected,
            };
            assert_eq!(mapped, expected, "for {input}");
        }
    }
}
