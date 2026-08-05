use std::{
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    database::{
        block_embedding_index::{
            BlockEmbeddingIndexProgress, BlockEmbeddingIndexState, InstallEmbeddingDisposition,
            RECONCILIATION_BATCH_SIZE,
        },
        DatabaseClient,
    },
    embedding_runtime::{EmbeddingRuntime, SemanticRuntimePhase},
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BlockEmbeddingIndexPhase {
    WaitingForRuntime,
    Indexing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockEmbeddingIndexStatus {
    pub phase: BlockEmbeddingIndexPhase,
    pub embedding_index_id: String,
    pub index_key: String,
    pub indexed_blocks: i64,
    pub total_blocks: i64,
    pub active: bool,
    pub message: Option<String>,
}

pub(crate) struct BlockEmbeddingIndexer {
    shutdown: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
    runtime: Option<Arc<EmbeddingRuntime>>,
    startup_error: Option<String>,
}

impl BlockEmbeddingIndexer {
    pub(crate) fn start(database: DatabaseClient, runtime: Arc<EmbeddingRuntime>) -> Self {
        let (shutdown, receiver) = mpsc::channel();
        let worker_runtime = Arc::clone(&runtime);
        match thread::Builder::new()
            .name("kosh-block-embedding-indexer".into())
            .spawn(move || worker_loop(database, worker_runtime, &receiver))
        {
            Ok(worker) => Self {
                shutdown: Some(shutdown),
                worker: Some(worker),
                runtime: Some(runtime),
                startup_error: None,
            },
            Err(error) => Self {
                shutdown: None,
                worker: None,
                runtime: Some(runtime),
                startup_error: Some(format!(
                    "could not start the block embedding indexer: {error}"
                )),
            },
        }
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn disabled() -> Self {
        Self {
            shutdown: None,
            worker: None,
            runtime: None,
            startup_error: None,
        }
    }

    pub(crate) fn status(
        &self,
        progress: BlockEmbeddingIndexProgress,
        runtime_phase: SemanticRuntimePhase,
        runtime_message: Option<String>,
    ) -> BlockEmbeddingIndexStatus {
        let complete = progress.active
            && progress.indexed_blocks == progress.total_blocks
            && progress.state == BlockEmbeddingIndexState::Idle;
        let message = self
            .startup_error
            .clone()
            .or(progress.error)
            .or(runtime_message);
        let phase =
            if self.startup_error.is_some() || progress.state == BlockEmbeddingIndexState::Failed {
                BlockEmbeddingIndexPhase::Failed
            } else if complete {
                BlockEmbeddingIndexPhase::Ready
            } else if runtime_phase == SemanticRuntimePhase::Ready {
                BlockEmbeddingIndexPhase::Indexing
            } else {
                BlockEmbeddingIndexPhase::WaitingForRuntime
            };
        BlockEmbeddingIndexStatus {
            phase,
            embedding_index_id: progress.embedding_index_id,
            index_key: progress.index_key,
            indexed_blocks: progress.indexed_blocks,
            total_blocks: progress.total_blocks,
            active: progress.active,
            message,
        }
    }
}

impl Drop for BlockEmbeddingIndexer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // Stop the sidecar before joining so an in-flight HTTP request wakes
        // promptly instead of holding application quit until its timeout.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
        if let Some(worker) = self.worker.take() {
            if let Err(error) = worker.join() {
                log::error!("block embedding indexer panicked during shutdown: {error:?}");
            }
        }
    }
}

fn worker_loop(database: DatabaseClient, runtime: Arc<EmbeddingRuntime>, shutdown: &Receiver<()>) {
    loop {
        if runtime.status().phase == SemanticRuntimePhase::Ready {
            match database.block_embedding_index_needs_reconciliation() {
                Ok(true) => {
                    if let Err(error) = reconcile_batch(&database, &runtime, shutdown) {
                        let _ = database
                            .record_block_embedding_index_failure(error, timestamp_now_ms());
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = database.record_block_embedding_index_failure(
                        error.to_string(),
                        timestamp_now_ms(),
                    );
                }
            }
        }
        match shutdown.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn reconcile_batch(
    database: &DatabaseClient,
    runtime: &EmbeddingRuntime,
    shutdown: &Receiver<()>,
) -> Result<(), String> {
    let pending = database
        .load_block_embedding_reconciliation_batch(RECONCILIATION_BATCH_SIZE)
        .map_err(|error| error.to_string())?;
    if pending.is_empty() {
        database
            .activate_block_embedding_index_if_complete(timestamp_now_ms())
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    for block in pending {
        if shutdown.try_recv().is_ok() {
            return Ok(());
        }
        let embedding = runtime
            .embed_document(&block.content)
            .map_err(|error| error.public_message())?;
        let disposition = database
            .install_block_embedding(block, embedding, timestamp_now_ms())
            .map_err(|error| error.to_string())?;
        if disposition == InstallEmbeddingDisposition::Stale {
            continue;
        }
    }
    Ok(())
}

fn timestamp_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
