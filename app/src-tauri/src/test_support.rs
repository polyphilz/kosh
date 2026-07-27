use std::{path::Path, sync::Arc};

use tempfile::TempDir;

use crate::{
    runtime::{
        deterministic::{FixedClock, SequenceIds},
        RuntimeState,
    },
    with_commands,
};

pub struct TestDataRoot {
    directory: TempDir,
}

impl TestDataRoot {
    pub fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("temporary Kosh data root"),
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}

impl Default for TestDataRoot {
    fn default() -> Self {
        Self::new()
    }
}

pub fn mock_app(
    data_root: &TestDataRoot,
    now_ms: i64,
    request_ids: impl IntoIterator<Item = String>,
) -> tauri::App<tauri::test::MockRuntime> {
    let state = RuntimeState::deterministic(
        data_root.path().to_owned(),
        Arc::new(FixedClock(now_ms)),
        SequenceIds::new(request_ids),
    );

    with_commands(tauri::test::mock_builder())
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock Kosh app")
}
