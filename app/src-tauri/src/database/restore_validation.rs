//! Descriptor-bound validation helpers for clean-directory recovery.

use std::{fs::File, os::fd::AsRawFd, path::PathBuf};

use super::{
    connection::{self, DatabaseKind, FileState},
    migrations, safety_snapshot, validation, DatabaseError, Result,
};

pub(crate) fn validate_pair_at(main_file: &File, media_file: &File) -> Result<()> {
    let mut main =
        connection::open_bound_writer(main_file, DatabaseKind::Main, FileState::Existing)?;
    let mut media =
        connection::open_bound_writer(media_file, DatabaseKind::Media, FileState::Existing)?;
    let main_status = migrations::inspect_main(&mut main)?;
    let media_status = migrations::inspect_media(&mut media)?;
    let expected = migrations::expected_heads();
    if main_status.pending
        || media_status.pending
        || main_status.head != expected.main
        || media_status.head != expected.media
    {
        return Err(invalid(&format!(
            "schema heads are ({:?}, {:?}), expected ({:?}, {:?})",
            main_status.head, media_status.head, expected.main, expected.media
        )));
    }
    let main_path = PathBuf::from(format!("/dev/fd/{}", main_file.as_raw_fd()));
    let media_path = PathBuf::from(format!("/dev/fd/{}", media_file.as_raw_fd()));
    validation::validate_migrated_pair(&mut main, &mut media, &main_path, &media_path)?;
    drop(main);
    drop(media);
    main_file.sync_all()?;
    media_file.sync_all()?;
    let main = connection::open_bound_read_only(main_file, DatabaseKind::Main)?;
    let media = connection::open_bound_read_only(media_file, DatabaseKind::Media)?;
    safety_snapshot::verify_recovery_pair_connections(&main, &media)
}

pub(crate) fn create_empty_media_at(file: &File) -> Result<rusqlite::Connection> {
    if file.metadata()?.len() != 0 {
        return Err(invalid("restore media destination is not empty"));
    }
    let mut media = connection::open_bound_writer(file, DatabaseKind::Media, FileState::Fresh)?;
    migrations::run_media(&mut media)?;
    Ok(media)
}

pub(crate) fn open_main_read_only_at(file: &File) -> Result<rusqlite::Connection> {
    connection::open_bound_read_only(file, DatabaseKind::Main)
}

fn invalid(reason: &str) -> DatabaseError {
    DatabaseError::Validation {
        kind: "restore",
        reason: reason.into(),
    }
}
