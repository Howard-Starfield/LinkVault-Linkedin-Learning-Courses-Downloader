//! Schema-v8 query indexes for the download bootstrap read path.
//!
//! `jobs.status`, `artifacts.job_id` and `job_events.job_id` were never
//! indexed, so the per-job lookups that `load_bootstrap_state` issues once per
//! job degraded into full table scans. These indexes restore the index search
//! those reads were written against; they are additive and idempotent, so an
//! interrupted install can simply be retried.

use rusqlite::{Connection, Result};

const INSTALL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_job_events_job ON job_events(job_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_job ON artifacts(job_id);
CREATE INDEX IF NOT EXISTS idx_job_events_recent
    ON job_events(created_at DESC, id DESC);
"#;

/// `(index name, owning table)` pairs that must exist after installation.
const EXPECTED_INDEXES: [(&str, &str); 4] = [
    ("idx_jobs_status", "jobs"),
    ("idx_job_events_job", "job_events"),
    ("idx_artifacts_job", "artifacts"),
    ("idx_job_events_recent", "job_events"),
];

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT linkedin_query_indexes_v8")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE linkedin_query_indexes_v8")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO linkedin_query_indexes_v8;
         RELEASE linkedin_query_indexes_v8;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    for (index, table) in EXPECTED_INDEXES {
        let owner: String = connection.query_row(
            "SELECT COALESCE((SELECT tbl_name FROM sqlite_master
                               WHERE type = 'index' AND name = ?1), '')",
            [index],
            |row| row.get(0),
        )?;
        if owner != table {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs (id TEXT PRIMARY KEY NOT NULL, status TEXT NOT NULL);
                 CREATE TABLE job_events (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     job_id TEXT NOT NULL,
                     created_at INTEGER NOT NULL
                 );
                 CREATE TABLE artifacts (
                     id TEXT PRIMARY KEY NOT NULL,
                     job_id TEXT NOT NULL
                 );",
            )
            .unwrap();
        connection
    }

    fn index_owner(connection: &Connection, name: &str) -> String {
        connection
            .query_row(
                "SELECT COALESCE((SELECT tbl_name FROM sqlite_master
                                   WHERE type = 'index' AND name = ?1), '')",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn linkedin_query_indexes_are_verified_and_idempotent() {
        let connection = connection();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        for (index, table) in EXPECTED_INDEXES {
            assert_eq!(index_owner(&connection, index), table);
        }
    }

    #[test]
    fn linkedin_query_indexes_reject_a_name_owned_by_the_wrong_table() {
        let connection = connection();
        // A stale index of the same name on the wrong table is silently kept by
        // `CREATE INDEX IF NOT EXISTS`, so verification has to catch it.
        connection
            .execute("CREATE INDEX idx_jobs_status ON job_events(created_at)", [])
            .unwrap();

        assert!(install_and_verify(&connection).is_err());
        assert_eq!(index_owner(&connection, "idx_job_events_job"), "");
        assert_eq!(index_owner(&connection, "idx_artifacts_job"), "");
        assert_eq!(index_owner(&connection, "idx_job_events_recent"), "");
    }
}
