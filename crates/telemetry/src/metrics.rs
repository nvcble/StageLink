use std::time::Instant;

use serde::{Deserialize, Serialize};

/// How a figure was obtained. Serialised alongside every value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricSource {
    /// Counted or timed directly by this service, on this machine.
    Measured,
    /// Derived arithmetically from configuration and measured values.
    Estimated,
    /// Sent to us by a receiver. Unverified by definition.
    ReceiverReported,
}

impl MetricSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Estimated => "estimated",
            Self::ReceiverReported => "receiver-reported",
        }
    }

    /// Whether the desktop can vouch for this figure.
    #[must_use]
    pub const fn is_authoritative(self) -> bool {
        matches!(self, Self::Measured)
    }
}

/// A scalar with its unit and provenance.
///
/// `value` is `Option` so "not measured yet" is distinct from zero — reporting
/// 0 ms of jitter when no receiver has reported is actively misleading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub value: Option<f64>,
    pub unit: &'static str,
    pub source: MetricSource,
    /// Shown as a tooltip; explains how the figure was produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

impl Measurement {
    #[must_use]
    pub const fn measured(value: Option<f64>, unit: &'static str) -> Self {
        Self { value, unit, source: MetricSource::Measured, note: None }
    }

    #[must_use]
    pub const fn estimated(value: Option<f64>, unit: &'static str) -> Self {
        Self { value, unit, source: MetricSource::Estimated, note: None }
    }

    #[must_use]
    pub const fn receiver_reported(value: Option<f64>, unit: &'static str) -> Self {
        Self { value, unit, source: MetricSource::ReceiverReported, note: None }
    }

    /// Attaches an explanatory note.
    #[must_use]
    pub const fn with_note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }

    /// A measurement that has no value yet.
    #[must_use]
    pub const fn unknown(unit: &'static str, source: MetricSource) -> Self {
        Self { value: None, unit, source, note: None }
    }
}

/// Converts a monotonically increasing counter into a per-second rate.
///
/// Used for packet rates and egress bandwidth. Keeps the previous sample so the
/// caller does not have to, and reports `None` until it has two samples — a
/// rate from a single observation is a guess, not a measurement.
#[derive(Debug)]
pub struct RateCounter {
    previous: Option<(Instant, u64)>,
    last_rate: Option<f64>,
}

impl Default for RateCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateCounter {
    #[must_use]
    pub const fn new() -> Self {
        Self { previous: None, last_rate: None }
    }

    /// Records a cumulative total and returns the rate per second since the
    /// previous sample.
    pub fn sample(&mut self, total: u64) -> Option<f64> {
        let now = Instant::now();
        let rate = match self.previous {
            Some((previous_at, previous_total)) => {
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                if elapsed <= 0.0 {
                    // Two samples in the same instant: keep the last rate
                    // rather than dividing by zero.
                    self.last_rate
                } else {
                    // A counter that went backwards means it was reset (a new
                    // stream). Report no rate for this window rather than a
                    // negative one.
                    total
                        .checked_sub(previous_total)
                        .map(|delta| delta as f64 / elapsed)
                }
            }
            None => None,
        };
        self.previous = Some((now, total));
        if rate.is_some() {
            self.last_rate = rate;
        }
        rate
    }

    /// The most recent computed rate, if any.
    #[must_use]
    pub const fn last(&self) -> Option<f64> {
        self.last_rate
    }

    /// Forgets history. Call when the underlying counter is reset.
    pub fn reset(&mut self) {
        self.previous = None;
        self.last_rate = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_measured_values_are_authoritative() {
        assert!(MetricSource::Measured.is_authoritative());
        assert!(!MetricSource::Estimated.is_authoritative());
        assert!(
            !MetricSource::ReceiverReported.is_authoritative(),
            "a receiver's own claim is never authoritative on the desktop"
        );
    }

    #[test]
    fn source_strings_match_the_control_protocol() {
        assert_eq!(MetricSource::Measured.as_str(), "measured");
        assert_eq!(MetricSource::Estimated.as_str(), "estimated");
        assert_eq!(MetricSource::ReceiverReported.as_str(), "receiver-reported");
    }

    #[test]
    fn source_round_trips_through_json_in_protocol_form() {
        let json = serde_json::to_string(&MetricSource::ReceiverReported).unwrap();
        assert_eq!(json, "\"receiver-reported\"");
    }

    #[test]
    fn an_unknown_measurement_is_null_not_zero() {
        let unknown = Measurement::unknown("ms", MetricSource::ReceiverReported);
        assert_eq!(unknown.value, None);
        let json = serde_json::to_string(&unknown).unwrap();
        assert!(json.contains("\"value\":null"), "got {json}");
        assert!(!json.contains("\"value\":0"));
    }

    #[test]
    fn notes_are_omitted_when_absent() {
        let json = serde_json::to_string(&Measurement::measured(Some(1.0), "ms")).unwrap();
        assert!(!json.contains("note"), "got {json}");
        let annotated =
            Measurement::measured(Some(1.0), "ms").with_note("driver-granted capture period");
        assert!(serde_json::to_string(&annotated).unwrap().contains("note"));
    }

    #[test]
    fn rate_counter_needs_two_samples() {
        let mut counter = RateCounter::new();
        assert_eq!(counter.sample(0), None, "one sample is not a rate");
        assert!(counter.last().is_none());
    }

    #[test]
    fn rate_counter_measures_a_steady_rate() {
        let mut counter = RateCounter::new();
        counter.sample(0);
        std::thread::sleep(core::time::Duration::from_millis(100));
        let rate = counter.sample(100).expect("second sample yields a rate");
        // 100 counts in ~0.1 s is ~1000/s; allow wide slack for scheduling.
        assert!(rate > 400.0 && rate < 3_000.0, "got {rate}");
    }

    #[test]
    fn a_reset_counter_does_not_report_a_negative_rate() {
        let mut counter = RateCounter::new();
        counter.sample(1_000);
        std::thread::sleep(core::time::Duration::from_millis(10));
        // The stream restarted, so the total went backwards.
        assert_eq!(
            counter.sample(5),
            None,
            "a counter reset must not produce a negative rate"
        );
    }

    #[test]
    fn reset_forgets_history() {
        let mut counter = RateCounter::new();
        counter.sample(0);
        std::thread::sleep(core::time::Duration::from_millis(10));
        counter.sample(10);
        assert!(counter.last().is_some());
        counter.reset();
        assert!(counter.last().is_none());
        assert_eq!(counter.sample(0), None);
    }
}
