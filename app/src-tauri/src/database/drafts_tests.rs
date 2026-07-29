use tempfile::TempDir;

use super::{
    drafts::SaveDraftWrite, tidbits::CreateTidbitWrite, ClearDraftInput, Database, DatabaseError,
    DatabasePaths, SaveDraftInput, SourceDraft, TidbitDraft,
};

struct TestLibrary {
    root: TempDir,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("test database");
        Self { root, database }
    }

    fn save(&self, input: SaveDraftInput, now_ms: i64, draft_id: &str) -> super::Draft {
        self.database
            .client()
            .save_draft(SaveDraftWrite {
                input,
                now_ms,
                draft_id: draft_id.into(),
                media_limits: super::MediaLimits::default(),
            })
            .expect("save draft")
    }
}

#[test]
fn capture_draft_round_trips_partial_input_and_compare_clears() {
    let library = TestLibrary::new();
    let first = library.save(
        SaveDraftInput {
            context_key: "capture".into(),
            tidbit_id: None,
            base_revision_id: None,
            title: Some("  unfinished title  ".into()),
            body_markdown: "half a shower thought".into(),
            sources: vec![
                SourceDraft {
                    label: Some("only a label so far".into()),
                    url: Some(String::new()),
                },
                SourceDraft {
                    label: None,
                    url: None,
                },
            ],
        },
        10,
        "019f547b-6200-7000-8000-000000006001",
    );
    assert_eq!(first.created_at_ms, 10);
    assert_eq!(first.updated_at_ms, 10);
    assert_eq!(first.title.as_deref(), Some("  unfinished title  "));
    assert_eq!(first.sources[0].url.as_deref(), Some(""));
    assert_eq!(first.sources[1].label, None);

    let second = library.save(
        SaveDraftInput {
            context_key: "capture".into(),
            tidbit_id: None,
            base_revision_id: None,
            title: None,
            body_markdown: "finished thought".into(),
            sources: Vec::new(),
        },
        10,
        "019f547b-6200-7000-8000-000000006002",
    );
    assert_eq!(second.id, first.id);
    assert_eq!(second.created_at_ms, first.created_at_ms);
    assert_eq!(second.updated_at_ms, 11);

    assert!(!library
        .database
        .client()
        .clear_draft(ClearDraftInput {
            context_key: "capture".into(),
            expected_updated_at_ms: first.updated_at_ms,
        })
        .expect("stale clear"));
    assert_eq!(
        library
            .database
            .client()
            .load_draft("capture".into())
            .expect("load draft"),
        Some(second.clone())
    );
    assert!(library
        .database
        .client()
        .clear_draft(ClearDraftInput {
            context_key: "capture".into(),
            expected_updated_at_ms: second.updated_at_ms,
        })
        .expect("exact clear"));
    assert_eq!(
        library
            .database
            .client()
            .load_draft("capture".into())
            .expect("load cleared draft"),
        None
    );
}

#[test]
fn draft_survives_clean_shutdown_and_reopen() {
    let library = TestLibrary::new();
    let expected = library.save(
        SaveDraftInput {
            context_key: "capture".into(),
            tidbit_id: None,
            base_revision_id: None,
            title: Some("Recovered".into()),
            body_markdown: "Still here after restart.".into(),
            sources: Vec::new(),
        },
        20,
        "019f547b-6200-7000-8000-000000006003",
    );
    let paths = DatabasePaths::new(library.root.path());
    library.database.shutdown().expect("clean shutdown");

    let reopened = Database::initialize(paths).expect("reopen database");
    assert_eq!(
        reopened
            .client()
            .load_draft("capture".into())
            .expect("load recovered draft"),
        Some(expected)
    );
}

#[test]
fn quick_add_draft_survives_clean_shutdown_and_uses_an_isolated_context() {
    let library = TestLibrary::new();
    let quick_add = library.save(
        SaveDraftInput {
            context_key: "quick-add".into(),
            tidbit_id: None,
            base_revision_id: None,
            title: Some("Captured globally".into()),
            body_markdown: "Still here after the quick window hides.".into(),
            sources: Vec::new(),
        },
        21,
        "019f547b-6200-7000-8000-000000006004",
    );
    let capture = library.save(
        SaveDraftInput {
            context_key: "capture".into(),
            tidbit_id: None,
            base_revision_id: None,
            title: Some("Main window".into()),
            body_markdown: "A separate capture draft.".into(),
            sources: Vec::new(),
        },
        22,
        "019f547b-6200-7000-8000-000000006005",
    );
    let paths = DatabasePaths::new(library.root.path());
    library.database.shutdown().expect("clean shutdown");

    let reopened = Database::initialize(paths).expect("reopen database");
    assert_eq!(
        reopened
            .client()
            .load_draft("quick-add".into())
            .expect("load quick-add draft"),
        Some(quick_add)
    );
    assert_eq!(
        reopened
            .client()
            .load_draft("capture".into())
            .expect("load capture draft"),
        Some(capture)
    );
}

#[test]
fn edit_draft_requires_the_matching_tidbit_revision() {
    let library = TestLibrary::new();
    let tidbit = library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: None,
                body_markdown: "Original".into(),
                sources: Vec::new(),
            },
            now_ms: 30,
            tidbit_id: "019f547b-6200-7000-8000-000000006010".into(),
            revision_id: "019f547b-6200-7000-8000-000000006011".into(),
            source_ids: Vec::new(),
        })
        .expect("create tidbit");
    let context_key = format!("edit:{}", tidbit.id);
    let saved = library.save(
        SaveDraftInput {
            context_key: context_key.clone(),
            tidbit_id: Some(tidbit.id.clone()),
            base_revision_id: Some(tidbit.current_revision_id.clone()),
            title: Some("Editing".into()),
            body_markdown: "Changed".into(),
            sources: Vec::new(),
        },
        31,
        "019f547b-6200-7000-8000-000000006012",
    );
    assert_eq!(saved.context_key, context_key);
    assert_eq!(saved.tidbit_id.as_deref(), Some(tidbit.id.as_str()));

    let error = library
        .database
        .client()
        .save_draft(SaveDraftWrite {
            input: SaveDraftInput {
                context_key: format!("edit:{}", tidbit.id),
                tidbit_id: Some(tidbit.id),
                base_revision_id: Some("019f547b-6200-7000-8000-000000006099".into()),
                title: None,
                body_markdown: "Should not replace the valid draft".into(),
                sources: Vec::new(),
            },
            now_ms: 32,
            draft_id: "019f547b-6200-7000-8000-000000006013".into(),
            media_limits: super::MediaLimits::default(),
        })
        .expect_err("reject unrelated revision");
    assert!(matches!(error, DatabaseError::InvalidInput(_)));
    assert_eq!(
        library
            .database
            .client()
            .load_draft(context_key)
            .expect("load retained draft"),
        Some(saved)
    );
}
