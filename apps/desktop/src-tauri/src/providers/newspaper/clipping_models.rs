//! Clipping aggregate models, validation limits, and safe error codes.
//!
//! Implements the approved contracts of specification 02 sections 3, 6, and
//! 18. Backend validation is authoritative: invalid input returns a typed
//! error code and never writes files or rows.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

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
pub const SEARCH_PAGE_LIMIT: u32 = 50;
pub const POSSIBLE_MATCH_LIMIT: usize = 25;
pub const FUZZY_CANDIDATE_LIMIT: usize = 100;
pub const SEARCH_SNIPPET_MAX_CHARS: usize = 240;
/// Canonical clipping asset MIME type (D-008).
pub const CLIPPING_ASSET_MIME: &str = "image/webp";
/// Registered page image MIME types accepted as source snapshots.
pub const SUPPORTED_SOURCE_MIME_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClippingRootKind {
    LegacyManaged,
    DownloadSnapshot,
}

impl ClippingRootKind {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::LegacyManaged => "legacy_managed",
            Self::DownloadSnapshot => "download_snapshot",
        }
    }

    pub fn from_sql(value: &str) -> Option<Self> {
        match value {
            "legacy_managed" => Some(Self::LegacyManaged),
            "download_snapshot" => Some(Self::DownloadSnapshot),
            _ => None,
        }
    }

    pub fn accepts_new_clippings(self) -> bool {
        matches!(self, Self::DownloadSnapshot)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClippingRoot {
    pub id: String,
    pub kind: ClippingRootKind,
    /// Backend locator. For download roots this is an absolute snapshot-root
    /// path; the legacy locator is resolved through application storage.
    pub locator: String,
    pub locator_key: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClippingRootStatus {
    Unchecked,
    Connected,
    Offline,
    MarkerMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClippingRootSummary {
    pub root_id: String,
    pub kind: String,
    pub display_path: String,
    pub status: ClippingRootStatus,
    pub last_checked_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReconnectNewspaperSnapshotRootResult {
    Cancelled,
    Connected { root: ClippingRootSummary },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClippingAssetState {
    Creating,
    Ready,
    Missing,
    DeletePending,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClippingSummary {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub asset_state: ClippingAssetState,
    pub asset_error_code: Option<String>,
    pub asset_version: u32,
    pub asset_pixel_width: u32,
    pub asset_pixel_height: u32,
    pub source_available: bool,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperClippingSummary {
    pub id: String,
    pub title: String,
    pub note_excerpt: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub thumbnail_ready: bool,
    pub thumbnail_url: Option<String>,
    pub thumbnail_version: Option<String>,
    pub source_available: bool,
    pub asset_state: ClippingAssetState,
    pub asset_error_code: Option<String>,
    pub asset_width: u32,
    pub asset_height: u32,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<ClippingSummary> for NewspaperClippingSummary {
    fn from(value: ClippingSummary) -> Self {
        Self {
            id: value.id,
            title: value.title,
            note_excerpt: value.excerpt,
            edition_code: value.edition_code,
            edition_name: value.edition_name,
            publication_date: value.publication_date,
            page_number: value.page_number,
            thumbnail_ready: false,
            thumbnail_url: None,
            thumbnail_version: None,
            source_available: value.source_available,
            asset_state: value.asset_state,
            asset_error_code: value.asset_error_code,
            asset_width: value.asset_pixel_width,
            asset_height: value.asset_pixel_height,
            revision: value.revision,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NewspaperClippingMatchField {
    Title,
    Note,
    Edition,
    Date,
    Page,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperClippingSearchSnippetPart {
    pub text: String,
    pub highlighted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperClippingSearchSnippet {
    pub field: NewspaperClippingMatchField,
    pub parts: Vec<NewspaperClippingSearchSnippetPart>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperClippingSearchResult {
    pub clipping: NewspaperClippingSummary,
    pub matched_fields: Vec<NewspaperClippingMatchField>,
    pub snippets: Vec<NewspaperClippingSearchSnippet>,
    pub possible_match: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchNewspaperClippingsRequest {
    pub query: String,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchNewspaperClippingsPage {
    pub items: Vec<NewspaperClippingSearchResult>,
    pub total: u32,
    pub offset: u32,
    pub limit: u32,
    pub note_search_applied: bool,
    pub revision: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPossibleNewspaperClippingsRequest {
    pub query: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPossibleNewspaperClippingsResponse {
    pub items: Vec<NewspaperClippingSearchResult>,
    pub limit: usize,
    pub revision: i64,
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

    pub asset_root_id: String,
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
        }
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
    let normalized = normalize_search_text(trimmed);
    if normalized.chars().count() > SEARCH_QUERY_MAX_CHARS {
        return Err(ClippingErrorCode::InvalidTitle);
    }
    Ok(normalized)
}

/// Compatibility-normalize and lowercase text for the derived search index.
/// Canonical clipping title/note bytes are never rewritten.
pub fn normalize_search_text(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).collect()
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
        assert_eq!(
            normalize_search_query("  ＬｉｎｋＶａｕｌｔ  ").unwrap(),
            "linkvault"
        );
        assert_eq!(normalize_search_text("CAFÉ"), normalize_search_text("café"));
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
    fn search_and_reconnect_ipc_models_use_the_approved_public_shape() {
        let public = NewspaperClippingSummary::from(ClippingSummary {
            id: VALID_ID.to_owned(),
            title: "Title".to_owned(),
            excerpt: "Note excerpt".to_owned(),
            edition_code: "NY".to_owned(),
            edition_name: "New York".to_owned(),
            publication_date: "2026-08-09".to_owned(),
            page_number: "A01".to_owned(),
            asset_state: ClippingAssetState::Ready,
            asset_error_code: None,
            asset_version: 1,
            asset_pixel_width: 320,
            asset_pixel_height: 200,
            source_available: true,
            revision: 2,
            created_at: 100,
            updated_at: 200,
        });
        let json = serde_json::to_value(&public).unwrap();
        assert_eq!(json["noteExcerpt"], "Note excerpt");
        assert_eq!(json["assetWidth"], 320);
        assert_eq!(json["assetHeight"], 200);
        assert_eq!(json["thumbnailReady"], false);
        assert!(json.get("excerpt").is_none());
        assert!(json.get("assetPixelWidth").is_none());

        let reconnected = ReconnectNewspaperSnapshotRootResult::Connected {
            root: ClippingRootSummary {
                root_id: "clipping-root-test".to_owned(),
                kind: "download_snapshot".to_owned(),
                display_path: r"C:\downloads\Newspaper snapshots".to_owned(),
                status: ClippingRootStatus::Connected,
                last_checked_at: Some(200),
            },
        };
        let json = serde_json::to_value(reconnected).unwrap();
        assert_eq!(json["status"], "connected");
        assert_eq!(json["root"]["rootId"], "clipping-root-test");
        assert!(json["root"].get("locator").is_none());
    }
}
