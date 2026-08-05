use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("database I/O failed: {0}")]
    Io(#[from] io::Error),

    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration operation failed: {0}")]
    Migration(#[from] refinery::Error),

    #[error("database JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("sqlite-vec auto-extension registration failed with SQLite code {0}")]
    VecRegistration(i32),

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

    #[error("invalid off-site backup configuration: {0}")]
    InvalidOffsiteBackupConfig(String),

    #[error("off-site backup configuration changed before this operation")]
    StaleOffsiteBackupConfig,

    #[error("another durable off-site backup operation is still pending")]
    OffsiteBackupOperationPending,

    #[error("off-site backup must be disabled before changing its recovery target")]
    OffsiteBackupMustBeDisabled,

    #[error("the durable off-site backup operation is no longer pending")]
    OffsiteBackupOperationNotFound,

    #[error("invalid off-site checkpoint: {0}")]
    InvalidOffsiteCheckpoint(String),

    #[error("off-site checkpoint changed before this operation")]
    StaleOffsiteCheckpoint,

    #[error("off-site checkpoint media is not completely uploaded")]
    OffsiteCheckpointMediaIncomplete,

    #[error(
        "off-site backup set {backup_set_id} cannot be reused until its queued credential cleanup completes"
    )]
    OffsiteBackupSetPendingCredentialCleanup { backup_set_id: String },

    #[error("credential cleanup for active off-site backup set {backup_set_id} is not authorized")]
    OffsiteCredentialCleanupNotAuthorized { backup_set_id: String },

    #[error("{entity} {id} was not found")]
    NotFound { entity: &'static str, id: String },

    #[error(
        "tidbit {id} changed before this operation: current content version is {actual_content_version_id}, expected {expected_content_version_id}"
    )]
    StaleTidbit {
        id: String,
        expected_content_version_id: String,
        actual_content_version_id: String,
    },

    #[error("tidbit {id} is deleted")]
    TidbitDeleted { id: String },

    #[error("database writer failed to shut down cleanly")]
    WriterPanicked,
}

pub type Result<T> = std::result::Result<T, DatabaseError>;
