//! Startup reconciliation and deferred cleanup for clipping aggregates
//! (specification 02 section 14, RECOVERY-001..005).
//!
//! Recovery runs after application database initialization and before any
//! clipping view claims ready state. It is idempotent: repeated runs reach
//! the same terminal states (AC-PERSIST-009). Diagnostics record only safe
//! IDs, states, operations, elapsed time, and error classes (RECOVERY-005).

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseErrorClass, DatabaseProvider,
};
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;

use super::clipping_assets::{thumbnail_owner, ClippingAssetLayout};
use super::clipping_models::{ClippingAssetState, ClippingError, ClippingErrorCode};
use super::clipping_repository::{self as repository};
use super::clipping_roots::ClippingRootRegistry;

/// Missing canonical bytes recorded by creation recovery.
pub const ASSET_CREATION_INCOMPLETE: &str = "ASSET_CREATION_INCOMPLETE";
/// Orphan age thresholds (RECOVERY-004).
pub const ORPHAN_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
pub const QUARANTINE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Mutation attempts are independently bounded per managed category. Directory
/// enumeration is complete, detached, streaming, and reported honestly
/// (D-031 / RECOVERY-004).
pub const CLEANUP_MUTATION_BUDGET_PER_CATEGORY: usize = 32;

fn write_context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupRecoverySummary {
    pub creating_marked_ready: usize,
    pub creating_marked_missing: usize,
    pub deletions_completed: usize,
    pub failures: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeferredCleanupSummary {
    pub enumerated: usize,
    pub mutations_attempted: usize,
    pub processed: usize,
    pub failures: usize,
    pub max_category_enumerated: usize,
    pub max_category_mutations: usize,
}

impl DeferredCleanupSummary {
    pub(crate) fn add(&mut self, other: Self) {
        self.enumerated = self.enumerated.saturating_add(other.enumerated);
        self.mutations_attempted = self
            .mutations_attempted
            .saturating_add(other.mutations_attempted);
        self.processed = self.processed.saturating_add(other.processed);
        self.failures = self.failures.saturating_add(other.failures);
        self.max_category_enumerated = self
            .max_category_enumerated
            .max(other.max_category_enumerated.max(other.enumerated));
        self.max_category_mutations = self
            .max_category_mutations
            .max(other.max_category_mutations.max(other.mutations_attempted));
    }
}

fn record_recovery_diagnostic(
    diagnostics: &DatabaseDiagnostics,
    operation: &'static str,
    elapsed: Duration,
    failed: bool,
) {
    diagnostics.record(DatabaseDiagnosticInput {
        kind: DatabaseDiagnosticKind::Recovery,
        operation,
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
        elapsed,
        queue_depth: 0,
        outcome: if failed {
            DatabaseDiagnosticOutcome::Error
        } else {
            DatabaseDiagnosticOutcome::Ok
        },
        error_class: failed.then_some(DatabaseErrorClass::Recovery),
    });
}

/// Recover one creating-state clipping to a terminal state (RECOVERY-001).
/// Shared by startup reconciliation and the idempotent create retry path
/// (FR-IDEMPOTENCY-003). Never creates a second row or changes the ID.
pub fn recover_creating_id(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    clipping_id: &str,
    now: i64,
) -> Result<ClippingAssetState, ClippingError> {
    let target = {
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_creating_rows(&connection)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .into_iter()
            .find(|row| row.id == clipping_id)
    };
    let Some(target) = target else {
        // Not (or no longer) creating: report the current public state.
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        return repository::row_state(&connection, clipping_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound));
    };
    ClippingAssetLayout::validate_relative_path_for_id(&target.asset_relative_path, &target.id)?;

    // 1. Canonical final file present and valid -> mark ready.
    if layout
        .verify_canonical_at(
            &target.id,
            &target.asset_relative_path,
            target.asset_byte_count,
            target.asset_pixel_width,
            target.asset_pixel_height,
            &target.asset_checksum_sha256,
        )
        .is_ok()
    {
        mark_ready(writer, &target.id, now)?;
        return Ok(ClippingAssetState::Ready);
    }

    // 2. Complete staging file present and valid -> promote, then mark ready.
    if layout
        .verify_staging(
            &target.id,
            target.asset_byte_count,
            target.asset_pixel_width,
            target.asset_pixel_height,
            &target.asset_checksum_sha256,
        )
        .is_ok()
        && layout
            .promote_staging_to(&target.id, &target.asset_relative_path)
            .is_ok()
        && layout
            .verify_canonical_at(
                &target.id,
                &target.asset_relative_path,
                target.asset_byte_count,
                target.asset_pixel_width,
                target.asset_pixel_height,
                &target.asset_checksum_sha256,
            )
            .is_ok()
    {
        mark_ready(writer, &target.id, now)?;
        return Ok(ClippingAssetState::Ready);
    }

    // 3. Otherwise the asset is unrecoverable; preserve the row as missing.
    let id = target.id.clone();
    let moved = writer
        .execute(
            write_context("clipping_recovery_mark_missing"),
            move |connection| {
                repository::mark_missing_from_creating(
                    connection,
                    &id,
                    ASSET_CREATION_INCOMPLETE,
                    now,
                )
                .map_err(Into::into)
            },
        )
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if !moved {
        return Err(ClippingError::new(ClippingErrorCode::RecoveryFailed));
    }
    layout.discard_staging(&target.id);
    Ok(ClippingAssetState::Missing)
}

fn mark_ready(writer: &DatabaseWriter, clipping_id: &str, now: i64) -> Result<(), ClippingError> {
    let id = clipping_id.to_string();
    let moved = writer
        .execute(
            write_context("clipping_recovery_mark_ready"),
            move |connection| {
                repository::mark_ready_from_creating(connection, &id, now).map_err(Into::into)
            },
        )
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if moved {
        Ok(())
    } else {
        Err(ClippingError::new(ClippingErrorCode::RecoveryFailed))
    }
}

/// Complete one confirmed deletion (RECOVERY-002 / DELETE-STATE-002..004).
pub fn complete_delete_pending_id(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    clipping_id: &str,
) -> Result<(), ClippingError> {
    let target = {
        let connection = open_runtime(db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_delete_pending_rows(&connection)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .into_iter()
            .find(|row| row.id == clipping_id)
    };
    let Some(target) = target else {
        return Ok(());
    };
    complete_delete_pending_target(
        writer,
        layout,
        layout,
        diagnostics,
        &target.id,
        &target.asset_relative_path,
    )
}

pub fn complete_delete_pending_target(
    writer: &DatabaseWriter,
    asset_layout: &ClippingAssetLayout,
    thumbnail_layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    clipping_id: &str,
    asset_relative_path: &str,
) -> Result<(), ClippingError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let trash_entry =
        asset_layout.move_canonical_to_trash_at(clipping_id, asset_relative_path, nonce)?;
    let id = clipping_id.to_string();
    let deleted = writer
        .execute(write_context("clipping_delete_row"), move |connection| {
            repository::delete_if_pending(connection, &id).map_err(Into::into)
        })
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
    if !deleted {
        // Leave a retryable delete_pending row and trash entry; the next
        // recovery pass completes them.
        return Err(ClippingError::new(ClippingErrorCode::DeleteFailed));
    }
    let thumbnail_started = std::time::Instant::now();
    let thumbnail_cleanup_failed = match thumbnail_layout.remove_thumbnails(clipping_id) {
        Ok(summary) => summary.failures > 0,
        Err(_) => true,
    };
    if thumbnail_cleanup_failed {
        record_recovery_diagnostic(
            diagnostics,
            "clipping_thumbnail_cleanup",
            thumbnail_started.elapsed(),
            true,
        );
    }
    if let Some(entry) = trash_entry {
        if asset_layout.remove_trash_entry(&entry).is_err() {
            // Trash cleanup failures are retryable through orphan cleanup and
            // never recreate the deleted clipping.
            return Err(ClippingError::new(ClippingErrorCode::DeleteFailed));
        }
    }
    Ok(())
}

/// Bounded synchronous reconciliation executed at startup after database
/// initialization (RECOVERY-001/002). Creating and delete_pending rows are
/// expected to be very few; both sets are processed fully.
pub fn run_startup_recovery(
    db_path: &Path,
    writer: &DatabaseWriter,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    now: i64,
) -> StartupRecoverySummary {
    let started = std::time::Instant::now();
    let mut summary = StartupRecoverySummary::default();
    let outcome = (|| -> Result<(), ClippingError> {
        let (creating_ids, delete_pending_ids) = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            let creating = repository::load_creating_rows(&connection)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let deleting = repository::load_delete_pending_ids(&connection)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            (creating, deleting)
        };
        for id in creating_ids {
            match recover_creating_id(db_path, writer, layout, &id, now) {
                Ok(ClippingAssetState::Ready) => summary.creating_marked_ready += 1,
                Ok(_) => summary.creating_marked_missing += 1,
                Err(_) => summary.failures += 1,
            }
        }
        for id in delete_pending_ids {
            match complete_delete_pending_id(db_path, writer, layout, diagnostics, &id) {
                Ok(()) => summary.deletions_completed += 1,
                Err(_) => summary.failures += 1,
            }
        }
        Ok(())
    })();
    if outcome.is_err() {
        summary.failures = summary.failures.saturating_add(1);
    }
    record_recovery_diagnostic(
        diagnostics,
        "clipping_startup_recovery",
        started.elapsed(),
        outcome.is_err() || summary.failures > 0,
    );
    summary
}

pub fn run_startup_recovery_roots(
    db_path: &Path,
    writer: &DatabaseWriter,
    roots: &ClippingRootRegistry,
    thumbnail_layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    now: i64,
) -> StartupRecoverySummary {
    let started = std::time::Instant::now();
    let mut summary = StartupRecoverySummary::default();
    let outcome = (|| -> Result<(), ClippingError> {
        let (creating, deleting) = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            (
                repository::load_creating_rows(&connection)
                    .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?,
                repository::load_delete_pending_rows(&connection)
                    .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?,
            )
        };
        for target in creating {
            let result = roots
                .resolve(&target.asset_root_id)
                .and_then(|layout| recover_creating_id(db_path, writer, &layout, &target.id, now));
            match result {
                Ok(ClippingAssetState::Ready) => summary.creating_marked_ready += 1,
                Ok(_) => summary.creating_marked_missing += 1,
                Err(_) => summary.failures += 1,
            }
        }
        for target in deleting {
            let result = roots.resolve(&target.asset_root_id).and_then(|layout| {
                complete_delete_pending_target(
                    writer,
                    &layout,
                    thumbnail_layout,
                    diagnostics,
                    &target.id,
                    &target.asset_relative_path,
                )
            });
            match result {
                Ok(()) => summary.deletions_completed += 1,
                Err(_) => summary.failures += 1,
            }
        }
        Ok(())
    })();
    if outcome.is_err() {
        summary.failures = summary.failures.saturating_add(1);
    }
    record_recovery_diagnostic(
        diagnostics,
        "clipping_startup_recovery",
        started.elapsed(),
        outcome.is_err() || summary.failures > 0,
    );
    summary
}

/// Detached deferred cleanup for orphaned managed entries (RECOVERY-004).
/// It completely and streaming-enumerates only managed clipping categories,
/// reports actual iterator consumption, and independently caps mutation
/// attempts. It never scans user newspaper download directories.
pub fn run_deferred_cleanup(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
) -> DeferredCleanupSummary {
    run_deferred_cleanup_at(db_path, layout, diagnostics, SystemTime::now())
}

pub(crate) fn run_deferred_cleanup_at(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    now_system: SystemTime,
) -> DeferredCleanupSummary {
    run_deferred_cleanup_at_scope(db_path, layout, diagnostics, now_system, None)
}

pub fn run_deferred_cleanup_for_root(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    root_id: &str,
    diagnostics: &DatabaseDiagnostics,
) -> DeferredCleanupSummary {
    run_deferred_cleanup_at_scope(
        db_path,
        layout,
        diagnostics,
        SystemTime::now(),
        Some(root_id),
    )
}

fn run_deferred_cleanup_at_scope(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
    now_system: SystemTime,
    root_id: Option<&str>,
) -> DeferredCleanupSummary {
    let started = std::time::Instant::now();
    let mut summary = DeferredCleanupSummary::default();
    let outcome = (|| -> Result<(), ClippingError> {
        let known_ids = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            match root_id {
                Some(root_id) => repository::load_all_ids_for_root(&connection, root_id),
                None => repository::load_all_ids(&connection),
            }
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .into_iter()
            .collect::<HashSet<_>>()
        };
        // Staging directories without a row older than the grace period move
        // to quarantine.
        summary.add(quarantine_orphans(
            layout,
            &layout.staging_dir()?,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
            "stale-staging",
        )?);

        // Canonical asset directories without a row older than the grace
        // period move to quarantine rather than immediate deletion.
        summary.add(quarantine_orphans(
            layout,
            &layout.assets_dir()?,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
            "stale-asset",
        )?);

        // Trash entries without a row older than the grace period may be
        // deleted outright; their rows were already removed by confirmed
        // deletion.
        summary.add(delete_orphan_trash(
            layout,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
        )?);

        // Derived clipping thumbnails are regenerable cache. Exact-ID files
        // without a row become eligible after the same orphan grace period.
        summary.add(delete_orphan_thumbnails(
            layout,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
        )?);

        // Quarantine entries are retained for seven days, then deleted.
        summary.add(delete_expired_quarantine(
            layout,
            now_system,
            QUARANTINE_RETENTION,
        )?);

        Ok(())
    })();
    if outcome.is_err() {
        summary.failures = summary.failures.saturating_add(1);
    }
    record_recovery_diagnostic(
        diagnostics,
        "clipping_deferred_cleanup",
        started.elapsed(),
        outcome.is_err() || summary.failures > 0,
    );
    summary
}

/// Cleanup for a download snapshot root. Only the reserved internal subtree is
/// enumerated; visible edition/date directories are durable user data and are
/// never recursively swept as cache or orphan space.
pub fn run_deferred_internal_cleanup(
    db_path: &Path,
    layout: &ClippingAssetLayout,
    root_id: &str,
    diagnostics: &DatabaseDiagnostics,
) -> DeferredCleanupSummary {
    let started = std::time::Instant::now();
    let now_system = SystemTime::now();
    let mut summary = DeferredCleanupSummary::default();
    let outcome = (|| -> Result<(), ClippingError> {
        let known_ids = {
            let connection = open_runtime(db_path)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
            repository::load_all_ids_for_root(&connection, root_id)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
                .into_iter()
                .collect::<HashSet<_>>()
        };
        summary.add(quarantine_orphans(
            layout,
            &layout.staging_dir()?,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
            "stale-staging",
        )?);
        summary.add(delete_orphan_trash(
            layout,
            &known_ids,
            now_system,
            ORPHAN_GRACE,
        )?);
        summary.add(delete_expired_quarantine(
            layout,
            now_system,
            QUARANTINE_RETENTION,
        )?);
        Ok(())
    })();
    if outcome.is_err() {
        summary.failures = summary.failures.saturating_add(1);
    }
    record_recovery_diagnostic(
        diagnostics,
        "clipping_snapshot_root_cleanup",
        started.elapsed(),
        outcome.is_err() || summary.failures > 0,
    );
    summary
}

fn directory_age(metadata: &fs::Metadata, now: SystemTime) -> Option<Duration> {
    let modified = metadata.modified().ok()?;
    now.duration_since(modified).ok()
}

fn quarantine_age(
    entry: &fs::DirEntry,
    metadata: &fs::Metadata,
    now: SystemTime,
) -> Option<Duration> {
    let timestamp = entry
        .file_name()
        .to_str()
        .and_then(|name| name.split('-').next())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)));
    timestamp
        .and_then(|created| now.duration_since(created).ok())
        .or_else(|| directory_age(metadata, now))
}

fn quarantine_orphans(
    layout: &ClippingAssetLayout,
    directory: &Path,
    known_ids: &HashSet<String>,
    now: SystemTime,
    grace: Duration,
    reason: &'static str,
) -> Result<DeferredCleanupSummary, ClippingError> {
    let mut summary = DeferredCleanupSummary::default();
    let mut candidates = Vec::with_capacity(CLEANUP_MUTATION_BUDGET_PER_CATEGORY);
    let entries = fs::read_dir(directory)
        .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
    for entry in entries {
        summary.enumerated = summary.enumerated.saturating_add(1);
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.failures = summary.failures.saturating_add(1);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if known_ids.contains(&name) {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.failures += 1;
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            summary.failures += 1;
            continue;
        }
        if !metadata.file_type().is_dir() {
            continue;
        }
        let Some(age) = directory_age(&metadata, now) else {
            summary.failures += 1;
            continue;
        };
        if age < grace {
            continue;
        }
        if candidates.len() < CLEANUP_MUTATION_BUDGET_PER_CATEGORY {
            candidates.push(entry.path());
        }
    }
    for candidate in candidates {
        let timestamp = now
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs() as i64)
            .unwrap_or_default();
        summary.mutations_attempted += 1;
        match layout.quarantine_directory(&candidate, reason, timestamp) {
            Ok(()) => summary.processed += 1,
            Err(_) => summary.failures += 1,
        }
    }
    summary.max_category_enumerated = summary.enumerated;
    summary.max_category_mutations = summary.mutations_attempted;
    Ok(summary)
}

fn delete_orphan_trash(
    layout: &ClippingAssetLayout,
    known_ids: &HashSet<String>,
    now: SystemTime,
    grace: Duration,
) -> Result<DeferredCleanupSummary, ClippingError> {
    let mut summary = DeferredCleanupSummary::default();
    let mut candidates = Vec::with_capacity(CLEANUP_MUTATION_BUDGET_PER_CATEGORY);
    let entries = fs::read_dir(layout.trash_dir()?)
        .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
    for entry in entries {
        summary.enumerated = summary.enumerated.saturating_add(1);
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.failures = summary.failures.saturating_add(1);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.failures += 1;
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            summary.failures += 1;
            continue;
        }
        if !metadata.file_type().is_dir() {
            continue;
        }
        // Trash entries are named <clipping-id>-<nonce>; keep entries whose
        // clipping ID still exists.
        let owned = name
            .get(..36)
            .filter(|_| name.as_bytes().get(36) == Some(&b'-'))
            .is_some_and(|id| known_ids.contains(id));
        if owned {
            continue;
        }
        let Some(age) = directory_age(&metadata, now) else {
            summary.failures += 1;
            continue;
        };
        if age < grace {
            continue;
        }
        if candidates.len() < CLEANUP_MUTATION_BUDGET_PER_CATEGORY {
            candidates.push(entry.path());
        }
    }
    for candidate in candidates {
        summary.mutations_attempted += 1;
        match layout.remove_trash_entry(&candidate) {
            Ok(()) => summary.processed += 1,
            Err(_) => summary.failures += 1,
        }
    }
    summary.max_category_enumerated = summary.enumerated;
    summary.max_category_mutations = summary.mutations_attempted;
    Ok(summary)
}

fn delete_orphan_thumbnails(
    layout: &ClippingAssetLayout,
    known_ids: &HashSet<String>,
    now: SystemTime,
    grace: Duration,
) -> Result<DeferredCleanupSummary, ClippingError> {
    let mut summary = DeferredCleanupSummary::default();
    let mut candidates = Vec::with_capacity(CLEANUP_MUTATION_BUDGET_PER_CATEGORY);
    let entries = fs::read_dir(layout.thumbnails_dir()?)
        .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
    for entry in entries {
        summary.enumerated = summary.enumerated.saturating_add(1);
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.failures = summary.failures.saturating_add(1);
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((owner, _)) = thumbnail_owner(&name) else {
            continue;
        };
        if known_ids.contains(owner) {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.failures = summary.failures.saturating_add(1);
                continue;
            }
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            summary.failures = summary.failures.saturating_add(1);
            continue;
        }
        let Some(age) = directory_age(&metadata, now) else {
            summary.failures = summary.failures.saturating_add(1);
            continue;
        };
        if age >= grace && candidates.len() < CLEANUP_MUTATION_BUDGET_PER_CATEGORY {
            candidates.push(entry.path());
        }
    }
    for candidate in candidates {
        summary.mutations_attempted = summary.mutations_attempted.saturating_add(1);
        match layout.remove_thumbnail_entry(&candidate) {
            Ok(()) => summary.processed = summary.processed.saturating_add(1),
            Err(_) => summary.failures = summary.failures.saturating_add(1),
        }
    }
    summary.max_category_enumerated = summary.enumerated;
    summary.max_category_mutations = summary.mutations_attempted;
    Ok(summary)
}

fn delete_expired_quarantine(
    layout: &ClippingAssetLayout,
    now: SystemTime,
    retention: Duration,
) -> Result<DeferredCleanupSummary, ClippingError> {
    let mut summary = DeferredCleanupSummary::default();
    let mut candidates = Vec::with_capacity(CLEANUP_MUTATION_BUDGET_PER_CATEGORY);
    let entries = fs::read_dir(layout.quarantine_dir()?)
        .map_err(|_| ClippingError::new(ClippingErrorCode::RecoveryFailed))?;
    for entry in entries {
        summary.enumerated = summary.enumerated.saturating_add(1);
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                summary.failures = summary.failures.saturating_add(1);
                continue;
            }
        };
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => {
                summary.failures += 1;
                continue;
            }
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            summary.failures += 1;
            continue;
        }
        let Some(age) = quarantine_age(&entry, &metadata, now) else {
            summary.failures += 1;
            continue;
        };
        if age < retention {
            continue;
        }
        if candidates.len() < CLEANUP_MUTATION_BUDGET_PER_CATEGORY {
            candidates.push(entry.path());
        }
    }
    for candidate in candidates {
        summary.mutations_attempted += 1;
        match layout.remove_quarantine_entry(&candidate) {
            Ok(()) => summary.processed += 1,
            Err(_) => summary.failures += 1,
        }
    }
    summary.max_category_enumerated = summary.enumerated;
    summary.max_category_mutations = summary.mutations_attempted;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    fn measure_complete_streaming_enumeration(entry_count: usize) {
        let directory = tempfile::tempdir().unwrap();
        let layout = ClippingAssetLayout::new(directory.path().join("newspaper-clippings"));
        let staging = layout.staging_dir().unwrap();
        for index in 0..entry_count {
            fs::create_dir(staging.join(format!("{index:08x}-1111-4111-8111-{index:012x}")))
                .unwrap();
        }
        let now = SystemTime::now()
            .checked_add(Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();
        let started = std::time::Instant::now();
        let summary = quarantine_orphans(
            &layout,
            &staging,
            &HashSet::new(),
            now,
            ORPHAN_GRACE,
            "measurement",
        )
        .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(summary.enumerated, entry_count);
        assert_eq!(
            summary.mutations_attempted,
            CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );
        assert_eq!(summary.processed, CLEANUP_MUTATION_BUDGET_PER_CATEGORY);
        assert_eq!(summary.failures, 0);
        assert_eq!(
            fs::read_dir(&staging).unwrap().count(),
            entry_count - CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );

        let longest_candidate_bytes = staging.to_string_lossy().len() + 1 + 36;
        let approximate_candidate_buffer_bytes = CLEANUP_MUTATION_BUDGET_PER_CATEGORY
            * (size_of::<std::path::PathBuf>() + longest_candidate_bytes);
        eprintln!(
            "cleanup_measurement entries={entry_count} elapsed_ms={} approximate_candidate_buffer_bytes={approximate_candidate_buffer_bytes}",
            elapsed.as_millis()
        );
    }

    #[test]
    fn cleanup_measurement_complete_enumeration_500_entries() {
        measure_complete_streaming_enumeration(500);
    }

    #[test]
    fn cleanup_measurement_complete_enumeration_5000_entries() {
        measure_complete_streaming_enumeration(5_000);
    }
}
