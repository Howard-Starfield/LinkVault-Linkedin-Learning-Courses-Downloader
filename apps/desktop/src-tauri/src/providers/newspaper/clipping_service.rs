//! Phase 1 orchestration boundary for clipping persistence and managed assets.
//! Image crop production belongs to Phase 2; this service accepts only a
//! complete, validated staging asset and never performs image work in a writer
//! closure.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::thread;
use std::time::Instant;

use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseErrorClass, DatabaseProvider,
};
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriter};
use crate::cache::open_runtime;

use super::clipping_assets::{
    ClippingAssetLayout, THUMBNAIL_CACHE_SCHEMA_VERSION, THUMBNAIL_MAX_HEIGHT, THUMBNAIL_MAX_WIDTH,
};
use super::clipping_crop;
use super::clipping_draft_repository::DraftCheckpointAck;
use super::clipping_models::{
    normalize_search_query, normalize_search_text, normalize_title, validate_asset_byte_count,
    validate_clipping_id, validate_edition_code, validate_edition_name, validate_list_limit,
    validate_note_markdown, validate_page_number, validate_publication_date, validate_sha256_hex,
    validate_source_mime, ClippingAssetState, ClippingError, ClippingErrorCode, ClippingRootStatus,
    ClippingSummary, CreateNewspaperClippingRequest, CreateNewspaperClippingResponse,
    EnsureNewspaperClippingThumbnailResponse, GetNewspaperClippingsPageRequest, NewspaperClipping,
    NewspaperClippingDetail, NewspaperClippingListQuery, NewspaperClippingMatchField,
    NewspaperClippingSearchResult, NewspaperClippingSearchSnippet,
    NewspaperClippingSearchSnippetPart, NewspaperClippingSort, NewspaperClippingsPage,
    SearchNewspaperClippingsPage, SearchNewspaperClippingsRequest,
    SearchPossibleNewspaperClippingsRequest, SearchPossibleNewspaperClippingsResponse,
    FUZZY_CANDIDATE_LIMIT, POSSIBLE_MATCH_LIMIT, SEARCH_PAGE_LIMIT, SEARCH_SNIPPET_MAX_CHARS,
};
use super::clipping_recovery;
use super::clipping_repository::{self as repository, ClippingDetail};
use super::clipping_roots::ClippingRootRegistry;

type CanonicalNoteDurability = (i64, Option<DraftCheckpointAck>);

#[derive(Clone)]
pub struct ClippingService {
    pub(super) db_path: PathBuf,
    pub(super) writer: DatabaseWriter,
    layout: ClippingAssetLayout,
    roots: ClippingRootRegistry,
    diagnostics: DatabaseDiagnostics,
    integrity_scheduler: Arc<IntegrityTransitionScheduler>,
    /// Phase 2 owns one native, full-page decode/crop/encode lane. This is a
    /// blocking mutex because callers reach it only from Tauri's blocking
    /// pool, never from the WebView-sensitive command path.
    crop_permit: Arc<Mutex<()>>,
    /// Derived thumbnails are serialized separately from full-page crop work.
    /// This prevents duplicate cache writers without blocking note updates.
    thumbnail_permit: Arc<Mutex<()>>,
    /// Shutdown first closes admission, then waits on `crop_permit` so a
    /// started operation reaches the Phase 1 recoverable state machine.
    crop_accepting: Arc<AtomicBool>,
}

pub(crate) const MEDIA_INTEGRITY_QUEUE_CAPACITY: usize = 32;

/// Whether the current staging directory is backed by a durable clipping row.
///
/// The Phase 1 create crash matrix permits immediate staging cleanup only
/// before the `creating` row exists. Once the row is present, recovery owns the
/// staging directory and can promote it or preserve the aggregate as missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StagingRegistrationState {
    Untracked,
    Tracked,
    Unknown,
}

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
        let roots = ClippingRootRegistry::new(db_path.clone(), writer.clone(), layout.clone());
        Self {
            db_path,
            writer,
            layout,
            roots,
            diagnostics,
            integrity_scheduler,
            crop_permit: Arc::new(Mutex::new(())),
            thumbnail_permit: Arc::new(Mutex::new(())),
            crop_accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn layout(&self) -> &ClippingAssetLayout {
        &self.layout
    }
    pub(crate) fn root_layout(&self, root_id: &str) -> Result<ClippingAssetLayout, ClippingError> {
        self.roots.resolve(root_id)
    }

    pub fn list_root_summaries(
        &self,
    ) -> Result<Vec<super::clipping_models::ClippingRootSummary>, ClippingError> {
        self.roots.list_summaries()
    }

    pub fn check_root(
        &self,
        root_id: &str,
        now: i64,
    ) -> Result<super::clipping_models::ClippingRootSummary, ClippingError> {
        self.roots.check(root_id, now)
    }

    pub fn reconnect_root(
        &self,
        root_id: &str,
        selected_snapshot_directory: &std::path::Path,
        now: i64,
    ) -> Result<super::clipping_models::ClippingRootSummary, ClippingError> {
        self.roots
            .reconnect(root_id, selected_snapshot_directory, now)
    }

    pub fn verified_root_open_path(&self, root_id: &str) -> Result<PathBuf, ClippingError> {
        self.roots.verified_open_path(root_id)
    }

    pub(crate) fn verify_root_fresh_for_integrity(
        &self,
        root_id: &str,
    ) -> Result<(), ClippingError> {
        self.roots.verify_fresh_for_integrity(root_id)
    }

    #[allow(dead_code)] // Consumed by the Phase 2 crop service after its storage rebase.
    pub(crate) fn register_source_job_root(
        &self,
        source_job_id: &str,
        now: i64,
    ) -> Result<super::clipping_models::ClippingRoot, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let destination = repository::load_batch_destination_for_job(&connection, source_job_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetRootUnavailable))?;
        drop(connection);
        self.roots
            .register_download_destination(std::path::Path::new(&destination), now)
    }

    #[allow(dead_code)] // Direct-path harness for isolated root tests; no command exposes it.
    pub(crate) fn register_download_destination(
        &self,
        destination: &std::path::Path,
        now: i64,
    ) -> Result<super::clipping_models::ClippingRoot, ClippingError> {
        self.roots.register_download_destination(destination, now)
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

    fn load_crop_source(
        &self,
        page_id: &str,
    ) -> Result<Option<repository::CropSourceRecord>, ClippingError> {
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        repository::load_crop_source(&connection, page_id)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))
    }

    /// Phase 2 orchestration boundary. This owns source resolution, crop
    /// staging, the required final source/version recheck, and the existing
    /// Phase 1 register/promote/ready lifecycle. The command adapter invokes
    /// it from `spawn_blocking`.
    pub fn create_newspaper_clipping(
        &self,
        request: CreateNewspaperClippingRequest,
        now: i64,
    ) -> Result<CreateNewspaperClippingResponse, ClippingError> {
        self.create_newspaper_clipping_inner(request, now, || {}, || {})
    }

    fn create_newspaper_clipping_inner<AfterPermit, AfterStaging>(
        &self,
        request: CreateNewspaperClippingRequest,
        now: i64,
        after_permit: AfterPermit,
        after_staging: AfterStaging,
    ) -> Result<CreateNewspaperClippingResponse, ClippingError>
    where
        AfterPermit: FnOnce(),
        AfterStaging: FnOnce(),
    {
        // All request checks occur before any source read or staging path
        // creation. A completed idempotent operation is still required to
        // carry a syntactically valid request/operation identifier.
        let rect = clipping_crop::validate_create_request(&request)?;
        if !self.crop_accepting.load(Ordering::Acquire) {
            return Err(ClippingError::new(ClippingErrorCode::ServiceUnavailable));
        }
        if let Some(existing) = self.read_by_id(&request.operation_id)? {
            return self.response_from_clipping(self.resolve_idempotent(existing, now)?);
        }

        // This lock is intentionally acquired before the source record or
        // source bytes are read. No database write transaction is held while
        // waiting or performing image work.
        let _permit = self
            .crop_permit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.crop_accepting.load(Ordering::Acquire) {
            return Err(ClippingError::new(ClippingErrorCode::ServiceUnavailable));
        }
        if let Some(existing) = self.read_by_id(&request.operation_id)? {
            return self.response_from_clipping(self.resolve_idempotent(existing, now)?);
        }
        after_permit();

        let source_record = self
            .load_crop_source(&request.page_id)?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourcePageNotFound))?;
        // Root registration is part of the create lifecycle and must happen
        // before staging. The destination is loaded through the persisted
        // source job; no frontend path can select the clipping authority.
        let asset_root = self.register_source_job_root(&source_record.job_id, now)?;
        let asset_layout = self.roots.resolve_for_creation(&asset_root.id)?;
        let prepared = clipping_crop::stage_crop(
            &request,
            rect,
            &source_record,
            &asset_layout,
            &self.diagnostics,
        )?;
        after_staging();

        // A media version/path/status change must not silently bind the user
        // selection to a different source after the expensive native work.
        let rechecked = self.load_crop_source(&request.page_id)?;
        if let Err(error) = clipping_crop::validate_source_recheck(
            request.expected_media_version,
            &source_record,
            rechecked.as_ref(),
            &prepared,
        ) {
            asset_layout.discard_staging(&request.operation_id);
            return Err(error);
        }

        let operation_id = request.operation_id;
        let asset_relative_path = ClippingAssetLayout::snapshot_relative_path(
            &source_record.edition_name,
            &source_record.edition_code,
            &source_record.publication_date,
            &source_record.page_number,
            &operation_id,
        )?;
        let record = repository::NewClippingRecord {
            id: operation_id,
            source_job_id: Some(source_record.job_id),
            source_page_id: Some(source_record.page_id),
            source_media_version_snapshot: source_record.media_version,
            source_kind_snapshot: prepared.source_kind,
            source_mime_type_snapshot: prepared.source_mime_type,
            source_checksum_snapshot: Some(prepared.source_checksum_sha256),
            edition_code_snapshot: source_record.edition_code,
            edition_name_snapshot: source_record.edition_name,
            publication_date_snapshot: source_record.publication_date,
            page_number_snapshot: source_record.page_number,
            source_pixel_width: prepared.source_pixel_width,
            source_pixel_height: prepared.source_pixel_height,
            crop_x: prepared.crop.x,
            crop_y: prepared.crop.y,
            crop_width: prepared.crop.width,
            crop_height: prepared.crop.height,
            asset_root_id: asset_root.id,
            asset_relative_path,
            asset_byte_count: prepared.asset_byte_count,
            asset_checksum_sha256: prepared.asset_checksum_sha256,
            title: prepared.title,
            now,
        };
        let clipping = self.register_staged(record)?;
        self.response_from_clipping(clipping)
    }

    fn response_from_clipping(
        &self,
        clipping: NewspaperClipping,
    ) -> Result<CreateNewspaperClippingResponse, ClippingError> {
        if clipping.asset_version == 0 {
            return Err(ClippingError::new(ClippingErrorCode::AssetValidationFailed));
        }
        Ok(CreateNewspaperClippingResponse {
            image_url: format!(
                "http://newspaper-media.localhost/clipping/{}?v={}",
                clipping.id, clipping.asset_version
            ),
            clipping_id: clipping.id,
            title: clipping.title,
            edition_code: clipping.edition_code_snapshot,
            edition_name: clipping.edition_name_snapshot,
            publication_date: clipping.publication_date_snapshot,
            page_number: clipping.page_number_snapshot,
            asset_version: clipping.asset_version,
            asset_width: clipping.asset_pixel_width,
            asset_height: clipping.asset_pixel_height,
            asset_byte_count: clipping.asset_byte_count,
            revision: clipping.revision,
            created_at: clipping.created_at,
        })
    }

    /// Close crop admission and wait for an in-flight command to either clean
    /// its staging operation or enter the Phase 1 tracked lifecycle before
    /// the writer is shut down.
    pub fn shutdown_crop_service(&self) {
        self.crop_accepting.store(false, Ordering::Release);
        let _permit = self
            .crop_permit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }

    pub fn register_staged(
        &self,
        record: repository::NewClippingRecord,
    ) -> Result<NewspaperClipping, ClippingError> {
        self.register_staged_inner(record, false)
    }

    fn register_staged_inner(
        &self,
        mut record: repository::NewClippingRecord,
        allow_legacy_fixture: bool,
    ) -> Result<NewspaperClipping, ClippingError> {
        let staging_id = record.id.clone();
        let staging_root_id = record.asset_root_id.clone();
        let mut registration = StagingRegistrationState::Untracked;
        let result = (|| {
            validate_record(&mut record)?;
            let existing = match self.read_by_id(&record.id) {
                Ok(existing) => existing,
                Err(error) => {
                    registration = StagingRegistrationState::Unknown;
                    return Err(error);
                }
            };
            if let Some(existing) = existing {
                registration = StagingRegistrationState::Tracked;
                return self.resolve_idempotent(existing, record.now);
            }
            let asset_layout = if allow_legacy_fixture {
                self.roots.resolve(&record.asset_root_id)?
            } else {
                self.roots.resolve_for_creation(&record.asset_root_id)?
            };
            asset_layout.verify_staging(
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
                let existing = match self.read_by_id(&record.id) {
                    Ok(existing) => existing,
                    Err(error) => {
                        registration = StagingRegistrationState::Unknown;
                        return Err(error);
                    }
                };
                if let Some(existing) = existing {
                    registration = StagingRegistrationState::Tracked;
                    return self.resolve_idempotent(existing, record.now);
                }
                return Err(ClippingError::new(ClippingErrorCode::DatabaseWriteFailed));
            }
            registration = StagingRegistrationState::Tracked;

            asset_layout.promote_staging_to(&record.id, &record.asset_relative_path)?;
            asset_layout.verify_canonical_at(
                &record.id,
                &record.asset_relative_path,
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
        })();

        if result.is_err() {
            self.cleanup_untracked_staging(&staging_root_id, &staging_id, registration);
        }
        result
    }

    fn cleanup_untracked_staging(
        &self,
        root_id: &str,
        clipping_id: &str,
        registration: StagingRegistrationState,
    ) {
        let discard = || {
            if let Ok(layout) = self.roots.resolve(root_id) {
                layout.discard_staging(clipping_id);
            }
        };
        match registration {
            StagingRegistrationState::Untracked => discard(),
            StagingRegistrationState::Tracked => {}
            StagingRegistrationState::Unknown => {
                // A failed ownership read cannot prove that staging is an
                // orphan. Retry once: clean only when no row is visible, and
                // otherwise retain a possible Creating row's recovery asset.
                if matches!(self.read_by_id(clipping_id), Ok(None)) {
                    discard();
                }
            }
        }
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
                let layout = self.roots.resolve(&existing.asset_root_id)?;
                clipping_recovery::recover_creating_id(
                    &self.db_path,
                    &self.writer,
                    &layout,
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
        self.update_note_inner(
            id,
            expected_revision,
            title,
            note_markdown,
            (now, None),
            || {},
        )
    }
    fn update_note_inner<F>(
        &self,
        id: &str,
        expected_revision: u64,
        title: &str,
        note_markdown: &str,
        durability: CanonicalNoteDurability,
        after_writer: F,
    ) -> Result<NewspaperClipping, ClippingError>
    where
        F: FnOnce(),
    {
        let (now, checkpoint) = durability;
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
                repository::update_note(
                    db,
                    &owned_id,
                    expected_revision,
                    &title,
                    &owned_note,
                    now,
                    checkpoint.as_ref(),
                )
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
        let existing = self
            .read_by_id(id)?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound))?;
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
                let asset_layout = self.roots.resolve(&existing.asset_root_id)?;
                clipping_recovery::complete_delete_pending_target(
                    &self.writer,
                    &asset_layout,
                    &self.layout,
                    &self.diagnostics,
                    id,
                    &existing.asset_relative_path,
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

    pub fn list_page(
        &self,
        request: GetNewspaperClippingsPageRequest,
    ) -> Result<NewspaperClippingsPage, ClippingError> {
        let sort = NewspaperClippingSort::from_request(&request.sort)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::InvalidId))?;
        let (items, total) = self.list(NewspaperClippingListQuery {
            query: request.query,
            sort,
            offset: request.offset,
            limit: request.limit,
        })?;
        Ok(NewspaperClippingsPage {
            items: items.into_iter().map(Into::into).collect(),
            total,
            offset: request.offset,
            limit: request.limit,
        })
    }

    pub fn detail_response(
        &self,
        id: &str,
    ) -> Result<Option<NewspaperClippingDetail>, ClippingError> {
        let Some(detail) = self.detail(id)? else {
            return Ok(None);
        };
        let storage_status = self
            .list_root_summaries()?
            .into_iter()
            .find(|root| root.root_id == detail.clipping.asset_root_id)
            .map(|root| root.status)
            .unwrap_or(ClippingRootStatus::Offline);
        Ok(Some(NewspaperClippingDetail {
            image_url: format!(
                "http://newspaper-media.localhost/clipping/{}?v={}",
                detail.clipping.id, detail.clipping.asset_version
            ),
            id: detail.clipping.id,
            title: detail.clipping.title,
            note_markdown: detail.clipping.note_markdown,
            edition_code: detail.clipping.edition_code_snapshot,
            edition_name: detail.clipping.edition_name_snapshot,
            publication_date: detail.clipping.publication_date_snapshot,
            page_number: detail.clipping.page_number_snapshot,
            source_available: detail.source_available,
            asset_state: detail.clipping.asset_state,
            asset_error_code: detail.clipping.asset_error_code,
            storage_status,
            asset_width: detail.clipping.asset_pixel_width,
            asset_height: detail.clipping.asset_pixel_height,
            revision: detail.clipping.revision,
            created_at: detail.clipping.created_at,
            updated_at: detail.clipping.updated_at,
        }))
    }

    pub fn update_note_response(
        &self,
        id: &str,
        expected_revision: u64,
        title: &str,
        note_markdown: &str,
        checkpoint: Option<DraftCheckpointAck>,
        now: i64,
    ) -> Result<NewspaperClippingDetail, ClippingError> {
        self.update_note_inner(
            id,
            expected_revision,
            title,
            note_markdown,
            (now, checkpoint),
            || {},
        )?;
        self.detail_response(id)?
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound))
    }

    pub fn ensure_thumbnail(
        &self,
        id: &str,
    ) -> Result<EnsureNewspaperClippingThumbnailResponse, ClippingError> {
        if !validate_clipping_id(id) {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        let _permit = self
            .thumbnail_permit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let clipping = self
            .read_by_id(id)?
            .filter(|clipping| clipping.asset_state.is_publicly_visible())
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::NotFound))?;
        if clipping.asset_state != ClippingAssetState::Ready {
            return Err(ClippingError::new(ClippingErrorCode::AssetMissing));
        }
        if let Ok((bytes, _)) = self
            .layout
            .read_thumbnail_for_protocol(id, clipping.asset_version)
        {
            let features = webp::BitstreamFeatures::new(&bytes)
                .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetValidationFailed))?;
            return Ok(thumbnail_response(
                id,
                clipping.asset_version,
                "ready",
                features.width(),
                features.height(),
            ));
        }

        let asset_layout = self.roots.resolve(&clipping.asset_root_id)?;
        let (canonical, _) = asset_layout.read_validated_canonical_at(
            id,
            &clipping.asset_relative_path,
            clipping.asset_byte_count,
            clipping.asset_pixel_width,
            clipping.asset_pixel_height,
            &clipping.asset_checksum_sha256,
        )?;
        let source = image::load_from_memory_with_format(&canonical, image::ImageFormat::WebP)
            .map_err(|_| ClippingError::new(ClippingErrorCode::AssetValidationFailed))?;
        // Cache density may grow between schema versions, but a derived preview
        // must never invent pixels beyond the canonical clipping dimensions.
        // DynamicImage::thumbnail computes the aspect-preserving fit before it
        // delegates to imageops::thumbnail, whose buffer-level API takes exact
        // output dimensions.
        let resized = source
            .thumbnail(
                source.width().min(THUMBNAIL_MAX_WIDTH),
                source.height().min(THUMBNAIL_MAX_HEIGHT),
            )
            .to_rgba8();
        let width = resized.width();
        let height = resized.height();
        let encoded = webp::Encoder::from_rgba(resized.as_raw(), width, height)
            .encode_lossless()
            .to_vec();
        self.layout
            .write_thumbnail_cache(id, clipping.asset_version, &encoded)?;
        Ok(thumbnail_response(
            id,
            clipping.asset_version,
            "generated",
            width,
            height,
        ))
    }

    pub fn search(
        &self,
        mut request: SearchNewspaperClippingsRequest,
    ) -> Result<SearchNewspaperClippingsPage, ClippingError> {
        if request.limit != SEARCH_PAGE_LIMIT {
            return Err(ClippingError::new(ClippingErrorCode::InvalidId));
        }
        request.query = normalize_search_query(&request.query).map_err(ClippingError::new)?;
        let note_search_applied = request.query.chars().count() >= 3;
        if request.query.is_empty() {
            return Ok(SearchNewspaperClippingsPage {
                items: Vec::new(),
                total: 0,
                offset: request.offset,
                limit: SEARCH_PAGE_LIMIT,
                note_search_applied: false,
                revision: 0,
            });
        }
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let (hits, total, revision) = repository::search_confident_clippings(
            &connection,
            &request.query,
            request.offset,
            SEARCH_PAGE_LIMIT,
        )
        .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let mut items = Vec::with_capacity(hits.len());
        for hit in hits {
            let mut matched_fields = Vec::with_capacity(5);
            let mut snippets = Vec::with_capacity(5);
            if hit.title_match {
                matched_fields.push(NewspaperClippingMatchField::Title);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Title,
                    &hit.clipping.title,
                ));
            }
            if hit.note_match {
                matched_fields.push(NewspaperClippingMatchField::Note);
                let raw = repository::load_note_search_snippet(
                    &connection,
                    &hit.clipping.id,
                    &request.query,
                )
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
                snippets.push(note_search_snippet(raw.as_deref(), &hit.clipping.excerpt));
            }
            if hit.edition_match {
                matched_fields.push(NewspaperClippingMatchField::Edition);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Edition,
                    &format!(
                        "{} ({})",
                        hit.clipping.edition_name, hit.clipping.edition_code
                    ),
                ));
            }
            if hit.date_match {
                matched_fields.push(NewspaperClippingMatchField::Date);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Date,
                    &hit.clipping.publication_date,
                ));
            }
            if hit.page_match {
                matched_fields.push(NewspaperClippingMatchField::Page);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Page,
                    &hit.clipping.page_number,
                ));
            }
            items.push(NewspaperClippingSearchResult {
                clipping: hit.clipping.into(),
                matched_fields,
                snippets,
                possible_match: false,
            });
        }
        Ok(SearchNewspaperClippingsPage {
            items,
            total,
            offset: request.offset,
            limit: SEARCH_PAGE_LIMIT,
            note_search_applied,
            revision,
        })
    }

    pub fn search_possible(
        &self,
        mut request: SearchPossibleNewspaperClippingsRequest,
    ) -> Result<SearchPossibleNewspaperClippingsResponse, ClippingError> {
        request.query = normalize_search_query(&request.query).map_err(ClippingError::new)?;
        if request.query.chars().count() < 4 {
            return Ok(SearchPossibleNewspaperClippingsResponse {
                items: Vec::new(),
                limit: POSSIBLE_MATCH_LIMIT,
                revision: 0,
            });
        }
        let connection = open_runtime(&self.db_path)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let confident_ids = repository::confident_search_ids(&connection, &request.query)
            .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let (candidates, revision) =
            repository::fuzzy_search_candidates(&connection, &request.query, FUZZY_CANDIDATE_LIMIT)
                .map_err(|_| ClippingError::new(ClippingErrorCode::DatabaseReadFailed))?;
        let mut scored = Vec::new();
        for candidate in candidates {
            if confident_ids.contains(&candidate.clipping.id) {
                continue;
            }
            let title_distance = bounded_fuzzy_distance(&request.query, &candidate.clipping.title);
            let note_distance = bounded_fuzzy_distance(&request.query, &candidate.note_window);
            let edition_value = format!(
                "{} {}",
                candidate.clipping.edition_name, candidate.clipping.edition_code
            );
            let edition_distance = bounded_fuzzy_distance(&request.query, &edition_value);
            let mut matched_fields = Vec::with_capacity(3);
            let mut snippets = Vec::with_capacity(3);
            if title_distance.is_some() {
                matched_fields.push(NewspaperClippingMatchField::Title);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Title,
                    &candidate.clipping.title,
                ));
            }
            if note_distance.is_some() {
                matched_fields.push(NewspaperClippingMatchField::Note);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Note,
                    &candidate.note_window,
                ));
            }
            if edition_distance.is_some() {
                matched_fields.push(NewspaperClippingMatchField::Edition);
                snippets.push(whole_field_snippet(
                    NewspaperClippingMatchField::Edition,
                    &edition_value,
                ));
            }
            let Some(best_distance) = [title_distance, note_distance, edition_distance]
                .into_iter()
                .flatten()
                .min()
            else {
                continue;
            };
            let field_rank = if title_distance == Some(best_distance) {
                0u8
            } else if note_distance == Some(best_distance) {
                1
            } else {
                2
            };
            scored.push((
                best_distance,
                field_rank,
                candidate.clipping.updated_at,
                candidate.clipping.id.clone(),
                NewspaperClippingSearchResult {
                    clipping: candidate.clipping.into(),
                    matched_fields,
                    snippets,
                    possible_match: true,
                },
            ));
        }
        scored.sort_by(
            |(left_distance, left_field, left_updated, left_id, _),
             (right_distance, right_field, right_updated, right_id, _)| {
                left_distance
                    .cmp(right_distance)
                    .then_with(|| left_field.cmp(right_field))
                    .then_with(|| right_updated.cmp(left_updated))
                    .then_with(|| left_id.cmp(right_id))
            },
        );
        Ok(SearchPossibleNewspaperClippingsResponse {
            items: scored
                .into_iter()
                .take(POSSIBLE_MATCH_LIMIT)
                .map(|(_, _, _, _, result)| result)
                .collect(),
            limit: POSSIBLE_MATCH_LIMIT,
            revision,
        })
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
        clipping_recovery::run_startup_recovery_roots(
            &self.db_path,
            &self.writer,
            &self.roots,
            &self.layout,
            diagnostics,
            now,
        )
    }

    pub fn run_deferred_cleanup(
        &self,
        diagnostics: &DatabaseDiagnostics,
    ) -> clipping_recovery::DeferredCleanupSummary {
        let mut summary = clipping_recovery::run_deferred_cleanup_for_root(
            &self.db_path,
            &self.layout,
            super::storage::LEGACY_CLIPPING_ROOT_ID,
            diagnostics,
        );
        let Ok(roots) = self.roots.list() else {
            summary.failures = summary.failures.saturating_add(1);
            return summary;
        };
        for root in roots {
            if root.kind != super::clipping_models::ClippingRootKind::DownloadSnapshot {
                continue;
            }
            match self.roots.resolve(&root.id) {
                Ok(layout) => summary.add(clipping_recovery::run_deferred_internal_cleanup(
                    &self.db_path,
                    &layout,
                    &root.id,
                    diagnostics,
                )),
                Err(_) => summary.failures = summary.failures.saturating_add(1),
            }
        }
        summary
    }
}

fn thumbnail_response(
    clipping_id: &str,
    asset_version: u32,
    status: &str,
    width: u32,
    height: u32,
) -> EnsureNewspaperClippingThumbnailResponse {
    let thumbnail_version = format!("{asset_version}-{THUMBNAIL_CACHE_SCHEMA_VERSION}");
    EnsureNewspaperClippingThumbnailResponse {
        status: status.to_string(),
        thumbnail_url: format!(
            "http://newspaper-media.localhost/clipping-thumbnail/{clipping_id}?v={thumbnail_version}"
        ),
        thumbnail_version,
        width,
        height,
    }
}

fn whole_field_snippet(
    field: NewspaperClippingMatchField,
    value: &str,
) -> NewspaperClippingSearchSnippet {
    NewspaperClippingSearchSnippet {
        field,
        parts: vec![NewspaperClippingSearchSnippetPart {
            text: value.chars().take(SEARCH_SNIPPET_MAX_CHARS).collect(),
            highlighted: true,
        }],
    }
}

fn note_search_snippet(raw: Option<&str>, fallback: &str) -> NewspaperClippingSearchSnippet {
    let mut parts = Vec::new();
    let mut highlighted = false;
    let mut buffer = String::new();
    let source = raw.unwrap_or(fallback);
    for character in source.chars() {
        if character == '\u{1e}' || character == '\u{1f}' {
            if !buffer.is_empty() {
                let text = repository::excerpt_from_markdown(&buffer);
                if !text.is_empty() {
                    parts.push(NewspaperClippingSearchSnippetPart { text, highlighted });
                }
                buffer.clear();
            }
            highlighted = character == '\u{1e}';
            continue;
        }
        buffer.push(character);
    }
    if !buffer.is_empty() {
        let text = repository::excerpt_from_markdown(&buffer);
        if !text.is_empty() {
            parts.push(NewspaperClippingSearchSnippetPart { text, highlighted });
        }
    }
    if parts.is_empty() {
        parts.push(NewspaperClippingSearchSnippetPart {
            text: fallback.chars().take(SEARCH_SNIPPET_MAX_CHARS).collect(),
            highlighted: false,
        });
    }
    let mut remaining = SEARCH_SNIPPET_MAX_CHARS;
    for part in &mut parts {
        let bounded: String = part.text.chars().take(remaining).collect();
        remaining = remaining.saturating_sub(bounded.chars().count());
        part.text = bounded;
    }
    parts.retain(|part| !part.text.is_empty());
    NewspaperClippingSearchSnippet {
        field: NewspaperClippingMatchField::Note,
        parts,
    }
}

fn bounded_fuzzy_distance(query: &str, candidate: &str) -> Option<usize> {
    let query: Vec<char> = query.chars().collect();
    if query.len() < 4 {
        return None;
    }
    let candidate: Vec<char> = normalize_search_text(candidate).chars().take(512).collect();
    if candidate.is_empty() {
        return None;
    }
    let limit = if query.len() <= 5 {
        1
    } else if query.len() <= 10 {
        2
    } else {
        3
    };
    let mut windows = std::collections::BTreeSet::<(usize, usize)>::new();
    if candidate.len().abs_diff(query.len()) <= limit {
        windows.insert((0, candidate.len()));
    }
    let mut token_start = None;
    for (index, character) in candidate
        .iter()
        .copied()
        .chain(std::iter::once(' '))
        .enumerate()
    {
        if character.is_alphanumeric() {
            token_start.get_or_insert(index);
        } else if let Some(start) = token_start.take() {
            let length = index.saturating_sub(start);
            if length.abs_diff(query.len()) <= limit {
                windows.insert((start, length));
            }
        }
    }
    if query.len() >= 3 && candidate.len() >= 3 {
        'query_trigrams: for query_index in 0..=query.len() - 3 {
            for candidate_index in 0..=candidate.len() - 3 {
                if query[query_index..query_index + 3]
                    == candidate[candidate_index..candidate_index + 3]
                {
                    let base = candidate_index.saturating_sub(query_index);
                    for length in query.len().saturating_sub(limit)..=query.len() + limit {
                        if base + length <= candidate.len() {
                            windows.insert((base, length));
                            if windows.len() >= 64 {
                                break 'query_trigrams;
                            }
                        }
                    }
                }
            }
        }
    }
    windows
        .into_iter()
        .filter_map(|(start, length)| {
            damerau_levenshtein_with_limit(&query, &candidate[start..start + length], limit)
        })
        .min()
}

fn damerau_levenshtein_with_limit(left: &[char], right: &[char], limit: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let infinity = limit + left.len() + right.len() + 1;
    let mut previous_previous = vec![infinity; right.len() + 1];
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![infinity; right.len() + 1];
    for i in 1..=left.len() {
        current.fill(infinity);
        current[0] = i;
        let start = i.saturating_sub(limit).max(1);
        let end = (i + limit).min(right.len());
        for j in start..=end {
            let substitution = previous[j - 1] + usize::from(left[i - 1] != right[j - 1]);
            let insertion = current[j - 1] + 1;
            let deletion = previous[j] + 1;
            let mut distance = substitution.min(insertion).min(deletion);
            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                distance = distance.min(previous_previous[j - 2] + 1);
            }
            current[j] = distance;
        }
        std::mem::swap(&mut previous_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[right.len()] <= limit).then_some(previous[right.len()])
}

#[cfg(test)]
impl ClippingService {
    pub(crate) fn register_staged_legacy_fixture(
        &self,
        record: repository::NewClippingRecord,
    ) -> Result<NewspaperClipping, ClippingError> {
        self.register_staged_inner(record, true)
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
    ClippingAssetLayout::validate_relative_path_for_id(&record.asset_relative_path, &record.id)?;
    record.title = normalize_title(&record.title).map_err(ClippingError::new)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[path = "tests.rs"]
    mod clipping_draft_service_tests;

    use super::super::clipping_assets::{encode_test_webp, sha256_hex};
    use super::super::clipping_models::{
        ClippingSourceKind, NewspaperClippingSort, NormalizedCropRect,
    };
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::{
        DatabaseDiagnosticOutcome, DatabaseDiagnostics, DatabaseErrorClass,
    };
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use rusqlite::params;
    use std::path::{Path, PathBuf};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::thread;
    use std::time::{Duration, SystemTime};

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const CROP_ID: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
    const CROP_PAGE_ID: &str = "crop_page_01";

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
            asset_root_id: crate::newspaper::storage::LEGACY_CLIPPING_ROOT_ID.to_owned(),
            asset_relative_path: ClippingAssetLayout::canonical_relative_path(id).unwrap(),
            asset_byte_count: bytes.len() as u64,
            asset_checksum_sha256: sha256_hex(&bytes),
            title: "New York · 2026-08-08 · A01".to_string(),
            now: 100,
        }
    }

    fn staged_snapshot_record(
        service: &ClippingService,
        destination: &Path,
        id: &str,
    ) -> repository::NewClippingRecord {
        staged_snapshot_record_with_dimensions(service, destination, id, 24, 16)
    }

    fn staged_snapshot_record_with_dimensions(
        service: &ClippingService,
        destination: &Path,
        id: &str,
        width: u32,
        height: u32,
    ) -> repository::NewClippingRecord {
        std::fs::create_dir_all(destination).unwrap();
        let root = service
            .register_download_destination(destination, 10)
            .unwrap();
        let layout = service.root_layout(&root.id).unwrap();
        let bytes = encode_test_webp(width, height);
        layout.write_staging(id, &bytes).unwrap();
        let mut record = staged_record(service, id);
        service.layout.discard_staging(id);
        record.source_pixel_width = width;
        record.source_pixel_height = height;
        record.crop_width = width;
        record.crop_height = height;
        record.asset_root_id = root.id;
        record.asset_relative_path = ClippingAssetLayout::snapshot_relative_path(
            &record.edition_name_snapshot,
            &record.edition_code_snapshot,
            &record.publication_date_snapshot,
            &record.page_number_snapshot,
            id,
        )
        .unwrap();
        record.asset_byte_count = bytes.len() as u64;
        record.asset_checksum_sha256 = sha256_hex(&bytes);
        record
    }

    fn crop_fixture_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([
                ((x.wrapping_mul(37) + y.wrapping_mul(13)) % 251) as u8,
                ((x.wrapping_mul(11) + y.wrapping_mul(71)) % 251) as u8,
                ((x ^ y).wrapping_mul(29) % 251) as u8,
                1 + ((x.wrapping_mul(19) + y.wrapping_mul(23)) % 254) as u8,
            ])
        }))
    }

    fn write_crop_source(root: &Path, name: &str, image: &DynamicImage) -> PathBuf {
        let path = root.join(name);
        image.save_with_format(&path, ImageFormat::Png).unwrap();
        path
    }

    fn insert_crop_source(
        service: &ClippingService,
        output_root: &Path,
        original_path: Option<&Path>,
        optimized_path: Option<&Path>,
        width: u32,
        height: u32,
        media_version: i64,
    ) {
        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_editions
                 (code, publication_date, name_zh, name_en, kind, schedule, source_url,
                  active, discovered, updated_at)
                 VALUES ('CROPTEST', '', '', 'Crop test edition', 'daily', 'daily',
                         'test://crop-edition', 1, 0, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, optimize_images,
                  optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('crop-batch', 'completed', ?1, 0, 0, 'webp_high', 1, 1, 1)",
                params![output_root.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, edition_publication_date, publication_date,
                  status, output_dir, page_count, completed_count, created_at, updated_at)
                 VALUES ('crop-job', 'crop-batch', 'CROPTEST', '', '2026-08-09',
                         'completed', ?1, 1, 1, 1, 1)",
                params![output_root.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, optimized_path, status,
                  original_bytes, final_bytes, checksum, pixel_width, pixel_height,
                  media_version, created_at, updated_at)
                 VALUES (?1, 'crop-job', 'A01', 'test://crop-page', ?2, ?3, 'completed',
                         1, 1, 'crop-source-checksum', ?4, ?5, ?6, 1, 1)",
                params![
                    CROP_PAGE_ID,
                    original_path.map(|path| path.to_string_lossy().into_owned()),
                    optimized_path.map(|path| path.to_string_lossy().into_owned()),
                    width,
                    height,
                    media_version,
                ],
            )
            .unwrap();
    }

    fn crop_request(operation_id: &str) -> CreateNewspaperClippingRequest {
        CreateNewspaperClippingRequest {
            operation_id: operation_id.to_string(),
            page_id: CROP_PAGE_ID.to_string(),
            expected_media_version: 1,
            rect: NormalizedCropRect {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            },
        }
    }

    #[test]
    fn clipping_crop_end_to_end_creates_one_ready_lossless_aggregate_without_paths() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_image = crop_fixture_image(64, 64);
        let source_path = write_crop_source(&source_root, "page.png", &source_image);
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);
        let request = crop_request(CROP_ID);

        let response = service
            .create_newspaper_clipping(request.clone(), 123)
            .unwrap();
        assert_eq!(response.clipping_id, CROP_ID);
        assert_eq!(response.asset_width, 32);
        assert_eq!(response.asset_height, 32);
        assert_eq!(response.revision, 1);
        assert_eq!(
            response.title,
            "Crop test edition \u{00b7} 2026-08-09 \u{00b7} A01"
        );
        assert!(response.image_url.contains("/clipping/"));
        assert!(!response
            .image_url
            .contains(&temp.path().to_string_lossy().to_string()));

        let clipping = service.detail(CROP_ID).unwrap().unwrap().clipping;
        assert_eq!(clipping.asset_state, ClippingAssetState::Ready);
        assert_eq!(clipping.source_kind_snapshot, ClippingSourceKind::Original);
        assert_eq!(
            clipping.asset_relative_path,
            format!(
                "Crop test edition - CROPTEST/2026-08-09/Page A01 - {CROP_ID}/clipping-v1.webp"
            )
        );
        assert_eq!((clipping.crop_x, clipping.crop_y), (16, 16));
        assert_eq!((clipping.crop_width, clipping.crop_height), (32, 32));
        assert_eq!(
            (clipping.source_pixel_width, clipping.source_pixel_height),
            (64, 64)
        );
        assert_eq!(
            clipping.source_checksum_snapshot.as_deref(),
            Some(sha256_hex(&std::fs::read(&source_path).unwrap()).as_str())
        );

        let asset_layout = service.root_layout(&clipping.asset_root_id).unwrap();
        let canonical_path = asset_layout
            .canonical_path_at(CROP_ID, &clipping.asset_relative_path)
            .unwrap();
        let bytes = std::fs::read(&canonical_path).unwrap();
        let decoded = webp::Decoder::new(&bytes).decode().unwrap();
        let expected = source_image.crop_imm(16, 16, 32, 32).to_rgba8().into_raw();
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 32);
        assert_eq!(&decoded[..], expected.as_slice());

        let retried = service.create_newspaper_clipping(request, 124).unwrap();
        assert_eq!(retried, response);
        assert_eq!(
            std::fs::read_dir(canonical_path.parent().unwrap().parent().unwrap())
                .unwrap()
                .count(),
            1,
            "an idempotent retry must not create another canonical directory"
        );
    }

    #[test]
    fn clipping_crop_same_page_and_timestamp_creates_distinct_readable_folders() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_image = crop_fixture_image(64, 64);
        let source_path = write_crop_source(&source_root, "page.png", &source_image);
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);

        let first_id = "11111111-1111-4111-8111-111111111111";
        let second_id = "22222222-2222-4222-8222-222222222222";
        service
            .create_newspaper_clipping(crop_request(first_id), 123)
            .unwrap();
        service
            .create_newspaper_clipping(crop_request(second_id), 123)
            .unwrap();

        let first = service.detail(first_id).unwrap().unwrap().clipping;
        let second = service.detail(second_id).unwrap().unwrap().clipping;
        assert_ne!(first.asset_relative_path, second.asset_relative_path);
        assert!(first
            .asset_relative_path
            .contains(&format!("/Page A01 - {first_id}/")));
        assert!(second
            .asset_relative_path
            .contains(&format!("/Page A01 - {second_id}/")));
        let layout = service.root_layout(&first.asset_root_id).unwrap();
        assert!(layout
            .canonical_path_at(first_id, &first.asset_relative_path)
            .unwrap()
            .is_file());
        assert!(layout
            .canonical_path_at(second_id, &second.asset_relative_path)
            .unwrap()
            .is_file());
    }

    #[test]
    fn clipping_crop_uses_registered_local_file_and_ignores_remote_source_url() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_image = crop_fixture_image(64, 64);
        let source_path = write_crop_source(&source_root, "local-page.png", &source_image);
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        connection
            .execute(
                "UPDATE newspaper_pages SET source_url = 'http://127.0.0.1:9/must-not-fetch.png'
                 WHERE id = ?1",
                params![CROP_PAGE_ID],
            )
            .unwrap();
        drop(connection);

        service
            .create_newspaper_clipping(crop_request(CROP_ID), 123)
            .unwrap();
        let clipping = service.detail(CROP_ID).unwrap().unwrap().clipping;
        assert_eq!(clipping.source_kind_snapshot, ClippingSourceKind::Original);
        assert_eq!(
            clipping.source_checksum_snapshot.as_deref(),
            Some(sha256_hex(&std::fs::read(&source_path).unwrap()).as_str())
        );

        let asset_layout = service.root_layout(&clipping.asset_root_id).unwrap();
        let canonical_path = asset_layout
            .canonical_path_at(CROP_ID, &clipping.asset_relative_path)
            .unwrap();
        let decoded = webp::Decoder::new(&std::fs::read(canonical_path).unwrap())
            .decode()
            .unwrap();
        let expected = source_image.crop_imm(16, 16, 32, 32).to_rgba8().into_raw();
        assert_eq!(&decoded[..], expected.as_slice());
    }

    #[test]
    fn clipping_crop_keeps_each_source_jobs_original_download_snapshot_root() {
        let (temp, service, _diagnostics) = fixture();
        let first_destination = temp.path().join("first-download-destination");
        let second_destination = temp.path().join("second-download-destination");
        std::fs::create_dir(&first_destination).unwrap();
        std::fs::create_dir(&second_destination).unwrap();
        let source_image = crop_fixture_image(64, 64);
        let first_source = write_crop_source(&first_destination, "first-page.png", &source_image);
        let second_source =
            write_crop_source(&second_destination, "second-page.png", &source_image);
        insert_crop_source(
            &service,
            &first_destination,
            Some(&first_source),
            None,
            64,
            64,
            1,
        );

        let first_id = "11111111-1111-4111-8111-111111111111";
        service
            .create_newspaper_clipping(crop_request(first_id), 123)
            .unwrap();
        let first = service.detail(first_id).unwrap().unwrap().clipping;
        let first_layout = service.root_layout(&first.asset_root_id).unwrap();
        let first_path = first_layout
            .canonical_path_at(first_id, &first.asset_relative_path)
            .unwrap();
        assert!(first_path.is_file());
        assert_eq!(
            first_layout.root(),
            first_destination
                .join("Newspaper snapshots")
                .canonicalize()
                .unwrap()
        );
        assert!(first_path.starts_with(first_layout.root()));

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, optimize_images,
                  optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('crop-batch-2', 'completed', ?1, 0, 0, 'webp_high', 1, 2, 2)",
                params![second_destination.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, edition_publication_date, publication_date,
                  status, output_dir, page_count, completed_count, created_at, updated_at)
                 VALUES ('crop-job-2', 'crop-batch-2', 'CROPTEST', '', '2026-08-10',
                         'completed', ?1, 1, 1, 2, 2)",
                params![second_destination.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, optimized_path, status,
                  original_bytes, final_bytes, checksum, pixel_width, pixel_height,
                  media_version, created_at, updated_at)
                 VALUES ('crop_page_02', 'crop-job-2', 'B02', 'test://crop-page-2', ?1, NULL,
                         'completed', 1, 1, 'crop-source-checksum-2', 64, 64, 1, 2, 2)",
                params![second_source.to_string_lossy()],
            )
            .unwrap();
        drop(connection);

        let second_id = "22222222-2222-4222-8222-222222222222";
        let mut second_request = crop_request(second_id);
        second_request.page_id = "crop_page_02".to_owned();
        service
            .create_newspaper_clipping(second_request, 124)
            .unwrap();
        let second = service.detail(second_id).unwrap().unwrap().clipping;
        let second_layout = service.root_layout(&second.asset_root_id).unwrap();
        let second_path = second_layout
            .canonical_path_at(second_id, &second.asset_relative_path)
            .unwrap();

        assert_ne!(first.asset_root_id, second.asset_root_id);
        assert!(second_path.is_file());
        assert_eq!(
            second_layout.root(),
            second_destination
                .join("Newspaper snapshots")
                .canonicalize()
                .unwrap()
        );
        assert!(second_path.starts_with(second_layout.root()));
        assert!(
            first_path.is_file(),
            "registering a later destination moved the first crop"
        );
    }

    #[test]
    fn clipping_crop_rejects_initial_stale_or_unready_pages_before_staging() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_path = write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 2);

        let stale = service.create_newspaper_clipping(crop_request(CROP_ID), 123);
        assert_eq!(stale.unwrap_err().code, ClippingErrorCode::SourceMediaStale);
        assert!(service.detail(CROP_ID).unwrap().is_none());
        assert!(!service.layout.root().join("staging").join(CROP_ID).exists());

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        connection
            .execute(
                "UPDATE newspaper_pages SET media_version = 1, status = 'pending' WHERE id = ?1",
                params![CROP_PAGE_ID],
            )
            .unwrap();
        let unready = service.create_newspaper_clipping(crop_request(CROP_ID), 124);
        assert_eq!(
            unready.unwrap_err().code,
            ClippingErrorCode::SourcePageNotReady
        );
        assert!(service.detail(CROP_ID).unwrap().is_none());
    }

    #[test]
    fn clipping_crop_rejects_invalid_repository_pixel_dimensions_before_staging() {
        // The current schema rejects non-positive page dimensions, but an old
        // or externally-corrupted SQLite database can still contain them.
        // SQLite itself also permits signed integers outside Rust's `u32`
        // range. Both stored dimension columns must fail at the repository
        // conversion boundary before crop staging can begin.
        for (operation_id, width, height) in [
            ("61111111-1111-4111-8111-111111111111", -1_i64, 64_i64),
            ("62222222-2222-4222-8222-222222222222", 64_i64, -1_i64),
            (
                "63333333-3333-4333-8333-333333333333",
                i64::from(u32::MAX) + 1,
                64_i64,
            ),
            (
                "64444444-4444-4444-8444-444444444444",
                64_i64,
                i64::from(u32::MAX) + 1,
            ),
        ] {
            let (temp, service, _diagnostics) = fixture();
            let source_root = temp.path().join("crop-source-root");
            std::fs::create_dir(&source_root).unwrap();
            let source_path =
                write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
            insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);

            let connection = rusqlite::Connection::open(&service.db_path).unwrap();
            connection
                .pragma_update(None, "ignore_check_constraints", "ON")
                .unwrap();
            connection
                .execute(
                    "UPDATE newspaper_pages
                     SET pixel_width = ?2, pixel_height = ?3
                     WHERE id = ?1",
                    params![CROP_PAGE_ID, width, height],
                )
                .unwrap();
            assert!(
                repository::load_crop_source(&connection, CROP_PAGE_ID).is_err(),
                "invalid SQLite dimensions must not convert into CropSourceRecord"
            );
            drop(connection);

            let error = service
                .create_newspaper_clipping(crop_request(operation_id), 123)
                .unwrap_err();
            assert_eq!(error.code, ClippingErrorCode::DatabaseReadFailed);
            assert!(service.detail(operation_id).unwrap().is_none());
            assert!(
                !service
                    .layout
                    .root()
                    .join("staging")
                    .join(operation_id)
                    .exists(),
                "repository conversion failure must not leave a staged asset"
            );
        }
    }

    #[test]
    fn clipping_crop_discards_staging_when_final_media_recheck_detects_version_path_or_status_change(
    ) {
        for (operation_id, mutation, expected_error) in [
            (
                "11111111-1111-4111-8111-111111111111",
                "version",
                ClippingErrorCode::SourceMediaStale,
            ),
            (
                "22222222-2222-4222-8222-222222222222",
                "path",
                ClippingErrorCode::SourceMediaStale,
            ),
            (
                "25252525-2525-4252-8252-252525252525",
                "status",
                ClippingErrorCode::SourcePageNotReady,
            ),
        ] {
            let (temp, service, _diagnostics) = fixture();
            let source_root = temp.path().join("crop-source-root");
            std::fs::create_dir(&source_root).unwrap();
            let source_path =
                write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
            let replacement_path =
                write_crop_source(&source_root, "replacement.png", &crop_fixture_image(64, 64));
            insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);
            let writer = service.writer.clone();
            let replacement = replacement_path.to_string_lossy().into_owned();
            let request = crop_request(operation_id);
            let result = service.create_newspaper_clipping_inner(
                request,
                123,
                || {},
                move || {
                    writer
                        .execute(ClippingService::context("test_crop_final_recheck"), move |db| {
                            match mutation {
                                "version" => db.execute(
                                    "UPDATE newspaper_pages SET media_version = media_version + 1 WHERE id = ?1",
                                    params![CROP_PAGE_ID],
                                )?,
                                "path" => db.execute(
                                    "UPDATE newspaper_pages SET original_path = ?2 WHERE id = ?1",
                                    params![CROP_PAGE_ID, replacement],
                                )?,
                                "status" => db.execute(
                                    "UPDATE newspaper_pages SET status = 'pending' WHERE id = ?1",
                                    params![CROP_PAGE_ID],
                                )?,
                                _ => unreachable!("fixed final-recheck test matrix"),
                            };
                            Ok(())
                        })
                        .unwrap();
                },
            );
            assert_eq!(result.unwrap_err().code, expected_error);
            assert!(service.detail(operation_id).unwrap().is_none());
            assert!(!service
                .layout
                .root()
                .join("staging")
                .join(operation_id)
                .exists());
        }
    }

    #[test]
    fn clipping_crop_serializes_distinct_requests_with_one_active_crop_section() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_path = write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_first_tx, release_first_rx) = std::sync::mpsc::sync_channel(1);
        let first_service = service.clone();
        let first_active = Arc::clone(&active);
        let first_max = Arc::clone(&max_active);
        let first = thread::spawn(move || {
            first_service.create_newspaper_clipping_inner(
                crop_request("33333333-3333-4333-8333-333333333333"),
                123,
                move || {
                    let now_active = first_active.fetch_add(1, Ordering::SeqCst) + 1;
                    first_max.fetch_max(now_active, Ordering::SeqCst);
                    first_entered_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    first_active.fetch_sub(1, Ordering::SeqCst);
                },
                || {},
            )
        });
        first_entered_rx.recv().unwrap();

        let (second_started_tx, second_started_rx) = std::sync::mpsc::sync_channel(1);
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::sync_channel(1);
        let second_service = service.clone();
        let second_active = Arc::clone(&active);
        let second_max = Arc::clone(&max_active);
        let second = thread::spawn(move || {
            second_started_tx.send(()).unwrap();
            second_service.create_newspaper_clipping_inner(
                crop_request("44444444-4444-4444-8444-444444444444"),
                124,
                move || {
                    let now_active = second_active.fetch_add(1, Ordering::SeqCst) + 1;
                    second_max.fetch_max(now_active, Ordering::SeqCst);
                    second_entered_tx.send(()).unwrap();
                    second_active.fetch_sub(1, Ordering::SeqCst);
                },
                || {},
            )
        });
        second_started_rx.recv().unwrap();
        assert!(
            matches!(
                second_entered_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "the second request entered the crop section while the first held the permit"
        );

        release_first_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second_entered_rx.recv().unwrap();
        second.join().unwrap().unwrap();
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert_eq!(
            service
                .list(NewspaperClippingListQuery {
                    query: String::new(),
                    sort: NewspaperClippingSort::UpdatedDesc,
                    offset: 0,
                    limit: 50,
                })
                .unwrap()
                .1,
            2
        );
    }

    #[test]
    fn clipping_crop_concurrent_duplicate_operation_returns_one_row_and_asset() {
        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_path = write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);
        let barrier = Arc::new(Barrier::new(3));
        let callers = [123_i64, 124]
            .into_iter()
            .map(|now| {
                let caller = service.clone();
                let ready = Arc::clone(&barrier);
                thread::spawn(move || {
                    ready.wait();
                    caller.create_newspaper_clipping(crop_request(CROP_ID), now)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let responses = callers
            .into_iter()
            .map(|caller| caller.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses[0], responses[1]);
        assert_eq!(responses[0].clipping_id, CROP_ID);
        let clipping = service.detail(CROP_ID).unwrap().unwrap().clipping;
        let asset_layout = service.root_layout(&clipping.asset_root_id).unwrap();
        let canonical_path = asset_layout
            .canonical_path_at(CROP_ID, &clipping.asset_relative_path)
            .unwrap();
        assert!(canonical_path.is_file());
        assert_eq!(
            std::fs::read_dir(canonical_path.parent().unwrap().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            service
                .list(NewspaperClippingListQuery {
                    query: String::new(),
                    sort: NewspaperClippingSort::UpdatedDesc,
                    offset: 0,
                    limit: 50,
                })
                .unwrap()
                .1,
            1
        );
    }

    #[test]
    fn clipping_crop_idempotently_recovers_creating_returns_missing_and_conflicts_delete_pending() {
        let (_temp, service, _diagnostics) = fixture();
        let creating_id = "66666666-6666-4666-8666-666666666666";
        let creating_record = staged_record(&service, creating_id);
        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        repository::insert_creating(&connection, &creating_record).unwrap();
        drop(connection);

        // An existing creating row is recovered from its owned staging asset
        // before source lookup, then returned as the same operation.
        let recovered = service
            .create_newspaper_clipping(crop_request(creating_id), 123)
            .unwrap();
        assert_eq!(recovered.clipping_id, creating_id);
        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        assert_eq!(
            repository::row_state(&connection, creating_id).unwrap(),
            Some(ClippingAssetState::Ready)
        );
        assert!(matches!(
            repository::mark_delete_pending(&connection, creating_id, 1).unwrap(),
            repository::DeleteIntentOutcome::Marked
        ));
        drop(connection);

        assert_eq!(
            service
                .create_newspaper_clipping(crop_request(creating_id), 124)
                .unwrap_err()
                .code,
            ClippingErrorCode::OperationConflict
        );
        assert!(service
            .layout
            .canonical_path(creating_id)
            .unwrap()
            .is_file());

        let missing_id = "77777777-7777-4777-8777-777777777777";
        let missing_record = staged_record(&service, missing_id);
        service.layout.discard_staging(missing_id);
        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        repository::insert_creating(&connection, &missing_record).unwrap();
        assert!(repository::mark_missing_from_creating(
            &connection,
            missing_id,
            clipping_recovery::ASSET_CREATION_INCOMPLETE,
            123,
        )
        .unwrap());
        drop(connection);

        // A terminal missing aggregate remains idempotent and never tries to
        // recrop from a potentially changed source page.
        let missing = service
            .create_newspaper_clipping(crop_request(missing_id), 124)
            .unwrap();
        assert_eq!(missing.clipping_id, missing_id);
        assert!(!service.layout.canonical_path(missing_id).unwrap().exists());
        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        assert_eq!(
            repository::row_state(&connection, missing_id).unwrap(),
            Some(ClippingAssetState::Missing)
        );
    }

    #[test]
    fn clipping_crop_shutdown_rejects_new_work_and_waits_for_accepted_creation() {
        let (rejected_temp, rejected_service, _diagnostics) = fixture();
        rejected_service.shutdown_crop_service();
        let rejected = rejected_service.create_newspaper_clipping(crop_request(CROP_ID), 123);
        assert_eq!(
            rejected.unwrap_err().code,
            ClippingErrorCode::ServiceUnavailable
        );
        assert!(!rejected_temp
            .path()
            .join("newspaper-clippings")
            .join("staging")
            .join(CROP_ID)
            .exists());

        let (temp, service, _diagnostics) = fixture();
        let source_root = temp.path().join("crop-source-root");
        std::fs::create_dir(&source_root).unwrap();
        let source_path = write_crop_source(&source_root, "page.png", &crop_fixture_image(64, 64));
        insert_crop_source(&service, &source_root, Some(&source_path), None, 64, 64, 1);
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let active_service = service.clone();
        let active = thread::spawn(move || {
            active_service.create_newspaper_clipping_inner(
                crop_request(CROP_ID),
                123,
                move || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                },
                || {},
            )
        });
        entered_rx.recv().unwrap();

        let (shutdown_started_tx, shutdown_started_rx) = std::sync::mpsc::sync_channel(1);
        let (shutdown_done_tx, shutdown_done_rx) = std::sync::mpsc::sync_channel(1);
        let shutdown_service = service.clone();
        let shutdown = thread::spawn(move || {
            shutdown_started_tx.send(()).unwrap();
            shutdown_service.shutdown_crop_service();
            shutdown_done_tx.send(()).unwrap();
        });
        shutdown_started_rx.recv().unwrap();
        assert!(
            matches!(
                shutdown_done_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "shutdown returned before the accepted crop reached a tracked state"
        );
        release_tx.send(()).unwrap();
        active.join().unwrap().unwrap();
        shutdown_done_rx.recv().unwrap();
        shutdown.join().unwrap();
        assert_eq!(
            service
                .detail(CROP_ID)
                .unwrap()
                .unwrap()
                .clipping
                .asset_state,
            ClippingAssetState::Ready
        );
        assert_eq!(
            service
                .create_newspaper_clipping(
                    crop_request("55555555-5555-4555-8555-555555555555"),
                    124
                )
                .unwrap_err()
                .code,
            ClippingErrorCode::ServiceUnavailable
        );
    }

    #[test]
    fn clipping_crop_discards_untracked_corrupt_staging_before_creating_row() {
        let (temp, service, _diagnostics) = fixture();
        let destination = temp.path().join("newspaper-downloads");
        let record = staged_snapshot_record(&service, &destination, CROP_ID);
        let asset_layout = service.root_layout(&record.asset_root_id).unwrap();
        let staging_path = asset_layout.staging_complete_path(CROP_ID).unwrap();
        let canonical_path = asset_layout
            .canonical_path_at(CROP_ID, &record.asset_relative_path)
            .unwrap();
        std::fs::write(&staging_path, b"not a webp asset").unwrap();

        assert_eq!(
            service.register_staged(record).unwrap_err().code,
            ClippingErrorCode::AssetValidationFailed
        );
        assert!(!staging_path.exists());
        assert!(!staging_path.parent().unwrap().exists());
        assert!(!canonical_path.exists());

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        assert_eq!(repository::row_state(&connection, CROP_ID).unwrap(), None);
    }

    #[test]
    fn clipping_crop_discards_untracked_staging_when_record_validation_fails() {
        let (_temp, service, _diagnostics) = fixture();
        let mut record = staged_record(&service, CROP_ID);
        let staging_path = service.layout.staging_complete_path(CROP_ID).unwrap();
        record.asset_byte_count = 0;

        assert_eq!(
            service.register_staged(record).unwrap_err().code,
            ClippingErrorCode::AssetValidationFailed
        );
        assert!(!staging_path.exists());
        assert!(!staging_path.parent().unwrap().exists());

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        assert_eq!(repository::row_state(&connection, CROP_ID).unwrap(), None);
    }

    #[test]
    fn clipping_crop_discards_untracked_staging_when_creating_insert_is_rejected() {
        let (temp, service, _diagnostics) = fixture();
        let destination = temp.path().join("newspaper-downloads");
        let record = staged_snapshot_record(&service, &destination, CROP_ID);
        let asset_layout = service.root_layout(&record.asset_root_id).unwrap();
        let staging_path = asset_layout.staging_complete_path(CROP_ID).unwrap();
        let canonical_path = asset_layout
            .canonical_path_at(CROP_ID, &record.asset_relative_path)
            .unwrap();
        service.writer.shutdown().unwrap();

        assert_eq!(
            service.register_staged(record).unwrap_err().code,
            ClippingErrorCode::DatabaseWriteFailed
        );
        assert!(!staging_path.exists());
        assert!(!staging_path.parent().unwrap().exists());
        assert!(!canonical_path.exists());

        let connection = rusqlite::Connection::open(&service.db_path).unwrap();
        assert_eq!(repository::row_state(&connection, CROP_ID).unwrap(), None);
    }

    #[test]
    fn persistence_gate_clipping_creation_is_ready_and_idempotent() {
        let (_temp, service, _diagnostics) = fixture();
        let record = staged_record(&service, ID);
        let created = service
            .register_staged_legacy_fixture(record.clone())
            .unwrap();
        assert_eq!(created.asset_state, ClippingAssetState::Ready);
        assert!(service.layout.canonical_path(ID).unwrap().is_file());
        let retried = service.register_staged_legacy_fixture(record).unwrap();
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
    fn persistence_gate_snapshot_root_creation_promotes_to_visible_nested_path() {
        let (temp, service, _diagnostics) = fixture();
        let destination = temp.path().join("newspaper-downloads");
        let record = staged_snapshot_record(&service, &destination, ID);
        let root_id = record.asset_root_id.clone();
        let relative = record.asset_relative_path.clone();

        let clipping = service.register_staged(record).unwrap();

        assert_eq!(clipping.asset_root_id, root_id);
        assert_eq!(clipping.asset_relative_path, relative);
        let layout = service.root_layout(&root_id).unwrap();
        assert!(layout.canonical_path_at(ID, &relative).unwrap().is_file());
        assert!(destination
            .join(super::super::clipping_roots::SNAPSHOT_DIRECTORY_NAME)
            .join(".linkvault")
            .join("clipping-root-v1.json")
            .is_file());
        assert!(!service.layout.canonical_path(ID).unwrap().exists());
    }

    #[test]
    fn persistence_gate_new_creation_rejects_legacy_managed_root() {
        let (_temp, service, _diagnostics) = fixture();
        let error = service
            .register_staged(staged_record(&service, ID))
            .unwrap_err();
        assert_eq!(error.code, ClippingErrorCode::AssetRootUnavailable);
        assert!(service.read_by_id(ID).unwrap().is_none());
        service.layout.discard_staging(ID);
    }

    #[test]
    fn clipping_ui_contract_lists_details_updates_and_generates_only_cached_thumbnail() {
        let (temp, service, _diagnostics) = fixture();
        let destination = temp.path().join("newspaper-downloads");
        let created = service
            .register_staged(staged_snapshot_record(&service, &destination, ID))
            .unwrap();

        let page = service
            .list_page(GetNewspaperClippingsPageRequest {
                query: String::new(),
                sort: "updated_desc".to_string(),
                offset: 0,
                limit: 50,
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, ID);
        assert!(page.items[0].note_excerpt.is_empty());

        let detail = service.detail_response(ID).unwrap().unwrap();
        assert_eq!(detail.revision, created.revision);
        assert!(detail.image_url.ends_with(&format!("/clipping/{ID}?v=1")));
        assert_eq!(detail.storage_status, ClippingRootStatus::Unchecked);

        let updated = service
            .update_note_response(ID, created.revision, "  Evidence  ", "中文 note", None, 101)
            .unwrap();
        assert_eq!(updated.title, "Evidence");
        assert_eq!(updated.note_markdown, "中文 note");
        assert_eq!(updated.revision, created.revision + 1);

        let thumbnail = service.ensure_thumbnail(ID).unwrap();
        assert_eq!(thumbnail.status, "generated");
        assert!(thumbnail.width <= THUMBNAIL_MAX_WIDTH);
        assert!(thumbnail.height <= THUMBNAIL_MAX_HEIGHT);
        assert_eq!(thumbnail.width, created.asset_pixel_width);
        assert_eq!(thumbnail.height, created.asset_pixel_height);
        assert_eq!(thumbnail.thumbnail_version, "1-2");
        assert!(service
            .layout
            .thumbnail_path(ID, created.asset_version)
            .unwrap()
            .is_file());
        let cached = service.ensure_thumbnail(ID).unwrap();
        assert_eq!(cached.status, "ready");
        assert_eq!(cached.thumbnail_version, thumbnail.thumbnail_version);

        let canonical = service
            .root_layout(&created.asset_root_id)
            .unwrap()
            .canonical_path_at(ID, &created.asset_relative_path)
            .unwrap();
        assert!(
            canonical.is_file(),
            "thumbnail generation must retain canonical media"
        );
    }

    #[test]
    fn clipping_thumbnail_cache_increases_density_without_upscaling_canonical_media() {
        let (temp, service, _diagnostics) = fixture();
        let destination = temp.path().join("newspaper-downloads");
        let created = service
            .register_staged(staged_snapshot_record_with_dimensions(
                &service,
                &destination,
                ID,
                1600,
                900,
            ))
            .unwrap();

        let thumbnail = service.ensure_thumbnail(ID).unwrap();
        assert_eq!(thumbnail.status, "generated");
        assert_eq!((thumbnail.width, thumbnail.height), (1024, 576));
        assert_eq!(thumbnail.thumbnail_version, "1-2");

        let thumbnail_bytes = std::fs::read(
            service
                .layout
                .thumbnail_path(ID, created.asset_version)
                .unwrap(),
        )
        .unwrap();
        let thumbnail_image = webp::Decoder::new(&thumbnail_bytes).decode().unwrap();
        assert_eq!(
            (thumbnail_image.width(), thumbnail_image.height()),
            (1024, 576)
        );

        let canonical_path = service
            .root_layout(&created.asset_root_id)
            .unwrap()
            .canonical_path_at(ID, &created.asset_relative_path)
            .unwrap();
        let canonical_image = webp::Decoder::new(&std::fs::read(canonical_path).unwrap())
            .decode()
            .unwrap();
        assert_eq!(
            (canonical_image.width(), canonical_image.height()),
            (1600, 900)
        );
    }

    #[test]
    fn persistence_gate_clipping_update_rejects_stale_revision() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
                (101, None),
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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

    fn create_search_fixture(
        service: &ClippingService,
        id: &str,
        title: &str,
        note: &str,
        now: i64,
    ) {
        let mut record = staged_record(service, id);
        record.now = now;
        let created = service.register_staged_legacy_fixture(record).unwrap();
        service
            .update_note(id, created.revision, title, note, now + 1)
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn create_search_fixture_with_snapshots(
        service: &ClippingService,
        id: &str,
        title: &str,
        note: &str,
        edition_code: &str,
        edition_name: &str,
        publication_date: &str,
        page_number: &str,
        now: i64,
    ) {
        let mut record = staged_record(service, id);
        record.title = title.to_owned();
        record.edition_code_snapshot = edition_code.to_owned();
        record.edition_name_snapshot = edition_name.to_owned();
        record.publication_date_snapshot = publication_date.to_owned();
        record.page_number_snapshot = page_number.to_owned();
        record.now = now;
        let created = service.register_staged_legacy_fixture(record).unwrap();
        if !note.is_empty() {
            service
                .update_note(id, created.revision, title, note, now + 1)
                .unwrap();
        }
    }

    fn seed_ready_search_rows(service: &ClippingService, count: usize) {
        let mut template = staged_record(service, ID);
        service.layout.discard_staging(ID);
        let connection = open_runtime(&service.db_path).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        for index in 0..count {
            let id = format!("b0000000-0000-4000-8000-{index:012}");
            template.id = id.clone();
            template.asset_relative_path =
                ClippingAssetLayout::canonical_relative_path(&id).unwrap();
            template.title = format!("Scale keyword {index:03}");
            template.now = 10_000 + index as i64;
            repository::insert_creating(&connection, &template).unwrap();
            assert!(repository::mark_ready_from_creating(&connection, &id, template.now).unwrap());
        }
        connection.execute_batch("COMMIT").unwrap();
    }

    fn search_request(query: &str) -> SearchNewspaperClippingsRequest {
        SearchNewspaperClippingsRequest {
            query: query.to_owned(),
            offset: 0,
            limit: SEARCH_PAGE_LIMIT,
        }
    }

    #[test]
    fn clipping_search_ranks_exact_prefix_then_note_and_excludes_short_note_queries() {
        let (_temp, service, _diagnostics) = fixture();
        let exact_id = "11111111-1111-4111-8111-111111111111";
        let prefix_id = "22222222-2222-4222-8222-222222222222";
        let note_id = "33333333-3333-4333-8333-333333333333";
        create_search_fixture(&service, exact_id, "Needle", "", 100);
        create_search_fixture(&service, prefix_id, "Needle prefix", "", 200);
        create_search_fixture(
            &service,
            note_id,
            "Unrelated title",
            "A safe **needle** note body",
            300,
        );

        let page = service.search(search_request("needle")).unwrap();
        assert_eq!(page.total, 3);
        assert!(page.note_search_applied);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.clipping.id.as_str())
                .collect::<Vec<_>>(),
            vec![exact_id, prefix_id, note_id]
        );
        assert_eq!(
            page.items[2].matched_fields,
            vec![NewspaperClippingMatchField::Note]
        );
        assert!(page.items[2].snippets[0]
            .parts
            .iter()
            .any(|part| part.highlighted && part.text.contains("needle")));

        let short = service.search(search_request("dl")).unwrap();
        assert!(!short.note_search_applied);
        assert_eq!(short.total, 2);
        assert!(short.items.iter().all(|item| !item
            .matched_fields
            .contains(&NewspaperClippingMatchField::Note)));
    }

    #[test]
    fn clipping_search_golden_ranking_is_weighted_stable_and_unicode_aware() {
        let (_temp, service, _diagnostics) = fixture();
        let exact_id = "10000000-0000-4000-8000-000000000001";
        let prefix_id = "10000000-0000-4000-8000-000000000002";
        let title_id = "10000000-0000-4000-8000-000000000003";
        let note_id = "10000000-0000-4000-8000-000000000004";
        let edition_a_id = "10000000-0000-4000-8000-000000000005";
        let edition_b_id = "10000000-0000-4000-8000-000000000006";
        create_search_fixture(&service, exact_id, "Needle", "", 100);
        create_search_fixture(&service, prefix_id, "Needle bulletin", "", 100);
        create_search_fixture(&service, title_id, "Daily needle bulletin", "", 100);
        create_search_fixture(
            &service,
            note_id,
            "Daily report",
            "Daily needle bulletin",
            100,
        );
        create_search_fixture_with_snapshots(
            &service,
            edition_a_id,
            "Daily report",
            "",
            "NA",
            "Needle Gazette",
            "2026-08-08",
            "A01",
            100,
        );
        create_search_fixture_with_snapshots(
            &service,
            edition_b_id,
            "Daily report",
            "",
            "NB",
            "Needle Gazette",
            "2026-08-08",
            "A01",
            100,
        );

        let page = service.search(search_request("needle")).unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.clipping.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                exact_id,
                prefix_id,
                title_id,
                note_id,
                edition_a_id,
                edition_b_id,
            ]
        );

        let chinese_title_id = "20000000-0000-4000-8000-000000000001";
        let chinese_note_id = "20000000-0000-4000-8000-000000000002";
        create_search_fixture(&service, chinese_title_id, "今日中文摘要", "", 200);
        create_search_fixture(
            &service,
            chinese_note_id,
            "今日报道",
            "这是一份中文摘要",
            200,
        );
        let chinese = service.search(search_request("中文摘要")).unwrap();
        assert_eq!(
            chinese
                .items
                .iter()
                .map(|item| item.clipping.id.as_str())
                .collect::<Vec<_>>(),
            vec![chinese_title_id, chinese_note_id]
        );
    }

    #[test]
    fn clipping_search_returns_cumulative_fields_and_bounded_plain_text_parts() {
        let (_temp, service, _diagnostics) = fixture();
        let id = "30000000-0000-4000-8000-000000000001";
        create_search_fixture_with_snapshots(
            &service,
            id,
            "2026 clipping",
            "**2026** [reference](https://example.invalid/secret) <script>inert</script>",
            "Y2026",
            "Chronicle 2026",
            "2026-08-08",
            "P2026",
            100,
        );

        let page = service.search(search_request("2026")).unwrap();
        assert_eq!(page.total, 1);
        let result = &page.items[0];
        assert_eq!(
            result.matched_fields,
            vec![
                NewspaperClippingMatchField::Title,
                NewspaperClippingMatchField::Note,
                NewspaperClippingMatchField::Edition,
                NewspaperClippingMatchField::Date,
                NewspaperClippingMatchField::Page,
            ]
        );
        let note = result
            .snippets
            .iter()
            .find(|snippet| snippet.field == NewspaperClippingMatchField::Note)
            .unwrap();
        assert!(note.parts.iter().any(|part| part.highlighted));
        assert!(
            note.parts
                .iter()
                .map(|part| part.text.chars().count())
                .sum::<usize>()
                <= SEARCH_SNIPPET_MAX_CHARS
        );
        assert!(note
            .parts
            .iter()
            .all(|part| !part.text.contains("https://example.invalid/secret")));
        assert!(note
            .parts
            .iter()
            .all(|part| !part.text.contains('\u{1e}') && !part.text.contains('\u{1f}')));
    }

    #[test]
    fn clipping_search_normalizes_width_and_treats_like_and_fts_syntax_literally() {
        let (_temp, service, _diagnostics) = fixture();
        let width_id = "44444444-4444-4444-8444-444444444444";
        let literal_id = "55555555-5555-4555-8555-555555555555";
        create_search_fixture(&service, width_id, "ＬｉｎｋＶａｕｌｔ digest", "", 100);
        create_search_fixture(
            &service,
            literal_id,
            "100% AND _ \"quoted\" path\\name O'Brien OR NOT",
            "operator OR NOT is plain content",
            200,
        );

        let normalized = service.search(search_request("linkvault")).unwrap();
        assert_eq!(normalized.total, 1);
        assert_eq!(normalized.items[0].clipping.id, width_id);
        assert_eq!(
            normalized.items[0].matched_fields,
            vec![NewspaperClippingMatchField::Title]
        );

        for literal in ["%", "_", "AND", "\"quoted\"", "\\", "O'Brien", "OR NOT"] {
            let page = service.search(search_request(literal)).unwrap();
            assert!(page.items.iter().any(|item| item.clipping.id == literal_id));
        }
    }

    #[test]
    fn clipping_search_never_fabricates_a_highlight_for_normalized_only_note_hits() {
        let (_temp, service, _diagnostics) = fixture();
        let id = "50000000-0000-4000-8000-000000000001";
        let note = format!("{}ＬｉｎｋＶａｕｌｔ", "prefix ".repeat(1_000));
        create_search_fixture(&service, id, "Unrelated", &note, 100);

        let page = service.search(search_request("linkvault")).unwrap();
        assert_eq!(page.total, 1);
        let result = &page.items[0];
        assert_eq!(
            result.matched_fields,
            vec![NewspaperClippingMatchField::Note]
        );
        let snippet = result
            .snippets
            .iter()
            .find(|snippet| snippet.field == NewspaperClippingMatchField::Note)
            .unwrap();
        assert!(snippet.parts.iter().all(|part| !part.highlighted));
        assert!(
            snippet
                .parts
                .iter()
                .map(|part| part.text.chars().count())
                .sum::<usize>()
                <= SEARCH_SNIPPET_MAX_CHARS
        );
    }

    #[test]
    fn clipping_possible_matches_are_separate_bounded_and_never_fuzz_date_or_page() {
        let (_temp, service, _diagnostics) = fixture();
        let fuzzy_title_id = "66666666-6666-4666-8666-666666666666";
        let confident_id = "77777777-7777-4777-8777-777777777777";
        let fuzzy_note_id = "88888888-8888-4888-8888-888888888888";
        let chinese_id = "99999999-9999-4999-8999-999999999999";
        create_search_fixture(&service, fuzzy_title_id, "Needle", "", 100);
        create_search_fixture(&service, confident_id, "Needel", "", 200);
        create_search_fixture(
            &service,
            fuzzy_note_id,
            "Unrelated",
            "A planetary research note",
            300,
        );
        create_search_fixture(&service, chinese_id, "今日中文摘要", "", 400);

        let short = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "nee".to_owned(),
            })
            .unwrap();
        assert!(short.items.is_empty());
        assert_eq!(short.limit, POSSIBLE_MATCH_LIMIT);

        let title = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "needel".to_owned(),
            })
            .unwrap();
        assert!(title.items.iter().any(|item| {
            item.clipping.id == fuzzy_title_id
                && item.possible_match
                && item
                    .matched_fields
                    .contains(&NewspaperClippingMatchField::Title)
        }));
        assert!(!title
            .items
            .iter()
            .any(|item| item.clipping.id == confident_id));

        let note = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "planetery".to_owned(),
            })
            .unwrap();
        assert!(note.items.iter().any(|item| {
            item.clipping.id == fuzzy_note_id
                && item.matched_fields == vec![NewspaperClippingMatchField::Note]
        }));

        let chinese = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "今日中文提要".to_owned(),
            })
            .unwrap();
        assert!(chinese.items.iter().any(|item| {
            item.clipping.id == chinese_id
                && item
                    .matched_fields
                    .contains(&NewspaperClippingMatchField::Title)
        }));

        let date = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "2027-08-08".to_owned(),
            })
            .unwrap();
        assert!(date
            .items
            .iter()
            .all(|item| !item.matched_fields.iter().any(|field| matches!(
                field,
                NewspaperClippingMatchField::Date | NewspaperClippingMatchField::Page
            ))));
    }

    #[test]
    fn clipping_fuzzy_distance_handles_transposition_and_unicode_substitution() {
        assert_eq!(bounded_fuzzy_distance("needel", "needle"), Some(1));
        assert_eq!(bounded_fuzzy_distance("新闻摘要", "新闻精要"), Some(1));
        assert_eq!(bounded_fuzzy_distance("needel", "entirely unrelated"), None);
    }

    #[test]
    fn clipping_search_index_failure_rolls_back_canonical_note_update() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged_legacy_fixture(staged_record(&service, ID))
            .unwrap();
        let connection = open_runtime(&service.db_path).unwrap();
        connection
            .execute("DROP TABLE newspaper_clippings_normalized_fts", [])
            .unwrap();
        drop(connection);

        let error = service
            .update_note(ID, created.revision, "Must roll back", "lost update", 200)
            .unwrap_err();
        assert_eq!(error.code, ClippingErrorCode::DatabaseWriteFailed);
        let unchanged = service.detail(ID).unwrap().unwrap().clipping;
        assert_eq!(unchanged.title, created.title);
        assert_eq!(unchanged.note_markdown, created.note_markdown);
        assert_eq!(unchanged.revision, created.revision);

        let connection = open_runtime(&service.db_path).unwrap();
        super::super::storage::repair_clipping_search_index(&connection).unwrap();
        drop(connection);
        let retried = service
            .update_note(ID, created.revision, "Recovered", "indexed", 201)
            .unwrap();
        assert_eq!(retried.title, "Recovered");
        assert_eq!(service.search(search_request("indexed")).unwrap().total, 1);
    }

    #[test]
    fn clipping_search_revision_changes_for_same_timestamp_note_commits() {
        let (_temp, service, _diagnostics) = fixture();
        let created = service
            .register_staged_legacy_fixture(staged_record(&service, ID))
            .unwrap();
        let before = service.search(search_request("new york")).unwrap();
        let updated = service
            .update_note(
                ID,
                created.revision,
                "New York updated",
                "new york note",
                created.updated_at,
            )
            .unwrap();
        assert_eq!(updated.updated_at, created.updated_at);
        let after = service.search(search_request("new york")).unwrap();
        assert_ne!(after.revision, before.revision);
        assert!(after.revision < (1i64 << 53));
    }

    #[test]
    fn clipping_possible_match_uses_only_a_bounded_window_from_a_maximum_note() {
        let (_temp, service, _diagnostics) = fixture();
        let id = "40000000-0000-4000-8000-000000000001";
        let suffix = " planetary research";
        let note = format!(
            "{}{}",
            "x".repeat(super::super::clipping_models::NOTE_MAX_UTF8_BYTES - suffix.len()),
            suffix
        );
        assert_eq!(
            note.len(),
            super::super::clipping_models::NOTE_MAX_UTF8_BYTES
        );
        create_search_fixture(&service, id, "Unrelated", &note, 100);

        let search_started = Instant::now();
        let possible = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "planetery".to_owned(),
            })
            .unwrap();
        let search_elapsed = search_started.elapsed();
        let result = possible
            .items
            .iter()
            .find(|item| item.clipping.id == id)
            .unwrap();
        let note_snippet = result
            .snippets
            .iter()
            .find(|snippet| snippet.field == NewspaperClippingMatchField::Note)
            .unwrap();
        assert!(
            note_snippet
                .parts
                .iter()
                .map(|part| part.text.chars().count())
                .sum::<usize>()
                <= SEARCH_SNIPPET_MAX_CHARS
        );
        assert!(result.clipping.note_excerpt.len() <= 160);
        eprintln!(
            "clipping_possible_profile note_bytes={} candidate_cap={} returned={} elapsed_ms={:.3}",
            note.len(),
            FUZZY_CANDIDATE_LIMIT,
            possible.items.len(),
            search_elapsed.as_secs_f64() * 1_000.0
        );
    }

    #[test]
    fn clipping_search_profiles_zero_one_and_five_hundred_rows_without_page_overlap() {
        let (_temp, service, _diagnostics) = fixture();
        assert_eq!(service.search(search_request("scale")).unwrap().total, 0);
        let seed_started = Instant::now();
        seed_ready_search_rows(&service, 500);
        let seed_elapsed = seed_started.elapsed();

        let first_started = Instant::now();
        let first = service.search(search_request("scale")).unwrap();
        let first_elapsed = first_started.elapsed();
        let last_started = Instant::now();
        let last = service
            .search(SearchNewspaperClippingsRequest {
                query: "scale".to_owned(),
                offset: 450,
                limit: SEARCH_PAGE_LIMIT,
            })
            .unwrap();
        let last_elapsed = last_started.elapsed();
        assert_eq!(first.total, 500);
        assert_eq!(first.items.len(), 50);
        assert_eq!(last.items.len(), 50);
        let first_ids: HashSet<_> = first
            .items
            .iter()
            .map(|item| item.clipping.id.as_str())
            .collect();
        assert!(last
            .items
            .iter()
            .all(|item| !first_ids.contains(item.clipping.id.as_str())));
        let one = service
            .search(SearchNewspaperClippingsRequest {
                query: "scale".to_owned(),
                offset: 499,
                limit: SEARCH_PAGE_LIMIT,
            })
            .unwrap();
        assert_eq!(one.items.len(), 1);
        eprintln!(
            "clipping_search_profile rows=500 seed_ms={:.3} first_page_ms={:.3} deep_page_ms={:.3}",
            seed_elapsed.as_secs_f64() * 1_000.0,
            first_elapsed.as_secs_f64() * 1_000.0,
            last_elapsed.as_secs_f64() * 1_000.0
        );
    }

    #[test]
    fn clipping_search_pages_fifty_and_caps_possible_matches_at_twenty_five() {
        let (_temp, service, _diagnostics) = fixture();
        for index in 0..51 {
            let id = format!("90000000-0000-4000-8000-{index:012}");
            create_search_fixture(
                &service,
                &id,
                &format!("Batch keyword {index:02}"),
                "",
                100 + index,
            );
        }
        let first = service.search(search_request("batch")).unwrap();
        let second = service
            .search(SearchNewspaperClippingsRequest {
                query: "batch".to_owned(),
                offset: 50,
                limit: SEARCH_PAGE_LIMIT,
            })
            .unwrap();
        assert_eq!(first.total, 51);
        assert_eq!(first.items.len(), 50);
        assert_eq!(second.items.len(), 1);
        let first_ids: HashSet<_> = first
            .items
            .iter()
            .map(|item| item.clipping.id.as_str())
            .collect();
        assert!(!first_ids.contains(second.items[0].clipping.id.as_str()));

        for index in 0..30 {
            let id = format!("a0000000-0000-4000-8000-{index:012}");
            create_search_fixture(
                &service,
                &id,
                &format!("Newspaper result {index:02}"),
                "",
                1_000 + index,
            );
        }
        let possible = service
            .search_possible(SearchPossibleNewspaperClippingsRequest {
                query: "newspapr".to_owned(),
            })
            .unwrap();
        assert_eq!(possible.items.len(), POSSIBLE_MATCH_LIMIT);
        assert!(possible.items.iter().all(|item| item.possible_match));
        assert_eq!(
            possible
                .items
                .iter()
                .map(|item| item.clipping.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            POSSIBLE_MATCH_LIMIT
        );
    }

    #[test]
    fn persistence_gate_clipping_delete_removes_only_aggregate_asset() {
        let (temp, service, _diagnostics) = fixture();
        let sentinel = temp.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"keep").unwrap();
        let created = service
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
        assert_eq!(service.search(search_request("new york")).unwrap().total, 0);
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
            .unwrap();
        let other_id = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
        service
            .register_staged_legacy_fixture(staged_record(&service, other_id))
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
            .register_staged_legacy_fixture(staged_record(&service, ID))
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
                .register_staged_legacy_fixture(staged_record(&service, &id))
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
    fn persistence_gate_production_cleanup_is_delayed_and_detached_from_application_setup() {
        let setup = include_str!("../../lib.rs");
        let startup = include_str!("clipping_startup.rs");
        assert!(setup.contains("recover_and_schedule_reconciliation"));
        assert!(!setup.contains("run_deferred_cleanup"));
        assert!(startup.contains("tokio::time::sleep(STARTUP_FOLDER_RECONCILIATION_DELAY).await"));
        assert!(startup.contains("tauri::async_runtime::spawn_blocking"));
        assert!(startup.contains("service.run_deferred_cleanup"));
        assert!(
            startup.find("tokio::time::sleep").unwrap() < startup.find("spawn_blocking").unwrap(),
            "managed-folder enumeration must begin only after the startup quiet period"
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
