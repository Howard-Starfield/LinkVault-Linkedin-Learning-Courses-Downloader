//! Newspaper library recovery: path resolve, edition import, and clipping import.
//!
//! Snapshot discovery stays filesystem-only; SQLite writes go through
//! `archive_service` (editions) and `ClippingService` (roots + clippings).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;

use super::archive_service;
use super::clipping_assets::{ASSETS_DIR, CANONICAL_FILE_NAME};
use super::clipping_models::{
    validate_clipping_id, validate_page_number, validate_publication_date, ClippingError,
    ClippingErrorCode,
};
use super::clipping_note_mirror::NOTE_MIRROR_FILE_NAME;
use super::clipping_roots::{
    existing_safe_directory, INTERNAL_DIRECTORY_NAME, ROOT_MARKER_FILE_NAME,
    SNAPSHOT_DIRECTORY_NAME,
};
use super::clipping_service::ClippingService;
use super::models::{RecoverNewspaperLibraryResult, RecoverSnapshotRootStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredClipping {
    pub id: String,
    pub edition_code: String,
    pub edition_name: String,
    pub publication_date: String,
    pub page_number: String,
    pub asset_relative_path: String,
    pub absolute_webp: PathBuf,
    pub note_markdown: String,
    pub note_read_failed: bool,
}

/// Parse a clipping directory name into `(page_number, clipping_id)`.
///
/// Accepts a bare lowercase UUID directory or `Page <page> - <uuid>`.
pub(super) fn parse_clipping_dir_name(name: &str) -> Option<(String, String)> {
    if validate_clipping_id(name) {
        return Some((String::new(), name.to_string()));
    }
    let suffix = name.rsplit_once(" - ")?;
    let (prefix, id) = suffix;
    if !validate_clipping_id(id) || !prefix.starts_with("Page ") {
        return None;
    }
    let page = prefix.strip_prefix("Page ")?.trim().to_string();
    Some((page, id.to_string()))
}

pub(super) fn discover_clippings(snapshot_root: &Path) -> Result<Vec<DiscoveredClipping>, String> {
    let metadata = fs::metadata(snapshot_root).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("Snapshot root is not a directory.".to_string());
    }

    let mut discovered = Vec::new();
    for edition_entry in fs::read_dir(snapshot_root).map_err(|error| error.to_string())? {
        let edition_entry = edition_entry.map_err(|error| error.to_string())?;
        if !edition_entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let edition_name_os = edition_entry.file_name();
        let edition_dir_name = edition_name_os.to_string_lossy();
        if should_skip_snapshot_child(&edition_dir_name) {
            continue;
        }
        let Some((edition_name, edition_code)) = parse_edition_folder_name(&edition_dir_name)
        else {
            continue;
        };

        for date_entry in fs::read_dir(edition_entry.path()).map_err(|error| error.to_string())? {
            let date_entry = date_entry.map_err(|error| error.to_string())?;
            if !date_entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let date_name = date_entry.file_name();
            let publication_date = date_name.to_string_lossy();
            if publication_date == INTERNAL_DIRECTORY_NAME
                || !validate_publication_date(&publication_date)
            {
                continue;
            }

            for clipping_entry in
                fs::read_dir(date_entry.path()).map_err(|error| error.to_string())?
            {
                let clipping_entry = clipping_entry.map_err(|error| error.to_string())?;
                if !clipping_entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    continue;
                }
                let clipping_dir_name = clipping_entry.file_name().to_string_lossy().into_owned();
                if clipping_dir_name == INTERNAL_DIRECTORY_NAME {
                    continue;
                }
                let clipping_dir = clipping_entry.path();
                let webp_path = clipping_dir.join(CANONICAL_FILE_NAME);
                if !webp_path.is_file() {
                    continue;
                }
                let Some((parsed_page, id)) = parse_clipping_dir_name(&clipping_dir_name) else {
                    continue;
                };
                let page_number = resolve_page_number(&parsed_page);
                let asset_relative_path = relative_path_under_snapshot(snapshot_root, &webp_path)?;
                let (note_markdown, note_read_failed) =
                    read_note_markdown(&clipping_dir.join(NOTE_MIRROR_FILE_NAME));
                discovered.push(DiscoveredClipping {
                    id,
                    edition_code: edition_code.clone(),
                    edition_name: edition_name.clone(),
                    publication_date: publication_date.to_string(),
                    page_number,
                    asset_relative_path,
                    absolute_webp: webp_path,
                    note_markdown,
                    note_read_failed,
                });
            }
        }
    }

    discovered.sort_by(|left, right| {
        left.asset_relative_path
            .cmp(&right.asset_relative_path)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(discovered)
}

fn should_skip_snapshot_child(name: &str) -> bool {
    name == INTERNAL_DIRECTORY_NAME || name == ASSETS_DIR
}

fn parse_edition_folder_name(name: &str) -> Option<(String, String)> {
    let (left, code) = name.rsplit_once(" - ")?;
    if code.len() == 2 && code.chars().all(|character| character.is_ascii_uppercase()) {
        Some((left.to_string(), code.to_string()))
    } else {
        None
    }
}

fn sanitize_page_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() && !r#"\/:*?"<>|"#.contains(*character))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches([' ', '.'])
        .to_owned()
}

fn resolve_page_number(parsed_page: &str) -> String {
    let sanitized = sanitize_page_label(parsed_page);
    if validate_page_number(&sanitized) {
        return sanitized;
    }
    for candidate in ["A01", "Page", "clipping"] {
        if validate_page_number(candidate) {
            return candidate.to_string();
        }
    }
    "A01".to_string()
}

fn relative_path_under_snapshot(snapshot_root: &Path, absolute: &Path) -> Result<String, String> {
    let relative = absolute
        .strip_prefix(snapshot_root)
        .map_err(|_| "Clipping asset is outside the snapshot root.".to_string())?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn read_note_markdown(path: &Path) -> (String, bool) {
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(contents) => (contents, false),
            Err(_) => (String::new(), true),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(_) => (String::new(), true),
    }
}

pub(super) fn recover(
    db_path: &Path,
    clipping_service: &ClippingService,
    path: &str,
) -> Result<RecoverNewspaperLibraryResult, String> {
    let (save_to, snapshot) = resolve_recover_roots(Path::new(path))?;
    let editions = archive_service::import(db_path, &save_to)?;
    let mut result = RecoverNewspaperLibraryResult {
        editions_imported: count_u32(editions.imported),
        editions_already_known: count_u32(editions.already_known),
        editions_skipped: count_u32(editions.skipped),
        clippings_imported: 0,
        clippings_already_known: 0,
        clippings_skipped: 0,
        snapshot_root: RecoverSnapshotRootStatus::Missing,
        warnings: Vec::new(),
    };

    let Some(snapshot) = snapshot else {
        return Ok(result);
    };

    let now = Utc::now().timestamp();
    match clipping_service.ensure_snapshot_root_for_destination(&save_to, now) {
        Ok((root, status)) => {
            result.snapshot_root = status;
            match discover_clippings(Path::new(&root.locator)) {
                Ok(discovered) => {
                    match clipping_service.import_discovered_clippings(&root, &discovered, now) {
                        Ok((imported, known, skipped, warnings)) => {
                            result.clippings_imported = imported;
                            result.clippings_already_known = known;
                            result.clippings_skipped = skipped;
                            result.warnings.extend(warnings);
                        }
                        Err(error) => {
                            result.warnings.push(format!(
                                "Clipping import did not finish: {}.",
                                error.as_safe_string()
                            ));
                        }
                    }
                }
                Err(error) => {
                    result
                        .warnings
                        .push(format!("Could not scan Newspaper snapshots: {error}"));
                }
            }
        }
        Err(error) => {
            result.snapshot_root = map_clipping_error_to_status(&error, &snapshot);
            result.warnings.push(format!(
                "Snapshot root could not be registered: {}.",
                error.as_safe_string()
            ));
        }
    }
    Ok(result)
}

fn resolve_recover_roots(pick: &Path) -> Result<(PathBuf, Option<PathBuf>), String> {
    let unsafe_folder =
        || "The selected newspaper library folder is not a usable directory.".to_string();
    if is_snapshot_directory(pick) {
        let snapshot = existing_safe_directory(pick).map_err(|_| unsafe_folder())?;
        let parent = snapshot.parent().ok_or_else(|| {
            "The Newspaper snapshots folder has no parent download folder.".to_string()
        })?;
        let save_to = existing_safe_directory(parent).map_err(|_| unsafe_folder())?;
        return Ok((save_to, Some(snapshot)));
    }

    let save_to = existing_safe_directory(pick).map_err(|_| unsafe_folder())?;
    let snapshot = save_to.join(SNAPSHOT_DIRECTORY_NAME);
    let snapshot = snapshot_directory_if_present(&snapshot);
    Ok((save_to, snapshot))
}

fn is_snapshot_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(SNAPSHOT_DIRECTORY_NAME))
}

fn snapshot_directory_if_present(snapshot: &Path) -> Option<PathBuf> {
    match fs::symlink_metadata(snapshot) {
        Ok(_) => Some(snapshot.to_path_buf()),
        Err(_) => None,
    }
}

fn map_clipping_error_to_status(
    error: &ClippingError,
    snapshot: &Path,
) -> RecoverSnapshotRootStatus {
    match error.code {
        ClippingErrorCode::AssetRootUnavailable if !snapshot_path_present(snapshot) => {
            RecoverSnapshotRootStatus::Missing
        }
        ClippingErrorCode::AssetRootUnavailable if snapshot_marker_present(snapshot) => {
            RecoverSnapshotRootStatus::MarkerMismatch
        }
        _ => RecoverSnapshotRootStatus::Unavailable,
    }
}

fn snapshot_path_present(snapshot: &Path) -> bool {
    fs::symlink_metadata(snapshot).is_ok()
}

fn snapshot_marker_present(snapshot: &Path) -> bool {
    snapshot
        .join(INTERNAL_DIRECTORY_NAME)
        .join(ROOT_MARKER_FILE_NAME)
        .is_file()
}

fn count_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(super) fn recovered_clipping_title(edition_name: &str, page_number: &str) -> String {
    let edition = edition_name.trim();
    let page = page_number.trim();
    if page.is_empty() {
        edition.to_string()
    } else if edition.is_empty() {
        page.to_string()
    } else {
        format!("{edition} · {page}")
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotRootMarker {
    schema_version: u32,
    root_id: String,
}

pub(super) fn read_snapshot_marker_root_id(snapshot_root: &Path) -> Option<String> {
    let bytes = fs::read(
        snapshot_root
            .join(INTERNAL_DIRECTORY_NAME)
            .join(ROOT_MARKER_FILE_NAME),
    )
    .ok()?;
    let marker: SnapshotRootMarker = serde_json::from_slice(&bytes).ok()?;
    if marker.schema_version == 1
        && marker.root_id.starts_with("clipping-root-")
        && marker.root_id.len() <= 96
        && marker
            .root_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        Some(marker.root_id)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TEST_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const OTHER_ID: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

    #[test]
    fn parse_clipping_dir_name_accepts_bare_uuid_and_page_prefix() {
        assert_eq!(
            parse_clipping_dir_name(TEST_ID),
            Some((String::new(), TEST_ID.to_string()))
        );
        assert_eq!(
            parse_clipping_dir_name(&format!("Page A01 - {OTHER_ID}")),
            Some(("A01".to_string(), OTHER_ID.to_string()))
        );
        assert_eq!(
            parse_clipping_dir_name(&format!("Page B02 - {TEST_ID}")),
            Some(("B02".to_string(), TEST_ID.to_string()))
        );
        assert!(parse_clipping_dir_name("not-a-clipping-dir").is_none());
        assert!(parse_clipping_dir_name(&format!("A01 - {TEST_ID}")).is_none());
        assert!(parse_clipping_dir_name("Page A01 - not-a-uuid").is_none());
    }

    #[test]
    fn resolve_page_number_defaults_bare_uuid_dirs_to_a01() {
        assert_eq!(resolve_page_number(""), "A01");
        assert_eq!(resolve_page_number("B02"), "B02");
        assert_eq!(resolve_page_number(" A/01:* "), "A01");
    }

    #[test]
    fn discover_clippings_walks_snapshot_tree_and_skips_reserved_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        fs::create_dir_all(root.join(".linkvault")).unwrap();
        fs::create_dir_all(root.join("assets/ignored-id")).unwrap();
        let edition = root.join("波士頓 - BO/2026-08-09");
        fs::create_dir_all(edition.join(TEST_ID)).unwrap();
        fs::create_dir_all(edition.join(format!("Page B02 - {OTHER_ID}"))).unwrap();
        write_placeholder_webp(&edition.join(TEST_ID).join(CANONICAL_FILE_NAME));
        write_placeholder_webp(
            &edition
                .join(format!("Page B02 - {OTHER_ID}"))
                .join(CANONICAL_FILE_NAME),
        );
        fs::write(
            edition
                .join(format!("Page B02 - {OTHER_ID}"))
                .join(NOTE_MIRROR_FILE_NAME),
            "recovered note",
        )
        .unwrap();
        fs::write(edition.join(TEST_ID).join(NOTE_MIRROR_FILE_NAME), "").unwrap();

        let clippings = discover_clippings(root).unwrap();
        assert_eq!(clippings.len(), 2);

        let bare = clippings
            .iter()
            .find(|clipping| clipping.id == TEST_ID)
            .expect("bare uuid clipping");
        assert_eq!(bare.edition_code, "BO");
        assert_eq!(bare.edition_name, "波士頓");
        assert_eq!(bare.publication_date, "2026-08-09");
        assert_eq!(bare.page_number, "A01");
        assert_eq!(bare.note_markdown, "");
        assert!(!bare.note_read_failed);
        assert!(bare.asset_relative_path.contains('/'));
        assert!(!bare.asset_relative_path.contains('\\'));
        assert_eq!(
            bare.asset_relative_path,
            format!("波士頓 - BO/2026-08-09/{TEST_ID}/clipping-v1.webp")
        );
        assert_eq!(
            bare.absolute_webp,
            edition.join(TEST_ID).join(CANONICAL_FILE_NAME)
        );

        let prefixed = clippings
            .iter()
            .find(|clipping| clipping.id == OTHER_ID)
            .expect("page-prefixed clipping");
        assert_eq!(prefixed.page_number, "B02");
        assert_eq!(prefixed.note_markdown, "recovered note");
        assert!(!prefixed.note_read_failed);
        assert_eq!(
            prefixed.asset_relative_path,
            format!("波士頓 - BO/2026-08-09/Page B02 - {OTHER_ID}/clipping-v1.webp")
        );
    }

    fn write_placeholder_webp(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
    }
}
