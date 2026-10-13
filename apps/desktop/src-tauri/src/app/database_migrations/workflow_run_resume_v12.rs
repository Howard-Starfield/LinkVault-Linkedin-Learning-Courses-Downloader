//! Schema-v12 durable resume bookkeeping for interrupted workflow runs.
//!
//! One optional row per run records whether an interrupted run is waiting for
//! its single automatic resume, waiting for the user to press Continue, or is
//! in the middle of a resumed attempt, plus the automatic-resume budget
//! (`auto_resume_used`). The budget is a durable boolean so a crash during the
//! automatic attempt cannot grant a second automatic attempt on the next
//! launch.
//!
//! Compatibility and rollback: the table is purely additive and only the
//! workflow resume code reads it, so the migration only ever creates it
//! (`IF NOT EXISTS`, safe to retry after an interrupted install). Rows belong
//! to the run: `ON DELETE CASCADE` removes them with the run. `initialize`
//! takes a verified backup before bumping `user_version`; an older app
//! refuses the newer schema version instead of running against it, so
//! rollback means restoring that backup (or dropping the table), which loses
//! only resume bookkeeping: those runs were already marked non-running.

use rusqlite::{Connection, Result};

const TABLE: &str = "workflow_run_resume";
const INSTALL: &str = r#"
CREATE TABLE IF NOT EXISTS workflow_run_resume (
    run_id TEXT NOT NULL PRIMARY KEY REFERENCES workflow_runs(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('auto_pending', 'needs_user', 'resuming')),
    auto_resume_used INTEGER NOT NULL DEFAULT 0 CHECK(auto_resume_used IN (0, 1)),
    last_interruption_reason TEXT,
    reason_code TEXT,
    expected_bytes INTEGER CHECK(expected_bytes IS NULL OR expected_bytes >= 0),
    expected_item_key TEXT,
    updated_at INTEGER NOT NULL
);
"#;

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT workflow_run_resume_v12")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE workflow_run_resume_v12")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO workflow_run_resume_v12;
         RELEASE workflow_run_resume_v12;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    let columns = connection
        .prepare(
            "SELECT name, type, \"notnull\", pk
             FROM pragma_table_info('workflow_run_resume') ORDER BY cid",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>>>()?;
    let expected = [
        ("run_id", "TEXT", 1, 1),
        ("state", "TEXT", 1, 0),
        ("auto_resume_used", "INTEGER", 1, 0),
        ("last_interruption_reason", "TEXT", 0, 0),
        ("reason_code", "TEXT", 0, 0),
        ("expected_bytes", "INTEGER", 0, 0),
        ("expected_item_key", "TEXT", 0, 0),
        ("updated_at", "INTEGER", 1, 0),
    ];
    if columns
        != expected
            .map(|(name, kind, not_null, primary)| {
                (name.to_string(), kind.to_string(), not_null, primary)
            })
            .to_vec()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let foreign_key: (String, String, String, String) = connection.query_row(
        "SELECT \"table\", \"from\", \"to\", on_delete
         FROM pragma_foreign_key_list('workflow_run_resume')",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if foreign_key
        != (
            "workflow_runs".to_string(),
            "run_id".to_string(),
            "id".to_string(),
            "CASCADE".to_string(),
        )
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [TABLE],
        |row| row.get(0),
    )?;
    let normalized: String = sql
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    for required in [
        "check(statein('auto_pending','needs_user','resuming'))",
        "check(auto_resume_usedin(0,1))",
    ] {
        if !normalized.contains(required) {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::{initialize_database, schema_version, CURRENT_SCHEMA_VERSION};
    use tempfile::tempdir;

    fn insert_run(connection: &Connection, id: &str) {
        connection
            .execute(
                "INSERT INTO workflow_runs (
                    id, workflow_type, provider, state, request_json, output_root,
                    created_at, updated_at
                 ) VALUES (?1, 'youtube_download', 'youtube', 'paused', '{}', 'C:/out', 1, 1)",
                [id],
            )
            .unwrap();
    }

    fn resume_rows(connection: &Connection) -> i64 {
        connection
            .query_row("SELECT COUNT(*) FROM workflow_run_resume", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn workflow_run_resume_migration_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        crate::app::database_migrations::migrate(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [TABLE],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[test]
    fn persistence_gate_v11_database_receives_workflow_run_resume_with_backup() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        {
            let (connection, _) = initialize_database(&db_path).unwrap();
            insert_run(&connection, "run-keep");
            connection
                .execute_batch(
                    "DROP TABLE workflow_run_resume;
                     PRAGMA user_version = 11;",
                )
                .unwrap();
            assert_eq!(schema_version(&connection).unwrap(), 11);
        }

        let (connection, initialization) = initialize_database(&db_path).unwrap();
        assert_eq!(initialization.from_version, 11);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        let backup_path = initialization
            .backup_path
            .expect("a populated v11 database must be backed up before migrating");
        assert!(backup_path.is_file());
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(resume_rows(&connection), 0);
        let preserved: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM workflow_runs WHERE id = 'run-keep'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, 1, "existing runs survive the migration");
    }

    #[test]
    fn deleting_a_run_removes_its_resume_row() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        insert_run(&connection, "run-a");
        insert_run(&connection, "run-b");
        for run in ["run-a", "run-b"] {
            connection
                .execute(
                    "INSERT INTO workflow_run_resume (run_id, state, updated_at)
                     VALUES (?1, 'auto_pending', 5)",
                    [run],
                )
                .unwrap();
        }
        connection
            .execute("DELETE FROM workflow_runs WHERE id = 'run-a'", [])
            .unwrap();
        assert_eq!(resume_rows(&connection), 1);
    }

    #[test]
    fn schema_rejects_unknown_runs_unknown_states_and_a_non_boolean_budget() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        insert_run(&connection, "run-a");
        let insert = |run: &str, state: &str, used: i64| {
            connection.execute(
                "INSERT OR REPLACE INTO workflow_run_resume
                    (run_id, state, auto_resume_used, updated_at)
                 VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![run, state, used],
            )
        };
        assert!(insert("missing-run", "auto_pending", 0).is_err());
        assert!(insert("run-a", "failed", 0).is_err());
        assert!(insert("run-a", "needs_user", 2).is_err());
        assert!(insert("run-a", "needs_user", 1).is_ok());
    }
}
