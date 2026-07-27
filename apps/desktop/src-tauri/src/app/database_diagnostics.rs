use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_DATABASE_DIAGNOSTICS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseDiagnosticKind {
    Initialization,
    Migration,
    Backup,
    WriterRequest,
    Contention,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Provider variants are activated incrementally by planned cutovers.
pub enum DatabaseProvider {
    App,
    Linkedin,
    Coursera,
    Newspaper,
    Workflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseDiagnosticOutcome {
    Ok,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseErrorClass {
    BackupIntegrity,
    Busy,
    Closed,
    DatabaseIntegrity,
    Io,
    Migration,
    Sqlite,
    TaskPanicked,
    UnsupportedSchema,
    WriterUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnosticEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub kind: DatabaseDiagnosticKind,
    pub operation: &'static str,
    pub provider: DatabaseProvider,
    pub workflow_id: Option<String>,
    pub elapsed_ms: u64,
    pub queue_depth: usize,
    pub outcome: DatabaseDiagnosticOutcome,
    pub error_class: Option<DatabaseErrorClass>,
}

pub struct DatabaseDiagnosticInput {
    pub kind: DatabaseDiagnosticKind,
    pub operation: &'static str,
    pub provider: DatabaseProvider,
    pub workflow_id: Option<String>,
    pub elapsed: Duration,
    pub queue_depth: usize,
    pub outcome: DatabaseDiagnosticOutcome,
    pub error_class: Option<DatabaseErrorClass>,
}

#[derive(Default)]
struct DatabaseDiagnosticsInner {
    next_sequence: AtomicU64,
    events: Mutex<VecDeque<DatabaseDiagnosticEvent>>,
}

#[derive(Clone, Default)]
pub struct DatabaseDiagnostics {
    inner: Arc<DatabaseDiagnosticsInner>,
}

impl DatabaseDiagnostics {
    pub fn record(&self, input: DatabaseDiagnosticInput) -> DatabaseDiagnosticEvent {
        let event = DatabaseDiagnosticEvent {
            sequence: self.inner.next_sequence.fetch_add(1, Ordering::SeqCst) + 1,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            kind: input.kind,
            operation: input.operation,
            provider: input.provider,
            workflow_id: input.workflow_id,
            elapsed_ms: input.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            queue_depth: input.queue_depth,
            outcome: input.outcome,
            error_class: input.error_class,
        };
        let mut events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if events.len() == MAX_DATABASE_DIAGNOSTICS {
            events.pop_front();
        }
        events.push_back(event.clone());
        event
    }

    #[allow(dead_code)] // Exposed to Phase 1 verification and the future diagnostics UI.
    pub fn snapshot(&self) -> Vec<DatabaseDiagnosticEvent> {
        self.inner
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn persistence_gate_diagnostics_are_bounded_and_structurally_redacted() {
        let diagnostics = DatabaseDiagnostics::default();
        for index in 0..600 {
            diagnostics.record(DatabaseDiagnosticInput {
                kind: DatabaseDiagnosticKind::WriterRequest,
                operation: "contention_probe",
                provider: DatabaseProvider::Workflow,
                workflow_id: Some(format!("probe-{index}")),
                elapsed: Duration::from_millis(2),
                queue_depth: index,
                outcome: DatabaseDiagnosticOutcome::Ok,
                error_class: None,
            });
        }

        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.len(), 512);
        assert_eq!(snapshot.first().unwrap().sequence, 89);
        assert_eq!(snapshot.last().unwrap().sequence, 600);

        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("message"));
        assert!(!serialized.contains("payload"));
        assert!(!serialized.contains("cookie"));
        assert!(!serialized.contains("authorization"));
        assert!(!serialized.contains("token"));
    }
}
