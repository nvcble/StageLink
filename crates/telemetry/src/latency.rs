//! Latency accounting.
//!
//! StageLink's planning target is roughly 15–30 ms from desktop to a wired
//! earphone under favourable conditions. That is a *target*, not a measurement,
//! and this module exists to keep the distinction visible in the product.
//!
//! What the desktop can genuinely know:
//!
//! * the capture period the driver granted,
//! * the packet frame duration we chose,
//! * how long audio actually sat in our send queue.
//!
//! What it cannot know:
//!
//! * Wi-Fi airtime and retransmission,
//! * the receiver's jitter buffer depth,
//! * the mobile OS audio output path (often the largest single term),
//! * the earphone itself.
//!
//! So we publish a desktop subtotal and refuse to present a total. Control
//! channel RTT is reported separately and explicitly *not* as audio latency —
//! conflating the two is the most common way these systems get misrepresented.

use serde::{Deserialize, Serialize};

use crate::metrics::Measurement;

/// Rendered beside every latency figure in the UI.
pub const LATENCY_DISCLAIMER: &str = "Desktop-side contributions only. End-to-end latency to an \
earphone also includes Wi-Fi airtime, the receiver's jitter buffer and the mobile audio output \
path, none of which the desktop can measure. Use the physical loopback procedure in \
docs/latency-measurement.md for a real end-to-end figure.";

/// Desktop-side latency contributions, each tagged with its provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyBreakdown {
    /// Capture period the driver granted. Measured (reported by the driver).
    pub capture_buffer_ms: Measurement,
    /// Packetiser frame duration. Estimated — it is a configured value.
    pub packet_frame_ms: Measurement,
    /// Time audio spent in the send queue. Measured.
    pub send_queue_ms: Measurement,
    /// Sum of the three above. Estimated, because it is derived.
    pub desktop_subtotal_ms: Measurement,
    /// Control-channel round trip. NOT audio latency.
    pub control_channel_rtt_ms: Measurement,
    /// Receiver's own claim about its output path. Receiver-reported.
    pub receiver_reported_output_ms: Measurement,
    pub disclaimer: &'static str,
}

impl LatencyBreakdown {
    /// Builds a breakdown from what is currently known.
    ///
    /// Any `None` input propagates: the subtotal is only produced when all of
    /// its terms are known, because a subtotal missing a term reads as
    /// optimistically low.
    #[must_use]
    pub fn new(
        capture_buffer_ms: Option<f64>,
        packet_frame_ms: Option<f64>,
        send_queue_ms: Option<f64>,
        control_rtt_ms: Option<f64>,
        receiver_output_ms: Option<f64>,
    ) -> Self {
        let subtotal = match (capture_buffer_ms, packet_frame_ms, send_queue_ms) {
            (Some(capture), Some(frame), Some(queue)) => Some(capture + frame + queue),
            _ => None,
        };

        Self {
            capture_buffer_ms: Measurement::measured(capture_buffer_ms, "ms")
                .with_note("Capture period granted by the audio driver."),
            packet_frame_ms: Measurement::estimated(packet_frame_ms, "ms")
                .with_note("Configured packet frame duration."),
            send_queue_ms: Measurement::measured(send_queue_ms, "ms")
                .with_note("Measured time audio spent waiting in the send queue."),
            desktop_subtotal_ms: Measurement::estimated(subtotal, "ms")
                .with_note("Sum of desktop-side contributions. Not end-to-end latency."),
            control_channel_rtt_ms: Measurement::measured(control_rtt_ms, "ms")
                .with_note("Control-channel round trip. This is NOT audio latency."),
            receiver_reported_output_ms: Measurement::receiver_reported(receiver_output_ms, "ms")
                .with_note("Reported by the receiver; unverified by the desktop."),
            disclaimer: LATENCY_DISCLAIMER,
        }
    }

    /// A breakdown with nothing known yet.
    #[must_use]
    pub fn unknown() -> Self {
        Self::new(None, None, None, None, None)
    }
}

/// Capture period in milliseconds from a frame count and sample rate.
#[must_use]
pub fn frames_to_millis(frames: u32, sample_rate: u32) -> Option<f64> {
    if sample_rate == 0 {
        return None;
    }
    Some((f64::from(frames) * 1000.0) / f64::from(sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricSource;

    #[test]
    fn subtotal_sums_the_desktop_terms() {
        let breakdown = LatencyBreakdown::new(Some(10.0), Some(5.0), Some(1.5), None, None);
        assert_eq!(breakdown.desktop_subtotal_ms.value, Some(16.5));
    }

    #[test]
    fn a_missing_term_suppresses_the_subtotal() {
        // Reporting 15 ms when the queue time is unknown would understate it.
        let breakdown = LatencyBreakdown::new(Some(10.0), Some(5.0), None, None, None);
        assert_eq!(breakdown.desktop_subtotal_ms.value, None);
    }

    #[test]
    fn the_subtotal_is_labelled_as_derived_not_measured() {
        let breakdown = LatencyBreakdown::new(Some(10.0), Some(5.0), Some(0.0), None, None);
        assert_eq!(breakdown.desktop_subtotal_ms.source, MetricSource::Estimated);
    }

    #[test]
    fn rtt_is_measured_but_kept_out_of_the_audio_subtotal() {
        let breakdown = LatencyBreakdown::new(Some(10.0), Some(5.0), Some(1.0), Some(40.0), None);
        assert_eq!(breakdown.control_channel_rtt_ms.value, Some(40.0));
        // 40 ms of RTT must not appear in the audio subtotal.
        assert_eq!(breakdown.desktop_subtotal_ms.value, Some(16.0));
        assert!(
            breakdown
                .control_channel_rtt_ms
                .note
                .is_some_and(|note| note.contains("NOT audio latency")),
            "the RTT field must carry its warning"
        );
    }

    #[test]
    fn receiver_output_latency_is_tagged_as_receiver_reported() {
        let breakdown = LatencyBreakdown::new(None, None, None, None, Some(22.0));
        assert_eq!(
            breakdown.receiver_reported_output_ms.source,
            MetricSource::ReceiverReported
        );
    }

    #[test]
    fn the_disclaimer_is_always_present() {
        assert!(LatencyBreakdown::unknown().disclaimer.contains("loopback"));
        assert!(
            LatencyBreakdown::unknown()
                .disclaimer
                .contains("the desktop can measure"),
            "the disclaimer must say which terms are outside the desktop's knowledge"
        );
    }

    #[test]
    fn period_conversion_matches_the_sample_rate() {
        assert_eq!(frames_to_millis(480, 48_000), Some(10.0));
        assert_eq!(frames_to_millis(240, 48_000), Some(5.0));
        assert_eq!(frames_to_millis(480, 0), None, "must not divide by zero");
    }
}
