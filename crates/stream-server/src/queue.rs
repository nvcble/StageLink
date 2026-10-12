//! Per-receiver bounded send queue.
//!
//! This is the mechanism that keeps one bad network path from harming anyone
//! else. Each receiver gets its own fixed-capacity queue of ready-to-send
//! datagrams. When a receiver's queue fills — because its Wi-Fi link is
//! congested, or the device has gone to sleep without closing the session — the
//! queue **drops the oldest packet** and carries on.
//!
//! Why drop the oldest rather than the newest: this is live monitoring. A
//! packet that has been waiting is already too late to be useful; the freshest
//! audio is the only audio worth sending. Dropping the newest would hand the
//! receiver an ever-growing delay, which in an IEM is worse than a dropout.
//!
//! What the queue never does: block, allocate after construction, or apply
//! backpressure to the packetiser. A full queue is a local, contained problem.

use std::collections::VecDeque;

/// Counters for one receiver's queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub enqueued: u64,
    /// Packets discarded because the queue was full.
    pub dropped_oldest: u64,
    pub dequeued: u64,
    /// High-water mark, for the diagnostics page.
    pub depth_max: usize,
}

/// A fixed-capacity queue of encoded datagrams for one receiver.
pub struct ReceiverQueue {
    packets: VecDeque<Vec<u8>>,
    capacity: usize,
    /// Reusable buffers, so a steady-state stream performs no allocation.
    recycled: Vec<Vec<u8>>,
    stats: QueueStats,
}

impl ReceiverQueue {
    /// Creates a queue holding at most `capacity` packets.
    ///
    /// Both the queue and the recycling pool are pre-allocated, so pushing and
    /// popping in the steady state never touches the allocator.
    #[must_use]
    pub fn new(capacity: usize, packet_bytes: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            packets: VecDeque::with_capacity(capacity),
            capacity,
            recycled: (0..capacity)
                .map(|_| Vec::with_capacity(packet_bytes))
                .collect(),
            stats: QueueStats::default(),
        }
    }

    /// Queues a copy of `datagram`, dropping the oldest packet if full.
    ///
    /// Returns true if nothing had to be dropped.
    pub fn push(&mut self, datagram: &[u8]) -> bool {
        let mut dropped = false;
        if self.packets.len() >= self.capacity {
            if let Some(evicted) = self.packets.pop_front() {
                self.recycle(evicted);
            }
            self.stats.dropped_oldest += 1;
            dropped = true;
        }

        let mut buffer = self.recycled.pop().unwrap_or_default();
        buffer.clear();
        buffer.extend_from_slice(datagram);
        self.packets.push_back(buffer);

        self.stats.enqueued += 1;
        self.stats.depth_max = self.stats.depth_max.max(self.packets.len());
        !dropped
    }

    /// Takes the next packet to transmit.
    ///
    /// The caller must return the buffer with [`ReceiverQueue::release`] once
    /// the socket has taken it, so it can be reused.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let packet = self.packets.pop_front();
        if packet.is_some() {
            self.stats.dequeued += 1;
        }
        packet
    }

    /// Returns a buffer obtained from [`ReceiverQueue::pop`] to the pool.
    pub fn release(&mut self, buffer: Vec<u8>) {
        self.recycle(buffer);
    }

    fn recycle(&mut self, mut buffer: Vec<u8>) {
        if self.recycled.len() < self.capacity {
            buffer.clear();
            self.recycled.push(buffer);
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.packets.len() >= self.capacity
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn stats(&self) -> QueueStats {
        self.stats
    }

    /// Queued audio expressed in milliseconds, given the packet frame duration.
    /// This is the receiver's share of desktop-side latency.
    #[must_use]
    pub fn queued_millis(&self, frame_duration_ms: f64) -> f64 {
        self.packets.len() as f64 * frame_duration_ms
    }

    /// Discards everything queued. Used when a stream restarts, so stale audio
    /// is not transmitted ahead of the live signal.
    pub fn clear(&mut self) {
        while let Some(buffer) = self.packets.pop_front() {
            self.recycle(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_come_out_in_order() {
        let mut queue = ReceiverQueue::new(4, 512);
        queue.push(&[1, 1, 1]);
        queue.push(&[2, 2, 2]);

        assert_eq!(queue.pop().unwrap(), vec![1, 1, 1]);
        assert_eq!(queue.pop().unwrap(), vec![2, 2, 2]);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn a_full_queue_drops_the_oldest_and_keeps_the_freshest_audio() {
        let mut queue = ReceiverQueue::new(2, 64);
        assert!(queue.push(&[1]));
        assert!(queue.push(&[2]));
        assert!(queue.is_full());

        // The third push must succeed by evicting packet 1, not by refusing.
        assert!(!queue.push(&[3]), "push should report that it dropped");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.stats().dropped_oldest, 1);

        // What remains is the two most recent packets.
        assert_eq!(queue.pop().unwrap(), vec![2]);
        assert_eq!(queue.pop().unwrap(), vec![3]);
    }

    #[test]
    fn pushing_never_fails_so_the_packetiser_is_never_blocked() {
        let mut queue = ReceiverQueue::new(2, 64);
        // A receiver that has stopped reading entirely.
        for index in 0..1_000_u16 {
            queue.push(&index.to_le_bytes());
        }
        assert_eq!(queue.len(), 2, "the queue stays bounded");
        assert_eq!(queue.stats().dropped_oldest, 998);
        // And the newest packet is still the one at the back.
        let newest = 999_u16.to_le_bytes().to_vec();
        assert_eq!(queue.pop().unwrap(), 998_u16.to_le_bytes().to_vec());
        assert_eq!(queue.pop().unwrap(), newest);
    }

    #[test]
    fn a_zero_capacity_request_still_yields_a_usable_queue() {
        let mut queue = ReceiverQueue::new(0, 64);
        assert_eq!(queue.capacity(), 1);
        assert!(queue.push(&[1]));
        assert_eq!(queue.pop().unwrap(), vec![1]);
    }

    #[test]
    fn buffers_are_recycled_rather_than_reallocated() {
        let mut queue = ReceiverQueue::new(4, 512);
        queue.push(&[7; 480]);
        let buffer = queue.pop().unwrap();
        let capacity_before = buffer.capacity();
        queue.release(buffer);

        queue.push(&[8; 480]);
        let reused = queue.pop().unwrap();
        assert_eq!(
            reused.capacity(),
            capacity_before,
            "the pooled buffer should have been reused"
        );
        assert_eq!(reused, vec![8; 480]);
    }

    #[test]
    fn stats_track_throughput_and_the_high_water_mark() {
        let mut queue = ReceiverQueue::new(8, 64);
        for _ in 0..5 {
            queue.push(&[0]);
        }
        queue.pop();
        queue.pop();

        let stats = queue.stats();
        assert_eq!(stats.enqueued, 5);
        assert_eq!(stats.dequeued, 2);
        assert_eq!(stats.depth_max, 5);
        assert_eq!(stats.dropped_oldest, 0);
    }

    #[test]
    fn queued_latency_is_reported_in_milliseconds() {
        let mut queue = ReceiverQueue::new(8, 64);
        for _ in 0..4 {
            queue.push(&[0]);
        }
        // Four 5 ms packets waiting is 20 ms of queued audio.
        assert!((queue.queued_millis(5.0) - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn clearing_discards_stale_audio_on_a_restart() {
        let mut queue = ReceiverQueue::new(4, 64);
        queue.push(&[1]);
        queue.push(&[2]);
        queue.clear();
        assert!(queue.is_empty());
        assert!(queue.pop().is_none());

        // And the queue is immediately reusable.
        queue.push(&[3]);
        assert_eq!(queue.pop().unwrap(), vec![3]);
    }

    #[test]
    fn one_receivers_congestion_does_not_touch_another() {
        let mut slow = ReceiverQueue::new(2, 64);
        let mut healthy = ReceiverQueue::new(2, 64);

        for index in 0..10_u8 {
            slow.push(&[index]);
            healthy.push(&[index]);
            // The healthy receiver keeps up.
            if let Some(buffer) = healthy.pop() {
                healthy.release(buffer);
            }
        }

        assert_eq!(slow.stats().dropped_oldest, 8, "the slow receiver loses audio");
        assert_eq!(
            healthy.stats().dropped_oldest,
            0,
            "the healthy receiver must be unaffected"
        );
        assert_eq!(healthy.stats().dequeued, 10);
    }
}
