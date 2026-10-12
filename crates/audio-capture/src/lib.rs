//! Windows audio capture for StageLink.
//!
//! One job: take the FOH monitor send off a physical input of a USB audio
//! interface and deliver it as a mono `f32` stream, with honest reporting about
//! what the driver actually agreed to.
//!
//! The backend is WASAPI. ASIO is deliberately *not* assumed to exist: plenty
//! of interfaces used in small venues have no usable ASIO driver, and a system
//! that only works with one is no use. [`AudioApi::available`] reports what this
//! build can really do at runtime rather than what it was compiled against.

pub mod device;
pub mod error;

#[cfg(windows)]
pub mod wasapi;

#[cfg(windows)]
mod capture_thread;

pub use device::{AudioApi, DeviceInfo, DeviceState};
pub use error::{CaptureError, Result};

#[cfg(windows)]
pub use capture_thread::{
    CaptureConfig, CaptureHandle, CaptureSink, CaptureStats, NegotiatedCapture,
    default_ring_millis, start_capture,
};

/// Enumerates capture endpoints.
///
/// `probe_formats` additionally asks each device which sample rates it will
/// accept, which means briefly activating it — slower, and avoided unless the
/// caller asked for it (the setup wizard does; the dashboard does not).
#[cfg(windows)]
pub fn enumerate_devices(probe_formats: bool) -> Result<Vec<DeviceInfo>> {
    wasapi::enumerate_capture_devices(probe_formats)
}

/// Non-Windows builds have no capture backend. Returning an empty list here
/// keeps the service compilable and testable on other platforms (CI runs the
/// protocol and buffer tests on Linux) while making it obvious that capture is
/// unavailable rather than silently producing fake devices.
#[cfg(not(windows))]
pub fn enumerate_devices(_probe_formats: bool) -> Result<Vec<DeviceInfo>> {
    Err(error::CaptureError::UnsupportedPlatform)
}
