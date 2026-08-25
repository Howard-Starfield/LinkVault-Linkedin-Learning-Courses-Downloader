use crate::app::database::open_runtime;
use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseErrorClass, DatabaseProvider,
};
use rusqlite::Connection;
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use thiserror::Error;

const WRITER_RUNNING: u8 = 0;
const WRITER_CLOSING: u8 = 1;
const WRITER_CLOSED: u8 = 2;

type ErasedWriteResult = Result<Box<dyn Any + Send>, DatabaseWriteError>;
type WriteTask = Box<dyn FnOnce(&mut Connection) -> ErasedWriteResult + Send>;

enum WriterMessage {
    Execute {
        context: DatabaseWriteContext,
        task: WriteTask,
        response: mpsc::SyncSender<ErasedWriteResult>,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct DatabaseWriteContext {
    pub operation: &'static str,
    pub provider: DatabaseProvider,
    pub workflow_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum DatabaseWriteError {
    #[error("WRITER_CLOSED")]
    Closed,
    #[error("WRITER_TASK_PANICKED")]
    TaskPanicked,
    #[error("WRITER_UNAVAILABLE")]
    WriterUnavailable,
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

impl DatabaseWriteError {
    fn diagnostic_class(&self) -> DatabaseErrorClass {
        match self {
            Self::Closed => DatabaseErrorClass::Closed,
            Self::TaskPanicked => DatabaseErrorClass::TaskPanicked,
            Self::WriterUnavailable => DatabaseErrorClass::WriterUnavailable,
            Self::Sqlite(error) if is_busy_error(error) => DatabaseErrorClass::Busy,
            Self::Sqlite(_) => DatabaseErrorClass::Sqlite,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)] // Captured by the Phase 1 baseline harness before UI exposure.
pub struct DatabaseWriterStats {
    pub accepted: usize,
    pub completed: usize,
    pub failed: usize,
    pub max_queue_depth: usize,
}

#[derive(Default)]
struct WriterCounters {
    accepted: AtomicUsize,
    completed: AtomicUsize,
    failed: AtomicUsize,
    queued: AtomicUsize,
    max_queue_depth: AtomicUsize,
}

struct DatabaseWriterInner {
    state: AtomicU8,
    sender: Mutex<Option<mpsc::Sender<WriterMessage>>>,
    join: Mutex<Option<JoinHandle<()>>>,
    counters: WriterCounters,
}

#[derive(Clone)]
pub struct DatabaseWriter {
    inner: Arc<DatabaseWriterInner>,
}

impl DatabaseWriter {
    pub fn start(
        path: PathBuf,
        diagnostics: DatabaseDiagnostics,
    ) -> Result<Self, DatabaseWriteError> {
        let connection = open_runtime(&path)?;
        let (sender, receiver) = mpsc::channel();
        let inner = Arc::new(DatabaseWriterInner {
            state: AtomicU8::new(WRITER_RUNNING),
            sender: Mutex::new(Some(sender)),
            join: Mutex::new(None),
            counters: WriterCounters::default(),
        });
        let worker_inner = Arc::clone(&inner);
        let join = thread::Builder::new()
            .name("linkvault-database-writer".to_string())
            .spawn(move || run_writer(connection, receiver, diagnostics, &worker_inner))
            .map_err(|_| DatabaseWriteError::WriterUnavailable)?;
        *inner
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(join);
        Ok(Self { inner })
    }

    pub fn execute<T, F>(
        &self,
        context: DatabaseWriteContext,
        task: F,
    ) -> Result<T, DatabaseWriteError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseWriteError> + Send + 'static,
    {
        if self.inner.state.load(Ordering::SeqCst) != WRITER_RUNNING {
            return Err(DatabaseWriteError::Closed);
        }

        let (response, result) = mpsc::sync_channel(1);
        let erased_task: WriteTask = Box::new(move |connection| {
            task(connection).map(|value| Box::new(value) as Box<dyn Any + Send>)
        });

        {
            let sender = self
                .inner
                .sender
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.inner.state.load(Ordering::SeqCst) != WRITER_RUNNING {
                return Err(DatabaseWriteError::Closed);
            }
            let queued = self.inner.counters.queued.fetch_add(1, Ordering::SeqCst) + 1;
            update_max(&self.inner.counters.max_queue_depth, queued);
            self.inner.counters.accepted.fetch_add(1, Ordering::SeqCst);
            if sender
                .as_ref()
                .ok_or(DatabaseWriteError::Closed)?
                .send(WriterMessage::Execute {
                    context,
                    task: erased_task,
                    response,
                })
                .is_err()
            {
                self.inner.counters.queued.fetch_sub(1, Ordering::SeqCst);
                self.inner.counters.failed.fetch_add(1, Ordering::SeqCst);
                return Err(DatabaseWriteError::WriterUnavailable);
            }
        }

        let erased = result
            .recv()
            .map_err(|_| DatabaseWriteError::WriterUnavailable)??;
        erased
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| DatabaseWriteError::WriterUnavailable)
    }

    #[allow(dead_code)] // Captured by the Phase 1 baseline harness before UI exposure.
    pub fn stats(&self) -> DatabaseWriterStats {
        DatabaseWriterStats {
            accepted: self.inner.counters.accepted.load(Ordering::SeqCst),
            completed: self.inner.counters.completed.load(Ordering::SeqCst),
            failed: self.inner.counters.failed.load(Ordering::SeqCst),
            max_queue_depth: self.inner.counters.max_queue_depth.load(Ordering::SeqCst),
        }
    }

    pub fn shutdown(&self) -> Result<(), DatabaseWriteError> {
        let previous = self.inner.state.compare_exchange(
            WRITER_RUNNING,
            WRITER_CLOSING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        if previous.is_ok() {
            let sender = self
                .inner
                .sender
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(sender) = sender {
                sender
                    .send(WriterMessage::Shutdown)
                    .map_err(|_| DatabaseWriteError::WriterUnavailable)?;
            }
        }

        if let Some(join) = self
            .inner
            .join
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            join.join()
                .map_err(|_| DatabaseWriteError::WriterUnavailable)?;
        }
        self.inner.state.store(WRITER_CLOSED, Ordering::SeqCst);
        Ok(())
    }
}

fn run_writer(
    mut connection: Connection,
    receiver: mpsc::Receiver<WriterMessage>,
    diagnostics: DatabaseDiagnostics,
    inner: &DatabaseWriterInner,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Execute {
                context,
                task,
                response,
            } => {
                let queue_depth = inner
                    .counters
                    .queued
                    .fetch_sub(1, Ordering::SeqCst)
                    .saturating_sub(1);
                let started = Instant::now();
                let outcome = catch_unwind(AssertUnwindSafe(|| task(&mut connection)))
                    .unwrap_or(Err(DatabaseWriteError::TaskPanicked));
                let (diagnostic_outcome, error_class) = match &outcome {
                    Ok(_) => {
                        inner.counters.completed.fetch_add(1, Ordering::SeqCst);
                        (DatabaseDiagnosticOutcome::Ok, None)
                    }
                    Err(error) => {
                        inner.counters.failed.fetch_add(1, Ordering::SeqCst);
                        (
                            DatabaseDiagnosticOutcome::Error,
                            Some(error.diagnostic_class()),
                        )
                    }
                };
                diagnostics.record(DatabaseDiagnosticInput {
                    kind: if error_class == Some(DatabaseErrorClass::Busy) {
                        DatabaseDiagnosticKind::Contention
                    } else {
                        DatabaseDiagnosticKind::WriterRequest
                    },
                    operation: context.operation,
                    provider: context.provider,
                    workflow_id: context.workflow_id,
                    elapsed: started.elapsed(),
                    queue_depth,
                    outcome: diagnostic_outcome,
                    error_class,
                });
                let _ = response.send(outcome);
            }
            WriterMessage::Shutdown => {
                diagnostics.record(DatabaseDiagnosticInput {
                    kind: DatabaseDiagnosticKind::Shutdown,
                    operation: "database_writer_shutdown",
                    provider: DatabaseProvider::App,
                    workflow_id: None,
                    elapsed: std::time::Duration::ZERO,
                    queue_depth: inner.counters.queued.load(Ordering::SeqCst),
                    outcome: DatabaseDiagnosticOutcome::Ok,
                    error_class: None,
                });
                break;
            }
        }
    }
    inner.state.store(WRITER_CLOSED, Ordering::SeqCst);
}

fn update_max(maximum: &AtomicUsize, candidate: usize) {
    let mut observed = maximum.load(Ordering::Relaxed);
    while candidate > observed {
        match maximum.compare_exchange_weak(
            observed,
            candidate,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => observed = actual,
        }
    }
}

fn is_busy_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::{initialize_database, open_runtime};
    use crate::app::database_diagnostics::{DatabaseDiagnostics, DatabaseProvider};
    use rusqlite::params;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn writer_context(operation: &'static str) -> DatabaseWriteContext {
        DatabaseWriteContext {
            operation,
            provider: DatabaseProvider::Workflow,
            workflow_id: None,
        }
    }

    fn initialized_writer() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        DatabaseDiagnostics,
        DatabaseWriter,
    ) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE writer_probe (
                    producer INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    PRIMARY KEY (producer, sequence)
                );",
            )
            .unwrap();
        drop(connection);
        let diagnostics = DatabaseDiagnostics::default();
        let writer = DatabaseWriter::start(db_path.clone(), diagnostics.clone()).unwrap();
        (directory, db_path, diagnostics, writer)
    }

    #[test]
    fn persistence_gate_writer_serializes_eight_hundred_concurrent_writes() {
        let (_directory, db_path, diagnostics, writer) = initialized_writer();
        let mut producers = Vec::new();

        for producer in 0..8 {
            let producer_writer = writer.clone();
            producers.push(thread::spawn(move || {
                for sequence in 0..100 {
                    producer_writer
                        .execute(writer_context("contention_probe"), move |connection| {
                            connection.execute(
                                "INSERT INTO writer_probe (producer, sequence) VALUES (?1, ?2)",
                                params![producer, sequence],
                            )?;
                            Ok(())
                        })
                        .unwrap();
                }
            }));
        }

        for producer in producers {
            producer.join().unwrap();
        }
        let stats = writer.stats();
        writer.shutdown().unwrap();

        let connection = open_runtime(&db_path).unwrap();
        let (rows, distinct_rows): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT producer || ':' || sequence)
                 FROM writer_probe",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 800);
        assert_eq!(distinct_rows, 800);
        assert_eq!(stats.completed, 800);
        assert_eq!(stats.failed, 0);
        assert!(diagnostics
            .snapshot()
            .iter()
            .all(|event| event.error_class.is_none()));
    }

    #[test]
    fn persistence_gate_reader_keeps_previous_snapshot_during_uncommitted_write() {
        let (_directory, db_path, _diagnostics, writer) = initialized_writer();
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let request_writer = writer.clone();
        let request = thread::spawn(move || {
            request_writer.execute(writer_context("snapshot_probe"), move |connection| {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "INSERT INTO writer_probe (producer, sequence) VALUES (99, 1)",
                    [],
                )?;
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                transaction.commit()?;
                Ok(())
            })
        });
        entered_rx.recv().unwrap();

        let started = Instant::now();
        let reader = open_runtime(&db_path).unwrap();
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM writer_probe", [], |row| row.get(0))
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(count, 0);
        assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");

        release_tx.send(()).unwrap();
        request.join().unwrap().unwrap();
        writer.shutdown().unwrap();
    }

    #[test]
    fn persistence_gate_shutdown_drains_accepted_work_and_rejects_late_work() {
        let (_directory, db_path, _diagnostics, writer) = initialized_writer();
        let accepted_writer = writer.clone();
        let accepted = thread::spawn(move || {
            accepted_writer.execute(writer_context("accepted_before_shutdown"), |connection| {
                thread::sleep(Duration::from_millis(25));
                connection.execute(
                    "INSERT INTO writer_probe (producer, sequence) VALUES (1, 1)",
                    [],
                )?;
                Ok(())
            })
        });
        let acceptance_deadline = Instant::now() + Duration::from_secs(1);
        while writer.stats().accepted == 0 && Instant::now() < acceptance_deadline {
            thread::yield_now();
        }
        assert_eq!(writer.stats().accepted, 1);

        writer.shutdown().unwrap();
        accepted.join().unwrap().unwrap();
        let late = writer.execute(writer_context("late_after_shutdown"), |_connection| Ok(()));
        assert!(matches!(late, Err(DatabaseWriteError::Closed)));

        let connection = open_runtime(&db_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM writer_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn persistence_gate_panicked_request_does_not_kill_writer() {
        let (_directory, db_path, _diagnostics, writer) = initialized_writer();
        let panicked = writer.execute::<(), _>(writer_context("panic_probe"), |_connection| {
            panic!("synthetic writer panic")
        });
        assert!(matches!(panicked, Err(DatabaseWriteError::TaskPanicked)));

        writer
            .execute(writer_context("after_panic"), |connection| {
                connection.execute(
                    "INSERT INTO writer_probe (producer, sequence) VALUES (2, 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        writer.shutdown().unwrap();

        let connection = open_runtime(&db_path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM writer_probe", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    #[ignore = "run through npm.cmd run verify:persistence-baseline"]
    fn persistence_release_baseline() {
        let (_directory, db_path, diagnostics, writer) = initialized_writer();
        let started = Instant::now();
        let mut producers = Vec::new();
        for producer in 0..8 {
            let producer_writer = writer.clone();
            producers.push(thread::spawn(move || {
                for sequence in 0..100 {
                    producer_writer
                        .execute(
                            writer_context("release_contention_probe"),
                            move |connection| {
                                connection.execute(
                                    "INSERT INTO writer_probe (producer, sequence) VALUES (?1, ?2)",
                                    params![producer, sequence],
                                )?;
                                Ok(())
                            },
                        )
                        .unwrap();
                }
            }));
        }
        for producer in producers {
            producer.join().unwrap();
        }
        let contention_elapsed = started.elapsed();
        let contention_stats = writer.stats();

        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let request_writer = writer.clone();
        let request = thread::spawn(move || {
            request_writer.execute(
                writer_context("release_snapshot_probe"),
                move |connection| {
                    let transaction = connection.transaction()?;
                    transaction.execute(
                        "INSERT INTO writer_probe (producer, sequence) VALUES (99, 1)",
                        [],
                    )?;
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    transaction.commit()?;
                    Ok(())
                },
            )
        });
        entered_rx.recv().unwrap();
        let snapshot_started = Instant::now();
        let reader = open_runtime(&db_path).unwrap();
        let snapshot_rows: i64 = reader
            .query_row("SELECT COUNT(*) FROM writer_probe", [], |row| row.get(0))
            .unwrap();
        let snapshot_elapsed = snapshot_started.elapsed();
        drop(reader);
        release_tx.send(()).unwrap();
        request.join().unwrap().unwrap();
        writer.shutdown().unwrap();

        assert_eq!(snapshot_rows, 800);
        assert_eq!(contention_stats.completed, 800);
        assert_eq!(contention_stats.failed, 0);
        assert!(contention_elapsed < Duration::from_secs(10));
        assert!(snapshot_elapsed < Duration::from_secs(10));

        let report = serde_json::json!({
            "schemaVersion": 1,
            "profile": "release",
            "producerCount": 8,
            "writesPerProducer": 100,
            "acceptedWrites": contention_stats.accepted,
            "completedWrites": contention_stats.completed,
            "failedWrites": contention_stats.failed,
            "contentionElapsedMs": contention_elapsed.as_millis(),
            "writesPerSecond": 800_000_u128 / contention_elapsed.as_millis().max(1),
            "maxQueueDepth": contention_stats.max_queue_depth,
            "snapshotRows": snapshot_rows,
            "snapshotReadElapsedMs": snapshot_elapsed.as_millis(),
            "retainedDiagnosticEvents": diagnostics.snapshot().len(),
        });
        println!("LINKVAULT_PERSISTENCE_BASELINE={report}");
    }

    #[test]
    #[ignore = "run through npm.cmd run verify:persistence-baseline"]
    fn persistence_release_diagnostic_sample() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let diagnostics = DatabaseDiagnostics::default();
        let (connection, _) =
            crate::app::database::initialize_database_with_diagnostics(&db_path, &diagnostics)
                .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE writer_probe (
                    producer INTEGER NOT NULL,
                    sequence INTEGER NOT NULL,
                    PRIMARY KEY (producer, sequence)
                );",
            )
            .unwrap();
        drop(connection);
        let writer = DatabaseWriter::start(db_path, diagnostics.clone()).unwrap();
        writer
            .execute(writer_context("diagnostic_success_probe"), |connection| {
                connection.execute(
                    "INSERT INTO writer_probe (producer, sequence) VALUES (1, 1)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(writer
            .execute(writer_context("diagnostic_error_probe"), |connection| {
                connection.execute(
                    "INSERT INTO writer_probe (producer, sequence) VALUES (1, 1)",
                    [],
                )?;
                Ok(())
            })
            .is_err());
        writer.shutdown().unwrap();

        let sample = serde_json::to_string(&diagnostics.snapshot()).unwrap();
        for forbidden in [
            "message",
            "payload",
            "cookie",
            "authorization",
            "token",
            "synthetic-secret",
        ] {
            assert!(!sample.to_ascii_lowercase().contains(forbidden));
        }
        println!("LINKVAULT_PERSISTENCE_DIAGNOSTICS={sample}");
    }
}
