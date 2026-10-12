//! A reproducible model of what a venue network does to audio packets.
//!
//! Every decision comes from one seeded generator, so a run is reproducible
//! from its `--seed` alone. That is the whole point: a receiver bug that only
//! appears under a particular loss pattern is close to undebuggable if the
//! pattern cannot be replayed, and "it happened once at a rehearsal" is not a
//! test case.
//!
//! ## What this models, and why each one is here
//!
//! | Impairment | Why it matters |
//! | --- | --- |
//! | Independent loss | The baseline. Exercises concealment. |
//! | Burst loss | 802.11 retries fail in runs, not at random. A buffer sized for independent loss underruns on bursts. |
//! | Reorder | Rare on a switch, real on Wi-Fi with aggregation. A receiver must not count a late packet as lost *and* then play it. |
//! | Duplicates | Retransmission at the AP. A receiver that plays both gets a click. |
//! | Outage | Somebody walks between the phone and the AP. Recovery is the test, not survival. |
//!
//! Jitter — variation in arrival time — is deliberately *not* here. It belongs
//! to pacing (`transmit.rs`), because it is a property of when a packet is sent
//! rather than of whether it survives.

use rand::{RngExt, SeedableRng};
use rand::rngs::StdRng;

/// What to do with one packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Send it now.
    Send,
    /// Drop it. The receiver sees a sequence gap.
    Drop,
    /// Send it twice.
    Duplicate,
    /// Hold it and send it after the next packet.
    Delay,
}

/// Impairment settings. All rates are percentages in `0.0..=100.0`.
#[derive(Debug, Clone, Copy)]
pub struct ImpairmentConfig {
    pub loss_percent: f64,
    /// Mean length of a loss burst, in packets. 1 means independent loss.
    pub burst_length: u32,
    pub reorder_percent: f64,
    pub duplicate_percent: f64,
    /// Length of a total outage, in milliseconds. 0 disables outages.
    pub outage_ms: u64,
    /// How often an outage starts, in milliseconds.
    pub outage_period_ms: u64,
}

impl Default for ImpairmentConfig {
    /// A clean network. The transmitter must be usable as a plain sender.
    fn default() -> Self {
        Self {
            loss_percent: 0.0,
            burst_length: 1,
            reorder_percent: 0.0,
            duplicate_percent: 0.0,
            outage_ms: 0,
            outage_period_ms: 0,
        }
    }
}

impl ImpairmentConfig {
    /// True when nothing is impaired, so the send path can skip the model
    /// entirely and the clean case stays exactly that.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.loss_percent <= 0.0
            && self.reorder_percent <= 0.0
            && self.duplicate_percent <= 0.0
            && self.outage_ms == 0
    }

    /// Rejects settings that would make a run meaningless.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("loss", self.loss_percent),
            ("reorder", self.reorder_percent),
            ("duplicate", self.duplicate_percent),
        ] {
            if !(0.0..=100.0).contains(&value) {
                return Err(format!("{name} must be a percentage between 0 and 100"));
            }
        }
        if self.burst_length == 0 {
            return Err("burst length must be at least 1 packet".to_owned());
        }
        if self.outage_ms > 0 && self.outage_period_ms <= self.outage_ms {
            return Err(
                "the outage period must be longer than the outage itself, or the link is \
                 never up"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// Applies an [`ImpairmentConfig`] deterministically.
pub struct Impairment {
    config: ImpairmentConfig,
    rng: StdRng,
    /// Packets still to drop in the current burst.
    burst_remaining: u32,
    /// Milliseconds into the outage cycle, advanced by the caller's pacing.
    elapsed_ms: u64,
    dropped: u64,
    duplicated: u64,
    delayed: u64,
    offered: u64,
}

impl Impairment {
    #[must_use]
    pub fn new(config: ImpairmentConfig, seed: u64) -> Self {
        Self {
            config,
            rng: StdRng::seed_from_u64(seed),
            burst_remaining: 0,
            elapsed_ms: 0,
            dropped: 0,
            duplicated: 0,
            delayed: 0,
            offered: 0,
        }
    }

    /// Advances the outage clock. Called once per packet with its duration.
    pub fn advance(&mut self, millis: u64) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(millis);
    }

    /// Whether the link is currently down.
    ///
    /// An outage is positional rather than random: it starts at a known offset
    /// in every run, so "does the receiver recover from a two-second blackout"
    /// is a test with a fixed answer instead of a probability.
    #[must_use]
    pub fn in_outage(&self) -> bool {
        if self.config.outage_ms == 0 || self.config.outage_period_ms == 0 {
            return false;
        }
        self.elapsed_ms % self.config.outage_period_ms < self.config.outage_ms
    }

    /// Decides what happens to the next packet.
    pub fn next_verdict(&mut self) -> Verdict {
        self.offered += 1;

        if self.in_outage() {
            self.dropped += 1;
            return Verdict::Drop;
        }

        // Finish a burst before starting anything else, so a burst is a run of
        // consecutive losses rather than a run of independent coin flips.
        if self.burst_remaining > 0 {
            self.burst_remaining -= 1;
            self.dropped += 1;
            return Verdict::Drop;
        }

        if self.roll(self.config.loss_percent) {
            // The first packet of the burst is this one.
            self.burst_remaining = self.config.burst_length.saturating_sub(1);
            self.dropped += 1;
            return Verdict::Drop;
        }

        if self.roll(self.config.duplicate_percent) {
            self.duplicated += 1;
            return Verdict::Duplicate;
        }

        if self.roll(self.config.reorder_percent) {
            self.delayed += 1;
            return Verdict::Delay;
        }

        Verdict::Send
    }

    fn roll(&mut self, percent: f64) -> bool {
        percent > 0.0 && self.rng.random::<f64>() * 100.0 < percent
    }

    #[must_use]
    pub const fn stats(&self) -> ImpairmentStats {
        ImpairmentStats {
            offered: self.offered,
            dropped: self.dropped,
            duplicated: self.duplicated,
            delayed: self.delayed,
        }
    }
}

/// What the impairment model actually did, as opposed to what was asked for.
///
/// Reported at the end of a run because the realised loss of a short run
/// differs from the configured rate, and a receiver's figures must be compared
/// against what was really sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImpairmentStats {
    pub offered: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub delayed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ImpairmentConfig {
        ImpairmentConfig::default()
    }

    #[test]
    fn a_clean_network_sends_everything() {
        let mut impairment = Impairment::new(config(), 1);
        for _ in 0..1_000 {
            assert_eq!(impairment.next_verdict(), Verdict::Send);
        }
        assert_eq!(impairment.stats().dropped, 0);
    }

    /// The property the whole crate rests on: a failure found at 3% loss has
    /// to be replayable from its seed, or it cannot be debugged.
    #[test]
    fn the_same_seed_produces_the_same_run() {
        let settings = ImpairmentConfig {
            loss_percent: 7.0,
            reorder_percent: 3.0,
            duplicate_percent: 2.0,
            ..config()
        };
        let verdicts = |seed| {
            let mut impairment = Impairment::new(settings, seed);
            (0..2_000).map(|_| impairment.next_verdict()).collect::<Vec<_>>()
        };
        assert_eq!(verdicts(42), verdicts(42));
        assert_ne!(verdicts(42), verdicts(43), "a different seed must differ");
    }

    #[test]
    fn loss_lands_near_the_configured_rate() {
        let settings = ImpairmentConfig { loss_percent: 10.0, ..config() };
        let mut impairment = Impairment::new(settings, 7);
        for _ in 0..20_000 {
            impairment.next_verdict();
        }
        let stats = impairment.stats();
        let realised = stats.dropped as f64 * 100.0 / stats.offered as f64;
        assert!((realised - 10.0).abs() < 1.0, "realised {realised}%");
    }

    /// A burst is consecutive. This is the distinction that matters: a jitter
    /// buffer sized for 10% independent loss underruns on 10% arriving as
    /// bursts of eight, and only the burst case reproduces the Wi-Fi failure.
    #[test]
    fn a_burst_drops_consecutive_packets() {
        let settings = ImpairmentConfig {
            loss_percent: 5.0,
            burst_length: 8,
            ..config()
        };
        let mut impairment = Impairment::new(settings, 11);
        let verdicts: Vec<_> = (0..4_000).map(|_| impairment.next_verdict()).collect();

        let mut longest = 0;
        let mut current = 0;
        for verdict in &verdicts {
            if *verdict == Verdict::Drop {
                current += 1;
                longest = longest.max(current);
            } else {
                current = 0;
            }
        }
        assert!(longest >= 8, "longest run of losses was {longest}");

        // And the realised loss is roughly rate x burst length, which is the
        // arithmetic a caller needs in order to ask for a target loss.
        let stats = impairment.stats();
        let realised = stats.dropped as f64 * 100.0 / stats.offered as f64;
        assert!(realised > 20.0, "realised {realised}% for 5% x 8");
    }

    #[test]
    fn an_outage_is_a_known_window_not_a_random_one() {
        let settings = ImpairmentConfig {
            outage_ms: 500,
            outage_period_ms: 2_000,
            ..config()
        };
        let mut impairment = Impairment::new(settings, 1);

        // 0-500 ms down, 500-2000 ms up, repeating.
        assert!(impairment.in_outage(), "an outage starts the cycle");
        impairment.advance(499);
        assert!(impairment.in_outage());
        impairment.advance(1);
        assert!(!impairment.in_outage(), "500 ms in, the link is back");
        impairment.advance(1_499);
        assert!(!impairment.in_outage());
        impairment.advance(1);
        assert!(impairment.in_outage(), "the next cycle starts at 2000 ms");
    }

    #[test]
    fn everything_is_dropped_during_an_outage() {
        let settings = ImpairmentConfig {
            outage_ms: 100,
            outage_period_ms: 1_000,
            ..config()
        };
        let mut impairment = Impairment::new(settings, 1);
        for _ in 0..20 {
            assert_eq!(impairment.next_verdict(), Verdict::Drop);
            impairment.advance(5);
        }
        assert!(!impairment.in_outage());
        assert_eq!(impairment.next_verdict(), Verdict::Send);
    }

    #[test]
    fn impossible_settings_are_refused_with_the_reason() {
        assert!(ImpairmentConfig { loss_percent: 101.0, ..config() }.validate().is_err());
        assert!(ImpairmentConfig { loss_percent: -1.0, ..config() }.validate().is_err());
        assert!(ImpairmentConfig { burst_length: 0, ..config() }.validate().is_err());

        // An outage as long as its period means the link is never up, which is
        // not a test of anything.
        let never_up = ImpairmentConfig {
            outage_ms: 1_000,
            outage_period_ms: 1_000,
            ..config()
        };
        assert!(never_up.validate().unwrap_err().contains("never up"));

        assert!(config().validate().is_ok());
    }

    #[test]
    fn a_clean_config_is_recognised_as_clean() {
        assert!(config().is_clean());
        assert!(!ImpairmentConfig { loss_percent: 0.1, ..config() }.is_clean());
        assert!(
            ImpairmentConfig { burst_length: 8, ..config() }.is_clean(),
            "a burst length with no loss rate drops nothing"
        );
    }
}
