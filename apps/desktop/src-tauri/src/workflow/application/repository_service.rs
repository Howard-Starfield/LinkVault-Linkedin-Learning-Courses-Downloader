//! DatabaseWriter-backed workflow repository.

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};
use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::state::{RunState, StepState};
use crate::workflow::domain::types::{
    NewWorkflowRun, NewWorkflowStep, RunRecord, StepRecord, WorkflowEventRecord,
};
use crate::workflow::infrastructure::sqlite_repository::SqliteWorkflowRepository;
use crate::workflow::ports::repository::WorkflowRepository;

#[derive(Clone)]
pub struct WorkflowRepositoryService {
    writer: DatabaseWriter,
    repository: SqliteWorkflowRepository,
}

impl WorkflowRepositoryService {
    pub fn new(writer: DatabaseWriter) -> Self {
        Self {
            writer,
            repository: SqliteWorkflowRepository,
        }
    }

    pub fn insert_run_with_steps_and_event(
        &self,
        run: NewWorkflowRun,
        steps: Vec<NewWorkflowStep>,
        event_type: &'static str,
        payload_json: String,
    ) -> Result<(), WorkflowError> {
        let workflow_id = Some(run.id.clone());
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_insert_run",
                    provider: DatabaseProvider::Workflow,
                    workflow_id,
                },
                move |connection| {
                    repository
                        .insert_run_with_steps_and_event(
                            connection,
                            &run,
                            &steps,
                            event_type,
                            &payload_json,
                        )
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn get_run(&self, id: String) -> Result<Option<RunRecord>, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_get_run",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: Some(id.clone()),
                },
                move |connection| repository.get_run(connection, &id).map_err(map_domain),
            )
            .map_err(map_writer)
    }

    pub fn list_events(&self, run_id: String) -> Result<Vec<WorkflowEventRecord>, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_list_events",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: Some(run_id.clone()),
                },
                move |connection| {
                    repository
                        .list_events(connection, &run_id)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn list_steps_for_run(&self, run_id: String) -> Result<Vec<StepRecord>, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_list_steps",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: Some(run_id.clone()),
                },
                move |connection| {
                    repository
                        .list_steps_for_run(connection, &run_id)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn transition_run(
        &self,
        run_id: String,
        to: RunState,
        error_message: Option<String>,
        event_type: &'static str,
        payload_json: String,
        updated_at: i64,
    ) -> Result<RunRecord, WorkflowError> {
        let repository = self.repository;
        let workflow_id = Some(run_id.clone());
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_transition_run",
                    provider: DatabaseProvider::Workflow,
                    workflow_id,
                },
                move |connection| {
                    repository
                        .transition_run(
                            connection,
                            &run_id,
                            to,
                            error_message.as_deref(),
                            event_type,
                            &payload_json,
                            updated_at,
                        )
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn transition_step(
        &self,
        step_id: String,
        to: StepState,
        error_message: Option<String>,
        event_type: &'static str,
        payload_json: String,
        updated_at: i64,
    ) -> Result<StepRecord, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_transition_step",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: Some(step_id.clone()),
                },
                move |connection| {
                    repository
                        .transition_step(
                            connection,
                            &step_id,
                            to,
                            error_message.as_deref(),
                            event_type,
                            &payload_json,
                            updated_at,
                        )
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn list_runs_by_workflow_type(
        &self,
        workflow_type: String,
        limit: i64,
    ) -> Result<Vec<RunRecord>, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_list_runs_by_type",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .list_runs_by_workflow_type(connection, &workflow_type, limit)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn claim_next_ready_step(
        &self,
        workflow_type: String,
        updated_at: i64,
    ) -> Result<Option<StepRecord>, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_claim_ready_step",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .claim_next_ready_step(connection, &workflow_type, updated_at)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn fail_running_runs(
        &self,
        workflow_type: String,
        warning: String,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_fail_running_runs",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .fail_running_runs(connection, &workflow_type, &warning, updated_at)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn fail_nonterminal_runs(
        &self,
        workflow_type: String,
        warning: String,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_fail_nonterminal_runs",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .fail_nonterminal_runs(connection, &workflow_type, &warning, updated_at)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn fail_expired_running_runs(
        &self,
        warning: String,
        updated_at: i64,
        lease_expires_before: i64,
    ) -> Result<usize, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_fail_expired_leases",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .fail_expired_running_runs(
                            connection,
                            &warning,
                            updated_at,
                            lease_expires_before,
                        )
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn delete_runs_by_workflow_type(
        &self,
        workflow_type: String,
    ) -> Result<usize, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_delete_runs_by_type",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .delete_runs_by_workflow_type(connection, &workflow_type)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn delete_terminal_runs(&self, workflow_type: String) -> Result<usize, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_delete_terminal_runs",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: None,
                },
                move |connection| {
                    repository
                        .delete_terminal_runs(connection, &workflow_type)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }

    pub fn delete_run_if_terminal(&self, id: String) -> Result<bool, WorkflowError> {
        let repository = self.repository;
        self.writer
            .execute(
                DatabaseWriteContext {
                    operation: "workflow_delete_terminal_run",
                    provider: DatabaseProvider::Workflow,
                    workflow_id: Some(id.clone()),
                },
                move |connection| {
                    repository
                        .delete_run_if_terminal(connection, &id)
                        .map_err(map_domain)
                },
            )
            .map_err(map_writer)
    }
}

fn map_writer(error: DatabaseWriteError) -> WorkflowError {
    WorkflowError::Writer(error.to_string())
}

fn map_domain(error: WorkflowError) -> DatabaseWriteError {
    DatabaseWriteError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(
        std::io::Error::new(std::io::ErrorKind::Other, error.to_string()),
    )))
}
