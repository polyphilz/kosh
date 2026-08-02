use std::{
    io::Cursor,
    sync::{Arc, Barrier},
    thread,
};

use super::{
    AttachmentIngestInput, Database, DatabasePaths, LexicalSearchMode, MediaLimits,
    SearchPassagesInput, SourceDraft, TidbitDraft,
};

const CAPTURE_THREADS: usize = 4;
const NOTES_PER_CAPTURE_THREAD: usize = 25;
const SEARCH_THREADS: usize = 2;
const SEARCHES_PER_THREAD: usize = 50;
const ATTACHMENTS: usize = 10;
const DRAFT_ID: &str = "019f547b-6200-7000-8000-00000000f001";

#[test]
fn mixed_local_workload_survives_contention_integrity_scan_and_restart() {
    let root = tempfile::tempdir().expect("mixed workload root");
    let paths = DatabasePaths::new(root.path());
    let database = Arc::new(Database::initialize(paths.clone()).expect("database"));
    let setup_client = database.client();
    setup_client
        .save_working_copy_for_test(DRAFT_ID.into(), None, 1, String::new(), Vec::new(), 1, true)
        .expect("attachment working copy");

    let worker_count = CAPTURE_THREADS + SEARCH_THREADS + 2;
    let start = Arc::new(Barrier::new(worker_count));
    let mut workers = Vec::new();

    for capture_thread in 0..CAPTURE_THREADS {
        let client = database.client();
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            start.wait();
            for note in 0..NOTES_PER_CAPTURE_THREAD {
                let ordinal = capture_thread * NOTES_PER_CAPTURE_THREAD + note;
                client
                    .create_tidbit_with_ids(
                        TidbitDraft {
                            body_markdown: format!(
                                "Concurrent stress evidence {ordinal:03}.\n\n```text\nworker={capture_thread}\n```"
                            ),
                            sources: vec![SourceDraft {
                                label: Some("Reliability fixture".into()),
                                url: Some(format!(
                                    "https://example.invalid/reliability/{ordinal:03}"
                                )),
                            }],
                        },
                        10_000 + ordinal as i64,
                        uuid::Uuid::now_v7().to_string(),
                        uuid::Uuid::now_v7().to_string(),
                        vec![uuid::Uuid::now_v7().to_string()],
                    )
                    .expect("concurrent capture");
            }
        }));
    }

    for _ in 0..SEARCH_THREADS {
        let client = database.client();
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            start.wait();
            for _ in 0..SEARCHES_PER_THREAD {
                client
                    .search_passages(SearchPassagesInput {
                        query: "concurrent stress evidence".into(),
                        mode: LexicalSearchMode::Default,
                        limit: 10,
                    })
                    .expect("concurrent lexical search");
            }
        }));
    }

    {
        let database = Arc::clone(&database);
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            start.wait();
            for ordinal in 0..ATTACHMENTS {
                database
                    .ingest_attachment(
                        AttachmentIngestInput {
                            draft_id: DRAFT_ID.into(),
                            display_filename: format!("stress-{ordinal:02}.txt"),
                            media_type: "text/plain".into(),
                            now_ms: 20_000 + ordinal as i64,
                            limits: MediaLimits::default(),
                        },
                        Cursor::new(
                            format!("attachment extraction evidence {ordinal:02}").into_bytes(),
                        ),
                    )
                    .expect("concurrent attachment extraction");
            }
        }));
    }

    {
        let client = database.client();
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            start.wait();
            for ordinal in 0..6 {
                client.rebuild_search().expect("concurrent search rebuild");
                client
                    .rebuild_embeddings(40_000 + ordinal)
                    .expect("concurrent embedding invalidation");
            }
        }));
    }

    for worker in workers {
        worker.join().expect("mixed workload worker");
    }

    setup_client.full_integrity_check().expect("live integrity");
    let before = setup_client
        .maintenance_snapshot()
        .expect("live diagnostics");
    assert_eq!(
        before.active_tidbits,
        (CAPTURE_THREADS * NOTES_PER_CAPTURE_THREAD) as u64
    );
    assert_eq!(before.attachments, ATTACHMENTS as u64);

    let exact = setup_client
        .search_passages(SearchPassagesInput {
            query: "\"Concurrent stress evidence 042\"".into(),
            mode: LexicalSearchMode::Exact,
            limit: 10,
        })
        .expect("exact result after contention");
    assert_eq!(exact.len(), 1);
    let passage_id = exact[0].passage_id.clone();
    let revision_id = exact[0]
        .citation
        .tidbit
        .as_ref()
        .expect("authored citation")
        .revision_id
        .clone();
    assert_eq!(
        exact[0].citation.sources[0].url.as_deref(),
        Some("https://example.invalid/reliability/042")
    );

    database.shutdown().expect("mixed workload shutdown");
    drop(setup_client);
    drop(database);
    let reopened = Database::initialize(paths).expect("mixed workload restart");
    let reopened_client = reopened.client();
    reopened_client
        .full_integrity_check()
        .expect("restart integrity");
    assert_eq!(
        reopened_client
            .resolve_citation(passage_id)
            .expect("restart citation")
            .tidbit
            .expect("restart authored citation")
            .revision_id,
        revision_id
    );
    assert_eq!(
        reopened_client
            .maintenance_snapshot()
            .expect("restart diagnostics")
            .active_tidbits,
        (CAPTURE_THREADS * NOTES_PER_CAPTURE_THREAD) as u64
    );
}
