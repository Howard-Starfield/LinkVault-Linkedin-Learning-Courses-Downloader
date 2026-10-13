//! Regression tests for restart reconcile and the one-shot automatic resume.

use rusqlite::Connection;

use super::sqlite_repository::SqliteWorkflowRepository;
use crate::app::database::initialize_database;
use crate::workflow::domain::resume::{ResumeState, ResumeTrigger, RunResumeRecord};
use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::{NewWorkflowRun, NewWorkflowStep, StepType, WorkflowType};
use crate::workflow::ports::repository::WorkflowRepository;

const REASON: &str = "Interrupted by an application restart";

fn open() -> (tempfile::TempDir, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let (connection, _) = initialize_database(&directory.path().join("resume.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    (directory, connection)
}

fn submit(connection: &Connection, id: &str, created_at: i64) {
    SqliteWorkflowRepository
        .insert_run_with_steps_and_event(
            connection,
            &NewWorkflowRun {
                id: id.to_string(),
                workflow_type: WorkflowType::youtube_download(),
                provider: "youtube".to_string(),
                legacy_origin: None,
                legacy_id: None,
                request_json: "{}".to_string(),
                output_root: ".".to_string(),
                created_at,
                ready_at: None,
            },
            &[NewWorkflowStep {
                id: format!("{id}-execute"),
                step_key: "vid".to_string(),
                step_type: StepType::youtube_execute(),
                created_at,
            }],
            "submitted",
            "{}",
        )
        .unwrap();
}

fn claim(connection: &Connection, now: i64) -> Option<String> {
    SqliteWorkflowRepository
        .claim_next_ready_step(connection, "youtube_download", now)
        .unwrap()
        .map(|step| step.id)
}

fn run_state(connection: &Connection, id: &str) -> RunState {
    SqliteWorkflowRepository
        .get_run(connection, id)
        .unwrap()
        .unwrap()
        .state
}

fn resume(connection: &Connection, id: &str) -> RunResumeRecord {
    SqliteWorkflowRepository
        .get_resume(connection, id)
        .unwrap()
        .unwrap()
}

fn reconcile(connection: &Connection, now: i64) {
    SqliteWorkflowRepository
        .reconcile_for_resume(connection, "youtube_download", REASON, now)
        .unwrap();
}

fn begin(connection: &Connection, id: &str, trigger: ResumeTrigger, now: i64) -> bool {
    SqliteWorkflowRepository
        .begin_resume(connection, id, trigger, now)
        .unwrap()
}

#[test]
fn restart_reconcile_makes_running_and_queued_runs_resumable_not_failed() {
    let (_dir, connection) = open();
    submit(&connection, "run-running", 1);
    submit(&connection, "run-queued", 2);
    claim(&connection, 5).unwrap();
    let summary = SqliteWorkflowRepository
        .reconcile_for_resume(&connection, "youtube_download", REASON, 10)
        .unwrap();
    assert_eq!(summary.resumable, 2);
    for id in ["run-running", "run-queued"] {
        assert_eq!(run_state(&connection, id), RunState::Paused, "{id}");
        let row = resume(&connection, id);
        assert_eq!(row.state, ResumeState::AutoPending);
        assert!(!row.auto_resume_used);
        assert!(row.reason.as_deref().unwrap().contains("restart"));
    }
    assert!(
        claim(&connection, 20).is_none(),
        "a paused run must not be claimable until it is resumed"
    );
}

#[test]
fn restart_reconcile_finishes_a_cancelling_run_as_cancelled() {
    let (_dir, connection) = open();
    submit(&connection, "run-cancel", 1);
    claim(&connection, 2).unwrap();
    SqliteWorkflowRepository
        .transition_run(
            &connection,
            "run-cancel",
            RunState::Cancelling,
            None,
            "run_cancelling",
            "{}",
            3,
        )
        .unwrap();
    let summary = SqliteWorkflowRepository
        .reconcile_for_resume(&connection, "youtube_download", REASON, 10)
        .unwrap();
    assert_eq!(summary.cancelled, 1);
    assert_eq!(run_state(&connection, "run-cancel"), RunState::Cancelled);
}

#[test]
fn automatic_resume_budget_is_spent_once_and_survives_a_crash() {
    let (_dir, connection) = open();
    submit(&connection, "run-a", 1);
    claim(&connection, 2).unwrap();
    reconcile(&connection, 10);

    assert!(begin(&connection, "run-a", ResumeTrigger::Auto, 11));
    let row = resume(&connection, "run-a");
    assert!(row.auto_resume_used, "budget is durable before any claim");
    assert_eq!(row.state, ResumeState::Resuming);
    assert!(
        !begin(&connection, "run-a", ResumeTrigger::Auto, 12),
        "a second automatic admission must be refused"
    );

    // The app is killed while the automatic attempt runs.
    claim(&connection, 13).expect("the resumed run is claimable");
    reconcile(&connection, 20);
    assert_eq!(run_state(&connection, "run-a"), RunState::Paused);
    let row = resume(&connection, "run-a");
    assert_eq!(
        row.state,
        ResumeState::NeedsUser,
        "a second interruption pauses"
    );
    assert!(row.auto_resume_used);
    assert!(!begin(&connection, "run-a", ResumeTrigger::Auto, 21));
    assert!(claim(&connection, 22).is_none());
}

#[test]
fn a_failed_resumed_attempt_pauses_and_continue_runs_exactly_one_attempt() {
    let (_dir, connection) = open();
    submit(&connection, "run-b", 1);
    claim(&connection, 2).unwrap();
    reconcile(&connection, 10);
    assert!(begin(&connection, "run-b", ResumeTrigger::Auto, 11));
    let step = claim(&connection, 12).unwrap();

    assert!(SqliteWorkflowRepository
        .pause_after_failed_attempt(
            &connection,
            "run-b",
            &step,
            "network dropped",
            Some("HELPER_FAILED"),
            0,
            13,
        )
        .unwrap());
    assert_eq!(run_state(&connection, "run-b"), RunState::Paused);
    let row = resume(&connection, "run-b");
    assert_eq!(row.state, ResumeState::NeedsUser);
    assert_eq!(row.reason.as_deref(), Some("network dropped"));
    assert!(
        claim(&connection, 14).is_none(),
        "a paused run is never retried automatically"
    );
    assert!(!begin(&connection, "run-b", ResumeTrigger::Auto, 14));

    assert!(begin(&connection, "run-b", ResumeTrigger::Manual, 15));
    assert!(
        !begin(&connection, "run-b", ResumeTrigger::Manual, 15),
        "a double click must not admit a second attempt"
    );
    assert!(claim(&connection, 16).is_some());
    assert!(
        claim(&connection, 16).is_none(),
        "only one attempt is queued"
    );
    assert!(
        !begin(&connection, "run-b", ResumeTrigger::Manual, 17),
        "continue while the attempt is running does nothing"
    );
}

#[test]
fn parking_for_the_user_keeps_the_budget_unspent() {
    let (_dir, connection) = open();
    submit(&connection, "run-c", 1);
    reconcile(&connection, 10);
    assert!(SqliteWorkflowRepository
        .park_for_user(
            &connection,
            "run-c",
            "helpers missing",
            Some("HELPER_MISSING"),
            11,
            None,
        )
        .unwrap());
    let row = resume(&connection, "run-c");
    assert_eq!(row.state, ResumeState::NeedsUser);
    assert!(!row.auto_resume_used, "no attempt started, no budget spent");
    assert!(!begin(&connection, "run-c", ResumeTrigger::Auto, 12));
    assert!(begin(&connection, "run-c", ResumeTrigger::Manual, 13));
}

#[test]
fn a_first_attempt_size_note_is_not_a_resumed_attempt() {
    let (_dir, connection) = open();
    submit(&connection, "run-d", 1);
    claim(&connection, 2).unwrap();
    SqliteWorkflowRepository
        .set_expected_bytes(&connection, "run-d", "occ-1", 5_000, 3)
        .unwrap();
    assert!(SqliteWorkflowRepository
        .list_resumable(&connection, "youtube_download")
        .unwrap()
        .is_empty());
    reconcile(&connection, 10);
    let row = resume(&connection, "run-d");
    assert_eq!(row.state, ResumeState::AutoPending);
    assert_eq!(row.expected_bytes, Some(5_000));
    assert!(begin(&connection, "run-d", ResumeTrigger::Auto, 11));
}
