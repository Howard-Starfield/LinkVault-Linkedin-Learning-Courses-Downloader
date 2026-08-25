//! SQLite-backed persistence for Coursera jobs and events.
//!
//! Schemas are installed once by `cache::initialize_database` during startup.
//! The functions here are pure SQL helpers.

#![allow(dead_code)] // Phase 9 — wired by Phase 10

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::coursera::error::CourseraResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseraJob {
    pub id: String,
    pub class_name: String,
    pub status: String,
    pub options_json: String,
    pub output_dir: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub counts_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedCourseraEvent {
    pub id: i64,
    pub job_id: String,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: i64,
}

pub fn insert_job(conn: &Connection, job: &CourseraJob) -> CourseraResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO coursera_jobs (id, class_name, status, options_json, output_dir, created_at, updated_at, counts_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            job.id, job.class_name, job.status, job.options_json,
            job.output_dir, job.created_at, job.updated_at, job.counts_json
        ],
    )?;
    Ok(())
}

pub fn update_job_status(
    conn: &Connection,
    id: &str,
    status: &str,
    updated_at: i64,
) -> CourseraResult<()> {
    conn.execute(
        "UPDATE coursera_jobs SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, updated_at, id],
    )?;
    Ok(())
}

pub fn append_job_event(
    conn: &Connection,
    job_id: &str,
    event_type: &str,
    payload_json: &str,
    created_at: i64,
) -> CourseraResult<i64> {
    conn.execute(
        "INSERT INTO coursera_job_events (job_id, event_type, payload_json, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![job_id, event_type, payload_json, created_at],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_jobs_by_status(conn: &Connection, status: &str) -> CourseraResult<Vec<CourseraJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, class_name, status, options_json, output_dir, created_at, updated_at, counts_json \
         FROM coursera_jobs WHERE status = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map(params![status], |row| {
            Ok(CourseraJob {
                id: row.get(0)?,
                class_name: row.get(1)?,
                status: row.get(2)?,
                options_json: row.get(3)?,
                output_dir: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                counts_json: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_recent_jobs(conn: &Connection, limit: usize) -> CourseraResult<Vec<CourseraJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, class_name, status, options_json, output_dir, created_at, updated_at, counts_json \
         FROM coursera_jobs ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok(CourseraJob {
                id: row.get(0)?,
                class_name: row.get(1)?,
                status: row.get(2)?,
                options_json: row.get(3)?,
                output_dir: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                counts_json: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_job(conn: &Connection, id: &str) -> CourseraResult<Option<CourseraJob>> {
    conn.query_row(
        "SELECT id, class_name, status, options_json, output_dir, created_at, updated_at, counts_json \
         FROM coursera_jobs WHERE id = ?1",
        params![id],
        |row| {
            Ok(CourseraJob {
                id: row.get(0)?,
                class_name: row.get(1)?,
                status: row.get(2)?,
                options_json: row.get(3)?,
                output_dir: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                counts_json: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_job_events(
    conn: &Connection,
    job_id: &str,
    limit: usize,
) -> CourseraResult<Vec<PersistedCourseraEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, event_type, payload_json, created_at \
         FROM coursera_job_events WHERE job_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![job_id, limit as i64], |row| {
            Ok(PersistedCourseraEvent {
                id: row.get(0)?,
                job_id: row.get(1)?,
                event_type: row.get(2)?,
                payload_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_recent_events(
    conn: &Connection,
    limit: usize,
) -> CourseraResult<Vec<PersistedCourseraEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, event_type, payload_json, created_at \
         FROM coursera_job_events ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok(PersistedCourseraEvent {
                id: row.get(0)?,
                job_id: row.get(1)?,
                event_type: row.get(2)?,
                payload_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_completed_jobs(conn: &Connection) -> CourseraResult<Vec<CourseraJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, class_name, status, options_json, output_dir, created_at, updated_at, counts_json \
         FROM coursera_jobs WHERE lower(status) = 'completed' ORDER BY updated_at DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CourseraJob {
                id: row.get(0)?,
                class_name: row.get(1)?,
                status: row.get(2)?,
                options_json: row.get(3)?,
                output_dir: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                counts_json: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn delete_job(conn: &Connection, id: &str) -> CourseraResult<()> {
    conn.execute(
        "DELETE FROM coursera_job_events WHERE job_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM coursera_jobs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn delete_failed_job(conn: &Connection, id: &str) -> CourseraResult<bool> {
    let transaction = conn.unchecked_transaction()?;
    let removed = transaction.execute(
        "DELETE FROM coursera_jobs
         WHERE id = ?1 AND lower(status) IN ('failed', 'cancelled')",
        params![id],
    )?;
    if removed == 0 {
        transaction.rollback()?;
        return Ok(false);
    }
    transaction.execute(
        "DELETE FROM coursera_job_events WHERE job_id = ?1",
        params![id],
    )?;
    transaction.commit()?;
    Ok(true)
}

pub fn retry_failed_job(
    conn: &Connection,
    id: &str,
    updated_at: i64,
) -> CourseraResult<Option<CourseraJob>> {
    let transaction = conn.unchecked_transaction()?;
    let updated = transaction.execute(
        "UPDATE coursera_jobs
         SET status = 'Queued', updated_at = ?2
         WHERE id = ?1 AND lower(status) IN ('failed', 'cancelled')",
        params![id, updated_at],
    )?;
    if updated == 0 {
        transaction.rollback()?;
        return Ok(None);
    }
    transaction.execute(
        "INSERT INTO coursera_job_events (job_id, event_type, payload_json, created_at)
         VALUES (?1, 'retry_queued', ?2, ?3)",
        params![
            id,
            serde_json::json!({ "message": "Retry queued" }).to_string(),
            updated_at
        ],
    )?;
    let job = get_job(&transaction, id)?;
    transaction.commit()?;
    Ok(job)
}

pub fn reconcile_after_restart(conn: &Connection, updated_at: i64) -> CourseraResult<usize> {
    let transaction = conn.unchecked_transaction()?;
    let mut stmt =
        transaction.prepare("SELECT id FROM coursera_jobs WHERE lower(status) = 'active'")?;
    let job_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let payload = serde_json::json!({
        "message": "Coursera job was interrupted by an application restart"
    })
    .to_string();
    for job_id in &job_ids {
        transaction.execute(
            "UPDATE coursera_jobs SET status = 'Failed', updated_at = ?2 WHERE id = ?1",
            params![job_id, updated_at],
        )?;
        transaction.execute(
            "INSERT INTO coursera_job_events (job_id, event_type, payload_json, created_at)
             VALUES (?1, 'restart_failed', ?2, ?3)",
            params![job_id, payload, updated_at],
        )?;
    }
    transaction.commit()?;
    Ok(job_ids.len())
}

pub fn clear_failed_jobs(conn: &Connection) -> CourseraResult<usize> {
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "DELETE FROM coursera_job_events
         WHERE job_id IN (
             SELECT id FROM coursera_jobs
             WHERE lower(status) IN ('failed', 'cancelled')
        )",
        [],
    )?;
    let count = transaction.execute(
        "DELETE FROM coursera_jobs WHERE lower(status) IN ('failed', 'cancelled')",
        [],
    )?;
    transaction.commit()?;
    Ok(count)
}

pub fn save_setting(conn: &Connection, key: &str, value_json: &str) -> CourseraResult<()> {
    conn.execute(
        "INSERT INTO coursera_settings (key, value_json) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![key, value_json],
    )?;
    Ok(())
}

pub fn load_setting(conn: &Connection, key: &str) -> CourseraResult<Option<String>> {
    let mut stmt = conn.prepare("SELECT value_json FROM coursera_settings WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE coursera_jobs (
                id TEXT PRIMARY KEY,
                class_name TEXT NOT NULL,
                status TEXT NOT NULL,
                options_json TEXT NOT NULL,
                output_dir TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                counts_json TEXT NOT NULL DEFAULT '{}'
             );
             CREATE TABLE coursera_job_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE coursera_settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn fixture_job(id: &str, status: &str) -> CourseraJob {
        CourseraJob {
            id: id.into(),
            class_name: "ml-005".into(),
            status: status.into(),
            options_json: "{}".into(),
            output_dir: ".".into(),
            created_at: 100,
            updated_at: 100,
            counts_json: "{}".into(),
        }
    }

    #[test]
    fn insert_and_list_jobs() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Queued")).unwrap();
        insert_job(&conn, &fixture_job("j2", "Failed")).unwrap();
        let all = list_recent_jobs(&conn, 10).unwrap();
        assert_eq!(all.len(), 2);
        let failed = list_jobs_by_status(&conn, "Failed").unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].id, "j2");
    }

    #[test]
    fn update_job_status_works() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Queued")).unwrap();
        update_job_status(&conn, "j1", "Running", 200).unwrap();
        let jobs = list_jobs_by_status(&conn, "Running").unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].updated_at, 200);
    }

    #[test]
    fn append_and_list_events() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Queued")).unwrap();
        let id = append_job_event(&conn, "j1", "file_finished", "{}", 100).unwrap();
        assert!(id > 0);
        let events = list_job_events(&conn, "j1", 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "file_finished");
    }

    #[test]
    fn list_recent_events_orders_newest_first() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Queued")).unwrap();
        append_job_event(&conn, "j1", "first", "{}", 100).unwrap();
        append_job_event(&conn, "j1", "second", "{}", 200).unwrap();
        let events = list_recent_events(&conn, 10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "second");
    }

    #[test]
    fn delete_job_removes_events() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Queued")).unwrap();
        append_job_event(&conn, "j1", "x", "{}", 100).unwrap();
        delete_job(&conn, "j1").unwrap();
        assert_eq!(list_recent_jobs(&conn, 10).unwrap().len(), 0);
        assert_eq!(list_job_events(&conn, "j1", 10).unwrap().len(), 0);
    }

    #[test]
    fn clear_failed_jobs_keeps_other_statuses() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Failed")).unwrap();
        insert_job(&conn, &fixture_job("j2", "Cancelled")).unwrap();
        insert_job(&conn, &fixture_job("j3", "Queued")).unwrap();
        append_job_event(&conn, "j1", "failed", "{}", 100).unwrap();
        append_job_event(&conn, "j2", "cancelled", "{}", 100).unwrap();
        append_job_event(&conn, "j3", "queued", "{}", 100).unwrap();
        let n = clear_failed_jobs(&conn).unwrap();
        assert_eq!(n, 2);
        assert_eq!(list_recent_jobs(&conn, 10).unwrap().len(), 1);
        let events = list_recent_events(&conn, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].job_id, "j3");
    }

    #[test]
    fn get_job_finds_exact_job() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Failed")).unwrap();
        assert_eq!(get_job(&conn, "j1").unwrap().unwrap().status, "Failed");
        assert!(get_job(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn delete_failed_job_rejects_non_terminal_status_and_removes_events_atomically() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("failed", "Failed")).unwrap();
        insert_job(&conn, &fixture_job("queued", "Queued")).unwrap();
        append_job_event(&conn, "failed", "failure", "{}", 100).unwrap();
        append_job_event(&conn, "queued", "queued", "{}", 100).unwrap();

        assert!(delete_failed_job(&conn, "failed").unwrap());
        assert!(!delete_failed_job(&conn, "queued").unwrap());
        assert!(get_job(&conn, "failed").unwrap().is_none());
        assert!(get_job(&conn, "queued").unwrap().is_some());
        assert!(list_job_events(&conn, "failed", 10).unwrap().is_empty());
        assert_eq!(list_job_events(&conn, "queued", 10).unwrap().len(), 1);
    }

    #[test]
    fn retry_failed_job_updates_only_failed_or_cancelled_jobs() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("failed", "Failed")).unwrap();
        insert_job(&conn, &fixture_job("queued", "Queued")).unwrap();

        let retried = retry_failed_job(&conn, "failed", 200).unwrap().unwrap();
        assert_eq!(retried.status, "Queued");
        assert_eq!(retried.updated_at, 200);
        assert_eq!(
            list_job_events(&conn, "failed", 10).unwrap()[0].event_type,
            "retry_queued"
        );
        assert!(retry_failed_job(&conn, "queued", 200).unwrap().is_none());
    }

    #[test]
    fn list_completed_jobs_returns_only_completed() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("j1", "Completed")).unwrap();
        insert_job(&conn, &fixture_job("j2", "Queued")).unwrap();
        let jobs = list_completed_jobs(&conn).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "j1");
    }

    #[test]
    fn save_and_load_setting() {
        let conn = fresh_schema();
        save_setting(&conn, "key1", "{\"a\":1}").unwrap();
        let got = load_setting(&conn, "key1").unwrap().unwrap();
        assert_eq!(got, "{\"a\":1}");
        save_setting(&conn, "key1", "{\"a\":2}").unwrap();
        let got = load_setting(&conn, "key1").unwrap().unwrap();
        assert_eq!(got, "{\"a\":2}");
    }

    #[test]
    fn load_setting_returns_none_for_missing() {
        let conn = fresh_schema();
        let got = load_setting(&conn, "missing").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn restart_reconciliation_marks_active_jobs_failed_and_leaves_others() {
        let conn = fresh_schema();
        insert_job(&conn, &fixture_job("active", "Active")).unwrap();
        insert_job(&conn, &fixture_job("queued", "Queued")).unwrap();
        insert_job(&conn, &fixture_job("done", "Completed")).unwrap();

        let count = reconcile_after_restart(&conn, 400).unwrap();
        assert_eq!(count, 1);
        assert_eq!(get_job(&conn, "active").unwrap().unwrap().status, "Failed");
        assert_eq!(get_job(&conn, "queued").unwrap().unwrap().status, "Queued");
        assert_eq!(get_job(&conn, "done").unwrap().unwrap().status, "Completed");
        let events = list_job_events(&conn, "active", 10).unwrap();
        assert_eq!(events[0].event_type, "restart_failed");
        assert_eq!(events[0].created_at, 400);
    }
}
