//! The StageLink receiver, as a platform-free library.
//!
//! This is the half of the system a performer actually depends on: it turns a
//! stream of UDP datagrams into a continuous mono signal in their ears, and it
//! has to keep doing that while the network misbehaves, the two clocks drift
//! apart, and nobody is in a position to look at a screen.
//!
//! ## What is here, and what is deliberately not
//!
//! Here: the jitter buffer, loss concealment, drift compensation, and the
//! personal monitor chain (gain, EQ, limiter). No sockets, no audio device, no
//! threads of its own, no platform API.
//!
//! Not here: anything that touches an operating system. The audio sink is per
//! platform — Oboe on Android, an AudioUnit on iOS — and is the only piece that
//! has to be written twice. That boundary is why roughly four fifths of the
//! difficult code can be tested on a desktop with no phone attached.
//!
//! ## Why Rust rather than the app's own language
//!
//! Two reasons that are not about preference:
//!
//! 1. **A garbage-collector pause is an audible dropout.** Absorbing one needs
//!    tens of milliseconds of extra buffer, which roughly doubles the latency
//!    budget the product is built around.
//! 2. **The loss figures have to agree with the desktop's.**
//!    `docs/protocol/pairing.md` obligation 2 requires it, and the only way to
//!    guarantee it is to run the same `sequence_delta` code on both ends.
//!
//! ## Hearing safety is not configurable away
//!
//! This library puts audio directly into someone's ears, often louder than
//! they would choose, for an hour at a time. The limiter is therefore not fully
//! parameterisable by the application on top: the ceiling is clamped to a hard
//! maximum no call can exceed, and an unconditional clip runs afterwards. See
//! [`dsp`].

pub mod conceal;
pub mod drift;
pub mod dsp;
pub mod resample;
pub mod jitter;

pub use conceal::Concealer;
pub use drift::{DriftConfig, DriftController};
pub use dsp::{MonitorChain, MonitorSettings, SafetyProfile};
pub use jitter::{Insert, JitterBuffer, Ready, SLOTS};
pub use resample::Resampler;
