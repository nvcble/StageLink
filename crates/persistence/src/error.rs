use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("could not create the data directory {path}: {source}")]
    DataDirectory {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("stored JSON for \"{key}\" could not be decoded: {source}")]
    Decode {
        key: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("value could not be encoded for storage: {0}")]
    Encode(#[from] serde_json::Error),

    #[error(
        "this database is at schema version {found}, newer than the {supported} this build \
         understands. Upgrade StageLink Core rather than letting an older build rewrite it."
    )]
    SchemaTooNew { found: u32, supported: u32 },

    #[error("migration {version} ({name}) failed: {source}")]
    Migration {
        version: u32,
        name: &'static str,
        #[source]
        source: rusqlite::Error,
    },

    #[error("no record found for {entity} \"{id}\"")]
    NotFound { entity: &'static str, id: String },

    #[error("secret store unavailable: {0}")]
    SecretStore(String),
}

pub type Result<T> = core::result::Result<T, PersistenceError>;

impl PersistenceError {
    /// Maps onto the control protocol's structured error codes.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "RECEIVER_NOT_FOUND",
            Self::Decode { .. } | Self::Encode(_) => "SETTINGS_INVALID",
            _ => "STORAGE_FAILURE",
        }
    }
}
