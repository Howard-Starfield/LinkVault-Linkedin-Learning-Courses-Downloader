//! LinkedIn request payload stored on workflow_runs.request_json.

use serde::{Deserialize, Serialize};

use crate::cache::JobRecord;
use crate::workflow::domain::state::RunState;
use crate::workflow::domain::types::RunRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedInWorkflowRequest {
    pub schema_version: u32,
    pub course_slug: String,
    pub source_url: String,
    pub selected_quality: String,
    pub download_videos: bool,
    pub download_exercises: bool,
    pub download_subtitles: bool,
    pub download_quizzes: bool,
    pub quiz_hints_json: String,
    pub scheduled_at: Option<i64>,
}

pub fn job_from_run(run: &RunRecord) -> JobRecord {
    let request = serde_json::from_str::<LinkedInWorkflowRequest>(&run.request_json).ok();
    JobRecord {
        id: run.id.clone(),
        course_slug: request
            .as_ref()
            .map(|value| value.course_slug.clone())
            .unwrap_or_else(|| run.id.clone()),
        source_url: request
            .as_ref()
            .map(|value| value.source_url.clone())
            .unwrap_or_default(),
        status: run_state_to_linkedin_status(run.state).to_string(),
        selected_quality: request
            .as_ref()
            .map(|value| value.selected_quality.clone())
            .unwrap_or_else(|| "720p".to_string()),
        download_videos: request
            .as_ref()
            .map(|value| value.download_videos)
            .unwrap_or(true),
        download_exercises: request
            .as_ref()
            .map(|value| value.download_exercises)
            .unwrap_or(true),
        download_subtitles: request
            .as_ref()
            .map(|value| value.download_subtitles)
            .unwrap_or(true),
        download_quizzes: request
            .as_ref()
            .map(|value| value.download_quizzes)
            .unwrap_or(true),
        quiz_hints_json: request
            .as_ref()
            .map(|value| value.quiz_hints_json.clone())
            .unwrap_or_else(|| "[]".to_string()),
        output_dir: run.output_root.clone(),
        paused: matches!(run.state, RunState::Paused),
        scheduled_at: request.as_ref().and_then(|value| value.scheduled_at),
        created_at: run.created_at,
        updated_at: run.updated_at,
    }
}

pub fn merge_linkedin_jobs(workflow: Vec<JobRecord>, legacy: Vec<JobRecord>) -> Vec<JobRecord> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::domain::state::RunState;
    use crate::workflow::domain::types::{RunRecord, WorkflowType};

    fn sample_run(id: &str, state: RunState, created_at: i64) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_type: WorkflowType::linkedin_download(),
            provider: "linkedin".to_string(),
            state,
            legacy_origin: None,
            legacy_id: None,
            request_json: r#"{"schemaVersion":1,"courseSlug":"foo","sourceUrl":"https://www.linkedin.com/learning/foo","selectedQuality":"720p","downloadVideos":true,"downloadExercises":true,"downloadSubtitles":true,"downloadQuizzes":true,"quizHintsJson":"[]","scheduledAt":null}"#.to_string(),
            output_root: "C:/downloads".to_string(),
            error_message: None,
            created_at,
            updated_at: created_at,
            completed_at: None,
        }
    }

    #[test]
    fn retry_wait_projects_as_queued() {
        let job = job_from_run(&sample_run("job-1", RunState::RetryWait, 10));
        assert_eq!(job.status, "queued");
        assert_eq!(job.course_slug, "foo");
    }

    #[test]
    fn merge_prefers_workflow_jobs_on_id_collision() {
        let workflow = vec![job_from_run(&sample_run("job-1", RunState::Queued, 20))];
        let legacy = vec![
            JobRecord {
                id: "job-1".to_string(),
                course_slug: "legacy".to_string(),
                source_url: "https://www.linkedin.com/learning/legacy".to_string(),
                status: "failed".to_string(),
                selected_quality: "1080".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: ".".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 1,
                updated_at: 1,
            },
            JobRecord {
                id: "job-legacy".to_string(),
                course_slug: "kept".to_string(),
                source_url: "https://www.linkedin.com/learning/kept".to_string(),
                status: "queued".to_string(),
                selected_quality: "720".to_string(),
                download_videos: true,
                download_exercises: true,
                download_subtitles: true,
                download_quizzes: true,
                quiz_hints_json: "[]".to_string(),
                output_dir: ".".to_string(),
                paused: false,
                scheduled_at: None,
                created_at: 5,
                updated_at: 5,
            },
        ];
        let merged = merge_linkedin_jobs(workflow, legacy);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "job-1");
        assert_eq!(merged[0].course_slug, "foo");
        assert_eq!(merged[1].id, "job-legacy");
    }
}

fn run_state_to_linkedin_status(state: RunState) -> &'static str {
    match state {
        RunState::Queued | RunState::Paused | RunState::RetryWait => "queued",
        RunState::Running | RunState::Cancelling => "active",
        RunState::Succeeded | RunState::SucceededWithWarnings => "completed",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
    }
}
