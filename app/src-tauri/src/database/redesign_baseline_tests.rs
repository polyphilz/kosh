use std::fs;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::backup::domain::BackupSetId;

use super::{
    CitationLocator, CitationState, Database, DatabasePaths, LexicalSearchMode, SearchPassagesInput,
};

const FIXTURE_JSON: &str = include_str!("fixtures/redesign-baseline-v1.json");
const MAIN_PROFILE: &[u8] = include_bytes!("fixtures/redesign-profile/main-v23.sqlite3");
const MEDIA_PROFILE: &[u8] = include_bytes!("fixtures/redesign-profile/media-v2.sqlite3");
const MAIN_PROFILE_SHA256: &str =
    "7c69408868df733caf84b9baa4d749ae8496403e2de81ebc1e4d5d65b81d78f6";
const MEDIA_PROFILE_SHA256: &str =
    "1cc1976a2df34b24bd5681227b3b61b56720a0339ca75dcaa898a304766a2678";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    schema_version: u32,
    active_note: ActiveNoteFixture,
    media: MediaFixture,
    saved_draft: SavedDraftFixture,
    deleted_note: DeletedNoteFixture,
    settings: SettingsFixture,
    backup: BackupFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveNoteFixture {
    id: String,
    initial_revision_id: String,
    current_revision_id: String,
    title: String,
    current_marker: String,
    source_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaFixture {
    image: MediaItemFixture,
    pdf: MediaItemFixture,
    text: MediaItemFixture,
    opaque: MediaItemFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaItemFixture {
    attachment_id: String,
    ocr_marker: Option<String>,
    page_marker: Option<String>,
    line_marker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedDraftFixture {
    context_key: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletedNoteFixture {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFixture {
    quick_add: String,
    main_window: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupFixture {
    backup_set_id: BackupSetId,
}

fn rich_body(fixture: &Fixture, marker: &str) -> String {
    format!(
        "# Arrays and vectors\n\n\
         - contiguous storage\n  - nested cache detail\n\n\
         1. infer the dtype\n2. inspect the shape\n\n\
         **Bold**, *italic*, ~~obsolete~~, and `ndarray`.\n\n\
         ```python\na = np.array([1, 2, 3])\n```\n\n\
         Inline math $a_i$ and display math:\n\n$$\\sum_i a_i$$\n\n\
         {marker}\n\n\
         {{{{kosh:image:{};width=70%;alt=Vector%20board}}}}\n\n\
         {{{{kosh:pdf:{}}}}}\n\n\
         {{{{kosh:attachment:{};caption=Vector%20scraps}}}}\n\n\
         {{{{kosh:attachment:{}}}}}",
        fixture.media.image.attachment_id,
        fixture.media.pdf.attachment_id,
        fixture.media.text.attachment_id,
        fixture.media.opaque.attachment_id,
    )
}

fn exact_result(database: &Database, query: &str) -> super::PassageSearchResult {
    let results = database
        .client()
        .search_passages(SearchPassagesInput {
            query: query.into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("search deterministic baseline");
    assert_eq!(results.len(), 1, "expected one result for {query:?}");
    results.into_iter().next().expect("single result")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn redesign_baseline_fixture_upgrades_legacy_content_media_and_provenance() {
    let fixture: Fixture = serde_json::from_str(FIXTURE_JSON).expect("parse baseline fixture");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(sha256_hex(MAIN_PROFILE), MAIN_PROFILE_SHA256);
    assert_eq!(sha256_hex(MEDIA_PROFILE), MEDIA_PROFILE_SHA256);

    let root = tempfile::tempdir().expect("temporary redesign baseline");
    let paths = DatabasePaths::new(root.path());
    fs::write(&paths.main, MAIN_PROFILE).expect("materialize frozen main database");
    fs::write(&paths.media, MEDIA_PROFILE).expect("materialize frozen media database");

    let frozen_main = rusqlite::Connection::open(&paths.main).expect("open frozen main database");
    let frozen_media =
        rusqlite::Connection::open(&paths.media).expect("open frozen media database");
    assert_eq!(
        frozen_main
            .query_row(
                "SELECT max(version) FROM refinery_schema_history",
                [],
                |row| { row.get::<_, i32>(0) }
            )
            .expect("frozen main migration head"),
        23
    );
    assert_eq!(
        frozen_media
            .query_row(
                "SELECT max(version) FROM refinery_schema_history",
                [],
                |row| { row.get::<_, i32>(0) }
            )
            .expect("frozen media migration head"),
        2
    );
    drop(frozen_main);
    drop(frozen_media);

    for launch in 0..2 {
        let database =
            Database::initialize(paths.clone()).expect("upgrade frozen baseline profile");
        let client = database.client();
        let loaded = client
            .load_tidbit(fixture.active_note.id.clone())
            .expect("load active baseline note");
        assert_eq!(
            loaded.current_revision_id,
            fixture.active_note.current_revision_id
        );
        assert_eq!(
            loaded.title.as_deref(),
            Some(fixture.active_note.title.as_str())
        );
        assert_eq!(
            loaded.body_markdown,
            rich_body(&fixture, &fixture.active_note.current_marker)
        );
        assert_eq!(
            client
                .load_draft(fixture.saved_draft.context_key.clone())
                .expect("load baseline saved draft")
                .expect("baseline saved draft")
                .body_markdown,
            fixture.saved_draft.body
        );

        let main = database
            .open_main_read_only()
            .expect("read frozen baseline state");
        assert!(main
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM tidbit WHERE id = ?1",
                [&fixture.deleted_note.id],
                |row| row.get::<_, bool>(0),
            )
            .expect("baseline deleted note state"));
        let historical_passage = main
            .query_row(
                "SELECT id FROM passage WHERE tidbit_revision_id = ?1 ORDER BY ordinal LIMIT 1",
                [&fixture.active_note.initial_revision_id],
                |row| row.get::<_, String>(0),
            )
            .expect("historical baseline passage");
        assert_eq!(
            client
                .resolve_citation(historical_passage)
                .expect("resolve historical citation")
                .state,
            CitationState::Historical
        );

        let authored = exact_result(&database, &fixture.active_note.current_marker);
        assert_eq!(authored.citation.state, CitationState::Current);
        assert_eq!(
            authored.citation.sources[0].url.as_deref(),
            Some(fixture.active_note.source_url.as_str())
        );
        let image = exact_result(
            &database,
            fixture
                .media
                .image
                .ocr_marker
                .as_deref()
                .expect("image marker"),
        );
        assert!(matches!(
            image.citation.locator,
            CitationLocator::OcrRegion { .. }
        ));
        let pdf = exact_result(
            &database,
            fixture
                .media
                .pdf
                .page_marker
                .as_deref()
                .expect("PDF marker"),
        );
        assert_eq!(pdf.citation.locator, CitationLocator::PdfPage { page: 1 });
        let text = exact_result(
            &database,
            fixture
                .media
                .text
                .line_marker
                .as_deref()
                .expect("text marker"),
        );
        assert_eq!(
            text.citation.locator,
            CitationLocator::TextLines {
                start_line: 1,
                end_line: 3,
            }
        );
        assert!(client
            .search_passages(SearchPassagesInput {
                query: "opaque baseline bytes".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("opaque bytes are not indexed")
            .is_empty());
        assert_eq!(
            client
                .load_generic_attachment_status(fixture.media.opaque.attachment_id.clone())
                .expect("load opaque attachment status")
                .extracted_line_count,
            0
        );
        assert_eq!(
            database
                .open_media_read_only()
                .expect("read frozen media database")
                .query_row("SELECT count(*) FROM media_blob", [], |row| row
                    .get::<_, u32>(0))
                .expect("frozen media blobs"),
            5
        );

        let settings = client
            .load_shortcut_settings()
            .expect("load baseline settings");
        assert_eq!(
            settings.keyboard_bindings[0].accelerator,
            fixture.settings.quick_add
        );
        assert_eq!(
            settings.keyboard_bindings[1].accelerator,
            fixture.settings.main_window
        );
        let backup = client
            .load_offsite_backup_config()
            .expect("load baseline backup config")
            .expect("baseline backup config");
        assert!(!backup.enabled);
        assert_eq!(backup.backup_set_id, fixture.backup.backup_set_id);
        database
            .shutdown()
            .unwrap_or_else(|error| panic!("shutdown launch {launch}: {error}"));
    }
}
