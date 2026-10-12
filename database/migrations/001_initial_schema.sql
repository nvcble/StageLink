-- StageLink schema, migration 001: initial_schema
--
-- REFERENCE COPY. The authoritative migration lives in
-- crates/persistence/src/migrations.rs and is compiled into the service; this
-- file is extracted from it so the schema can be read and diffed without
-- reading Rust. It is verified against the source by
-- tests/protocol (migration-reference.test.ts).
--
-- Applied automatically on first run. SQLite user_version tracks progress, and
-- each migration runs in its own transaction so an interrupted upgrade leaves
-- the database at a clean, known version.
-- Key/value application settings. One row per top-level settings group, each
-- holding validated JSON. Storing groups rather than individual keys keeps the
-- read path to a single query and lets the service validate a whole group with
-- one schema.
CREATE TABLE settings (
    key         TEXT    PRIMARY KEY NOT NULL,
    value       TEXT    NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

-- Receivers the operator has seen. `pairing_state` drives authorisation; a
-- revoked row is kept rather than deleted so the same device cannot silently
-- come back as a brand-new unknown receiver.
CREATE TABLE receivers (
    id                 TEXT    PRIMARY KEY NOT NULL,
    device_name        TEXT    NOT NULL,
    display_name       TEXT,
    platform           TEXT    NOT NULL,
    platform_version   TEXT,
    app_version        TEXT,
    -- Long-term public key, hex encoded. NOT a secret; the matching private
    -- key never leaves the device, and our own keys live in the OS credential
    -- store, not here.
    public_key         TEXT,
    key_fingerprint    TEXT,
    pairing_state      TEXT    NOT NULL,
    last_ip_address    TEXT,
    first_seen_at      INTEGER NOT NULL,
    last_seen_at       INTEGER NOT NULL,
    approved_at        INTEGER,
    revoked_at         INTEGER
) STRICT;

CREATE INDEX idx_receivers_pairing_state ON receivers (pairing_state);
CREATE INDEX idx_receivers_last_seen ON receivers (last_seen_at DESC);

-- One row per session (one service or rehearsal).
CREATE TABLE sessions (
    id               TEXT    PRIMARY KEY NOT NULL,
    label            TEXT,
    state            TEXT    NOT NULL,
    created_at       INTEGER NOT NULL,
    started_at       INTEGER,
    ended_at         INTEGER,
    duration_ms      INTEGER NOT NULL DEFAULT 0,
    peak_receivers   INTEGER NOT NULL DEFAULT 0,
    total_receivers  INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT
) STRICT;

CREATE INDEX idx_sessions_created ON sessions (created_at DESC);

-- Operational event log, scoped to a session. Deleted with its session so the
-- database cannot grow without bound.
CREATE TABLE session_events (
    id          TEXT    PRIMARY KEY NOT NULL,
    session_id  TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    at          INTEGER NOT NULL,
    kind        TEXT    NOT NULL,
    severity    TEXT    NOT NULL,
    message     TEXT    NOT NULL,
    data        TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
) STRICT;

CREATE INDEX idx_session_events_session ON session_events (session_id, at DESC);

-- Which receivers took part in which session. Supports the per-receiver
-- session history in the UI.
CREATE TABLE session_receivers (
    session_id   TEXT    NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    receiver_id  TEXT    NOT NULL REFERENCES receivers (id) ON DELETE CASCADE,
    joined_at    INTEGER NOT NULL,
    left_at      INTEGER,
    PRIMARY KEY (session_id, receiver_id, joined_at)
) STRICT;
