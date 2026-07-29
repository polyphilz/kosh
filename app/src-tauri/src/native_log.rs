use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use log::{LevelFilter, Log, Metadata, Record};
use serde::Serialize;

pub(crate) const MAX_LOG_FILE_BYTES: u64 = 512 * 1024;
pub(crate) const MAX_LOG_FILES: usize = 3;
const LOG_FILENAME: &str = "kosh.log";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeLogDiagnostics {
    pub paths: Vec<String>,
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub disk_usage_bytes: u64,
}

pub(crate) fn install(data_root: &Path) -> io::Result<()> {
    let logger = NativeLogger {
        sink: Mutex::new(RotatingLog::open(&log_directory(data_root))?),
    };
    log::set_boxed_logger(Box::new(logger))
        .map_err(|error| io::Error::other(format!("could not install native logger: {error}")))?;
    log::set_max_level(LevelFilter::Info);
    log::info!("native logging initialized");
    Ok(())
}

pub(crate) fn diagnostics(data_root: &Path) -> io::Result<NativeLogDiagnostics> {
    let paths = log_paths(&log_directory(data_root));
    let disk_usage_bytes = paths.iter().try_fold(0_u64, |total, path| {
        let bytes = match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) => 0,
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        Ok::<_, io::Error>(total.saturating_add(bytes))
    })?;
    Ok(NativeLogDiagnostics {
        paths: paths
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        max_file_bytes: MAX_LOG_FILE_BYTES,
        max_files: MAX_LOG_FILES,
        disk_usage_bytes,
    })
}

fn log_directory(data_root: &Path) -> PathBuf {
    data_root.join("logs")
}

fn log_paths(directory: &Path) -> Vec<PathBuf> {
    (0..MAX_LOG_FILES)
        .map(|index| {
            if index == 0 {
                directory.join(LOG_FILENAME)
            } else {
                directory.join(format!("{LOG_FILENAME}.{index}"))
            }
        })
        .collect()
}

struct NativeLogger {
    sink: Mutex<RotatingLog>,
}

impl Log for NativeLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let mut line = format!(
            "{timestamp_ms} {} {}: {}\n",
            record.level(),
            record.target(),
            record.args()
        );
        bound_line(&mut line);
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = sink.write(line.as_bytes());
    }

    fn flush(&self) {
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = sink.file.flush();
    }
}

fn bound_line(line: &mut String) {
    if line.len() as u64 <= MAX_LOG_FILE_BYTES {
        return;
    }
    let mut boundary = (MAX_LOG_FILE_BYTES - 1) as usize;
    while !line.is_char_boundary(boundary) {
        boundary -= 1;
    }
    line.truncate(boundary);
    line.push('\n');
}

struct RotatingLog {
    directory: PathBuf,
    file: File,
    bytes_written: u64,
}

impl RotatingLog {
    fn open(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let current = directory.join(LOG_FILENAME);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current)?;
        let bytes_written = file.metadata()?.len();
        Ok(Self {
            directory: directory.to_owned(),
            file,
            bytes_written,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.bytes_written > 0
            && self.bytes_written.saturating_add(bytes.len() as u64) > MAX_LOG_FILE_BYTES
        {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.bytes_written = self.bytes_written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        for index in (1..MAX_LOG_FILES).rev() {
            let destination = self.directory.join(format!("{LOG_FILENAME}.{index}"));
            let source = if index == 1 {
                self.directory.join(LOG_FILENAME)
            } else {
                self.directory.join(format!("{LOG_FILENAME}.{}", index - 1))
            };
            match fs::remove_file(&destination) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            match fs::rename(&source, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.directory.join(LOG_FILENAME))?;
        self.bytes_written = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{bound_line, diagnostics, RotatingLog, MAX_LOG_FILES, MAX_LOG_FILE_BYTES};

    #[test]
    fn oversized_unicode_log_lines_are_truncated_on_a_character_boundary() {
        let mut line = "🗒️".repeat(MAX_LOG_FILE_BYTES as usize);
        bound_line(&mut line);
        assert!(line.len() as u64 <= MAX_LOG_FILE_BYTES);
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn rotating_log_remains_bounded_and_reports_exact_paths() {
        let root = tempfile::tempdir().expect("temporary root");
        let directory = root.path().join("logs");
        let mut log = RotatingLog::open(&directory).expect("open log");
        let payload = vec![b'x'; 300 * 1024];
        for _ in 0..8 {
            log.write(&payload).expect("write log");
        }
        drop(log);

        let report = diagnostics(root.path()).expect("diagnostics");
        assert_eq!(report.paths.len(), MAX_LOG_FILES);
    }

    #[test]
    fn rotation_keeps_only_the_configured_number_of_files() {
        let root = tempfile::tempdir().expect("temporary root");
        let log_directory = root.path().join("logs");
        let mut log = RotatingLog::open(&log_directory).expect("open log");
        let payload = vec![b'x'; 300 * 1024];
        for _ in 0..8 {
            log.write(&payload).expect("write log");
        }
        drop(log);

        let files = std::fs::read_dir(&log_directory)
            .expect("read logs")
            .collect::<Result<Vec<_>, _>>()
            .expect("log entries");
        assert_eq!(files.len(), MAX_LOG_FILES);
        assert!(files
            .iter()
            .all(|entry| entry.metadata().expect("metadata").len() <= 512 * 1024));
        let report = diagnostics(root.path()).expect("diagnostics");
        assert_eq!(
            report.disk_usage_bytes,
            files
                .iter()
                .map(|entry| entry.metadata().expect("metadata").len())
                .sum::<u64>()
        );
    }
}
