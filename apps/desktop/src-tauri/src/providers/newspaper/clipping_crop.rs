//! Deterministic, source-resolution Newspaper clipping crop pipeline.
//!
//! This module owns Phase 2's untrusted request validation, authoritative
//! registered-source handling, exact normalized geometry, JPEG orientation,
//! lossless WebP encoding, and staging validation. It deliberately contains
//! no Tauri command, writer closure, or frontend behavior.

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use image::{io::Limits, io::Reader as ImageReader, DynamicImage, GenericImageView, ImageFormat};

use crate::app::database_diagnostics::{
    DatabaseDiagnosticInput, DatabaseDiagnosticKind, DatabaseDiagnosticOutcome,
    DatabaseDiagnostics, DatabaseProvider,
};

use super::{
    clipping_assets::{is_webp_container, sha256_hex, ClippingAssetLayout},
    clipping_models::{
        normalize_title, validate_clipping_id, validate_edition_code, validate_edition_name,
        validate_page_number, validate_publication_date, ClippingError, ClippingErrorCode,
        ClippingSourceKind, CreateNewspaperClippingRequest, NormalizedCropRect,
    },
    clipping_repository::CropSourceRecord,
};

pub const NORMALIZED_EPSILON: f64 = 0.000_001;
pub const MAX_SOURCE_FILE_BYTES: u64 = 1_073_741_824;
pub const MAX_SOURCE_DIMENSION: u32 = 32_768;
pub const MAX_SOURCE_PIXELS: u64 = 80_000_000;
pub const MAX_OUTPUT_BYTES: u64 = 536_870_912;
pub const MIN_CROP_WIDTH: u32 = 32;
pub const MIN_CROP_HEIGHT: u32 = 32;

/// Validated boundaries retained as normalized fractions so the exact source
/// coordinate conversion is defined in one pure place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValidatedNormalizedCropRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// Deterministic, source-pixel geometry persisted by Phase 1 registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcePixelCropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Internal timings retained for release-baseline measurement. The values are
/// not part of IPC and carry no path, image, or source-url data.
#[derive(Clone, Debug, Default)]
pub struct CropPipelineTimings {
    pub source_read: Duration,
    pub decode: Duration,
    pub crop: Duration,
    pub encode: Duration,
    pub validation: Duration,
    pub filesystem: Duration,
}

/// Fully staged output ready for the existing Phase 1 `register_staged`
/// lifecycle. Source paths are retained only long enough for the final
/// media-version/stability recheck and are never persisted or serialized.
#[derive(Debug)]
pub struct PreparedClipping {
    pub source_kind: ClippingSourceKind,
    pub source_mime_type: String,
    pub source_checksum_sha256: String,
    pub source_pixel_width: u32,
    pub source_pixel_height: u32,
    pub crop: SourcePixelCropRect,
    pub asset_byte_count: u64,
    pub asset_checksum_sha256: String,
    pub title: String,
    pub timings: CropPipelineTimings,
    selected_registered_path: String,
    selected_canonical_path: PathBuf,
    source_identity: SourceFileIdentity,
    source_job_output_dir: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFileIdentity {
    length: u64,
    modified: SystemTime,
}

struct ResolvedSource {
    kind: ClippingSourceKind,
    mime_type: &'static str,
    registered_path: String,
    canonical_path: PathBuf,
    identity: SourceFileIdentity,
    checksum_sha256: String,
    image: DynamicImage,
    read_elapsed: Duration,
    decode_elapsed: Duration,
}

#[derive(Clone, Copy)]
struct CandidateFailure {
    code: ClippingErrorCode,
    /// A source-security or source-stability issue must never silently fall
    /// through to a different registered candidate.
    terminal: bool,
}

impl CandidateFailure {
    fn normal(code: ClippingErrorCode) -> Self {
        Self {
            code,
            terminal: false,
        }
    }

    fn terminal(code: ClippingErrorCode) -> Self {
        Self {
            code,
            terminal: true,
        }
    }
}

/// Validates all request fields that can be checked before source-file reads
/// or staging-directory creation (FR-CROP-001..005).
pub fn validate_create_request(
    request: &CreateNewspaperClippingRequest,
) -> Result<ValidatedNormalizedCropRect, ClippingError> {
    if !validate_clipping_id(&request.operation_id) {
        return Err(ClippingError::new(ClippingErrorCode::InvalidId));
    }
    if !valid_page_id(&request.page_id) {
        return Err(ClippingError::new(ClippingErrorCode::SourcePageNotFound));
    }
    if request.expected_media_version <= 0 {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaStale));
    }
    validate_normalized_rect(request.rect)
}

/// Pure normalized-rectangle validation required by specification 03 section
/// 15. It intentionally performs the approved checks in the documented order.
pub fn validate_normalized_rect(
    rect: NormalizedCropRect,
) -> Result<ValidatedNormalizedCropRect, ClippingError> {
    let invalid = || ClippingError::new(ClippingErrorCode::InvalidCropRect);
    if !rect.x.is_finite()
        || !rect.y.is_finite()
        || !rect.width.is_finite()
        || !rect.height.is_finite()
    {
        return Err(invalid());
    }
    if rect.x < -NORMALIZED_EPSILON || rect.y < -NORMALIZED_EPSILON {
        return Err(invalid());
    }
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(invalid());
    }
    if rect.x > 1.0 + NORMALIZED_EPSILON || rect.y > 1.0 + NORMALIZED_EPSILON {
        return Err(invalid());
    }
    let right = rect.x + rect.width;
    if right > 1.0 + NORMALIZED_EPSILON {
        return Err(invalid());
    }
    let bottom = rect.y + rect.height;
    if bottom > 1.0 + NORMALIZED_EPSILON {
        return Err(invalid());
    }
    Ok(ValidatedNormalizedCropRect {
        left: rect.x.clamp(0.0, 1.0),
        top: rect.y.clamp(0.0, 1.0),
        right: right.clamp(0.0, 1.0),
        bottom: bottom.clamp(0.0, 1.0),
    })
}

/// Converts validated normalized boundaries with the binding floor/ceil edge
/// algorithm. Width/height are derived from the independently rounded edges,
/// never rounded independently.
pub fn to_source_pixels(
    rect: ValidatedNormalizedCropRect,
    source_width: u32,
    source_height: u32,
) -> Result<SourcePixelCropRect, ClippingError> {
    if source_width == 0 || source_height == 0 {
        return Err(ClippingError::new(ClippingErrorCode::InvalidCropRect));
    }
    let width = f64::from(source_width);
    let height = f64::from(source_height);
    let left = (rect.left * width).floor().clamp(0.0, width) as u32;
    let top = (rect.top * height).floor().clamp(0.0, height) as u32;
    let right = (rect.right * width).ceil().clamp(f64::from(left), width) as u32;
    let bottom = (rect.bottom * height).ceil().clamp(f64::from(top), height) as u32;
    let crop = SourcePixelCropRect {
        x: left,
        y: top,
        width: right
            .checked_sub(left)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::InvalidCropRect))?,
        height: bottom
            .checked_sub(top)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::InvalidCropRect))?,
    };
    if crop.width < MIN_CROP_WIDTH || crop.height < MIN_CROP_HEIGHT {
        return Err(ClippingError::new(ClippingErrorCode::CropTooSmall));
    }
    if crop
        .x
        .checked_add(crop.width)
        .map_or(true, |right| right > source_width)
        || crop
            .y
            .checked_add(crop.height)
            .map_or(true, |bottom| bottom > source_height)
    {
        return Err(ClippingError::new(ClippingErrorCode::InvalidCropRect));
    }
    Ok(crop)
}

/// Resolves the registered source, crops it with exact source-pixel geometry,
/// writes lossless bytes to the Phase 1 staging path, and validates the final
/// staging payload. The caller owns final DB recheck/registration.
pub fn stage_crop(
    request: &CreateNewspaperClippingRequest,
    rect: ValidatedNormalizedCropRect,
    source_record: &CropSourceRecord,
    layout: &ClippingAssetLayout,
    diagnostics: &DatabaseDiagnostics,
) -> Result<PreparedClipping, ClippingError> {
    validate_source_record(source_record, request.expected_media_version)?;
    // The title becomes part of the registered aggregate. Validate it before
    // any source read or staging mutation so an unusually long (but otherwise
    // valid) edition name cannot leave an untracked staging operation behind.
    let title = normalize_title(&format!(
        "{} \u{00b7} {} \u{00b7} {}",
        source_record.edition_name, source_record.publication_date, source_record.page_number
    ))
    .map_err(ClippingError::new)?;
    let root = resolve_registered_output_root(&source_record.output_dir)?;
    let source = resolve_best_source(source_record, &root)?;
    let (source_width, source_height) = source.image.dimensions();
    if source_record.stored_pixel_width != Some(source_width)
        || source_record.stored_pixel_height != Some(source_height)
    {
        diagnostics.record(DatabaseDiagnosticInput {
            kind: DatabaseDiagnosticKind::Recovery,
            operation: "clipping_crop_source_dimension_mismatch",
            provider: DatabaseProvider::Newspaper,
            workflow_id: Some(source_record.page_id.clone()),
            elapsed: Duration::ZERO,
            queue_depth: 0,
            outcome: DatabaseDiagnosticOutcome::Ok,
            error_class: None,
        });
    }
    validate_source_dimensions(source_width, source_height)?;
    let crop = to_source_pixels(rect, source_width, source_height)?;

    let crop_started = Instant::now();
    let cropped = source
        .image
        .crop_imm(crop.x, crop.y, crop.width, crop.height);
    let crop_elapsed = crop_started.elapsed();

    let encode_started = Instant::now();
    let encoded = encode_lossless_webp(&cropped)?;
    let encode_elapsed = encode_started.elapsed();
    let asset_byte_count = u64::try_from(encoded.len())
        .map_err(|_| ClippingError::new(ClippingErrorCode::OutputTooLarge))?;
    validate_output_byte_count(asset_byte_count)?;
    let asset_checksum_sha256 = sha256_hex(&encoded);

    let validation_started = Instant::now();
    validate_lossless_output(
        &encoded,
        crop.width,
        crop.height,
        asset_byte_count,
        &asset_checksum_sha256,
    )?;
    let validation_elapsed = validation_started.elapsed();

    let filesystem_started = Instant::now();
    layout.write_staging(&request.operation_id, &encoded)?;
    let filesystem_elapsed = filesystem_started.elapsed();

    Ok(PreparedClipping {
        source_kind: source.kind,
        source_mime_type: source.mime_type.to_string(),
        source_checksum_sha256: source.checksum_sha256,
        source_pixel_width: source_width,
        source_pixel_height: source_height,
        crop,
        asset_byte_count,
        asset_checksum_sha256,
        title,
        timings: CropPipelineTimings {
            source_read: source.read_elapsed,
            decode: source.decode_elapsed,
            crop: crop_elapsed,
            encode: encode_elapsed,
            validation: validation_elapsed,
            filesystem: filesystem_elapsed,
        },
        selected_registered_path: source.registered_path,
        selected_canonical_path: source.canonical_path,
        source_identity: source.identity,
        source_job_output_dir: source_record.output_dir.clone(),
    })
}

/// Revalidates the authoritative source row and the selected file immediately
/// before Phase 1 inserts its `creating` row. It rejects stale media rather
/// than silently rebinding a user selection to changed page content.
pub fn validate_source_recheck(
    expected_media_version: i64,
    initial_record: &CropSourceRecord,
    rechecked_record: Option<&CropSourceRecord>,
    prepared: &PreparedClipping,
) -> Result<(), ClippingError> {
    let record = rechecked_record
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourcePageNotReady))?;
    validate_source_record(record, expected_media_version)?;
    if record.output_dir != initial_record.output_dir || record.page_id != initial_record.page_id {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaStale));
    }
    let current_registered_path = match prepared.source_kind {
        ClippingSourceKind::Original => record.original_path.as_deref(),
        ClippingSourceKind::Optimized => record.optimized_path.as_deref(),
    };
    if current_registered_path != Some(prepared.selected_registered_path.as_str()) {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaStale));
    }

    let root = resolve_registered_output_root(&prepared.source_job_output_dir)?;
    // Revalidate the current database-registered spelling, not merely the
    // old canonical path. That catches a post-read symlink/reparse swap or
    // path escape even when the former target is still present on disk.
    let current_path = validate_registered_source_path(
        Path::new(current_registered_path.expect("checked above")),
        &root,
    )
    .map_err(|failure| ClippingError::new(failure.code))?;
    if current_path != prepared.selected_canonical_path {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaStale));
    }
    let identity =
        source_file_identity(&current_path).map_err(|failure| ClippingError::new(failure.code))?;
    if identity != prepared.source_identity {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceMediaChangedDuringRead,
        ));
    }
    Ok(())
}

fn valid_page_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_source_record(
    record: &CropSourceRecord,
    expected_media_version: i64,
) -> Result<(), ClippingError> {
    if record.page_status != "completed" {
        return Err(ClippingError::new(ClippingErrorCode::SourcePageNotReady));
    }
    if record.media_version <= 0 || record.media_version != expected_media_version {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaStale));
    }
    if !valid_page_id(&record.page_id)
        || !valid_page_id(&record.job_id)
        || !validate_edition_code(&record.edition_code)
        || !validate_edition_name(&record.edition_name)
        || !validate_publication_date(&record.publication_date)
        || !validate_page_number(&record.page_number)
        || record.output_dir.trim().is_empty()
    {
        return Err(ClippingError::new(ClippingErrorCode::SourcePageNotFound));
    }
    Ok(())
}

fn resolve_registered_output_root(output_dir: &str) -> Result<PathBuf, ClippingError> {
    let path = PathBuf::from(output_dir);
    if !path.is_absolute() {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| ClippingError::new(ClippingErrorCode::SourceMediaPathInvalid))?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_dir() {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    path.canonicalize()
        .map_err(|_| ClippingError::new(ClippingErrorCode::SourceMediaPathInvalid))
}

fn resolve_best_source(
    record: &CropSourceRecord,
    root: &Path,
) -> Result<ResolvedSource, ClippingError> {
    let mut last_failure = None;
    if let Some(original_path) = record.original_path.as_deref() {
        match read_candidate(ClippingSourceKind::Original, original_path, root) {
            Ok(original) => {
                if let Some(optimized_path) = record.optimized_path.as_deref() {
                    match read_candidate(ClippingSourceKind::Optimized, optimized_path, root) {
                        Ok(optimized)
                            if optimized.image.dimensions() != original.image.dimensions() =>
                        {
                            // The reader displays the optimized page whenever it exists.
                            // A differently dimensioned retained original cannot be used
                            // silently because its geometry would not match the selection.
                            return Ok(optimized);
                        }
                        Ok(_) => {}
                        Err(failure) if failure.terminal => {
                            return Err(ClippingError::new(failure.code));
                        }
                        Err(_) => {}
                    }
                }
                return Ok(original);
            }
            Err(failure) if failure.terminal => return Err(ClippingError::new(failure.code)),
            Err(failure) => last_failure = Some(failure.code),
        }
    }
    if let Some(optimized_path) = record.optimized_path.as_deref() {
        match read_candidate(ClippingSourceKind::Optimized, optimized_path, root) {
            Ok(optimized) => return Ok(optimized),
            Err(failure) if failure.terminal => return Err(ClippingError::new(failure.code)),
            Err(failure) => last_failure = Some(failure.code),
        }
    }
    Err(ClippingError::new(
        last_failure.unwrap_or(ClippingErrorCode::SourceMediaUnavailable),
    ))
}

fn read_candidate(
    kind: ClippingSourceKind,
    registered_path: &str,
    root: &Path,
) -> Result<ResolvedSource, CandidateFailure> {
    read_candidate_with_after_read(kind, registered_path, root, |_| {})
}

/// The after-read hook makes the stable-read invariant directly testable
/// without changing production behavior: the production path is the empty
/// callback above, while the test path mutates only a generated fixture
/// between its two metadata snapshots.
fn read_candidate_with_after_read<F>(
    kind: ClippingSourceKind,
    registered_path: &str,
    root: &Path,
    after_read: F,
) -> Result<ResolvedSource, CandidateFailure>
where
    F: FnOnce(&Path),
{
    let raw_path = PathBuf::from(registered_path);
    if !raw_path.is_absolute() {
        return Err(CandidateFailure::terminal(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    let canonical_path = validate_registered_source_path(&raw_path, root)?;
    let metadata = fs::symlink_metadata(&canonical_path)
        .map_err(|_| CandidateFailure::normal(ClippingErrorCode::SourceMediaUnavailable))?;
    validate_source_file_bytes(metadata.len())?;
    let expected = mime_and_format_for_path(&canonical_path)
        .ok_or_else(|| CandidateFailure::normal(ClippingErrorCode::SourceMediaUnsupported))?;
    let identity = source_file_identity(&canonical_path)?;
    let read_started = Instant::now();
    let bytes = fs::read(&canonical_path)
        .map_err(|_| CandidateFailure::terminal(ClippingErrorCode::SourceMediaChangedDuringRead))?;
    let read_elapsed = read_started.elapsed();
    after_read(&canonical_path);
    let after_identity = source_file_identity(&canonical_path)?;
    if identity != after_identity || u64::try_from(bytes.len()).ok() != Some(identity.length) {
        return Err(CandidateFailure::terminal(
            ClippingErrorCode::SourceMediaChangedDuringRead,
        ));
    }
    let sniffed = image::guess_format(&bytes)
        .map_err(|_| CandidateFailure::normal(ClippingErrorCode::SourceMediaUnsupported))?;
    if sniffed != expected.1 {
        return Err(CandidateFailure::normal(
            ClippingErrorCode::SourceMediaUnsupported,
        ));
    }
    if sniffed == ImageFormat::WebP {
        let features = webp::BitstreamFeatures::new(&bytes)
            .ok_or_else(|| CandidateFailure::normal(ClippingErrorCode::SourceMediaDecodeFailed))?;
        if features.has_animation() {
            return Err(CandidateFailure::normal(
                ClippingErrorCode::SourceMediaUnsupported,
            ));
        }
    }
    let (header_width, header_height) = ImageReader::with_format(Cursor::new(&bytes), sniffed)
        .into_dimensions()
        .map_err(|_| CandidateFailure::normal(ClippingErrorCode::SourceMediaDecodeFailed))?;
    validate_source_dimensions(header_width, header_height)
        .map_err(|error| CandidateFailure::normal(error.code))?;
    let decode_started = Instant::now();
    let image =
        decode_limited(&bytes, sniffed).map_err(|error| CandidateFailure::normal(error.code))?;
    let image = if sniffed == ImageFormat::Jpeg {
        apply_jpeg_exif_orientation(image, &bytes)
            .map_err(|error| CandidateFailure::normal(error.code))?
    } else {
        image
    };
    validate_source_dimensions(image.width(), image.height())
        .map_err(|error| CandidateFailure::normal(error.code))?;
    Ok(ResolvedSource {
        kind,
        mime_type: expected.0,
        registered_path: registered_path.to_string(),
        canonical_path,
        identity,
        checksum_sha256: sha256_hex(&bytes),
        image,
        read_elapsed,
        decode_elapsed: decode_started.elapsed(),
    })
}

fn validate_registered_source_path(
    candidate: &Path,
    root: &Path,
) -> Result<PathBuf, CandidateFailure> {
    let metadata = match fs::symlink_metadata(candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CandidateFailure::normal(
                ClippingErrorCode::SourceMediaUnavailable,
            ));
        }
        Err(_) => {
            return Err(CandidateFailure::normal(
                ClippingErrorCode::SourceMediaUnavailable,
            ));
        }
    };
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Err(CandidateFailure::terminal(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| CandidateFailure::terminal(ClippingErrorCode::SourceMediaPathInvalid))?;
    if !canonical.starts_with(root) {
        return Err(CandidateFailure::terminal(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    Ok(canonical)
}

fn source_file_identity(path: &Path) -> Result<SourceFileIdentity, CandidateFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| CandidateFailure::terminal(ClippingErrorCode::SourceMediaChangedDuringRead))?;
    if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Err(CandidateFailure::terminal(
            ClippingErrorCode::SourceMediaPathInvalid,
        ));
    }
    let modified = metadata
        .modified()
        .map_err(|_| CandidateFailure::terminal(ClippingErrorCode::SourceMediaChangedDuringRead))?;
    Ok(SourceFileIdentity {
        length: metadata.len(),
        modified,
    })
}

fn mime_and_format_for_path(path: &Path) -> Option<(&'static str, ImageFormat)> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Some(("image/jpeg", ImageFormat::Jpeg)),
        Some("png") => Some(("image/png", ImageFormat::Png)),
        Some("webp") => Some(("image/webp", ImageFormat::WebP)),
        _ => None,
    }
}

fn validate_source_dimensions(width: u32, height: u32) -> Result<(), ClippingError> {
    if width == 0 || height == 0 || width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaTooLarge));
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceMediaTooLarge))?;
    if pixels > MAX_SOURCE_PIXELS {
        return Err(ClippingError::new(ClippingErrorCode::SourceMediaTooLarge));
    }
    Ok(())
}

/// Checks the metadata length before allocating a source byte buffer. Keeping
/// this small guard separate makes the 1 GiB boundary deterministic to test
/// without creating a giant fixture file.
fn validate_source_file_bytes(length: u64) -> Result<(), CandidateFailure> {
    if length == 0 {
        return Err(CandidateFailure::normal(
            ClippingErrorCode::SourceMediaUnavailable,
        ));
    }
    if length > MAX_SOURCE_FILE_BYTES {
        return Err(CandidateFailure::normal(
            ClippingErrorCode::SourceMediaTooLarge,
        ));
    }
    Ok(())
}

/// Keeps the output limit boundary deterministic to test without asking a
/// unit test to allocate a half-gigabyte WebP buffer. The production caller
/// passes the actual encoded byte length before staging anything.
fn validate_output_byte_count(byte_count: u64) -> Result<(), ClippingError> {
    if byte_count == 0 {
        return Err(ClippingError::new(ClippingErrorCode::EncodeFailed));
    }
    if byte_count > MAX_OUTPUT_BYTES {
        return Err(ClippingError::new(ClippingErrorCode::OutputTooLarge));
    }
    Ok(())
}

fn decode_limited(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, ClippingError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_SOURCE_PIXELS.saturating_mul(4));
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| ClippingError::new(ClippingErrorCode::SourceMediaDecodeFailed))
}

fn encode_lossless_webp(cropped: &DynamicImage) -> Result<Vec<u8>, ClippingError> {
    let rgba = cropped.to_rgba8();
    if rgba.width() == 0 || rgba.height() == 0 {
        return Err(ClippingError::new(ClippingErrorCode::SourceCropFailed));
    }
    Ok(
        webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
            .encode_lossless()
            .to_vec(),
    )
}

fn validate_lossless_output(
    bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
    expected_byte_count: u64,
    expected_checksum: &str,
) -> Result<(), ClippingError> {
    validate_output_byte_count(expected_byte_count)?;
    if bytes.is_empty()
        || u64::try_from(bytes.len()).ok() != Some(expected_byte_count)
        || sha256_hex(bytes) != expected_checksum
        || !is_webp_container(bytes)
    {
        return Err(ClippingError::new(
            ClippingErrorCode::OutputValidationFailed,
        ));
    }
    let features = webp::BitstreamFeatures::new(bytes)
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::OutputValidationFailed))?;
    if features.has_animation()
        || features.width() != expected_width
        || features.height() != expected_height
    {
        return Err(ClippingError::new(
            ClippingErrorCode::OutputValidationFailed,
        ));
    }
    let decoded = webp::Decoder::new(bytes)
        .decode()
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::OutputValidationFailed))?;
    if decoded.width() != expected_width || decoded.height() != expected_height {
        return Err(ClippingError::new(
            ClippingErrorCode::OutputValidationFailed,
        ));
    }
    Ok(())
}

/// Parses the JPEG APP1 EXIF orientation tag without adding an image-metadata
/// dependency. A missing tag means identity orientation; malformed EXIF or an
/// unsupported tag is rejected rather than silently ignored.
fn apply_jpeg_exif_orientation(
    image: DynamicImage,
    bytes: &[u8],
) -> Result<DynamicImage, ClippingError> {
    match jpeg_exif_orientation(bytes)? {
        1 => Ok(image),
        2 => Ok(image.fliph()),
        3 => Ok(image.rotate180()),
        4 => Ok(image.flipv()),
        // EXIF 5 is transpose: horizontal mirror then 270 degrees clockwise.
        5 => Ok(image.fliph().rotate270()),
        6 => Ok(image.rotate90()),
        // EXIF 7 is transverse: horizontal mirror then 90 degrees clockwise.
        7 => Ok(image.fliph().rotate90()),
        8 => Ok(image.rotate270()),
        _ => Err(ClippingError::new(
            ClippingErrorCode::SourceOrientationUnsupported,
        )),
    }
}

fn jpeg_exif_orientation(bytes: &[u8]) -> Result<u16, ClippingError> {
    if bytes.len() < 4 || bytes[0..2] != [0xff, 0xd8] {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceOrientationUnsupported,
        ));
    }
    let mut offset = 2_usize;
    while offset + 1 < bytes.len() {
        if bytes[offset] != 0xff {
            return Ok(1);
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= bytes.len() {
            break;
        }
        let marker = bytes[offset];
        offset += 1;
        if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        if marker == 0xda {
            break;
        }
        if offset + 2 > bytes.len() {
            return Err(ClippingError::new(
                ClippingErrorCode::SourceOrientationUnsupported,
            ));
        }
        let segment_length = usize::from(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]));
        if segment_length < 2 || offset + segment_length > bytes.len() {
            return Err(ClippingError::new(
                ClippingErrorCode::SourceOrientationUnsupported,
            ));
        }
        let payload = &bytes[offset + 2..offset + segment_length];
        if marker == 0xe1 && payload.starts_with(b"Exif\0\0") {
            return parse_tiff_orientation(&payload[6..]);
        }
        offset += segment_length;
    }
    Ok(1)
}

fn parse_tiff_orientation(tiff: &[u8]) -> Result<u16, ClippingError> {
    if tiff.len() < 8 {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceOrientationUnsupported,
        ));
    }
    let little_endian = match &tiff[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => {
            return Err(ClippingError::new(
                ClippingErrorCode::SourceOrientationUnsupported,
            ))
        }
    };
    if read_u16(tiff, 2, little_endian) != Some(42) {
        return Err(ClippingError::new(
            ClippingErrorCode::SourceOrientationUnsupported,
        ));
    }
    let ifd_offset = usize::try_from(
        read_u32(tiff, 4, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?,
    )
    .map_err(|_| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
    let count = usize::from(
        read_u16(tiff, ifd_offset, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?,
    );
    let entries_offset = ifd_offset
        .checked_add(2)
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
    for index in 0..count {
        let entry = entries_offset
            .checked_add(index.checked_mul(12).ok_or_else(|| {
                ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported)
            })?)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
        let tag = read_u16(tiff, entry, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
        if tag != 0x0112 {
            continue;
        }
        let field_type = read_u16(tiff, entry + 2, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
        let value_count = read_u32(tiff, entry + 4, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
        // TIFF SHORT values wider than one element are stored out-of-line.
        // This parser intentionally supports only the inline single-value
        // orientation form, so it must not treat the offset bytes as an
        // orientation value for a malformed/multi-value entry.
        if field_type != 3 || value_count != 1 {
            return Err(ClippingError::new(
                ClippingErrorCode::SourceOrientationUnsupported,
            ));
        }
        let orientation = read_u16(tiff, entry + 8, little_endian)
            .ok_or_else(|| ClippingError::new(ClippingErrorCode::SourceOrientationUnsupported))?;
        if !(1..=8).contains(&orientation) {
            return Err(ClippingError::new(
                ClippingErrorCode::SourceOrientationUnsupported,
            ));
        }
        return Ok(orientation);
    }
    Ok(1)
}

fn read_u16(bytes: &[u8], offset: usize, little_endian: bool) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let value: [u8; 2] = bytes.get(offset..end)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(value)
    } else {
        u16::from_be_bytes(value)
    })
}

fn read_u32(bytes: &[u8], offset: usize, little_endian: bool) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(value)
    } else {
        u32::from_be_bytes(value)
    })
}

fn is_symlink_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::{io::Cursor, path::Path};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NormalizedCropRect {
        NormalizedCropRect {
            x,
            y,
            width,
            height,
        }
    }

    fn valid_rect(x: f64, y: f64, width: f64, height: f64) -> ValidatedNormalizedCropRect {
        validate_normalized_rect(rect(x, y, width, height)).unwrap()
    }

    fn patterned_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(width, height, |x, y| {
            // Deliberately high-frequency, non-transparent generated pixels.
            // A lossless round trip cannot hide a one-pixel shift or blur.
            Rgba([
                ((x.wrapping_mul(37) + y.wrapping_mul(13)) % 251) as u8,
                ((x.wrapping_mul(11) + y.wrapping_mul(71)) % 251) as u8,
                ((x ^ y).wrapping_mul(29) % 251) as u8,
                1 + ((x.wrapping_mul(19) + y.wrapping_mul(23)) % 254) as u8,
            ])
        }))
    }

    fn write_png(path: &Path, image: &DynamicImage) {
        image.save_with_format(path, ImageFormat::Png).unwrap();
    }

    fn write_jpeg(path: &Path, image: &DynamicImage) {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Jpeg).unwrap();
        fs::write(path, bytes.into_inner()).unwrap();
    }

    fn write_webp(path: &Path, image: &DynamicImage) {
        fs::write(path, encode_lossless_webp(image).unwrap()).unwrap();
    }

    fn decoded_webp_rgba(bytes: &[u8]) -> Vec<u8> {
        let decoded = webp::Decoder::new(bytes).decode().unwrap();
        match decoded.layout() {
            webp::PixelLayout::Rgba => decoded.iter().copied().collect(),
            webp::PixelLayout::Rgb => {
                let mut rgba = Vec::with_capacity(decoded.len() / 3 * 4);
                for rgb in decoded.chunks_exact(3) {
                    rgba.extend_from_slice(rgb);
                    rgba.push(255);
                }
                rgba
            }
        }
    }

    fn source_root() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("job-output");
        fs::create_dir(&root).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        (temp, root, canonical_root)
    }

    /// Generated source fixtures must exercise reparse handling where the
    /// host permits it. Windows junctions cover the common non-admin setup;
    /// POSIX uses an ordinary directory symlink.
    fn create_dir_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
                .expect("temporary directory symlink fixture must be creatable");
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return;
            }
            let output = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &target.to_string_lossy(),
                ])
                .output()
                .expect("temporary directory junction fixture command must run");
            assert!(
                output.status.success(),
                "temporary directory junction fixture must be creatable"
            );
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            panic!("the crop path-security fixture has no supported link implementation")
        }
    }

    fn crop_source_record(
        root: &Path,
        original_path: Option<&Path>,
        optimized_path: Option<&Path>,
    ) -> CropSourceRecord {
        CropSourceRecord {
            page_id: "page_01".to_string(),
            job_id: "job_01".to_string(),
            page_number: "A01".to_string(),
            page_status: "completed".to_string(),
            original_path: original_path.map(|path| path.to_string_lossy().into_owned()),
            optimized_path: optimized_path.map(|path| path.to_string_lossy().into_owned()),
            stored_pixel_width: Some(64),
            stored_pixel_height: Some(64),
            media_version: 1,
            edition_code: "TEST".to_string(),
            edition_name: "Test edition".to_string(),
            publication_date: "2026-08-09".to_string(),
            output_dir: root.to_string_lossy().into_owned(),
        }
    }

    fn crop_request(rect: NormalizedCropRect) -> CreateNewspaperClippingRequest {
        CreateNewspaperClippingRequest {
            operation_id: "7c9e6679-7425-40de-944b-e07fc1f90ae7".to_string(),
            page_id: "page_01".to_string(),
            expected_media_version: 1,
            rect,
        }
    }

    fn assert_candidate_error(path: &Path, root: &Path, expected: ClippingErrorCode) {
        match read_candidate(ClippingSourceKind::Original, path.to_str().unwrap(), root) {
            Ok(_) => panic!("expected {} for generated fixture", expected.as_str()),
            Err(failure) => assert_eq!(failure.code, expected),
        }
    }

    fn animated_webp_bytes() -> Vec<u8> {
        let config = webp::WebPConfig::new().unwrap();
        let first = vec![0x11_u8; 32 * 32 * 4];
        let second = vec![0xe4_u8; 32 * 32 * 4];
        let mut encoder = webp::AnimEncoder::new(32, 32, &config);
        // Use two distinct frames and monotonic timestamps so this is an
        // actual animated bitstream, not merely a VP8X animation flag.
        encoder.add_frame(webp::AnimFrame::from_rgba(&first, 32, 32, 1_000));
        encoder.add_frame(webp::AnimFrame::from_rgba(&second, 32, 32, 1_250));
        encoder.encode().to_vec()
    }

    fn exif_orientation_jpeg_bytes(orientation: u16, value_count: u32) -> Vec<u8> {
        // A minimal JPEG envelope containing an APP1 Exif segment. The image
        // data is deliberately irrelevant to the EXIF parser tests below;
        // source-format tests use a fully encoded JPEG fixture separately.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42_u16.to_le_bytes());
        tiff.extend_from_slice(&8_u32.to_le_bytes());
        tiff.extend_from_slice(&1_u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112_u16.to_le_bytes());
        tiff.extend_from_slice(&3_u16.to_le_bytes());
        tiff.extend_from_slice(&value_count.to_le_bytes());
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&0_u16.to_le_bytes());
        tiff.extend_from_slice(&0_u32.to_le_bytes());

        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let segment_length = u16::try_from(payload.len() + 2).unwrap();
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&segment_length.to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0xff, 0xd9]);
        bytes
    }

    fn orientation_fixture() -> DynamicImage {
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(3, 2, |x, y| {
            Rgba([
                10 + (x * 60 + y * 17) as u8,
                20 + (x * 41 + y * 23) as u8,
                30 + (x * 19 + y * 47) as u8,
                255,
            ])
        }))
    }

    #[test]
    fn normalized_geometry_uses_exact_floor_and_ceil_edges() {
        let table = [
            (
                rect(0.0, 0.0, 1.0, 1.0),
                100,
                80,
                SourcePixelCropRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 80,
                },
            ),
            (
                rect(0.103, 0.207, 0.203, 0.401),
                1_000,
                1_000,
                SourcePixelCropRect {
                    x: 103,
                    y: 207,
                    width: 203,
                    height: 401,
                },
            ),
            (
                rect(0.684, 0.612, 0.316, 0.388),
                997,
                991,
                SourcePixelCropRect {
                    x: 681,
                    y: 606,
                    width: 316,
                    height: 385,
                },
            ),
        ];
        for (input, width, height, expected) in table {
            let actual =
                to_source_pixels(validate_normalized_rect(input).unwrap(), width, height).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn normalized_geometry_clamps_only_epsilon_boundary_noise() {
        let accepted = validate_normalized_rect(rect(
            -NORMALIZED_EPSILON,
            -NORMALIZED_EPSILON,
            1.0 + NORMALIZED_EPSILON,
            1.0 + NORMALIZED_EPSILON,
        ))
        .unwrap();
        assert_eq!(
            to_source_pixels(accepted, 64, 64).unwrap(),
            SourcePixelCropRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            }
        );

        for invalid in [
            rect(-NORMALIZED_EPSILON * 1.01, 0.0, 1.0, 1.0),
            rect(0.0, -NORMALIZED_EPSILON * 1.01, 1.0, 1.0),
            rect(0.0, 0.0, 0.0, 0.5),
            rect(0.0, 0.0, 0.5, -0.1),
            rect(1.0 + NORMALIZED_EPSILON * 1.01, 0.0, 0.1, 0.1),
            rect(0.8, 0.0, 0.2 + NORMALIZED_EPSILON * 1.01, 0.1),
            rect(f64::NAN, 0.0, 0.2, 0.2),
            rect(0.0, f64::INFINITY, 0.2, 0.2),
            rect(0.0, 0.0, f64::NEG_INFINITY, 0.2),
        ] {
            assert_eq!(
                validate_normalized_rect(invalid).unwrap_err().code,
                ClippingErrorCode::InvalidCropRect
            );
        }
    }

    #[test]
    fn source_geometry_rejects_small_crops_and_preserves_all_edges() {
        assert_eq!(
            to_source_pixels(valid_rect(0.0, 0.0, 31.0 / 100.0, 1.0), 100, 100)
                .unwrap_err()
                .code,
            ClippingErrorCode::CropTooSmall
        );
        for input in [
            rect(0.0, 0.0, 0.32, 0.32),
            rect(0.68, 0.0, 0.32, 0.32),
            rect(0.0, 0.68, 0.32, 0.32),
            rect(0.68, 0.68, 0.32, 0.32),
        ] {
            let actual =
                to_source_pixels(validate_normalized_rect(input).unwrap(), 100, 100).unwrap();
            assert!(actual.x + actual.width <= 100);
            assert!(actual.y + actual.height <= 100);
            assert!(actual.width >= MIN_CROP_WIDTH);
            assert!(actual.height >= MIN_CROP_HEIGHT);
        }
    }

    #[test]
    fn normalized_reverse_drag_and_adjacent_edges_keep_the_source_pixel_boundary() {
        // The reader normalizes either drag direction to the same min/abs
        // rectangle before this backend boundary. The crop result must remain
        // independent of drag direction.
        let forward = rect(0.125, 0.2, 0.5, 0.4);
        let reverse_normalized = rect(0.125, 0.2, 0.5, 0.4);
        assert_eq!(
            to_source_pixels(validate_normalized_rect(forward).unwrap(), 997, 991).unwrap(),
            to_source_pixels(
                validate_normalized_rect(reverse_normalized).unwrap(),
                997,
                991,
            )
            .unwrap()
        );

        let left = to_source_pixels(valid_rect(0.0, 0.0, 0.5, 1.0), 997, 100).unwrap();
        let right = to_source_pixels(valid_rect(0.5, 0.0, 0.5, 1.0), 997, 100).unwrap();
        // A fractional shared normalized boundary intentionally overlaps one
        // pixel after floor/ceil rounding; it must never leave a gap.
        assert_eq!(left.x + left.width, 499);
        assert_eq!(right.x, 498);
        assert!(right.x <= left.x + left.width);
        assert_eq!(right.x + right.width, 997);
    }

    #[test]
    fn source_geometry_handles_very_large_dimensions_without_overflow() {
        let crop = to_source_pixels(valid_rect(0.0, 0.0, 1.0, 1.0), u32::MAX, u32::MAX).unwrap();
        assert_eq!(crop.x, 0);
        assert_eq!(crop.y, 0);
        assert_eq!(crop.width, u32::MAX);
        assert_eq!(crop.height, u32::MAX);
    }

    #[test]
    fn randomized_geometry_invariants_hold() {
        let mut state = 0x72f5_1a33_u64;
        let next = |state: &mut u64| {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            f64::from((*state >> 11) as u32) / f64::from(u32::MAX)
        };
        for _ in 0..10_000 {
            let x = next(&mut state) * 0.8;
            let y = next(&mut state) * 0.8;
            let width = (1.0 - x) * (0.1 + 0.9 * next(&mut state));
            let height = (1.0 - y) * (0.1 + 0.9 * next(&mut state));
            let crop = to_source_pixels(valid_rect(x, y, width, height), 100_000, 100_000).unwrap();
            assert!(crop.width >= MIN_CROP_WIDTH);
            assert!(crop.height >= MIN_CROP_HEIGHT);
            assert!(crop.x.checked_add(crop.width).unwrap() <= 100_000);
            assert!(crop.y.checked_add(crop.height).unwrap() <= 100_000);
        }
    }

    #[test]
    fn lossless_webp_round_trip_preserves_exact_rgba_pixels() {
        let source = patterned_image(73, 59);
        let expected_rgba = source.to_rgba8().into_raw();
        let encoded = encode_lossless_webp(&source).unwrap();
        let expected_byte_count = u64::try_from(encoded.len()).unwrap();
        let expected_checksum = sha256_hex(&encoded);

        validate_lossless_output(&encoded, 73, 59, expected_byte_count, &expected_checksum)
            .unwrap();

        let decoded = webp::Decoder::new(&encoded).decode().unwrap();
        assert_eq!(decoded.width(), 73);
        assert_eq!(decoded.height(), 59);
        assert_eq!(decoded.layout(), webp::PixelLayout::Rgba);
        assert_eq!(&decoded[..], expected_rgba.as_slice());
    }

    #[test]
    fn source_format_fixtures_accept_static_png_jpeg_and_webp() {
        let (_temp, root, canonical_root) = source_root();
        let image = patterned_image(64, 48);
        let png = root.join("source.png");
        let jpeg = root.join("source.jpg");
        let webp = root.join("source.webp");
        write_png(&png, &image);
        write_jpeg(&jpeg, &image);
        write_webp(&webp, &image);

        for (path, expected_mime) in [
            (png.as_path(), "image/png"),
            (jpeg.as_path(), "image/jpeg"),
            (webp.as_path(), "image/webp"),
        ] {
            let resolved = match read_candidate(
                ClippingSourceKind::Original,
                path.to_str().unwrap(),
                &canonical_root,
            ) {
                Ok(resolved) => resolved,
                Err(failure) => panic!(
                    "{} should be a valid static source, got {}",
                    path.display(),
                    failure.code.as_str()
                ),
            };
            assert_eq!(resolved.kind, ClippingSourceKind::Original);
            assert_eq!(resolved.mime_type, expected_mime);
            assert_eq!(resolved.image.dimensions(), (64, 48));
        }
    }

    #[test]
    fn stage_crop_preserves_decoded_regions_for_jpeg_png_and_webp_sources() {
        struct SourceFormatFixture {
            file_name: &'static str,
            write: fn(&Path, &DynamicImage),
            source_image: DynamicImage,
            operation_id: &'static str,
        }

        let (temp, root, canonical_root) = source_root();
        let mut opaque = patterned_image(64, 64).to_rgba8();
        for pixel in opaque.pixels_mut() {
            pixel[3] = 255;
        }
        let fixtures = [
            SourceFormatFixture {
                file_name: "opaque.jpg",
                write: write_jpeg,
                source_image: DynamicImage::ImageRgba8(opaque),
                operation_id: "b1b1b1b1-b1b1-4b1b-8b1b-b1b1b1b1b1b1",
            },
            SourceFormatFixture {
                file_name: "alpha.png",
                write: write_png,
                source_image: patterned_image(64, 64),
                operation_id: "c2c2c2c2-c2c2-4c2c-8c2c-c2c2c2c2c2c2",
            },
            SourceFormatFixture {
                file_name: "lossless.webp",
                write: write_webp,
                source_image: patterned_image(64, 64),
                operation_id: "d3d3d3d3-d3d3-4d3d-8d3d-d3d3d3d3d3d3",
            },
        ];

        for fixture in fixtures {
            let source_path = root.join(fixture.file_name);
            (fixture.write)(&source_path, &fixture.source_image);
            let decoded_source = match read_candidate(
                ClippingSourceKind::Original,
                source_path.to_str().unwrap(),
                &canonical_root,
            ) {
                Ok(source) => source.image,
                Err(failure) => panic!(
                    "{} should resolve before its crop, got {}",
                    source_path.display(),
                    failure.code.as_str()
                ),
            };
            let expected = decoded_source
                .crop_imm(16, 16, 32, 32)
                .to_rgba8()
                .into_raw();
            let record = crop_source_record(&root, Some(&source_path), None);
            let mut request = crop_request(rect(0.25, 0.25, 0.5, 0.5));
            request.operation_id = fixture.operation_id.to_string();
            let layout =
                ClippingAssetLayout::new(temp.path().join(format!("assets-{}", fixture.file_name)));
            let prepared = stage_crop(
                &request,
                validate_create_request(&request).unwrap(),
                &record,
                &layout,
                &DatabaseDiagnostics::default(),
            )
            .unwrap();
            let output =
                fs::read(layout.staging_complete_path(fixture.operation_id).unwrap()).unwrap();
            let decoded_output = webp::Decoder::new(&output).decode().unwrap();
            assert_eq!(
                (decoded_output.width(), decoded_output.height()),
                (prepared.crop.width, prepared.crop.height)
            );
            assert_eq!(
                decoded_webp_rgba(&output),
                expected,
                "{}",
                fixture.file_name
            );
        }
    }

    #[test]
    fn source_format_fixtures_reject_mislabelled_corrupt_and_truncated_files() {
        let (_temp, root, canonical_root) = source_root();
        let image = patterned_image(64, 48);
        let valid_png = root.join("valid.png");
        write_png(&valid_png, &image);

        let mislabeled = root.join("png-content.jpg");
        fs::copy(&valid_png, &mislabeled).unwrap();
        assert_candidate_error(
            &mislabeled,
            &canonical_root,
            ClippingErrorCode::SourceMediaUnsupported,
        );

        let corrupt = root.join("corrupt.png");
        fs::write(&corrupt, b"not a source image").unwrap();
        assert_candidate_error(
            &corrupt,
            &canonical_root,
            ClippingErrorCode::SourceMediaUnsupported,
        );

        let mut truncated_bytes = fs::read(&valid_png).unwrap();
        truncated_bytes.truncate(truncated_bytes.len() - 10);
        let truncated = root.join("truncated.png");
        fs::write(&truncated, truncated_bytes).unwrap();
        assert_candidate_error(
            &truncated,
            &canonical_root,
            ClippingErrorCode::SourceMediaDecodeFailed,
        );
    }

    #[test]
    fn source_format_fixtures_reject_empty_and_animated_webp() {
        let (_temp, root, canonical_root) = source_root();
        let empty = root.join("empty.webp");
        fs::write(&empty, []).unwrap();
        assert_candidate_error(
            &empty,
            &canonical_root,
            ClippingErrorCode::SourceMediaUnavailable,
        );

        let animated = root.join("animated.webp");
        let bytes = animated_webp_bytes();
        assert!(
            webp::BitstreamFeatures::new(&bytes)
                .unwrap()
                .has_animation(),
            "fixture must contain more than a static WebP container"
        );
        fs::write(&animated, bytes).unwrap();
        assert_candidate_error(
            &animated,
            &canonical_root,
            ClippingErrorCode::SourceMediaUnsupported,
        );
    }

    #[test]
    fn source_and_output_limits_reject_oversized_bytes_dimensions_and_pixels_before_decode() {
        assert!(validate_source_file_bytes(MAX_SOURCE_FILE_BYTES).is_ok());
        assert_eq!(
            validate_source_file_bytes(MAX_SOURCE_FILE_BYTES + 1)
                .unwrap_err()
                .code,
            ClippingErrorCode::SourceMediaTooLarge
        );
        assert_eq!(
            validate_source_file_bytes(0).unwrap_err().code,
            ClippingErrorCode::SourceMediaUnavailable
        );

        assert!(validate_source_dimensions(MAX_SOURCE_DIMENSION, 1).is_ok());
        assert_eq!(
            validate_source_dimensions(MAX_SOURCE_DIMENSION + 1, 1)
                .unwrap_err()
                .code,
            ClippingErrorCode::SourceMediaTooLarge
        );
        assert!(validate_source_dimensions(10_000, 8_000).is_ok());
        assert_eq!(
            validate_source_dimensions(10_000, 8_001).unwrap_err().code,
            ClippingErrorCode::SourceMediaTooLarge
        );
        assert!(validate_output_byte_count(MAX_OUTPUT_BYTES).is_ok());
        assert_eq!(
            validate_output_byte_count(MAX_OUTPUT_BYTES + 1)
                .unwrap_err()
                .code,
            ClippingErrorCode::OutputTooLarge
        );
        assert_eq!(
            validate_output_byte_count(0).unwrap_err().code,
            ClippingErrorCode::EncodeFailed
        );
    }

    #[test]
    fn create_request_validation_rejects_noncanonical_ids_page_ids_and_versions() {
        let valid = crop_request(rect(0.0, 0.0, 1.0, 1.0));
        assert!(validate_create_request(&valid).is_ok());

        let invalid_cases = [
            (
                CreateNewspaperClippingRequest {
                    operation_id: "7C9E6679-7425-40DE-944B-E07FC1F90AE7".to_string(),
                    ..valid.clone()
                },
                ClippingErrorCode::InvalidId,
            ),
            (
                CreateNewspaperClippingRequest {
                    page_id: "page/escape".to_string(),
                    ..valid.clone()
                },
                ClippingErrorCode::SourcePageNotFound,
            ),
            (
                CreateNewspaperClippingRequest {
                    expected_media_version: 0,
                    ..valid
                },
                ClippingErrorCode::SourceMediaStale,
            ),
        ];
        for (request, expected) in invalid_cases {
            assert_eq!(
                validate_create_request(&request).unwrap_err().code,
                expected
            );
        }
    }

    #[test]
    fn source_resolver_prefers_original_falls_back_normally_and_never_falls_through_security() {
        let (temp, root, canonical_root) = source_root();
        let original = root.join("original.png");
        let optimized = root.join("optimized.png");
        let original_image = patterned_image(64, 64);
        let optimized_image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([7, 17, 29, 255])));
        write_png(&original, &original_image);
        write_png(&optimized, &optimized_image);

        let original_first = crop_source_record(&root, Some(&original), Some(&optimized));
        let selected = resolve_best_source(&original_first, &canonical_root).unwrap();
        assert_eq!(selected.kind, ClippingSourceKind::Original);
        assert_eq!(selected.image.to_rgba8(), original_image.to_rgba8());

        let missing_original = root.join("missing.png");
        let fallback = crop_source_record(&root, Some(&missing_original), Some(&optimized));
        let selected = resolve_best_source(&fallback, &canonical_root).unwrap();
        assert_eq!(selected.kind, ClippingSourceKind::Optimized);
        assert_eq!(selected.image.to_rgba8(), optimized_image.to_rgba8());

        let outside = temp.path().join("outside.png");
        write_png(&outside, &original_image);
        let unsafe_original = crop_source_record(&root, Some(&outside), Some(&optimized));
        match resolve_best_source(&unsafe_original, &canonical_root) {
            Ok(_) => panic!("an out-of-root original must not silently fall back"),
            Err(error) => assert_eq!(error.code, ClippingErrorCode::SourceMediaPathInvalid),
        }
    }

    #[test]
    fn source_resolver_uses_displayed_optimized_media_when_oriented_dimensions_differ() {
        let (_temp, root, canonical_root) = source_root();
        let original = root.join("original.png");
        let optimized = root.join("optimized.png");
        write_png(&original, &patterned_image(64, 64));
        write_png(&optimized, &patterned_image(80, 64));
        let record = crop_source_record(&root, Some(&original), Some(&optimized));

        let selected = resolve_best_source(&record, &canonical_root).unwrap();
        assert_eq!(selected.kind, ClippingSourceKind::Optimized);
        assert_eq!(selected.image.dimensions(), (80, 64));
    }

    #[test]
    fn source_resolver_rejects_directory_and_detects_mutation_during_read() {
        let (_temp, root, canonical_root) = source_root();
        assert_candidate_error(
            &root,
            &canonical_root,
            ClippingErrorCode::SourceMediaPathInvalid,
        );

        let source = root.join("changing.png");
        write_png(&source, &patterned_image(64, 64));
        match read_candidate_with_after_read(
            ClippingSourceKind::Original,
            source.to_str().unwrap(),
            &canonical_root,
            |path| write_png(path, &patterned_image(65, 64)),
        ) {
            Ok(_) => panic!("a post-read source mutation must not be accepted"),
            Err(failure) => assert_eq!(
                failure.code,
                ClippingErrorCode::SourceMediaChangedDuringRead
            ),
        }
    }

    #[test]
    fn source_resolver_rejects_reparse_source_and_registered_output_root() {
        let (temp, root, canonical_root) = source_root();
        let outside = temp.path().join("outside-source");
        fs::create_dir(&outside).unwrap();
        let outside_source = outside.join("page.png");
        write_png(&outside_source, &patterned_image(64, 64));

        let source_link = root.join("linked-source");
        let root_link = temp.path().join("linked-output-root");
        create_dir_link(&outside, &source_link);
        create_dir_link(&root, &root_link);

        assert_candidate_error(
            &source_link.join("page.png"),
            &canonical_root,
            ClippingErrorCode::SourceMediaPathInvalid,
        );
        assert_eq!(
            resolve_registered_output_root(root_link.to_str().unwrap())
                .unwrap_err()
                .code,
            ClippingErrorCode::SourceMediaPathInvalid
        );
    }

    #[test]
    fn registered_jpeg_orientation_is_applied_before_source_geometry() {
        let (_temp, root, canonical_root) = source_root();
        let raw = orientation_fixture();
        let path = root.join("oriented.jpg");
        let mut encoded = Cursor::new(Vec::new());
        raw.write_to(&mut encoded, ImageFormat::Jpeg).unwrap();
        let envelope = exif_orientation_jpeg_bytes(6, 1);
        let mut oriented_bytes = Vec::new();
        oriented_bytes.extend_from_slice(&encoded.get_ref()[..2]);
        oriented_bytes.extend_from_slice(&envelope[2..envelope.len() - 2]);
        oriented_bytes.extend_from_slice(&encoded.get_ref()[2..]);
        fs::write(&path, oriented_bytes).unwrap();

        let resolved = match read_candidate(
            ClippingSourceKind::Original,
            path.to_str().unwrap(),
            &canonical_root,
        ) {
            Ok(resolved) => resolved,
            Err(failure) => panic!("oriented JPEG rejected with {}", failure.code.as_str()),
        };
        assert_eq!(resolved.image.dimensions(), (2, 3));
    }

    #[test]
    fn stage_crop_persists_exact_lossless_source_region_to_staging() {
        let (temp, root, _canonical_root) = source_root();
        let source_image = patterned_image(64, 64);
        let source_path = root.join("source.png");
        write_png(&source_path, &source_image);
        let record = crop_source_record(&root, Some(&source_path), None);
        let request = crop_request(rect(0.25, 0.25, 0.5, 0.5));
        let layout = ClippingAssetLayout::new(temp.path().join("newspaper-clippings"));
        let prepared = stage_crop(
            &request,
            validate_create_request(&request).unwrap(),
            &record,
            &layout,
            &DatabaseDiagnostics::default(),
        )
        .unwrap();

        assert_eq!(
            prepared.crop,
            SourcePixelCropRect {
                x: 16,
                y: 16,
                width: 32,
                height: 32,
            }
        );
        assert_eq!(prepared.source_kind, ClippingSourceKind::Original);
        assert_eq!(prepared.source_mime_type, "image/png");
        let bytes = fs::read(layout.staging_complete_path(&request.operation_id).unwrap()).unwrap();
        let decoded = webp::Decoder::new(&bytes).decode().unwrap();
        let expected = source_image.crop_imm(16, 16, 32, 32).to_rgba8().into_raw();
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 32);
        assert_eq!(&decoded[..], expected.as_slice());
        assert_eq!(prepared.asset_checksum_sha256, sha256_hex(&bytes));
    }

    #[test]
    fn stage_crop_full_page_preserves_every_source_pixel_and_alpha_value() {
        let (temp, root, _canonical_root) = source_root();
        let source_image = patterned_image(64, 64);
        let source_path = root.join("source.png");
        write_png(&source_path, &source_image);
        let record = crop_source_record(&root, Some(&source_path), None);
        let request = crop_request(rect(0.0, 0.0, 1.0, 1.0));
        let layout = ClippingAssetLayout::new(temp.path().join("newspaper-clippings"));
        let prepared = stage_crop(
            &request,
            validate_create_request(&request).unwrap(),
            &record,
            &layout,
            &DatabaseDiagnostics::default(),
        )
        .unwrap();

        assert_eq!(
            prepared.crop,
            SourcePixelCropRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            }
        );
        let bytes = fs::read(layout.staging_complete_path(&request.operation_id).unwrap()).unwrap();
        let decoded = webp::Decoder::new(&bytes).decode().unwrap();
        let expected = source_image.to_rgba8().into_raw();
        assert_eq!(&decoded[..], expected.as_slice());
    }

    #[test]
    fn stage_crop_uses_decoded_dimensions_and_records_a_path_free_mismatch_diagnostic() {
        let (temp, root, _canonical_root) = source_root();
        let source_path = root.join("source.png");
        write_png(&source_path, &patterned_image(64, 64));
        let mut record = crop_source_record(&root, Some(&source_path), None);
        record.stored_pixel_width = Some(63);
        record.stored_pixel_height = Some(65);
        let request = crop_request(rect(0.25, 0.25, 0.5, 0.5));
        let layout = ClippingAssetLayout::new(temp.path().join("newspaper-clippings"));
        let diagnostics = DatabaseDiagnostics::default();

        let prepared = stage_crop(
            &request,
            validate_create_request(&request).unwrap(),
            &record,
            &layout,
            &diagnostics,
        )
        .unwrap();

        assert_eq!(
            (prepared.source_pixel_width, prepared.source_pixel_height),
            (64, 64)
        );
        assert_eq!((prepared.crop.x, prepared.crop.y), (16, 16));
        let event = diagnostics
            .snapshot()
            .into_iter()
            .find(|event| event.operation == "clipping_crop_source_dimension_mismatch")
            .expect("dimension mismatch must create a safe diagnostic");
        assert_eq!(event.workflow_id.as_deref(), Some("page_01"));
        assert_eq!(event.outcome, DatabaseDiagnosticOutcome::Ok);
        assert_eq!(event.error_class, None);
        assert!(!format!("{event:?}").contains(&root.to_string_lossy().to_string()));
    }

    #[test]
    fn stage_crop_rejects_invalid_derived_title_before_staging_creation() {
        let (temp, root, _canonical_root) = source_root();
        let source_path = root.join("source.png");
        write_png(&source_path, &patterned_image(64, 64));
        let mut record = crop_source_record(&root, Some(&source_path), None);
        // This is permitted by the source snapshot limit but its full derived
        // title exceeds the clipping title contract.
        record.edition_name = "x".repeat(200);
        let request = crop_request(rect(0.0, 0.0, 1.0, 1.0));
        let managed_root = temp.path().join("newspaper-clippings");
        let layout = ClippingAssetLayout::new(managed_root.clone());
        let result = stage_crop(
            &request,
            validate_create_request(&request).unwrap(),
            &record,
            &layout,
            &DatabaseDiagnostics::default(),
        );

        match result {
            Ok(_) => panic!("an invalid aggregate title must fail before staging"),
            Err(error) => assert_eq!(error.code, ClippingErrorCode::InvalidTitle),
        }
        assert!(
            !managed_root
                .join("staging")
                .join(&request.operation_id)
                .exists(),
            "a rejected derived title must not leave an untracked staging operation"
        );
    }

    #[test]
    fn jpeg_exif_orientation_applies_every_supported_value_before_geometry() {
        let source = orientation_fixture();
        for (orientation, expected) in [
            (1, source.clone()),
            (2, source.fliph()),
            (3, source.rotate180()),
            (4, source.flipv()),
            (5, source.fliph().rotate270()),
            (6, source.rotate90()),
            (7, source.fliph().rotate90()),
            (8, source.rotate270()),
        ] {
            let actual = apply_jpeg_exif_orientation(
                source.clone(),
                &exif_orientation_jpeg_bytes(orientation, 1),
            )
            .unwrap();
            assert_eq!(
                actual.to_rgba8(),
                expected.to_rgba8(),
                "orientation {orientation}"
            );
        }
    }

    #[test]
    fn jpeg_exif_orientation_rejects_malformed_or_out_of_line_entries() {
        for bytes in [
            exif_orientation_jpeg_bytes(1, 2),
            exif_orientation_jpeg_bytes(9, 1),
            vec![0xff, 0xd8, 0xff, 0xe1, 0x00, 0x10, b'E', b'x', b'i', b'f'],
        ] {
            assert_eq!(
                apply_jpeg_exif_orientation(orientation_fixture(), &bytes)
                    .unwrap_err()
                    .code,
                ClippingErrorCode::SourceOrientationUnsupported
            );
        }
    }
}
