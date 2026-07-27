use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::database::{Database, DatabaseClient, DatabasePaths};

pub(crate) trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub(crate) trait IdGenerator: Send + Sync {
    fn next_id(&self) -> String;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }
}

struct UuidV7Generator;

impl IdGenerator for UuidV7Generator {
    fn next_id(&self) -> String {
        uuid::Uuid::now_v7().to_string()
    }
}

pub(crate) struct RuntimeState {
    data_dir: PathBuf,
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl RuntimeState {
    pub(crate) fn production(data_dir: PathBuf) -> crate::database::Result<Self> {
        let database = Database::initialize(DatabasePaths::new(&data_dir))?;
        Ok(Self {
            data_dir,
            database: Arc::new(database),
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidV7Generator),
        })
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn deterministic(
        data_dir: PathBuf,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        let database =
            Database::initialize(DatabasePaths::new(&data_dir)).expect("temporary Kosh database");
        Self {
            data_dir,
            database: Arc::new(database),
            clock,
            ids,
        }
    }

    pub(crate) fn database_client(&self) -> DatabaseClient {
        self.database.client()
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    pub(crate) fn next_ids(&self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.ids.next_id()).collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProbe {
    pub data_dir: String,
    pub now_ms: i64,
    pub request_id: String,
}

#[tauri::command]
pub(crate) fn runtime_probe(state: State<'_, RuntimeState>) -> RuntimeProbe {
    RuntimeProbe {
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        now_ms: state.clock.now_ms(),
        request_id: state.ids.next_id(),
    }
}

#[cfg(feature = "test-support")]
pub(crate) mod deterministic {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::{Clock, IdGenerator};

    pub(crate) struct FixedClock(pub(crate) i64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    pub(crate) struct SequenceIds {
        values: Mutex<VecDeque<String>>,
    }

    impl SequenceIds {
        pub(crate) fn new(values: impl IntoIterator<Item = String>) -> Arc<Self> {
            Arc::new(Self {
                values: Mutex::new(values.into_iter().collect()),
            })
        }
    }

    impl IdGenerator for SequenceIds {
        fn next_id(&self) -> String {
            self.values
                .lock()
                .expect("sequence ID mutex poisoned")
                .pop_front()
                .expect("deterministic ID sequence exhausted")
        }
    }
}
