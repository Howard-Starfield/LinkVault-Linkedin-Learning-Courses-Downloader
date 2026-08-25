//! Shared runtime state for newspaper command coordination.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use super::models::OptimizationRuntimeStatus;

pub struct NewspaperState {
    pub(super) db_path: PathBuf,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) download_running: AtomicBool,
    pub(super) optimization_running: Arc<AtomicBool>,
    library_revision: AtomicU64,
    progress_revision: AtomicU64,
    optimization_runtime: Mutex<OptimizationRuntimeStatus>,
    pub(super) dimension_backfill_running: Arc<AtomicBool>,
}

impl NewspaperState {
    pub fn new(db_path: PathBuf) -> Self {
        Self::with_cancellation(db_path, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_cancellation(db_path: PathBuf, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            db_path,
            cancelled,
            download_running: AtomicBool::new(false),
            optimization_running: Arc::new(AtomicBool::new(false)),
            library_revision: AtomicU64::new(1),
            progress_revision: AtomicU64::new(1),
            optimization_runtime: Mutex::new(OptimizationRuntimeStatus::default()),
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

    pub fn progress_revision(&self) -> u64 {
        self.progress_revision.load(Ordering::SeqCst)
    }

    pub fn invalidate_progress(&self) -> u64 {
        self.progress_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn optimization_runtime(&self) -> OptimizationRuntimeStatus {
        self.optimization_runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_optimization_runtime(&self, status: OptimizationRuntimeStatus) {
        *self
            .optimization_runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
    }
}
