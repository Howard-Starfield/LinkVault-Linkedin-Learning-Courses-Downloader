//! Durable resume bookkeeping (`workflow_run_resume`) and the transitions that
//! keep it consistent with the run and step state machines.
//!
//! Every function here is one transaction on the writer connection, so a crash
//! can never leave the budget flag and the run state disagreeing.
//!
//! A row with `state = 'resuming'` and `auto_resume_used = 0` is only an
//! "expected size" note written by a first, never-interrupted attempt; it is
//! not a resumed attempt and is ignored everywhere a resume decision is made.

use rusqlite::{params, Connection, OptionalExtension};

use super::sqlite_repository::SqliteWorkflowRepository;
use crate::workflow::domain::errors::WorkflowError;
use crate::workflow::domain::resume::{
    ReconcileSummary, ResumeState, ResumeTrigger, RunResumeRecord, INTERRUPTED_AGAIN_REASON,
};
use crate::workflow::domain::state::{RunState, StepState};
use crate::workflow::domain::transitions::{validate_run_transition, validate_step_transition};
use crate::workflow::domain::types::RunRecord;
use crate::workflow::ports::repository::WorkflowRepository;

const RESUME_COLUMNS: &str = "run_id, state, auto_resume_used, last_interruption_reason,
     reason_code, expected_bytes, expected_item_key, updated_at, generation, last_revision";

fn map_resume(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunResumeRecord> {
    let state_text: String = row.get(1)?;
    let state = ResumeState::parse(&state_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    let expected: Option<i64> = row.get(5)?;
    Ok(RunResumeRecord {
        run_id: row.get(0)?,
        state,
        auto_resume_used: row.get::<_, i64>(2)? != 0,
        reason: row.get(3)?,
        reason_code: row.get(4)?,
        expected_bytes: expected.and_then(|value| u64::try_from(value).ok()),
        expected_item_key: row.get(6)?,
        updated_at: row.get(7)?,
        generation: row.get(8)?,
        last_revision: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
    })
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

impl SqliteWorkflowRepository {
    pub fn get_resume(
        &self,
        connection: &Connection,
        run_id: &str,
    ) -> Result<Option<RunResumeRecord>, WorkflowError> {
        connection
            .query_row(
                &format!("SELECT {RESUME_COLUMNS} FROM workflow_run_resume WHERE run_id = ?1"),
                params![run_id],
                map_resume,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Runs of `workflow_type` that are waiting on a resume decision or are in
    /// a resumed attempt, oldest first.
    pub fn list_resumable(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<Vec<(RunRecord, RunResumeRecord)>, WorkflowError> {
        let run_ids: Vec<String> = {
            let mut statement = connection.prepare(
                "SELECT r.id FROM workflow_runs r
                 JOIN workflow_run_resume m ON m.run_id = r.id
                 WHERE r.workflow_type = ?1
                   AND r.state IN ('paused', 'queued', 'retry_wait', 'running')
                   AND NOT (m.state = 'resuming' AND m.auto_resume_used = 0)
                 ORDER BY r.created_at ASC, r.id ASC",
            )?;
            let ids = statement
                .query_map(params![workflow_type], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            ids
        };
        let mut found = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            if let (Some(run), Some(resume)) = (
                self.get_run(connection, &run_id)?,
                self.get_resume(connection, &run_id)?,
            ) {
                found.push((run, resume));
            }
        }
        Ok(found)
    }

    /// Every run of `workflow_type` that is not finished, with no limit:
    /// queued, running, retry_wait, paused and cancelling.
    pub fn list_nonterminal_runs(
        &self,
        connection: &Connection,
        workflow_type: &str,
    ) -> Result<Vec<RunRecord>, WorkflowError> {
        let run_ids: Vec<String> = {
            let mut statement = connection.prepare(
                "SELECT id FROM workflow_runs
                 WHERE workflow_type = ?1
                   AND state IN ('queued', 'running', 'retry_wait', 'paused', 'cancelling')
                 ORDER BY created_at ASC, id ASC",
            )?;
            let ids = statement
                .query_map(params![workflow_type], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            ids
        };
        let mut runs = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            if let Some(run) = self.get_run(connection, &run_id)? {
                runs.push(run);
            }
        }
        Ok(runs)
    }

    /// Restart reconcile: make every non-terminal run of `workflow_type`
    /// resumable instead of failing it.
    ///
    /// * `cancelling` finishes as `cancelled` (the user already asked for it).
    /// * Every other non-terminal run becomes durably `paused`; a step that
    ///   was `running` becomes `retry_wait` so the existing claim path can run
    ///   it again after [`Self::begin_resume`].
    /// * Resume state: `needs_user` stays `needs_user`; a run that was in a
    ///   resumed attempt was interrupted a second time and becomes
    ///   `needs_user`; `auto_pending` stays `auto_pending`; anything else is
    ///   `auto_pending`, unless the run was already `paused` with no record
    ///   (then it waits for the user). The budget flag is never reset here.
    pub fn reconcile_for_resume(
        &self,
        connection: &Connection,
        workflow_type: &str,
        reason: &str,
        now: i64,
    ) -> Result<ReconcileSummary, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let runs: Vec<(String, RunState)> = {
            let mut statement = tx.prepare(
                "SELECT id, state FROM workflow_runs
                 WHERE workflow_type = ?1
                   AND state IN ('queued', 'running', 'cancelling', 'paused', 'retry_wait')
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = statement
                .query_map(params![workflow_type], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut parsed = Vec::with_capacity(rows.len());
            for (id, state) in rows {
                parsed.push((id, RunState::parse(&state)?));
            }
            parsed
        };
        let mut summary = ReconcileSummary::default();
        for (run_id, state) in runs {
            if state == RunState::Cancelling {
                tx.execute(
                    "UPDATE workflow_steps SET state = 'cancelled', error_message = ?2, updated_at = ?3
                     WHERE run_id = ?1 AND state IN ('pending', 'ready', 'running', 'retry_wait')",
                    params![run_id, "cancelled by user", now],
                )?;
                tx.execute(
                    "UPDATE workflow_runs
                     SET state = 'cancelled', error_message = ?2, updated_at = ?3, completed_at = ?3
                     WHERE id = ?1",
                    params![run_id, "cancelled by user", now],
                )?;
                append_event(&tx, &run_id, None, "restart_cancelled", "{}", now)?;
                summary.cancelled += 1;
                continue;
            }
            tx.execute(
                "UPDATE workflow_steps SET state = 'retry_wait', updated_at = ?2
                 WHERE run_id = ?1 AND state = 'running'",
                params![run_id, now],
            )?;
            let existing = self.get_resume(&tx, &run_id)?;
            let (next, next_reason, used) = match existing {
                Some(row) => match (row.state, row.auto_resume_used) {
                    (ResumeState::AutoPending, used) => (
                        ResumeState::AutoPending,
                        row.reason.unwrap_or_else(|| reason.to_string()),
                        used,
                    ),
                    (ResumeState::NeedsUser, used) => (
                        ResumeState::NeedsUser,
                        row.reason.unwrap_or_else(|| reason.to_string()),
                        used,
                    ),
                    // A resumed attempt (budget spent) was cut short again.
                    (ResumeState::Resuming, true) => (
                        ResumeState::NeedsUser,
                        INTERRUPTED_AGAIN_REASON.to_string(),
                        true,
                    ),
                    // Only an expected-size note: a first attempt was cut short.
                    (ResumeState::Resuming, false) => {
                        (ResumeState::AutoPending, reason.to_string(), false)
                    }
                },
                None if state == RunState::Paused => {
                    (ResumeState::NeedsUser, reason.to_string(), false)
                }
                None => (ResumeState::AutoPending, reason.to_string(), false),
            };
            if state != RunState::Paused {
                validate_run_transition(state, RunState::Paused)?;
                tx.execute(
                    "UPDATE workflow_runs SET state = 'paused', error_message = ?2, updated_at = ?3
                     WHERE id = ?1",
                    params![run_id, next_reason, now],
                )?;
            }
            tx.execute(
                "INSERT INTO workflow_run_resume
                    (run_id, state, auto_resume_used, last_interruption_reason, updated_at, generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)
                 ON CONFLICT(run_id) DO UPDATE SET
                    state = excluded.state,
                    auto_resume_used = excluded.auto_resume_used,
                    last_interruption_reason = excluded.last_interruption_reason,
                    updated_at = excluded.updated_at,
                    generation = workflow_run_resume.generation + 1",
                params![run_id, next.as_str(), i64::from(used), next_reason, now],
            )?;
            append_event(
                &tx,
                &run_id,
                None,
                "restart_interrupted",
                &format!("{{\"resume\":\"{}\"}}", next.as_str()),
                now,
            )?;
            summary.resumable += 1;
        }
        tx.commit()?;
        Ok(summary)
    }

    /// Admit one resumed attempt. Returns `false` (and changes nothing) when
    /// the run is not paused or the trigger is not allowed, which makes
    /// repeated and concurrent calls idempotent: exactly one caller wins.
    ///
    /// `Auto` requires `auto_pending` with the budget unspent. `Manual`
    /// accepts `needs_user` or `auto_pending`. Both set `auto_resume_used`
    /// and `resuming` and requeue the run in the same transaction, so the
    /// budget is durable before any attempt can be claimed.
    pub fn begin_resume(
        &self,
        connection: &Connection,
        run_id: &str,
        trigger: ResumeTrigger,
        now: i64,
    ) -> Result<bool, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let Some(run) = self.get_run(&tx, run_id)? else {
            return Ok(false);
        };
        let Some(row) = self.get_resume(&tx, run_id)? else {
            return Ok(false);
        };
        if run.state != RunState::Paused {
            return Ok(false);
        }
        let allowed = match trigger {
            ResumeTrigger::Auto => row.state == ResumeState::AutoPending && !row.auto_resume_used,
            ResumeTrigger::Manual => {
                matches!(row.state, ResumeState::NeedsUser | ResumeState::AutoPending)
            }
            ResumeTrigger::ManualAt(generation) => {
                row.generation == generation
                    && matches!(row.state, ResumeState::NeedsUser | ResumeState::AutoPending)
            }
        };
        if !allowed {
            return Ok(false);
        }
        let retry_deadline: Option<i64> = tx.query_row(
            "SELECT MIN(CASE WHEN state = 'retry_wait' THEN updated_at END)
             FROM workflow_steps WHERE run_id = ?1",
            params![run_id],
            |result| result.get(0),
        )?;
        let (target, run_updated_at) = match retry_deadline {
            Some(deadline) => (RunState::RetryWait, deadline.min(now)),
            None => (RunState::Queued, now),
        };
        validate_run_transition(run.state, target)?;
        tx.execute(
            "UPDATE workflow_runs SET state = ?2, error_message = NULL, updated_at = ?3
             WHERE id = ?1",
            params![run_id, target.as_str(), run_updated_at],
        )?;
        tx.execute(
            "UPDATE workflow_run_resume
             SET state = 'resuming', auto_resume_used = 1, updated_at = ?2
             WHERE run_id = ?1",
            params![run_id, now],
        )?;
        append_event(
            &tx,
            run_id,
            None,
            "run_resume_started",
            &format!("{{\"trigger\":\"{}\"}}", trigger.as_str()),
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// A resumed attempt failed: pause the run for the user instead of
    /// failing it. Step `running -> retry_wait`, run `running -> paused`, and
    /// the record becomes `needs_user`, all in one transaction. A run that is
    /// being cancelled is left alone (returns `false`).
    pub fn pause_after_failed_attempt(
        &self,
        connection: &Connection,
        run_id: &str,
        step_id: &str,
        reason: &str,
        reason_code: Option<&str>,
        last_revision: u64,
        now: i64,
    ) -> Result<bool, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let Some(run) = self.get_run(&tx, run_id)? else {
            return Err(WorkflowError::RunNotFound(run_id.to_string()));
        };
        if run.state != RunState::Running {
            return Ok(false);
        }
        let step = self
            .list_steps_for_run(&tx, run_id)?
            .into_iter()
            .find(|step| step.id == step_id);
        if let Some(step) = step {
            if step.state == StepState::Running {
                validate_step_transition(step.state, StepState::RetryWait)?;
                tx.execute(
                    "UPDATE workflow_steps SET state = 'retry_wait', error_message = ?2, updated_at = ?3
                     WHERE id = ?1",
                    params![step_id, reason, now],
                )?;
            }
        }
        validate_run_transition(run.state, RunState::Paused)?;
        tx.execute(
            "UPDATE workflow_runs SET state = 'paused', error_message = ?2, updated_at = ?3
             WHERE id = ?1",
            params![run_id, reason, now],
        )?;
        tx.execute(
            "INSERT INTO workflow_run_resume
                (run_id, state, auto_resume_used, last_interruption_reason, reason_code, updated_at, generation, last_revision)
             VALUES (?1, 'needs_user', 1, ?2, ?3, ?4, 1, ?5)
             ON CONFLICT(run_id) DO UPDATE SET
                state = 'needs_user',
                last_interruption_reason = excluded.last_interruption_reason,
                reason_code = excluded.reason_code,
                updated_at = excluded.updated_at,
                generation = workflow_run_resume.generation + 1,
                last_revision = MAX(workflow_run_resume.last_revision, excluded.last_revision)",
            params![
                run_id,
                reason,
                reason_code,
                now,
                i64::try_from(last_revision).unwrap_or(i64::MAX)
            ],
        )?;
        append_event(&tx, run_id, Some(step_id), "run_paused_for_user", "{}", now)?;
        tx.commit()?;
        Ok(true)
    }

    /// Park a paused run for the user with `reason` without starting an
    /// attempt (helper missing, folder unavailable, not enough disk space).
    /// The budget flag is untouched. Returns `false` when the run is not
    /// paused.
    pub fn park_for_user(
        &self,
        connection: &Connection,
        run_id: &str,
        reason: &str,
        reason_code: Option<&str>,
        now: i64,
        expected_generation: Option<i64>,
    ) -> Result<bool, WorkflowError> {
        let tx = connection.unchecked_transaction()?;
        let Some(run) = self.get_run(&tx, run_id)? else {
            return Ok(false);
        };
        if run.state != RunState::Paused {
            return Ok(false);
        }
        // A park decided against an older pause must not overwrite a newer one.
        if let (Some(expected), Some(current)) = (expected_generation, self.get_resume(&tx, run_id)?)
        {
            if current.generation != expected {
                return Ok(false);
            }
        }
        tx.execute(
            "INSERT INTO workflow_run_resume
                (run_id, state, auto_resume_used, last_interruption_reason, reason_code, updated_at, generation)
             VALUES (?1, 'needs_user', 0, ?2, ?3, ?4, 1)
             ON CONFLICT(run_id) DO UPDATE SET
                state = 'needs_user',
                last_interruption_reason = excluded.last_interruption_reason,
                reason_code = excluded.reason_code,
                updated_at = excluded.updated_at,
                generation = workflow_run_resume.generation + 1",
            params![run_id, reason, reason_code, now],
        )?;
        tx.execute(
            "UPDATE workflow_runs SET error_message = ?2, updated_at = ?3 WHERE id = ?1",
            params![run_id, reason, now],
        )?;
        append_event(&tx, run_id, None, "run_parked_for_user", "{}", now)?;
        tx.commit()?;
        Ok(true)
    }

    /// Remember how large the current item is so a later resume can check
    /// free disk space. Only a running run gets a note; an existing record
    /// keeps its state and budget.
    pub fn set_expected_bytes(
        &self,
        connection: &Connection,
        run_id: &str,
        item_key: &str,
        bytes: u64,
        now: i64,
    ) -> Result<(), WorkflowError> {
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        connection.execute(
            "INSERT INTO workflow_run_resume
                (run_id, state, auto_resume_used, expected_bytes, expected_item_key, updated_at)
             SELECT id, 'resuming', 0, ?2, ?3, ?4 FROM workflow_runs
             WHERE id = ?1 AND state = 'running'
             ON CONFLICT(run_id) DO UPDATE SET
                expected_bytes = excluded.expected_bytes,
                expected_item_key = excluded.expected_item_key",
            params![run_id, bytes, item_key, now],
        )?;
        Ok(())
    }
}
