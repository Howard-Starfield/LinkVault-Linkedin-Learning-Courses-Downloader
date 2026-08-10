#![cfg(feature = "durability-baseline")]

//! Opt-in release collector for clipping-note recovery write latency and WAL growth.
//!
//! The ten-minute workload is clock-compressed to the approved maximum of 300
//! recovery submissions. It uses the real schema-v6 repository and the single
//! application `DatabaseWriter`; it does not sleep or force per-edit WAL checkpoints.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::app::{
    database::{initialize_database, open_runtime},
    database_diagnostics::DatabaseDiagnostics,
    database_writer::DatabaseWriter,
};
use crate::providers::newspaper::{
    clipping_draft_models::CheckpointClippingNoteRequest,
    clipping_draft_service::ClippingDraftService, clipping_models::ClippingError,
};

const CLIPPING_ID: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const WRITER_SESSION_ID: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const LATENCY_SAMPLES: usize = 25;
const TEN_MINUTE_MAX_WAIT_WRITES: usize = 300;
const NEAR_CANONICAL_MARKDOWN_BYTES: usize = 2 * 1024 * 1024 - 128;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LatencyReport {
    payload_bytes: usize,
    samples: usize,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageSizes {
    database_bytes: u64,
    wal_bytes: u64,
    shm_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkloadReport {
    equivalent_typing_minutes: usize,
    max_wait_ms: usize,
    checkpoint_writes: usize,
    payload_bytes: usize,
    elapsed_ms: f64,
    initial_storage: StorageSizes,
    peak_storage: StorageSizes,
    post_idle_storage: StorageSizes,
    working_set_before_bytes: i64,
    peak_working_set_bytes: i64,
    post_idle_working_set_bytes: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DurabilityBaselineReport {
    profile: &'static str,
    schema_version: i32,
    writer: &'static str,
    latency: Vec<LatencyReport>,
    ten_minute_workload: WorkloadReport,
    accepted_writes: usize,
    completed_writes: usize,
    failed_writes: usize,
    max_queue_depth: usize,
    final_draft_rows: usize,
    final_writer_sequence: u64,
}

/// Measures the real serialized checkpoint path in a disposable schema-v6 database.
pub fn run() -> Result<String, String> {
    let temp = tempfile::tempdir().map_err(string_error)?;
    let db_path = temp.path().join("linkvault.sqlite3");
    let (connection, initialization) = initialize_database(&db_path).map_err(string_error)?;
    seed_ready_clipping(&connection)?;
    drop(connection);

    let writer = DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default())
        .map_err(string_error)?;
    let service = ClippingDraftService::new(db_path.clone(), writer.clone());
    let mut sequence = 0_u64;
    let mut now = 1_000_i64;
    let mut latency = Vec::new();
    for payload_bytes in [1024, 100 * 1024, 2 * 1024 * 1024] {
        let markdown = "x".repeat(payload_bytes);
        let mut samples = Vec::with_capacity(LATENCY_SAMPLES);
        for _ in 0..LATENCY_SAMPLES {
            sequence += 1;
            now += 1;
            let started = Instant::now();
            checkpoint(&service, sequence, now, markdown.clone())?;
            samples.push(started.elapsed());
        }
        latency.push(summarize_latency(payload_bytes, samples));
    }

    drop(service);
    writer.shutdown().map_err(string_error)?;
    truncate_wal(&db_path)?;
    let writer = DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default())
        .map_err(string_error)?;
    let service = ClippingDraftService::new(db_path.clone(), writer.clone());

    let initial_storage = storage_sizes(&db_path);
    let working_set_before = current_working_set_bytes();
    let mut peak_working_set = working_set_before;
    let mut peak_storage = initial_storage;
    let workload_markdown = "w".repeat(NEAR_CANONICAL_MARKDOWN_BYTES);
    let workload_started = Instant::now();
    for write_index in 0..TEN_MINUTE_MAX_WAIT_WRITES {
        sequence += 1;
        now += 2;
        checkpoint(&service, sequence, now, workload_markdown.clone())?;
        if write_index % 10 == 0 || write_index + 1 == TEN_MINUTE_MAX_WAIT_WRITES {
            peak_storage = storage_max(peak_storage, storage_sizes(&db_path));
            peak_working_set = peak_working_set.max(current_working_set_bytes());
        }
    }
    let workload_elapsed = workload_started.elapsed();
    drop(service);
    let writer_stats = writer.stats();
    writer.shutdown().map_err(string_error)?;

    let checkpoint_connection = truncate_wal(&db_path)?;
    let (final_draft_rows, final_writer_sequence) = final_draft_state(&checkpoint_connection)?;
    drop(checkpoint_connection);
    let post_idle_storage = storage_sizes(&db_path);
    let post_idle_working_set = current_working_set_bytes();

    let report = DurabilityBaselineReport {
        profile: "release",
        schema_version: initialization.to_version,
        writer: "DatabaseWriter",
        latency,
        ten_minute_workload: WorkloadReport {
            equivalent_typing_minutes: 10,
            max_wait_ms: 2_000,
            checkpoint_writes: TEN_MINUTE_MAX_WAIT_WRITES,
            payload_bytes: workload_markdown.len(),
            elapsed_ms: milliseconds(workload_elapsed),
            initial_storage,
            peak_storage,
            post_idle_storage,
            working_set_before_bytes: working_set_before,
            peak_working_set_bytes: peak_working_set,
            post_idle_working_set_bytes: post_idle_working_set,
        },
        accepted_writes: writer_stats.accepted,
        completed_writes: writer_stats.completed,
        failed_writes: writer_stats.failed,
        max_queue_depth: writer_stats.max_queue_depth,
        final_draft_rows,
        final_writer_sequence,
    };
    serde_json::to_string_pretty(&report).map_err(string_error)
}

fn checkpoint(
    service: &ClippingDraftService,
    writer_sequence: u64,
    now: i64,
    markdown: String,
) -> Result<(), String> {
    let ack = service
        .checkpoint(
            CheckpointClippingNoteRequest {
                clipping_id: CLIPPING_ID.to_string(),
                base_revision: 1,
                writer_session_id: WRITER_SESSION_ID.to_string(),
                writer_sequence,
                title: "Durability baseline".to_string(),
                markdown,
            },
            now,
        )
        .map_err(safe_clipping_error)?;
    if ack.writer_sequence != writer_sequence {
        return Err("checkpoint returned a mismatched writer sequence".to_string());
    }
    Ok(())
}

fn seed_ready_clipping(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO newspaper_clippings (
                id, source_job_id, source_page_id, source_media_version_snapshot,
                source_kind_snapshot, source_mime_type_snapshot, source_checksum_snapshot,
                edition_code_snapshot, edition_name_snapshot, publication_date_snapshot,
                page_number_snapshot, source_pixel_width, source_pixel_height,
                crop_x, crop_y, crop_width, crop_height, asset_root_id,
                asset_relative_path, asset_mime_type, asset_pixel_width,
                asset_pixel_height, asset_byte_count, asset_checksum_sha256,
                asset_version, asset_state, asset_error_code, title, note_markdown,
                revision, created_at, updated_at
             ) VALUES (
                ?1, NULL, NULL, 1, 'original', 'image/webp', NULL,
                'BASELINE', 'Durability baseline', '2026-08-10', 'A01', 100, 100,
                0, 0, 100, 100, 'legacy-managed-v1', ?2, 'image/webp', 100, 100,
                1, ?3, 1, 'ready', NULL, 'Durability baseline', '', 1, 1, 1
             )",
            params![
                CLIPPING_ID,
                format!("assets/{CLIPPING_ID}/clipping-v1.webp"),
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ],
        )
        .map_err(string_error)?;
    Ok(())
}

fn summarize_latency(payload_bytes: usize, mut samples: Vec<Duration>) -> LatencyReport {
    samples.sort_unstable();
    LatencyReport {
        payload_bytes,
        samples: samples.len(),
        p50_ms: milliseconds(samples[nearest_rank(samples.len(), 50)]),
        p95_ms: milliseconds(samples[nearest_rank(samples.len(), 95)]),
        max_ms: milliseconds(*samples.last().expect("latency samples are non-empty")),
    }
}

fn nearest_rank(sample_count: usize, percentile: usize) -> usize {
    (sample_count * percentile).div_ceil(100).saturating_sub(1)
}

fn final_draft_state(connection: &Connection) -> Result<(usize, u64), String> {
    connection
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(writer_sequence), 0)
             FROM newspaper_clipping_note_drafts WHERE clipping_id = ?1",
            [CLIPPING_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(string_error)
}

fn truncate_wal(db_path: &Path) -> Result<Connection, String> {
    let connection = open_runtime(db_path).map_err(string_error)?;
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(string_error)?;
    Ok(connection)
}

fn storage_sizes(db_path: &Path) -> StorageSizes {
    StorageSizes {
        database_bytes: file_size(db_path),
        wal_bytes: file_size(&sidecar_path(db_path, "-wal")),
        shm_bytes: file_size(&sidecar_path(db_path, "-shm")),
    }
}

fn storage_max(left: StorageSizes, right: StorageSizes) -> StorageSizes {
    StorageSizes {
        database_bytes: left.database_bytes.max(right.database_bytes),
        wal_bytes: left.wal_bytes.max(right.wal_bytes),
        shm_bytes: left.shm_bytes.max(right.shm_bytes),
    }
}

fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", db_path.display()))
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn safe_clipping_error(error: ClippingError) -> String {
    error.code.as_str().to_string()
}

fn string_error(error: impl ToString) -> String {
    error.to_string()
}

fn milliseconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

#[cfg(windows)]
fn current_working_set_bytes() -> i64 {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = zeroed();
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ) != 0
        {
            counters.WorkingSetSize as i64
        } else {
            0
        }
    }
}

#[cfg(not(windows))]
fn current_working_set_bytes() -> i64 {
    0
}
