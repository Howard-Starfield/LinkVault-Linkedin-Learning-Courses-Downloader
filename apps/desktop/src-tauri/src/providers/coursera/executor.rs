//! Coursera step executor. Domain download logic stays provider-owned.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::coursera::auth;
use crate::coursera::config::CourseraOptions;
use crate::coursera::downloader::NativeDownloader;
use crate::coursera::orchestrator::{CourseEvent, CourseSummary, CourseraDownloader};
use crate::coursera::projection::CourseraWorkflowRequest;
use crate::workflow::domain::types::{RunRecord, StepRecord};
use crate::workflow::ports::executor::{ExecutorOutcome, StepExecutor};

pub struct CourseraDownloadExecutor {
    pub data_dir: PathBuf,
    pub cancellation: Arc<AtomicBool>,
}

impl StepExecutor for CourseraDownloadExecutor {
    fn workflow_type(&self) -> &'static str {
        "coursera_download"
    }

    fn execute(&self, run: &RunRecord, _step: &StepRecord) -> ExecutorOutcome {
        if self.cancellation.load(Ordering::Relaxed) {
            return ExecutorOutcome::cancelled("Coursera download was cancelled".to_string());
        }
        let request = match serde_json::from_str::<CourseraWorkflowRequest>(&run.request_json) {
            Ok(request) => request,
            Err(error) => return ExecutorOutcome::failed(error.to_string()),
        };
        let mut options = request.options;
        options.class_names = vec![request.class_name.clone()];
        options.output_dir = PathBuf::from(&run.output_root);
        match download_course(
            &request.class_name,
            &options,
            &self.data_dir,
            Arc::clone(&self.cancellation),
        ) {
            Ok((summary, _events)) => outcome_from_summary(&summary, &self.cancellation),
            Err(error) => {
                if self.cancellation.load(Ordering::Relaxed) {
                    ExecutorOutcome::cancelled(error)
                } else {
                    ExecutorOutcome::failed(error)
                }
            }
        }
    }
}

fn outcome_from_summary(summary: &CourseSummary, cancellation: &AtomicBool) -> ExecutorOutcome {
    if cancellation.load(Ordering::Relaxed) {
        return ExecutorOutcome::cancelled("Coursera download was cancelled".to_string());
    }
    let payload = serde_json::json!({
        "skipped": summary.skipped,
        "failed": summary.failed
    })
    .to_string();
    if summary.failed.is_empty() && summary.completed {
        ExecutorOutcome::succeeded(payload)
    } else {
        ExecutorOutcome::failed(format!(
            "Coursera download finished with {} failed item(s)",
            summary.failed.len()
        ))
    }
}

pub fn download_course(
    class_name: &str,
    options: &CourseraOptions,
    data_dir: &std::path::Path,
    cancellation: Arc<AtomicBool>,
) -> Result<(CourseSummary, Vec<CourseEvent>), String> {
    let cauth = auth::read_cached_cauth(data_dir)
        .ok_or_else(|| "Saved Coursera CAUTH token is required before downloading.".to_string())?;
    let cookie_header = auth::make_cookie_values(&cauth)
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let class_name = class_name.to_string();
    rt.block_on(async move {
        let session = auth::AuthSession::from_cauth(cauth, "saved-cauth")
            .await
            .map_err(|error| error.to_string())?;
        let syllabus = crate::coursera::syllabus::fetch_syllabus(&session.client, &class_name)
            .await
            .map_err(|error| error.to_string())?;
        let modules = crate::coursera::syllabus::parse_syllabus(&syllabus)
            .map_err(|error| error.to_string())?;
        let events: Arc<Mutex<Vec<CourseEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&events);
        let on_event: Arc<dyn Fn(CourseEvent) + Send + Sync> = Arc::new(move |event| {
            if let Ok(mut guard) = captured_events.lock() {
                guard.push(event);
            }
        });
        let downloader = NativeDownloader::with_cookie_header(Some(cookie_header))
            .map_err(|error| error.to_string())?;
        let output_root = options.output_dir.clone();
        let coursera_downloader = CourseraDownloader {
            client: &session.client,
            options,
            output_root: &output_root,
            downloader: Arc::new(downloader),
            cancellation,
            slug: &class_name,
            on_event: Some(on_event),
        };
        let summary = coursera_downloader
            .download_modules(modules)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<(CourseSummary, Vec<CourseEvent>), String>((
            summary,
            events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::config::StartCourseraRequest;

    fn sample_request() -> CourseraWorkflowRequest {
        let req = StartCourseraRequest {
            classes: vec!["ml-005".to_string()],
            output_dir: ".".to_string(),
            force_redownload: false,
            selected_resolution: "540p".to_string(),
            formats: Vec::new(),
            ignored_formats: Vec::new(),
            subtitle_language: "all".to_string(),
            download_quizzes: false,
            download_notebooks: false,
            download_about: false,
            resume: false,
            overwrite: false,
            generate_playlists: false,
            section_filter: String::new(),
            lecture_filter: String::new(),
            resource_filter: String::new(),
            jobs: 1,
            download_delay_seconds: 60,
        };
        CourseraWorkflowRequest {
            schema_version: 1,
            class_name: "ml-005".to_string(),
            force_redownload: false,
            options: req.into_options().unwrap(),
        }
    }

    #[test]
    fn missing_token_fails_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let executor = CourseraDownloadExecutor {
            data_dir: temp.path().to_path_buf(),
            cancellation: Arc::new(AtomicBool::new(false)),
        };
        let run = RunRecord {
            id: "coursera-ml-test".to_string(),
            workflow_type: crate::workflow::domain::types::WorkflowType::coursera_download(),
            provider: "coursera".to_string(),
            state: crate::workflow::domain::state::RunState::Running,
            legacy_origin: None,
            legacy_id: None,
            request_json: serde_json::to_string(&sample_request()).unwrap(),
            output_root: ".".to_string(),
            error_message: None,
            created_at: 1,
            updated_at: 1,
            completed_at: None,
        };
        let step = StepRecord {
            id: "step".to_string(),
            run_id: run.id.clone(),
            step_key: "execute".to_string(),
            step_type: crate::workflow::domain::types::StepType::coursera_execute(),
            state: crate::workflow::domain::state::StepState::Running,
            attempt: 1,
            error_message: None,
            created_at: 1,
            updated_at: 1,
        };
        let outcome = executor.execute(&run, &step);
        assert!(!outcome.succeeded);
        assert!(outcome.error_message.unwrap_or_default().contains("CAUTH"));
    }
}
