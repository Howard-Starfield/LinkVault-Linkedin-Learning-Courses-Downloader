//! Shared runtime state for newspaper command coordination.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

pub struct NewspaperState {
    pub(super) db_path: PathBuf,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) running: AtomicBool,
    library_revision: AtomicU64,
    pub(super) dimension_backfill_running: Arc<AtomicBool>,
}

impl NewspaperState {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            cancelled: Arc::new(AtomicBool::new(false)),
            running: AtomicBool::new(false),
            library_revision: AtomicU64::new(1),
            dimension_backfill_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn library_revision(&self) -> u64 {
        self.library_revision.load(Ordering::SeqCst)
    }

    pub fn invalidate_library(&self) -> u64 {
        self.library_revision.fetch_add(1, Ordering::SeqCst) + 1
    }
}
