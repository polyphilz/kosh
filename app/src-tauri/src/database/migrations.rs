use refinery::{Migration, Runner};
use rusqlite::{Connection, OptionalExtension};

use super::{
    connection::DatabaseKind,
    error::{DatabaseError, Result},
};

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
    // enforcement is enabled. Refinery still wraps the checksummed migration
    // set in one transaction; validation runs immediately after enforcement
    // is restored.
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let migration = main_runner().run(connection).map_err(DatabaseError::from);
    let restore = connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(DatabaseError::from);
    migration?;
    restore
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
