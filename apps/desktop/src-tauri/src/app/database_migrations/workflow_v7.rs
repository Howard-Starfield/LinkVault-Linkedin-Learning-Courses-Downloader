//! Schema-v7 workflow kernel tables.

use rusqlite::{Connection, Result};

const INSTALL: &str = r#"
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_type TEXT NOT NULL,
    provider TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'queued','running','paused','retry_wait','cancelling',
        'succeeded','succeeded_with_warnings','failed','cancelled'
    )),
    legacy_origin TEXT,
    legacy_id TEXT,
    request_json TEXT NOT NULL,
    output_root TEXT NOT NULL,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_runs_legacy
    ON workflow_runs(legacy_origin, legacy_id)
    WHERE legacy_origin IS NOT NULL AND legacy_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_state ON workflow_runs(state);

CREATE TABLE IF NOT EXISTS workflow_steps (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    step_key TEXT NOT NULL,
    step_type TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'pending','ready','running','retry_wait',
        'succeeded','skipped','failed','cancelled'
    )),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(run_id, step_key)
);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_ready ON workflow_steps(state, created_at);

CREATE TABLE IF NOT EXISTS workflow_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    step_id TEXT REFERENCES workflow_steps(id) ON DELETE SET NULL,
    sequence INTEGER NOT NULL CHECK(sequence >= 1),
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_workflow_events_run ON workflow_events(run_id, sequence);
"#;

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT workflow_kernel_v7")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE workflow_kernel_v7")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO workflow_kernel_v7;
         RELEASE workflow_kernel_v7;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    for table in ["workflow_runs", "workflow_steps", "workflow_events"] {
        let exists: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let run_state_sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'workflow_runs'",
        [],
        |row| row.get(0),
    )?;
    let normalized: String = run_state_sql
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '`' && *character != '"')
        .flat_map(char::to_lowercase)
        .collect();
    for required in [
        "statetextnotnullcheck(statein(",
        "succeeded_with_warnings",
        "legacy_origintext",
        "legacy_idtext",
        "request_jsontextnotnull",
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
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn workflow_kernel_migration_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'workflow_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn persistence_gate_v6_database_receives_workflow_and_youtube_tables() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        {
            let (connection, _) = initialize_database(&db_path).unwrap();
            connection
                .execute_batch(
                    "DROP TABLE IF EXISTS workflow_events;
                     DROP TABLE IF EXISTS workflow_steps;
                     DROP TABLE IF EXISTS workflow_runs;
                     DROP TABLE IF EXISTS youtube_jobs;
                     PRAGMA user_version = 6;",
                )
                .unwrap();
        }

        let (connection, initialization) = initialize_database(&db_path).unwrap();
        assert_eq!(initialization.from_version, 6);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        assert!(initialization.backup_path.is_some());
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        for table in [
            "youtube_jobs",
            "workflow_runs",
            "workflow_steps",
            "workflow_events",
        ] {
            let exists: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table} must exist after v6→current migration");
        }
    }
}
