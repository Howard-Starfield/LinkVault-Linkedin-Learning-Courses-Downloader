use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use chrono::Utc;
use image::GenericImageView;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Mutex, Semaphore};

const THUMBNAIL_SCHEMA_VERSION: i64 = 1;
const THUMBNAIL_WIDTH: u32 = 420;
const THUMBNAIL_HEIGHT: u32 = 176;
const MAX_IN_FLIGHT: usize = 14;
const MAX_ACTIVE: usize = 2;
const RETRY_AFTER_MS: u32 = 500;

type SharedThumbnailResult = Result<ThumbnailDescriptor, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThumbnailSource {
    job_id: String,
    page_id: String,
    path: PathBuf,
    media_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailDescriptor {
    pub url: String,
    pub version: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum EnsureThumbnailResult {
    #[serde(rename = "ready")]
    Ready {
        thumbnail_url: String,
        thumbnail_version: String,
        width: u32,
        height: u32,
    },
    #[serde(rename = "generated")]
    Generated {
        thumbnail_url: String,
        thumbnail_version: String,
        width: u32,
        height: u32,
    },
    #[serde(rename = "busy")]
    Busy { retry_after_ms: u32 },
}

impl EnsureThumbnailResult {
    fn ready(descriptor: ThumbnailDescriptor) -> Self {
        Self::Ready {
            thumbnail_url: descriptor.url,
            thumbnail_version: descriptor.version,
            width: descriptor.width,
            height: descriptor.height,
        }
    }

    fn generated(descriptor: ThumbnailDescriptor) -> Self {
        Self::Generated {
            thumbnail_url: descriptor.url,
            thumbnail_version: descriptor.version,
            width: descriptor.width,
            height: descriptor.height,
        }
    }
}

pub struct ThumbnailCoordinator {
    db_path: PathBuf,
    cache_root: PathBuf,
    permits: Arc<Semaphore>,
    in_flight: Mutex<HashMap<String, watch::Receiver<Option<SharedThumbnailResult>>>>,
    active: AtomicUsize,
    generation_starts: AtomicUsize,
}

impl ThumbnailCoordinator {
    pub fn new(db_path: PathBuf) -> Self {
        let cache_root = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("newspaper-thumbnails")
            .join(format!("v{THUMBNAIL_SCHEMA_VERSION}"));
        Self {
            db_path,
            cache_root,
            permits: Arc::new(Semaphore::new(MAX_ACTIVE)),
            in_flight: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            generation_starts: AtomicUsize::new(0),
        }
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    pub async fn pending_count(&self) -> usize {
        self.in_flight
            .lock()
            .await
            .len()
            .saturating_sub(self.active_count())
    }

    pub fn generation_start_count(&self) -> usize {
        self.generation_starts.load(Ordering::SeqCst)
    }

    pub async fn cached_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<ThumbnailDescriptor>, String> {
        let db_path = self.db_path.clone();
        let cache_root = self.cache_root.clone();
        let job_id = job_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            let source = source_for_job(&connection, &job_id)?;
            source
                .map(|source| valid_cached_thumbnail(&connection, &cache_root, &source))
                .transpose()
                .map(Option::flatten)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub async fn ensure(&self, job_id: String) -> Result<EnsureThumbnailResult, String> {
        let db_path = self.db_path.clone();
        let cache_root = self.cache_root.clone();
        let lookup_job_id = job_id.clone();
        let (source, cached) = tauri::async_runtime::spawn_blocking(move || {
            let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
            let source = source_for_job(&connection, &lookup_job_id)?
                .ok_or_else(|| "JOB_NOT_READABLE".to_string())?;
            let cached = valid_cached_thumbnail(&connection, &cache_root, &source)?;
            Ok::<_, String>((source, cached))
        })
        .await
        .map_err(|error| error.to_string())??;

        if let Some(descriptor) = cached {
            return Ok(EnsureThumbnailResult::ready(descriptor));
        }

        let key = format!(
            "{}:{}:{}",
            source.job_id, source.page_id, source.media_version
        );
        enum Reservation {
            Leader(watch::Sender<Option<SharedThumbnailResult>>),
            Follower(watch::Receiver<Option<SharedThumbnailResult>>),
            Busy,
        }
        let reservation = {
            let mut in_flight = self.in_flight.lock().await;
            if let Some(receiver) = in_flight.get(&key) {
                Reservation::Follower(receiver.clone())
            } else if in_flight.len() >= MAX_IN_FLIGHT {
                Reservation::Busy
            } else {
                let (sender, receiver) = watch::channel(None);
                in_flight.insert(key.clone(), receiver);
                Reservation::Leader(sender)
            }
        };

        match reservation {
            Reservation::Busy => Ok(EnsureThumbnailResult::Busy {
                retry_after_ms: RETRY_AFTER_MS,
            }),
            Reservation::Follower(mut receiver) => {
                if receiver.borrow().is_none() {
                    receiver
                        .changed()
                        .await
                        .map_err(|_| "THUMBNAIL_GENERATION_CANCELLED".to_string())?;
                }
                let result = receiver
                    .borrow()
                    .clone()
                    .ok_or_else(|| "THUMBNAIL_GENERATION_CANCELLED".to_string())??;
                Ok(EnsureThumbnailResult::ready(result))
            }
            Reservation::Leader(sender) => {
                let permit = self
                    .permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| "THUMBNAIL_GENERATION_CANCELLED".to_string())?;
                self.active.fetch_add(1, Ordering::SeqCst);
                self.generation_starts.fetch_add(1, Ordering::SeqCst);
                let db_path = self.db_path.clone();
                let cache_root = self.cache_root.clone();
                let generated = tauri::async_runtime::spawn_blocking(move || {
                    let _permit = permit;
                    generate_thumbnail(&db_path, &cache_root, &source)
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
                self.active.fetch_sub(1, Ordering::SeqCst);
                let _ = sender.send(Some(generated.clone()));
                self.in_flight.lock().await.remove(&key);
                generated.map(EnsureThumbnailResult::generated)
            }
        }
    }
}

fn source_for_job(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<ThumbnailSource>, String> {
    connection
        .query_row(
            "SELECT id, COALESCE(optimized_path, original_path), media_version
             FROM newspaper_pages
             WHERE job_id = ?1 AND status = 'completed'
               AND COALESCE(optimized_path, original_path) IS NOT NULL
             ORDER BY CASE WHEN page_number = 'A01' THEN 0 ELSE 1 END, page_number
             LIMIT 1",
            params![job_id],
            |row| {
                Ok(ThumbnailSource {
                    job_id: job_id.to_string(),
                    page_id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    media_version: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn valid_cached_thumbnail(
    connection: &Connection,
    cache_root: &Path,
    source: &ThumbnailSource,
) -> Result<Option<ThumbnailDescriptor>, String> {
    let record = connection
        .query_row(
            "SELECT cache_path, source_page_id, source_media_version,
                    cache_schema_version, mime_type, pixel_width, pixel_height, byte_count
             FROM newspaper_thumbnail_cache WHERE job_id = ?1",
            params![source.job_id],
            |row| {
                Ok((
                    PathBuf::from(row.get::<_, String>(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, u64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((path, page_id, media_version, schema_version, mime_type, width, height, bytes)) =
        record
    else {
        return Ok(None);
    };
    if page_id != source.page_id
        || media_version != source.media_version
        || schema_version != THUMBNAIL_SCHEMA_VERSION
        || mime_type != "image/webp"
        || width != THUMBNAIL_WIDTH
        || height != THUMBNAIL_HEIGHT
    {
        return Ok(None);
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() != bytes {
        return Ok(None);
    }
    let canonical_root = match cache_root.canonicalize() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    let canonical_path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !canonical_path.starts_with(canonical_root) {
        return Ok(None);
    }
    let version = thumbnail_version(source);
    Ok(Some(ThumbnailDescriptor {
        url: thumbnail_url(&source.job_id, &version),
        version,
        width,
        height,
    }))
}

fn generate_thumbnail(
    db_path: &Path,
    cache_root: &Path,
    source: &ThumbnailSource,
) -> SharedThumbnailResult {
    std::fs::create_dir_all(cache_root).map_err(|_| "THUMBNAIL_WRITE_FAILED".to_string())?;
    let image = image::open(&source.path).map_err(|_| "SOURCE_IMAGE_UNAVAILABLE".to_string())?;
    let (width, height) = image.dimensions();
    let crop_height = (height * 32 / 100).max(1);
    let cropped = image.crop_imm(0, 0, width, crop_height);
    let resized = cropped.resize_to_fill(
        THUMBNAIL_WIDTH,
        THUMBNAIL_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = resized.to_rgba8();
    let encoded =
        webp::Encoder::from_rgba(rgba.as_raw(), THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT).encode(82.0);
    if encoded.is_empty() {
        return Err("THUMBNAIL_WRITE_FAILED".to_string());
    }

    let file_key = format!("{:x}", Sha256::digest(source.job_id.as_bytes()));
    let file_name = format!(
        "{file_key}-m{}-s{THUMBNAIL_SCHEMA_VERSION}.webp",
        source.media_version
    );
    let final_path = cache_root.join(file_name);
    let part_path = final_path.with_extension("webp.part");
    std::fs::write(&part_path, &*encoded).map_err(|_| "THUMBNAIL_WRITE_FAILED".to_string())?;
    if final_path.exists() {
        std::fs::remove_file(&final_path).map_err(|_| "THUMBNAIL_WRITE_FAILED".to_string())?;
    }
    std::fs::rename(&part_path, &final_path).map_err(|_| "THUMBNAIL_WRITE_FAILED".to_string())?;

    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let prior_path = connection
        .query_row(
            "SELECT cache_path FROM newspaper_thumbnail_cache WHERE job_id = ?1",
            params![source.job_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Err(error) = connection.execute(
        "INSERT INTO newspaper_thumbnail_cache
         (job_id, source_page_id, source_media_version, cache_schema_version,
          cache_path, mime_type, pixel_width, pixel_height, byte_count, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'image/webp', ?6, ?7, ?8, ?9)
         ON CONFLICT(job_id) DO UPDATE SET
            source_page_id = excluded.source_page_id,
            source_media_version = excluded.source_media_version,
            cache_schema_version = excluded.cache_schema_version,
            cache_path = excluded.cache_path,
            mime_type = excluded.mime_type,
            pixel_width = excluded.pixel_width,
            pixel_height = excluded.pixel_height,
            byte_count = excluded.byte_count,
            updated_at = excluded.updated_at",
        params![
            source.job_id,
            source.page_id,
            source.media_version,
            THUMBNAIL_SCHEMA_VERSION,
            final_path.to_string_lossy(),
            THUMBNAIL_WIDTH,
            THUMBNAIL_HEIGHT,
            encoded.len() as u64,
            Utc::now().timestamp(),
        ],
    ) {
        let _ = std::fs::remove_file(&final_path);
        return Err(error.to_string());
    }
    if let Some(prior_path) = prior_path {
        let prior_path = PathBuf::from(prior_path);
        if prior_path != final_path && prior_path.starts_with(cache_root) {
            let _ = std::fs::remove_file(prior_path);
        }
    }
    let version = thumbnail_version(source);
    Ok(ThumbnailDescriptor {
        url: thumbnail_url(&source.job_id, &version),
        version,
        width: THUMBNAIL_WIDTH,
        height: THUMBNAIL_HEIGHT,
    })
}

fn thumbnail_version(source: &ThumbnailSource) -> String {
    format!("{}-{}", source.media_version, THUMBNAIL_SCHEMA_VERSION)
}

pub fn thumbnail_url(job_id: &str, version: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        format!("http://newspaper-media.localhost/thumbnail/{job_id}?v={version}")
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        format!("newspaper-media://localhost/thumbnail/{job_id}?v={version}")
    }
}

pub fn page_url(page_id: &str, media_version: i64) -> String {
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        format!("http://newspaper-media.localhost/page/{page_id}?v={media_version}")
    }
    #[cfg(not(any(target_os = "windows", target_os = "android")))]
    {
        format!("newspaper-media://localhost/page/{page_id}?v={media_version}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("linkvault.sqlite3");
        let connection = Connection::open(&db_path).unwrap();
        super::super::storage::initialize(&connection).unwrap();
        let image_path = directory.path().join("A01.jpg");
        ImageBuffer::from_fn(800, 1200, |x, y| {
            let value = ((x + y) % 255) as u8;
            Rgb([value, value.wrapping_add(40), value.wrapping_add(80)])
        })
        .save(&image_path)
        .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_batches
                 (id, status, destination, delay_minutes, optimize_images,
                  optimization_profile, keep_original_jpg, created_at, updated_at)
                 VALUES ('batch', 'completed', ?1, 0, 0, 'webp_high', 1, 1, 1)",
                params![directory.path().to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_jobs
                 (id, batch_id, edition_code, publication_date, status, output_dir,
                  page_count, completed_count, created_at, updated_at)
                 VALUES ('job', 'batch', 'NY', '2026-07-25', 'completed', ?1, 1, 1, 1, 1)",
                params![directory.path().to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO newspaper_pages
                 (id, job_id, page_number, source_url, original_path, status,
                  original_bytes, final_bytes, checksum, pixel_width, pixel_height,
                  media_version, created_at, updated_at)
                 VALUES ('page', 'job', 'A01', 'https://example.test/A01.jpg', ?1,
                         'completed', 1, 1, 'checksum', 800, 1200, 1, 1, 1)",
                params![image_path.to_string_lossy()],
            )
            .unwrap();
        drop(connection);
        (directory, db_path)
    }

    #[tokio::test]
    async fn generation_is_exact_and_valid_cache_is_reused() {
        let (_directory, db_path) = fixture();
        let coordinator = ThumbnailCoordinator::new(db_path.clone());

        let generated = coordinator.ensure("job".to_string()).await.unwrap();
        assert!(matches!(generated, EnsureThumbnailResult::Generated { .. }));
        assert_eq!(coordinator.generation_start_count(), 1);

        let connection = Connection::open(db_path).unwrap();
        let path: String = connection
            .query_row(
                "SELECT cache_path FROM newspaper_thumbnail_cache WHERE job_id = 'job'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            image::open(path).unwrap().dimensions(),
            (THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT)
        );

        let ready = coordinator.ensure("job".to_string()).await.unwrap();
        assert!(matches!(ready, EnsureThumbnailResult::Ready { .. }));
        assert_eq!(coordinator.generation_start_count(), 1);
    }

    #[tokio::test]
    async fn duplicate_requests_share_one_generation() {
        let (_directory, db_path) = fixture();
        let coordinator = Arc::new(ThumbnailCoordinator::new(db_path));
        let left = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.ensure("job".to_string()).await })
        };
        let right = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.ensure("job".to_string()).await })
        };

        left.await.unwrap().unwrap();
        right.await.unwrap().unwrap();
        assert_eq!(coordinator.generation_start_count(), 1);
        assert_eq!(coordinator.active_count(), 0);
        assert_eq!(coordinator.pending_count().await, 0);
    }
}
