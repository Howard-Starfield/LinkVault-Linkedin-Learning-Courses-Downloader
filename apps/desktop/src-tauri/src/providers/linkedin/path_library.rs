//! LinkedIn path membership catalog. Jobs remain download attempts.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::app::database_diagnostics::DatabaseProvider;
use crate::app::database_writer::{DatabaseWriteContext, DatabaseWriteError, DatabaseWriter};

use super::expansion::PathCapture;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathSlug(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CourseSlug(String);

impl PathSlug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, PathLibraryError> {
        parse_slug(value.as_ref()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CourseSlug {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, PathLibraryError> {
        parse_slug(value.as_ref()).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_slug(value: &str) -> Result<String, PathLibraryError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(PathLibraryError::InvalidSlug {
            value: value.to_string(),
        });
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Error)]
pub enum PathLibraryError {
    #[error("invalid LinkedIn slug '{value}'")]
    InvalidSlug { value: String },
    #[error(transparent)]
    Database(#[from] DatabaseWriteError),
}

#[derive(Clone)]
pub struct PathLibrary {
    writer: DatabaseWriter,
}

impl PathLibrary {
    pub fn new(writer: DatabaseWriter) -> Self {
        Self { writer }
    }

    pub fn capture_path(&self, capture: PathCapture) -> Result<(), PathLibraryError> {
        let path = PathSlug::parse(&capture.path_slug)?;
        let mut members = Vec::with_capacity(capture.members.len());
        for (position, slug) in capture.members.iter().enumerate() {
            members.push((position as i64, CourseSlug::parse(slug)?));
        }
        let title = capture.title;
        let source_url = capture.source_url;
        let now = unix_timestamp();
        self.writer
            .execute(write_context("capture_path"), move |connection| {
                upsert_path(connection, &path, &title, &source_url, &members, now)?;
                Ok(())
            })?;
        Ok(())
    }

    pub fn capture_standalone(&self, course: CourseSlug) -> Result<(), PathLibraryError> {
        let now = unix_timestamp();
        self.writer
            .execute(write_context("capture_standalone"), move |connection| {
                connection.execute(
                    "INSERT INTO linkedin_standalone_courses (course_slug, created_at)
                     VALUES (?1, ?2)
                     ON CONFLICT(course_slug) DO NOTHING",
                    params![course.as_str(), now],
                )?;
                Ok(())
            })?;
        Ok(())
    }
}

fn write_context(operation: &'static str) -> DatabaseWriteContext {
    DatabaseWriteContext {
        operation,
        provider: DatabaseProvider::Linkedin,
        workflow_id: None,
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn upsert_path(
    connection: &mut Connection,
    path: &PathSlug,
    title: &str,
    source_url: &str,
    members: &[(i64, CourseSlug)],
    now: i64,
) -> Result<(), DatabaseWriteError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO linkedin_learning_paths (path_slug, title, source_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(path_slug) DO UPDATE SET
            title = excluded.title,
            source_url = excluded.source_url,
            updated_at = excluded.updated_at",
        params![path.as_str(), title, source_url, now],
    )?;
    for (position, course) in members {
        transaction.execute(
            "INSERT INTO linkedin_path_membership (path_slug, course_slug, position, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path_slug, course_slug) DO UPDATE SET
                position = excluded.position",
            params![path.as_str(), course.as_str(), position, now],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::database::initialize_database;
    use crate::app::database_diagnostics::DatabaseDiagnostics;
    use tempfile::TempDir;

    fn harness() -> (TempDir, PathLibrary, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let (connection, _) = initialize_database(&db_path).unwrap();
        drop(connection);
        let writer =
            DatabaseWriter::start(db_path.clone(), DatabaseDiagnostics::default()).unwrap();
        (directory, PathLibrary::new(writer), db_path)
    }

    fn path_capture(slug: &str, title: &str, members: &[&str]) -> PathCapture {
        PathCapture {
            path_slug: slug.to_string(),
            title: title.to_string(),
            source_url: format!("https://www.linkedin.com/learning/paths/{slug}"),
            members: members.iter().map(|member| (*member).to_string()).collect(),
        }
    }

    fn open_reader(db_path: &std::path::Path) -> Connection {
        crate::cache::open_runtime(db_path).unwrap()
    }

    #[test]
    fn capture_path_twice_is_one_path_row_and_stable_membership() {
        let (_directory, library, db_path) = harness();
        let capture = path_capture(
            "github-cert",
            "GitHub Certificate",
            &["practical-github-actions", "practical-github-copilot"],
        );
        library.capture_path(capture.clone()).unwrap();
        library.capture_path(capture).unwrap();

        let connection = open_reader(&db_path);
        let path_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM linkedin_learning_paths", [], |row| {
                row.get(0)
            })
            .unwrap();
        let members: Vec<(String, i64)> = connection
            .prepare(
                "SELECT course_slug, position FROM linkedin_path_membership
                 WHERE path_slug = 'github-cert' ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(path_count, 1);
        assert_eq!(
            members,
            vec![
                ("practical-github-actions".to_string(), 0),
                ("practical-github-copilot".to_string(), 1),
            ]
        );
    }

    #[test]
    fn same_course_slug_can_belong_to_two_paths() {
        let (_directory, library, db_path) = harness();
        library
            .capture_path(path_capture(
                "path-a",
                "Path A",
                &["shared-course", "only-a"],
            ))
            .unwrap();
        library
            .capture_path(path_capture(
                "path-b",
                "Path B",
                &["shared-course", "only-b"],
            ))
            .unwrap();

        let connection = open_reader(&db_path);
        let memberships: Vec<String> = connection
            .prepare(
                "SELECT path_slug FROM linkedin_path_membership
                 WHERE course_slug = 'shared-course' ORDER BY path_slug",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            memberships,
            vec!["path-a".to_string(), "path-b".to_string()]
        );
        let path_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM linkedin_learning_paths", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(path_count, 2);
    }

    #[test]
    fn capture_standalone_plus_membership_records_both() {
        let (_directory, library, db_path) = harness();
        library
            .capture_path(path_capture(
                "github-cert",
                "GitHub Certificate",
                &["practical-github-actions"],
            ))
            .unwrap();
        library
            .capture_standalone(CourseSlug::parse("practical-github-actions").unwrap())
            .unwrap();
        library
            .capture_standalone(CourseSlug::parse("practical-github-actions").unwrap())
            .unwrap();

        let connection = open_reader(&db_path);
        let standalone: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM linkedin_standalone_courses WHERE course_slug = 'practical-github-actions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let membership: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM linkedin_path_membership
                 WHERE path_slug = 'github-cert' AND course_slug = 'practical-github-actions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(standalone, 1);
        assert_eq!(membership, 1);
    }
}
