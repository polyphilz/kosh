use rusqlite::params;
use tempfile::TempDir;

use super::{
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    Database, DatabaseError, DatabasePaths, DeleteTidbitInput, EditTidbitInput, ListTidbitsInput,
    SourceDraft, Tidbit, TidbitDraft,
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
