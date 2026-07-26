//! Catalog discovery, persistence, and edition row mapping.

use std::path::Path;

use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection};

use super::models::{EditionKind, NewspaperEdition, PublicationSchedule};

pub(super) fn list(db_path: &Path) -> Result<Vec<NewspaperEdition>, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    list_with_connection(&connection)
}

pub(super) async fn refresh(db_path: &Path) -> Result<Vec<NewspaperEdition>, String> {
    let html = reqwest::Client::new()
        .get("https://ep.worldjournal.com/")
        .header(
            reqwest::header::USER_AGENT,
            super::client::CHROME_USER_AGENT,
        )
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .text()
        .await
        .map_err(|error| error.to_string())?;
    let discovered = super::catalog::discover_specials(&html);
    let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    for edition in discovered {
        let publication_date = edition
            .publication_date
            .map(|value| value.to_string())
            .unwrap_or_default();
        transaction
            .execute(
                "INSERT INTO newspaper_editions
                (code, publication_date, name_zh, name_en, kind, schedule, source_url,
                 active, discovered, discovered_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, 'special', 'ad_hoc', ?5, 1, 1, ?6, ?6)
                ON CONFLICT(code, publication_date) DO UPDATE SET
                    name_zh = excluded.name_zh, source_url = excluded.source_url,
                    active = 1, discovered = 1, discovered_at = excluded.discovered_at,
                    updated_at = excluded.updated_at",
                params![
                    edition.code,
                    publication_date,
                    edition.name_zh,
                    edition.name_en,
                    edition.source_url,
                    Utc::now().timestamp()
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    list_with_connection(&connection)
}

pub(super) fn list_with_connection(
    connection: &Connection,
) -> Result<Vec<NewspaperEdition>, String> {
    let mut statement = connection
        .prepare(
            "SELECT code, name_zh, name_en, kind, schedule, source_url,
                NULLIF(publication_date, ''), discovered
         FROM newspaper_editions WHERE active = 1
         ORDER BY CASE kind WHEN 'daily' THEN 0 WHEN 'weekly' THEN 1 ELSE 2 END,
                  code, publication_date DESC",
        )
        .map_err(|error| error.to_string())?;
    let result = statement
        .query_map([], |row| {
            let kind: String = row.get(3)?;
            let schedule: String = row.get(4)?;
            let publication_date: Option<String> = row.get(6)?;
            Ok(NewspaperEdition {
                code: row.get(0)?,
                name_zh: row.get(1)?,
                name_en: row.get(2)?,
                kind: match kind.as_str() {
                    "weekly" => EditionKind::Weekly,
                    "special" => EditionKind::Special,
                    _ => EditionKind::Daily,
                },
                schedule: match schedule.as_str() {
                    "weekly_sunday" => PublicationSchedule::WeeklySunday,
                    "ad_hoc" => PublicationSchedule::AdHoc,
                    _ => PublicationSchedule::Daily,
                },
                source_url: row.get(5)?,
                publication_date: publication_date
                    .and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok()),
                discovered: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string());
    result
}
