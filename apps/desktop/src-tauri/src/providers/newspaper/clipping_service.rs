//! Phase 1 orchestration boundary for clipping persistence and managed assets.
//! Image crop production belongs to Phase 2; this service accepts only a
//! complete, validated staging asset and never performs image work in a writer
//! closure.

use std::path::PathBuf;

use crate::app::database_diagnostics::{DatabaseDiagnostics, DatabaseProvider};
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
}

impl ClippingService {
    pub fn new(db_path: PathBuf, writer: DatabaseWriter, layout: ClippingAssetLayout) -> Self {
        Self {
            db_path,
            writer,
            layout,
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
        match outcome {
            repository::NoteUpdateOutcome::Updated { .. }
            | repository::NoteUpdateOutcome::Unchanged { .. } => self
                .read_by_id(id)?
                .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound)),
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
            ClippingService::new(db_path, writer, layout),
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
        service.delete(ID, created.revision).unwrap();
        assert!(service.detail(ID).unwrap().is_none());
        assert!(!service.layout.canonical_path(ID).unwrap().exists());
        assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
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
        );
        let recovered = retry_service.recover_startup(&retry_diagnostics, 102);
        assert_eq!(recovered.deletions_completed, 1);
        assert_eq!(recovered.failures, 0);
        assert!(retry_service.detail(ID).unwrap().is_none());
    }

    #[test]
    fn persistence_gate_deferred_cleanup_is_bounded_managed_and_retains_new_quarantine() {
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
        for index in 0..(clipping_recovery::CLEANUP_BUDGET_PER_CATEGORY + 8) {
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
            clipping_recovery::CLEANUP_BUDGET_PER_CATEGORY
        );
        assert_eq!(
            std::fs::read_dir(staging).unwrap().count(),
            clipping_recovery::CLEANUP_BUDGET_PER_CATEGORY + 8
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
