//! Clipping aggregate models, validation limits, and safe error codes.
//!
//! Implements the approved contracts of specification 02 sections 3, 6, and
//! 18. Backend validation is authoritative: invalid input returns a typed
//! error code and never writes files or rows.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Maximum Unicode scalar values for a trimmed clipping title (D-014).
pub const TITLE_MAX_CHARS: usize = 200;
/// Maximum UTF-8 bytes for a trimmed clipping title.
pub const TITLE_MAX_UTF8_BYTES: usize = 800;
/// Maximum UTF-8 bytes for the Markdown note body.
pub const NOTE_MAX_UTF8_BYTES: usize = 2_097_152;
/// Maximum UTF-8 bytes for edition code snapshots.
pub const EDITION_CODE_MAX_UTF8_BYTES: usize = 32;
/// Maximum UTF-8 bytes for edition name snapshots.
pub const EDITION_NAME_MAX_UTF8_BYTES: usize = 256;
/// Maximum UTF-8 bytes for page number snapshots.
pub const PAGE_NUMBER_MAX_UTF8_BYTES: usize = 64;
/// Maximum Unicode scalar values for a trimmed search query.
pub const SEARCH_QUERY_MAX_CHARS: usize = 200;
/// Maximum canonical asset size in bytes (512 MiB).
pub const ASSET_MAX_BYTES: u64 = 536_870_912;
/// Maximum list page size.
pub const LIST_LIMIT_MAX: u32 = 100;
/// Default frontend list page size.
pub const LIST_LIMIT_DEFAULT: u32 = 50;
/// Canonical clipping asset MIME type (D-008).
pub const CLIPPING_ASSET_MIME: &str = "image/webp";
/// Registered page image MIME types accepted as source snapshots.
pub const SUPPORTED_SOURCE_MIME_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

/// Frontend crop coordinates normalized against the rendered source image.
/// The native crop pipeline validates and converts these values before any
/// source file read (specification 03 sections 2, 3, and 15).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedCropRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Thin IPC request for the Phase 2 native crop command. All source
/// provenance and destination paths are deliberately absent: Rust derives
/// them from the registered Newspaper page record.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperClippingRequest {
    pub operation_id: String,
    pub page_id: String,
    pub expected_media_version: i64,
    pub rect: NormalizedCropRect,
}

/// Safe response returned only after Phase 1 has promoted and marked the
/// clipping asset ready. No filesystem path crosses IPC.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperClippingResponse {
    pub clipping_id: String,
    pub title: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub image_url: String,
    pub asset_version: u32,
    pub asset_width: u32,
    pub asset_height: u32,
    pub asset_byte_count: u64,
    pub revision: u64,
    pub created_at: i64,
}

/// Safe, structured failure returned by the asynchronous Phase 2 command.
/// It intentionally carries only the stable code/message/retry classification
/// and the caller-provided idempotency key, never raw paths or decoder/SQL
/// causes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewspaperClippingFailure {
    pub code: String,
    pub safe_message: String,
    pub retryable: bool,
    pub operation_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClippingSourceKind {
    Original,
    Optimized,
}

impl ClippingSourceKind {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Optimized => "optimized",
        }
    }

    pub fn from_sql(value: &str) -> Option<Self> {
        match value {
            "original" => Some(Self::Original),
            "optimized" => Some(Self::Optimized),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClippingAssetState {
    Creating,
    Ready,
    Missing,
    DeletePending,
}

impl ClippingAssetState {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::DeletePending => "delete_pending",
        }
    }

    pub fn from_sql(value: &str) -> Option<Self> {
        match value {
            "creating" => Some(Self::Creating),
            "ready" => Some(Self::Ready),
            "missing" => Some(Self::Missing),
            "delete_pending" => Some(Self::DeletePending),
            _ => None,
        }
    }

    /// Ordinary list/detail visibility excludes recovery states
    /// (AC-PERSIST-001).
    pub fn is_publicly_visible(self) -> bool {
        matches!(self, Self::Ready | Self::Missing)
    }
}

/// The persisted clipping aggregate (specification 02 section 3).
#[derive(Clone, Debug, PartialEq)]
pub struct NewspaperClipping {
    pub id: String,

    pub source_job_id: Option<String>,
    pub source_page_id: Option<String>,
    pub source_media_version_snapshot: i64,
    pub source_kind_snapshot: ClippingSourceKind,
    pub source_mime_type_snapshot: String,
    pub source_checksum_snapshot: Option<String>,

    pub edition_code_snapshot: String,
    pub edition_name_snapshot: String,
    pub publication_date_snapshot: String,
    pub page_number_snapshot: String,

    pub source_pixel_width: u32,
    pub source_pixel_height: u32,
    pub crop_x: u32,
    pub crop_y: u32,
    pub crop_width: u32,
    pub crop_height: u32,

    pub asset_relative_path: String,
    pub asset_mime_type: String,
    pub asset_pixel_width: u32,
    pub asset_pixel_height: u32,
    pub asset_byte_count: u64,
    pub asset_checksum_sha256: String,
    pub asset_version: u32,
    pub asset_state: ClippingAssetState,
    pub asset_error_code: Option<String>,

    pub title: String,
    pub note_markdown: String,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewspaperClippingSort {
    UpdatedDesc,
    CreatedDesc,
    PublicationDesc,
    TitleAsc,
}

impl NewspaperClippingSort {
    pub fn from_request(value: &str) -> Option<Self> {
        match value {
            "updated_desc" => Some(Self::UpdatedDesc),
            "created_desc" => Some(Self::CreatedDesc),
            "publication_desc" => Some(Self::PublicationDesc),
            "title_asc" => Some(Self::TitleAsc),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewspaperClippingListQuery {
    pub query: String,
    pub sort: NewspaperClippingSort,
    pub offset: u32,
    pub limit: u32,
}

/// Stable safe error codes (specification 02 section 18). Commands surface
/// only these codes; raw underlying errors remain diagnostic-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClippingErrorCode {
    InvalidId,
    InvalidTitle,
    NoteTooLarge,
    InvalidMarkdown,
    NotFound,
    NotEditable,
    RevisionConflict,
    OperationConflict,
    AssetRootUnavailable,
    AssetPathInvalid,
    AssetCollision,
    AssetWriteFailed,
    AssetPromotionFailed,
    AssetValidationFailed,
    AssetMissing,
    AssetChecksumMismatch,
    DatabaseWriteFailed,
    DatabaseReadFailed,
    RecoveryFailed,
    DeleteFailed,
    InvalidCropRect,
    CropTooSmall,
    SourcePageNotFound,
    SourcePageNotReady,
    SourceMediaStale,
    SourceMediaUnavailable,
    SourceMediaPathInvalid,
    SourceMediaUnsupported,
    SourceMediaTooLarge,
    SourceMediaDecodeFailed,
    SourceMediaChangedDuringRead,
    SourceOrientationUnsupported,
    SourceDimensionMismatch,
    SourceCropFailed,
    EncodeFailed,
    OutputTooLarge,
    OutputValidationFailed,
    ServiceUnavailable,
}

impl ClippingErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidId => "CLIPPING_INVALID_ID",
            Self::InvalidTitle => "CLIPPING_INVALID_TITLE",
            Self::NoteTooLarge => "CLIPPING_NOTE_TOO_LARGE",
            Self::InvalidMarkdown => "CLIPPING_INVALID_MARKDOWN",
            Self::NotFound => "CLIPPING_NOT_FOUND",
            Self::NotEditable => "CLIPPING_NOT_EDITABLE",
            Self::RevisionConflict => "CLIPPING_REVISION_CONFLICT",
            Self::OperationConflict => "CLIPPING_OPERATION_CONFLICT",
            Self::AssetRootUnavailable => "CLIPPING_ASSET_ROOT_UNAVAILABLE",
            Self::AssetPathInvalid => "CLIPPING_ASSET_PATH_INVALID",
            Self::AssetCollision => "CLIPPING_ASSET_COLLISION",
            Self::AssetWriteFailed => "CLIPPING_ASSET_WRITE_FAILED",
            Self::AssetPromotionFailed => "CLIPPING_ASSET_PROMOTION_FAILED",
            Self::AssetValidationFailed => "CLIPPING_ASSET_VALIDATION_FAILED",
            Self::AssetMissing => "CLIPPING_ASSET_MISSING",
            Self::AssetChecksumMismatch => "CLIPPING_ASSET_CHECKSUM_MISMATCH",
            Self::DatabaseWriteFailed => "CLIPPING_DATABASE_WRITE_FAILED",
            Self::DatabaseReadFailed => "CLIPPING_DATABASE_READ_FAILED",
            Self::RecoveryFailed => "CLIPPING_RECOVERY_FAILED",
            Self::DeleteFailed => "CLIPPING_DELETE_FAILED",
            Self::InvalidCropRect => "INVALID_CROP_RECT",
            Self::CropTooSmall => "CROP_TOO_SMALL",
            Self::SourcePageNotFound => "SOURCE_PAGE_NOT_FOUND",
            Self::SourcePageNotReady => "SOURCE_PAGE_NOT_READY",
            Self::SourceMediaStale => "SOURCE_MEDIA_STALE",
            Self::SourceMediaUnavailable => "SOURCE_MEDIA_UNAVAILABLE",
            Self::SourceMediaPathInvalid => "SOURCE_MEDIA_PATH_INVALID",
            Self::SourceMediaUnsupported => "SOURCE_MEDIA_UNSUPPORTED",
            Self::SourceMediaTooLarge => "SOURCE_MEDIA_TOO_LARGE",
            Self::SourceMediaDecodeFailed => "SOURCE_MEDIA_DECODE_FAILED",
            Self::SourceMediaChangedDuringRead => "SOURCE_MEDIA_CHANGED_DURING_READ",
            Self::SourceOrientationUnsupported => "SOURCE_ORIENTATION_UNSUPPORTED",
            Self::SourceDimensionMismatch => "SOURCE_DIMENSION_MISMATCH",
            Self::SourceCropFailed => "SOURCE_CROP_FAILED",
            Self::EncodeFailed => "CLIPPING_ENCODE_FAILED",
            Self::OutputTooLarge => "CLIPPING_OUTPUT_TOO_LARGE",
            Self::OutputValidationFailed => "CLIPPING_OUTPUT_VALIDATION_FAILED",
            Self::ServiceUnavailable => "CLIPPING_SERVICE_UNAVAILABLE",
        }
    }

    pub fn safe_message(self) -> &'static str {
        match self {
            Self::InvalidCropRect => "The selected crop area is invalid.",
            Self::CropTooSmall => "Select an area at least 32 by 32 source pixels.",
            Self::SourcePageNotFound => "The selected newspaper page is unavailable.",
            Self::SourcePageNotReady => "The selected newspaper page is not ready.",
            Self::SourceMediaStale => "The displayed page changed. Refresh it and try again.",
            Self::SourceMediaUnavailable => "The source page media is unavailable.",
            Self::SourceMediaPathInvalid => "The registered source media is unsafe to use.",
            Self::SourceMediaUnsupported => "The source page media is unsupported.",
            Self::SourceMediaTooLarge => "The source page exceeds the safe clipping limit.",
            Self::SourceMediaDecodeFailed => "The source page could not be decoded.",
            Self::SourceMediaChangedDuringRead => "The source page changed while it was read.",
            Self::SourceOrientationUnsupported => "The source page orientation is unsupported.",
            Self::SourceDimensionMismatch => {
                "The retained original does not match the displayed page dimensions."
            }
            Self::SourceCropFailed => "The source page crop could not be created.",
            Self::EncodeFailed => "The clipping image could not be encoded.",
            Self::OutputTooLarge => "The clipping output exceeds the safe size limit.",
            Self::OutputValidationFailed => "The clipping output could not be validated.",
            Self::ServiceUnavailable => "Clipping is temporarily unavailable.",
            Self::InvalidId => "The clipping operation identifier is invalid.",
            Self::InvalidTitle => "The clipping title is invalid.",
            Self::NoteTooLarge => "The clipping note is too large.",
            Self::InvalidMarkdown => "The clipping note contains invalid content.",
            Self::NotFound => "The clipping was not found.",
            Self::NotEditable => "The clipping is not editable right now.",
            Self::RevisionConflict => "The clipping changed in another window.",
            Self::OperationConflict => "The clipping operation conflicts with existing state.",
            Self::AssetRootUnavailable => "Clipping storage is unavailable.",
            Self::AssetPathInvalid => "The clipping asset path is invalid.",
            Self::AssetCollision => "A clipping asset already exists for this operation.",
            Self::AssetWriteFailed => "The clipping asset could not be written.",
            Self::AssetPromotionFailed => "The clipping asset could not be finalized.",
            Self::AssetValidationFailed => "The clipping asset could not be validated.",
            Self::AssetMissing => "The clipping asset is missing.",
            Self::AssetChecksumMismatch => "The clipping asset failed an integrity check.",
            Self::DatabaseWriteFailed => "The clipping could not be saved.",
            Self::DatabaseReadFailed => "Clipping data could not be read.",
            Self::RecoveryFailed => "Clipping recovery did not complete.",
            Self::DeleteFailed => "The clipping could not be deleted.",
        }
    }

    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::SourcePageNotReady
                | Self::SourceMediaChangedDuringRead
                | Self::EncodeFailed
                | Self::ServiceUnavailable
                | Self::AssetWriteFailed
                | Self::AssetPromotionFailed
                | Self::DatabaseWriteFailed
        )
    }
}

/// A clipping failure that only ever carries a safe code. Raw causes are
/// intentionally dropped so they cannot reach React or protocol bodies.
#[derive(Debug)]
pub struct ClippingError {
    pub code: ClippingErrorCode,
}

impl ClippingError {
    pub fn new(code: ClippingErrorCode) -> Self {
        Self { code }
    }

    pub fn as_safe_string(&self) -> String {
        self.code.as_str().to_string()
    }
}

impl From<ClippingErrorCode> for ClippingError {
    fn from(code: ClippingErrorCode) -> Self {
        Self::new(code)
    }
}

impl CreateNewspaperClippingFailure {
    pub fn from_code(operation_id: String, code: ClippingErrorCode) -> Self {
        Self {
            code: code.as_str().to_string(),
            safe_message: code.safe_message().to_string(),
            retryable: code.is_retryable(),
            operation_id,
        }
    }
}

/// Validate a canonical lowercase UUID clipping/operation identifier
/// (specification 02 section 6, FR-CROP-001 constraints). The ID doubles as
/// the idempotency key, so path separators, dots, percent escapes, NUL, and
/// lookalike characters are rejected outright.
pub fn validate_clipping_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let is_hyphen_position = index == 8 || index == 13 || index == 18 || index == 23;
        if is_hyphen_position {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase() {
            return false;
        }
    }
    true
}

/// Trim and validate a clipping title (D-014).
pub fn normalize_title(raw: &str) -> Result<String, ClippingErrorCode> {
    let trimmed = raw.trim();
    let chars = trimmed.chars().count();
    if !(1..=TITLE_MAX_CHARS).contains(&chars) || trimmed.len() > TITLE_MAX_UTF8_BYTES {
        return Err(ClippingErrorCode::InvalidTitle);
    }
    Ok(trimmed.to_string())
}

/// Validate a Markdown note body (specification 02 section 6). Markdown is
/// stored as data; this layer enforces only size and NUL limits.
pub fn validate_note_markdown(raw: &str) -> Result<(), ClippingErrorCode> {
    if raw.len() > NOTE_MAX_UTF8_BYTES {
        return Err(ClippingErrorCode::NoteTooLarge);
    }
    if raw.contains('\u{0000}') {
        return Err(ClippingErrorCode::InvalidMarkdown);
    }
    Ok(())
}

pub fn validate_edition_code(value: &str) -> bool {
    !value.is_empty() && value.len() <= EDITION_CODE_MAX_UTF8_BYTES
}

pub fn validate_edition_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= EDITION_NAME_MAX_UTF8_BYTES
}

pub fn validate_page_number(value: &str) -> bool {
    !value.is_empty() && value.len() <= PAGE_NUMBER_MAX_UTF8_BYTES
}

/// Publication dates are exact `YYYY-MM-DD` snapshots from the source job.
pub fn validate_publication_date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%d").to_string() == value)
        .unwrap_or(false)
}

pub fn validate_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !value.bytes().any(|byte| byte.is_ascii_uppercase())
}

pub fn validate_source_mime(value: &str) -> bool {
    SUPPORTED_SOURCE_MIME_TYPES.contains(&value)
}

pub fn validate_asset_byte_count(count: u64) -> bool {
    count > 0 && count <= ASSET_MAX_BYTES
}

pub fn validate_list_limit(limit: u32) -> Result<u32, ClippingErrorCode> {
    if limit == 0 || limit > LIST_LIMIT_MAX {
        return Err(ClippingErrorCode::InvalidId);
    }
    Ok(limit)
}

/// Trim and bound a search query (D-019).
pub fn normalize_search_query(raw: &str) -> Result<String, ClippingErrorCode> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > SEARCH_QUERY_MAX_CHARS {
        return Err(ClippingErrorCode::InvalidTitle);
    }
    Ok(trimmed.to_string())
}

/// Escape `%`, `_`, and the escape character itself before wrapping a search
/// term in `%...%` (D-019 bound escaped LIKE).
pub fn escape_like_pattern(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len() + 8);
    for ch in term.chars() {
        match ch {
            '%' | '_' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    #[test]
    fn clipping_id_validation_accepts_canonical_lowercase_uuids() {
        assert!(validate_clipping_id(VALID_ID));
    }

    #[test]
    fn clipping_id_validation_rejects_malformed_and_lookalike_values() {
        for candidate in [
            "",
            "0F8FAD5B-D9CB-469F-A165-70867728950E", // uppercase
            "0f8fad5b-d9cb-469f-a165-70867728950",  // too short
            "0f8fad5b-d9cb-469f-a165-70867728950ee", // too long
            "0f8fad5bd9cb-469f-a165-70867728950e",  // moved hyphen
            "0f8fad5b_d9cb-469f-a165-70867728950e", // underscore
            "0f8fad5b/d9cb-469f-a165-70867728950e", // path separator
            "0f8fad5b.d9cb-469f-a165-70867728950e", // dot
            "0f8fad5b-d9cb-469f-a165-70867728950g", // non-hex
            "../../etc/passwd",
        ] {
            assert!(!validate_clipping_id(candidate), "accepted {candidate:?}");
        }
    }

    #[test]
    fn title_validation_trims_and_bounds() {
        assert_eq!(
            normalize_title("  New York \u{00b7} A06  ").unwrap(),
            "New York \u{00b7} A06"
        );
        assert_eq!(
            normalize_title("   ").unwrap_err(),
            ClippingErrorCode::InvalidTitle
        );
        assert_eq!(
            normalize_title(&"a".repeat(TITLE_MAX_CHARS + 1)).unwrap_err(),
            ClippingErrorCode::InvalidTitle
        );
        assert_eq!(
            normalize_title(&"\u{4e2d}".repeat(TITLE_MAX_CHARS + 1)).unwrap_err(),
            ClippingErrorCode::InvalidTitle
        );
        // The byte bound is inclusive: 200 four-byte scalars are exactly
        // 800 UTF-8 bytes and remain valid.
        assert!(normalize_title(&"\u{1f4f0}".repeat(200)).is_ok());
    }

    #[test]
    fn note_validation_enforces_size_and_nul_limits() {
        assert!(validate_note_markdown("").is_ok());
        assert!(validate_note_markdown("plain *markdown*").is_ok());
        assert_eq!(
            validate_note_markdown(&"a".repeat(NOTE_MAX_UTF8_BYTES + 1)).unwrap_err(),
            ClippingErrorCode::NoteTooLarge
        );
        assert_eq!(
            validate_note_markdown("ok\u{0000}bad").unwrap_err(),
            ClippingErrorCode::InvalidMarkdown
        );
    }

    #[test]
    fn provenance_field_limits_are_enforced() {
        assert!(validate_edition_code("NY"));
        assert!(!validate_edition_code(""));
        assert!(!validate_edition_code(&"x".repeat(33)));
        assert!(validate_edition_name("New York"));
        assert!(!validate_edition_name(&"x".repeat(257)));
        assert!(validate_page_number("A06"));
        assert!(!validate_page_number(&"x".repeat(65)));
        assert!(validate_publication_date("2026-08-07"));
        assert!(!validate_publication_date("2026-13-07"));
        assert!(!validate_publication_date("2026-8-7"));
        assert!(!validate_publication_date(""));
    }

    #[test]
    fn checksum_mime_and_size_validation_match_spec_limits() {
        assert!(validate_sha256_hex(&"a".repeat(64)));
        assert!(!validate_sha256_hex(&"A".repeat(64)));
        assert!(!validate_sha256_hex(&"a".repeat(63)));
        assert!(validate_source_mime("image/jpeg"));
        assert!(validate_source_mime("image/png"));
        assert!(validate_source_mime("image/webp"));
        assert!(!validate_source_mime("text/html"));
        assert!(validate_asset_byte_count(1));
        assert!(validate_asset_byte_count(ASSET_MAX_BYTES));
        assert!(!validate_asset_byte_count(0));
        assert!(!validate_asset_byte_count(ASSET_MAX_BYTES + 1));
    }

    #[test]
    fn search_terms_escape_like_wildcards() {
        assert_eq!(escape_like_pattern("50%_off\\now"), "50\\%\\_off\\\\now");
        let normalized = normalize_search_query("  \u{4e2d}\u{6587}  ").unwrap();
        assert_eq!(normalized, "\u{4e2d}\u{6587}");
        assert!(normalize_search_query(&"q".repeat(SEARCH_QUERY_MAX_CHARS + 1)).is_err());
    }

    #[test]
    fn asset_state_visibility_hides_recovery_states() {
        assert!(ClippingAssetState::Ready.is_publicly_visible());
        assert!(ClippingAssetState::Missing.is_publicly_visible());
        assert!(!ClippingAssetState::Creating.is_publicly_visible());
        assert!(!ClippingAssetState::DeletePending.is_publicly_visible());
    }

    #[test]
    fn error_codes_round_trip_through_safe_strings() {
        assert_eq!(
            ClippingErrorCode::RevisionConflict.as_str(),
            "CLIPPING_REVISION_CONFLICT"
        );
        let error = ClippingError::new(ClippingErrorCode::AssetMissing);
        assert_eq!(error.as_safe_string(), "CLIPPING_ASSET_MISSING");
    }

    #[test]
    fn crop_failure_serialization_has_only_the_safe_ipc_contract() {
        let failure = CreateNewspaperClippingFailure::from_code(
            "7c9e6679-7425-40de-944b-e07fc1f90ae7".to_string(),
            ClippingErrorCode::SourceMediaPathInvalid,
        );
        let json = serde_json::to_value(&failure).unwrap();
        let object = json.as_object().unwrap();
        let mut fields = object.keys().map(String::as_str).collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(fields, ["code", "operationId", "retryable", "safeMessage"]);
        assert_eq!(object["code"], "SOURCE_MEDIA_PATH_INVALID");
        assert_eq!(
            object["safeMessage"],
            "The registered source media is unsafe to use."
        );
        let serialized = json.to_string();
        assert!(!serialized.contains("C:\\sensitive\\newspaper\\page.png"));
        assert!(!serialized.contains("SELECT "));
        assert!(!serialized.contains("decoder"));
    }
}
