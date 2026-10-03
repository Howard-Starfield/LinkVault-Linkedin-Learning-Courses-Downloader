use super::*;
use crate::app::database_diagnostics::{DatabaseDiagnosticOutcome, DatabaseDiagnostics};
use crate::newspaper::{batch_service, models::CreateNewspaperBatchRequest};

struct Fixture {
    _directory: tempfile::TempDir,
    db_path: PathBuf,
    writer: DatabaseWriter,
    diagnostics: DatabaseDiagnostics,
    schedule: NewspaperSchedule,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("test.db");
        let (connection, _) = crate::cache::initialize_database(&db_path).unwrap();
        drop(connection);
        let diagnostics = DatabaseDiagnostics::default();
        let writer = DatabaseWriter::start(db_path.clone(), diagnostics.clone()).unwrap();
        let schedule = create(
            &writer,
            CreateNewspaperScheduleRequest {
                cron_time: "12:00".to_string(),
                destination: directory
                    .path()
                    .join("papers")
                    .to_string_lossy()
                    .into_owned(),
                edition_codes: vec!["NY".to_string()],
                date_mode: DateMode::Single,
                delay_seconds: 15,
                optimize_images: true,
                optimization_profile: "webp_balanced".to_string(),
                optimization_quality: 70,
                keep_original_jpg: false,
            },
        )
        .unwrap();
        Self {
            _directory: directory,
            db_path,
            writer,
            diagnostics,
            schedule,
        }
    }
    fn connection(&self) -> Connection {
        crate::cache::open_runtime(&self.db_path).unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    fn seed_run(&self, id: &str, date: &str, state: &str, position: i64) {
        let request = NewspaperWorkflowRequest {
            schema_version: 1,
            batch_id: "prior-batch".to_string(),
            edition_code: "NY".to_string(),
            edition_name: "New York".to_string(),
            edition_publication_date: String::new(),
            publication_date: date.to_string(),
            queue_position: position,
            delay_seconds: 0,
            scheduled_at: None,
            optimize_images: true,
        };
        let json = serde_json::to_string(&request).unwrap();
        self.connection().execute("INSERT INTO workflow_runs (id, workflow_type, provider, state, request_json, output_root, created_at, updated_at) VALUES (?1, 'newspaper_download', 'newspaper', ?2, ?3, 'other-destination', ?4, ?4)", params![id, state, json, position]).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.writer.shutdown();
    }
}

fn at(hour: u32) -> i64 {
    Local
        .with_ymd_and_hms(2026, 10, 3, hour, 0, 0)
        .single()
        .unwrap()
        .timestamp()
}
fn dates() -> Vec<String> {
    (0..=6)
        .map(|offset| {
            (Local
                .timestamp_opt(at(13), 0)
                .single()
                .unwrap()
                .date_naive()
                - chrono::Duration::days(6 - offset))
            .to_string()
        })
        .collect()
}

#[test]
fn seven_day_reconciliation_respects_today_cron_and_preserves_atomic_payloads() {
    let fixture = Fixture::new();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(9)).unwrap(),
        6
    );
    assert_eq!(fixture.count("workflow_runs"), 6);
    assert_eq!(fixture.count("workflow_steps"), 6);
    assert_eq!(fixture.count("workflow_events"), 6);
    assert_eq!(next_due_at(&fixture.db_path, at(9)).unwrap(), Some(at(12)));
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        1
    );
    assert_eq!(fixture.count("workflow_runs"), 7);
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    assert_eq!(fixture.count("newspaper_batches"), 2);
    let window: (String, String) = fixture.connection().query_row("SELECT MIN(json_extract(request_json, '$.publicationDate')), MAX(json_extract(request_json, '$.publicationDate')) FROM workflow_runs", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(window, (dates()[0].clone(), dates()[6].clone()));
}

#[test]
fn same_day_watermark_does_not_hide_a_middle_gap_and_checkpoint_preserves_removal() {
    let fixture = Fixture::new();
    for (index, date) in dates().iter().enumerate().filter(|(index, _)| *index != 3) {
        fixture.seed_run(&format!("prior-{index}"), date, "succeeded", index as i64);
    }
    fixture
        .connection()
        .execute(
            "UPDATE newspaper_schedules SET last_run_date = '2026-10-03'",
            [],
        )
        .unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        1
    );
    fixture
        .connection()
        .execute("DELETE FROM workflow_runs", [])
        .unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    assert_eq!(fixture.count("workflow_runs"), 0);
    assert_eq!(fixture.count("newspaper_batches"), 1);
}

#[test]
fn all_existing_intents_are_preserved_without_retry_or_destination_duplication() {
    let fixture = Fixture::new();
    let states = [
        "queued",
        "running",
        "paused",
        "retry_wait",
        "failed",
        "cancelled",
        "succeeded_with_warnings",
    ];
    for (index, (date, state)) in dates().iter().zip(states).enumerate() {
        fixture.seed_run(&format!("intent-{index}"), date, state, index as i64);
    }
    let mut connection = fixture.connection();
    let response = batch_service::create_with_connection(
        &mut connection,
        CreateNewspaperBatchRequest {
            edition_codes: vec!["NY".to_string()],
            date_mode: DateMode::Single,
            start_date: dates()[6].clone(),
            end_date: None,
            destination: fixture.schedule.destination.clone(),
            scheduled_at: None,
            delay_seconds: 0,
            optimize_images: true,
            optimization_profile: "webp_balanced".to_string(),
            optimization_quality: 70,
            keep_original_jpg: false,
        },
    )
    .unwrap();
    connection.execute("UPDATE newspaper_jobs SET status = 'unavailable', dismissed = 1, paused = 1 WHERE id = ?1", params![response.jobs[0].id]).unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    let after: Vec<String> = connection
        .prepare("SELECT state FROM workflow_runs ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(after, states);
    let legacy: (String, bool, bool) = connection
        .query_row(
            "SELECT status, dismissed, paused FROM newspaper_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(legacy, ("unavailable".to_string(), true, true));
}

#[test]
fn history_deduplication_has_no_thousand_run_cap() {
    let fixture = Fixture::new();
    fixture.seed_run("old-covered", &dates()[0], "succeeded", 1);
    for index in 0..1_100 {
        fixture.seed_run(
            &format!("noise-{index}"),
            "2020-01-01",
            "succeeded",
            index + 2,
        );
    }
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        6
    );
    let duplicate: i64 = fixture.connection().query_row("SELECT COUNT(*) FROM workflow_runs WHERE json_extract(request_json, '$.publicationDate') = ?1", [&dates()[0]], |row| row.get(0)).unwrap();
    assert_eq!(duplicate, 1);
}

#[test]
fn concurrent_manual_and_automatic_producers_share_final_writer_deduplication() {
    let fixture = Fixture::new();
    let writer = fixture.writer.clone();
    let db_path = fixture.db_path.clone();
    let automatic =
        std::thread::spawn(move || materialize_due_at(&writer, &db_path, at(13)).unwrap());
    let manual = batch_service::create(
        &fixture.writer,
        CreateNewspaperBatchRequest {
            edition_codes: vec!["NY".to_string()],
            date_mode: DateMode::Single,
            start_date: dates()[6].clone(),
            end_date: None,
            destination: fixture.schedule.destination.clone(),
            scheduled_at: None,
            delay_seconds: 0,
            optimize_images: true,
            optimization_profile: "webp_balanced".to_string(),
            optimization_quality: 70,
            keep_original_jpg: false,
        },
    )
    .unwrap();
    let automatic_created = automatic.join().unwrap();
    assert_eq!(manual.jobs.len() + automatic_created, 7);
    assert_eq!(fixture.count("workflow_runs"), 7);
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
}

#[test]
fn completed_marker_and_publication_cadence_are_respected_without_history() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    let edition = catalog_service::list_with_connection(&connection)
        .unwrap()
        .into_iter()
        .find(|edition| edition.code == "NY")
        .unwrap();
    let output_dir = candidate_output_dir(&fixture.schedule, &edition, &dates()[6]);
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::write(output_dir.join(".complete"), b"").unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        6
    );
    connection.execute("INSERT INTO newspaper_editions (code, publication_date, name_zh, name_en, kind, schedule, source_url, active, discovered, discovered_at, updated_at) VALUES ('SPECIAL', '2026-09-30', 'Special', 'Special', 'special', 'ad_hoc', 'test://special', 1, 1, 1, 1)", []).unwrap();
    let weekly = catalog_service::list_with_connection(&connection)
        .unwrap()
        .into_iter()
        .find(|edition| edition.kind == EditionKind::Weekly)
        .unwrap();
    connection
        .execute(
            "UPDATE newspaper_schedules SET edition_codes_json = ?1",
            [
                serde_json::to_string(&vec![weekly.code, "SPECIAL@2026-09-30".to_string()])
                    .unwrap(),
            ],
        )
        .unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        2
    );
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
}

#[test]
fn writer_sql_failure_rolls_back_every_intent_and_never_marks_success() {
    let fixture = Fixture::new();
    fixture.connection().execute_batch("CREATE TRIGGER reject_schedule_step BEFORE INSERT ON workflow_steps BEGIN SELECT RAISE(ABORT, 'injected schedule failure'); END;").unwrap();
    assert!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13))
            .unwrap_err()
            .contains("injected schedule failure")
    );
    for table in [
        "newspaper_batches",
        "workflow_runs",
        "workflow_steps",
        "workflow_events",
        "newspaper_settings",
    ] {
        assert_eq!(fixture.count(table), 0, "{table}");
    }
    let last_run: Option<String> = fixture
        .connection()
        .query_row("SELECT last_run_date FROM newspaper_schedules", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(last_run, None);
    assert_eq!(fixture.writer.stats().failed, 1);
    assert_eq!(
        fixture.diagnostics.snapshot().last().unwrap().outcome,
        DatabaseDiagnosticOutcome::Error
    );
    fixture
        .connection()
        .execute_batch("DROP TRIGGER reject_schedule_step")
        .unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        7
    );
    fixture.writer.shutdown().unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap_err(),
        "WRITER_CLOSED"
    );
    assert_eq!(fixture.count("workflow_runs"), 7);
}

#[test]
fn disabled_schedule_and_delete_cannot_create_or_leave_runnable_work() {
    let fixture = Fixture::new();
    toggle(&fixture.writer, &fixture.schedule.id, false).unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    assert_eq!(next_due_at(&fixture.db_path, at(9)).unwrap(), None);
    toggle(&fixture.writer, &fixture.schedule.id, true).unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        7
    );
    delete(&fixture.writer, &fixture.schedule.id).unwrap();
    assert_eq!(fixture.count("newspaper_schedules"), 0);
    assert_eq!(fixture.count("newspaper_settings"), 0);
    let runnable: i64 = fixture
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM workflow_runs WHERE state != 'cancelled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runnable, 0);
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
}

#[test]
fn unindexed_completed_markers_block_other_destination_regardless_of_schedule_order() {
    let fixture = Fixture::new();
    let edition = catalog_service::list_with_connection(&fixture.connection())
        .unwrap()
        .into_iter()
        .find(|edition| edition.code == "NY")
        .unwrap();
    for date in dates() {
        let output_dir = candidate_output_dir(&fixture.schedule, &edition, &date);
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::write(output_dir.join(".complete"), b"").unwrap();
    }
    fixture
        .connection()
        .execute(
            "UPDATE newspaper_schedules SET cron_time = '23:00' WHERE id = ?1",
            [&fixture.schedule.id],
        )
        .unwrap();
    let mut other = fixture.schedule.clone();
    other.id = "other".to_string();
    other.destination = fixture
        ._directory
        .path()
        .join("different")
        .to_string_lossy()
        .into_owned();
    fixture.connection().execute("INSERT INTO newspaper_schedules
        (id, enabled, cron_time, destination, edition_codes_json, date_mode, delay_seconds, optimize_images,
         optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
        VALUES (?1, 1, '12:00', ?2, '[\"NY\"]', 'last7_days', 0, 1, 'webp_balanced', 70, 0, ?3, ?3)",
        params![other.id, other.destination, fixture.schedule.created_at + 1]).unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    assert_eq!(fixture.count("workflow_runs"), 0);
    assert_eq!(fixture.count("newspaper_batches"), 0);
}

#[test]
fn deleting_running_work_preserves_cooperative_kernel_cancellation() {
    let fixture = Fixture::new();
    materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap();
    let connection = fixture.connection();
    let run_id: String = connection
        .query_row("SELECT id FROM workflow_runs LIMIT 1", [], |row| row.get(0))
        .unwrap();
    connection
        .execute(
            "UPDATE workflow_runs SET state = 'running' WHERE id = ?1",
            [&run_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE workflow_steps SET state = 'running' WHERE run_id = ?1",
            [&run_id],
        )
        .unwrap();
    assert!(delete(&fixture.writer, &fixture.schedule.id).unwrap());
    let state: String = connection
        .query_row(
            "SELECT state FROM workflow_runs WHERE id = ?1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    let step_state: String = connection
        .query_row(
            "SELECT state FROM workflow_steps WHERE run_id = ?1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "cancelling");
    assert_eq!(step_state, "running");
    assert_eq!(fixture.count("newspaper_schedules"), 0);
    assert_eq!(fixture.count("newspaper_settings"), 0);
}

#[test]
fn disabled_vault_markers_block_enabled_schedule_without_parsing_disabled_cron() {
    let fixture = Fixture::new();
    let edition = catalog_service::list_with_connection(&fixture.connection())
        .unwrap()
        .into_iter()
        .find(|edition| edition.code == "NY")
        .unwrap();
    for date in dates() {
        let output_dir = candidate_output_dir(&fixture.schedule, &edition, &date);
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::write(output_dir.join(".complete"), b"").unwrap();
    }
    fixture
        .connection()
        .execute(
            "UPDATE newspaper_schedules SET enabled = 0, cron_time = 'invalid' WHERE id = ?1",
            [&fixture.schedule.id],
        )
        .unwrap();
    fixture.connection().execute("INSERT INTO newspaper_schedules
        (id, enabled, cron_time, destination, edition_codes_json, date_mode, delay_seconds, optimize_images,
         optimization_profile, optimization_quality, keep_original_jpg, created_at, updated_at)
        VALUES ('enabled', 1, '12:00', 'other-root', '[\"NY\"]', 'single', 0, 1, 'webp_balanced', 70, 0, 1, 1)", []).unwrap();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        0
    );
    assert_eq!(fixture.count("workflow_runs"), 0);
    assert_eq!(fixture.count("newspaper_batches"), 0);
    let disabled_checkpoint: i64 = fixture
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM newspaper_settings WHERE key = ?1",
            [checkpoint_key(&fixture.schedule.id)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(disabled_checkpoint, 0);
}

#[test]
fn unchanged_reconciliation_preserves_checkpoint_and_schedule_update_times() {
    let fixture = Fixture::new();
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13)).unwrap(),
        7
    );
    assert_eq!(
        materialize_due_at(&fixture.writer, &fixture.db_path, at(13) + 60).unwrap(),
        0
    );
    let connection = fixture.connection();
    let schedule_updated: i64 = connection
        .query_row(
            "SELECT updated_at FROM newspaper_schedules WHERE id = ?1",
            [&fixture.schedule.id],
            |row| row.get(0),
        )
        .unwrap();
    let checkpoint_updated: i64 = connection
        .query_row(
            "SELECT updated_at FROM newspaper_settings WHERE key = ?1",
            [checkpoint_key(&fixture.schedule.id)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(schedule_updated, at(13));
    assert_eq!(checkpoint_updated, at(13));
}
