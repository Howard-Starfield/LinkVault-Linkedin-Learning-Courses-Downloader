//! Existing-archive registration and repair workflows.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, Utc};
use image::GenericImageView;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use super::{
    job_repository, models::RepairNewspaperLibraryResult, naming, optimization_service, storage,
};

pub(super) fn repair(db_path: &Path) -> Result<RepairNewspaperLibraryResult, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let legacy_pages = {
        let mut statement = connection
            .prepare(
                "SELECT id, original_path FROM newspaper_pages
                 WHERE status = 'completed' AND optimized_path IS NULL
                   AND LOWER(original_path) LIKE '%.php'",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut renamed_files = 0_u32;
    let mut warnings = Vec::new();
    for (page_id, original_path) in legacy_pages {
        let source = PathBuf::from(&original_path);
        let bytes = match std::fs::read(&source) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("Could not read {}: {error}", source.display()));
                continue;
            }
        };
        let extension = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Jpeg) => "jpg",
            Ok(image::ImageFormat::Png) => "png",
            Ok(image::ImageFormat::WebP) => "webp",
            Ok(_) => "jpg",
            Err(error) => {
                warnings.push(format!("Could not identify {}: {error}", source.display()));
                continue;
            }
        };
        let destination = source.with_extension(extension);
        if destination.exists() && destination != source {
            warnings.push(format!(
                "Could not rename {} because {} already exists.",
                source.display(),
                destination.display()
            ));
            continue;
        }
        if destination != source {
            std::fs::rename(&source, &destination).map_err(|error| error.to_string())?;
        }
        connection
            .execute(
                "UPDATE newspaper_pages SET original_path = ?2, updated_at = ?3 WHERE id = ?1",
                params![
                    page_id,
                    destination.to_string_lossy(),
                    Utc::now().timestamp()
                ],
            )
            .map_err(|error| error.to_string())?;
        renamed_files = renamed_files.saturating_add(1);
    }

    let jobs = {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT j.id
                 FROM newspaper_jobs j
                 JOIN newspaper_batches b ON b.id = j.batch_id
                 JOIN newspaper_pages p ON p.job_id = j.id
                 WHERE j.status IN ('completed', 'partial')
                   AND b.optimize_images = 1
                   AND p.status = 'completed'
                   AND p.optimized_path IS NULL
                   AND p.original_path IS NOT NULL",
            )
            .map_err(|error| error.to_string())?;
        let result = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        result
    };
    let mut optimized_jobs = 0_u32;
    for job_id in jobs {
        let job = job_repository::find(&connection, &job_id)?
            .ok_or_else(|| format!("Newspaper job disappeared during repair: {job_id}"))?;
        connection
            .execute(
                "UPDATE newspaper_jobs SET warning = NULL WHERE id = ?1",
                params![job.id],
            )
            .map_err(|error| error.to_string())?;
        optimization_service::optimize_job(db_path, &job)?;
        storage::finalize_job(&connection, &job.id, Utc::now().timestamp())
            .map_err(|error| error.to_string())?;
        optimized_jobs = optimized_jobs.saturating_add(1);
    }
    let (removed_source_files, cleanup_warnings) = remove_redundant_optimized_sources(&connection)?;
    warnings.extend(cleanup_warnings);
    Ok(RepairNewspaperLibraryResult {
        renamed_files,
        optimized_jobs,
        removed_source_files,
        warnings,
    })
}

fn remove_redundant_optimized_sources(
    connection: &Connection,
) -> Result<(u32, Vec<String>), String> {
    let candidates = connection
        .prepare(
            "SELECT p.original_path, p.optimized_path
             FROM newspaper_pages p
             JOIN newspaper_jobs j ON j.id = p.job_id
             JOIN newspaper_batches b ON b.id = j.batch_id
             WHERE p.status = 'completed'
               AND b.keep_original_jpg = 0
               AND p.original_path IS NOT NULL
               AND p.optimized_path IS NOT NULL
               AND p.original_path <> p.optimized_path",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut removed = 0_u32;
    let mut warnings = Vec::new();
    for (source, optimized) in candidates {
        let source = PathBuf::from(source);
        let optimized = PathBuf::from(optimized);
        if !source.exists() {
            continue;
        }
        let source_reader = match image::io::Reader::open(&source)
            .and_then(|reader| reader.with_guessed_format())
        {
            Ok(reader) => reader,
            Err(error) => {
                warnings.push(format!(
                    "Could not validate source {}: {error}",
                    source.display()
                ));
                continue;
            }
        };
        let optimized_reader = match image::io::Reader::open(&optimized)
            .and_then(|reader| reader.with_guessed_format())
        {
            Ok(reader) => reader,
            Err(error) => {
                warnings.push(format!(
                    "Could not validate optimized image {}: {error}",
                    optimized.display()
                ));
                continue;
            }
        };
        if source_reader.format() != Some(image::ImageFormat::Jpeg)
            || optimized_reader.format() != Some(image::ImageFormat::WebP)
        {
            warnings.push(format!(
                "Kept source {} because the validated pair is not JPEG and WebP.",
                source.display()
            ));
            continue;
        }
        let source_dimensions = image::image_dimensions(&source);
        let optimized_dimensions = image::image_dimensions(&optimized);
        if source_dimensions.is_err()
            || optimized_dimensions.is_err()
            || source_dimensions.as_ref().ok() != optimized_dimensions.as_ref().ok()
        {
            warnings.push(format!(
                "Kept source {} because optimized dimensions could not be matched.",
                source.display()
            ));
            continue;
        }
        match std::fs::remove_file(&source) {
            Ok(()) => removed = removed.saturating_add(1),
            Err(error) => warnings.push(format!(
                "Could not remove redundant source {}: {error}",
                source.display()
            )),
        }
    }
    Ok((removed, warnings))
}

pub struct EditionImportCounts {
    pub imported: usize,
    pub already_known: usize,
    pub skipped: usize,
}

const YOUTUBES_DIRECTORY_NAME: &str = "Youtubes";

pub(super) fn import(db_path: &Path, root: &Path) -> Result<EditionImportCounts, String> {
    if !root.is_dir() {
        return Err("The selected newspaper archive folder does not exist.".to_string());
    }
    if root
        .file_name()
        .is_some_and(|name| is_snapshot_directory_name(&name.to_string_lossy()))
    {
        return Err(
            "The Newspaper snapshots folder cannot be imported as source pages.".to_string(),
        );
    }

    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let known_codes = load_catalog_codes(&connection)?;
    let mut groups: BTreeMap<(String, String, PathBuf), Vec<PathBuf>> = BTreeMap::new();
    let mut skipped = 0;

    let root_entries = std::fs::read_dir(root).map_err(|error| error.to_string())?;
    for entry in root_entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            skipped += 1;
            continue;
        };
        if is_snapshot_directory_name(name) || is_youtubes_directory_name(name) {
            continue;
        }
        match parse_edition_folder_name(name) {
            Some((_, code)) if known_codes.contains(&code) => {
                collect_edition_date_images(&path, &code, &mut groups)?;
            }
            _ => skipped += 1,
        }
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    is_snapshot_directory_name(&name) || is_youtubes_directory_name(&name)
                }) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !is_importable_image(&path) {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some((code, date)) = archive_identity(file_name, path.parent()) else {
                continue;
            };
            groups
                .entry((code, date, path.parent().unwrap_or(root).to_path_buf()))
                .or_default()
                .push(path);
        }
    }

    let now = Utc::now().timestamp();
    let batch_id = naming::unique_id("newspaper-import");
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO newspaper_batches
            (id, status, destination, delay_minutes, optimize_images,
             optimization_profile, keep_original_jpg, created_at, updated_at, completed_at)
            VALUES (?1, 'completed', ?2, 0, 0, 'webp_high', 1, ?3, ?3, ?3)",
            params![batch_id, root.to_string_lossy(), now],
        )
        .map_err(|error| error.to_string())?;
    let mut imported = 0;
    let mut already_known = 0;
    for ((code, date, directory), mut files) in groups {
        if !known_codes.contains(&code) {
            continue;
        }
        files.sort();
        files.dedup();
        let job_id = naming::unique_id("newspaper-import-job");
        let mut valid_pages = Vec::new();
        for file in files {
            let bytes = match std::fs::read(&file) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let image = match image::load_from_memory(&bytes) {
                Ok(image) => image,
                Err(_) => continue,
            };
            let (width, height) = image.dimensions();
            let page_number = archive_page_number(&file);
            valid_pages.push((
                file,
                page_number,
                bytes.len() as u64,
                format!("{:x}", Sha256::digest(&bytes)),
                width,
                height,
            ));
        }
        if valid_pages.is_empty() {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO newspaper_jobs
                (id, batch_id, edition_code, publication_date, status, output_dir,
                 page_count, completed_count, original_bytes, final_bytes,
                 created_at, updated_at, completed_at)
                VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?6, ?7, ?7, ?8, ?8, ?8)
                ON CONFLICT(edition_code, publication_date, output_dir) DO NOTHING",
                params![
                    job_id,
                    batch_id,
                    code,
                    date,
                    directory.to_string_lossy(),
                    valid_pages.len() as i64,
                    valid_pages.iter().map(|item| item.2).sum::<u64>(),
                    now,
                ],
            )
            .map_err(|error| error.to_string())?;
        if transaction.changes() == 0 {
            already_known += 1;
            continue;
        }
        for (file, page_number, bytes, checksum, width, height) in valid_pages {
            transaction
                .execute(
                    "INSERT INTO newspaper_pages
                    (id, job_id, page_number, source_url, original_path, status,
                     attempts, original_bytes, final_bytes, checksum,
                     pixel_width, pixel_height, created_at, updated_at)
                    VALUES (?1, ?2, ?3, 'archive://local', ?4, 'completed', 0,
                            ?5, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        naming::unique_id("newspaper-import-page"),
                        job_id,
                        page_number,
                        file.to_string_lossy(),
                        bytes,
                        checksum,
                        width,
                        height,
                        now,
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        imported += 1;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(EditionImportCounts {
        imported,
        already_known,
        skipped,
    })
}

fn load_catalog_codes(connection: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare("SELECT code FROM newspaper_editions WHERE publication_date = ''")
        .map_err(|error| error.to_string())?;
    let codes = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(codes)
}

fn collect_edition_date_images(
    edition_dir: &Path,
    code: &str,
    groups: &mut BTreeMap<(String, String, PathBuf), Vec<PathBuf>>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(edition_dir).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(date) = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok())
            .map(str::to_string)
        else {
            continue;
        };
        let mut images = Vec::new();
        let files = std::fs::read_dir(&path).map_err(|error| error.to_string())?;
        for file_entry in files {
            let file_entry = file_entry.map_err(|error| error.to_string())?;
            let file_path = file_entry.path();
            if file_path.is_dir() || !is_importable_image(&file_path) {
                continue;
            }
            images.push(file_path);
        }
        if !images.is_empty() {
            groups
                .entry((code.to_string(), date, path))
                .or_default()
                .extend(images);
        }
    }
    Ok(())
}

fn parse_edition_folder_name(name: &str) -> Option<(String, String)> {
    let (left, code) = name.rsplit_once(" - ")?;
    if code.len() == 2 && code.chars().all(|c| c.is_ascii_uppercase()) {
        Some((left.to_string(), code.to_string()))
    } else {
        None
    }
}

fn is_snapshot_directory_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(super::clipping_roots::SNAPSHOT_DIRECTORY_NAME)
}

fn is_youtubes_directory_name(name: &str) -> bool {
    name == YOUTUBES_DIRECTORY_NAME
}

fn is_importable_image(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp")
}

pub(super) fn archive_identity(file_name: &str, parent: Option<&Path>) -> Option<(String, String)> {
    let code = file_name
        .split('_')
        .next()
        .filter(|value| value.len() == 2 && value.chars().all(|ch| ch.is_ascii_uppercase()))?
        .to_string();
    let parent_date = parent
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
        .filter(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok())
        .map(str::to_string);
    let file_date = file_name
        .split('_')
        .nth(1)
        .filter(|value| value.len() >= 8)
        .and_then(|value| NaiveDate::parse_from_str(&value[..8], "%Y%m%d").ok())
        .map(|value| value.to_string());
    Some((code, parent_date.or(file_date)?))
}

fn archive_page_number(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("page");
    let tail = stem.rsplit('_').next().unwrap_or(stem);
    let page = tail
        .char_indices()
        .find(|(_, character)| character.is_ascii_alphabetic())
        .map(|(index, _)| &tail[index..])
        .unwrap_or(tail);
    naming::sanitize_segment(page)
}
