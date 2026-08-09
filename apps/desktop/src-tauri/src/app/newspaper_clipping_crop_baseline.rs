#![cfg(feature = "crop-baseline")]

//! Application-owned, opt-in release measurement harness for the Phase 2 clipping crop pipeline.
//!
//! The feature-gated example generates only temporary source/media fixtures,
//! exercises the real staging and Phase 1 register/promote/ready lifecycle,
//! and prints safe aggregate timings. It is deliberately not a test: routine
//! `cargo test` must never allocate the 80-million-pixel boundary fixture.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use serde::Serialize;

use crate::app::{
    database::initialize_database, database_diagnostics::DatabaseDiagnostics,
    database_writer::DatabaseWriter,
};

use crate::providers::newspaper::{
    clipping_assets::ClippingAssetLayout,
    clipping_crop::{self, PreparedClipping},
    clipping_models::{
        ClippingAssetState, ClippingError, CreateNewspaperClippingRequest, NormalizedCropRect,
    },
    clipping_repository::{CropSourceRecord, NewClippingRecord},
    clipping_service::ClippingService,
};

const BASELINE_DATE: &str = "2026-08-09";

#[derive(Clone, Copy)]
struct BaselineCase {
    id: &'static str,
    source_format: &'static str,
    file_name: &'static str,
    image_format: ImageFormat,
    source_width: u32,
    source_height: u32,
    limit_near: bool,
    operation_id: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CropBaselineCase {
    case_id: &'static str,
    source_format: &'static str,
    selected_source_kind: &'static str,
    source_width: u32,
    source_height: u32,
    source_bytes: u64,
    crop_width: u32,
    crop_height: u32,
    output_bytes: u64,
    queue_wait_ms: f64,
    read_ms: f64,
    decode_ms: f64,
    crop_ms: f64,
    encode_ms: f64,
    validate_ms: f64,
    filesystem_ms: f64,
    database_ms: f64,
    total_ms: f64,
    working_set_delta_bytes: i64,
}

/// Runs the approved small JPEG, representative alpha PNG, and exact
/// 80-million-pixel static WebP cases. The caller owns committing the dated
/// report after it supplies machine/commit metadata from the measured host.
pub fn run() -> Result<String, String> {
    let cases = [
        BaselineCase {
            id: "small-jpeg",
            source_format: "jpeg",
            file_name: "page.jpg",
            image_format: ImageFormat::Jpeg,
            source_width: 256,
            source_height: 192,
            limit_near: false,
            operation_id: "88888888-8888-4888-8888-888888888888",
        },
        BaselineCase {
            id: "representative-alpha-png",
            source_format: "png",
            file_name: "page.png",
            image_format: ImageFormat::Png,
            source_width: 1_600,
            source_height: 2_200,
            limit_near: false,
            operation_id: "99999999-9999-4999-8999-999999999999",
        },
        BaselineCase {
            id: "limit-near-static-webp",
            source_format: "webp",
            file_name: "page.webp",
            image_format: ImageFormat::WebP,
            source_width: 8_000,
            source_height: 10_000,
            limit_near: true,
            operation_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        },
    ];
    let reports = cases
        .into_iter()
        .map(measure_case)
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string_pretty(&reports).map_err(|error| error.to_string())
}

fn measure_case(case: BaselineCase) -> Result<CropBaselineCase, String> {
    if case.limit_near
        && u64::from(case.source_width) * u64::from(case.source_height)
            != clipping_crop::MAX_SOURCE_PIXELS
    {
        return Err("limit-near baseline fixture no longer matches MAX_SOURCE_PIXELS".to_string());
    }

    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let db_path = temp.path().join("linkvault.sqlite3");
    let (connection, _) = initialize_database(&db_path).map_err(|error| error.to_string())?;
    drop(connection);
    let diagnostics = DatabaseDiagnostics::default();
    let writer = DatabaseWriter::start(db_path.clone(), diagnostics.clone())
        .map_err(|error| error.to_string())?;
    let layout = ClippingAssetLayout::new(temp.path().join("newspaper-clippings"));
    let service =
        ClippingService::new(db_path, writer.clone(), layout.clone(), diagnostics.clone());
    let source_root = temp.path().join("crop-baseline-source");
    fs::create_dir(&source_root).map_err(|error| error.to_string())?;
    let source_image =
        benchmark_fixture_image(case.source_width, case.source_height, case.limit_near);
    let source_path = write_source(
        &source_root,
        case.file_name,
        &source_image,
        case.image_format,
    )?;
    drop(source_image);
    let source_bytes = fs::metadata(&source_path)
        .map_err(|error| error.to_string())?
        .len();
    let source_record = crop_source_record(
        &source_root,
        &source_path,
        case.source_width,
        case.source_height,
    );
    let request = crop_request(case.operation_id);
    let rect = clipping_crop::validate_create_request(&request).map_err(safe_error)?;

    let working_set_before = current_working_set_bytes();
    let total_started = Instant::now();
    let prepared = clipping_crop::stage_crop(&request, rect, &source_record, &layout, &diagnostics)
        .map_err(safe_error)?;
    let timings = prepared.timings.clone();
    let source_kind = prepared.source_kind.as_sql();
    let crop_width = prepared.crop.width;
    let crop_height = prepared.crop.height;
    let output_bytes = prepared.asset_byte_count;
    let record = staged_record(&request, &source_record, prepared)?;
    let persistence_started = Instant::now();
    let clipping = service.register_staged(record).map_err(safe_error)?;
    let database_elapsed = persistence_started.elapsed();
    let total_elapsed = total_started.elapsed();
    if clipping.asset_state != ClippingAssetState::Ready
        || (clipping.asset_pixel_width, clipping.asset_pixel_height) != (crop_width, crop_height)
    {
        return Err(
            "crop baseline did not reach a ready, dimension-matching aggregate".to_string(),
        );
    }
    let working_set_delta = current_working_set_bytes() - working_set_before;

    service.shutdown_crop_service();
    drop(service);
    writer.shutdown().map_err(|error| error.to_string())?;

    Ok(CropBaselineCase {
        case_id: case.id,
        source_format: case.source_format,
        selected_source_kind: source_kind,
        source_width: case.source_width,
        source_height: case.source_height,
        source_bytes,
        crop_width,
        crop_height,
        output_bytes,
        queue_wait_ms: 0.0,
        read_ms: milliseconds(timings.source_read),
        decode_ms: milliseconds(timings.decode),
        crop_ms: milliseconds(timings.crop),
        encode_ms: milliseconds(timings.encode),
        validate_ms: milliseconds(timings.validation),
        filesystem_ms: milliseconds(timings.filesystem),
        database_ms: milliseconds(database_elapsed),
        total_ms: milliseconds(total_elapsed),
        working_set_delta_bytes: working_set_delta,
    })
}

fn benchmark_fixture_image(width: u32, height: u32, limit_near: bool) -> DynamicImage {
    if !limit_near {
        return DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([
                ((x.wrapping_mul(37) + y.wrapping_mul(13)) % 251) as u8,
                ((x.wrapping_mul(11) + y.wrapping_mul(71)) % 251) as u8,
                ((x ^ y).wrapping_mul(29) % 251) as u8,
                1 + ((x.wrapping_mul(19) + y.wrapping_mul(23)) % 254) as u8,
            ])
        }));
    }
    // A low-entropy, high-contrast grid keeps the boundary source practical
    // while still exercising the full decoded raster and lossless encoder.
    DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
        let grid = ((x / 32) + (y / 32)) % 2;
        let value = if grid == 0 { 0x24 } else { 0xda };
        Rgba([value, value ^ 0x7f, value ^ 0xc3, 255])
    }))
}

fn write_source(
    root: &Path,
    file_name: &str,
    image: &DynamicImage,
    format: ImageFormat,
) -> Result<PathBuf, String> {
    let path = root.join(file_name);
    match format {
        ImageFormat::WebP => {
            let rgba = image.to_rgba8();
            let bytes = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                .encode_lossless()
                .to_vec();
            fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        _ => image
            .save_with_format(&path, format)
            .map_err(|error| error.to_string())?,
    }
    Ok(path)
}

fn crop_source_record(
    root: &Path,
    source_path: &Path,
    source_width: u32,
    source_height: u32,
) -> CropSourceRecord {
    CropSourceRecord {
        page_id: "baseline_page_01".to_string(),
        job_id: "baseline_job_01".to_string(),
        page_number: "A01".to_string(),
        page_status: "completed".to_string(),
        original_path: Some(source_path.to_string_lossy().into_owned()),
        optimized_path: None,
        stored_pixel_width: Some(source_width),
        stored_pixel_height: Some(source_height),
        media_version: 1,
        edition_code: "BASELINE".to_string(),
        edition_name: "Crop baseline".to_string(),
        publication_date: BASELINE_DATE.to_string(),
        output_dir: root.to_string_lossy().into_owned(),
    }
}

fn crop_request(operation_id: &str) -> CreateNewspaperClippingRequest {
    CreateNewspaperClippingRequest {
        operation_id: operation_id.to_string(),
        page_id: "baseline_page_01".to_string(),
        expected_media_version: 1,
        rect: NormalizedCropRect {
            x: 0.125,
            y: 0.125,
            width: 0.75,
            height: 0.75,
        },
    }
}

fn staged_record(
    request: &CreateNewspaperClippingRequest,
    source: &CropSourceRecord,
    prepared: PreparedClipping,
) -> Result<NewClippingRecord, String> {
    Ok(NewClippingRecord {
        id: request.operation_id.clone(),
        source_job_id: None,
        source_page_id: None,
        source_media_version_snapshot: source.media_version,
        source_kind_snapshot: prepared.source_kind,
        source_mime_type_snapshot: prepared.source_mime_type,
        source_checksum_snapshot: Some(prepared.source_checksum_sha256),
        edition_code_snapshot: source.edition_code.clone(),
        edition_name_snapshot: source.edition_name.clone(),
        publication_date_snapshot: source.publication_date.clone(),
        page_number_snapshot: source.page_number.clone(),
        source_pixel_width: prepared.source_pixel_width,
        source_pixel_height: prepared.source_pixel_height,
        crop_x: prepared.crop.x,
        crop_y: prepared.crop.y,
        crop_width: prepared.crop.width,
        crop_height: prepared.crop.height,
        asset_relative_path: ClippingAssetLayout::canonical_relative_path(&request.operation_id)
            .map_err(safe_error)?,
        asset_byte_count: prepared.asset_byte_count,
        asset_checksum_sha256: prepared.asset_checksum_sha256,
        title: prepared.title,
        now: 123,
    })
}

fn safe_error(error: ClippingError) -> String {
    error.code.as_str().to_string()
}

fn milliseconds(duration: std::time::Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

#[cfg(windows)]
fn current_working_set_bytes() -> i64 {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };

    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = zeroed();
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ) != 0
        {
            counters.WorkingSetSize as i64
        } else {
            0
        }
    }
}

#[cfg(not(windows))]
fn current_working_set_bytes() -> i64 {
    0
}
