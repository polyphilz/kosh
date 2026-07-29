use rusqlite::params;
use tempfile::TempDir;

use super::{
    tidbits::{CreateTidbitWrite, EditTidbitWrite, TidbitListScope, TIDBIT_PURGE_DELAY_MS},
    Database, DatabaseError, DatabasePaths, DeleteTidbitInput, EditTidbitInput,
    ListTidbitRevisionsInput, ListTidbitsInput, PurgeTidbitInput, SourceDraft, Tidbit, TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("test database");
        Self {
            _root: root,
            database,
        }
    }

    fn create(
        &self,
        tidbit_id: &str,
        revision_id: &str,
        now_ms: i64,
        draft: TidbitDraft,
        source_ids: &[&str],
    ) -> Tidbit {
        self.database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: draft,
                now_ms,
                tidbit_id: tidbit_id.into(),
                revision_id: revision_id.into(),
                source_ids: source_ids.iter().map(|id| (*id).into()).collect(),
            })
            .expect("create tidbit")
    }
}

#[test]
fn shower_thought_and_long_markdown_round_trip_exactly() {
    let library = TestLibrary::new();
    let body = "\n# A shower thought\n\nHeat is just impatient motion.  \n";
    let created = library.create(
        "019f547b-6200-7000-8000-000000001001",
        "019f547b-6200-7000-8000-000000001002",
        10,
        TidbitDraft {
            title: Some("   ".into()),
            body_markdown: body.into(),
            sources: vec![
                SourceDraft {
                    label: Some("  Thermodynamics notes  ".into()),
                    url: None,
                },
                SourceDraft {
                    label: None,
                    url: Some("HTTPS://Example.COM:443/chapter?q=heat#section-2".into()),
                },
            ],
        },
        &[
            "019f547b-6200-7000-8000-000000001003",
            "019f547b-6200-7000-8000-000000001004",
        ],
    );

    assert_eq!(created.title, None);
    assert_eq!(created.display_title, "A shower thought");
    assert_eq!(created.body_markdown, body);
    assert_eq!(
        created.sources[0].label.as_deref(),
        Some("Thermodynamics notes")
    );
    assert_eq!(
        created.sources[1].url.as_deref(),
        Some("https://example.com/chapter?q=heat")
    );
    assert_eq!(
        library
            .database
            .client()
            .load_tidbit(created.id.clone())
            .expect("load created tidbit"),
        created
    );

    let long_body = format!(
        "# Chapter notes\n\n```rust\nfn answer() -> i32 {{ 42 }}\n```\n\n$$E = mc^2$$\n\n{}",
        "A precise observation. ".repeat(1_000)
    );
    let edited = library
        .database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id,
                title: Some("Chapter 7".into()),
                body_markdown: long_body.clone(),
                sources: Vec::new(),
            },
            now_ms: 11,
            revision_id: "019f547b-6200-7000-8000-000000001005".into(),
            source_ids: Vec::new(),
        })
        .expect("edit long tidbit");
    assert_eq!(edited.body_markdown, long_body);
    assert_eq!(edited.display_title, "Chapter 7");

    let main = library
        .database
        .open_main_read_only()
        .expect("read-only main");
    let persisted_title: Option<String> = main
        .query_row(
            "SELECT title
             FROM tidbit_revision
             WHERE id = '019f547b-6200-7000-8000-000000001002'",
            [],
            |row| row.get(0),
        )
        .expect("authored title");
    assert_eq!(persisted_title, None);
}

#[test]
fn edits_preserve_history_and_reject_stale_revision_tokens() {
    let library = TestLibrary::new();
    let created = library.create(
        "019f547b-6200-7000-8000-000000001101",
        "019f547b-6200-7000-8000-000000001102",
        20,
        TidbitDraft {
            title: None,
            body_markdown: "Original body".into(),
            sources: vec![
                SourceDraft {
                    label: Some("First".into()),
                    url: Some("https://example.com/first".into()),
                },
                SourceDraft {
                    label: Some("Second".into()),
                    url: None,
                },
            ],
        },
        &[
            "019f547b-6200-7000-8000-000000001103",
            "019f547b-6200-7000-8000-000000001104",
        ],
    );
    let edited = library
        .database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id.clone(),
                title: Some("Revised".into()),
                body_markdown: "Revised body".into(),
                sources: vec![
                    SourceDraft {
                        label: Some("Second".into()),
                        url: None,
                    },
                    SourceDraft {
                        label: Some("Third".into()),
                        url: Some("https://example.com/third".into()),
                    },
                ],
            },
            now_ms: 20,
            revision_id: "019f547b-6200-7000-8000-000000001105".into(),
            source_ids: vec![
                "019f547b-6200-7000-8000-000000001106".into(),
                "019f547b-6200-7000-8000-000000001107".into(),
            ],
        })
        .expect("edit current revision");
    assert_eq!(edited.updated_at_ms, 21);
    assert_eq!(edited.revision_number, 2);
    assert_eq!(
        edited
            .sources
            .iter()
            .map(|source| source.label.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Second"), Some("Third")]
    );

    let stale = library
        .database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id,
                title: None,
                body_markdown: "Lost update".into(),
                sources: Vec::new(),
            },
            now_ms: 22,
            revision_id: "019f547b-6200-7000-8000-000000001108".into(),
            source_ids: Vec::new(),
        })
        .expect_err("stale editor");
    assert!(matches!(stale, DatabaseError::StaleTidbit { .. }));

    let main = library
        .database
        .open_main_read_only()
        .expect("read-only main");
    let revision_count: i64 = main
        .query_row(
            "SELECT count(*) FROM tidbit_revision WHERE tidbit_id = ?1",
            params![created.id],
            |row| row.get(0),
        )
        .expect("revision count");
    assert_eq!(revision_count, 2);
    let original: (String, String) = main
        .query_row(
            "SELECT revision.body_markdown, group_concat(source.label, ',')
             FROM tidbit_revision AS revision
             JOIN tidbit_revision_source AS membership
               ON membership.tidbit_revision_id = revision.id
             JOIN source ON source.id = membership.source_id
             WHERE revision.id = '019f547b-6200-7000-8000-000000001102'
             ORDER BY membership.sort_order",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("historical revision");
    assert_eq!(original, ("Original body".into(), "First,Second".into()));
}

#[test]
fn active_listing_is_bounded_cursor_paginated_and_excludes_deleted_tidbits() {
    let library = TestLibrary::new();
    let records = [
        (
            "019f547b-6200-7000-8000-000000001201",
            "019f547b-6200-7000-8000-000000001202",
            10,
            "Oldest",
        ),
        (
            "019f547b-6200-7000-8000-000000001203",
            "019f547b-6200-7000-8000-000000001204",
            20,
            "Middle",
        ),
        (
            "019f547b-6200-7000-8000-000000001205",
            "019f547b-6200-7000-8000-000000001206",
            30,
            "Newest",
        ),
    ]
    .map(|(tidbit_id, revision_id, now_ms, body)| {
        library.create(
            tidbit_id,
            revision_id,
            now_ms,
            TidbitDraft {
                title: None,
                body_markdown: body.into(),
                sources: Vec::new(),
            },
            &[],
        )
    });

    let first = library
        .database
        .client()
        .list_tidbits(ListTidbitsInput {
            limit: 2,
            cursor: None,
            scope: super::tidbits::TidbitListScope::Active,
        })
        .expect("first page");
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.display_title.as_str())
            .collect::<Vec<_>>(),
        vec!["Newest", "Middle"]
    );
    let second = library
        .database
        .client()
        .list_tidbits(ListTidbitsInput {
            limit: 2,
            cursor: first.next_cursor,
            scope: super::tidbits::TidbitListScope::Active,
        })
        .expect("second page");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].display_title, "Oldest");
    assert_eq!(second.next_cursor, None);

    let deleted = library
        .database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: records[1].id.clone(),
                expected_revision_id: records[1].current_revision_id.clone(),
            },
            40,
        )
        .expect("soft delete");
    assert_eq!(deleted.deleted_at_ms, Some(40));
    assert_eq!(
        library
            .database
            .client()
            .load_tidbit(deleted.id.clone())
            .expect("deleted tidbit remains resolvable")
            .deleted_at_ms,
        Some(40)
    );
    let active = library
        .database
        .client()
        .list_tidbits(ListTidbitsInput {
            limit: 100,
            cursor: None,
            scope: super::tidbits::TidbitListScope::Active,
        })
        .expect("active list");
    assert_eq!(active.items.len(), 2);
    assert!(active.items.iter().all(|item| item.id != deleted.id));
}

#[test]
fn unsafe_source_urls_are_rejected_without_partial_authored_data() {
    let library = TestLibrary::new();
    let error = library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: None,
                body_markdown: "Unsafe provenance".into(),
                sources: vec![SourceDraft {
                    label: Some("Do not open".into()),
                    url: Some("javascript:alert(1)".into()),
                }],
            },
            now_ms: 10,
            tidbit_id: "019f547b-6200-7000-8000-000000001301".into(),
            revision_id: "019f547b-6200-7000-8000-000000001302".into(),
            source_ids: vec!["019f547b-6200-7000-8000-000000001303".into()],
        })
        .expect_err("unsafe URL");
    assert!(matches!(error, DatabaseError::InvalidInput(_)));

    let duplicate = library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: None,
                body_markdown: "Duplicate provenance".into(),
                sources: vec![
                    SourceDraft {
                        label: Some("Reference".into()),
                        url: Some("HTTPS://Example.COM:443/page#first".into()),
                    },
                    SourceDraft {
                        label: Some(" Reference ".into()),
                        url: Some("https://example.com/page#second".into()),
                    },
                ],
            },
            now_ms: 10,
            tidbit_id: "019f547b-6200-7000-8000-000000001304".into(),
            revision_id: "019f547b-6200-7000-8000-000000001305".into(),
            source_ids: vec![
                "019f547b-6200-7000-8000-000000001306".into(),
                "019f547b-6200-7000-8000-000000001307".into(),
            ],
        })
        .expect_err("duplicate normalized sources");
    assert!(matches!(duplicate, DatabaseError::InvalidInput(_)));

    let main = library
        .database
        .open_main_read_only()
        .expect("read-only main");
    let tidbit_count: i64 = main
        .query_row("SELECT count(*) FROM tidbit", [], |row| row.get(0))
        .expect("tidbit count");
    assert_eq!(tidbit_count, 0);
}

#[test]
fn library_history_and_trash_are_paginated_with_exact_revision_detail() {
    let library = TestLibrary::new();
    let created = library.create(
        "019f547b-6200-7000-8000-000000001401",
        "019f547b-6200-7000-8000-000000001402",
        100,
        TidbitDraft {
            title: Some("Library lifecycle".into()),
            body_markdown: "First immutable body".into(),
            sources: vec![SourceDraft {
                label: Some("Primary source".into()),
                url: Some("https://example.com/library#ignored".into()),
            }],
        },
        &["019f547b-6200-7000-8000-000000001403"],
    );
    let edited = library
        .database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id.clone(),
                title: Some("Library lifecycle revised".into()),
                body_markdown: "Second immutable body".into(),
                sources: Vec::new(),
            },
            now_ms: 101,
            revision_id: "019f547b-6200-7000-8000-000000001404".into(),
            source_ids: Vec::new(),
        })
        .expect("edit tidbit");

    let first = library
        .database
        .client()
        .list_tidbit_revisions(ListTidbitRevisionsInput {
            tidbit_id: created.id.clone(),
            limit: 1,
            before_revision_number: None,
        })
        .expect("first history page");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].revision_number, 2);
    assert!(first.items[0].is_current);
    let second = library
        .database
        .client()
        .list_tidbit_revisions(ListTidbitRevisionsInput {
            tidbit_id: created.id.clone(),
            limit: 1,
            before_revision_number: first.next_before_revision_number,
        })
        .expect("second history page");
    assert_eq!(second.items[0].revision_number, 1);
    assert_eq!(second.next_before_revision_number, None);
    let original = library
        .database
        .client()
        .load_tidbit_revision(created.id.clone(), created.current_revision_id.clone())
        .expect("load exact historical revision");
    assert_eq!(original.body_markdown, "First immutable body");
    assert!(!original.is_current);
    assert_eq!(
        original.sources[0].url.as_deref(),
        Some("https://example.com/library")
    );
    assert_eq!(
        library
            .database
            .client()
            .load_source_url(original.sources[0].id.clone())
            .expect("load trusted source URL"),
        "https://example.com/library"
    );

    let deleted = library
        .database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: created.id,
                expected_revision_id: edited.current_revision_id,
            },
            200,
        )
        .expect("soft delete");
    let trash = library
        .database
        .client()
        .list_tidbits(ListTidbitsInput {
            limit: 10,
            cursor: None,
            scope: TidbitListScope::Deleted,
        })
        .expect("trash page");
    assert_eq!(trash.items.len(), 1);
    assert_eq!(trash.items[0].deleted_at_ms, deleted.deleted_at_ms);
    assert_eq!(
        trash.items[0].purge_eligible_at_ms,
        Some(deleted.deleted_at_ms.expect("deleted timestamp") + TIDBIT_PURGE_DELAY_MS)
    );
}

#[test]
fn permanent_purge_is_delayed_transactional_and_removes_authored_history() {
    let library = TestLibrary::new();
    let created = library.create(
        "019f547b-6200-7000-8000-000000001501",
        "019f547b-6200-7000-8000-000000001502",
        100,
        TidbitDraft {
            title: Some("Purge me".into()),
            body_markdown: "Private authored content".into(),
            sources: vec![SourceDraft {
                label: Some("Private source".into()),
                url: Some("https://example.com/private".into()),
            }],
        },
        &["019f547b-6200-7000-8000-000000001503"],
    );
    let source_id = created.sources[0].id.clone();
    let deleted = library
        .database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id.clone(),
            },
            200,
        )
        .expect("soft delete");
    let too_early = library
        .database
        .client()
        .purge_tidbit(
            PurgeTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id.clone(),
            },
            deleted.deleted_at_ms.expect("deleted timestamp") + TIDBIT_PURGE_DELAY_MS - 1,
        )
        .expect_err("purge must honor grace period");
    assert!(too_early
        .to_string()
        .contains("cannot be permanently deleted"));
    assert!(library
        .database
        .client()
        .load_tidbit(created.id.clone())
        .is_ok());
    let direct =
        rusqlite::Connection::open(&library.database.paths().main).expect("direct invariant probe");
    direct
        .pragma_update(None, "foreign_keys", "ON")
        .expect("foreign keys");
    assert!(direct
        .execute(
            "INSERT INTO tidbit_purge_authorization(
                tidbit_id, expected_revision_id, authorized_at
             ) VALUES(?1, ?2, ?3)",
            params![
                created.id,
                created.current_revision_id,
                deleted.deleted_at_ms.expect("deleted timestamp") + TIDBIT_PURGE_DELAY_MS - 1
            ],
        )
        .is_err());
    assert!(direct
        .execute(
            "DELETE FROM tidbit_revision_source WHERE tidbit_revision_id = ?1",
            params![created.current_revision_id],
        )
        .is_err());
    direct
        .execute(
            "INSERT INTO tidbit_purge_authorization(
                tidbit_id, expected_revision_id, authorized_at
             ) VALUES(?1, ?2, ?3)",
            params![
                created.id,
                created.current_revision_id,
                deleted.deleted_at_ms.expect("deleted timestamp") + TIDBIT_PURGE_DELAY_MS
            ],
        )
        .expect("eligible direct authorization");
    assert!(direct
        .execute(
            "UPDATE tidbit_purge_authorization
             SET expected_revision_id = ?2
             WHERE tidbit_id = ?1",
            params![created.id, "019f547b-6200-7000-8000-000000001599"],
        )
        .is_err());
    direct
        .execute(
            "UPDATE tidbit SET deleted_at = NULL WHERE id = ?1",
            params![created.id],
        )
        .expect("simulate a restored tidbit with retained authorization");
    assert_eq!(
        direct
            .query_row(
                "SELECT count(*) FROM tidbit_purge_authorization WHERE tidbit_id = ?1",
                params![created.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("count retained authorizations"),
        0
    );
    assert!(direct
        .execute(
            "DELETE FROM tidbit_revision_source WHERE tidbit_revision_id = ?1",
            params![created.current_revision_id],
        )
        .is_err());
    direct
        .execute(
            "UPDATE tidbit SET deleted_at = ?2 WHERE id = ?1",
            params![created.id, deleted.deleted_at_ms],
        )
        .expect("restore deleted state for writer purge");
    drop(direct);

    assert!(library
        .database
        .client()
        .purge_tidbit(
            PurgeTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id,
            },
            deleted.deleted_at_ms.expect("deleted timestamp") + TIDBIT_PURGE_DELAY_MS,
        )
        .expect("eligible permanent purge"));
    assert!(matches!(
        library.database.client().load_tidbit(created.id),
        Err(DatabaseError::NotFound { .. })
    ));
    assert!(matches!(
        library.database.client().load_source_url(source_id),
        Err(DatabaseError::NotFound { .. })
    ));
    library
        .database
        .client()
        .full_integrity_check()
        .expect("purged database remains valid");
}
