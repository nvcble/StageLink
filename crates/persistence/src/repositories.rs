//! Typed repository layer.
//!
//! Callers never write SQL. Each repository owns the statements for one table
//! and returns plain Rust structs, so a schema change is contained here instead
//! of being spread across the service.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{PersistenceError, Result};

/// Settings storage.
///
/// Settings are stored as one JSON document per top-level group (`audio`,
/// `network`, ...). The service validates each group against its schema on the
/// way in and on the way out, so a hand-edited database cannot push the audio
/// engine into an invalid configuration.
pub struct SettingsRepository<'a> {
    connection: &'a Connection,
}

impl<'a> SettingsRepository<'a> {
    #[must_use]
    pub const fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    /// Reads and decodes one settings group. `None` when never written.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<String> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;

        match raw {
            None => Ok(None),
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|source| PersistenceError::Decode { key: key.to_owned(), source }),
        }
    }

    /// Writes one settings group, replacing any previous value.
    pub fn set<T: Serialize>(&self, key: &str, value: &T, now_ms: i64) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.connection.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, json, now_ms],
        )?;
        Ok(())
    }

    /// Returns every stored group as raw JSON, for diagnostics export.
    pub fn all_raw(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self
            .connection
            .prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Removes a group, so the next read falls back to defaults.
    pub fn clear(&self, key: &str) -> Result<()> {
        self.connection
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }
}

/// A persisted receiver authorisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverRecord {
    pub id: String,
    pub device_name: String,
    pub display_name: Option<String>,
    pub platform: String,
    pub platform_version: Option<String>,
    pub app_version: Option<String>,
    /// Receiver's long-term public key, hex. Not a secret.
    pub public_key: Option<String>,
    pub key_fingerprint: Option<String>,
    pub pairing_state: String,
    pub last_ip_address: Option<String>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub approved_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl ReceiverRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            device_name: row.get("device_name")?,
            display_name: row.get("display_name")?,
            platform: row.get("platform")?,
            platform_version: row.get("platform_version")?,
            app_version: row.get("app_version")?,
            public_key: row.get("public_key")?,
            key_fingerprint: row.get("key_fingerprint")?,
            pairing_state: row.get("pairing_state")?,
            last_ip_address: row.get("last_ip_address")?,
            first_seen_at: row.get("first_seen_at")?,
            last_seen_at: row.get("last_seen_at")?,
            approved_at: row.get("approved_at")?,
            revoked_at: row.get("revoked_at")?,
        })
    }

    /// Whether this receiver may join a session without fresh approval.
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        self.pairing_state == "approved" && self.revoked_at.is_none()
    }
}

/// Receiver authorisation storage.
pub struct ReceiverRepository<'a> {
    connection: &'a Connection,
}

impl<'a> ReceiverRepository<'a> {
    #[must_use]
    pub const fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    /// Inserts or updates a receiver, preserving `first_seen_at` and the
    /// approval timestamps of an existing row.
    pub fn upsert(&self, record: &ReceiverRecord) -> Result<()> {
        self.connection.execute(
            "INSERT INTO receivers (
                 id, device_name, display_name, platform, platform_version, app_version,
                 public_key, key_fingerprint, pairing_state, last_ip_address,
                 first_seen_at, last_seen_at, approved_at, revoked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (id) DO UPDATE SET
                 device_name      = excluded.device_name,
                 display_name     = COALESCE(excluded.display_name, receivers.display_name),
                 platform         = excluded.platform,
                 platform_version = excluded.platform_version,
                 app_version      = excluded.app_version,
                 public_key       = COALESCE(excluded.public_key, receivers.public_key),
                 key_fingerprint  = COALESCE(excluded.key_fingerprint, receivers.key_fingerprint),
                 pairing_state    = excluded.pairing_state,
                 last_ip_address  = excluded.last_ip_address,
                 last_seen_at     = excluded.last_seen_at,
                 approved_at      = COALESCE(excluded.approved_at, receivers.approved_at),
                 revoked_at       = excluded.revoked_at",
            params![
                record.id,
                record.device_name,
                record.display_name,
                record.platform,
                record.platform_version,
                record.app_version,
                record.public_key,
                record.key_fingerprint,
                record.pairing_state,
                record.last_ip_address,
                record.first_seen_at,
                record.last_seen_at,
                record.approved_at,
                record.revoked_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<ReceiverRecord>> {
        Ok(self
            .connection
            .query_row(
                "SELECT * FROM receivers WHERE id = ?1",
                [id],
                ReceiverRecord::from_row,
            )
            .optional()?)
    }

    pub fn list(&self) -> Result<Vec<ReceiverRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM receivers ORDER BY last_seen_at DESC")?;
        let rows = statement
            .query_map([], ReceiverRecord::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Receivers that may rejoin without operator approval.
    pub fn list_trusted(&self) -> Result<Vec<ReceiverRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT * FROM receivers
             WHERE pairing_state = 'approved' AND revoked_at IS NULL
             ORDER BY last_seen_at DESC",
        )?;
        let rows = statement
            .query_map([], ReceiverRecord::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn set_display_name(&self, id: &str, display_name: &str) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE receivers SET display_name = ?2 WHERE id = ?1",
            params![id, display_name],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound { entity: "receiver", id: id.to_owned() });
        }
        Ok(())
    }

    /// Marks a receiver approved and trusted.
    pub fn approve(&self, id: &str, now_ms: i64) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE receivers
             SET pairing_state = 'approved', approved_at = ?2, revoked_at = NULL
             WHERE id = ?1",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound { entity: "receiver", id: id.to_owned() });
        }
        Ok(())
    }

    /// Withdraws authorisation. The row is kept so the device cannot return as
    /// an unrecognised newcomer.
    pub fn revoke(&self, id: &str, now_ms: i64) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE receivers SET pairing_state = 'revoked', revoked_at = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound { entity: "receiver", id: id.to_owned() });
        }
        Ok(())
    }
}

/// A persisted session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub label: Option<String>,
    pub state: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: i64,
    pub peak_receivers: i64,
    pub total_receivers: i64,
    pub last_error: Option<String>,
}

impl SessionRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            label: row.get("label")?,
            state: row.get("state")?,
            created_at: row.get("created_at")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            duration_ms: row.get("duration_ms")?,
            peak_receivers: row.get("peak_receivers")?,
            total_receivers: row.get("total_receivers")?,
            last_error: row.get("last_error")?,
        })
    }
}

/// Session and session-event storage.
pub struct SessionRepository<'a> {
    connection: &'a Connection,
}

impl<'a> SessionRepository<'a> {
    #[must_use]
    pub const fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    pub fn create(&self, record: &SessionRecord) -> Result<()> {
        self.connection.execute(
            "INSERT INTO sessions (
                 id, label, state, created_at, started_at, ended_at,
                 duration_ms, peak_receivers, total_receivers, last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.id,
                record.label,
                record.state,
                record.created_at,
                record.started_at,
                record.ended_at,
                record.duration_ms,
                record.peak_receivers,
                record.total_receivers,
                record.last_error,
            ],
        )?;
        Ok(())
    }

    pub fn update(&self, record: &SessionRecord) -> Result<()> {
        let changed = self.connection.execute(
            "UPDATE sessions SET
                 label = ?2, state = ?3, started_at = ?4, ended_at = ?5,
                 duration_ms = ?6, peak_receivers = ?7, total_receivers = ?8, last_error = ?9
             WHERE id = ?1",
            params![
                record.id,
                record.label,
                record.state,
                record.started_at,
                record.ended_at,
                record.duration_ms,
                record.peak_receivers,
                record.total_receivers,
                record.last_error,
            ],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound { entity: "session", id: record.id.clone() });
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<SessionRecord>> {
        Ok(self
            .connection
            .query_row(
                "SELECT * FROM sessions WHERE id = ?1",
                [id],
                SessionRecord::from_row,
            )
            .optional()?)
    }

    /// Most recent sessions first.
    pub fn history(&self, limit: u32, offset: u32) -> Result<Vec<SessionRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT * FROM sessions ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement
            .query_map(params![limit, offset], SessionRecord::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count(&self) -> Result<u32> {
        let total: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(total.max(0) as u32)
    }

    /// Appends an operational event.
    pub fn append_event(
        &self,
        id: &str,
        session_id: &str,
        at: i64,
        kind: &str,
        severity: &str,
        message: &str,
        data: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO session_events (id, session_id, at, kind, severity, message, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, session_id, at, kind, severity, message, data],
        )?;
        Ok(())
    }

    /// Newest events first.
    pub fn events(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<(String, i64, String, String, String, Option<String>)>> {
        let mut statement = self.connection.prepare(
            "SELECT id, at, kind, severity, message, data FROM session_events
             WHERE session_id = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![session_id, limit], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Records that a receiver joined a session.
    pub fn record_join(&self, session_id: &str, receiver_id: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "INSERT OR IGNORE INTO session_receivers (session_id, receiver_id, joined_at)
             VALUES (?1, ?2, ?3)",
            params![session_id, receiver_id, at],
        )?;
        Ok(())
    }

    /// Closes the most recent open participation row for a receiver.
    pub fn record_leave(&self, session_id: &str, receiver_id: &str, at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE session_receivers SET left_at = ?3
             WHERE session_id = ?1 AND receiver_id = ?2 AND left_at IS NULL",
            params![session_id, receiver_id, at],
        )?;
        Ok(())
    }

    /// Deletes sessions older than `cutoff_ms`, keeping at least `keep_at_least`
    /// of the newest. Stops the database growing without bound on a machine that
    /// runs services for years.
    pub fn prune(&self, cutoff_ms: i64, keep_at_least: u32) -> Result<usize> {
        let removed = self.connection.execute(
            "DELETE FROM sessions
             WHERE created_at < ?1
               AND id NOT IN (SELECT id FROM sessions ORDER BY created_at DESC LIMIT ?2)",
            params![cutoff_ms, keep_at_least],
        )?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct AudioSettings {
        device_id: Option<String>,
        channel_index: u16,
        sample_rate: u32,
    }

    fn receiver(id: &str) -> ReceiverRecord {
        ReceiverRecord {
            id: id.to_owned(),
            device_name: "Pixel 8".to_owned(),
            display_name: None,
            platform: "android".to_owned(),
            platform_version: Some("15".to_owned()),
            app_version: Some("0.1.0".to_owned()),
            public_key: Some("aabb".to_owned()),
            key_fingerprint: Some("AB:CD".to_owned()),
            pairing_state: "pending".to_owned(),
            last_ip_address: Some("10.0.0.5".to_owned()),
            first_seen_at: 1_000,
            last_seen_at: 1_000,
            approved_at: None,
            revoked_at: None,
        }
    }

    fn session(id: &str) -> SessionRecord {
        SessionRecord {
            id: id.to_owned(),
            label: Some("Sunday 1st".to_owned()),
            state: "idle".to_owned(),
            created_at: 1_000,
            started_at: None,
            ended_at: None,
            duration_ms: 0,
            peak_receivers: 0,
            total_receivers: 0,
            last_error: None,
        }
    }

    // -- settings ----------------------------------------------------------

    #[test]
    fn an_unwritten_settings_group_reads_as_none() {
        let database = Database::open_in_memory().unwrap();
        let repository = SettingsRepository::new(database.connection());
        assert_eq!(repository.get::<AudioSettings>("audio").unwrap(), None);
    }

    #[test]
    fn settings_round_trip() {
        let database = Database::open_in_memory().unwrap();
        let repository = SettingsRepository::new(database.connection());
        let settings = AudioSettings {
            device_id: Some("{endpoint}".to_owned()),
            channel_index: 2,
            sample_rate: 48_000,
        };
        repository.set("audio", &settings, 123).unwrap();
        assert_eq!(
            repository.get::<AudioSettings>("audio").unwrap(),
            Some(settings)
        );
    }

    #[test]
    fn writing_settings_twice_replaces_rather_than_duplicates() {
        let database = Database::open_in_memory().unwrap();
        let repository = SettingsRepository::new(database.connection());
        repository
            .set(
                "audio",
                &AudioSettings { device_id: None, channel_index: 0, sample_rate: 44_100 },
                1,
            )
            .unwrap();
        repository
            .set(
                "audio",
                &AudioSettings { device_id: None, channel_index: 1, sample_rate: 48_000 },
                2,
            )
            .unwrap();

        assert_eq!(repository.all_raw().unwrap().len(), 1);
        let stored = repository.get::<AudioSettings>("audio").unwrap().unwrap();
        assert_eq!(stored.sample_rate, 48_000);
        assert_eq!(stored.channel_index, 1);
    }

    #[test]
    fn corrupt_settings_json_reports_the_key_instead_of_panicking() {
        let database = Database::open_in_memory().unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('audio', 'not json', 1)",
                [],
            )
            .unwrap();
        let repository = SettingsRepository::new(database.connection());
        match repository.get::<AudioSettings>("audio") {
            Err(PersistenceError::Decode { key, .. }) => assert_eq!(key, "audio"),
            other => panic!("expected a Decode error naming the key, got {other:?}"),
        }
    }

    #[test]
    fn clearing_settings_falls_back_to_none() {
        let database = Database::open_in_memory().unwrap();
        let repository = SettingsRepository::new(database.connection());
        repository
            .set(
                "audio",
                &AudioSettings { device_id: None, channel_index: 0, sample_rate: 48_000 },
                1,
            )
            .unwrap();
        repository.clear("audio").unwrap();
        assert_eq!(repository.get::<AudioSettings>("audio").unwrap(), None);
    }

    // -- receivers ---------------------------------------------------------

    #[test]
    fn receivers_round_trip() {
        let database = Database::open_in_memory().unwrap();
        let repository = ReceiverRepository::new(database.connection());
        let record = receiver("r1");
        repository.upsert(&record).unwrap();
        assert_eq!(repository.get("r1").unwrap(), Some(record));
    }

    #[test]
    fn upsert_preserves_first_seen_and_the_operator_label() {
        let database = Database::open_in_memory().unwrap();
        let repository = ReceiverRepository::new(database.connection());
        repository.upsert(&receiver("r1")).unwrap();
        repository.set_display_name("r1", "Lead Vocal").unwrap();

        // The device reconnects and re-announces itself without a label.
        let mut reconnect = receiver("r1");
        reconnect.first_seen_at = 9_999;
        reconnect.last_seen_at = 9_999;
        reconnect.display_name = None;
        repository.upsert(&reconnect).unwrap();

        let stored = repository.get("r1").unwrap().unwrap();
        assert_eq!(stored.first_seen_at, 1_000, "first_seen_at must not move");
        assert_eq!(stored.last_seen_at, 9_999);
        assert_eq!(
            stored.display_name.as_deref(),
            Some("Lead Vocal"),
            "a reconnect must not wipe the operator's label"
        );
    }

    #[test]
    fn approval_then_revocation_keeps_the_row_but_drops_trust() {
        let database = Database::open_in_memory().unwrap();
        let repository = ReceiverRepository::new(database.connection());
        repository.upsert(&receiver("r1")).unwrap();

        repository.approve("r1", 2_000).unwrap();
        let approved = repository.get("r1").unwrap().unwrap();
        assert!(approved.is_trusted());
        assert_eq!(approved.approved_at, Some(2_000));
        assert_eq!(repository.list_trusted().unwrap().len(), 1);

        repository.revoke("r1", 3_000).unwrap();
        let revoked = repository.get("r1").unwrap().unwrap();
        assert!(!revoked.is_trusted());
        assert_eq!(revoked.pairing_state, "revoked");
        assert_eq!(revoked.revoked_at, Some(3_000));
        assert!(
            repository.list_trusted().unwrap().is_empty(),
            "a revoked receiver must not be trusted"
        );
        assert!(
            repository.get("r1").unwrap().is_some(),
            "the row must be kept so the device cannot return as a stranger"
        );
    }

    #[test]
    fn re_approving_a_revoked_receiver_clears_the_revocation() {
        let database = Database::open_in_memory().unwrap();
        let repository = ReceiverRepository::new(database.connection());
        repository.upsert(&receiver("r1")).unwrap();
        repository.revoke("r1", 1).unwrap();
        repository.approve("r1", 2).unwrap();
        let record = repository.get("r1").unwrap().unwrap();
        assert!(record.is_trusted());
        assert_eq!(record.revoked_at, None);
    }

    #[test]
    fn operating_on_an_unknown_receiver_reports_not_found() {
        let database = Database::open_in_memory().unwrap();
        let repository = ReceiverRepository::new(database.connection());
        assert!(matches!(
            repository.approve("ghost", 1),
            Err(PersistenceError::NotFound { entity: "receiver", .. })
        ));
        assert!(matches!(
            repository.set_display_name("ghost", "x"),
            Err(PersistenceError::NotFound { .. })
        ));
        assert!(repository.get("ghost").unwrap().is_none());
    }

    // -- sessions ----------------------------------------------------------

    #[test]
    fn sessions_round_trip_and_update() {
        let database = Database::open_in_memory().unwrap();
        let repository = SessionRepository::new(database.connection());
        let mut record = session("s1");
        repository.create(&record).unwrap();

        record.state = "streaming".to_owned();
        record.started_at = Some(2_000);
        record.peak_receivers = 4;
        repository.update(&record).unwrap();

        let stored = repository.get("s1").unwrap().unwrap();
        assert_eq!(stored.state, "streaming");
        assert_eq!(stored.peak_receivers, 4);
        assert_eq!(stored.label.as_deref(), Some("Sunday 1st"));
    }

    #[test]
    fn history_is_newest_first_and_paginates() {
        let database = Database::open_in_memory().unwrap();
        let repository = SessionRepository::new(database.connection());
        for index in 0..5 {
            let mut record = session(&format!("s{index}"));
            record.created_at = 1_000 + i64::from(index);
            repository.create(&record).unwrap();
        }
        assert_eq!(repository.count().unwrap(), 5);

        let first_page = repository.history(2, 0).unwrap();
        assert_eq!(first_page.len(), 2);
        assert_eq!(first_page[0].id, "s4", "newest first");
        assert_eq!(first_page[1].id, "s3");

        let second_page = repository.history(2, 2).unwrap();
        assert_eq!(second_page[0].id, "s2");
    }

    #[test]
    fn events_are_returned_newest_first() {
        let database = Database::open_in_memory().unwrap();
        let repository = SessionRepository::new(database.connection());
        repository.create(&session("s1")).unwrap();
        repository
            .append_event("e1", "s1", 1, "session.created", "info", "created", None)
            .unwrap();
        repository
            .append_event(
                "e2",
                "s1",
                2,
                "stream.started",
                "info",
                "streaming",
                Some(r#"{"receivers":0}"#),
            )
            .unwrap();

        let events = repository.events("s1", 10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "e2");
        assert_eq!(events[0].5.as_deref(), Some(r#"{"receivers":0}"#));
    }

    #[test]
    fn join_and_leave_are_recorded_against_the_session() {
        let database = Database::open_in_memory().unwrap();
        ReceiverRepository::new(database.connection())
            .upsert(&receiver("r1"))
            .unwrap();
        let repository = SessionRepository::new(database.connection());
        repository.create(&session("s1")).unwrap();

        repository.record_join("s1", "r1", 100).unwrap();
        repository.record_leave("s1", "r1", 200).unwrap();

        let left_at: Option<i64> = database
            .connection()
            .query_row(
                "SELECT left_at FROM session_receivers WHERE session_id = 's1' AND receiver_id = 'r1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left_at, Some(200));
    }

    #[test]
    fn pruning_respects_the_minimum_retained_count() {
        let database = Database::open_in_memory().unwrap();
        let repository = SessionRepository::new(database.connection());
        for index in 0..10 {
            let mut record = session(&format!("s{index}"));
            record.created_at = i64::from(index);
            repository.create(&record).unwrap();
        }
        // Everything is older than the cutoff, but the 3 newest must survive.
        let removed = repository.prune(1_000_000, 3).unwrap();
        assert_eq!(removed, 7);
        assert_eq!(repository.count().unwrap(), 3);
        assert!(repository.get("s9").unwrap().is_some());
        assert!(repository.get("s0").unwrap().is_none());
    }

    #[test]
    fn updating_a_missing_session_reports_not_found() {
        let database = Database::open_in_memory().unwrap();
        let repository = SessionRepository::new(database.connection());
        assert!(matches!(
            repository.update(&session("ghost")),
            Err(PersistenceError::NotFound { entity: "session", .. })
        ));
    }
}
