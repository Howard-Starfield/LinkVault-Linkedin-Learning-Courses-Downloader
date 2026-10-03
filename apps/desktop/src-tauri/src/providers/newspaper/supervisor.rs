//! Newspaper planning and compatibility work hosted by the shared supervisor.
//! This adapter owns no timer or worker; the workflow runtime owns their lifetime.

use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};

use rusqlite::Connection;
use tauri::{Emitter, Manager};

use crate::app::database_writer::DatabaseWriter;
use crate::workflow::application::runtime::{SupervisorHook, WorkflowRuntime};
use crate::workflow::domain::errors::WorkflowError;

use super::{
    commands, library_events, models::OptimizationRunOptions, queue_service, schedule_service,
    state::NewspaperState,
};

pub struct NewspaperSupervisor {
    app: tauri::AppHandle,
    legacy_retry_after: Arc<AtomicI64>,
    optimization_retry_after: Arc<AtomicI64>,
}

impl NewspaperSupervisor {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self {
            app,
            legacy_retry_after: Arc::new(AtomicI64::new(0)),
            optimization_retry_after: Arc::new(AtomicI64::new(0)),
        }
    }
}

pub(super) fn invalidate_activity(app: &tauri::AppHandle) {
    let state = app.state::<NewspaperState>();
    let revision = state.invalidate_progress();
    let _ = app.emit(
        "newspaper://activity-invalidated",
        serde_json::json!({ "revision": revision }),
    );
}

pub(super) fn rearm_if_idle(app: &tauri::AppHandle) {
    let state = app.state::<NewspaperState>();
    if !state.download_running.load(Ordering::SeqCst)
        && !state.optimization_running.load(Ordering::SeqCst)
        && !app
            .state::<WorkflowRuntime>()
            .is_workflow_type_executing("newspaper_download")
        && !app.state::<WorkflowRuntime>().is_shutting_down()
        && !app
            .state::<WorkflowRuntime>()
            .is_workflow_type_executing("newspaper_optimization")
    {
        state.cancelled.store(false, Ordering::SeqCst);
    }
}

/// Completion/start notifications are small; the screen fetches its own state.
pub fn download_changed(app: &tauri::AppHandle, job_id: &str) {
    invalidate_activity(app);
    if let Ok(connection) = crate::cache::open_runtime(app.state::<NewspaperState>().db_path()) {
        if let Ok(Some(job)) = super::job_repository::find(&connection, job_id) {
            if matches!(job.status.as_str(), "completed" | "partial" | "optimizing") {
                library_events::emit(app, &app.state::<NewspaperState>(), &[job]);
            }
        }
    }
    app.state::<WorkflowRuntime>().wake();
}

impl SupervisorHook for NewspaperSupervisor {
    fn shutdown(&self) {
        self.app
            .state::<NewspaperState>()
            .cancelled
            .store(true, Ordering::SeqCst);
    }

    fn reconcile(&self, runtime: &WorkflowRuntime, now: i64) -> Result<Option<i64>, WorkflowError> {
        let state = self.app.state::<NewspaperState>();
        let writer = self.app.state::<DatabaseWriter>();
        let created = schedule_service::materialize_due_at(&writer, state.db_path(), now)
            .map_err(WorkflowError::Writer)?;
        if created > 0 {
            invalidate_activity(&self.app);
        }
        rearm_if_idle(&self.app);
        let connection = crate::cache::open_runtime(state.db_path())?;
        let mut next =
            schedule_service::next_due_at(state.db_path(), now).map_err(WorkflowError::Sqlite)?;
        if !state.download_running.load(Ordering::SeqCst)
            && !runtime.is_workflow_type_executing("newspaper_download")
            && !state.cancelled.load(Ordering::SeqCst)
        {
            if let Some(deadline) = legacy_deadline(&connection, now)? {
                let deadline = deadline.max(self.legacy_retry_after.load(Ordering::SeqCst));
                if deadline <= now {
                    let app = self.app.clone();
                    let retry_after = Arc::clone(&self.legacy_retry_after);
                    runtime.spawn_supervisor_task("newspaper_download", move || {
                        let state = app.state::<NewspaperState>();
                        if state.download_running.swap(true, Ordering::SeqCst) {
                            return;
                        }
                        invalidate_activity(&app);
                        let result = tauri::async_runtime::block_on(queue_service::process_queue(
                            state.db_path(),
                            &state.cancelled,
                            &app,
                        ));
                        state.download_running.store(false, Ordering::SeqCst);
                        retry_after.store(
                            if result.is_err() {
                                chrono::Utc::now().timestamp().saturating_add(30)
                            } else {
                                0
                            },
                            Ordering::SeqCst,
                        );
                        if let Ok(jobs) = result {
                            library_events::emit(&app, &state, &jobs);
                        }
                        invalidate_activity(&app);
                    })?;
                } else {
                    next = minimum_deadline(next, Some(deadline));
                }
            }
        }
        if !state.optimization_running.load(Ordering::SeqCst)
            && !state.cancelled.load(Ordering::SeqCst)
        {
            if let Some(deadline) = optimization_deadline(&connection, now)? {
                let deadline = deadline.max(self.optimization_retry_after.load(Ordering::SeqCst));
                if deadline <= now {
                    let app = self.app.clone();
                    let retry_after = Arc::clone(&self.optimization_retry_after);
                    runtime.spawn_supervisor_task("newspaper_optimization", move || {
                        let state = app.state::<NewspaperState>();
                        let result =
                            tauri::async_runtime::block_on(commands::run_optimization_pass(
                                &app,
                                &state,
                                OptimizationRunOptions::default(),
                            ));
                        retry_after.store(
                            if result.is_err() {
                                chrono::Utc::now().timestamp().saturating_add(30)
                            } else {
                                0
                            },
                            Ordering::SeqCst,
                        );
                    })?;
                } else {
                    next = minimum_deadline(next, Some(deadline));
                }
            }
        }
        Ok(next)
    }
}

fn minimum_deadline(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn legacy_deadline(connection: &Connection, now: i64) -> rusqlite::Result<Option<i64>> {
    connection.query_row(
        "SELECT MIN(MAX(COALESCE(j.retry_at, ?1), COALESCE(b.scheduled_at, ?1)))
         FROM newspaper_jobs j JOIN newspaper_batches b ON b.id = j.batch_id
         WHERE j.status = 'queued' AND j.paused = 0 AND j.dismissed = 0
           AND b.status IN ('queued', 'scheduled', 'active')
           AND NOT EXISTS (SELECT 1 FROM workflow_runs r WHERE r.id = j.id
               AND r.state IN ('queued', 'running', 'retry_wait', 'paused', 'cancelling'))",
        [now],
        |row| row.get(0),
    )
}

pub(super) fn optimization_deadline(
    connection: &Connection,
    now: i64,
) -> rusqlite::Result<Option<i64>> {
    connection.query_row(
        "SELECT MIN(CASE WHEN t.page_id IS NULL THEN ?1
            WHEN t.status = 'pending' THEN COALESCE(t.retry_at, ?1)
            WHEN t.status = 'running' THEN COALESCE(t.lease_expires_at, ?1) END)
         FROM newspaper_pages p
         JOIN newspaper_jobs j ON j.id = p.job_id
         JOIN newspaper_batches b ON b.id = j.batch_id
         LEFT JOIN newspaper_optimization_tasks t ON t.page_id = p.id
         WHERE j.status IN ('optimizing', 'completed', 'partial')
           AND j.paused = 0 AND j.dismissed = 0 AND b.optimize_images = 1
           AND b.status NOT IN ('paused', 'cancelled') AND p.status = 'completed'
           AND p.original_path IS NOT NULL AND p.optimized_path IS NULL
           AND (t.page_id IS NULL OR t.attempts < ?2)",
        rusqlite::params![now, super::optimization_tasks::MAX_ATTEMPTS],
        |row| row.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimization_deadline_sleeps_for_retries_and_ignores_paused_or_exhausted_work() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE newspaper_batches(id TEXT, status TEXT, optimize_images INTEGER);
            CREATE TABLE newspaper_jobs(id TEXT, batch_id TEXT, status TEXT, paused INTEGER, dismissed INTEGER);
            CREATE TABLE newspaper_pages(id TEXT, job_id TEXT, status TEXT, original_path TEXT, optimized_path TEXT);
            CREATE TABLE newspaper_optimization_tasks(page_id TEXT, status TEXT, retry_at INTEGER, lease_expires_at INTEGER, attempts INTEGER);
            INSERT INTO newspaper_batches VALUES('batch','active',1);
            INSERT INTO newspaper_jobs VALUES('job','batch','partial',0,0);
            INSERT INTO newspaper_pages VALUES('page','job','completed','original.jpg',NULL);").unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), Some(100));
        connection
            .execute(
                "INSERT INTO newspaper_optimization_tasks VALUES('page','pending',300,NULL,1)",
                [],
            )
            .unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), Some(300));
        connection
            .execute("UPDATE newspaper_jobs SET paused=1", [])
            .unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), None);
        connection
            .execute("UPDATE newspaper_jobs SET paused=0", [])
            .unwrap();
        connection
            .execute("UPDATE newspaper_batches SET status='cancelled'", [])
            .unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), None);
        connection
            .execute("UPDATE newspaper_batches SET status='active'", [])
            .unwrap();
        connection
            .execute(
                "UPDATE newspaper_optimization_tasks SET status='running',lease_expires_at=500",
                [],
            )
            .unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), Some(500));
        connection
            .execute("UPDATE newspaper_optimization_tasks SET attempts=3", [])
            .unwrap();
        assert_eq!(optimization_deadline(&connection, 100).unwrap(), None);
    }

    #[test]
    fn legacy_deadline_respects_every_eligibility_gate_and_release_retry() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE newspaper_batches(id TEXT, status TEXT, scheduled_at INTEGER);
            CREATE TABLE newspaper_jobs(id TEXT, batch_id TEXT, status TEXT, paused INTEGER, dismissed INTEGER, retry_at INTEGER);
            CREATE TABLE workflow_runs(id TEXT, state TEXT);
            INSERT INTO newspaper_batches VALUES('batch', 'scheduled', 200);
            INSERT INTO newspaper_jobs VALUES('job', 'batch', 'queued', 0, 0, 300);").unwrap();
        assert_eq!(legacy_deadline(&connection, 100).unwrap(), Some(300));
        connection
            .execute("INSERT INTO workflow_runs VALUES('job', 'running')", [])
            .unwrap();
        assert_eq!(legacy_deadline(&connection, 100).unwrap(), None);
        connection
            .execute(
                "UPDATE workflow_runs SET state = 'succeeded_with_warnings'",
                [],
            )
            .unwrap();
        assert_eq!(legacy_deadline(&connection, 400).unwrap(), Some(300));
        for predicate in ["paused = 1", "dismissed = 1", "status = 'completed'"] {
            connection
                .execute(
                    "UPDATE newspaper_jobs SET paused = 0, dismissed = 0, status = 'queued'",
                    [],
                )
                .unwrap();
            connection
                .execute(&format!("UPDATE newspaper_jobs SET {predicate}"), [])
                .unwrap();
            assert_eq!(legacy_deadline(&connection, 100).unwrap(), None);
        }
    }
}
