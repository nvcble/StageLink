use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tracing::{debug, info};

use crate::error::{PersistenceError, Result};
use crate::migrations::{MIGRATIONS, latest_version};

/// Schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// An open StageLink database.
///
/// Not `Clone` and not internally synchronised: the service owns exactly one and
/// serialises access through its control task. That is a deliberate constraint
/// rather than an oversight — it makes it impossible to accidentally touch the
/// database from the audio path.
pub struct Database {
    connection: Connection,
    path: PathBuf,
}

impl Database {
    /// Opens (creating if needed) the database at `path` and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| PersistenceError::DataDirectory {
                path: parent.display().to_string(),
                source,
            })?;
        }

        let connection = Connection::open(&path)?;
        let mut database = Self { connection, path };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    /// Opens an in-memory database. Used by tests and by the `--ephemeral`
    /// service flag.
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        let mut database = Self { connection, path: PathBuf::from(":memory:") };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    fn configure(&mut self) -> Result<()> {
        // WAL keeps readers from blocking the writer, which matters because the
        // service writes session events while the UI is reading history.
        // `NORMAL` synchronous is the right trade with WAL: durable against
        // process crashes, which is what we care about, without an fsync per
        // commit during a live service.
        self.connection.pragma_update(None, "journal_mode", "WAL")?;
        self.connection.pragma_update(None, "synchronous", "NORMAL")?;
        self.connection.pragma_update(None, "foreign_keys", true)?;
        // A statement must not hang forever if a stale reader holds a lock.
        self.connection
            .busy_timeout(core::time::Duration::from_secs(5))?;
        Ok(())
    }

    /// Reads the schema version from the SQLite header.
    pub fn schema_version(&self) -> Result<u32> {
        let version: i64 =
            self.connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(version.max(0) as u32)
    }

    /// Applies any migrations this database has not seen.
    ///
    /// Refuses to touch a database written by a newer build: silently running
    /// an old binary against a new schema is how data gets destroyed.
    pub fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version()?;
        let target = latest_version();

        if current > target {
            return Err(PersistenceError::SchemaTooNew { found: current, supported: target });
        }
        if current == target {
            debug!(version = current, "database schema is current");
            return Ok(());
        }

        info!(from = current, to = target, "migrating database schema");

        for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
            // Each migration is its own transaction, so an interruption leaves
            // the database at a clean, known version.
            let transaction = self.connection.transaction()?;
            transaction
                .execute_batch(migration.sql)
                .map_err(|source| PersistenceError::Migration {
                    version: migration.version,
                    name: migration.name,
                    source,
                })?;
            transaction.pragma_update(None, "user_version", migration.version)?;
            transaction.commit()?;
            info!(version = migration.version, name = migration.name, "migration applied");
        }

        Ok(())
    }

    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    #[must_use]
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a consistent copy of the database to `destination`, safe to run
    /// while the service is live. Used by the "export diagnostics" action.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<()> {
        self.connection
            .backup(rusqlite::MAIN_DB, destination, None)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_exists(database: &Database, name: &str) -> bool {
        database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    }

    #[test]
    fn a_fresh_database_is_migrated_to_the_latest_version() {
        let database = Database::open_in_memory().unwrap();
        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn the_initial_migration_creates_every_table() {
        let database = Database::open_in_memory().unwrap();
        for table in [
            "settings",
            "receivers",
            "sessions",
            "session_events",
            "session_receivers",
        ] {
            assert!(table_exists(&database, table), "missing table {table}");
        }
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut database = Database::open_in_memory().unwrap();
        database.migrate().unwrap();
        database.migrate().unwrap();
        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let database = Database::open_in_memory().unwrap();
        // Inserting an event for a session that does not exist must fail,
        // otherwise orphaned rows accumulate silently.
        let result = database.connection().execute(
            "INSERT INTO session_events (id, session_id, at, kind, severity, message)
             VALUES ('e1', 'no-such-session', 0, 'error.raised', 'error', 'x')",
            [],
        );
        assert!(result.is_err(), "foreign keys must be enforced");
    }

    #[test]
    fn deleting_a_session_cascades_to_its_events() {
        let database = Database::open_in_memory().unwrap();
        let connection = database.connection();
        connection
            .execute(
                "INSERT INTO sessions (id, state, created_at) VALUES ('s1', 'idle', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_events (id, session_id, at, kind, severity, message)
                 VALUES ('e1', 's1', 1, 'session.created', 'info', 'created')",
                [],
            )
            .unwrap();
        connection
            .execute("DELETE FROM sessions WHERE id = 's1'", [])
            .unwrap();
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn refuses_to_open_a_database_from_a_newer_build() {
        let mut database = Database::open_in_memory().unwrap();
        // Pretend a future StageLink wrote this file.
        database
            .connection()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .unwrap();
        match database.migrate() {
            Err(PersistenceError::SchemaTooNew { found, supported }) => {
                assert_eq!(found, SCHEMA_VERSION + 5);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
    }

    #[test]
    fn opening_a_file_creates_missing_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("deeper").join("stagelink.db");
        let database = Database::open(&path).unwrap();
        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(path.exists());
    }

    #[test]
    fn a_database_survives_being_closed_and_reopened() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("stagelink.db");
        {
            let database = Database::open(&path).unwrap();
            database
                .connection()
                .execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES ('audio', '{}', 1)",
                    [],
                )
                .unwrap();
        }
        let reopened = Database::open(&path).unwrap();
        let count: i64 = reopened
            .connection()
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "settings must survive a restart");
        assert_eq!(reopened.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn backup_produces_a_readable_copy() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open_in_memory().unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('audio', '{\"a\":1}', 1)",
                [],
            )
            .unwrap();

        let backup_path = temp.path().join("backup.db");
        database.backup_to(&backup_path).unwrap();

        let restored = Connection::open(&backup_path).unwrap();
        let value: String = restored
            .query_row("SELECT value FROM settings WHERE key = 'audio'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "{\"a\":1}");
    }
}
