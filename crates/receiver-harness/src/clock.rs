//! A virtual clock, and the event loop that runs on it.
//!
//! ## Why not just run it in real time
//!
//! The scenarios that matter take as long as a church service. Clock drift
//! accumulates 216 ms over 90 minutes at 40 ppm; a buffer-depth ratchet from
//! 802.11 retry bursts shows up over tens of minutes; the drift controller
//! itself settles over 10–20 seconds and has to be watched for far longer than
//! that to know it is stable rather than slow.
//!
//! None of that is testable at 1× speed. On a virtual clock a 90-minute
//! scenario runs in a couple of seconds, which is the difference between a
//! property being verified on every commit and being verified never.
//!
//! ## Why the two clocks are separate
//!
//! The sender's packet cadence and the receiver's DAC cadence are driven by
//! two independent rates with their own ppm errors, exactly as two crystals
//! are. That is the entire physical situation the drift controller exists to
//! handle, and simulating it with one clock would make the test pass for the
//! wrong reason.

use std::collections::BinaryHeap;
use std::cmp::Reverse;

/// A scheduled packet arrival.
///
/// Ordered by delivery time so the heap hands them back in arrival order,
/// which is what makes reordering and jitter come out as a receiver would
/// really see them.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Delivery {
    at_us: u64,
    /// Tie-break, so two packets delivered in the same microsecond keep the
    /// order they were sent in rather than an arbitrary one.
    seq: u64,
    datagram: Vec<u8>,
}

/// What happens next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The sender should emit a packet.
    SendPacket,
    /// A packet arrives at the receiver.
    Deliver,
    /// The DAC asks for a block of audio.
    RenderBlock,
    /// The run is over.
    Done,
}

/// Drives a sender and a receiver on independent clocks.
pub struct VirtualClock {
    now_us: u64,
    end_us: u64,
    next_send_us: u64,
    next_render_us: u64,
    /// Microseconds between packets, as the *sender's* clock measures them.
    send_interval_us: f64,
    /// Microseconds between render calls, as the *receiver's* clock measures.
    render_interval_us: f64,
    /// Fractional accumulators, so a cadence of 4999.8 µs does not round into
    /// a drift of its own and swamp the ppm being simulated.
    send_fraction: f64,
    render_fraction: f64,
    pending: BinaryHeap<Reverse<Delivery>>,
    sent: u64,
    sending_finished: bool,
}

impl VirtualClock {
    /// `sender_ppm` and `dac_ppm` are each clock's error against nominal.
    #[must_use]
    pub fn new(
        packet_interval_us: f64,
        block_interval_us: f64,
        sender_ppm: f64,
        dac_ppm: f64,
        duration_secs: f64,
    ) -> Self {
        // A clock running fast produces its events sooner.
        let send_interval_us = packet_interval_us / (1.0 + sender_ppm / 1e6);
        let render_interval_us = block_interval_us / (1.0 + dac_ppm / 1e6);

        Self {
            now_us: 0,
            end_us: (duration_secs * 1e6) as u64,
            next_send_us: 0,
            next_render_us: 0,
            send_interval_us,
            render_interval_us,
            send_fraction: 0.0,
            render_fraction: 0.0,
            pending: BinaryHeap::new(),
            sent: 0,
            sending_finished: false,
        }
    }

    #[must_use]
    pub const fn now_us(&self) -> u64 {
        self.now_us
    }

    /// Stops producing packets. Deliveries already scheduled still arrive.
    pub fn finish_sending(&mut self) {
        self.sending_finished = true;
    }

    /// Schedules a datagram to arrive after `delay_us`.
    pub fn deliver_after(&mut self, datagram: Vec<u8>, delay_us: u64) {
        self.sent += 1;
        self.pending.push(Reverse(Delivery {
            at_us: self.now_us + delay_us,
            seq: self.sent,
            datagram,
        }));
    }

    /// Advances to the next event and says what it is.
    pub fn next_event(&mut self) -> Event {
        let send_at = if self.sending_finished {
            u64::MAX
        } else {
            self.next_send_us
        };
        let deliver_at = self.pending.peek().map_or(u64::MAX, |next| next.0.at_us);

        let next = send_at.min(deliver_at).min(self.next_render_us);
        if next == u64::MAX || next > self.end_us {
            // Deliveries and renders still pending at the end are dropped:
            // the run is over, and rendering past it would report audio the
            // scenario did not ask for.
            return Event::Done;
        }

        self.now_us = next;

        // Delivery first when times tie: a packet that arrives in the same
        // microsecond as a render deadline was there before the DAC asked, and
        // pretending otherwise would invent a dropout.
        if deliver_at == next {
            Event::Deliver
        } else if send_at == next {
            self.next_send_us = self.advance(self.next_send_us, self.send_interval_us, true);
            Event::SendPacket
        } else {
            self.next_render_us =
                self.advance(self.next_render_us, self.render_interval_us, false);
            Event::RenderBlock
        }
    }

    /// Takes the datagram for a `Deliver` event.
    pub fn take_delivery(&mut self) -> Option<Vec<u8>> {
        self.pending.pop().map(|next| next.0.datagram)
    }

    /// Advances a deadline, carrying the fractional microsecond.
    fn advance(&mut self, from: u64, interval_us: f64, sending: bool) -> u64 {
        let fraction = if sending {
            &mut self.send_fraction
        } else {
            &mut self.render_fraction
        };
        *fraction += interval_us;
        let whole = fraction.floor();
        *fraction -= whole;
        from + whole as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(sender_ppm: f64, dac_ppm: f64, seconds: f64) -> VirtualClock {
        // 5 ms packets, 5 ms render blocks.
        VirtualClock::new(5_000.0, 5_000.0, sender_ppm, dac_ppm, seconds)
    }

    /// Counts events over a run.
    fn tally(clock: &mut VirtualClock) -> (u64, u64) {
        let (mut sends, mut renders) = (0, 0);
        loop {
            match clock.next_event() {
                Event::SendPacket => {
                    sends += 1;
                    clock.deliver_after(vec![0], 0);
                }
                Event::Deliver => {
                    clock.take_delivery();
                }
                Event::RenderBlock => renders += 1,
                Event::Done => break,
            }
        }
        (sends, renders)
    }

    #[test]
    fn matched_clocks_produce_matched_event_counts() {
        let mut clock = clock(0.0, 0.0, 10.0);
        let (sends, renders) = tally(&mut clock);
        // 10 seconds at 200 per second.
        assert_eq!(sends, 2_001);
        assert_eq!(renders, 2_001);
    }

    /// The whole point of two clocks: over a long enough run they produce
    /// measurably different event counts, which is what drains a buffer.
    #[test]
    fn a_fast_sender_produces_more_packets_than_the_dac_consumes() {
        // 1000 ppm over 100 s is 0.1 s, which is 20 packets.
        let mut clock = clock(1_000.0, 0.0, 100.0);
        let (sends, renders) = tally(&mut clock);
        assert!(
            sends > renders + 15,
            "sender produced {sends}, DAC consumed {renders}"
        );
    }

    #[test]
    fn a_fast_dac_consumes_more_than_the_sender_produces() {
        let mut clock = clock(0.0, 1_000.0, 100.0);
        let (sends, renders) = tally(&mut clock);
        assert!(renders > sends + 15);
    }

    /// The fractional accumulator matters: rounding 4999.8 µs down to 4999
    /// would add 40 ppm of its own, which is the same size as the drift being
    /// simulated.
    #[test]
    fn the_simulated_drift_is_the_drift_asked_for() {
        for ppm in [40.0, -40.0, 200.0] {
            let mut clock = clock(ppm, 0.0, 600.0);
            let (sends, renders) = tally(&mut clock);
            let measured = (sends as f64 / renders as f64 - 1.0) * 1e6;
            assert!(
                (measured - ppm).abs() < 10.0,
                "asked for {ppm} ppm, simulated {measured:.1} ppm"
            );
        }
    }

    #[test]
    fn deliveries_arrive_in_time_order_however_they_were_scheduled() {
        let mut clock = clock(0.0, 0.0, 1.0);
        clock.finish_sending();
        clock.deliver_after(vec![3], 30_000);
        clock.deliver_after(vec![1], 10_000);
        clock.deliver_after(vec![2], 20_000);

        let mut order = Vec::new();
        loop {
            match clock.next_event() {
                Event::Deliver => order.push(clock.take_delivery().unwrap()[0]),
                Event::Done => break,
                _ => {}
            }
        }
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn two_deliveries_in_the_same_microsecond_keep_their_send_order() {
        let mut clock = clock(0.0, 0.0, 1.0);
        clock.finish_sending();
        clock.deliver_after(vec![1], 5_000);
        clock.deliver_after(vec![2], 5_000);

        let mut order = Vec::new();
        loop {
            match clock.next_event() {
                Event::Deliver => order.push(clock.take_delivery().unwrap()[0]),
                Event::Done => break,
                _ => {}
            }
        }
        assert_eq!(order, vec![1, 2], "a tie must not reorder packets by itself");
    }

    #[test]
    fn a_run_ends_at_its_deadline_rather_than_when_the_queue_empties() {
        let mut clock = clock(0.0, 0.0, 0.1);
        let (sends, _) = tally(&mut clock);
        assert_eq!(sends, 21, "100 ms at 5 ms per packet");
    }
}
