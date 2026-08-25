//! Orchestrator — walks `ModulesV1` and dispatches each item.

#![allow(dead_code)] // Phase 8 — wired by Phase 10

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use reqwest::Client;
use serde::Serialize;

use crate::coursera::config::CourseraOptions;
use crate::coursera::downloader::Downloader;
use crate::coursera::error::CourseraResult;
use crate::coursera::extractors::{dispatch, ExtractionContext};
use crate::coursera::syllabus::ModulesV1;

/// Eight-variant `CourseEvent` tagged enum. Frontend subscribes to
/// `coursera://job-event`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CourseEvent {
    CourseStarted {
        slug: String,
    },
    ModuleStarted {
        index: usize,
        name: String,
    },
    SectionStarted {
        index: usize,
        name: String,
        dir: String,
    },
    FileStarted {
        url: String,
        dest: String,
    },
    FileProgress {
        url: String,
        bytes: u64,
        total: Option<u64>,
    },
    FileFinished {
        url: String,
        dest: String,
        bytes: u64,
    },
    FileSkipped {
        url: String,
        reason: String,
    },
    FileFailed {
        url: String,
        error: String,
        retryable: bool,
    },
    ModuleFinished {
        index: usize,
    },
    CourseFinished {
        slug: String,
        completed: bool,
        skipped: Vec<String>,
        failed: Vec<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct CourseSummary {
    pub completed: bool,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
}

pub struct CourseraDownloader<'a> {
    pub client: &'a Client,
    pub options: &'a CourseraOptions,
    pub output_root: &'a PathBuf,
    pub downloader: Arc<dyn Downloader>,
    pub cancellation: Arc<AtomicBool>,
    pub slug: &'a str,
    pub on_event: Option<Arc<dyn Fn(CourseEvent) + Send + Sync>>,
}

impl<'a> CourseraDownloader<'a> {
    pub async fn download_modules(&self, modules: ModulesV1) -> CourseraResult<CourseSummary> {
        self.emit(CourseEvent::CourseStarted {
            slug: self.slug.to_string(),
        });
        let ctx = ExtractionContext::new(self.client, self.options);
        let mut summary = CourseSummary::default();
        for (m_idx, module) in modules.modules.iter().enumerate() {
            if self.cancellation.load(Ordering::Relaxed) {
                break;
            }
            self.emit(CourseEvent::ModuleStarted {
                index: m_idx,
                name: module.name.clone(),
            });
            for (l_idx, lesson) in module.lessons.iter().enumerate() {
                if self.cancellation.load(Ordering::Relaxed) {
                    break;
                }
                let dir_name = crate::coursera::format::build_section_dir_name(
                    m_idx + 1,
                    &module.name,
                    l_idx + 1,
                    &lesson.name,
                    self.options,
                );
                self.emit(CourseEvent::SectionStarted {
                    index: l_idx,
                    name: lesson.name.clone(),
                    dir: dir_name.clone(),
                });
                for item in &lesson.items {
                    if self.cancellation.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = dispatch(&ctx, item).await;
                    self.process_dispatch(item, &result, &dir_name, &mut summary)
                        .await;
                }
            }
            self.emit(CourseEvent::ModuleFinished { index: m_idx });
        }
        summary.completed = self.cancellation.load(Ordering::Relaxed) == false;
        self.emit(CourseEvent::CourseFinished {
            slug: self.slug.to_string(),
            completed: summary.completed,
            skipped: summary.skipped.clone(),
            failed: summary.failed.clone(),
        });
        Ok(summary)
    }

    async fn process_dispatch(
        &self,
        item: &crate::coursera::syllabus::ItemV2,
        result: &crate::coursera::extractors::DispatchResult,
        dir: &str,
        summary: &mut CourseSummary,
    ) {
        use crate::coursera::extractors::DispatchResult;
        match result {
            DispatchResult::Links(links) => {
                let filtered =
                    crate::coursera::filter::find_resources_to_get(links.clone(), self.options);
                for link in filtered {
                    if !link.url.starts_with("http://") && !link.url.starts_with("https://") {
                        self.emit(CourseEvent::FileSkipped {
                            url: link.url.clone(),
                            reason: "non-http Coursera asset resolver is not implemented yet"
                                .to_string(),
                        });
                        summary.skipped.push(link.url.clone());
                        continue;
                    }
                    let Some(dest) = crate::coursera::format::safe_join(
                        self.output_root,
                        &[self.slug, dir, &link.filename],
                    ) else {
                        self.emit(CourseEvent::FileFailed {
                            url: link.url.clone(),
                            error: "refusing to write outside the Coursera output root".into(),
                            retryable: false,
                        });
                        summary.failed.push(link.url.clone());
                        continue;
                    };
                    self.emit(CourseEvent::FileStarted {
                        url: link.url.clone(),
                        dest: dest.to_string_lossy().to_string(),
                    });
                    let mut on_progress = |p: crate::coursera::downloader::DownloadProgress| match p
                    {
                        crate::coursera::downloader::DownloadProgress::Started { url, total } => {
                            self.emit(CourseEvent::FileProgress {
                                url,
                                bytes: 0,
                                total,
                            });
                        }
                        crate::coursera::downloader::DownloadProgress::Progress {
                            url,
                            bytes,
                            total,
                        } => {
                            self.emit(CourseEvent::FileProgress { url, bytes, total });
                        }
                        crate::coursera::downloader::DownloadProgress::Finished { .. } => {}
                    };
                    match self.downloader.download(&link.url, &dest, &mut on_progress) {
                        Ok(()) => {
                            self.emit(CourseEvent::FileFinished {
                                url: link.url.clone(),
                                dest: dest.to_string_lossy().to_string(),
                                bytes: 0,
                            });
                        }
                        Err(e) => {
                            let retryable = e.is_retryable();
                            self.emit(CourseEvent::FileFailed {
                                url: link.url.clone(),
                                error: e.to_string(),
                                retryable,
                            });
                            summary.failed.push(link.url.clone());
                        }
                    }
                }
            }
            DispatchResult::QuizHtml(html) | DispatchResult::ExamHtml(html) => {
                let Some(dest) = crate::coursera::format::safe_join(
                    self.output_root,
                    &[self.slug, dir, &html.filename],
                ) else {
                    summary.failed.push(html.filename.clone());
                    self.emit(CourseEvent::FileFailed {
                        url: html.filename.clone(),
                        error: "refusing to write outside the Coursera output root".into(),
                        retryable: false,
                    });
                    return;
                };
                if let Err(e) = crate::coursera::format::create_parent_dir(&dest) {
                    summary.failed.push(html.filename.clone());
                    self.emit(CourseEvent::FileFailed {
                        url: html.filename.clone(),
                        error: e.to_string(),
                        retryable: false,
                    });
                    return;
                }
                let res = std::fs::write(&dest, &html.html);
                if let Err(e) = res {
                    summary.failed.push(html.filename.clone());
                    self.emit(CourseEvent::FileFailed {
                        url: html.filename.clone(),
                        error: e.to_string(),
                        retryable: false,
                    });
                } else {
                    self.emit(CourseEvent::FileFinished {
                        url: html.filename.clone(),
                        dest: dest.to_string_lossy().to_string(),
                        bytes: html.html.len() as u64,
                    });
                }
            }
            DispatchResult::Skipped { reason } => {
                self.emit(CourseEvent::FileSkipped {
                    url: item.id.clone(),
                    reason: reason.clone(),
                });
                summary.skipped.push(item.id.clone());
            }
        }
    }

    fn emit(&self, ev: CourseEvent) {
        if let Some(handler) = &self.on_event {
            handler(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coursera::config::CourseraOptions;
    use crate::coursera::downloader::NativeDownloader;
    use crate::coursera::syllabus::ModulesV1;
    use crate::coursera::utils::mkdir_p;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn empty_modules() -> ModulesV1 {
        ModulesV1::default()
    }

    #[test]
    fn empty_modules_produces_a_course_finished_event() {
        let tmp = tempdir().unwrap();
        let out = tmp.path().to_path_buf();
        let client = crate::coursera::client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let downloader: Arc<dyn Downloader> = Arc::new(NativeDownloader::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let captured: std::sync::Arc<std::sync::Mutex<Vec<CourseEvent>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = captured.clone();
        let on_event: Arc<dyn Fn(CourseEvent) + Send + Sync> = Arc::new(move |e| {
            captured2.lock().unwrap().push(e);
        });
        let d = CourseraDownloader {
            client: &client,
            options: &opts,
            output_root: &out,
            downloader,
            cancellation: cancel,
            slug: "ml-005",
            on_event: Some(on_event),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let summary = rt.block_on(d.download_modules(empty_modules())).unwrap();
        assert!(summary.completed);
        let events = captured.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, CourseEvent::CourseStarted { slug } if slug == "ml-005")));
        assert!(events
            .iter()
            .any(|e| matches!(e, CourseEvent::CourseFinished { .. })));
        mkdir_p(Path::new(".")).unwrap(); // touch
    }

    fn dummy_item() -> crate::coursera::syllabus::ItemV2 {
        crate::coursera::syllabus::ItemV2 {
            id: "item-1".into(),
            type_name: "quiz".into(),
            name: "Quiz".into(),
            slug: "quiz".into(),
            asset_id: None,
            raw: serde_json::json!({}),
        }
    }

    struct RecordingDownloader {
        dests: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl Downloader for RecordingDownloader {
        fn download(
            &self,
            _url: &str,
            dest: &Path,
            _on_progress: &mut dyn FnMut(crate::coursera::downloader::DownloadProgress),
        ) -> Result<(), crate::coursera::downloader::DownloadError> {
            self.dests.lock().unwrap().push(dest.to_path_buf());
            Ok(())
        }
    }

    #[test]
    fn quiz_html_rejects_path_traversal_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().to_path_buf();
        let client = crate::coursera::client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let downloader: Arc<dyn Downloader> = Arc::new(NativeDownloader::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let captured: std::sync::Arc<std::sync::Mutex<Vec<CourseEvent>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = captured.clone();
        let on_event: Arc<dyn Fn(CourseEvent) + Send + Sync> = Arc::new(move |e| {
            captured2.lock().unwrap().push(e);
        });
        let d = CourseraDownloader {
            client: &client,
            options: &opts,
            output_root: &out,
            downloader,
            cancellation: cancel,
            slug: "ml-005",
            on_event: Some(on_event),
        };
        let html = crate::coursera::extractors::HtmlArtifact {
            filename: "../../pwned.html".into(),
            html: "<html>pwn</html>".into(),
        };
        let mut summary = CourseSummary::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(d.process_dispatch(
            &dummy_item(),
            &crate::coursera::extractors::DispatchResult::QuizHtml(html),
            "01_Module/01_Welcome",
            &mut summary,
        ));
        let escaped = tmp.path().parent().unwrap().join("pwned.html");
        assert!(
            !escaped.exists(),
            "traversal filename must not write outside output_root"
        );
        assert!(
            !out.join("ml-005").join("pwned.html").exists(),
            "dot-dot segments must not land a file in a sibling directory"
        );
        assert!(
            !summary.failed.is_empty(),
            "unsafe destinations must be recorded as failed"
        );
        assert!(
            captured.lock().unwrap().iter().any(|event| matches!(
                event,
                CourseEvent::FileFailed { error, .. }
                    if error.contains("refusing to write outside the Coursera output root")
            )),
            "must emit FileFailed with the refusal message"
        );
    }

    #[test]
    fn quiz_html_writes_under_output_root() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().to_path_buf();
        let client = crate::coursera::client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let downloader: Arc<dyn Downloader> = Arc::new(NativeDownloader::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let d = CourseraDownloader {
            client: &client,
            options: &opts,
            output_root: &out,
            downloader,
            cancellation: cancel,
            slug: "ml-005",
            on_event: None,
        };
        let html = crate::coursera::extractors::HtmlArtifact {
            filename: "quiz.html".into(),
            html: "<html>ok</html>".into(),
        };
        let mut summary = CourseSummary::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(d.process_dispatch(
            &dummy_item(),
            &crate::coursera::extractors::DispatchResult::QuizHtml(html),
            "01_Module/01_Welcome",
            &mut summary,
        ));
        let expected = out
            .join("ml-005")
            .join("01_Module")
            .join("01_Welcome")
            .join("quiz.html");
        assert_eq!(
            std::fs::read_to_string(&expected).unwrap(),
            "<html>ok</html>"
        );
        assert!(summary.failed.is_empty());
    }

    #[test]
    fn resource_link_rejects_path_traversal_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().to_path_buf();
        let client = crate::coursera::client::build_client().unwrap();
        let opts = CourseraOptions::default();
        let recorder = Arc::new(RecordingDownloader {
            dests: std::sync::Mutex::new(Vec::new()),
        });
        let downloader: Arc<dyn Downloader> = recorder.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let d = CourseraDownloader {
            client: &client,
            options: &opts,
            output_root: &out,
            downloader,
            cancellation: cancel,
            slug: "ml-005",
            on_event: None,
        };
        let links = vec![crate::coursera::extractors::ResourceLink {
            url: "https://example.com/video.mp4".into(),
            filename: r"..\..\pwned.mp4".into(),
            kind: "video".into(),
        }];
        let mut summary = CourseSummary::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(d.process_dispatch(
            &dummy_item(),
            &crate::coursera::extractors::DispatchResult::Links(links),
            "01_Module/01_Welcome",
            &mut summary,
        ));
        assert!(
            recorder.dests.lock().unwrap().is_empty(),
            "downloader must not be invoked for an escaping destination"
        );
        assert!(!summary.failed.is_empty());
    }
}
