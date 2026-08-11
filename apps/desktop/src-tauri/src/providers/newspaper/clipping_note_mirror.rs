//! Readable, export-only Markdown projection for clipping notes.
//!
//! SQLite remains canonical. This module never imports `note.md`; it only
//! replaces the file with the latest validated database Markdown. Writes use a
//! same-directory part file and an atomic replace so a crash cannot expose a
//! partially written note.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::clipping_assets::{is_symlink_or_reparse, ClippingAssetLayout};
use super::clipping_models::{ClippingError, ClippingErrorCode};

pub const NOTE_MIRROR_FILE_NAME: &str = "note.md";
const NOTE_MIRROR_PART_FILE_NAME: &str = ".note.md.part";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteMirrorWriteOutcome {
    Current,
    Written,
}

pub fn write_note_mirror(
    layout: &ClippingAssetLayout,
    clipping_id: &str,
    asset_relative_path: &str,
    markdown: &str,
) -> Result<NoteMirrorWriteOutcome, ClippingError> {
    let canonical = layout
        .contained_regular_file(&layout.canonical_path_at(clipping_id, asset_relative_path)?)?;
    let directory = canonical
        .parent()
        .ok_or_else(|| ClippingError::new(ClippingErrorCode::AssetPathInvalid))?;
    let target = directory.join(NOTE_MIRROR_FILE_NAME);
    let part = directory.join(NOTE_MIRROR_PART_FILE_NAME);

    if let Some(existing) = read_safe_file_if_present(&target)? {
        if existing == markdown.as_bytes() {
            remove_safe_part_if_present(&part)?;
            return Ok(NoteMirrorWriteOutcome::Current);
        }
    }
    remove_safe_part_if_present(&part)?;

    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|_| mirror_write_error())?;
        file.write_all(markdown.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| mirror_write_error())?;
        drop(file);
        atomic_replace(&part, &target)?;
        sync_parent(directory)?;
        Ok(NoteMirrorWriteOutcome::Written)
    })();

    if write_result.is_err() {
        let _ = remove_safe_part_if_present(&part);
    }
    write_result
}

fn read_safe_file_if_present(path: &Path) -> Result<Option<Vec<u8>>, ClippingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
                return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
            }
            fs::read(path).map(Some).map_err(|_| mirror_write_error())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(mirror_write_error()),
    }
}

fn remove_safe_part_if_present(path: &Path) -> Result<(), ClippingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_symlink_or_reparse(&metadata) || !metadata.file_type().is_file() {
                return Err(ClippingError::new(ClippingErrorCode::AssetPathInvalid));
            }
            fs::remove_file(path).map_err(|_| mirror_write_error())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(mirror_write_error()),
    }
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> Result<(), ClippingError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both buffers are owned, NUL-terminated UTF-16 paths and remain
    // alive for the duration of the synchronous Win32 call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(mirror_write_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> Result<(), ClippingError> {
    fs::rename(source, target).map_err(|_| mirror_write_error())
}

#[cfg(unix)]
fn sync_parent(directory: &Path) -> Result<(), ClippingError> {
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| mirror_write_error())
}

#[cfg(not(unix))]
fn sync_parent(_directory: &Path) -> Result<(), ClippingError> {
    // MOVEFILE_WRITE_THROUGH covers the rename durability boundary on Windows.
    Ok(())
}

fn mirror_write_error() -> ClippingError {
    ClippingError::new(ClippingErrorCode::AssetWriteFailed)
}
