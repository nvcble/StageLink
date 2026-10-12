//! Control-method dispatch.
//!
//! One `match` on the method name, with each arm validating its own parameters.
//! There is deliberately no dynamic command construction and no catch-all that
//! forwards an unknown name anywhere: a method that is not listed here does not
//! exist, and the renderer is told so.
//!
//! Methods that belong to a later phase return `NOT_IMPLEMENTED` with the phase
//! named, rather than a plausible-looking empty success. A UI that cannot tell
//! "no receivers connected" from "receiver support is not built yet" would be
//! actively misleading during a service.

use std::sync::Arc;

use desktop_ipc::codec::ServiceErrorPayload;
use desktop_ipc::handler::{ControlHandler, EventSink};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::audio_engine::AudioEngine;
use crate::serialize;
use crate::state::{DiagnosticsSubscription, ServiceState, now_ms};

/// Control protocol version this service speaks.
pub const CONTROL_PROTOCOL_VERSION: &str = "1.0.0";

/// Service version, from Cargo.
pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every control method this service dispatches.
///
/// Published rather than implicit so two things can be checked mechanically:
/// the unit test below asserts each name is actually handled, and the protocol
/// cross-check tests assert the list matches the TypeScript registry. A method
/// added to the `match` but not here, or vice versa, fails a test.
pub const DISPATCHED_METHODS: &[&str] = &[
    "audio.listDevices",
    "audio.selectDevice",
    "audio.releaseDevice",
    "audio.getStatus",
    "audio.test",
    "stream.start",
    "stream.stop",
    "stream.getStatus",
    "receivers.list",
    "receivers.rename",
    "receivers.disconnect",
    "receivers.revoke",
    "receivers.createJoinCode",
    "receiver.heartbeat",
    "receiver.telemetry",
    "network.getStatus",
    "network.selectAdapter",
    "network.checkFirewall",
    "session.getCurrent",
    "session.history",
    "session.events",
    "diagnostics.getSnapshot",
    "diagnostics.subscribe",
    "settings.get",
    "settings.update",
    "service.getInfo",
    "service.ping",
    "service.shutdown",
];

pub struct StageLinkHandler {
    state: Arc<ServiceState>,
    /// Set when a client is attached, so the telemetry task can push to it.
    event_sink: parking_lot::Mutex<Option<EventSink>>,
}

impl StageLinkHandler {
    #[must_use]
    pub fn new(state: Arc<ServiceState>) -> Self {
        Self { state, event_sink: parking_lot::Mutex::new(None) }
    }

    /// The current event sink, if a client is attached.
    #[must_use]
    pub fn sink(&self) -> Option<EventSink> {
        self.event_sink.lock().clone()
    }

    #[must_use]
    pub fn state(&self) -> Arc<ServiceState> {
        Arc::clone(&self.state)
    }

    /// Builds the "this is a later phase" error.
    fn not_implemented(method: &str, phase: &str) -> ServiceErrorPayload {
        ServiceErrorPayload::new(
            "NOT_IMPLEMENTED",
            format!("{method} is not available in this build."),
        )
        .with_detail(format!("Scheduled for {phase}."))
    }

    fn invalid(message: impl Into<String>, field: &str) -> ServiceErrorPayload {
        ServiceErrorPayload::new("INVALID_REQUEST", message).with_field(field)
    }

    // -- audio -------------------------------------------------------------

    async fn audio_list_devices(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let probe = params
            .get("probeFormats")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Enumeration activates COM objects and can take a second; keep it off
        // the async runtime so telemetry keeps flowing while it runs.
        let devices = tokio::task::spawn_blocking(move || AudioEngine::list_devices(probe))
            .await
            .map_err(|error| {
                ServiceErrorPayload::new("INTERNAL", format!("enumeration task failed: {error}"))
            })?
            .map_err(serialize::capture_error)?;

        Ok(json!({
            "devices": devices.iter().map(serialize::device).collect::<Vec<_>>(),
            "availableApis": audio_capture::AudioApi::available()
                .iter()
                .map(|api| api.as_str())
                .collect::<Vec<_>>(),
        }))
    }

    async fn audio_select_device(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let device_id = params
            .get("deviceId")
            .and_then(Value::as_str)
            .ok_or_else(|| Self::invalid("deviceId is required", "params.deviceId"))?
            .to_owned();

        let channel_index = params
            .get("channelIndex")
            .and_then(Value::as_u64)
            .ok_or_else(|| Self::invalid("channelIndex is required", "params.channelIndex"))?;
        let channel_index = u16::try_from(channel_index).map_err(|_| {
            Self::invalid("channelIndex is out of range", "params.channelIndex")
        })?;

        let settings = self.state.settings_snapshot();
        let sample_rate = params
            .get("sampleRate")
            .and_then(Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok())
            .unwrap_or(settings.audio.sample_rate);

        let share_mode = match params.get("shareMode").and_then(Value::as_str) {
            Some("exclusive") => audio_core::ShareMode::Exclusive,
            Some("shared") | None => audio_core::ShareMode::Shared,
            Some(other) => {
                return Err(Self::invalid(
                    format!("{other} is not a valid share mode"),
                    "params.shareMode",
                ));
            }
        };

        let state = Arc::clone(&self.state);
        let device_for_task = device_id.clone();
        let status = tokio::task::spawn_blocking(move || {
            state.audio.open(
                &device_for_task,
                channel_index,
                sample_rate,
                share_mode,
                now_ms(),
            )
        })
        .await
        .map_err(|error| {
            ServiceErrorPayload::new("INTERNAL", format!("open task failed: {error}"))
        })?
        .map_err(serialize::capture_error)?;

        // Remember the choice so the next launch comes up on the same input.
        let mut updated = self.state.settings_snapshot();
        updated.audio.device_id = Some(device_id);
        updated.audio.device_name = status.device.as_ref().map(|device| device.name.clone());
        updated.audio.channel_index = channel_index;
        updated.audio.sample_rate = status
            .negotiated
            .map_or(sample_rate, |negotiated| negotiated.format.sample_rate);
        updated.audio.share_mode = match share_mode {
            audio_core::ShareMode::Exclusive => "exclusive".to_owned(),
            audio_core::ShareMode::Shared => "shared".to_owned(),
        };

        if let Err(error) = updated.validate() {
            warn!(field = %error.field, "refusing to persist invalid audio settings");
        } else {
            *self.state.settings.lock() = updated.clone();
            if let Err(error) = self.state.persist_settings(&updated) {
                // The device is open and working; failing the request because a
                // preference could not be written would be the wrong call.
                warn!(%error, "could not persist the audio device selection");
            }
        }

        Ok(serialize::audio_status(&status))
    }

    async fn audio_release_device(&self) -> Result<Value, ServiceErrorPayload> {
        let state = Arc::clone(&self.state);
        tokio::task::spawn_blocking(move || state.audio.close(now_ms()))
            .await
            .map_err(|error| {
                ServiceErrorPayload::new("INTERNAL", format!("close task failed: {error}"))
            })?;
        Ok(serialize::audio_status(&self.state.audio.status()))
    }

    async fn audio_test(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        if !self.state.audio.is_running() {
            return Err(ServiceErrorPayload::new(
                "STREAM_NO_INPUT",
                "Select an audio input before testing it.",
            ));
        }

        let duration_ms = params
            .get("durationMs")
            .and_then(Value::as_u64)
            .unwrap_or(3_000)
            .clamp(250, 30_000);

        // Drain whatever has accumulated so the window only contains audio
        // captured during the test, then listen.
        let levels = self.state.audio.levels();
        let _ = levels.drain();
        let before = self.state.audio.status().frames_captured;

        tokio::time::sleep(core::time::Duration::from_millis(duration_ms)).await;

        let after = self.state.audio.status().frames_captured;
        let window = levels.drain();

        let frames_analysed = after.saturating_sub(before);
        let (peak_db, rms_db, clipping) = match window {
            Some(window) => (
                serialize::finite_db(window.peak),
                serialize::finite_db(window.rms),
                window.window_clips > 0,
            ),
            None => (None, None, false),
        };

        // -60 dBFS is roughly the noise floor of a quiet line input; below that
        // there is nothing an operator would call signal.
        let signal_detected = peak_db.is_some_and(|db| db > -60.0);
        let verdict = if frames_analysed == 0 {
            "No audio arrived from the interface. Check that the device is still connected."
        } else if clipping {
            "Signal is clipping. Reduce the AUX/monitor send level on the mixer."
        } else if !signal_detected {
            "No signal detected. Check the mixer's monitor send and the cable to this input."
        } else if peak_db.is_some_and(|db| db > -3.0) {
            "Signal present but very hot. Consider trimming the send a few dB."
        } else if peak_db.is_some_and(|db| db < -30.0) {
            "Signal present but quiet. Raise the monitor send on the mixer."
        } else {
            "Signal present at a healthy level."
        };

        Ok(json!({
            "signalDetected": signal_detected,
            "peakDb": peak_db,
            "rmsDb": rms_db,
            "clipping": clipping,
            "framesAnalysed": frames_analysed,
            "verdict": verdict,
        }))
    }

    // -- settings ----------------------------------------------------------

    fn settings_get(&self) -> Result<Value, ServiceErrorPayload> {
        serde_json::to_value(self.state.settings_snapshot()).map_err(|error| {
            ServiceErrorPayload::new("INTERNAL", format!("could not encode settings: {error}"))
        })
    }

    fn settings_update(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let current = self.state.settings_snapshot();
        let updated = current.apply_patch(&params).map_err(|error| {
            ServiceErrorPayload::new("SETTINGS_INVALID", error.message.clone())
                .with_field(error.field.clone())
        })?;

        // A release build must never be talked into the plaintext transport.
        if self.state.release_build && updated.streaming.transport == "udp-plaintext" {
            return Err(ServiceErrorPayload::new(
                "STREAM_TRANSPORT_REFUSED",
                "Unencrypted audio transport is not permitted in a release build.",
            )
            .with_field("streaming.transport"));
        }

        self.state
            .persist_settings(&updated)
            .map_err(serialize::persistence_error)?;
        *self.state.settings.lock() = updated.clone();
        *self.state.selected_adapter.lock() = updated.network.adapter_id.clone();

        let restart_required = crate::settings::AppSettings::restart_required_for(&params);
        Ok(json!({
            "settings": serde_json::to_value(&updated).unwrap_or(Value::Null),
            "restartRequired": restart_required,
        }))
    }

    // -- network -----------------------------------------------------------

    /// Enumerates adapters off the async runtime.
    ///
    /// Blocking: the Windows call walks the whole adapter table, and doing that
    /// on a runtime worker would stall every other control request behind it.
    async fn enumerate_adapters(
        &self,
    ) -> Result<Vec<device_discovery::adapters::NetworkAdapter>, ServiceErrorPayload> {
        tokio::task::spawn_blocking(device_discovery::adapters::enumerate_adapters)
            .await
            .map_err(|error| {
                ServiceErrorPayload::new("INTERNAL", format!("adapter task failed: {error}"))
            })?
            .map_err(|error| {
                ServiceErrorPayload::new(
                    "NETWORK_ADAPTER_UNAVAILABLE",
                    format!("could not enumerate network adapters: {error}"),
                )
            })
    }

    async fn network_get_status(&self) -> Result<Value, ServiceErrorPayload> {
        let adapters = self.enumerate_adapters().await?;
        let settings = self.state.settings_snapshot();
        let selected = self.state.selected_adapter.lock().clone();
        Ok(serialize::network_status(&adapters, selected.as_deref(), &settings))
    }

    async fn network_select_adapter(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let adapter_id = params
            .get("adapterId")
            .and_then(Value::as_str)
            .ok_or_else(|| Self::invalid("adapterId is required", "params.adapterId"))?
            .to_owned();

        let adapters = tokio::task::spawn_blocking(device_discovery::adapters::enumerate_adapters)
            .await
            .map_err(|error| {
                ServiceErrorPayload::new("INTERNAL", format!("adapter task failed: {error}"))
            })?
            .map_err(|error| {
                ServiceErrorPayload::new(
                    "NETWORK_ADAPTER_UNAVAILABLE",
                    format!("could not enumerate network adapters: {error}"),
                )
            })?;

        let adapter = adapters
            .iter()
            .find(|candidate| candidate.id == adapter_id)
            .ok_or_else(|| {
                ServiceErrorPayload::new(
                    "NETWORK_ADAPTER_UNAVAILABLE",
                    "That network adapter is no longer present.",
                )
            })?;

        let requested_address = params.get("address").and_then(Value::as_str);
        let bind_address = match requested_address {
            Some(address) => {
                if !adapter
                    .addresses
                    .iter()
                    .any(|info| info.address == address)
                {
                    return Err(Self::invalid(
                        "that address does not belong to the chosen adapter",
                        "params.address",
                    ));
                }
                address.to_owned()
            }
            None => adapter
                .primary_ipv4()
                .ok_or_else(|| {
                    ServiceErrorPayload::new(
                        "NETWORK_ADAPTER_UNAVAILABLE",
                        "That adapter has no IPv4 address, so audio cannot be sent from it.",
                    )
                })?
                .address
                .clone(),
        };

        let mut settings = self.state.settings_snapshot();
        let bind_for_server = bind_address.clone();
        settings.network.adapter_id = Some(adapter_id.clone());
        settings.network.bind_address = Some(bind_address);
        settings.validate().map_err(|error| {
            ServiceErrorPayload::new("SETTINGS_INVALID", error.message).with_field(error.field)
        })?;

        self.state
            .persist_settings(&settings)
            .map_err(serialize::persistence_error)?;
        *self.state.settings.lock() = settings.clone();
        *self.state.selected_adapter.lock() = Some(adapter_id.clone());

        // Receivers join on the adapter the operator just chose, so the
        // listener moves with it. A listener left on the old adapter would be
        // offering the join port on a network they did not pick.
        self.ensure_control_server(&bind_for_server).await;

        let status = serialize::network_status(&adapters, Some(&adapter_id), &settings);

        // Broadcast it. The caller gets the status back anyway, but anything
        // else looking at network state — the join panel needs a bound address
        // before it will mint a code — must not be left with a stale view just
        // because the change came from somewhere it was not watching.
        if let Some(sink) = self.sink() {
            let _ = sink.send(desktop_ipc::codec::Envelope::event(
                "network.status",
                status.clone(),
                now_ms(),
            ));
        }

        Ok(status)
    }

    // -- session -----------------------------------------------------------

    fn session_get_current(&self) -> Result<Value, ServiceErrorPayload> {
        let session = self.state.session.lock();
        Ok(match session.as_ref() {
            Some(session) => serialize::session(session, now_ms()),
            None => Value::Null,
        })
    }

    fn session_history(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 200) as u32;
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0) as u32;

        let database = self.state.database.lock();
        let repository = persistence::repositories::SessionRepository::new(database.connection());
        let sessions = repository
            .history(limit, offset)
            .map_err(serialize::persistence_error)?;
        let total = repository.count().map_err(serialize::persistence_error)?;

        Ok(json!({
            "sessions": sessions.iter().map(serialize::session_record).collect::<Vec<_>>(),
            "total": total,
        }))
    }

    fn session_events(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| Self::invalid("sessionId is required", "params.sessionId"))?;
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 1_000) as u32;

        let database = self.state.database.lock();
        let repository = persistence::repositories::SessionRepository::new(database.connection());
        let rows = repository
            .events(session_id, limit)
            .map_err(serialize::persistence_error)?;

        Ok(json!({
            "events": rows
                .iter()
                .map(|(id, at, kind, severity, message, data)| json!({
                    "id": id,
                    "sessionId": session_id,
                    "at": at,
                    "kind": kind,
                    "severity": severity,
                    "message": message,
                    "data": data
                        .as_ref()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok()),
                }))
                .collect::<Vec<_>>(),
        }))
    }

    // -- streaming ---------------------------------------------------------

    /// Starts sending audio.
    ///
    /// Everything that could make this a lie is checked first: a device must
    /// actually be capturing, an adapter must actually be present, and the
    /// transport must be one this build will carry. A dashboard that says
    /// "streaming" while nobody can hear anything is worse than an error.
    async fn stream_start(&self) -> Result<Value, ServiceErrorPayload> {
        if self.state.sender.lock().is_some() {
            // Idempotent rather than an error: two clicks on Start should not
            // produce a failure dialog, and the stream is already what was
            // asked for.
            return Ok(self.stream_status());
        }

        let settings = self.state.settings_snapshot();
        let adapters = self.enumerate_adapters().await?;
        let selected = self.state.selected_adapter.lock().clone();
        let bind_address = serialize::selected_address(&adapters, selected.as_deref(), &settings)
            .ok_or_else(|| {
                ServiceErrorPayload::new(
                    "NETWORK_ADAPTER_UNAVAILABLE",
                    "Choose a streaming network adapter before starting.",
                )
                .with_detail("Pick the adapter facing your access point on the Network page.")
            })?;

        // The device's negotiated rate, not the configured one: WASAPI may
        // have given us 44.1 kHz whatever the setting says, and labelling the
        // packets 48 kHz would make every receiver play the mix sharp.
        let sample_rate = self.state.audio.sample_rate().ok_or_else(|| {
            ServiceErrorPayload::new(
                "STREAM_NO_INPUT",
                "No audio is being captured, so there is nothing to stream.",
            )
            .with_detail("Select an input device on the Audio Input page first.")
        })?;

        let config = crate::stream_task::stream_config(&settings, &bind_address, sample_rate);
        let session_id = crate::stream_task::new_session_id();
        // The audio key is derived from the join token, so an encrypted
        // stream needs one to exist. Held only for the call: it is not stored
        // anywhere, and the sender keeps the derived key rather than the
        // credential it came from.
        let token = self
            .state
            .join_code
            .lock()
            .as_ref()
            .map(|minted| minted.code.token.clone());
        let sender = crate::stream_task::start(
            &self.state.audio,
            config,
            session_id,
            self.state.release_build,
            token.as_deref(),
        )?;

        let started_at = now_ms();
        let stream_port = sender.config().stream_port;

        // Receivers that joined before the operator pressed Start — which is
        // the normal order, since people connect their phones while the band
        // is still setting up — are enrolled now. Without this they sit in the
        // roster being sent nothing at all.
        //
        // The order inside this block is load-bearing. Every receiver is told
        // the session id *before* any audio is aimed at it, because the audio
        // key is derived from that id: packets that arrive first cannot be
        // opened and are discarded. Getting this backwards costs a few refused
        // packets at the top of every stream, and told nothing at all it is
        // the whole service.
        let params = crate::stream_task::params(session_id, sender.config());
        if let Some(server) = self.state.control_server.lock().as_ref() {
            let sessions = server.sessions();
            sessions.broadcast("stream.params", &params, now_ms());
            for (receiver_id, address) in sessions.audio_destinations(stream_port) {
                sender.add_destination(&receiver_id, address);
            }
        }

        *self.state.sender.lock() = Some(crate::state::ActiveStream {
            sender,
            session_id,
            started_at,
        });

        let status = self.stream_status();
        self.push_event("stream.status", status.clone());
        Ok(status)
    }

    /// Stops sending audio.
    fn stream_stop(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let immediate = params
            .get("immediate")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if let Some(mut active) = self.state.sender.lock().take() {
            // Draining sends an end-of-stream marker, so receivers stop
            // cleanly instead of waiting out their jitter buffers on sudden
            // silence.
            crate::stream_task::stop(&mut active.sender, immediate);
        }

        let status = self.stream_status();
        self.push_event("stream.status", status.clone());
        Ok(status)
    }

    fn stream_status(&self) -> Value {
        match self.state.sender.lock().as_ref() {
            Some(active) => {
                crate::stream_task::status(&active.sender, active.started_at, active.session_id)
            }
            None => serialize::stream_status_stopped(),
        }
    }

    /// Sends an event if a client is attached. Dropped otherwise: a renderer
    /// that is not listening asks for the state when it reconnects.
    fn push_event(&self, event: &str, payload: Value) {
        if let Some(sink) = self.sink() {
            let _ = sink.send(desktop_ipc::codec::Envelope::event(
                event,
                payload,
                now_ms(),
            ));
        }
    }

    // -- receivers ---------------------------------------------------------

    fn receivers_list(&self) -> Result<Value, ServiceErrorPayload> {
        // Really empty until the control server lands. No placeholder devices:
        // the UI shows its empty state and the join QR instead.
        let roster = self.state.roster.lock();
        let settings = self.state.settings_snapshot();
        Ok(json!({
            "receivers": roster.entries().iter().map(serialize::roster_entry).collect::<Vec<_>>(),
            "joinCode": self.current_join_code(),
            "maxReceivers": settings.streaming.max_receivers,
        }))
    }

    /// The current join code, or `Value::Null` if there is none or it expired.
    ///
    /// Expiry is evaluated on read rather than on a timer: a code nobody is
    /// asking about does not need waking a thread to retire it.
    fn current_join_code(&self) -> Value {
        let now = now_ms();
        let mut held = self.state.join_code.lock();
        if held.as_ref().is_some_and(|minted| !minted.code.is_valid(now)) {
            *held = None;
        }
        match held.as_ref() {
            Some(minted) => serialize::join_code(
                &minted.code,
                &minted.host,
                &self.state.settings_snapshot(),
            ),
            None => Value::Null,
        }
    }

    /// Sets the operator's label for a receiver.
    ///
    /// Persisted, so it survives the phone going into a pocket and coming
    /// back — the whole point of having a label rather than using the device's
    /// own name.
    fn receivers_rename(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let receiver_id = Self::required_receiver_id(&params)?;
        let display_name = params
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Self::invalid("displayName is required", "params.displayName"))?;

        {
            let database = self.state.database.lock();
            persistence::ReceiverRepository::new(database.connection())
                .set_display_name(receiver_id, display_name)
                .map_err(serialize::persistence_error)?;
        }

        let mut roster = self.state.roster.lock();
        let entry = roster
            .rename(receiver_id, Some(display_name))
            .ok_or_else(|| Self::receiver_not_found(receiver_id))?;
        let payload = serialize::roster_entry(entry);
        drop(roster);

        self.push_roster();
        Ok(payload)
    }

    /// Drops a receiver from this session. It may rejoin if it still has a
    /// valid code and has not been revoked.
    fn receivers_disconnect(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let receiver_id = Self::required_receiver_id(&params)?;

        let entry = self
            .state
            .roster
            .lock()
            .get(receiver_id)
            .cloned()
            .ok_or_else(|| Self::receiver_not_found(receiver_id))?;

        if let Some(server) = self.state.control_server.lock().as_ref() {
            server.disconnect(receiver_id);
        }
        self.state.roster.lock().leave(receiver_id);
        if let Some(active) = self.state.sender.lock().as_ref() {
            active.sender.remove_destination(receiver_id);
        }

        self.push_roster();
        Ok(serialize::roster_entry(&entry))
    }

    /// Withdraws a receiver's authorisation permanently.
    ///
    /// Persisted *before* the disconnect, so a device that reconnects in the
    /// gap is already refused. The other order would leave a window in which
    /// the operator has revoked a phone and it is back before the revocation
    /// is recorded.
    fn receivers_revoke(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let receiver_id = Self::required_receiver_id(&params)?;

        {
            let database = self.state.database.lock();
            persistence::ReceiverRepository::new(database.connection())
                .revoke(receiver_id, now_ms())
                .map_err(serialize::persistence_error)?;
        }

        let entry = self.state.roster.lock().get(receiver_id).cloned();

        if let Some(server) = self.state.control_server.lock().as_ref() {
            server.disconnect(receiver_id);
        }
        self.state.roster.lock().leave(receiver_id);
        if let Some(active) = self.state.sender.lock().as_ref() {
            active.sender.remove_destination(receiver_id);
        }
        self.push_roster();

        // A device can be revoked without being connected — an operator
        // ejecting a phone that is already out of range, say — so a missing
        // roster entry is reported from the database instead of as an error.
        match entry {
            Some(entry) => {
                let mut payload = serialize::roster_entry(&entry);
                payload["pairingState"] = Value::String("revoked".to_owned());
                payload["connectionState"] = Value::String("offline".to_owned());
                Ok(payload)
            }
            None => {
                let database = self.state.database.lock();
                let record = persistence::ReceiverRepository::new(database.connection())
                    .get(receiver_id)
                    .map_err(serialize::persistence_error)?
                    .ok_or_else(|| Self::receiver_not_found(receiver_id))?;
                Ok(serialize::stored_receiver(&record))
            }
        }
    }

    fn required_receiver_id(params: &Value) -> Result<&str, ServiceErrorPayload> {
        params
            .get("receiverId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Self::invalid("receiverId is required", "params.receiverId"))
    }

    fn receiver_not_found(receiver_id: &str) -> ServiceErrorPayload {
        ServiceErrorPayload::new(
            "RECEIVER_NOT_FOUND",
            format!("No receiver {receiver_id} is known."),
        )
    }

    /// Pushes the roster to the renderer.
    fn push_roster(&self) {
        let roster = self.state.roster.lock();
        let receivers: Vec<Value> = roster
            .entries()
            .iter()
            .map(serialize::roster_entry)
            .collect();
        drop(roster);
        self.push_event("receivers.changed", json!({ "receivers": receivers }));
    }

    /// Makes sure the receiver control server is listening on `bind_address`.
    ///
    /// Started lazily rather than at boot, because until an adapter is chosen
    /// there is no address to bind to — and binding to all of them would put
    /// the join port on every network the machine is attached to, including a
    /// venue's guest Wi-Fi.
    pub async fn ensure_control_server(&self, bind_address: &str) {
        {
            let running = self.state.control_server.lock();
            if running
                .as_ref()
                .is_some_and(|server| server.local_address().ip().to_string() == bind_address)
            {
                return;
            }
        }

        // The address changed, so the old listener is on the wrong network.
        self.state.control_server.lock().take();

        let settings = self.state.settings_snapshot();
        let host = Arc::new(crate::control_task::ServiceControlHost::new(
            Arc::clone(&self.state),
            {
                let state = Arc::clone(&self.state);
                Arc::new(move |event: &str, payload: Value| {
                    state.push_event(event, payload);
                })
            },
        ));

        let config = control_server::ControlServerConfig {
            bind_address: bind_address.to_owned(),
            port: settings.network.control_port,
        };
        match control_server::ControlServer::start(config, host).await {
            Ok(server) => {
                info!(address = %server.local_address(), "receivers can now join");
                *self.state.control_server.lock() = Some(server);
            }
            Err(error) => {
                // Not fatal, and deliberately not an error returned to the
                // caller: the operator asked to select an adapter or mint a
                // code, and that part worked. The failure is surfaced as an
                // operational error instead, with what to do about it.
                warn!(%error, port = settings.network.control_port, "could not start the control server");
                self.push_event(
                    "service.error",
                    json!({
                        "code": "STREAM_BIND_FAILED",
                        "message": format!(
                            "Receivers cannot join: port {} could not be opened.",
                            settings.network.control_port
                        ),
                        "detail": "Another application may be using it. Check the Network page.",
                        "at": now_ms(),
                        "retryable": true,
                    }),
                );
            }
        }
    }

    /// Mints a join code, replacing any previous one.
    ///
    /// Rotation is the whole mechanism for "that QR got shared" — the old token
    /// stops working the moment this returns.
    async fn receivers_create_join_code(&self) -> Result<Value, ServiceErrorPayload> {
        let settings = self.state.settings_snapshot();

        // The address has to come from an adapter that is *present*, through
        // the same derivation the Network page uses. Gating on the persisted
        // `bind_address` alone was wrong in both directions: it minted codes
        // the join panel then refused to display, and it would have put the
        // address of an unplugged adapter into a QR that every device fails to
        // connect to.
        let adapters = self.enumerate_adapters().await?;
        let selected = self.state.selected_adapter.lock().clone();
        let host = serialize::selected_address(&adapters, selected.as_deref(), &settings)
            .ok_or_else(|| {
                ServiceErrorPayload::new(
                    "NETWORK_ADAPTER_UNAVAILABLE",
                    "Choose a streaming network adapter before creating a join code.",
                )
                .with_detail(
                    "Receivers need an address to connect to. Pick the adapter facing your \
                     access point on the Network page.",
                )
            })?;

        // Minting a code is the moment it matters that something is listening:
        // the operator is about to hold a QR up to a band. Starting the server
        // here as well as on adapter selection means a code is never displayed
        // for a port nobody is accepting on.
        self.ensure_control_server(&host).await;

        let code = receiver_session::JoinCode::mint(
            now_ms(),
            receiver_session::join_code::DEFAULT_TTL_MS,
        );
        let payload = serialize::join_code(&code, &host, &settings);
        *self.state.join_code.lock() =
            Some(crate::state::MintedJoinCode { code, host });

        // Push it so any other view of the QR updates rather than showing a
        // code that has just been invalidated.
        if let Some(sink) = self.sink() {
            let _ = sink.send(desktop_ipc::codec::Envelope::event(
                "receivers.joinCode",
                payload.clone(),
                now_ms(),
            ));
        }

        debug!("minted a join code");
        Ok(payload)
    }

    // -- diagnostics -------------------------------------------------------

    fn diagnostics_snapshot(&self) -> Result<Value, ServiceErrorPayload> {
        Ok(serialize::diagnostics_snapshot(&self.state))
    }

    fn diagnostics_subscribe(&self, params: Value) -> Result<Value, ServiceErrorPayload> {
        let enabled = params
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| Self::invalid("enabled is required", "params.enabled"))?;
        let interval_ms = params
            .get("intervalMs")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(200, 10_000) as u32;

        *self.state.diagnostics_streaming.lock() =
            DiagnosticsSubscription { enabled, interval_ms };
        debug!(enabled, interval_ms, "diagnostics subscription updated");
        Ok(json!({ "enabled": enabled, "intervalMs": interval_ms }))
    }
}

impl ControlHandler for StageLinkHandler {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, ServiceErrorPayload> {
        // `params` may legitimately be null for no-argument methods; normalise
        // so every arm can use `.get()` without checking.
        let params = if params.is_null() { json!({}) } else { params };

        match method {
            // audio
            "audio.listDevices" => self.audio_list_devices(params).await,
            "audio.selectDevice" => self.audio_select_device(params).await,
            "audio.releaseDevice" => self.audio_release_device().await,
            "audio.getStatus" => Ok(serialize::audio_status(&self.state.audio.status())),
            "audio.test" => self.audio_test(params).await,

            // settings
            "settings.get" => self.settings_get(),
            "settings.update" => self.settings_update(params),

            // network
            "network.getStatus" => self.network_get_status().await,
            "network.selectAdapter" => self.network_select_adapter(params).await,
            "network.checkFirewall" => Err(Self::not_implemented(
                "network.checkFirewall",
                "Phase 4 (desktop management)",
            )),

            // session
            "session.getCurrent" => self.session_get_current(),
            "session.history" => self.session_history(params),
            "session.events" => self.session_events(params),

            // receivers
            "receivers.list" => self.receivers_list(),
            "receivers.createJoinCode" => self.receivers_create_join_code().await,
            "receivers.rename" => self.receivers_rename(params),
            "receivers.disconnect" => self.receivers_disconnect(params),
            "receivers.revoke" => self.receivers_revoke(params),

            // Called by a receiver, not by the desktop UI. They arrive on the
            // control server, which does not exist yet, so right now they are
            // reachable only over the local pipe where no receiver can be.
            //
            // **They must stay refused on this path when the control server
            // lands.** A heartbeat or a telemetry report only means anything
            // attributed to an admitted receiver, and a pipe client has no
            // receiver identity — honouring one here would let the renderer
            // keep a phantom device alive in the roster, or write quality
            // figures no device reported. The control server implements them
            // against the connection's own identity; this arm does not.
            "receiver.heartbeat" | "receiver.telemetry" => {
                Err(Self::not_implemented(method, "Phase 5 (receiver connections)"))
            }

            // streaming
            "stream.start" => self.stream_start().await,
            "stream.stop" => self.stream_stop(params),
            "stream.getStatus" => Ok(self.stream_status()),

            // diagnostics
            "diagnostics.getSnapshot" => self.diagnostics_snapshot(),
            "diagnostics.subscribe" => self.diagnostics_subscribe(params),

            // service
            "service.ping" => Ok(json!({
                "at": now_ms(),
                "uptimeMs": self.state.uptime_ms(),
            })),
            "service.getInfo" => Ok(serialize::service_info(&self.state)),
            "service.shutdown" => Ok(json!({ "accepted": true })),

            unknown => {
                warn!(method = unknown, "rejected an unknown control method");
                Err(ServiceErrorPayload::new(
                    "UNKNOWN_METHOD",
                    format!("{unknown} is not a StageLink control method."),
                ))
            }
        }
    }

    fn service_version(&self) -> String {
        SERVICE_VERSION.to_owned()
    }

    fn control_protocol_version(&self) -> String {
        CONTROL_PROTOCOL_VERSION.to_owned()
    }

    fn audio_protocol_version(&self) -> u8 {
        audio_protocol::AUDIO_PROTOCOL_VERSION
    }

    fn available_audio_apis(&self) -> Vec<String> {
        audio_capture::AudioApi::available()
            .iter()
            .map(|api| api.as_str().to_owned())
            .collect()
    }

    fn on_client_connected(&self, sink: EventSink) {
        // Push a full snapshot immediately: a reconnecting renderer must not
        // have to poll to find out what the audio engine is doing.
        let _ = sink.send(desktop_ipc::codec::Envelope::event(
            "audio.status",
            serialize::audio_status(&self.state.audio.status()),
            now_ms(),
        ));
        let _ = sink.send(desktop_ipc::codec::Envelope::event(
            "service.state",
            serialize::service_info(&self.state),
            now_ms(),
        ));
        *self.event_sink.lock() = Some(sink.clone());
        // Also shared, so the control server can report a receiver joining
        // or leaving without a handle on this handler.
        *self.state.event_sink.lock() = Some(sink);
        debug!("event sink attached");
    }

    fn on_client_disconnected(&self) {
        *self.event_sink.lock() = None;
        // Deliberately *not* stopping audio: losing the UI must never take the
        // monitor feed down mid-service.
        debug!("event sink detached; audio continues");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use persistence::{Database, SecretStore};

    fn handler() -> StageLinkHandler {
        let database = Database::open_in_memory().unwrap();
        let directory =
            std::env::temp_dir().join(format!("stagelink-handler-{}", uuid::Uuid::new_v4()));
        let secrets = SecretStore::new(&directory).unwrap();
        StageLinkHandler::new(ServiceState::new(database, secrets, false))
    }

    /// Selects the first adapter that could really carry a stream, returning
    /// the address the service bound to.
    ///
    /// `None` means this machine has no usable adapter, in which case the
    /// caller skips rather than asserting about hardware that is not there.
    /// Nothing is stubbed: a join code's host must come from a real adapter,
    /// and a test that invented one would not be testing that.
    async fn select_a_usable_adapter(handler: &StageLinkHandler) -> Option<String> {
        let status = handler.handle("network.getStatus", Value::Null).await.ok()?;
        for adapter in status["adapters"].as_array()?.clone() {
            let usable = adapter["isUp"] == true
                && adapter["kind"] != "loopback"
                && adapter["addresses"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|entry| entry["family"] == "ipv4"));
            if !usable {
                continue;
            }
            if let Ok(updated) = handler
                .handle(
                    "network.selectAdapter",
                    json!({ "adapterId": adapter["id"] }),
                )
                .await
            {
                return updated["selectedAddress"].as_str().map(ToOwned::to_owned);
            }
        }
        None
    }

    #[tokio::test]
    async fn every_published_method_is_actually_dispatched() {
        // Guards against DISPATCHED_METHODS and the match arms drifting apart.
        let handler = handler();
        for method in DISPATCHED_METHODS {
            let outcome = handler.handle(method, json!({})).await;
            if let Err(error) = &outcome {
                assert_ne!(
                    error.code, "UNKNOWN_METHOD",
                    "{method} is published but not dispatched"
                );
            }
        }
    }

    #[tokio::test]
    async fn ping_reports_a_real_uptime() {
        let handler = handler();
        let result = handler.handle("service.ping", Value::Null).await.unwrap();
        assert!(result["at"].as_i64().unwrap() > 0);
        assert!(result["uptimeMs"].as_i64().is_some());
    }

    #[tokio::test]
    async fn an_unknown_method_is_refused_rather_than_guessed_at() {
        let handler = handler();
        let error = handler
            .handle("system.exec", json!({ "cmd": "calc.exe" }))
            .await
            .expect_err("unknown methods must be refused");
        assert_eq!(error.code, "UNKNOWN_METHOD");
    }

    #[tokio::test]
    async fn unimplemented_methods_report_not_implemented_with_a_phase() {
        let handler = handler();
        // Only these two remain, and they are not "not built yet" — they are
        // refused on *this* path for good. A heartbeat only means something
        // attributed to an admitted receiver, and a pipe client has no
        // receiver identity; honouring one here would let the renderer keep a
        // phantom device alive in the roster. The control server implements
        // them against the connection's own identity.
        for method in ["receiver.heartbeat", "receiver.telemetry"] {
            match handler.handle(method, json!({})).await {
                Err(error) => {
                    assert_eq!(error.code, "NOT_IMPLEMENTED", "for {method}");
                    assert!(
                        error.detail.as_ref().is_some_and(|d| d.contains("Phase")),
                        "{method} should name the phase"
                    );
                }
                Ok(value) => panic!("{method} must not return a fake success: {value}"),
            }
        }
    }

    #[tokio::test]
    async fn settings_round_trip_through_the_control_methods() {
        let handler = handler();

        let initial = handler.handle("settings.get", Value::Null).await.unwrap();
        assert_eq!(initial["audio"]["sampleRate"], 48_000);
        assert_eq!(initial["streaming"]["transport"], "udp-encrypted");

        let updated = handler
            .handle("settings.update", json!({ "audio": { "channelIndex": 2 } }))
            .await
            .unwrap();
        assert_eq!(updated["settings"]["audio"]["channelIndex"], 2);
        assert_eq!(
            updated["restartRequired"],
            json!(["audio.channelIndex"]),
            "changing the input channel needs a restart and must say so"
        );

        let reread = handler.handle("settings.get", Value::Null).await.unwrap();
        assert_eq!(reread["audio"]["channelIndex"], 2);
    }

    #[tokio::test]
    async fn an_invalid_settings_patch_is_refused_with_its_field() {
        let handler = handler();
        let error = handler
            .handle("settings.update", json!({ "audio": { "sampleRate": 3 } }))
            .await
            .expect_err("an impossible sample rate must be refused");
        assert_eq!(error.code, "SETTINGS_INVALID");
        assert_eq!(error.field.as_deref(), Some("audio.sampleRate"));
    }

    #[tokio::test]
    async fn an_appearance_change_does_not_require_a_restart() {
        let handler = handler();
        let updated = handler
            .handle("settings.update", json!({ "appearance": { "theme": "light" } }))
            .await
            .unwrap();
        assert_eq!(updated["restartRequired"], json!([]));
    }

    #[tokio::test]
    async fn selecting_a_device_requires_both_parameters() {
        let handler = handler();
        let error = handler
            .handle("audio.selectDevice", json!({ "channelIndex": 0 }))
            .await
            .expect_err("deviceId is mandatory");
        assert_eq!(error.code, "INVALID_REQUEST");
        assert_eq!(error.field.as_deref(), Some("params.deviceId"));

        let error = handler
            .handle("audio.selectDevice", json!({ "deviceId": "x" }))
            .await
            .expect_err("channelIndex is mandatory");
        assert_eq!(error.field.as_deref(), Some("params.channelIndex"));
    }

    #[tokio::test]
    async fn an_unknown_share_mode_is_refused() {
        let handler = handler();
        let error = handler
            .handle(
                "audio.selectDevice",
                json!({ "deviceId": "x", "channelIndex": 0, "shareMode": "telepathic" }),
            )
            .await
            .expect_err("an unknown share mode must be refused");
        assert_eq!(error.field.as_deref(), Some("params.shareMode"));
    }

    #[tokio::test]
    async fn audio_status_on_a_fresh_service_is_idle_with_no_device() {
        let handler = handler();
        let status = handler.handle("audio.getStatus", Value::Null).await.unwrap();
        assert_eq!(status["state"], "idle");
        assert_eq!(status["device"], Value::Null);
        assert_eq!(status["framesCaptured"], 0);
        assert_eq!(status["deviceConnected"], false);
    }

    #[tokio::test]
    async fn testing_an_input_before_one_is_selected_is_refused() {
        let handler = handler();
        let error = handler
            .handle("audio.test", json!({ "durationMs": 250 }))
            .await
            .expect_err("there is nothing to test yet");
        assert_eq!(error.code, "STREAM_NO_INPUT");
    }

    #[tokio::test]
    async fn the_receiver_list_is_empty_not_populated_with_demo_devices() {
        let handler = handler();
        let result = handler.handle("receivers.list", Value::Null).await.unwrap();
        assert_eq!(
            result["receivers"].as_array().unwrap().len(),
            0,
            "there must be no fabricated receivers"
        );
        assert_eq!(result["joinCode"], Value::Null, "no code until one is minted");
        assert_eq!(result["maxReceivers"], 16);
    }

    #[tokio::test]
    async fn minting_a_join_code_needs_a_streaming_adapter_first() {
        let handler = handler();
        // A code pointing nowhere would just produce devices that fail to join,
        // so this is refused with the action to take rather than a null host.
        let error = handler
            .handle("receivers.createJoinCode", Value::Null)
            .await
            .expect_err("no adapter is selected yet");
        assert_eq!(error.code, "NETWORK_ADAPTER_UNAVAILABLE");
        assert!(error.detail.unwrap().contains("Network page"));
    }

    /// The defect the desktop smoke test caught: minting used to need only a
    /// persisted `bindAddress`, while every other part of the system — the
    /// Network page, the join panel's own gate — asks whether an adapter is
    /// selected. The result was a real token the operator could never see,
    /// behind a warning telling them to do something they appeared to have
    /// done. Worse, the address could belong to an adapter that had since been
    /// unplugged, producing a QR nothing can connect to.
    #[tokio::test]
    async fn an_address_without_a_selected_adapter_does_not_allow_minting() {
        let handler = handler();
        handler
            .handle(
                "settings.update",
                json!({ "network": { "bindAddress": "10.0.0.1" } }),
            )
            .await
            .unwrap();

        let error = handler
            .handle("receivers.createJoinCode", Value::Null)
            .await
            .expect_err("an address alone is not a network selection");
        assert_eq!(error.code, "NETWORK_ADAPTER_UNAVAILABLE");

        // And the panel's view agrees, which is the whole point.
        let status = handler.handle("network.getStatus", Value::Null).await.unwrap();
        assert_eq!(status["selectedAddress"], Value::Null);
    }

    #[tokio::test]
    async fn a_minted_join_code_carries_a_scannable_payload() {
        let handler = handler();
        let Some(address) = select_a_usable_adapter(&handler).await else {
            eprintln!("skipping: no usable network adapter on this machine");
            return;
        };

        let code = handler
            .handle("receivers.createJoinCode", Value::Null)
            .await
            .unwrap();

        let payload = code["qrPayload"].as_str().unwrap();
        assert!(payload.starts_with("stagelink://join?"));
        assert!(payload.contains(&format!("h={address}")), "got {payload}");
        assert!(payload.contains(code["token"].as_str().unwrap()));
        assert_eq!(code["host"], address);
        assert!(code["expiresAt"].as_i64().unwrap() > code["issuedAt"].as_i64().unwrap());

        // And it is what receivers.list now reports.
        let list = handler.handle("receivers.list", Value::Null).await.unwrap();
        assert_eq!(list["joinCode"]["token"], code["token"]);
        assert_eq!(list["joinCode"]["host"], address);
    }

    #[tokio::test]
    async fn rotating_replaces_the_previous_code() {
        let handler = handler();
        if select_a_usable_adapter(&handler).await.is_none() {
            eprintln!("skipping: no usable network adapter on this machine");
            return;
        }

        let first = handler
            .handle("receivers.createJoinCode", Value::Null)
            .await
            .unwrap();
        let second = handler
            .handle("receivers.createJoinCode", Value::Null)
            .await
            .unwrap();

        assert_ne!(
            first["token"], second["token"],
            "rotation is the answer to a shared QR, so the token must change"
        );
        let list = handler.handle("receivers.list", Value::Null).await.unwrap();
        assert_eq!(
            list["joinCode"]["token"], second["token"],
            "only the newest code is live"
        );
    }

    /// Joins a receiver through the control host, which is how one really
    /// appears — there is no other way to put something in the roster.
    fn join_a_receiver(handler: &StageLinkHandler, key: &str) -> String {
        let host = crate::control_task::ServiceControlHost::new(
            handler.state(),
            Arc::new(|_event: &str, _payload: Value| {}),
        );
        let admission = control_server::join::Admission {
            receiver_id: control_server::join::receiver_id_for(key),
            identity: receiver_session::ReceiverIdentity::from_wire(
                "Pixel 8",
                "android",
                Some("14".to_owned()),
                Some("0.1.0".to_owned()),
            ),
            address: "10.0.0.5:51000".parse().unwrap(),
            audio_port: Some(45_872),
        };
        control_server::server::ControlHost::on_joined(&host, &admission);
        admission.receiver_id
    }

    #[tokio::test]
    async fn a_joined_receiver_appears_in_the_list_with_its_own_name() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");

        let list = handler.handle("receivers.list", Value::Null).await.unwrap();
        let receivers = list["receivers"].as_array().unwrap();
        assert_eq!(receivers.len(), 1);
        assert_eq!(receivers[0]["id"], receiver_id);
        assert_eq!(receivers[0]["deviceName"], "Pixel 8");
        assert_eq!(receivers[0]["platform"], "android");
        assert_eq!(receivers[0]["pairingState"], "approved");
    }

    #[tokio::test]
    async fn renaming_a_receiver_sets_the_label_the_operator_sees() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");

        let renamed = handler
            .handle(
                "receivers.rename",
                json!({ "receiverId": receiver_id, "displayName": "Lead Vocal" }),
            )
            .await
            .unwrap();
        assert_eq!(renamed["displayName"], "Lead Vocal");
        assert_eq!(renamed["deviceName"], "Pixel 8", "the device name is kept");

        // And it is persisted, so it survives the phone reconnecting.
        let state = handler.state();
        let database = state.database.lock();
        let record = persistence::ReceiverRepository::new(database.connection())
            .get(&receiver_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.display_name.as_deref(), Some("Lead Vocal"));
    }

    #[tokio::test]
    async fn renaming_an_unknown_receiver_is_refused() {
        let handler = handler();
        let error = handler
            .handle(
                "receivers.rename",
                json!({ "receiverId": "ghost", "displayName": "Nobody" }),
            )
            .await
            .expect_err("there is no such receiver");
        assert!(matches!(
            error.code.as_str(),
            "RECEIVER_NOT_FOUND" | "STORAGE_FAILURE"
        ));
    }

    #[tokio::test]
    async fn an_empty_display_name_is_refused_rather_than_clearing_the_label() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");
        let error = handler
            .handle(
                "receivers.rename",
                json!({ "receiverId": receiver_id, "displayName": "   " }),
            )
            .await
            .expect_err("whitespace is not a name");
        assert_eq!(error.code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn disconnecting_drops_the_receiver_but_does_not_revoke_it() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");

        handler
            .handle("receivers.disconnect", json!({ "receiverId": receiver_id }))
            .await
            .unwrap();

        let list = handler.handle("receivers.list", Value::Null).await.unwrap();
        assert!(list["receivers"].as_array().unwrap().is_empty());

        // Still approved: it may rejoin with a valid code.
        let state = handler.state();
        let database = state.database.lock();
        let record = persistence::ReceiverRepository::new(database.connection())
            .get(&receiver_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.pairing_state, "approved");
    }

    #[tokio::test]
    async fn revoking_withdraws_the_authorisation_permanently() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");

        let revoked = handler
            .handle("receivers.revoke", json!({ "receiverId": receiver_id }))
            .await
            .unwrap();
        assert_eq!(revoked["pairingState"], "revoked");

        let state = handler.state();
        let database = state.database.lock();
        let record = persistence::ReceiverRepository::new(database.connection())
            .get(&receiver_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.pairing_state, "revoked");
        assert!(record.revoked_at.is_some());
    }

    /// An operator ejecting a phone that has already gone out of range still
    /// has to be able to revoke it, or the only way to withdraw access would
    /// be to wait for the device to come back.
    #[tokio::test]
    async fn a_receiver_can_be_revoked_while_disconnected() {
        let handler = handler();
        let receiver_id = join_a_receiver(&handler, "ab");
        handler
            .handle("receivers.disconnect", json!({ "receiverId": receiver_id }))
            .await
            .unwrap();

        let revoked = handler
            .handle("receivers.revoke", json!({ "receiverId": receiver_id }))
            .await
            .expect("a device that is not here can still be revoked");
        assert_eq!(revoked["pairingState"], "revoked");
        assert_eq!(revoked["connectionState"], "offline");
    }

    #[tokio::test]
    async fn acting_on_a_receiver_with_no_id_is_refused() {
        let handler = handler();
        for method in ["receivers.rename", "receivers.disconnect", "receivers.revoke"] {
            let error = handler
                .handle(method, json!({}))
                .await
                .expect_err("an id is required");
            assert_eq!(error.code, "INVALID_REQUEST", "for {method}");
            assert_eq!(error.field.as_deref(), Some("params.receiverId"));
        }
    }

    #[tokio::test]
    async fn there_is_no_current_session_on_a_fresh_service() {
        let handler = handler();
        let result = handler
            .handle("session.getCurrent", Value::Null)
            .await
            .unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn session_history_is_empty_and_paginated() {
        let handler = handler();
        let result = handler
            .handle("session.history", json!({ "limit": 10 }))
            .await
            .unwrap();
        assert_eq!(result["total"], 0);
        assert_eq!(result["sessions"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn session_events_requires_a_session_id() {
        let handler = handler();
        let error = handler
            .handle("session.events", json!({}))
            .await
            .expect_err("sessionId is mandatory");
        assert_eq!(error.field.as_deref(), Some("params.sessionId"));
    }

    #[tokio::test]
    async fn a_diagnostics_snapshot_tags_every_figure_with_its_provenance() {
        let handler = handler();
        let snapshot = handler
            .handle("diagnostics.getSnapshot", Value::Null)
            .await
            .unwrap();

        assert!(snapshot["serviceUptimeMs"].as_i64().is_some());
        let latency = &snapshot["latency"];
        assert_eq!(latency["captureBufferMs"]["source"], "measured");
        assert_eq!(latency["packetFrameMs"]["source"], "estimated");
        assert_eq!(
            latency["receiverReportedOutputMs"]["source"],
            "receiver-reported"
        );
        assert!(
            latency["disclaimer"]
                .as_str()
                .unwrap()
                .contains("loopback"),
            "the latency disclaimer must be present"
        );
    }

    #[tokio::test]
    async fn an_idle_service_reports_null_latency_terms_rather_than_zero() {
        let handler = handler();
        let snapshot = handler
            .handle("diagnostics.getSnapshot", Value::Null)
            .await
            .unwrap();
        assert_eq!(
            snapshot["latency"]["captureBufferMs"]["value"],
            Value::Null,
            "with no device open there is no capture period to report"
        );
        assert_eq!(snapshot["latency"]["desktopSubtotalMs"]["value"], Value::Null);
    }

    #[tokio::test]
    async fn diagnostics_subscription_can_be_toggled_and_is_clamped() {
        let handler = handler();
        let result = handler
            .handle(
                "diagnostics.subscribe",
                json!({ "enabled": true, "intervalMs": 10 }),
            )
            .await
            .unwrap();
        assert_eq!(result["enabled"], true);
        assert_eq!(result["intervalMs"], 200, "too-fast polling must be clamped");

        let error = handler
            .handle("diagnostics.subscribe", json!({}))
            .await
            .expect_err("enabled is mandatory");
        assert_eq!(error.field.as_deref(), Some("params.enabled"));
    }

    #[tokio::test]
    async fn service_info_reports_the_protocol_versions() {
        let handler = handler();
        let info = handler.handle("service.getInfo", Value::Null).await.unwrap();
        assert_eq!(info["controlProtocolVersion"], CONTROL_PROTOCOL_VERSION);
        assert_eq!(info["audioProtocolVersion"], 1);
        assert_eq!(info["version"], SERVICE_VERSION);
    }

    #[tokio::test]
    async fn stream_status_reports_stopped_rather_than_pretending_to_run() {
        let handler = handler();
        let status = handler
            .handle("stream.getStatus", Value::Null)
            .await
            .unwrap();
        assert_eq!(status["state"], "stopped");
        assert_eq!(status["packetsSent"], 0);
        assert_eq!(status["activeReceivers"], 0);
        assert_eq!(status["config"], Value::Null);
    }

    #[tokio::test]
    async fn device_enumeration_returns_a_real_list_shape() {
        let handler = handler();
        let result = handler
            .handle("audio.listDevices", json!({ "probeFormats": false }))
            .await;

        match result {
            Ok(value) => {
                assert!(value["devices"].is_array());
                let apis = value["availableApis"].as_array().unwrap();
                if cfg!(windows) {
                    assert!(apis.iter().any(|api| api == "wasapi"));
                }
                assert!(
                    !apis.iter().any(|api| api == "asio"),
                    "ASIO must not be advertised without a backend"
                );
                for device in value["devices"].as_array().unwrap() {
                    assert!(device["id"].as_str().is_some_and(|id| !id.is_empty()));
                    assert!(device["name"].as_str().is_some());
                    assert_eq!(device["api"], "wasapi");
                }
            }
            Err(error) => {
                // Acceptable on a machine with no audio stack at all.
                assert_eq!(error.code, "AUDIO_CAPTURE_FAILED");
            }
        }
    }

    #[tokio::test]
    async fn null_params_are_accepted_for_no_argument_methods() {
        let handler = handler();
        assert!(handler.handle("audio.getStatus", Value::Null).await.is_ok());
        assert!(handler.handle("settings.get", Value::Null).await.is_ok());
        assert!(handler.handle("receivers.list", Value::Null).await.is_ok());
    }
}
