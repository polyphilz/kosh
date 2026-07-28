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
        embedding_index::{
            InstallEmbeddingDisposition, PassageEmbeddingIndexProgress, PassageEmbeddingIndexState,
            RECONCILIATION_BATCH_SIZE,
        },
        DatabaseClient,
    },
    embedding_runtime::{EmbeddingRuntime, SemanticRuntimePhase},
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PassageEmbeddingIndexPhase {
    WaitingForRuntime,
    Indexing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassageEmbeddingIndexStatus {
    pub phase: PassageEmbeddingIndexPhase,
    pub embedding_index_id: String,
    pub index_key: String,
    pub indexed_passages: i64,
    pub total_passages: i64,
    pub active: bool,
    pub message: Option<String>,
}

pub(crate) struct PassageEmbeddingIndexer {
    shutdown: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
    runtime: Option<Arc<EmbeddingRuntime>>,
    startup_error: Option<String>,
}

impl PassageEmbeddingIndexer {
    pub(crate) fn start(database: DatabaseClient, runtime: Arc<EmbeddingRuntime>) -> Self {
        let (shutdown, receiver) = mpsc::channel();
        let worker_runtime = Arc::clone(&runtime);
        match thread::Builder::new()
            .name("kosh-passage-embedding-indexer".into())
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
                    "could not start the passage embedding indexer: {error}"
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
        progress: PassageEmbeddingIndexProgress,
        runtime_phase: SemanticRuntimePhase,
        runtime_message: Option<String>,
    ) -> PassageEmbeddingIndexStatus {
        let complete = progress.active
            && progress.indexed_passages == progress.total_passages
            && progress.state == PassageEmbeddingIndexState::Idle;
        let message = self
            .startup_error
            .clone()
            .or(progress.error)
            .or(runtime_message);
        let phase = if self.startup_error.is_some()
            || progress.state == PassageEmbeddingIndexState::Failed
        {
            PassageEmbeddingIndexPhase::Failed
        } else if complete {
            PassageEmbeddingIndexPhase::Ready
        } else if runtime_phase == SemanticRuntimePhase::Ready {
            PassageEmbeddingIndexPhase::Indexing
        } else {
            PassageEmbeddingIndexPhase::WaitingForRuntime
        };
        PassageEmbeddingIndexStatus {
            phase,
            embedding_index_id: progress.embedding_index_id,
            index_key: progress.index_key,
            indexed_passages: progress.indexed_passages,
            total_passages: progress.total_passages,
            active: progress.active,
            message,
        }
    }
}

impl Drop for PassageEmbeddingIndexer {
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
                log::error!("passage embedding indexer panicked during shutdown: {error:?}");
            }
        }
    }
}

fn worker_loop(database: DatabaseClient, runtime: Arc<EmbeddingRuntime>, shutdown: &Receiver<()>) {
    loop {
        if runtime.status().phase == SemanticRuntimePhase::Ready {
            match database.passage_embedding_index_needs_reconciliation() {
                Ok(true) => {
                    if let Err(error) = reconcile_batch(&database, &runtime, shutdown) {
                        let _ = database
                            .record_passage_embedding_index_failure(error, timestamp_now_ms());
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = database.record_passage_embedding_index_failure(
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
        .load_embedding_reconciliation_batch(RECONCILIATION_BATCH_SIZE)
        .map_err(|error| error.to_string())?;
    if pending.is_empty() {
        database
            .activate_passage_embedding_index_if_complete(timestamp_now_ms())
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    for passage in pending {
        if shutdown.try_recv().is_ok() {
            return Ok(());
        }
        let embedding = runtime
            .embed_document(&passage.content)
            .map_err(|error| error.public_message())?;
        let disposition = database
            .install_passage_embedding(passage, embedding, timestamp_now_ms())
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
