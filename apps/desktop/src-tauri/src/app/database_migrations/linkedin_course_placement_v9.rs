//! Schema-v9 LinkedIn FirstWriterHome placement and frozen path folder names.

use rusqlite::{params, Connection, Result};

use crate::providers::linkedin::placement::LayoutName;

const CREATE_PLACEMENT: &str = r#"
CREATE TABLE IF NOT EXISTS linkedin_course_placement (
    course_slug TEXT NOT NULL,
    output_root TEXT NOT NULL,
    home_kind TEXT NOT NULL CHECK (home_kind IN ('standalone', 'certificate')),
    path_slug TEXT,
    layout_name TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (course_slug, output_root)
);
"#;

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT linkedin_course_placement_v9")?;
    let result = (|| {
        ensure_path_layout_name_column(connection)?;
        backfill_path_layout_names(connection)?;
        connection.execute_batch(CREATE_PLACEMENT)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE linkedin_course_placement_v9")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO linkedin_course_placement_v9;
         RELEASE linkedin_course_placement_v9;",
    );
    result
}

fn ensure_path_layout_name_column(connection: &Connection) -> Result<()> {
    if table_has_column(connection, "linkedin_learning_paths", "layout_name")? {
        return Ok(());
    }
    connection.execute(
        "ALTER TABLE linkedin_learning_paths ADD COLUMN layout_name TEXT",
        [],
    )?;
    Ok(())
}

fn backfill_path_layout_names(connection: &Connection) -> Result<()> {
    let rows = connection
        .prepare("SELECT path_slug, title FROM linkedin_learning_paths WHERE layout_name IS NULL")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>>>()?;
    for (path_slug, title) in rows {
        let layout_name = LayoutName::from_title(&title);
        connection.execute(
            "UPDATE linkedin_learning_paths SET layout_name = ?1 WHERE path_slug = ?2 AND layout_name IS NULL",
            params![layout_name.as_str(), path_slug],
        )?;
    }
    Ok(())
}

fn verify(connection: &Connection) -> Result<()> {
    let exists: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'linkedin_course_placement'",
        [],
        |row| row.get(0),
    )?;
    if exists != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if !table_has_column(connection, "linkedin_learning_paths", "layout_name")? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|name| name == column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::{initialize_database, schema_version, CURRENT_SCHEMA_VERSION};
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn linkedin_course_placement_migration_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        crate::app::database_migrations::linkedin_path_library_v8::install_and_verify(&connection)
            .unwrap();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'linkedin_course_placement'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
        assert!(table_has_column(&connection, "linkedin_learning_paths", "layout_name").unwrap());
    }

    #[test]
    fn v8_paths_backfill_sanitized_layout_name_and_placement_table() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        crate::app::database_migrations::linkedin_path_library_v8::install_and_verify(&connection)
            .unwrap();
        connection
            .execute(
                "INSERT INTO linkedin_learning_paths
                    (path_slug, title, source_url, created_at, updated_at)
                 VALUES ('github-cert', 'Career Essentials in GitHub?',
                    'https://www.linkedin.com/learning/paths/github-cert', 10, 10)",
                [],
            )
            .unwrap();
        install_and_verify(&connection).unwrap();
        let layout_name: String = connection
            .query_row(
                "SELECT layout_name FROM linkedin_learning_paths WHERE path_slug = 'github-cert'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(layout_name, "Career Essentials in GitHub-");
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'linkedin_course_placement'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[test]
    fn persistence_gate_v8_database_receives_course_placement_table() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        {
            let (connection, _) = initialize_database(&db_path).unwrap();
            connection
                .execute_batch(
                    "DROP TABLE IF EXISTS linkedin_course_placement;
                     PRAGMA user_version = 8;",
                )
                .unwrap();
        }

        let (connection, initialization) = initialize_database(&db_path).unwrap();
        assert_eq!(initialization.from_version, 8);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        assert!(initialization.backup_path.is_some());
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'linkedin_course_placement'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
}
