use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabasePaths {
    pub root: PathBuf,
    pub main: PathBuf,
    pub media: PathBuf,
    pub ownership_lock: PathBuf,
}

impl DatabasePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            main: root.join("kosh.sqlite3"),
            media: root.join("media.sqlite3"),
            ownership_lock: root.join("kosh.lock"),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
