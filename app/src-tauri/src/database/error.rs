use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("database I/O failed: {0}")]
    Io(#[from] io::Error),

    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration operation failed: {0}")]
    Migration(#[from] refinery::Error),

    #[error("database pair is incomplete: main is {main_state}, media is {media_state}")]
    IncompletePair {
        main_state: &'static str,
        media_state: &'static str,
    },

    #[error("Kosh library at {path} is already open by another writer")]
    DatabaseInUse { path: PathBuf },

    #[error(
        "{kind} database at {path} has application_id {actual:#010x}; expected {expected:#010x}"
    )]
    WrongApplicationId {
        kind: &'static str,
        path: PathBuf,
        expected: i32,
        actual: i32,
    },

    #[error("{kind} database migration history is incompatible: {reason}")]
    IncompatibleMigrationHistory { kind: &'static str, reason: String },

    #[error("{kind} database validation failed: {reason}")]
    Validation { kind: &'static str, reason: String },

    #[error("database writer is unavailable")]
    WriterUnavailable,

    #[error("invalid database command: {0}")]
    InvalidInput(String),

    #[error("{entity} {id} was not found")]
    NotFound { entity: &'static str, id: String },

    #[error(
        "tidbit {id} changed before this operation: current revision is {actual_revision_id}, expected {expected_revision_id}"
    )]
    StaleTidbit {
        id: String,
        expected_revision_id: String,
        actual_revision_id: String,
    },

    #[error("tidbit {id} is deleted")]
    TidbitDeleted { id: String },

    #[error("media blob is still referenced by {references} attachment(s)")]
    MediaInUse { references: i64 },

    #[error("database writer failed to shut down cleanly")]
    WriterPanicked,
}

pub type Result<T> = std::result::Result<T, DatabaseError>;
