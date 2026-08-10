use super::*;
use crate::newspaper::clipping_draft_service::{
    CheckpointClippingNoteRequest, ClaimClippingNoteRecoveryRequest, ClippingNoteRecoveryResponse,
    ClippingNoteRecoveryStatus, DiscardClippingNoteRecoveryRequest,
    LoadClippingNoteRecoveryRequest, RECOVERY_MARKDOWN_MAX_BYTES, RECOVERY_TITLE_MAX_BYTES,
};

const SESSION_A: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
const SESSION_B: &str = "16fd2706-8baf-433b-82eb-8c7fada847da";

fn checkpoint(
    service: &ClippingService,
    session: &str,
    sequence: u64,
    base_revision: u64,
    title: &str,
    markdown: &str,
) -> Result<crate::newspaper::clipping_draft_service::ClippingNoteCheckpointAck, ClippingError> {
    service.draft_service().checkpoint(
        CheckpointClippingNoteRequest {
            clipping_id: ID.to_string(),
            base_revision,
            writer_session_id: session.to_string(),
            writer_sequence: sequence,
            title: title.to_string(),
            markdown: markdown.to_string(),
        },
        200 + sequence as i64,
    )
}

fn load(service: &ClippingService) -> ClippingNoteRecoveryResponse {
    service
        .draft_service()
        .load(&LoadClippingNoteRecoveryRequest {
            clipping_id: ID.to_string(),
        })
        .unwrap()
}

#[test]
fn clipping_note_recovery_orders_sessions_without_mutating_canonical_or_search() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    let before = service.detail(ID).unwrap().unwrap().clipping;

    checkpoint(
        &service,
        SESSION_A,
        1,
        created.revision,
        "draft",
        "recoveryonlyterm",
    )
    .unwrap();
    checkpoint(&service, SESSION_A, 2, created.revision, "newer", "second").unwrap();
    checkpoint(&service, SESSION_A, 2, created.revision, "newer", "second").unwrap();
    assert_eq!(
        checkpoint(&service, SESSION_A, 1, created.revision, "stale", "stale")
            .unwrap_err()
            .code,
        ClippingErrorCode::RecoveryStaleSequence
    );
    assert_eq!(
        checkpoint(&service, SESSION_B, 3, created.revision, "other", "other")
            .unwrap_err()
            .code,
        ClippingErrorCode::RecoveryWriterConflict
    );

    let after = service.detail(ID).unwrap().unwrap().clipping;
    assert_eq!(
        (
            after.title,
            after.note_markdown,
            after.revision,
            after.updated_at
        ),
        (
            before.title,
            before.note_markdown,
            before.revision,
            before.updated_at
        )
    );
    let connection = open_runtime(&service.db_path).unwrap();
    let indexed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_clippings_fts
             WHERE newspaper_clippings_fts MATCH 'recoveryonlyterm'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0, "recovery content must not enter canonical FTS");
}

#[test]
fn clipping_note_recovery_load_claim_and_discard_are_exact_identity_operations() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(&service, SESSION_A, 4, created.revision, "draft", "body").unwrap();
    let loaded = load(&service);
    assert_eq!(loaded.status, ClippingNoteRecoveryStatus::Matching);
    assert_eq!(loaded.identity.as_ref().unwrap().writer_sequence, 4);

    let claimed = service
        .draft_service()
        .claim(ClaimClippingNoteRecoveryRequest {
            clipping_id: ID.to_string(),
            prior_writer_session_id: SESSION_A.to_string(),
            prior_writer_sequence: 4,
            writer_session_id: SESSION_B.to_string(),
        })
        .unwrap();
    let identity = claimed.identity.unwrap();
    assert_eq!(identity.writer_session_id, SESSION_B);
    assert_eq!(identity.writer_sequence, 1);
    assert_eq!(
        checkpoint(&service, SESSION_A, 5, created.revision, "lost", "lost")
            .unwrap_err()
            .code,
        ClippingErrorCode::RecoveryWriterConflict
    );
    checkpoint(&service, SESSION_B, 2, created.revision, "owned", "owned").unwrap();

    let stale = service
        .draft_service()
        .discard(DiscardClippingNoteRecoveryRequest {
            clipping_id: ID.to_string(),
            writer_session_id: SESSION_B.to_string(),
            writer_sequence: 1,
        })
        .unwrap_err();
    assert_eq!(stale.code, ClippingErrorCode::RecoveryStaleSequence);
    service
        .draft_service()
        .discard(DiscardClippingNoteRecoveryRequest {
            clipping_id: ID.to_string(),
            writer_session_id: SESSION_B.to_string(),
            writer_sequence: 2,
        })
        .unwrap();
    assert_eq!(load(&service).status, ClippingNoteRecoveryStatus::None);
}

#[test]
fn clipping_note_canonical_save_clears_only_its_atomic_acknowledged_checkpoint() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(
        &service,
        SESSION_A,
        1,
        created.revision,
        "canonical",
        "first",
    )
    .unwrap();
    let updated = service
        .update_note_response(
            ID,
            created.revision,
            "canonical",
            "first",
            Some(DraftCheckpointAck {
                writer_session_id: SESSION_A.to_string(),
                writer_sequence: 1,
            }),
            301,
        )
        .unwrap();
    assert_eq!(load(&service).status, ClippingNoteRecoveryStatus::None);

    checkpoint(
        &service,
        SESSION_A,
        2,
        updated.revision,
        "canonical",
        "first",
    )
    .unwrap();
    let unchanged = service
        .update_note_response(
            ID,
            updated.revision,
            "canonical",
            "first",
            Some(DraftCheckpointAck {
                writer_session_id: SESSION_A.to_string(),
                writer_sequence: 2,
            }),
            302,
        )
        .unwrap();
    assert_eq!(unchanged.revision, updated.revision);
    assert_eq!(load(&service).status, ClippingNoteRecoveryStatus::None);

    checkpoint(
        &service,
        SESSION_A,
        3,
        updated.revision,
        "different",
        "draft",
    )
    .unwrap();
    service
        .update_note_response(
            ID,
            updated.revision,
            "canonical",
            "first",
            Some(DraftCheckpointAck {
                writer_session_id: SESSION_A.to_string(),
                writer_sequence: 3,
            }),
            302,
        )
        .unwrap();
    assert_eq!(load(&service).identity.unwrap().writer_sequence, 3);

    checkpoint(
        &service,
        SESSION_A,
        4,
        updated.revision,
        "newest",
        "visible",
    )
    .unwrap();
    let older = service
        .update_note_response(
            ID,
            updated.revision,
            "older",
            "submitted",
            Some(DraftCheckpointAck {
                writer_session_id: SESSION_A.to_string(),
                writer_sequence: 3,
            }),
            303,
        )
        .unwrap();
    assert_eq!(load(&service).identity.unwrap().writer_sequence, 4);
    let conflict = service.update_note_response(
        ID,
        updated.revision,
        "conflict",
        "must not clear",
        Some(DraftCheckpointAck {
            writer_session_id: SESSION_A.to_string(),
            writer_sequence: 4,
        }),
        304,
    );
    assert_eq!(
        conflict.unwrap_err().code,
        ClippingErrorCode::RevisionConflict
    );
    assert_eq!(load(&service).identity.unwrap().writer_sequence, 4);
    assert_eq!(older.revision, updated.revision + 1);
}

#[test]
fn clipping_note_canonical_sql_failure_rolls_back_and_retains_recovery() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(
        &service,
        SESSION_A,
        1,
        created.revision,
        "draft",
        "retained",
    )
    .unwrap();
    let connection = open_runtime(&service.db_path).unwrap();
    connection
        .execute("DROP TABLE newspaper_clippings_normalized_fts", [])
        .unwrap();
    drop(connection);

    let error = service
        .update_note_response(
            ID,
            created.revision,
            "new canonical",
            "must roll back",
            Some(DraftCheckpointAck {
                writer_session_id: SESSION_A.to_string(),
                writer_sequence: 1,
            }),
            350,
        )
        .unwrap_err();
    assert_eq!(error.code, ClippingErrorCode::DatabaseWriteFailed);
    let canonical = service.detail(ID).unwrap().unwrap().clipping;
    assert_eq!(
        (canonical.title, canonical.revision),
        (created.title, created.revision)
    );
    assert_eq!(load(&service).identity.unwrap().writer_sequence, 1);
}

#[test]
fn clipping_note_authorized_delete_cascades_its_recovery_row() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(&service, SESSION_A, 1, created.revision, "draft", "body").unwrap();
    service.delete(ID, created.revision).unwrap();
    let connection = open_runtime(&service.db_path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_clipping_note_drafts WHERE clipping_id = ?1",
            [ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn clipping_note_recovery_enforces_its_separate_byte_envelope_and_safe_errors() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(
        &service,
        SESSION_A,
        1,
        created.revision,
        &"t".repeat(RECOVERY_TITLE_MAX_BYTES),
        &"m".repeat(RECOVERY_MARKDOWN_MAX_BYTES),
    )
    .unwrap();
    for (title, markdown, expected) in [
        (
            "t".repeat(RECOVERY_TITLE_MAX_BYTES + 1),
            "private-title".to_string(),
            ClippingErrorCode::RecoveryTooLarge,
        ),
        (
            "title".to_string(),
            "m".repeat(RECOVERY_MARKDOWN_MAX_BYTES + 1),
            ClippingErrorCode::RecoveryTooLarge,
        ),
        (
            "title\0secret".to_string(),
            "private-markdown".to_string(),
            ClippingErrorCode::RecoveryInvalid,
        ),
    ] {
        let error =
            checkpoint(&service, SESSION_A, 2, created.revision, &title, &markdown).unwrap_err();
        assert_eq!(error.code, expected);
        let safe = error.as_safe_string();
        assert!(!safe.contains("private") && !safe.contains("SELECT") && !safe.contains("\\"));
    }
}

#[test]
fn clipping_note_recovery_classifies_changed_and_invalid_rows_without_deleting_them() {
    let (_temp, service, _diagnostics) = fixture();
    let created = service
        .register_staged_legacy_fixture(staged_record(&service, ID))
        .unwrap();
    checkpoint(&service, SESSION_A, 1, created.revision, "draft", "body").unwrap();
    service
        .update_note(ID, created.revision, "canonical", "changed", 401)
        .unwrap();
    assert_eq!(
        load(&service).status,
        ClippingNoteRecoveryStatus::CanonicalChanged
    );

    let connection = open_runtime(&service.db_path).unwrap();
    connection
        .execute(
            "UPDATE newspaper_clipping_note_drafts SET draft_markdown = char(0)
             WHERE clipping_id = ?1",
            [ID],
        )
        .unwrap();
    drop(connection);
    let invalid = load(&service);
    assert_eq!(invalid.status, ClippingNoteRecoveryStatus::Invalid);
    assert!(invalid.draft.is_none());
    let claimed = service
        .draft_service()
        .claim(ClaimClippingNoteRecoveryRequest {
            clipping_id: ID.to_string(),
            prior_writer_session_id: SESSION_A.to_string(),
            prior_writer_sequence: 1,
            writer_session_id: SESSION_B.to_string(),
        })
        .unwrap();
    assert_eq!(claimed.status, ClippingNoteRecoveryStatus::Invalid);
    assert_eq!(claimed.identity.unwrap().writer_session_id, SESSION_A);
    let connection = open_runtime(&service.db_path).unwrap();
    let retained: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_clipping_note_drafts WHERE clipping_id = ?1",
            [ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 1);
}
