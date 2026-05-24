use crate::security::is_safe_relative_archive_path;
use serde::Serialize;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use zip::ZipArchive;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExerciseArchiveExtractionResult {
    pub archive_path: PathBuf,
    pub destination_directory: Option<PathBuf>,
    pub attempted: bool,
    pub succeeded: bool,
    pub archive_deleted: bool,
    pub message: Option<String>,
}

impl ExerciseArchiveExtractionResult {
    fn skipped(archive_path: PathBuf) -> Self {
        Self {
            archive_path,
            destination_directory: None,
            attempted: false,
            succeeded: false,
            archive_deleted: false,
            message: None,
        }
    }

    fn extracted(
        archive_path: PathBuf,
        destination_directory: PathBuf,
        archive_deleted: bool,
        message: Option<String>,
    ) -> Self {
        Self {
            archive_path,
            destination_directory: Some(destination_directory),
            attempted: true,
            succeeded: true,
            archive_deleted,
            message,
        }
    }

    fn failed(
        archive_path: PathBuf,
        destination_directory: Option<PathBuf>,
        message: String,
    ) -> Self {
        Self {
            archive_path,
            destination_directory,
            attempted: true,
            succeeded: false,
            archive_deleted: false,
            message: Some(message),
        }
    }
}

#[derive(Debug, Error)]
enum ExerciseArchiveError {
    #[error("Archive file does not exist.")]
    ArchiveMissing,
    #[error("Archive path has no parent directory.")]
    MissingParentDirectory,
    #[error("Archive contains an unsafe file path: {0}")]
    UnsafeEntryPath(String),
    #[error("Archive IO failed: {0}")]
    Io(#[from] io::Error),
    #[error("Zip archive read failed: {0}")]
    Zip(#[from] zip::result::ZipError),
}

pub fn extract_zip_and_delete_archive(
    archive_path: impl AsRef<Path>,
) -> ExerciseArchiveExtractionResult {
    let archive_path = archive_path.as_ref().to_path_buf();

    if archive_path.as_os_str().is_empty() || !archive_path.is_file() {
        return ExerciseArchiveExtractionResult::failed(
            archive_path,
            None,
            ExerciseArchiveError::ArchiveMissing.to_string(),
        );
    }

    if !has_zip_extension(&archive_path) {
        return ExerciseArchiveExtractionResult::skipped(archive_path);
    }

    match extract_zip_to_destination(&archive_path) {
        Ok(destination_directory) => match fs::remove_file(&archive_path) {
            Ok(()) => ExerciseArchiveExtractionResult::extracted(
                archive_path,
                destination_directory,
                true,
                None,
            ),
            Err(error) => ExerciseArchiveExtractionResult::extracted(
                archive_path,
                destination_directory,
                false,
                Some(format!(
                    "Extracted successfully, but could not delete the zip file: {error}"
                )),
            ),
        },
        Err(error) => {
            ExerciseArchiveExtractionResult::failed(archive_path, None, error.to_string())
        }
    }
}

fn extract_zip_to_destination(archive_path: &Path) -> Result<PathBuf, ExerciseArchiveError> {
    let parent_directory = archive_path
        .parent()
        .ok_or(ExerciseArchiveError::MissingParentDirectory)?;
    let archive_base_name = archive_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Exercise Files");
    let temporary_directory =
        create_temporary_extraction_directory(parent_directory, archive_base_name)?;

    match extract_zip_safely(archive_path, &temporary_directory).and_then(|_| {
        move_extracted_content(parent_directory, &temporary_directory, archive_base_name)
    }) {
        Ok(destination_directory) => Ok(destination_directory),
        Err(error) => {
            try_delete_directory(&temporary_directory);
            Err(error)
        }
    }
}

fn extract_zip_safely(
    archive_path: &Path,
    destination_directory: &Path,
) -> Result<(), ExerciseArchiveError> {
    let archive_file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(archive_file)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let entry_name = entry.name().trim();
        if entry_name.is_empty() {
            continue;
        }

        let Some(entry_path) = safe_archive_entry_path(entry_name) else {
            return Err(ExerciseArchiveError::UnsafeEntryPath(
                entry_name.to_string(),
            ));
        };
        let target_path = destination_directory.join(entry_path);

        if entry.is_dir() {
            fs::create_dir_all(&target_path)?;
            continue;
        }

        if let Some(entry_directory) = target_path.parent() {
            fs::create_dir_all(entry_directory)?;
        }

        let mut output = File::create(&target_path)?;
        io::copy(&mut entry, &mut output)?;
    }

    Ok(())
}

fn move_extracted_content(
    parent_directory: &Path,
    temporary_directory: &Path,
    archive_base_name: &str,
) -> Result<PathBuf, ExerciseArchiveError> {
    let extracted_content_directory =
        extracted_content_directory(temporary_directory, archive_base_name)?;
    let moving_temporary_root = extracted_content_directory == temporary_directory;
    let destination_name = if moving_temporary_root {
        archive_base_name.to_string()
    } else {
        extracted_content_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(archive_base_name)
            .to_string()
    };
    let destination_directory = unique_directory_path(parent_directory, &destination_name);

    fs::rename(&extracted_content_directory, &destination_directory)?;
    if !moving_temporary_root {
        try_delete_directory(temporary_directory);
    }

    Ok(destination_directory)
}

fn extracted_content_directory(
    temporary_directory: &Path,
    archive_base_name: &str,
) -> Result<PathBuf, ExerciseArchiveError> {
    let mut top_level_files = 0;
    let mut top_level_directories = Vec::new();

    for entry in fs::read_dir(temporary_directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            top_level_directories.push(entry.path());
        } else if file_type.is_file() {
            top_level_files += 1;
        }
    }

    if top_level_files == 0 && top_level_directories.len() == 1 {
        let only_directory = &top_level_directories[0];
        if only_directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(archive_base_name))
        {
            return Ok(only_directory.clone());
        }
    }

    Ok(temporary_directory.to_path_buf())
}

fn safe_archive_entry_path(entry_name: &str) -> Option<PathBuf> {
    let normalized = entry_name.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return None;
    }

    let path = Path::new(&normalized);
    if !is_safe_relative_archive_path(path) {
        return None;
    }

    Some(path.to_path_buf())
}

fn create_temporary_extraction_directory(
    parent_directory: &Path,
    archive_base_name: &str,
) -> Result<PathBuf, ExerciseArchiveError> {
    let safe_name = safe_file_name(archive_base_name);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    for attempt in 0..1000 {
        let temporary_directory = parent_directory.join(format!(
            ".extracting-{safe_name}-{}-{attempt}",
            nonce + u128::from(std::process::id())
        ));
        match fs::create_dir(&temporary_directory) {
            Ok(()) => return Ok(temporary_directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary extraction directory",
    )
    .into())
}

fn unique_directory_path(parent_directory: &Path, requested_name: &str) -> PathBuf {
    let mut safe_name = safe_file_name(requested_name);
    if safe_name.trim().is_empty() {
        safe_name = "Exercise Files".to_string();
    }

    let mut candidate = parent_directory.join(&safe_name);
    let mut suffix = 2;
    while candidate.exists() {
        candidate = parent_directory.join(format!("{safe_name} ({suffix})"));
        suffix += 1;
    }
    candidate
}

fn safe_file_name(file_name: &str) -> String {
    let sanitized = file_name
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        .collect::<String>();

    if sanitized.trim().is_empty() {
        "Exercise Files".to_string()
    } else {
        sanitized
    }
}

fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn try_delete_directory(directory_path: &Path) {
    let _ = fs::remove_dir_all(directory_path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn extract_zip_and_delete_archive_with_valid_zip_extracts_to_folder_and_deletes_zip() {
        let root = tempfile::tempdir().unwrap();
        let zip_path = root.path().join("exercise.zip");
        create_zip(&zip_path, &[("chapter-1/readme.txt", "hello")]);

        let result = extract_zip_and_delete_archive(&zip_path);

        assert!(result.attempted);
        assert!(result.succeeded);
        assert!(result.archive_deleted);
        assert!(!zip_path.exists());
        assert_eq!(
            fs::read_to_string(root.path().join("exercise/chapter-1/readme.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn extract_zip_and_delete_archive_with_single_root_folder_does_not_duplicate_root_folder() {
        let root = tempfile::tempdir().unwrap();
        let zip_path = root.path().join("Ex_Files_Sample.zip");
        create_zip(&zip_path, &[("Ex_Files_Sample/start.txt", "ready")]);

        let result = extract_zip_and_delete_archive(&zip_path);

        assert!(result.succeeded);
        assert!(!zip_path.exists());
        assert_eq!(
            fs::read_to_string(root.path().join("Ex_Files_Sample/start.txt")).unwrap(),
            "ready"
        );
        assert!(!root.path().join("Ex_Files_Sample/Ex_Files_Sample").exists());
    }

    #[test]
    fn extract_zip_and_delete_archive_with_non_zip_file_skips_and_keeps_file() {
        let root = tempfile::tempdir().unwrap();
        let file_path = root.path().join("notes.txt");
        fs::write(&file_path, "not an archive").unwrap();

        let result = extract_zip_and_delete_archive(&file_path);

        assert!(!result.attempted);
        assert!(!result.succeeded);
        assert!(file_path.exists());
    }

    #[test]
    fn extract_zip_and_delete_archive_with_unsafe_zip_path_fails_and_keeps_zip() {
        let root = tempfile::tempdir().unwrap();
        let root_name = root.path().file_name().unwrap().to_string_lossy();
        let outside_file_name = format!("{root_name}-outside.txt");
        let outside_path = root.path().parent().unwrap().join(&outside_file_name);
        let zip_path = root.path().join("unsafe.zip");
        create_zip(&zip_path, &[(&format!("../{outside_file_name}"), "escape")]);

        let result = extract_zip_and_delete_archive(&zip_path);

        assert!(result.attempted);
        assert!(!result.succeeded);
        assert!(zip_path.exists());
        assert!(!outside_path.exists());
        assert_eq!(extracting_directories(root.path()).len(), 0);
    }

    #[test]
    fn extract_zip_and_delete_archive_uses_unique_destination_when_folder_exists() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("exercise")).unwrap();
        let zip_path = root.path().join("exercise.zip");
        create_zip(&zip_path, &[("readme.txt", "hello")]);

        let result = extract_zip_and_delete_archive(&zip_path);

        assert!(result.succeeded);
        assert_eq!(
            result.destination_directory.as_deref(),
            Some(root.path().join("exercise (2)").as_path())
        );
        assert_eq!(
            fs::read_to_string(root.path().join("exercise (2)/readme.txt")).unwrap(),
            "hello"
        );
    }

    fn create_zip(zip_path: &Path, entries: &[(&str, &str)]) {
        let file = File::create(zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();

        for (entry_name, contents) in entries {
            zip.start_file(entry_name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }

        zip.finish().unwrap();
    }

    fn extracting_directories(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".extracting-"))
            })
            .collect()
    }
}
