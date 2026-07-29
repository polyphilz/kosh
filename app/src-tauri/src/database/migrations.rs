use refinery::{Migration, Runner, Target};
use rusqlite::{params, Connection, OptionalExtension};

use super::{
    connection::DatabaseKind,
    error::{DatabaseError, Result},
    passages,
};
use crate::research::{grounded_citation, GroundedResearchCitation};

mod main_embedded {
    use refinery::embed_migrations;
    embed_migrations!("./src/database/migrations/main");
}

mod media_embedded {
    use refinery::embed_migrations;
    embed_migrations!("./src/database/migrations/media");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct MigrationHeads {
    pub main: Option<i32>,
    pub media: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct MigrationStatus {
    pub head: Option<i32>,
    pub pending: bool,
}

pub fn main_runner() -> Runner {
    main_embedded::migrations::runner()
        .set_grouped(true)
        .set_abort_divergent(true)
        .set_abort_missing(true)
}

pub fn media_runner() -> Runner {
    media_embedded::migrations::runner()
        .set_grouped(true)
        .set_abort_divergent(true)
        .set_abort_missing(true)
}

pub fn inspect_main(connection: &mut Connection) -> Result<MigrationStatus> {
    inspect(connection, DatabaseKind::Main, &main_runner())
}

pub fn inspect_media(connection: &mut Connection) -> Result<MigrationStatus> {
    inspect(connection, DatabaseKind::Media, &media_runner())
}

pub fn run_main(connection: &mut Connection) -> Result<()> {
    // SQLite cannot rebuild a referenced parent table while foreign-key
    // enforcement is enabled. Refinery wraps each checksummed migration run in
    // a transaction; validation runs immediately after enforcement is restored.
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let migration = run_main_with_legacy_research_snapshot(connection);
    let restore = connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(DatabaseError::from);
    migration?;
    restore
}

fn run_main_with_legacy_research_snapshot(connection: &mut Connection) -> Result<()> {
    if legacy_research_citation_count(connection)? > 0 {
        // Citation snapshots need V11's complete passage provenance. Older
        // databases advance to that released boundary first; a crash there is
        // safe because the next launch repeats this read-only snapshot step.
        if inspect_main(connection)?.head.unwrap_or_default() < 11 {
            main_runner()
                .set_target(Target::Version(11))
                .run(connection)?;
        }
        snapshot_legacy_research_citations(connection)?;
    }
    main_runner().run(connection)?;
    Ok(())
}

fn legacy_research_citation_count(connection: &Connection) -> Result<i64> {
    let exists = connection
        .query_row(
            "SELECT 1
             FROM sqlite_schema
             WHERE type = 'table' AND name = 'research_citation'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(0);
    }
    Ok(
        connection.query_row("SELECT count(*) FROM research_citation", [], |row| {
            row.get(0)
        })?,
    )
}

fn snapshot_legacy_research_citations(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS legacy_research_citation_snapshot (
            run_id TEXT PRIMARY KEY,
            citations_json TEXT NOT NULL
                CHECK (
                    json_valid(citations_json)
                    AND json_type(citations_json) = 'array'
                )
         ) STRICT;",
    )?;
    let citation_rows = {
        let mut statement = transaction.prepare(
            "SELECT research_run_id, passage_id, cited_text
             FROM research_citation
             ORDER BY research_run_id, ordinal",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let mut current_run_id: Option<String> = None;
    let mut citations = Vec::<GroundedResearchCitation>::new();
    for (run_id, passage_id, cited_text) in citation_rows {
        if let Some(previous_run_id) = current_run_id
            .as_deref()
            .filter(|current| *current != run_id)
        {
            persist_legacy_research_snapshot(&transaction, previous_run_id, &citations)?;
            citations.clear();
        }
        current_run_id = Some(run_id);
        let mut evidence = passages::resolve_citation(&transaction, &passage_id)?;
        // V1 stored the exact cited excerpt separately from the containing
        // passage. Preserve that narrower text while retaining the passage
        // identity, locator, owner, sources, and historical state.
        evidence.excerpt = cited_text;
        let number = u32::try_from(citations.len() + 1).map_err(|_| DatabaseError::Validation {
            kind: "main",
            reason: "legacy research answer has too many citations".into(),
        })?;
        citations.push(grounded_citation(number, evidence));
    }
    if let Some(run_id) = current_run_id {
        persist_legacy_research_snapshot(&transaction, &run_id, &citations)?;
    }
    transaction.commit()?;
    Ok(())
}

fn persist_legacy_research_snapshot(
    connection: &Connection,
    run_id: &str,
    citations: &[GroundedResearchCitation],
) -> Result<()> {
    let citations_json =
        serde_json::to_string(citations).map_err(|error| DatabaseError::Validation {
            kind: "main",
            reason: format!("legacy research citations could not be serialized: {error}"),
        })?;
    connection.execute(
        "INSERT INTO legacy_research_citation_snapshot(run_id, citations_json)
         VALUES(?1, ?2)
         ON CONFLICT(run_id) DO UPDATE SET citations_json = excluded.citations_json",
        params![run_id, citations_json],
    )?;
    Ok(())
}

pub fn run_media(connection: &mut Connection) -> Result<()> {
    media_runner().run(connection)?;
    Ok(())
}

pub fn current_heads(main: &mut Connection, media: &mut Connection) -> Result<MigrationHeads> {
    Ok(MigrationHeads {
        main: inspect_main(main)?.head,
        media: inspect_media(media)?.head,
    })
}

pub fn expected_heads() -> MigrationHeads {
    MigrationHeads {
        main: latest_version(&main_runner()),
        media: latest_version(&media_runner()),
    }
}

fn inspect(
    connection: &mut Connection,
    kind: DatabaseKind,
    runner: &Runner,
) -> Result<MigrationStatus> {
    let history_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema
             WHERE type = 'table' AND name = 'refinery_schema_history'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let applied = if history_exists {
        runner.get_applied_migrations(connection)?
    } else {
        Vec::new()
    };
    validate_history(kind, runner.get_migrations(), &applied)?;
    let head = applied.last().map(Migration::version);
    Ok(MigrationStatus {
        head,
        pending: applied.len() < runner.get_migrations().len(),
    })
}

fn validate_history(
    kind: DatabaseKind,
    available: &[Migration],
    applied: &[Migration],
) -> Result<()> {
    if applied.len() > available.len() {
        return incompatible(
            kind,
            format!(
                "database has {} migrations but this binary knows {}",
                applied.len(),
                available.len()
            ),
        );
    }

    let mut available = available.iter().collect::<Vec<_>>();
    available.sort_by_key(|migration| migration.version());
    for (position, applied_migration) in applied.iter().enumerate() {
        let expected = available[position];
        if applied_migration.version() != expected.version()
            || applied_migration.name() != expected.name()
            || applied_migration.checksum() != expected.checksum()
        {
            return incompatible(
                kind,
                format!(
                    "applied {} does not match embedded {} at position {}",
                    applied_migration,
                    expected,
                    position + 1
                ),
            );
        }
    }
    Ok(())
}

fn incompatible<T>(kind: DatabaseKind, reason: String) -> Result<T> {
    Err(DatabaseError::IncompatibleMigrationHistory {
        kind: kind.label(),
        reason,
    })
}

fn latest_version(runner: &Runner) -> Option<i32> {
    runner.get_migrations().iter().map(Migration::version).max()
}
