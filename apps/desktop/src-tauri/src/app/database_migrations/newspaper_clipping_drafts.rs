//! Schema-v6 recovery checkpoint migration for newspaper clipping notes.

use rusqlite::{Connection, Result};

const TABLE: &str = "newspaper_clipping_note_drafts";
const INSTALL: &str = r#"
CREATE TABLE IF NOT EXISTS newspaper_clipping_note_drafts (
    clipping_id TEXT PRIMARY KEY
        REFERENCES newspaper_clippings(id) ON DELETE CASCADE,
    base_revision INTEGER NOT NULL CHECK(base_revision >= 1),
    writer_session_id TEXT NOT NULL,
    writer_sequence INTEGER NOT NULL CHECK(writer_sequence >= 1),
    draft_title TEXT NOT NULL,
    draft_markdown TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

pub fn install_and_verify(connection: &Connection) -> Result<()> {
    connection.execute_batch("SAVEPOINT newspaper_clipping_drafts_v6")?;
    let result = (|| {
        connection.execute_batch(INSTALL)?;
        verify(connection)
    })();
    if result.is_ok() {
        connection.execute_batch("RELEASE newspaper_clipping_drafts_v6")?;
        return Ok(());
    }
    let _ = connection.execute_batch(
        "ROLLBACK TO newspaper_clipping_drafts_v6;
         RELEASE newspaper_clipping_drafts_v6;",
    );
    result
}

fn verify(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [TABLE],
        |row| row.get(0),
    )?;
    let normalized: String = sql
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '`' && *character != '"')
        .flat_map(char::to_lowercase)
        .collect();
    for required in [
        "clipping_idtextprimarykey",
        "base_revisionintegernotnullcheck(base_revision>=1)",
        "writer_session_idtextnotnull",
        "writer_sequenceintegernotnullcheck(writer_sequence>=1)",
        "draft_titletextnotnull",
        "draft_markdowntextnotnull",
        "updated_atintegernotnull",
    ] {
        if !normalized.contains(required) {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let columns = connection
        .prepare(
            "SELECT name, type, \"notnull\", pk
             FROM pragma_table_info('newspaper_clipping_note_drafts') ORDER BY cid",
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
        ("clipping_id", "TEXT", 0, 1),
        ("base_revision", "INTEGER", 1, 0),
        ("writer_session_id", "TEXT", 1, 0),
        ("writer_sequence", "INTEGER", 1, 0),
        ("draft_title", "TEXT", 1, 0),
        ("draft_markdown", "TEXT", 1, 0),
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
         FROM pragma_foreign_key_list('newspaper_clipping_note_drafts')",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if foreign_key
        != (
            "newspaper_clippings".to_string(),
            "clipping_id".to_string(),
            "id".to_string(),
            "CASCADE".to_string(),
        )
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let derived_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE (type = 'trigger' AND tbl_name = ?1)
            OR (type = 'table' AND name LIKE 'newspaper_clipping_note_drafts_fts%')",
        [TABLE],
        |row| row.get(0),
    )?;
    if derived_count != 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::{initialize_database, CURRENT_SCHEMA_VERSION};

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        connection
            .execute("CREATE TABLE newspaper_clippings (id TEXT PRIMARY KEY)", [])
            .unwrap();
        connection
    }

    #[test]
    fn clipping_draft_migration_is_verified_idempotent_and_has_no_search_objects() {
        let connection = connection();
        install_and_verify(&connection).unwrap();
        install_and_verify(&connection).unwrap();

        let derived_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE name LIKE 'newspaper_clipping_note_drafts_fts%'
                    OR (type = 'trigger' AND tbl_name = 'newspaper_clipping_note_drafts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(derived_count, 0);
    }

    #[test]
    fn clipping_draft_migration_rejects_an_existing_incompatible_table() {
        let connection = connection();
        connection
            .execute(
                "CREATE TABLE newspaper_clipping_note_drafts (clipping_id TEXT)",
                [],
            )
            .unwrap();

        assert!(install_and_verify(&connection).is_err());
        let columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('newspaper_clipping_note_drafts')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            columns, 1,
            "failed migration must not rewrite existing data"
        );
    }

    #[test]
    fn clipping_deletion_cascades_only_through_the_foreign_key() {
        let connection = connection();
        install_and_verify(&connection).unwrap();
        connection
            .execute("INSERT INTO newspaper_clippings(id) VALUES('clip')", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_clipping_note_drafts
                    (clipping_id, base_revision, writer_session_id, writer_sequence,
                     draft_title, draft_markdown, updated_at)
                 VALUES('clip', 1, 'session', 1, 'title', 'note', 1)",
                [],
            )
            .unwrap();
        connection
            .execute("DELETE FROM newspaper_clippings WHERE id = 'clip'", [])
            .unwrap();
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_clipping_note_drafts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn schema_v5_database_receives_a_verified_backup_before_v6() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&path).unwrap();
        connection
            .execute("DROP TABLE newspaper_clipping_note_drafts", [])
            .unwrap();
        connection.pragma_update(None, "user_version", 5).unwrap();
        drop(connection);

        let (connection, initialization) = initialize_database(&path).unwrap();
        assert_eq!(initialization.from_version, 5);
        assert_eq!(initialization.to_version, CURRENT_SCHEMA_VERSION);
        let backup_path = initialization.backup_path.unwrap();
        let backup = Connection::open(backup_path).unwrap();
        let backup_version: i32 = backup
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let backup_table: i64 = backup
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'newspaper_clipping_note_drafts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((backup_version, backup_table), (5, 0));
        verify(&connection).unwrap();
        let migrated_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM newspaper_clipping_note_drafts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated_rows, 0);
    }

    #[test]
    fn failed_v6_migration_retains_backup_version_and_incompatible_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&path).unwrap();
        connection
            .execute("DROP TABLE newspaper_clipping_note_drafts", [])
            .unwrap();
        connection
            .execute(
                "CREATE TABLE newspaper_clipping_note_drafts (
                    clipping_id TEXT PRIMARY KEY, retained TEXT NOT NULL
                 )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_clipping_note_drafts VALUES('broken', 'keep-me')",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 5).unwrap();
        drop(connection);

        assert!(initialize_database(&path).is_err());
        let connection = Connection::open(&path).unwrap();
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let retained: String = connection
            .query_row(
                "SELECT retained FROM newspaper_clipping_note_drafts WHERE clipping_id = 'broken'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((version, retained.as_str()), (5, "keep-me"));
        let backups = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(&format!("pre-migration-v5-to-v{CURRENT_SCHEMA_VERSION}"))
                    && entry.path().extension().is_some_and(|value| value == "bak")
            })
            .count();
        assert_eq!(backups, 1);
    }
}
