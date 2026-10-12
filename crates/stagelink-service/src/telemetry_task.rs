//! The telemetry task.
//!
//! Drains the meter accumulator at a fixed cadence and pushes events to the
//! connected client. This is the only thing that turns audio into UI updates,
//! and it is deliberately the *only* thing: the capture thread writes atomics
//! and nothing else, so the renderer cannot influence audio timing no matter
//! how it behaves.
//!
//! Cadences:
//!
//! * Meters at `METER_HZ` (25 Hz). Fast enough to look continuous, slow enough
//!   that the Electron IPC channel carries ~25 small messages a second rather
//!   than one per audio buffer.
//! * Diagnostics only while the renderer has subscribed, at its requested
//!   interval. The Diagnostics page subscribes on mount and unsubscribes on
//!   unmount, so an idle dashboard costs nothing.

use std::sync::Arc;

use audio_core::{LevelSnapshot, PeakHold};
use desktop_ipc::codec::Envelope;
use tokio::time::{Duration, Instant, MissedTickBehavior, interval};
use tracing::debug;

use crate::handler::StageLinkHandler;
use crate::serialize;
use crate::state::now_ms;

/// Meter event rate. Decoupled from the audio buffer rate on purpose.
pub const METER_HZ: u32 = 25;

/// Peak-hold decay, dB per second.
const PEAK_DECAY_DB_PER_SEC: f32 = 20.0;

/// Runs until `shutdown` fires.
pub async fn run(handler: Arc<StageLinkHandler>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let state = handler.state();
    let levels = state.audio.levels();

    let meter_period = Duration::from_micros(1_000_000 / u64::from(METER_HZ));
    let mut meter_ticker = interval(meter_period);
    // If the runtime is briefly starved, skip the missed ticks rather than
    // firing a burst of stale meter frames at the UI.
    meter_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut diagnostics_ticker = interval(Duration::from_millis(500));
    diagnostics_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut diagnostics_interval_ms = 500_u32;

    let mut hold = PeakHold::new(PEAK_DECAY_DB_PER_SEC);
    let mut last_meter_at = Instant::now();
    let mut last_audio_state: Option<String> = None;

    debug!(meter_hz = METER_HZ, "telemetry task started");

    loop {
        tokio::select! {
            biased;

            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    debug!("telemetry task stopping");
                    return;
                }
            }

            _ = meter_ticker.tick() => {
                let elapsed = last_meter_at.elapsed().as_secs_f32();
                last_meter_at = Instant::now();

                // No client attached: still drain, so the accumulator does not
                // report a huge stale window to the next client that connects.
                let window = levels.drain();
                let Some(sink) = handler.sink() else { continue };

                let snapshot = match window {
                    Some(window) => LevelSnapshot::from_window(window, &mut hold, elapsed),
                    // The capture path delivered nothing this window. Report
                    // silence and let the hold decay rather than repeating the
                    // last real reading, which would look like live audio.
                    None => LevelSnapshot::stalled(&mut hold, elapsed, levels.total_clips()),
                };

                // Only meter while capture is actually open, so an idle
                // dashboard shows "no input" instead of a flat silent meter
                // that could be mistaken for a working but quiet feed.
                if state.audio.is_running() || snapshot.window_frames > 0 {
                    let _ = sink.send(Envelope::event(
                        "audio.levels",
                        serialize::audio_levels(&snapshot, now_ms()),
                        now_ms(),
                    ));
                }

                // Push a status event whenever the capture state changes, so
                // the UI learns about a disconnected interface without polling.
                let status = state.audio.status();
                let current = status.state.as_str().to_owned();
                if last_audio_state.as_deref() != Some(current.as_str()) {
                    last_audio_state = Some(current);
                    let _ = sink.send(Envelope::event(
                        "audio.status",
                        serialize::audio_status(&status),
                        now_ms(),
                    ));
                }
            }

            _ = diagnostics_ticker.tick() => {
                let subscription = *state.diagnostics_streaming.lock();
                if subscription.interval_ms != diagnostics_interval_ms {
                    diagnostics_interval_ms = subscription.interval_ms;
                    diagnostics_ticker =
                        interval(Duration::from_millis(u64::from(diagnostics_interval_ms)));
                    diagnostics_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                }
                if !subscription.enabled {
                    continue;
                }
                let Some(sink) = handler.sink() else { continue };
                // A dropped event is acceptable here: the UI can always re-read the

                // same state with a request, and a full queue means the client is not

                // keeping up, which must not stall telemetry.

                let _ = sink.send(Envelope::event(
                    "diagnostics.snapshot",
                    serialize::diagnostics_snapshot(&state),
                    now_ms(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use persistence::{Database, SecretStore};

    use crate::state::ServiceState;

    fn handler() -> Arc<StageLinkHandler> {
        let database = Database::open_in_memory().unwrap();
        let directory =
            std::env::temp_dir().join(format!("stagelink-telemetry-{}", uuid::Uuid::new_v4()));
        let secrets = SecretStore::new(&directory).unwrap();
        Arc::new(StageLinkHandler::new(ServiceState::new(database, secrets, false)))
    }

    #[test]
    fn the_meter_cadence_is_well_under_the_audio_buffer_rate() {
        // At 48 kHz with a 480-frame period the driver fires ~100 times a
        // second. Metering at 25 Hz means the IPC channel sees a quarter of
        // that, and never one message per audio buffer.
        assert_eq!(METER_HZ, 25);
        assert!(METER_HZ < 100);
    }

    #[tokio::test]
    async fn the_task_stops_promptly_when_asked() {
        let handler = handler();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(Arc::clone(&handler), shutdown_rx));

        tokio::time::sleep(Duration::from_millis(120)).await;
        shutdown_tx.send(true).unwrap();

        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the telemetry task must stop within two seconds")
            .expect("and must not panic");
    }

    #[tokio::test]
    async fn no_meter_events_are_emitted_while_no_client_is_attached() {
        let handler = handler();
        assert!(handler.sink().is_none());

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(Arc::clone(&handler), shutdown_rx));
        tokio::time::sleep(Duration::from_millis(150)).await;
        shutdown_tx.send(true).unwrap();
        let _ = task.await;
        // Nothing to assert beyond "it did not panic or block": the point is
        // that a detached service keeps running happily.
    }

    #[tokio::test]
    async fn an_idle_service_does_not_push_fabricated_meter_levels() {
        use desktop_ipc::handler::{ControlHandler, EventSink};

        let handler = handler();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        handler.on_client_connected(EventSink::new(sender));

        // Drain the snapshot events the handshake pushes.
        let mut saw_levels = false;
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(Arc::clone(&handler), shutdown_rx));
        tokio::time::sleep(Duration::from_millis(200)).await;
        shutdown_tx.send(true).unwrap();
        let _ = task.await;

        while let Ok(envelope) = receiver.try_recv() {
            if let desktop_ipc::codec::EnvelopeKind::Event { event, .. } = envelope.kind {
                if event == "audio.levels" {
                    saw_levels = true;
                }
            }
        }
        assert!(
            !saw_levels,
            "with no capture device open there is no level to report"
        );
    }

    #[tokio::test]
    async fn connecting_a_client_immediately_receives_a_state_snapshot() {
        use desktop_ipc::handler::{ControlHandler, EventSink};

        let handler = handler();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        handler.on_client_connected(EventSink::new(sender));

        let mut events = Vec::new();
        while let Ok(envelope) = receiver.try_recv() {
            if let desktop_ipc::codec::EnvelopeKind::Event { event, .. } = envelope.kind {
                events.push(event);
            }
        }
        assert!(
            events.contains(&"audio.status".to_owned()),
            "a reconnecting renderer must not have to poll: {events:?}"
        );
        assert!(events.contains(&"service.state".to_owned()));
    }

    #[tokio::test]
    async fn diagnostics_are_pushed_only_while_subscribed() {
        use desktop_ipc::handler::{ControlHandler, EventSink};

        let handler = handler();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
        handler.on_client_connected(EventSink::new(sender));
        while receiver.try_recv().is_ok() {}

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run(Arc::clone(&handler), shutdown_rx));

        // Unsubscribed: nothing should arrive.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut snapshots = 0;
        while let Ok(envelope) = receiver.try_recv() {
            if let desktop_ipc::codec::EnvelopeKind::Event { event, .. } = envelope.kind {
                if event == "diagnostics.snapshot" {
                    snapshots += 1;
                }
            }
        }
        assert_eq!(snapshots, 0, "an unsubscribed client must get no snapshots");

        // Subscribe and expect pushes.
        handler
            .handle(
                "diagnostics.subscribe",
                serde_json::json!({ "enabled": true, "intervalMs": 200 }),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        shutdown_tx.send(true).unwrap();
        let _ = task.await;

        let mut subscribed_snapshots = 0;
        while let Ok(envelope) = receiver.try_recv() {
            if let desktop_ipc::codec::EnvelopeKind::Event { event, .. } = envelope.kind {
                if event == "diagnostics.snapshot" {
                    subscribed_snapshots += 1;
                }
            }
        }
        assert!(
            subscribed_snapshots > 0,
            "a subscribed client must receive diagnostics"
        );
    }
}
