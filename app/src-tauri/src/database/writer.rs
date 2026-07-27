use std::sync::mpsc::{self, Sender, SyncSender};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::{
    error::{DatabaseError, Result},
    migrations::MigrationHeads,
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnostics {
    pub migration_heads: MigrationHeads,
    pub main_journal_mode: String,
    pub media_journal_mode: String,
    pub main_foreign_keys: bool,
    pub media_foreign_keys: bool,
}

pub(super) enum WriterMessage {
    Diagnostics {
        reply: SyncSender<Result<DatabaseDiagnostics>>,
    },
    FullIntegrityCheck {
        reply: SyncSender<Result<()>>,
    },
    ReapMediaBlob {
        sha256: Vec<u8>,
        now: i64,
        reason: String,
        reply: SyncSender<Result<bool>>,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct DatabaseClient {
    sender: Sender<WriterMessage>,
}

impl DatabaseClient {
    pub(super) fn new(sender: Sender<WriterMessage>) -> Self {
        Self { sender }
    }

    pub fn diagnostics(&self) -> Result<DatabaseDiagnostics> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::Diagnostics { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn reap_media_blob(&self, sha256: Vec<u8>, now: i64, reason: String) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ReapMediaBlob {
                sha256,
                now,
                reason,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn full_integrity_check(&self) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::FullIntegrityCheck { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(super) fn shutdown(&self) -> Result<()> {
        self.sender
            .send(WriterMessage::Shutdown)
            .map_err(|_| DatabaseError::WriterUnavailable)
    }
}

pub(super) fn diagnostics(
    main: &mut Connection,
    media: &mut Connection,
) -> Result<DatabaseDiagnostics> {
    let migration_heads = super::migrations::current_heads(main, media)?;
    let main_journal_mode =
        main.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    let media_journal_mode =
        media.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    let main_foreign_keys =
        main.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;
    let media_foreign_keys =
        media.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;

    Ok(DatabaseDiagnostics {
        migration_heads,
        main_journal_mode,
        media_journal_mode,
        main_foreign_keys,
        media_foreign_keys,
    })
}

pub(super) fn reap_media_blob(
    main: &Connection,
    media: &mut Connection,
    sha256: Vec<u8>,
    now: i64,
    reason: String,
) -> Result<bool> {
    if sha256.len() != 32 {
        return Err(DatabaseError::InvalidInput(
            "media digest must contain 32 bytes".into(),
        ));
    }
    if now < 0 {
        return Err(DatabaseError::InvalidInput(
            "media reap timestamp must not be negative".into(),
        ));
    }
    if reason.trim().is_empty() {
        return Err(DatabaseError::InvalidInput(
            "media reap reason must not be empty".into(),
        ));
    }

    // Both writable connections are owned by this worker, so no attachment
    // mutation can race this reference check and the following media transaction.
    let references: i64 = main.query_row(
        "SELECT count(*) FROM attachment WHERE sha256 = ?1",
        params![&sha256],
        |row| row.get(0),
    )?;
    if references > 0 {
        return Err(DatabaseError::MediaInUse { references });
    }

    let transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists = transaction
        .query_row(
            "SELECT 1 FROM media_blob WHERE sha256 = ?1",
            params![&sha256],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        transaction.rollback()?;
        return Ok(false);
    }

    transaction.execute(
        "DELETE FROM media_blob_reap_authorization WHERE sha256 = ?1",
        params![&sha256],
    )?;
    transaction.execute(
        "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
         VALUES(?1, ?2, ?3)",
        params![&sha256, now, reason],
    )?;
    let deleted =
        transaction.execute("DELETE FROM media_blob WHERE sha256 = ?1", params![&sha256])?;
    transaction.commit()?;
    Ok(deleted == 1)
}
