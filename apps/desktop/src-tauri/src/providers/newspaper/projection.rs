//! Newspaper request payload stored on workflow_runs.request_json.

use serde::{Deserialize, Serialize};

use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::RunRecord;

use super::models::NewspaperJob;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperWorkflowRequest {
    pub schema_version: u32,
    pub batch_id: String,
    pub edition_code: String,
    pub edition_name: String,
    pub edition_publication_date: String,
    pub publication_date: String,
    pub queue_position: i64,
    pub delay_seconds: u32,
    pub scheduled_at: Option<i64>,
    pub optimize_images: bool,
}

pub fn job_from_run(run: &RunRecord) -> NewspaperJob {
    let request = serde_json::from_str::<NewspaperWorkflowRequest>(&run.request_json).ok();
    NewspaperJob {
        id: run.id.clone(),
        batch_id: request
            .as_ref()
            .map(|value| value.batch_id.clone())
            .unwrap_or_default(),
        edition_code: request
            .as_ref()
            .map(|value| value.edition_code.clone())
            .unwrap_or_default(),
        edition_name: request
            .as_ref()
            .map(|value| value.edition_name.clone())
            .unwrap_or_default(),
        publication_date: request
            .as_ref()
            .map(|value| value.publication_date.clone())
            .unwrap_or_default(),
        status: run_state_to_newspaper_status(run.state).to_string(),
        output_dir: run.output_root.clone(),
        page_count: 0,
        completed_count: 0,
        failed_count: 0,
        retry_at: match run.state {
            RunState::RetryWait => Some(run.updated_at),
            _ => None,
        },
        retry_count: 0,
        warning: run.error_message.clone(),
        queue_position: request
            .as_ref()
            .map(|value| value.queue_position)
            .unwrap_or(0),
        paused: matches!(run.state, RunState::Paused),
        dismissed: false,
        created_at: run.created_at,
        updated_at: run.updated_at,
        completed_at: run.completed_at,
    }
}

pub fn merge_newspaper_jobs(
    workflow: Vec<NewspaperJob>,
    legacy: Vec<NewspaperJob>,
) -> Vec<NewspaperJob> {
    let mut merged = legacy;
    let seen: std::collections::HashSet<String> = merged.iter().map(|job| job.id.clone()).collect();
    for job in workflow {
        if !seen.contains(&job.id) {
            merged.push(job);
        }
    }
    merged.sort_by(|left, right| {
        left.dismissed
            .cmp(&right.dismissed)
            .then_with(|| left.queue_position.cmp(&right.queue_position))
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
    merged
}

fn run_state_to_newspaper_status(state: RunState) -> &'static str {
    match state {
        RunState::Queued | RunState::Paused | RunState::RetryWait => "queued",
        RunState::Running | RunState::Cancelling => "active",
        RunState::Succeeded | RunState::SucceededWithWarnings => "completed",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::types::WorkflowType;

    fn sample_run(id: &str, state: RunState) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_type: WorkflowType::newspaper_download(),
            provider: "newspaper".to_string(),
            state,
            legacy_origin: None,
            legacy_id: None,
            request_json: r#"{"schemaVersion":1,"batchId":"batch-1","editionCode":"NY","editionName":"World Journal","editionPublicationDate":"","publicationDate":"2026-07-24","queuePosition":3,"delaySeconds":15,"scheduledAt":null,"optimizeImages":true}"#.to_string(),
            output_root: "C:/papers/NY/2026-07-24".to_string(),
            error_message: None,
            created_at: 10,
            updated_at: 20,
            completed_at: None,
        }
    }

    #[test]
    fn retry_wait_projects_as_queued() {
        let job = job_from_run(&sample_run("job-1", RunState::RetryWait));
        assert_eq!(job.status, "queued");
        assert_eq!(job.edition_code, "NY");
        assert_eq!(job.retry_at, Some(20));
    }

    #[test]
    fn merge_prefers_legacy_rows_for_page_counts() {
        let workflow = vec![job_from_run(&sample_run("job-1", RunState::Succeeded))];
        let mut legacy = job_from_run(&sample_run("job-1", RunState::Succeeded));
        legacy.status = "partial".to_string();
        legacy.page_count = 12;
        let merged = merge_newspaper_jobs(workflow, vec![legacy]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, "partial");
        assert_eq!(merged[0].page_count, 12);
    }
}
