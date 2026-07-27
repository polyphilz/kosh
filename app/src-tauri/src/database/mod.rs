mod connection;
mod error;
mod migrations;
mod paths;
mod validation;
mod writer;

#[cfg(test)]
mod tests;

use std::{
    fs,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    thread::{self, JoinHandle},
};

use rusqlite::Connection;

pub use error::{DatabaseError, Result};
pub use paths::DatabasePaths;
use writer::WriterMessage;
pub use writer::{DatabaseClient, DatabaseDiagnostics};

use connection::DatabaseKind;

#[derive(Debug)]
pub struct Database {
    paths: DatabasePaths,
    client: DatabaseClient,
    writer_thread: Mutex<Option<JoinHandle<()>>>,
}

impl Database {
    pub fn initialize(paths: DatabasePaths) -> Result<Self> {
        fs::create_dir_all(paths.root())?;

        let main_state = connection::inspect_file(&paths.main)?;
        let media_state = connection::inspect_file(&paths.media)?;
        if main_state != media_state {
            return Err(DatabaseError::IncompletePair {
                main_state: main_state.label(),
                media_state: media_state.label(),
            });
        }

        let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, main_state)?;
        let mut media = connection::open_writer(&paths.media, DatabaseKind::Media, media_state)?;
        let main_status = migrations::inspect_main(&mut main)?;
        let media_status = migrations::inspect_media(&mut media)?;
        // Grouped migrations are transactional within each file. Cross-file work is
        // ordered media-first so a crash can leave only an orphaned media capability,
        // never authored metadata pointing at bytes that were not committed.
        if media_status.pending {
            migrations::run_media(&mut media)?;
        }
        if main_status.pending {
            migrations::run_main(&mut main)?;
        }
        validation::validate_migrated_pair(&mut main, &mut media, &paths.main, &paths.media)?;

        let (sender, receiver) = mpsc::channel();
        let client = DatabaseClient::new(sender);
        let writer_thread = thread::Builder::new()
            .name("kosh-database-writer".into())
            .spawn(move || writer_loop(main, media, receiver))?;

        Ok(Self {
            paths,
            client,
            writer_thread: Mutex::new(Some(writer_thread)),
        })
    }

    pub fn paths(&self) -> &DatabasePaths {
        &self.paths
    }

    pub fn client(&self) -> DatabaseClient {
        self.client.clone()
    }

    pub fn open_main_read_only(&self) -> Result<Connection> {
        connection::open_read_only(&self.paths.main, DatabaseKind::Main)
    }

    pub fn open_media_read_only(&self) -> Result<Connection> {
        connection::open_read_only(&self.paths.media, DatabaseKind::Media)
    }

    pub fn shutdown(&self) -> Result<()> {
        let thread = self
            .writer_thread
            .lock()
            .map_err(|_| DatabaseError::WriterPanicked)?
            .take();
        if let Some(thread) = thread {
            self.client.shutdown()?;
            thread.join().map_err(|_| DatabaseError::WriterPanicked)?;
        }
        Ok(())
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.client.shutdown();
        if let Ok(thread) = self.writer_thread.get_mut() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn writer_loop(mut main: Connection, mut media: Connection, receiver: Receiver<WriterMessage>) {
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Diagnostics { reply } => {
                let _ = reply.send(writer::diagnostics(&mut main, &mut media));
            }
            WriterMessage::Shutdown => break,
        }
    }
}
