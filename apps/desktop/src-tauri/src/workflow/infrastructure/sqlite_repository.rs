//! SQLite implementation of `WorkflowRepository`.

use rusqlite::{params, Connection, OptionalExtension};

use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::state::{RunState, StepState};
use crate::workflow::domain::transitions::{validate_run_transition, validate_step_transition};
use crate::workflow::domain::types::{
    NewWorkflowRun, NewWorkflowStep, RunRecord, StepRecord, StepType, WorkflowEventRecord,
    WorkflowType,
};
use crate::workflow::ports::repository::WorkflowRepository;

#[derive(Debug, Default, Clone, Copy)]
pub struct SqliteWorkflowRepository;

impl WorkflowRepository for SqliteWorkflowRepository {
    fn insert_run_with_steps_and_event(
        &self,
        connection: &Connection,
        run: &NewWorkflowRun,
        steps: &[NewWorkflowStep],
        event_type: &str,
        payload_json: &str,
    ) -> Result<(), WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let (run_state, step_state, updated_at) = match run.ready_at {
            Some(ready_at) if ready_at > run.created_at => ("retry_wait", "retry_wait", ready_at),
            _ => ("queued", "ready", run.created_at),
        };
        tx.execute(
            "INSERT INTO workflow_runs
                (id, workflow_type, provider, state, legacy_origin, legacy_id,
                 request_json, output_root, error_message, created_at, updated_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, NULL)",
            params![
                run.id,
                run.workflow_type.as_str(),
                run.provider,
                run_state,
                run.legacy_origin,
                run.legacy_id,
                run.request_json,
                run.output_root,
                run.created_at,
                updated_at
            ],
        )?;
        for step in steps {
            tx.execute(
                "INSERT INTO workflow_steps
                    (id, run_id, step_key, step_type, state, attempt, error_message, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, ?6, ?7)",
                params![
                    step.id,
                    run.id,
                    step.step_key,
                    step.step_type.as_str(),
                    step_state,
                    step.created_at,
                    updated_at
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO workflow_events
                (run_id, step_id, sequence, event_type, payload_json, created_at)
             VALUES (?1, NULL, 1, ?2, ?3, ?4)",
            params![run.id, event_type, payload_json, run.created_at],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get_run(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<Option<RunRecord>, WorkflowError> {
        connection
            .query_row(
                "SELECT id, workflow_type, provider, state, legacy_origin, legacy_id,
                        request_json, output_root, error_message, created_at, updated_at, completed_at
                 FROM workflow_runs WHERE id = ?1",
                params![id],
                map_run,
            )
            .optional()
            .map_err(Into::into)
    }

    fn list_runs_by_state(
        &self,
        connection: &Connection,
        state: RunState,
    ) -> Result<Vec<RunRecord>, WorkflowError> {
        let mut stmt = connection.prepare(
            "SELECT id, workflow_type, provider, state, legacy_origin, legacy_id,
                    request_json, output_root, error_message, created_at, updated_at, completed_at
             FROM workflow_runs WHERE state = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![state.as_str()], map_run)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn list_runs_by_workflow_type(
        &self,
        connection: &Connection,
        workflow_type: &str,
        limit: i64,
    ) -> Result<Vec<RunRecord>, WorkflowError> {
        let mut stmt = connection.prepare(
            "SELECT id, workflow_type, provider, state, legacy_origin, legacy_id,
                    request_json, output_root, error_message, created_at, updated_at, completed_at
             FROM workflow_runs WHERE workflow_type = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![workflow_type, limit], map_run)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn list_steps_for_run(
        &self,
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<StepRecord>, WorkflowError> {
        let mut stmt = connection.prepare(
            "SELECT id, run_id, step_key, step_type, state, attempt, error_message, created_at, updated_at
             FROM workflow_steps WHERE run_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], map_step)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn transition_run(
        &self,
        connection: &Connection,
        run_id: &str,
        to: RunState,
        error_message: Option<&str>,
        event_type: &str,
        payload_json: &str,
        updated_at: i64,
    ) -> Result<RunRecord, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let current = tx
            .query_row(
                "SELECT id, workflow_type, provider, state, legacy_origin, legacy_id,
                        request_json, output_root, error_message, created_at, updated_at, completed_at
                 FROM workflow_runs WHERE id = ?1",
                params![run_id],
                map_run,
            )
            .optional()?
            .ok_or_else(|| WorkflowError::RunNotFound(run_id.to_string()))?;
        validate_run_transition(current.state, to)?;
        let completed_at = if to.is_terminal() {
            Some(updated_at)
        } else {
            None
        };
        tx.execute(
            "UPDATE workflow_runs
             SET state = ?2, error_message = ?3, updated_at = ?4, completed_at = ?5
             WHERE id = ?1",
            params![run_id, to.as_str(), error_message, updated_at, completed_at],
        )?;
        append_event(&tx, run_id, None, event_type, payload_json, updated_at)?;
        tx.commit()?;
        self.get_run(connection, run_id)?
            .ok_or_else(|| WorkflowError::RunNotFound(run_id.to_string()))
    }

    fn transition_step(
        &self,
        connection: &Connection,
        step_id: &str,
        to: StepState,
        error_message: Option<&str>,
        event_type: &str,
        payload_json: &str,
        updated_at: i64,
    ) -> Result<StepRecord, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let current = tx
            .query_row(
                "SELECT id, run_id, step_key, step_type, state, attempt, error_message, created_at, updated_at
                 FROM workflow_steps WHERE id = ?1",
                params![step_id],
                map_step,
            )
            .optional()?
            .ok_or_else(|| WorkflowError::StepNotFound(step_id.to_string()))?;
        validate_step_transition(current.state, to)?;
        tx.execute(
            "UPDATE workflow_steps
             SET state = ?2, error_message = ?3, updated_at = ?4, attempt = attempt + ?5
             WHERE id = ?1",
            params![
                step_id,
                to.as_str(),
                error_message,
                updated_at,
                i64::from(to == StepState::Running)
            ],
        )?;
        append_event(
            &tx,
            &current.run_id,
            Some(step_id),
            event_type,
            payload_json,
            updated_at,
        )?;
        tx.commit()?;
        connection
            .query_row(
                "SELECT id, run_id, step_key, step_type, state, attempt, error_message, created_at, updated_at
                 FROM workflow_steps WHERE id = ?1",
                params![step_id],
                map_step,
            )
            .map_err(Into::into)
    }

    fn claim_next_ready_step(
        &self,
        connection: &Connection,
        workflow_type: &str,
        updated_at: i64,
    ) -> Result<Option<StepRecord>, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let claimed = tx
            .query_row(
                "SELECT s.id, s.run_id, s.step_key, s.step_type, s.state, s.attempt,
                        s.error_message, s.created_at, s.updated_at
                 FROM workflow_steps s
                 INNER JOIN workflow_runs r ON r.id = s.run_id
                 WHERE (
                        (s.state = 'ready' AND r.state = 'queued')
                     OR (s.state = 'retry_wait' AND r.state = 'retry_wait' AND s.updated_at <= ?2)
                   )
                   AND r.workflow_type = ?1
                 ORDER BY s.created_at ASC
                 LIMIT 1",
                params![workflow_type, updated_at],
                map_step,
            )
            .optional()?;
        let Some(step) = claimed else {
            tx.commit()?;
            return Ok(None);
        };
        validate_step_transition(step.state, StepState::Running)?;
        let run_state = RunState::parse(&tx.query_row(
            "SELECT state FROM workflow_runs WHERE id = ?1",
            params![step.run_id],
            |row| row.get::<_, String>(0),
        )?)?;
        validate_run_transition(run_state, RunState::Running)?;
        tx.execute(
            "UPDATE workflow_steps
             SET state = 'running', updated_at = ?2, attempt = attempt + 1
             WHERE id = ?1",
            params![step.id, updated_at],
        )?;
        tx.execute(
            "UPDATE workflow_runs SET state = 'running', updated_at = ?2 WHERE id = ?1",
            params![step.run_id, updated_at],
        )?;
        append_event(
            &tx,
            &step.run_id,
            Some(&step.id),
            "step_claimed",
            "{}",
            updated_at,
        )?;
        tx.commit()?;
        connection
            .query_row(
                "SELECT id, run_id, step_key, step_type, state, attempt, error_message, created_at, updated_at
                 FROM workflow_steps WHERE id = ?1",
                params![step.id],
                map_step,
            )
            .optional()
            .map_err(Into::into)
    }

    fn fail_running_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
        warning: &str,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let run_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM workflow_runs WHERE workflow_type = ?1 AND state = 'running'",
            )?;
            let ids = stmt
                .query_map(params![workflow_type], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            ids
        };
        for run_id in &run_ids {
            tx.execute(
                "UPDATE workflow_steps
                 SET state = 'failed', error_message = ?2, updated_at = ?3
                 WHERE run_id = ?1 AND state = 'running'",
                params![run_id, warning, updated_at],
            )?;
            tx.execute(
                "UPDATE workflow_runs
                 SET state = 'failed', error_message = ?2, updated_at = ?3, completed_at = ?3
                 WHERE id = ?1",
                params![run_id, warning, updated_at],
            )?;
            append_event(&tx, run_id, None, "restart_failed", "{}", updated_at)?;
        }
        tx.commit()?;
        Ok(run_ids.len())
    }

    fn fail_nonterminal_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
        warning: &str,
        updated_at: i64,
    ) -> Result<usize, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let run_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM workflow_runs
                 WHERE workflow_type = ?1
                   AND state IN ('queued', 'running', 'cancelling', 'paused', 'retry_wait')",
            )?;
            let ids = stmt
                .query_map(params![workflow_type], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            ids
        };
        for run_id in &run_ids {
            tx.execute(
                "UPDATE workflow_steps
                 SET state = 'failed', error_message = ?2, updated_at = ?3
                 WHERE run_id = ?1
                   AND state IN ('pending', 'ready', 'running', 'retry_wait')",
                params![run_id, warning, updated_at],
            )?;
            tx.execute(
                "UPDATE workflow_runs
                 SET state = 'failed', error_message = ?2, updated_at = ?3, completed_at = ?3
                 WHERE id = ?1",
                params![run_id, warning, updated_at],
            )?;
            append_event(&tx, run_id, None, "restart_failed", "{}", updated_at)?;
        }
        tx.commit()?;
        Ok(run_ids.len())
    }

    fn fail_expired_running_runs(
        &self,
        connection: &Connection,
        warning: &str,
        updated_at: i64,
        lease_expires_before: i64,
    ) -> Result<usize, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let run_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM workflow_runs
                 WHERE state = 'running' AND updated_at <= ?1",
            )?;
            let ids = stmt
                .query_map(params![lease_expires_before], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            ids
        };
        for run_id in &run_ids {
            tx.execute(
                "UPDATE workflow_steps
                 SET state = 'failed', error_message = ?2, updated_at = ?3
                 WHERE run_id = ?1 AND state = 'running'",
                params![run_id, warning, updated_at],
            )?;
            tx.execute(
                "UPDATE workflow_runs
                 SET state = 'failed', error_message = ?2, updated_at = ?3, completed_at = ?3
                 WHERE id = ?1",
                params![run_id, warning, updated_at],
            )?;
            append_event(&tx, run_id, None, "lease_expired", "{}", updated_at)?;
        }
        tx.commit()?;
        Ok(run_ids.len())
    }

    fn list_events(
        &self,
        connection: &Connection,
        run_id: &str,
    ) -> Result<Vec<WorkflowEventRecord>, WorkflowError> {
        let mut stmt = connection.prepare(
            "SELECT id, run_id, step_id, sequence, event_type, payload_json, created_at
             FROM workflow_events WHERE run_id = ?1 ORDER BY sequence ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok(WorkflowEventRecord {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    step_id: row.get(2)?,
                    sequence: row.get(3)?,
                    event_type: row.get(4)?,
                    payload_json: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn delete_runs_by_workflow_type(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<usize, WorkflowError> {
        delete_matching_runs(
            connection,
            "SELECT id FROM workflow_runs WHERE workflow_type = ?1",
            workflow_type,
        )
    }

    fn delete_terminal_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<usize, WorkflowError> {
        delete_matching_runs(
            connection,
            "SELECT id FROM workflow_runs
             WHERE workflow_type = ?1 AND state IN ('failed', 'cancelled')",
            workflow_type,
        )
    }

    fn delete_run_if_terminal(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<bool, WorkflowError> {
        let Some(run) = self.get_run(connection, id)? else {
            return Ok(false);
        };
        if !matches!(run.state, RunState::Failed | RunState::Cancelled) {
            return Ok(false);
        }
        delete_run_cascade(connection, id)?;
        Ok(true)
    }
}

fn delete_matching_runs(
    connection: &Connection,
    sql: &str,
    workflow_type: &str,
) -> Result<usize, WorkflowError> {
    let tx = connection.unchecked_transaction()?;
    let run_ids: Vec<String> = {
        let mut stmt = tx.prepare(sql)?;
        let ids = stmt
            .query_map(params![workflow_type], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        ids
    };
    for run_id in &run_ids {
        delete_run_cascade(&tx, run_id)?;
    }
    tx.commit()?;
    Ok(run_ids.len())
}

fn delete_run_cascade(connection: &Connection, run_id: &str) -> Result<(), WorkflowError> {
    connection.execute(
        "DELETE FROM workflow_events WHERE run_id = ?1",
        params![run_id],
    )?;
    connection.execute(
        "DELETE FROM workflow_steps WHERE run_id = ?1",
        params![run_id],
    )?;
    connection.execute("DELETE FROM workflow_runs WHERE id = ?1", params![run_id])?;
    Ok(())
}

fn append_event(
    connection: &Connection,
    run_id: &str,
    step_id: Option<&str>,
    event_type: &str,
    payload_json: &str,
    created_at: i64,
) -> Result<(), WorkflowError> {
    let next: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM workflow_events WHERE run_id = ?1",
        params![run_id],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO workflow_events
            (run_id, step_id, sequence, event_type, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![run_id, step_id, next, event_type, payload_json, created_at],
    )?;
    Ok(())
}

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    let state = RunState::parse(row.get::<_, String>(3)?.as_str()).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid run state",
            )),
        )
    })?;
    Ok(RunRecord {
        id: row.get(0)?,
        workflow_type: WorkflowType::from_owned(row.get(1)?),
        provider: row.get(2)?,
        state,
        legacy_origin: row.get(4)?,
        legacy_id: row.get(5)?,
        request_json: row.get(6)?,
        output_root: row.get(7)?,
        error_message: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        completed_at: row.get(11)?,
    })
}

fn map_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<StepRecord> {
    let state = StepState::parse(row.get::<_, String>(4)?.as_str()).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid step state",
            )),
        )
    })?;
    Ok(StepRecord {
        id: row.get(0)?,
        run_id: row.get(1)?,
        step_key: row.get(2)?,
        step_type: StepType::from_owned(row.get(3)?),
        state,
        attempt: row.get(5)?,
        error_message: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
