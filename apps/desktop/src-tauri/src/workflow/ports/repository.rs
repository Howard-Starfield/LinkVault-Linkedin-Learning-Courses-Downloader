//! Persistence port for workflow runs, steps, and events.

use rusqlite::Connection;

use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::state::{RunState, StepState};
use crate::workflow::domain::types::{
    NewWorkflowRun, NewWorkflowStep, RunRecord, StepRecord, WorkflowEventRecord,
};

pub trait WorkflowRepository {
    fn insert_run_with_steps_and_event(
        &self,
        connection: &Connection,
        run: &NewWorkflowRun,
        steps: &[NewWorkflowStep],
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), WorkflowError>;

    fn get_run(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<Option<RunRecord>, WorkflowError>;

    fn list_runs_by_state(
        &self,
        connection: &Connection,
        state: RunState,
    ) -> Result<Vec<RunRecord>, WorkflowError>;

    fn list_steps_for_run(
        &self,
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<StepRecord>, WorkflowError>;

    fn list_runs_by_workflow_type(
        &self,
        connection: &Connection,
        workflow_type: &str,
        limit: i64,
    ) -> Result<Vec<RunRecord>, WorkflowError>;

    fn transition_run(
        &self,
        connection: &Connection,
        run_id: &str,
        to: RunState,
        error_message: Option<&str>,
        event_type: &str,
        payload_json: &str,
        updated_at: i64,
    ) -> Result<RunRecord, WorkflowError>;

    fn transition_step(
        &self,
        connection: &Connection,
        step_id: &str,
        to: StepState,
        error_message: Option<&str>,
        event_type: &str,
        payload_json: &str,
        updated_at: i64,
    ) -> Result<StepRecord, WorkflowError>;

    fn claim_next_ready_step(
        &self,
        connection: &Connection,
        workflow_type: &str,
        updated_at: i64,
    ) -> Result<Option<StepRecord>, WorkflowError>;

    fn fail_running_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
        warning: &str,
        updated_at: i64,
    ) -> Result<usize, WorkflowError>;

    /// Fail non-terminal runs for a workflow type (YouTube restart reconcile).
    /// Does not change shared `fail_running_runs` semantics for other providers.
    fn fail_nonterminal_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
        warning: &str,
        updated_at: i64,
    ) -> Result<usize, WorkflowError>;

    fn fail_expired_running_runs(
        &self,
        connection: &Connection,
        warning: &str,
        updated_at: i64,
        lease_expires_before: i64,
    ) -> Result<usize, WorkflowError>;

    fn list_events(
        &self,
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<WorkflowEventRecord>, WorkflowError>;

    fn delete_runs_by_workflow_type(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<usize, WorkflowError>;

    fn delete_terminal_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<usize, WorkflowError>;

    fn delete_run_if_terminal(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<bool, WorkflowError>;
}
