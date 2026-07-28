use rusqlite::params;
use tempfile::TempDir;

use super::{deterministic_passage_id, CitationLocator, CitationState};
use crate::database::{
    connection::{self, DatabaseKind, FileState},
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    Database, DatabasePaths, DeleteTidbitInput, EditTidbitInput, RestoreTidbitInput, SourceDraft,
    TidbitDraft,
};

struct TestLibrary {
    _root: TempDir,
    database: Database,
}

impl TestLibrary {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary citation library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("citation database");
        Self {
            _root: root,
            database,
        }
    }

    fn active_passage_ids(&self, tidbit_id: &str) -> Vec<String> {
        let connection = self
            .database
            .open_main_read_only()
            .expect("read active passages");
        let mut statement = connection
            .prepare(
                "SELECT passage.id
                 FROM active_passage
                 JOIN passage ON passage.id = active_passage.passage_id
                 WHERE active_passage.tidbit_id = ?1
                 ORDER BY passage.ordinal",
            )
            .expect("active passage query");
        statement
            .query_map(params![tidbit_id], |row| row.get(0))
            .expect("active passages")
            .collect::<Result<Vec<_>, _>>()
            .expect("active passage IDs")
    }
}

#[test]
fn authored_citations_are_deterministic_and_follow_the_tidbit_lifecycle() {
    let library = TestLibrary::new();
    let tidbit_id = "019f547b-6200-7000-8000-000000002001";
    let first_revision_id = "019f547b-6200-7000-8000-000000002002";
    let created = library
        .database
        .client()
        .create_tidbit(CreateTidbitWrite {
            input: TidbitDraft {
                title: None,
                body_markdown:
                    "# Heat\n\nHeat is impatient motion.\n\nTemperature is a distribution.".into(),
                sources: vec![SourceDraft {
                    label: Some("First notebook".into()),
                    url: Some("https://example.com/first".into()),
                }],
            },
            now_ms: 10,
            tidbit_id: tidbit_id.into(),
            revision_id: first_revision_id.into(),
            source_ids: vec!["019f547b-6200-7000-8000-000000002003".into()],
        })
        .expect("create cited tidbit");
    let first_passages = library.active_passage_ids(tidbit_id);
    assert_eq!(first_passages.len(), 2);
    assert_eq!(
        first_passages,
        vec![
            deterministic_passage_id(first_revision_id, 0).expect("first deterministic ID"),
            deterministic_passage_id(first_revision_id, 1).expect("second deterministic ID"),
        ]
    );

    let heading = library
        .database
        .client()
        .resolve_citation(first_passages[0].clone())
        .expect("resolve heading citation");
    assert_eq!(heading.state, CitationState::Current);
    assert_eq!(heading.excerpt, "Heat");
    assert_eq!(heading.sources[0].label.as_deref(), Some("First notebook"));
    assert_eq!(
        heading
            .tidbit
            .as_ref()
            .map(|tidbit| tidbit.revision_id.as_str()),
        Some(first_revision_id)
    );
    let CitationLocator::MarkdownBlocks {
        start_block,
        end_block,
        source_start_byte,
        source_end_byte,
        ..
    } = heading.locator
    else {
        panic!("authored citation needs a Markdown locator");
    };
    assert_eq!((start_block, end_block, source_start_byte), (0, 0, 0));
    assert_eq!(
        &created.body_markdown[source_start_byte as usize..source_end_byte as usize],
        "# Heat\n"
    );

    let edited = library
        .database
        .client()
        .edit_tidbit(EditTidbitWrite {
            input: EditTidbitInput {
                id: created.id.clone(),
                expected_revision_id: created.current_revision_id,
                title: Some("Revised heat".into()),
                body_markdown: "# Heat\n\nA revised observation.".into(),
                sources: vec![SourceDraft {
                    label: Some("Second notebook".into()),
                    url: Some("https://example.com/second".into()),
                }],
            },
            now_ms: 11,
            revision_id: "019f547b-6200-7000-8000-000000002004".into(),
            source_ids: vec!["019f547b-6200-7000-8000-000000002005".into()],
        })
        .expect("edit cited tidbit");
    let second_passages = library.active_passage_ids(tidbit_id);
    assert_eq!(second_passages.len(), 2);
    assert!(first_passages
        .iter()
        .all(|passage_id| !second_passages.contains(passage_id)));

    let historical = library
        .database
        .client()
        .resolve_citation(first_passages[1].clone())
        .expect("resolve historical citation");
    assert_eq!(historical.state, CitationState::Historical);
    assert_eq!(
        historical.sources[0].label.as_deref(),
        Some("First notebook")
    );
    assert_eq!(
        historical.tidbit.as_ref().map(|tidbit| tidbit.deleted),
        Some(false)
    );
    let current = library
        .database
        .client()
        .resolve_citation(second_passages[1].clone())
        .expect("resolve edited citation");
    assert_eq!(current.state, CitationState::Current);
    assert_eq!(current.sources[0].label.as_deref(), Some("Second notebook"));

    let deleted = library
        .database
        .client()
        .delete_tidbit(
            DeleteTidbitInput {
                id: edited.id.clone(),
                expected_revision_id: edited.current_revision_id.clone(),
            },
            12,
        )
        .expect("delete cited tidbit");
    assert!(library.active_passage_ids(tidbit_id).is_empty());
    let deleted_citation = library
        .database
        .client()
        .resolve_citation(second_passages[1].clone())
        .expect("resolve deleted citation");
    assert_eq!(deleted_citation.state, CitationState::Historical);
    assert_eq!(
        deleted_citation
            .tidbit
            .as_ref()
            .map(|tidbit| tidbit.deleted),
        Some(true)
    );

    library
        .database
        .client()
        .restore_tidbit(
            RestoreTidbitInput {
                id: deleted.id,
                expected_revision_id: deleted.current_revision_id,
            },
            13,
        )
        .expect("restore cited tidbit");
    assert_eq!(library.active_passage_ids(tidbit_id), second_passages);
    assert_eq!(
        library
            .database
            .client()
            .resolve_citation(second_passages[1].clone())
            .expect("resolve restored citation")
            .state,
        CitationState::Current
    );
    assert_eq!(
        library
            .database
            .client()
            .resolve_citation(first_passages[1].clone())
            .expect("old revision stays historical")
            .state,
        CitationState::Historical
    );
}

#[test]
fn attachment_citations_resolve_typed_file_and_line_provenance() {
    let root = tempfile::tempdir().expect("temporary attachment citation library");
    let paths = DatabasePaths::new(root.path());
    drop(Database::initialize(paths.clone()).expect("attachment citation database"));
    let connection = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("main writer");
    connection
        .execute_batch(
            "INSERT INTO attachment(
                id, created_at, updated_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(
                '019f547b-6200-7000-8000-000000002101',
                10, 10, zeroblob(32), 'evidence.txt', 'text/plain', 12, 'TEXT', 'READY'
             );
             INSERT INTO attachment_extraction(
                id, attachment_id, extractor, extractor_version, content_hash,
                status, created_at, started_at, completed_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000002102',
                '019f547b-6200-7000-8000-000000002101',
                'text', '1', zeroblob(32), 'READY', 10, 10, 10
             );
             INSERT INTO attachment_segment(
                id, extraction_id, ordinal, locator_kind, line_start, line_end,
                content, content_hash
             ) VALUES(
                '019f547b-6200-7000-8000-000000002103',
                '019f547b-6200-7000-8000-000000002102',
                0, 'TEXT_LINES', 4, 7, 'exact attachment evidence', zeroblob(32)
             );
             INSERT INTO passage(
                id, attachment_segment_id, owner_kind, ordinal, content,
                content_hash, locator_kind, locator_json, created_at,
                construction_version, heading_context_json
             ) VALUES(
                '019f547b-6200-7000-8000-000000002104',
                '019f547b-6200-7000-8000-000000002103',
                'ATTACHMENT', 0, 'exact attachment evidence', zeroblob(32),
                'TEXT_LINES', '{\"start\":4,\"end\":7}', 10,
                'text-lines-v1', '[]'
             );",
        )
        .expect("attachment citation fixture");

    let citation = super::resolve_citation(&connection, "019f547b-6200-7000-8000-000000002104")
        .expect("resolve attachment citation");
    assert_eq!(citation.state, CitationState::Current);
    assert_eq!(citation.excerpt, "exact attachment evidence");
    assert_eq!(citation.sources, Vec::new());
    assert!(citation.tidbit.is_none());
    assert_eq!(
        citation.attachment.as_ref().map(|attachment| {
            (
                attachment.display_filename.as_str(),
                attachment.media_type.as_str(),
            )
        }),
        Some(("evidence.txt", "text/plain"))
    );
    assert_eq!(
        citation.locator,
        CitationLocator::TextLines {
            start_line: 4,
            end_line: 7,
        }
    );
}

#[test]
fn startup_reconciles_preexisting_revisions_and_retains_old_builder_versions() {
    let root = tempfile::tempdir().expect("temporary upgraded library");
    let paths = DatabasePaths::new(root.path());
    drop(Database::initialize(paths.clone()).expect("initial schema"));
    let connection = connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
        .expect("pre-upgrade writer");
    connection
        .execute_batch(
            "BEGIN;
             INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
             VALUES(
                '019f547b-6200-7000-8000-000000002201',
                10, 10, '019f547b-6200-7000-8000-000000002202'
             );
             INSERT INTO tidbit_revision(
                id, tidbit_id, revision_number, created_at, body_markdown, content_hash
             ) VALUES(
                '019f547b-6200-7000-8000-000000002202',
                '019f547b-6200-7000-8000-000000002201',
                1, 10, 'Content authored before passage construction.', zeroblob(32)
             );
             INSERT INTO passage(
                id, tidbit_revision_id, owner_kind, ordinal, content,
                content_hash, locator_kind, locator_json, created_at
             ) VALUES(
                '019f547b-6200-7000-8000-000000002203',
                '019f547b-6200-7000-8000-000000002202',
                'AUTHOR', 0, 'Legacy passage', zeroblob(32),
                'MARKDOWN_BLOCKS', '{\"start\":0,\"end\":0}', 10
             );
             COMMIT;",
        )
        .expect("preexisting revision and legacy passage");
    drop(connection);

    let upgraded = Database::initialize(paths).expect("reconciled library");
    let active_ids = {
        let connection = upgraded
            .open_main_read_only()
            .expect("read reconciled library");
        let mut statement = connection
            .prepare("SELECT passage_id FROM active_passage ORDER BY passage_id")
            .expect("active passage query");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("active rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("active IDs")
    };
    assert_eq!(active_ids.len(), 1);
    let passage_count: i64 = upgraded
        .open_main_read_only()
        .expect("read retained passage versions")
        .query_row(
            "SELECT count(*)
             FROM passage
             WHERE tidbit_revision_id = '019f547b-6200-7000-8000-000000002202'",
            [],
            |row| row.get(0),
        )
        .expect("passage version count");
    assert_eq!(passage_count, 2);
    assert_eq!(
        upgraded
            .client()
            .resolve_citation(active_ids[0].clone())
            .expect("reconciled citation")
            .state,
        CitationState::Current
    );
    assert_eq!(
        upgraded
            .client()
            .resolve_citation("019f547b-6200-7000-8000-000000002203".into())
            .expect("legacy citation remains resolvable")
            .state,
        CitationState::Historical
    );
}
