use std::path::Path;

use chrono::{Local, Utc};
use rusqlite::{params, Connection};
use tempfile::tempdir;

use super::{
    archive_service, batch_service, catalog_service, job_service,
    models::*,
    optimization_service,
    optimizer::{optimize_page, OptimizationOutcome},
    page_metadata, queue_service, reader_service, schedule_service, storage,
};

fn request(destination: &Path, date: &str) -> CreateNewspaperBatchRequest {
    CreateNewspaperBatchRequest {
        edition_codes: vec!["NY".to_string()],
        date_mode: DateMode::Single,
        start_date: date.to_string(),
        end_date: None,
        destination: destination.to_string_lossy().into_owned(),
        scheduled_at: None,
        delay_seconds: 15,
        optimize_images: true,
        optimization_profile: "webp_high".to_string(),
        optimization_quality: 92,
        keep_original_jpg: false,
    }
}

#[test]
fn request_validation_rejects_invalid_delay_profile_and_quality() {
    let mut request = CreateNewspaperBatchRequest {
        edition_codes: vec!["NY".to_string()],
        date_mode: DateMode::Single,
        start_date: "2026-07-24".to_string(),
        end_date: None,
        destination: "C:/papers".to_string(),
        scheduled_at: None,
        delay_seconds: 3_601,
        optimize_images: true,
        optimization_profile: "webp_high".to_string(),
        optimization_quality: 92,
        keep_original_jpg: false,
    };
    assert!(batch_service::validate_request(&request).is_err());
    request.delay_seconds = 15;
    request.optimization_profile = "lossless".to_string();
    assert!(batch_service::validate_request(&request).is_err());
    request.optimization_profile = "webp_high".to_string();
    request.optimization_quality = 25;
    assert!(batch_service::validate_request(&request).is_ok());
    request.optimization_quality = 24;
    assert!(batch_service::validate_request(&request).is_err());
}

#[test]
fn one_failed_page_does_not_retain_successfully_converted_sources() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let mut batch_request = request(&destination, "2026-07-24");
    batch_request.optimization_quality = 25;
    let job = batch_service::create_with_connection(&mut connection, batch_request)
        .unwrap()
        .jobs
        .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();

    let valid_path = Path::new(&job.output_dir).join("A01.jpg");
    let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
        let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
        image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
    });
    image
        .save_with_format(&valid_path, image::ImageFormat::Jpeg)
        .unwrap();
    let invalid_path = Path::new(&job.output_dir).join("A02.jpg");
    std::fs::write(&invalid_path, b"not an image").unwrap();

    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', page_count = 2, completed_count = 2
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    for (id, page, path) in [
        ("valid-page", "A01", &valid_path),
        ("invalid-page", "A02", &invalid_path),
    ] {
        let bytes = std::fs::metadata(path).unwrap().len();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'test://page', ?4, 'completed',
                         ?5, ?5, 'checksum', 1, 1)",
                params![id, job.id, page, path.to_string_lossy(), bytes],
            )
            .unwrap();
    }
    drop(connection);

    optimization_service::optimize_job(&db_path, &job).unwrap();

    assert!(valid_path.with_extension("webp").exists());
    assert!(
        !valid_path.exists(),
        "a warning on another page must not retain a successfully converted source"
    );
    assert!(invalid_path.exists());
    let connection = Connection::open(&db_path).unwrap();
    let warning: Option<String> = connection
        .query_row(
            "SELECT warning FROM newspaper_jobs WHERE id = ?1",
            params![job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(warning.is_some());
}

#[test]
fn list_catalog_reads_seeded_regular_editions() {
    let directory = tempdir().unwrap();
    let (connection, _) =
        crate::cache::initialize_database(&directory.path().join("test.db")).unwrap();
    let catalog = catalog_service::list_with_connection(&connection).unwrap();
    assert_eq!(catalog.len(), 13);
    assert_eq!(
        catalog
            .iter()
            .filter(|item| item.kind == EditionKind::Daily)
            .count(),
        10
    );
}

#[test]
fn archive_identity_uses_parent_date_or_filename_date() {
    assert_eq!(
        archive_service::archive_identity(
            "NY_20260724_NY20260724A01.jpg",
            Some(Path::new("C:/archive/2026-07-24"))
        ),
        Some(("NY".to_string(), "2026-07-24".to_string()))
    );
    assert_eq!(
        archive_service::archive_identity(
            "LA_20260723_LA20260723A01.jpg",
            Some(Path::new("C:/archive"))
        ),
        Some(("LA".to_string(), "2026-07-23".to_string()))
    );
}

#[test]
fn duplicate_batch_request_skips_existing_job_instead_of_failing() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");

    let first =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap();
    let second =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap();

    assert_eq!(first.jobs.len(), 1);
    assert!(second.jobs.is_empty());
    assert_eq!(second.skipped_count, 1);
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM newspaper_jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn queue_controls_are_persisted_and_removal_deletes_downloaded_files_and_identity() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let first =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-21"))
            .unwrap()
            .jobs
            .remove(0);
    let second =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-22"))
            .unwrap()
            .jobs
            .remove(0);
    let third =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-23"))
            .unwrap()
            .jobs
            .remove(0);

    let reordered = vec![third.id.clone(), first.id.clone(), second.id.clone()];
    job_service::reorder(&mut connection, &reordered, 100).unwrap();
    let persisted_order = connection
        .prepare("SELECT id FROM newspaper_jobs ORDER BY queue_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(persisted_order, reordered);

    assert_eq!(
        job_service::set_pause(&connection, &first.id, true, 101).unwrap(),
        "queued"
    );
    let paused: bool = connection
        .query_row(
            "SELECT paused FROM newspaper_jobs WHERE id = ?1",
            params![first.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(paused);

    std::fs::create_dir_all(&second.output_dir).unwrap();
    std::fs::write(Path::new(&second.output_dir).join(".complete"), b"").unwrap();
    let page_path = Path::new(&second.output_dir).join("A01.webp");
    std::fs::write(&page_path, b"downloaded page").unwrap();
    let thumbnail_path = directory
        .path()
        .join("newspaper-thumbnails")
        .join("v1")
        .join("second.webp");
    std::fs::create_dir_all(thumbnail_path.parent().unwrap()).unwrap();
    std::fs::write(&thumbnail_path, b"thumbnail").unwrap();
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'completed' WHERE id = ?1",
            params![second.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_pages
             (id, job_id, page_number, source_url, optimized_path, status,
              media_version, created_at, updated_at)
             VALUES ('second-page', ?1, 'A01', 'test://page', ?2, 'completed', 1, 1, 1)",
            params![second.id, page_path.to_string_lossy()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_thumbnail_cache
             (job_id, source_page_id, source_media_version, cache_schema_version,
              cache_path, mime_type, pixel_width, pixel_height, byte_count, updated_at)
             VALUES (?1, 'second-page', 1, 1, ?2, 'image/webp', 420, 176, 9, 1)",
            params![second.id, thumbnail_path.to_string_lossy()],
        )
        .unwrap();

    let (_, previous_status) =
        job_service::delete_with_connection(&mut connection, &second.id).unwrap();
    assert_eq!(previous_status, "completed");
    assert!(
        !Path::new(&second.output_dir).exists(),
        "trash must remove the exact downloaded edition directory"
    );
    assert!(
        !thumbnail_path.exists(),
        "trash must remove the edition's generated thumbnail cache"
    );
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_jobs WHERE id = ?1",
            params![second.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "trash must release the duplicate identity");
}

#[test]
fn last_seven_days_batch_creates_all_seven_daily_jobs() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let mut batch_request = request(&destination, "2000-01-01");
    batch_request.date_mode = DateMode::Last7Days;

    let response = batch_service::create_with_connection(&mut connection, batch_request).unwrap();
    let today = Local::now().date_naive();
    let expected_start = (today - chrono::Duration::days(6)).to_string();
    let expected_end = today.to_string();
    let dates = response
        .jobs
        .iter()
        .map(|job| job.publication_date.clone())
        .collect::<Vec<_>>();

    assert_eq!(response.jobs.len(), 7);
    assert_eq!(dates.first(), Some(&expected_start));
    assert_eq!(dates.last(), Some(&expected_end));
    assert_eq!(response.skipped_count, 0);
}

#[test]
fn a_new_batch_restores_a_legacy_dismissed_completed_edition() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let job =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();
    std::fs::write(Path::new(&job.output_dir).join(".complete"), b"").unwrap();
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', dismissed = 1
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();

    let response =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap();

    assert!(response.jobs.is_empty());
    assert_eq!(response.skipped_count, 1);
    let restored: (String, bool) = connection
        .query_row(
            "SELECT status, dismissed FROM newspaper_jobs WHERE id = ?1",
            params![job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(restored, ("completed".to_string(), false));
}

#[test]
fn removal_refuses_an_active_download() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let job =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
    connection
        .execute(
            "UPDATE newspaper_jobs SET status = 'active' WHERE id = ?1",
            params![job.id],
        )
        .unwrap();

    let result = job_service::delete_with_connection(&mut connection, &job.id);

    assert!(result.is_err());
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM newspaper_jobs WHERE id = ?1",
            params![job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 1);
}

#[test]
fn removal_refuses_a_directory_outside_the_batch_destination() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let job =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
    let outside = directory.path().join("must-not-delete");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
    connection
        .execute(
            "UPDATE newspaper_jobs SET output_dir = ?2 WHERE id = ?1",
            params![job.id, outside.to_string_lossy()],
        )
        .unwrap();

    let result = job_service::delete_with_connection(&mut connection, &job.id);

    assert!(result.is_err());
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn release_retry_is_persisted_for_thirty_minutes() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let today = Local::now().date_naive().to_string();
    let mut job = batch_service::create_with_connection(
        &mut connection,
        request(&directory.path().join("papers"), &today),
    )
    .unwrap()
    .jobs
    .remove(0);
    let before = Utc::now().timestamp();

    queue_service::schedule_release_retry(&db_path, &mut job, "Not released.").unwrap();

    let persisted: (String, i64, i64) = connection
        .query_row(
            "SELECT status, retry_at, retry_count FROM newspaper_jobs WHERE id = ?1",
            params![job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(persisted.0, "queued");
    assert!((before + 1_800..=before + 1_802).contains(&persisted.1));
    assert_eq!(persisted.2, 1);
    assert_eq!(job.status, "awaiting_release");
}

#[test]
fn reading_progress_resumes_last_page_without_regressing_furthest_page() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let job = batch_service::create_with_connection(
        &mut connection,
        request(&directory.path().join("papers"), "2026-07-24"),
    )
    .unwrap()
    .jobs
    .remove(0);
    for index in 0..3 {
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'test://page', 'completed', 1, 1)",
                params![
                    format!("reading-page-{index}"),
                    job.id,
                    format!("A0{}", index + 1)
                ],
            )
            .unwrap();
    }

    let forward =
        reader_service::save_progress(&connection, &job.id, "reading-page-2", 10).unwrap();
    assert_eq!(forward.last_page_index, 2);
    assert_eq!(forward.furthest_page_index, 2);
    assert_eq!(forward.read_page_count, 1);

    let backward =
        reader_service::save_progress(&connection, &job.id, "reading-page-0", 11).unwrap();
    assert_eq!(backward.last_page_id, "reading-page-0");
    assert_eq!(backward.last_page_index, 0);
    assert_eq!(backward.furthest_page_index, 2);
    assert_eq!(backward.read_page_count, 2);
    let repeated =
        reader_service::save_progress(&connection, &job.id, "reading-page-0", 12).unwrap();
    assert_eq!(repeated.read_page_count, 2);
    let completed_coverage =
        reader_service::save_progress(&connection, &job.id, "reading-page-1", 13).unwrap();
    assert_eq!(completed_coverage.furthest_page_index, 2);
    assert_eq!(completed_coverage.read_page_count, 3);
    assert_eq!(
        reader_service::list_progress(&connection).unwrap(),
        vec![completed_coverage]
    );

    connection
        .execute(
            "DELETE FROM newspaper_read_pages WHERE job_id = ?1",
            params![job.id],
        )
        .unwrap();
    storage::initialize(&connection).unwrap();
    let migrated = reader_service::list_progress(&connection).unwrap();
    assert_eq!(migrated[0].read_page_count, 1);
    assert_eq!(migrated[0].last_page_id, "reading-page-1");
}

#[test]
fn reader_manifest_stays_non_blocking_while_background_backfill_enriches_dimensions() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let job = batch_service::create_with_connection(
        &mut connection,
        request(&directory.path().join("papers"), "2026-07-24"),
    )
    .unwrap()
    .jobs
    .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();
    let page_path = Path::new(&job.output_dir).join("A01.jpg");
    image::RgbImage::from_pixel(320, 480, image::Rgb([40, 80, 120]))
        .save_with_format(&page_path, image::ImageFormat::Jpeg)
        .unwrap();
    let bytes = std::fs::metadata(&page_path).unwrap().len();
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', page_count = 1, completed_count = 1
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_pages
             (id, job_id, page_number, source_url, original_path, status,
              original_bytes, final_bytes, checksum, created_at, updated_at)
             VALUES ('legacy-reader-page', ?1, 'A01', 'test://page', ?2,
                     'completed', ?3, ?3, 'checksum', 1, 1)",
            params![job.id, page_path.to_string_lossy(), bytes],
        )
        .unwrap();
    drop(connection);

    let manifest = reader_service::manifest(&db_path, &job.id).unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(manifest[0].pixel_width, None);
    assert_eq!(manifest[0].pixel_height, None);

    let connection = Connection::open(&db_path).unwrap();
    let before: (Option<u32>, Option<u32>) = connection
        .query_row(
            "SELECT pixel_width, pixel_height FROM newspaper_pages
             WHERE id = 'legacy-reader-page'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(before, (None, None));
    drop(connection);

    let candidates = page_metadata::missing_candidates(&db_path).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(page_metadata::backfill(&db_path, &candidates).unwrap(), 1);
    let connection = Connection::open(&db_path).unwrap();
    let after: (Option<u32>, Option<u32>) = connection
        .query_row(
            "SELECT pixel_width, pixel_height FROM newspaper_pages
             WHERE id = 'legacy-reader-page'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, (Some(320), Some(480)));
}

#[test]
fn progress_rollup_updates_before_job_finalization() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let job = batch_service::create_with_connection(
        &mut connection,
        request(&directory.path().join("papers"), "2026-07-24"),
    )
    .unwrap()
    .jobs
    .remove(0);
    connection
        .execute(
            "UPDATE newspaper_jobs SET page_count = 3 WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    for (index, status) in ["completed", "completed", "failed"].iter().enumerate() {
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, status, original_bytes,
                  final_bytes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'test://page', ?4, 100, 80, 1, 1)",
                params![
                    format!("page-{index}"),
                    job.id,
                    format!("A0{}", index + 1),
                    status
                ],
            )
            .unwrap();
    }

    queue_service::refresh_job_progress(&connection, &job.id).unwrap();

    let progress: (i64, i64, i64, i64) = connection
        .query_row(
            "SELECT completed_count, failed_count, original_bytes, final_bytes
             FROM newspaper_jobs WHERE id = ?1",
            params![job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(progress, (2, 1, 300, 240));
}

#[test]
fn due_daily_schedule_materializes_only_once_per_local_date() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_schedules
             (id, enabled, cron_time, destination, edition_codes_json, delay_seconds,
              optimize_images, optimization_profile, keep_original_jpg, created_at, updated_at)
             VALUES ('schedule-1', 1, '00:00', ?1, '[\"NY\"]', 15, 1,
                     'webp_high', 0, 1, 1)",
            params![directory.path().join("papers").to_string_lossy()],
        )
        .unwrap();
    drop(connection);

    schedule_service::materialize_due(&db_path).unwrap();
    schedule_service::materialize_due(&db_path).unwrap();

    let connection = Connection::open(&db_path).unwrap();
    let job_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM newspaper_jobs", [], |row| row.get(0))
        .unwrap();
    let last_run: String = connection
        .query_row(
            "SELECT last_run_date FROM newspaper_schedules WHERE id = 'schedule-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(job_count, 1);
    assert_eq!(last_run, Local::now().date_naive().to_string());
}

#[test]
fn repair_renames_legacy_php_image_and_runs_optimization() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let job =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();
    let legacy_path = Path::new(&job.output_dir).join("A01.php");
    let image = image::ImageBuffer::from_fn(320, 480, |x, y| {
        image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
    });
    image
        .save_with_format(&legacy_path, image::ImageFormat::Jpeg)
        .unwrap();
    let bytes = std::fs::metadata(&legacy_path).unwrap().len();
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', page_count = 1, completed_count = 1
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_pages
             (id, job_id, page_number, source_url, original_path, status,
              original_bytes, final_bytes, checksum, created_at, updated_at)
             VALUES ('legacy-page', ?1, 'A01', 'test://page', ?2, 'completed',
                     ?3, ?3, 'checksum', 1, 1)",
            params![job.id, legacy_path.to_string_lossy(), bytes],
        )
        .unwrap();
    drop(connection);

    let result = archive_service::repair(&db_path).unwrap();

    assert_eq!(result.renamed_files, 1);
    assert_eq!(result.optimized_jobs, 1);
    assert!(!legacy_path.exists());
    let connection = Connection::open(&db_path).unwrap();
    let paths: (String, Option<String>) = connection
        .query_row(
            "SELECT original_path, optimized_path FROM newspaper_pages
             WHERE id = 'legacy-page'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(paths.0.ends_with(".jpg"));
    assert!(
        Path::new(paths.1.as_deref().unwrap_or(&paths.0)).exists(),
        "the repaired display image should exist"
    );
}

#[test]
fn repair_removes_only_valid_redundant_jpeg_sources() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let mut batch_request = request(&destination, "2026-07-24");
    batch_request.optimization_quality = 25;
    let job = batch_service::create_with_connection(&mut connection, batch_request)
        .unwrap()
        .jobs
        .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();

    let redundant_source = Path::new(&job.output_dir).join("A01.jpg");
    let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
        let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
        image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
    });
    image
        .save_with_format(&redundant_source, image::ImageFormat::Jpeg)
        .unwrap();
    let redundant_webp = match optimize_page(&redundant_source, 25).unwrap() {
        OptimizationOutcome::Replaced { path, .. } => path,
        OptimizationOutcome::KeptOriginal { .. } => {
            panic!("test image should be smaller as WebP")
        }
    };
    let fallback_source = Path::new(&job.output_dir).join("A02.jpg");
    image
        .save_with_format(&fallback_source, image::ImageFormat::Jpeg)
        .unwrap();
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', page_count = 2, completed_count = 2
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    for (id, page, source, optimized) in [
        (
            "redundant-source-page",
            "A01",
            &redundant_source,
            &redundant_webp,
        ),
        (
            "fallback-source-page",
            "A02",
            &fallback_source,
            &fallback_source,
        ),
    ] {
        let original_bytes = std::fs::metadata(source).unwrap().len();
        let final_bytes = std::fs::metadata(optimized).unwrap().len();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path,
                  optimized_path, status, original_bytes, final_bytes,
                  checksum, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'test://page', ?4, ?5, 'completed',
                         ?6, ?7, 'checksum', 1, 1)",
                params![
                    id,
                    job.id,
                    page,
                    source.to_string_lossy(),
                    optimized.to_string_lossy(),
                    original_bytes,
                    final_bytes
                ],
            )
            .unwrap();
    }
    drop(connection);

    let result = archive_service::repair(&db_path).unwrap();

    assert_eq!(result.removed_source_files, 1);
    assert!(!redundant_source.exists());
    assert!(redundant_webp.exists());
    assert!(
        fallback_source.exists(),
        "an active JPG fallback must never be removed"
    );
}

#[tokio::test]
async fn optimization_queue_runs_after_download_completion_and_is_resumable() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("test.db");
    let (mut connection, _) = crate::cache::initialize_database(&db_path).unwrap();
    let destination = directory.path().join("papers");
    let job =
        batch_service::create_with_connection(&mut connection, request(&destination, "2026-07-24"))
            .unwrap()
            .jobs
            .remove(0);
    std::fs::create_dir_all(&job.output_dir).unwrap();
    let original_path = Path::new(&job.output_dir).join("A01.jpg");
    let image = image::ImageBuffer::from_fn(480, 640, |x, y| {
        let value = ((x.wrapping_mul(31) + y.wrapping_mul(17)) % 255) as u8;
        image::Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
    });
    image
        .save_with_format(&original_path, image::ImageFormat::Jpeg)
        .unwrap();
    let bytes = std::fs::metadata(&original_path).unwrap().len();
    connection
        .execute(
            "UPDATE newspaper_jobs
             SET status = 'completed', page_count = 1, completed_count = 1
             WHERE id = ?1",
            params![job.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO newspaper_pages
             (id, job_id, page_number, source_url, original_path, status,
              original_bytes, final_bytes, checksum, created_at, updated_at)
             VALUES ('queue-page', ?1, 'A01', 'test://page', ?2, 'completed',
                     ?3, ?3, 'checksum', 1, 1)",
            params![job.id, original_path.to_string_lossy(), bytes],
        )
        .unwrap();
    drop(connection);

    let first = optimization_service::process_queue(&db_path).await.unwrap();
    let second = optimization_service::process_queue(&db_path).await.unwrap();

    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    let connection = Connection::open(&db_path).unwrap();
    let display_path: String = connection
        .query_row(
            "SELECT optimized_path FROM newspaper_pages WHERE id = 'queue-page'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(Path::new(&display_path).exists());
}
