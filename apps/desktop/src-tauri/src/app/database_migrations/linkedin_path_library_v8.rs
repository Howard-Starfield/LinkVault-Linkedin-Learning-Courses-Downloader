//! Schema-v8 LinkedIn path membership catalog.

use rusqlite::{Connection, Result};

const INSTALL: &str = r#"
CREATE TABLE IF NOT EXISTS linkedin_learning_paths (
    path_slug TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    source_url TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS linkedin_path_membership (
    path_slug TEXT NOT NULL,
    course_slug TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (path_slug, course_slug),
    FOREIGN KEY (path_slug) REFERENCES linkedin_learning_paths(path_slug)
);

CREATE TABLE IF NOT EXISTS linkedin_standalone_courses (
    course_slug TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS linkedin_video_files (
    course_slug TEXT NOT NULL,
    video_slug TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (course_slug, video_slug)
);

CREATE TABLE IF NOT EXISTS linkedin_video_progress (
    course_slug TEXT NOT NULL,
    video_slug TEXT NOT NULL,
    position_ms INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    completed_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (course_slug, video_slug)
);
"#;

const TABLES: [&str; 5] = [
    "linkedin_learning_paths",
    "linkedin_path_membership",
    "linkedin_standalone_courses",
    "linkedin_video_files",
    "linkedin_video_progress",
];

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT linkedin_path_library_v8")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE linkedin_path_library_v8")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO linkedin_path_library_v8;
         RELEASE linkedin_path_library_v8;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    for table in TABLES {
        let exists: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
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
    fn linkedin_path_library_migration_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        for table in TABLES {
            let exists: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table} must exist");
        }
    }

    #[test]
    fn persistence_gate_v7_database_receives_linkedin_path_library_tables() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        {
            let (connection, _) = initialize_database(&db_path).unwrap();
            connection
                .execute_batch(
                    "DROP TABLE IF EXISTS linkedin_video_progress;
                     DROP TABLE IF EXISTS linkedin_video_files;
                     DROP TABLE IF EXISTS linkedin_standalone_courses;
                     DROP TABLE IF EXISTS linkedin_path_membership;
                     DROP TABLE IF EXISTS linkedin_learning_paths;
                     PRAGMA user_version = 7;",
                )
                .unwrap();
        }

        let (connection, initialization) = initialize_database(&db_path).unwrap();
        assert_eq!(initialization.from_version, 7);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        assert!(initialization.backup_path.is_some());
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        for table in TABLES {
            let exists: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table} must exist after v7→current migration");
        }
    }
}
