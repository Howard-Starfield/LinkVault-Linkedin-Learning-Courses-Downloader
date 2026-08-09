//! Phase 1 orchestration boundary for clipping persistence and managed assets.
//! Image crop production belongs to Phase 2; this service accepts only a
//! complete, validated staging asset and never performs image work in a writer
//! closure.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseErrorClass, DatabaseProvider,
};
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;

use super::clipping_assets::ClippingAssetLayout;
use super::clipping_models::{
    normalize_search_query, normalize_title, validate_asset_byte_count, validate_clipping_id,
    validate_edition_code, validate_edition_name, validate_list_limit, validate_note_markdown,
    validate_page_number, validate_publication_date, validate_sha256_hex, validate_source_mime,
    ClippingAssetState, ClippingError, ClippingErrorCode, NewspaperClipping,
    NewspaperClippingListQuery,
};
use super::clipping_recovery;
use super::clipping_repository::{self as repository, ClippingDetail, ClippingSummary};

#[derive(Clone)]
pub struct ClippingService {
    db_path: PathBuf,
    writer: DatabaseWriter,
    layout: ClippingAssetLayout,
    diagnostics: DatabaseDiagnostics,
    integrity_scheduler: Arc<IntegrityTransitionScheduler>,
}

pub(crate) const MEDIA_INTEGRITY_QUEUE_CAPACITY: usize = 32;

struct IntegrityTransition {
    clipping_id: String,
    error_code: &'static str,
    now: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntegrityScheduleOutcome {
    Scheduled,
    Coalesced,
    Unavailable,
}

struct IntegrityTransitionScheduler {
    sender: mpsc::SyncSender<IntegrityTransition>,
    pending: Arc<IntegrityPending>,
    diagnostics: DatabaseDiagnostics,
}

struct IntegrityPending {
    ids: Mutex<HashSet<String>>,
    changed: Condvar,
}

impl IntegrityTransitionScheduler {
    fn new(writer: DatabaseWriter, diagnostics: DatabaseDiagnostics) -> Self {
        let (sender, receiver) =
            mpsc::sync_channel::<IntegrityTransition>(MEDIA_INTEGRITY_QUEUE_CAPACITY);
        let pending = Arc::new(IntegrityPending {
            ids: Mutex::new(HashSet::new()),
            changed: Condvar::new(),
        });
        let worker_pending = Arc::clone(&pending);
        let worker_diagnostics = diagnostics.clone();
        let _ = thread::Builder::new()
            .name("linkvault-clipping-integrity".to_string())
            .spawn(move || {
                while let Ok(transition) = receiver.recv() {
                    let started = Instant::now();
                    let id = transition.clipping_id.clone();
                    let code = transition.error_code.to_string();
                    let result = writer.execute(
                        ClippingService::context("clipping_media_mark_missing"),
                        move |db| {
                            repository::mark_missing_from_ready(db, &id, &code, transition.now)
                                .map_err(Into::into)
                        },
                    );
                    worker_pending
                        .ids
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&transition.clipping_id);
                    worker_pending.changed.notify_all();
                    if result.is_err() {
                        record_integrity_diagnostic(
                            &worker_diagnostics,
                            "clipping_media_integrity_transition",
                            started.elapsed(),
                        );
                    }
                }
            });
        Self {
            sender,
            pending,
            diagnostics,
        }
    }

    fn schedule(
        &self,
        clipping_id: &str,
        error_code: &'static str,
        now: i64,
    ) -> IntegrityScheduleOutcome {
        let mut pending = self
            .pending
            .ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.contains(clipping_id) {
            return IntegrityScheduleOutcome::Coalesced;
        }
        if pending.len() >= MEDIA_INTEGRITY_QUEUE_CAPACITY {
            drop(pending);
            record_integrity_diagnostic(
                &self.diagnostics,
                "clipping_media_integrity_schedule",
                std::time::Duration::ZERO,
            );
            return IntegrityScheduleOutcome::Unavailable;
        }
        pending.insert(clipping_id.to_string());
        let transition = IntegrityTransition {
            clipping_id: clipping_id.to_string(),
            error_code,
            now,
        };
        match self.sender.try_send(transition) {
            Ok(()) => IntegrityScheduleOutcome::Scheduled,
            Err(_) => {
                pending.remove(clipping_id);
                drop(pending);
                record_integrity_diagnostic(
                    &self.diagnostics,
                    "clipping_media_integrity_schedule",
                    std::time::Duration::ZERO,
                );
                IntegrityScheduleOutcome::Unavailable
            }
        }
    }

    #[allow(dead_code)] // Deterministic queue-bound instrumentation for Phase 1 verification.
    fn pending_count(&self) -> usize {
        self.pending
            .ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    #[allow(dead_code)] // Deterministic queue-drain instrumentation for Phase 1 verification.
    fn wait_until_idle(&self, timeout: std::time::Duration) -> bool {
        let pending = self
            .pending
            .ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (pending, _) = self
            .pending
            .changed
            .wait_timeout_while(pending, timeout, |ids| !ids.is_empty())
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.is_empty()
    }
}

fn record_integrity_diagnostic(
    diagnostics: &DatabaseDiagnostics,
    operation: &'static str,
    elapsed: std::time::Duration,
) {
    diagnostics.record(DatabaseDiagnosticInput {
        kind: DatabaseDiagnosticKind::Recovery,
        operation,
        provider: DatabaseProvider::Newspaper,
        workflow_id: None,
        elapsed,
        queue_depth: 0,
        outcome: DatabaseDiagnosticOutcome::Error,
        error_class: Some(DatabaseErrorClass::Recovery),
    });
}

impl ClippingService {
    pub fn new(
        db_path: PathBuf,
        writer: DatabaseWriter,
        layout: ClippingAssetLayout,
        diagnostics: DatabaseDiagnostics,
    ) -> Self {
        let integrity_scheduler = Arc::new(IntegrityTransitionScheduler::new(
            writer.clone(),
            diagnostics.clone(),
        ));
        Self {
            db_path,
            writer,
            layout,
            diagnostics,
            integrity_scheduler,
        }
    }

    pub fn layout(&self) -> &ClippingAssetLayout {
        &self.layout
    }

    fn context(operation: &'static str) -> DatabaseWriteContext {
        DatabaseWriteContext {
            operation,
            provider: DatabaseProvider::Newspaper,
            workflow_id: None,
        }
    }

    fn read_by_id(&self, id: &str) -> Result<Option<NewspaperClipping>, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_by_id(&connection, id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))
    }

    pub fn register_staged(
        &self,
        mut record: repository::NewClippingRecord,
    ) -> Result<NewspaperClipping, ClippingError> {
        validate_record(&mut record)?;
        if let Some(existing) = self.read_by_id(&record.id)? {
            return self.resolve_idempotent(existing, record.now);
        }
        self.layout.verify_staging(
            &record.id,
            record.asset_byte_count,
            record.crop_width,
            record.crop_height,
            &record.asset_checksum_sha256,
        )?;

        let insert_record = record.clone();
        let inserted = self
            .writer
            .execute(Self::context("clipping_insert_creating"), move |db| {
                repository::insert_creating(db, &insert_record).map_err(Into::into)
            });
        if inserted.is_err() {
            if let Some(existing) = self.read_by_id(&record.id)? {
                return self.resolve_idempotent(existing, record.now);
            }
            self.layout.discard_staging(&record.id);
            return Err(ClippingError::new(ClippingErrorCode::DatabaseWriteFailed));
        }

        self.layout.promote_staging(&record.id)?;
        self.layout.verify_canonical(
            &record.id,
            record.asset_byte_count,
            record.crop_width,
            record.crop_height,
            &record.asset_checksum_sha256,
        )?;
        let id = record.id.clone();
        let marked = self
            .writer
            .execute(Self::context("clipping_mark_ready"), move |db| {
                repository::mark_ready_from_creating(db, &id, record.now).map_err(Into::into)
            })
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
        if !marked {
            return Err(ClippingError::new(ClippingErrorCode::OperationConflict));
        }
        self.read_by_id(&record.id)?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound))
    }

    fn resolve_idempotent(
        &self,
        existing: NewspaperClipping,
        now: i64,
    ) -> Result<NewspaperClipping, ClippingError> {
        match existing.asset_state {
            ClippingAssetState::Ready | ClippingAssetState::Missing => Ok(existing),
            ClippingAssetState::DeletePending => {
                Err(ClippingError::new(ClippingErrorCode::OperationConflict))
            }
            ClippingAssetState::Creating => {
                clipping_recovery::recover_creating_id(
                    &self.db_path,
                    &self.writer,
                    &self.layout,
                    &existing.id,
                    now,
                )?;
                self.read_by_id(&existing.id)?
                    .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound))
            }
        }
    }

    pub fn update_note(
        &self,
        id: &str,
        expected_revision: u64,
        title: &str,
        note_markdown: &str,
        now: i64,
    ) -> Result<NewspaperClipping, ClippingError> {
        self.update_note_inner(id, expected_revision, title, note_markdown, now, || {})
    }

    fn update_note_inner<F>(
        &self,
        id: &str,
        expected_revision: u64,
        title: &str,
        note_markdown: &str,
        now: i64,
        after_writer: F,
    ) -> Result<NewspaperClipping, ClippingError>
    where
        F: FnOnce(),
    {
        if !validate_clipping_id(id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        let title = normalize_title(title).map_err(ClippingError::new)?;
        validate_note_markdown(note_markdown).map_err(ClippingError::new)?;
        let owned_id = id.to_string();
        let owned_note = note_markdown.to_string();
        let outcome = self
            .writer
            .execute(Self::context("clipping_update_note"), move |db| {
                repository::update_note(db, &owned_id, expected_revision, &title, &owned_note, now)
                    .map_err(Into::into)
            })
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
        after_writer();
        match outcome {
            repository::NoteUpdateOutcome::Updated { clipping }
            | repository::NoteUpdateOutcome::Unchanged { clipping } => Ok(clipping),
            repository::NoteUpdateOutcome::NotFound => {
                Err(ClippingError::new(ClippingErrorCode::NotFound))
            }
            repository::NoteUpdateOutcome::Conflict { .. } => {
                Err(ClippingError::new(ClippingErrorCode::RevisionConflict))
            }
            repository::NoteUpdateOutcome::NotEditable => {
                Err(ClippingError::new(ClippingErrorCode::NotEditable))
            }
        }
    }

    pub(crate) fn schedule_media_integrity_transition(
        &self,
        clipping_id: &str,
        error_code: &'static str,
        now: i64,
    ) -> IntegrityScheduleOutcome {
        if !validate_clipping_id(clipping_id) {
            return IntegrityScheduleOutcome::Unavailable;
        }
        self.integrity_scheduler
            .schedule(clipping_id, error_code, now)
    }

    #[allow(dead_code)] // Exposed to the media protocol Phase 1 verification fixtures.
    pub(crate) fn pending_media_integrity_transitions(&self) -> usize {
        self.integrity_scheduler.pending_count()
    }

    #[allow(dead_code)] // Exposed to the media protocol Phase 1 verification fixtures.
    pub(crate) fn wait_for_media_integrity_transitions(
        &self,
        timeout: std::time::Duration,
    ) -> bool {
        self.integrity_scheduler.wait_until_idle(timeout)
    }

    pub fn delete(&self, id: &str, expected_revision: u64) -> Result<(), ClippingError> {
        if !validate_clipping_id(id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        let owned_id = id.to_string();
        let outcome = self
            .writer
            .execute(Self::context("clipping_mark_delete_pending"), move |db| {
                repository::mark_delete_pending(db, &owned_id, expected_revision)
                    .map_err(Into::into)
            })
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseWriteFailed))?;
        match outcome {
            repository::DeleteIntentOutcome::Marked => {
                clipping_recovery::complete_delete_pending_id(
                    &self.db_path,
                    &self.writer,
                    &self.layout,
                    &self.diagnostics,
                    id,
                )
            }
            repository::DeleteIntentOutcome::NotFound => {
                Err(ClippingError::new(ClippingErrorCode::NotFound))
            }
            repository::DeleteIntentOutcome::Conflict { .. } => {
                Err(ClippingError::new(ClippingErrorCode::RevisionConflict))
            }
            repository::DeleteIntentOutcome::NotEditable => {
                Err(ClippingError::new(ClippingErrorCode::NotEditable))
            }
        }
    }

    pub fn list(
        &self,
        mut query: NewspaperClippingListQuery,
    ) -> Result<(Vec<ClippingSummary>, u32), ClippingError> {
        query.query = normalize_search_query(&query.query).map_err(ClippingError::new)?;
        validate_list_limit(query.limit).map_err(ClippingError::new)?;
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::list_clippings(&connection, &query)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))
    }

    pub fn detail(&self, id: &str) -> Result<Option<ClippingDetail>, ClippingError> {
        if !validate_clipping_id(id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_detail(&connection, id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))
    }

    pub fn recover_startup(
        &self,
        diagnostics: &DatabaseDiagnostics,
        now: i64,
    ) -> clipping_recovery::StartupRecoverySummary {
        clipping_recovery::run_startup_recovery(
            &self.db_path,
            &self.writer,
            &self.layout,
            diagnostics,
            now,
        )
    }

    pub fn run_deferred_cleanup(
        &self,
        diagnostics: &DatabaseDiagnostics,
    ) -> clipping_recovery::DeferredCleanupSummary {
        clipping_recovery::run_deferred_cleanup(&self.db_path, &self.layout, diagnostics)
    }
}

fn validate_record(record: &mut repository::NewClippingRecord) -> Result<(), ClippingError> {
    if !validate_clipping_id(&record.id) {
        return Err(ClippingError::new(ClippingErrorCode::InvalidId));
    }
    let invalid = || ClippingError::new(ClippingErrorCode::AssetValidationFailed);
    if record.source_media_version_snapshot <= 0
        || !validate_source_mime(&record.source_mime_type_snapshot)
        || !validate_edition_code(&record.edition_code_snapshot)
        || !validate_edition_name(&record.edition_name_snapshot)
        || !validate_publication_date(&record.publication_date_snapshot)
        || !validate_page_number(&record.page_number_snapshot)
        || record.source_pixel_width == 0
        || record.source_pixel_height == 0
        || record.crop_width == 0
        || record.crop_height == 0
        || record
            .crop_x
            .checked_add(record.crop_width)
            .map_or(true, |x| x > record.source_pixel_width)
        || record
            .crop_y
            .checked_add(record.crop_height)
            .map_or(true, |y| y > record.source_pixel_height)
        || !validate_asset_byte_count(record.asset_byte_count)
        || !validate_sha256_hex(&record.asset_checksum_sha256)
        || record
            .source_checksum_snapshot
            .as_deref()
            .is_some_and(|value| !validate_sha256_hex(value))
    {
        return Err(invalid());
    }
    let expected_path = ClippingAssetLayout::canonical_relative_path(&record.id)?;
    if record.asset_relative_path != expected_path {
        return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
    }
    record.title = normalize_title(&record.title).map_err(ClippingError::new)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::clipping_assets::{encode_test_webp, sha256_hex};
    use super::super::clipping_models::{ClippingSourceKind, NewspaperClippingSort};
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::{
        DatabaseDiagnosticOutcome, DatabaseDiagnostics, DatabaseErrorClass,
    };
    use std::path::Path;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, SystemTime};

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn fixture() -> (tempfile::TempDir, ClippingService, DatabaseDiagnostics) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let diagnostics = DatabaseDiagnostics::default();
        let writer = DatabaseWriter::start(db_path.clone(), diagnostics.clone()).unwrap();
        let layout = ClippingAssetLayout::new(temp.path().join("newspaper-clippings"));
        (
            temp,
            ClippingService::new(db_path, writer, layout, diagnostics.clone()),
            diagnostics,
        )
    }

    fn staged_record(service: &ClippingService, id: &str) -> repository::NewClippingRecord {
        let bytes = encode_test_webp(24, 16);
        service.layout.write_staging(id, &bytes).unwrap();
        repository::NewClippingRecord {
            id: id.to_string(),
            source_job_id: None,
            source_page_id: None,
            source_media_version_snapshot: 1,
            source_kind_snapshot: ClippingSourceKind::Optimized,
            source_mime_type_snapshot: "image/webp".to_string(),
            source_checksum_snapshot: None,
            edition_code_snapshot: "NY".to_string(),
            edition_name_snapshot: "New York".to_string(),
            publication_date_snapshot: "2026-08-08".to_string(),
            page_number_snapshot: "A01".to_string(),
            source_pixel_width: 24,
            source_pixel_height: 16,
            crop_x: 0,
            crop_y: 0,
            crop_width: 24,
            crop_height: 16,
            asset_relative_path: ClippingAssetLayout::canonical_relative_path(id).unwrap(),
            asset_byte_count: bytes.len() as u64,
            asset_checksum_sha256: sha256_hex(&bytes),
            title: "New York · 2026-08-08 · A01".to_string(),
            now: 100,
        }
    }

    #[test]
    fn persistence_gate_clipping_creation_is_ready_and_idempotent() {
        let (_temp, service, _diagnostics) = fixture();
        let record = staged_record(&service, ID);
        let created = service.register_staged(record.clone()).unwrap();
        assert_eq!(created.asset_state, ClippingAssetState::Ready);
        assert!(service.layout.canonical_path(ID).unwrap().is_file());
        let retried = service.register_staged(record).unwrap();
        assert_eq!(retried.id, created.id);
        let query = NewspaperClippingListQuery {
            query: String::new(),
            sort: NewspaperClippingSort::UpdatedDesc,
            offset: 0,
            limit: 50,
        };
        assert_eq!(service.list(query).unwrap().1, 1);
    }

    #[test]
    fn persistence_gate_clipping_update_rejects_stale_revision() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let updated = service
            .update_note(ID, created.revision, "A title", "winner", 101)
            .unwrap();
        assert_eq!(updated.revision, 2);
        let stale = service.update_note(ID, created.revision, "A title", "loser", 102);
        assert_eq!(stale.unwrap_err().code, ClippingErrorCode::RevisionConflict);
        assert_eq!(
            service.detail(ID).unwrap().unwrap().clipping.note_markdown,
            "winner"
        );
    }

    #[test]
    fn persistence_gate_concurrent_updates_have_one_revision_winner() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut callers = Vec::new();
        for winner in ["first", "second"] {
            let caller = service.clone();
            let ready = barrier.clone();
            callers.push(thread::spawn(move || {
                ready.wait();
                caller.update_note(ID, created.revision, winner, winner, 101)
            }));
        }
        barrier.wait();
        let results: Vec<_> = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    result
                        .as_ref()
                        .is_err_and(|error| error.code == ClippingErrorCode::RevisionConflict)
                })
                .count(),
            1
        );
        let stored = service.detail(ID).unwrap().unwrap().clipping;
        assert_eq!(stored.revision, 2);
        assert_eq!(stored.title, stored.note_markdown);
        assert!(matches!(stored.title.as_str(), "first" | "second"));
    }

    #[test]
    fn persistence_gate_note_update_returns_its_own_writer_acknowledged_snapshot() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let (acknowledged_tx, acknowledged_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let first_service = service.clone();
        let first = thread::spawn(move || {
            first_service.update_note_inner(
                ID,
                created.revision,
                "  Revision two  ",
                "first caller",
                101,
                || {
                    acknowledged_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                },
            )
        });

        acknowledged_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("revision two must commit before the response is released");
        let third = service
            .update_note(ID, 2, "Revision three", "second caller", 102)
            .unwrap();
        assert_eq!(third.revision, 3);
        release_tx.send(()).unwrap();

        let acknowledged = first.join().unwrap().unwrap();
        assert_eq!(acknowledged.revision, 2);
        assert_eq!(acknowledged.title, "Revision two");
        assert_eq!(acknowledged.note_markdown, "first caller");
        let stored = service.detail(ID).unwrap().unwrap().clipping;
        assert_eq!(stored.revision, 3);
        assert_eq!(stored.note_markdown, "second caller");
    }

    #[test]
    fn persistence_gate_note_update_preserves_noop_not_found_and_not_editable_outcomes() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let unchanged = service
            .update_note(
                ID,
                created.revision,
                &created.title,
                &created.note_markdown,
                101,
            )
            .unwrap();
        assert_eq!(unchanged.revision, created.revision);
        assert_eq!(unchanged.updated_at, created.updated_at);

        let missing = service.update_note(
            "7c9e6679-7425-40de-944b-e07fc1f90ae7",
            1,
            "Missing",
            "note",
            102,
        );
        assert_eq!(missing.unwrap_err().code, ClippingErrorCode::NotFound);

        let owned_id = ID.to_string();
        service
            .writer
            .execute(
                ClippingService::context("test_make_clipping_not_editable"),
                move |db| {
                    db.execute(
                        "UPDATE newspaper_clippings SET asset_state = 'creating' WHERE id = ?1",
                        rusqlite::params![owned_id],
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        let not_editable = service.update_note(ID, created.revision, "Blocked", "note", 103);
        assert_eq!(
            not_editable.unwrap_err().code,
            ClippingErrorCode::NotEditable
        );
    }

    #[test]
    fn persistence_gate_list_searches_full_note_but_returns_only_bounded_excerpt() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let note = format!("visible {} TAIL_MARKER", "x".repeat(5_000));
        service
            .update_note(ID, created.revision, "A title", &note, 101)
            .unwrap();

        let (rows, total) = service
            .list(NewspaperClippingListQuery {
                query: "TAIL_MARKER".to_string(),
                sort: NewspaperClippingSort::UpdatedDesc,
                offset: 0,
                limit: 50,
            })
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].excerpt.len() <= 160);
        assert!(!rows[0].excerpt.contains("TAIL_MARKER"));
        assert!(!rows[0].source_available);
    }

    #[test]
    fn persistence_gate_clipping_delete_removes_only_aggregate_asset() {
        let (temp, service, _diagnostics) = fixture();
        let sentinel = temp.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"keep").unwrap();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let other_id = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
        for version in [1, 2] {
            std::fs::write(
                service.layout.thumbnail_path(ID, version).unwrap(),
                encode_test_webp(4, 4),
            )
            .unwrap();
        }
        let other_thumbnail = service.layout.thumbnail_path(other_id, 1).unwrap();
        std::fs::write(&other_thumbnail, encode_test_webp(4, 4)).unwrap();
        service.delete(ID, created.revision).unwrap();
        assert!(service.detail(ID).unwrap().is_none());
        assert!(!service.layout.canonical_path(ID).unwrap().exists());
        assert!(!service.layout.thumbnail_path(ID, 1).unwrap().exists());
        assert!(!service.layout.thumbnail_path(ID, 2).unwrap().exists());
        assert!(other_thumbnail.exists());
        assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
    }

    #[test]
    fn persistence_gate_thumbnail_cache_failures_do_not_block_confirmed_deletion() {
        let (temp, service, diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let other_id = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
        service
            .register_staged(staged_record(&service, other_id))
            .unwrap();
        let thumbnails = service.layout.thumbnails_dir().unwrap();
        let undeletable = service.layout.thumbnail_path(ID, 1).unwrap();
        std::fs::create_dir(&undeletable).unwrap();

        let outside = temp.path().join("outside-thumbnail-link");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        let reparse = service.layout.thumbnail_path(ID, 2).unwrap();
        let reparse_created = create_dir_link(&outside, &reparse);

        let removable = service.layout.thumbnail_path(ID, 3).unwrap();
        std::fs::write(&removable, encode_test_webp(4, 4)).unwrap();
        let other = service.layout.thumbnail_path(other_id, 1).unwrap();
        std::fs::write(&other, encode_test_webp(4, 4)).unwrap();
        let lookalike = thumbnails.join(format!("{ID}0-asset-1.webp"));
        std::fs::write(&lookalike, encode_test_webp(4, 4)).unwrap();
        let malformed = thumbnails.join("unrelated-malformed-entry");
        std::fs::create_dir(&malformed).unwrap();

        service.delete(ID, created.revision).unwrap();

        let connection = open_runtime(&service.db_path).unwrap();
        let row_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_clippings WHERE id = ?1",
                rusqlite::params![ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 0, "title and note row must be durably deleted");
        assert!(!service.layout.canonical_path(ID).unwrap().exists());
        assert!(undeletable.exists());
        if reparse_created {
            assert!(reparse.exists());
        }
        assert!(!removable.exists());
        assert!(other.exists());
        assert!(lookalike.exists());
        assert!(malformed.exists());
        assert_eq!(
            std::fs::read(outside.join("sentinel.txt")).unwrap(),
            b"keep-me"
        );

        let event = diagnostics
            .snapshot()
            .into_iter()
            .find(|event| event.operation == "clipping_thumbnail_cleanup")
            .expect("cache failure must emit a safe diagnostic");
        assert_eq!(event.outcome, DatabaseDiagnosticOutcome::Error);
        assert_eq!(event.error_class, Some(DatabaseErrorClass::Recovery));
        let diagnostics_text = format!("{event:?}");
        assert!(!diagnostics_text.contains(&temp.path().to_string_lossy().to_string()));

        // A later detached cleanup pass removes ordinary orphan cache files,
        // while malformed and reparse entries remain contained and diagnosed.
        for index in 0..=clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY {
            let orphan_id = format!("{index:08x}-2222-4222-8222-{index:012x}");
            std::fs::write(
                service.layout.thumbnail_path(&orphan_id, 1).unwrap(),
                encode_test_webp(2, 2),
            )
            .unwrap();
        }
        let later_leftover = service.layout.thumbnail_path(ID, 4).unwrap();
        std::fs::write(&later_leftover, encode_test_webp(2, 2)).unwrap();
        let future = SystemTime::now()
            .checked_add(Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();
        for _ in 0..3 {
            let summary = clipping_recovery::run_deferred_cleanup_at(
                &service.db_path,
                &service.layout,
                &diagnostics,
                future,
            );
            assert!(
                summary.max_category_mutations
                    <= clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY
            );
            if !later_leftover.exists() {
                break;
            }
        }
        assert!(!later_leftover.exists());
        assert!(other.exists());
        assert!(lookalike.exists());
        assert!(malformed.exists());
        assert_eq!(
            std::fs::read(outside.join("sentinel.txt")).unwrap(),
            b"keep-me"
        );
    }

    #[test]
    fn persistence_gate_reset_preserves_canonical_asset_bytes() {
        let (_temp, service, _diagnostics) = fixture();
        service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let canonical = service.layout.canonical_path(ID).unwrap();
        let before = std::fs::read(&canonical).unwrap();
        let connection = open_runtime(&service.db_path).unwrap();
        let reset = crate::cache::clear_newspaper_provider_data(&connection).unwrap();
        assert_eq!(reset.clippings_preserved, 1);
        assert_eq!(std::fs::read(&canonical).unwrap(), before);
        assert!(service.detail(ID).unwrap().is_some());
    }

    #[test]
    fn persistence_gate_creating_recovery_is_repeatable() {
        let (_temp, service, diagnostics) = fixture();
        let record = staged_record(&service, ID);
        let inserted = record.clone();
        service
            .writer
            .execute(
                ClippingService::context("test_insert_creating"),
                move |db| repository::insert_creating(db, &inserted).map_err(Into::into),
            )
            .unwrap();
        let first = service.recover_startup(&diagnostics, 101);
        assert_eq!(first.creating_marked_ready, 1);
        let second = service.recover_startup(&diagnostics, 102);
        assert_eq!(second.creating_marked_ready, 0);
        assert_eq!(
            service.detail(ID).unwrap().unwrap().clipping.asset_state,
            ClippingAssetState::Ready
        );
    }

    #[test]
    fn persistence_gate_incomplete_creation_recovers_to_visible_missing_state() {
        let (_temp, service, diagnostics) = fixture();
        let record = staged_record(&service, ID);
        service.layout.discard_staging(ID);
        let inserted = record.clone();
        service
            .writer
            .execute(
                ClippingService::context("test_insert_incomplete_creating"),
                move |db| repository::insert_creating(db, &inserted).map_err(Into::into),
            )
            .unwrap();

        let recovered = service.recover_startup(&diagnostics, 101);
        assert_eq!(recovered.creating_marked_missing, 1);
        assert_eq!(recovered.failures, 0);
        let clipping = service.detail(ID).unwrap().unwrap().clipping;
        assert_eq!(clipping.asset_state, ClippingAssetState::Missing);
        assert_eq!(
            clipping.asset_error_code.as_deref(),
            Some(clipping_recovery::ASSET_CREATION_INCOMPLETE)
        );
    }

    #[test]
    fn persistence_gate_row_recovery_failure_is_diagnostic_and_retryable() {
        let (_temp, service, diagnostics) = fixture();
        let record = staged_record(&service, ID);
        let inserted = record.clone();
        service
            .writer
            .execute(
                ClippingService::context("test_insert_retryable_creating"),
                move |db| repository::insert_creating(db, &inserted).map_err(Into::into),
            )
            .unwrap();
        service.writer.shutdown().unwrap();

        let failed = service.recover_startup(&diagnostics, 101);
        assert_eq!(failed.failures, 1);
        let connection = open_runtime(&service.db_path).unwrap();
        assert_eq!(
            repository::row_state(&connection, ID).unwrap(),
            Some(ClippingAssetState::Creating)
        );
        drop(connection);
        let failed_event = diagnostics
            .snapshot()
            .into_iter()
            .find(|event| event.operation == "clipping_startup_recovery")
            .unwrap();
        assert_eq!(failed_event.outcome, DatabaseDiagnosticOutcome::Error);
        assert_eq!(failed_event.error_class, Some(DatabaseErrorClass::Recovery));

        let retry_diagnostics = DatabaseDiagnostics::default();
        let retry_writer =
            DatabaseWriter::start(service.db_path.clone(), retry_diagnostics.clone()).unwrap();
        let retry_service = ClippingService::new(
            service.db_path.clone(),
            retry_writer,
            service.layout.clone(),
            retry_diagnostics.clone(),
        );
        let recovered = retry_service.recover_startup(&retry_diagnostics, 102);
        assert_eq!(recovered.creating_marked_ready, 1);
        assert_eq!(recovered.failures, 0);
        assert_eq!(
            retry_service
                .detail(ID)
                .unwrap()
                .unwrap()
                .clipping
                .asset_state,
            ClippingAssetState::Ready
        );
    }

    #[test]
    fn persistence_gate_delete_recovery_retries_after_database_failure() {
        let (_temp, service, diagnostics) = fixture();
        let created = service
            .register_staged(staged_record(&service, ID))
            .unwrap();
        let owned_id = ID.to_string();
        service
            .writer
            .execute(
                ClippingService::context("test_mark_retryable_delete"),
                move |db| {
                    repository::mark_delete_pending(db, &owned_id, created.revision)
                        .map_err(Into::into)
                },
            )
            .unwrap();
        service.writer.shutdown().unwrap();

        let failed = service.recover_startup(&diagnostics, 101);
        assert_eq!(failed.failures, 1);
        assert!(!service.layout.canonical_path(ID).unwrap().exists());
        let connection = open_runtime(&service.db_path).unwrap();
        assert_eq!(
            repository::row_state(&connection, ID).unwrap(),
            Some(ClippingAssetState::DeletePending)
        );
        drop(connection);

        let retry_diagnostics = DatabaseDiagnostics::default();
        let retry_writer =
            DatabaseWriter::start(service.db_path.clone(), retry_diagnostics.clone()).unwrap();
        let retry_service = ClippingService::new(
            service.db_path.clone(),
            retry_writer,
            service.layout.clone(),
            retry_diagnostics.clone(),
        );
        let recovered = retry_service.recover_startup(&retry_diagnostics, 102);
        assert_eq!(recovered.deletions_completed, 1);
        assert_eq!(recovered.failures, 0);
        assert!(retry_service.detail(ID).unwrap().is_none());
    }

    #[test]
    fn persistence_gate_deferred_cleanup_is_managed_and_retains_new_quarantine() {
        const STAGING_ORPHAN: &str = "11111111-1111-4111-8111-111111111111";
        const ASSET_ORPHAN: &str = "22222222-2222-4222-8222-222222222222";

        let (temp, service, diagnostics) = fixture();
        let outside_downloads = temp.path().join("newspaper-downloads");
        std::fs::create_dir_all(&outside_downloads).unwrap();
        std::fs::write(outside_downloads.join("sentinel.txt"), b"keep-me").unwrap();

        let staging_orphan = service.layout.staging_dir().unwrap().join(STAGING_ORPHAN);
        std::fs::create_dir_all(&staging_orphan).unwrap();
        let asset_orphan = service.layout.assets_dir().unwrap().join(ASSET_ORPHAN);
        std::fs::create_dir_all(&asset_orphan).unwrap();
        let trash_orphan = service.layout.trash_dir().unwrap().join(format!("{ID}-1"));
        std::fs::create_dir_all(&trash_orphan).unwrap();
        let expired_quarantine = service
            .layout
            .quarantine_dir()
            .unwrap()
            .join("1-stale-staging-expired");
        std::fs::create_dir_all(&expired_quarantine).unwrap();

        let future = SystemTime::now()
            .checked_add(Duration::from_secs(8 * 24 * 60 * 60))
            .unwrap();
        let summary = clipping_recovery::run_deferred_cleanup_at(
            &service.db_path,
            &service.layout,
            &diagnostics,
            future,
        );

        assert_eq!(summary.processed, 4);
        assert_eq!(summary.failures, 0);
        assert!(!staging_orphan.exists());
        assert!(!asset_orphan.exists());
        assert!(!trash_orphan.exists());
        assert!(!expired_quarantine.exists());
        assert_eq!(
            std::fs::read(outside_downloads.join("sentinel.txt")).unwrap(),
            b"keep-me"
        );
        let retained: Vec<_> = std::fs::read_dir(service.layout.quarantine_dir().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(retained.len(), 2, "new quarantine entries need seven days");
        let event = diagnostics
            .snapshot()
            .into_iter()
            .find(|event| event.operation == "clipping_deferred_cleanup")
            .unwrap();
        assert_eq!(event.outcome, DatabaseDiagnosticOutcome::Ok);
        assert_eq!(event.error_class, None);
    }

    #[test]
    fn persistence_gate_cleanup_path_failure_is_reported_and_retryable() {
        let (temp, service, diagnostics) = fixture();
        let outside = temp.path().join("outside-cleanup");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("sentinel.txt"), b"keep-me").unwrap();
        let link = service
            .layout
            .staging_dir()
            .unwrap()
            .join("33333333-3333-4333-8333-333333333333");
        if !create_dir_link(&outside, &link) {
            eprintln!("directory link creation unavailable on this machine");
            return;
        }
        let future = SystemTime::now()
            .checked_add(Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();

        for _ in 0..2 {
            let summary = clipping_recovery::run_deferred_cleanup_at(
                &service.db_path,
                &service.layout,
                &diagnostics,
                future,
            );
            assert_eq!(summary.processed, 0);
            assert_eq!(summary.failures, 1);
            assert!(link.exists(), "failed item must remain retryable");
        }
        assert_eq!(
            std::fs::read(outside.join("sentinel.txt")).unwrap(),
            b"keep-me"
        );
        let events: Vec<_> = diagnostics
            .snapshot()
            .into_iter()
            .filter(|event| event.operation == "clipping_deferred_cleanup")
            .collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| {
            event.outcome == DatabaseDiagnosticOutcome::Error
                && event.error_class == Some(DatabaseErrorClass::Recovery)
        }));
    }

    #[test]
    fn persistence_gate_cleanup_attempts_are_bounded_per_category() {
        let (_temp, service, diagnostics) = fixture();
        let future = SystemTime::now()
            .checked_add(Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();
        let timestamp = future
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let staging = service.layout.staging_dir().unwrap();
        let quarantine = service.layout.quarantine_dir().unwrap();
        for index in 0..(clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8) {
            let name = format!("orphan-{index:02}");
            std::fs::create_dir_all(staging.join(&name)).unwrap();
            // Force the quarantine move to collide so failed attempts remain
            // available for a later launch.
            std::fs::create_dir_all(quarantine.join(format!("{timestamp}-stale-staging-{name}")))
                .unwrap();
        }

        let summary = clipping_recovery::run_deferred_cleanup_at(
            &service.db_path,
            &service.layout,
            &diagnostics,
            future,
        );
        assert_eq!(summary.processed, 0);
        assert_eq!(
            summary.failures,
            clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );
        assert_eq!(
            summary.max_category_enumerated,
            clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8
        );
        assert_eq!(
            summary.max_category_mutations,
            clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );
        assert_eq!(
            std::fs::read_dir(staging).unwrap().count(),
            clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8
        );
    }

    #[test]
    fn persistence_gate_cleanup_complete_enumeration_reaches_later_entries() {
        let (temp, service, diagnostics) = fixture();
        service
            .writer
            .execute(
                ClippingService::context("test_seed_retired_cleanup_cursor"),
                |connection| {
                    connection.execute(
                        "INSERT INTO newspaper_settings (key, value_json, updated_at)
                         VALUES ('clipping_cleanup_cursor_v1', 'retired-sentinel', 77)",
                        [],
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        let outside_downloads = temp.path().join("newspaper-downloads-never-enumerated");
        std::fs::create_dir(&outside_downloads).unwrap();
        std::fs::write(outside_downloads.join("sentinel.txt"), b"keep-me").unwrap();

        for index in 0..(clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8) {
            let id = format!("{index:08x}-1111-4111-8111-{index:012x}");
            service
                .register_staged(staged_record(&service, &id))
                .unwrap();
        }
        let later_orphan = service
            .layout
            .assets_dir()
            .unwrap()
            .join("ffffffff-ffff-4fff-8fff-ffffffffffff");
        std::fs::create_dir(&later_orphan).unwrap();

        let future = SystemTime::now()
            .checked_add(Duration::from_secs(8 * 24 * 60 * 60))
            .unwrap();
        let fresh_timestamp = future
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 24 * 60 * 60;
        let quarantine = service.layout.quarantine_dir().unwrap();
        for index in 0..(clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8) {
            std::fs::create_dir(quarantine.join(format!("{fresh_timestamp}-fresh-{index:04}")))
                .unwrap();
        }
        let later_expired = quarantine.join("z-expired-entry");
        std::fs::create_dir(&later_expired).unwrap();

        let first = clipping_recovery::run_deferred_cleanup_at(
            &service.db_path,
            &service.layout,
            &diagnostics,
            future,
        );
        assert!(!later_orphan.exists());
        assert!(!later_expired.exists());
        assert!(
            first.max_category_enumerated
                >= clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 9
        );
        assert!(
            first.max_category_mutations <= clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );

        let second = clipping_recovery::run_deferred_cleanup_at(
            &service.db_path,
            &service.layout,
            &diagnostics,
            future,
        );
        assert!(
            !later_orphan.exists(),
            "complete enumeration must not recreate a removed orphan"
        );
        assert!(
            !later_expired.exists(),
            "complete enumeration must not recreate removed quarantine"
        );
        assert!(
            second.max_category_enumerated
                >= clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY + 8
        );
        assert!(
            second.max_category_mutations
                <= clipping_recovery::CLEANUP_MUTATION_BUDGET_PER_CATEGORY
        );
        assert_eq!(
            std::fs::read(outside_downloads.join("sentinel.txt")).unwrap(),
            b"keep-me"
        );
        let connection = open_runtime(&service.db_path).unwrap();
        let retired_cursor: (String, i64) = connection
            .query_row(
                "SELECT value_json, updated_at FROM newspaper_settings
                 WHERE key = 'clipping_cleanup_cursor_v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retired_cursor, ("retired-sentinel".to_string(), 77));
        let diagnostics_text = format!("{:?}", diagnostics.snapshot());
        assert!(!diagnostics_text.contains(&temp.path().to_string_lossy().to_string()));
    }

    #[test]
    fn persistence_gate_production_cleanup_is_detached_from_application_setup() {
        let source = include_str!("../../lib.rs");
        let start = source
            .find("let cleanup_service = clipping_service.clone();")
            .expect("production setup must schedule clipping cleanup");
        let end = source[start..]
            .find("app.manage(diagnostics);")
            .map(|offset| start + offset)
            .expect("cleanup scheduling must finish before state management continues");
        let scheduling = &source[start..end];
        assert!(scheduling.contains("tauri::async_runtime::spawn_blocking"));
        assert!(scheduling.contains("cleanup_service.run_deferred_cleanup"));
        assert!(
            !scheduling.contains(".await"),
            "application setup must not wait for detached enumeration"
        );
    }

    fn create_dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
    }
}
