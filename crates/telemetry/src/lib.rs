//! StageLink telemetry.
//!
//! The organising idea of this crate is [`MetricSource`]: every number that
//! reaches the operator carries a tag saying how it was obtained. An engineer
//! debugging a dropout mid-service needs to know instantly whether "3.2 % loss"
//! was counted here or claimed by a phone, and whether "7 ms" was timed or
//! calculated. Mixing the three silently is how people end up chasing a problem
//! that does not exist.

pub mod latency;
pub mod metrics;
pub mod system;

pub use latency::{LatencyBreakdown, LATENCY_DISCLAIMER};
pub use metrics::{Measurement, MetricSource, RateCounter};
pub use system::{SystemMetrics, SystemProbe};
