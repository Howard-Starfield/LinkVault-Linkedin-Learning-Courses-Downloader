//! Schema-v11 YouTube in-app playback resume positions.
//!
//! One row per (run, occurrence). Rows belong to the workflow run, so the
//! `ON DELETE CASCADE` foreign key removes them whenever YouTube history for
//! that run is deleted or cleared (`delete_run_if_terminal`,
//! `delete_runs_by_workflow_type`, ...), with no provider-side cleanup code to
//! forget.
//!
//! Compatibility and rollback: the table is purely additive and nothing else
//! reads it, so the migration only ever creates it (`IF NOT EXISTS`, safe to
//! retry after an interrupted install). `initialize` takes a verified backup
//! before bumping `user_version`; an older app refuses the newer schema
//! version instead of running against it, so rollback means restoring that
//! backup (or dropping the table), which loses only resume positions.

use rusqlite::{Connection, Result};

const TABLE: &str = "youtube_playback_progress";
const INSTALL: &str = r#"
CREATE TABLE IF NOT EXISTS youtube_playback_progress (
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    occurrence_id TEXT NOT NULL,
    position_seconds REAL NOT NULL CHECK(position_seconds >= 0),
    duration_seconds REAL NOT NULL CHECK(duration_seconds >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0, 1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (run_id, occurrence_id)
);
"#;

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT youtube_playback_progress_v11")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE youtube_playback_progress_v11")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO youtube_playback_progress_v11;
         RELEASE youtube_playback_progress_v11;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    let columns = connection
        .prepare(
            "SELECT name, type, \"notnull\", pk
             FROM pragma_table_info('youtube_playback_progress') ORDER BY cid",
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
        ("occurrence_id", "TEXT", 1, 2),
        ("position_seconds", "REAL", 1, 0),
        ("duration_seconds", "REAL", 1, 0),
        ("completed", "INTEGER", 1, 0),
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
         FROM pragma_foreign_key_list('youtube_playback_progress')",
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
        "check(position_seconds>=0)",
        "check(duration_seconds>=0)",
        "check(completedin(0,1))",
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
                 ) VALUES (?1, 'youtube_download', 'youtube', 'succeeded', '{}', 'C:/out', 1, 1)",
                [id],
            )
            .unwrap();
    }

    fn progress_rows(connection: &Connection) -> i64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM youtube_playback_progress",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn youtube_playback_progress_migration_is_idempotent() {
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
    fn persistence_gate_v10_database_receives_youtube_playback_progress_with_backup() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        {
            let (connection, _) = initialize_database(&db_path).unwrap();
            insert_run(&connection, "run-keep");
            connection
                .execute_batch(
                    "DROP TABLE youtube_playback_progress;
                     PRAGMA user_version = 10;",
                )
                .unwrap();
            assert_eq!(schema_version(&connection).unwrap(), 10);
        }

        let (connection, initialization) = initialize_database(&db_path).unwrap();
        assert_eq!(initialization.from_version, 10);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        let backup_path = initialization
            .backup_path
            .expect("a populated v10 database must be backed up before migrating");
        assert!(backup_path.is_file());
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(progress_rows(&connection), 0);
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
    fn deleting_a_run_removes_its_playback_progress() {
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
                    "INSERT INTO youtube_playback_progress
                        (run_id, occurrence_id, position_seconds, duration_seconds, completed, updated_at)
                     VALUES (?1, 'occ-1', 12.5, 100.0, 0, 5)",
                    [run],
                )
                .unwrap();
        }
        connection
            .execute("DELETE FROM workflow_runs WHERE id = 'run-a'", [])
            .unwrap();
        assert_eq!(progress_rows(&connection), 1);
        let remaining: String = connection
            .query_row("SELECT run_id FROM youtube_playback_progress", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, "run-b");
    }

    #[test]
    fn schema_rejects_rows_for_unknown_runs_and_negative_values() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        insert_run(&connection, "run-a");
        let insert = |run: &str, position: f64, completed: i64| {
            connection.execute(
                "INSERT INTO youtube_playback_progress
                    (run_id, occurrence_id, position_seconds, duration_seconds, completed, updated_at)
                 VALUES (?1, 'occ-1', ?2, 10.0, ?3, 1)",
                rusqlite::params![run, position, completed],
            )
        };
        assert!(insert("missing-run", 1.0, 0).is_err());
        assert!(insert("run-a", -1.0, 0).is_err());
        assert!(insert("run-a", 1.0, 2).is_err());
        assert!(insert("run-a", 1.0, 1).is_ok());
    }
}
