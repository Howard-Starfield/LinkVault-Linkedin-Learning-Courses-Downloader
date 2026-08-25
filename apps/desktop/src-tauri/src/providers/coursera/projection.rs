//! Coursera request payload stored on workflow_runs.request_json.

use serde::{Deserialize, Serialize};

use crate::coursera::config::CourseraOptions;
use crate::coursera::job::{CourseraJob, PersistedCourseraEvent};
use crate::coursera::workflow_compat::run_state_to_coursera_status;
use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::{RunRecord, WorkflowEventRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraWorkflowRequest {
    pub schema_version: u32,
    pub class_name: String,
    pub force_redownload: bool,
    pub options: CourseraOptions,
}

pub fn workflow_run_to_coursera_job(run: &RunRecord) -> CourseraJob {
    let request = parse_workflow_request(&run.request_json);
    let class_name = request
        .as_ref()
        .map(|value| value.class_name.clone())
        .or_else(|| json_class_name(&run.request_json))
        .unwrap_or_else(|| run.id.clone());
    let options_json = request
        .as_ref()
        .and_then(|value| serde_json::to_string(&value.options).ok())
        .unwrap_or_else(|| run.request_json.clone());
    CourseraJob {
        id: run.id.clone(),
        class_name,
        status: run_state_to_coursera_status(run.state).to_string(),
        options_json,
        output_dir: run.output_root.clone(),
        created_at: run.created_at,
        updated_at: run.updated_at,
        counts_json: "{}".to_string(),
    }
}

pub fn workflow_event_to_coursera_event(event: &WorkflowEventRecord) -> PersistedCourseraEvent {
    PersistedCourseraEvent {
        id: event.id,
        job_id: event.run_id.clone(),
        event_type: event.event_type.clone(),
        payload_json: event.payload_json.clone(),
        created_at: event.created_at,
    }
}

pub fn merge_coursera_jobs(
    workflow: Vec<CourseraJob>,
    legacy: Vec<CourseraJob>,
) -> Vec<CourseraJob> {
    let mut merged = workflow;
    let seen: std::collections::HashSet<String> = merged.iter().map(|job| job.id.clone()).collect();
    for job in legacy {
        if !seen.contains(&job.id) {
            merged.push(job);
        }
    }
    merged.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    merged
}

pub fn completed_class_names(
    jobs: &[CourseraJob],
    runs: &[RunRecord],
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for job in jobs {
        if job.status.eq_ignore_ascii_case("completed") {
            names.insert(job.class_name.clone());
        }
    }
    for run in runs {
        if matches!(
            run.state,
            RunState::Succeeded | RunState::SucceededWithWarnings
        ) {
            if let Some(request) = parse_workflow_request(&run.request_json) {
                names.insert(request.class_name);
            }
        }
    }
    names
}

pub fn parse_workflow_request(request_json: &str) -> Option<CourseraWorkflowRequest> {
    serde_json::from_str(request_json).ok()
}

fn json_class_name(request_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(request_json)
        .ok()
        .and_then(|value| {
            value
                .get("className")
                .and_then(|name| name.as_str())
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::types::WorkflowType;

    fn sample_run(id: &str, state: RunState) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_type: WorkflowType::coursera_download(),
            provider: "coursera".to_string(),
            state,
            legacy_origin: None,
            legacy_id: None,
            request_json:
                "{\"schemaVersion\":1,\"className\":\"ml-005\",\"forceRedownload\":false}"
                    .to_string(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 10,
            updated_at: 10,
            completed_at: None,
        }
    }

    #[test]
    fn workflow_run_projects_queued_status() {
        let job = workflow_run_to_coursera_job(&sample_run("wf-1", RunState::Queued));
        assert_eq!(job.id, "wf-1");
        assert_eq!(job.class_name, "ml-005");
        assert_eq!(job.status, "Queued");
    }

    #[test]
    fn merge_prefers_workflow_row_for_same_id() {
        let workflow = vec![CourseraJob {
            id: "same".into(),
            class_name: "new".into(),
            status: "Queued".into(),
            options_json: "{}".into(),
            output_dir: ".".into(),
            created_at: 2,
            updated_at: 2,
            counts_json: "{}".into(),
        }];
        let legacy = vec![CourseraJob {
            id: "same".into(),
            class_name: "old".into(),
            status: "Completed".into(),
            options_json: "{}".into(),
            output_dir: ".".into(),
            created_at: 1,
            updated_at: 1,
            counts_json: "{}".into(),
        }];
        let merged = merge_coursera_jobs(workflow, legacy);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].class_name, "new");
    }
}
