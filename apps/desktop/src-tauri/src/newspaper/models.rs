use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditionKind {
    Daily,
    Weekly,
    Special,
}

impl EditionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Special => "special",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationSchedule {
    Daily,
    WeeklySunday,
    AdHoc,
}

impl PublicationSchedule {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::WeeklySunday => "weekly_sunday",
            Self::AdHoc => "ad_hoc",
        }
    }

    pub fn accepts(self, date: NaiveDate) -> bool {
        use chrono::{Datelike, Weekday};

        match self {
            Self::Daily | Self::AdHoc => true,
            Self::WeeklySunday => date.weekday() == Weekday::Sun,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperEdition {
    pub code: String,
    pub name_zh: String,
    pub name_en: String,
    pub kind: EditionKind,
    pub schedule: PublicationSchedule,
    pub source_url: String,
    pub publication_date: Option<NaiveDate>,
    pub discovered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateMode {
    Single,
    #[serde(alias = "last_7_days")]
    Last7Days,
    Custom,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DateExpansionError {
    #[error("end date is required for a custom range")]
    MissingEndDate,
    #[error("end date must not precede start date")]
    ReversedRange,
    #[error("custom date range must not exceed 31 days")]
    RangeTooLong,
}

pub fn expand_dates(
    mode: DateMode,
    start: NaiveDate,
    end: Option<NaiveDate>,
) -> Result<Vec<NaiveDate>, DateExpansionError> {
    let end = match mode {
        DateMode::Single | DateMode::Last7Days => start,
        DateMode::Custom => end.ok_or(DateExpansionError::MissingEndDate)?,
    };
    let start = match mode {
        DateMode::Last7Days => start - chrono::Duration::days(6),
        _ => start,
    };

    if end < start {
        return Err(DateExpansionError::ReversedRange);
    }
    let day_count = (end - start).num_days() + 1;
    if matches!(mode, DateMode::Custom) && day_count > 31 {
        return Err(DateExpansionError::RangeTooLong);
    }

    Ok((0..day_count)
        .map(|offset| start + chrono::Duration::days(offset))
        .collect())
}

pub const fn default_optimization_quality() -> u8 {
    86
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperBatchRequest {
    pub edition_codes: Vec<String>,
    pub date_mode: DateMode,
    pub start_date: String,
    pub end_date: Option<String>,
    pub destination: String,
    pub scheduled_at: Option<i64>,
    pub delay_seconds: u32,
    pub optimize_images: bool,
    pub optimization_profile: String,
    #[serde(default = "default_optimization_quality")]
    pub optimization_quality: u8,
    pub keep_original_jpg: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewspaperBatch {
    pub id: String,
    pub status: String,
    pub destination: String,
    pub scheduled_at: Option<i64>,
    pub delay_seconds: u32,
    pub optimize_images: bool,
    pub optimization_profile: String,
    pub optimization_quality: u8,
    pub keep_original_jpg: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewspaperJob {
    pub id: String,
    pub batch_id: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub status: String,
    pub output_dir: String,
    pub page_count: u32,
    pub completed_count: u32,
    pub failed_count: u32,
    pub retry_at: Option<i64>,
    pub retry_count: u32,
    pub warning: Option<String>,
    pub queue_position: i64,
    pub paused: bool,
    pub dismissed: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewspaperSchedule {
    pub id: String,
    pub enabled: bool,
    pub cron_time: String,
    pub destination: String,
    pub edition_codes: Vec<String>,
    pub delay_seconds: u32,
    pub optimize_images: bool,
    pub optimization_profile: String,
    pub optimization_quality: u8,
    pub keep_original_jpg: bool,
    pub last_run_date: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperScheduleRequest {
    pub cron_time: String,
    pub destination: String,
    pub edition_codes: Vec<String>,
    pub delay_seconds: u32,
    pub optimize_images: bool,
    pub optimization_profile: String,
    #[serde(default = "default_optimization_quality")]
    pub optimization_quality: u8,
    pub keep_original_jpg: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperPage {
    pub id: String,
    pub job_id: String,
    pub canonical_index: u32,
    pub page_number: String,
    pub section_name: Option<String>,
    pub source_url: String,
    pub display_path: Option<String>,
    pub status: String,
    pub media_url: Option<String>,
    pub media_version: i64,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub final_bytes: Option<u64>,
    pub checksum: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperReadingProgress {
    pub job_id: String,
    pub last_page_id: String,
    pub last_page_index: u32,
    pub furthest_page_index: u32,
    pub read_page_count: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperLibraryItem {
    pub job_id: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub status: String,
    pub output_dir: String,
    pub page_count: u32,
    pub completed_count: u32,
    pub warning: Option<String>,
    pub updated_at: i64,
    pub thumbnail_ready: bool,
    pub thumbnail_url: Option<String>,
    pub thumbnail_version: Option<String>,
    pub last_page_id: Option<String>,
    pub last_page_index: Option<u32>,
    pub furthest_page_index: Option<u32>,
    pub read_page_count: u32,
    pub reading_updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperLibraryPage {
    pub items: Vec<NewspaperLibraryItem>,
    pub total: u32,
    pub offset: u32,
    pub limit: u32,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperActivitySnapshot {
    pub jobs: Vec<NewspaperJob>,
    pub batches: Vec<NewspaperBatch>,
    pub schedules: Vec<NewspaperSchedule>,
    pub has_live_activity: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperBootstrap {
    pub catalog: Vec<NewspaperEdition>,
    pub batches: Vec<NewspaperBatch>,
    pub jobs: Vec<NewspaperJob>,
    pub schedules: Vec<NewspaperSchedule>,
    pub reading_progress: Vec<NewspaperReadingProgress>,
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperBatchResponse {
    pub batch: NewspaperBatch,
    pub jobs: Vec<NewspaperJob>,
    pub skipped_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairNewspaperLibraryResult {
    pub renamed_files: u32,
    pub optimized_jobs: u32,
    pub removed_source_files: u32,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn last_seven_days_are_oldest_first() {
        let dates = expand_dates(DateMode::Last7Days, date("2026-07-24"), None).unwrap();
        assert_eq!(dates.first(), Some(&date("2026-07-18")));
        assert_eq!(dates.last(), Some(&date("2026-07-24")));
    }

    #[test]
    fn legacy_last_seven_days_spelling_deserializes() {
        assert_eq!(
            serde_json::from_str::<DateMode>(r#""last_7_days""#).unwrap(),
            DateMode::Last7Days
        );
    }

    #[test]
    fn custom_range_rejects_more_than_31_inclusive_days() {
        assert_eq!(
            expand_dates(
                DateMode::Custom,
                date("2026-01-01"),
                Some(date("2026-02-01"))
            ),
            Err(DateExpansionError::RangeTooLong)
        );
    }

    #[test]
    fn sunday_schedule_accepts_only_sunday() {
        assert!(PublicationSchedule::WeeklySunday.accepts(date("2026-07-19")));
        assert!(!PublicationSchedule::WeeklySunday.accepts(date("2026-07-20")));
    }
}
