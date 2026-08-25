//! Filename and directory layout for downloaded resources.

#![allow(dead_code)] // Phase 6 — wired by Phase 8

use std::path::{Path, PathBuf};

use crate::coursera::config::CourseraOptions;
use crate::coursera::utils::clean_filename;

/// Build a lecture filename like `01_title.mp4`. If `combined_section_lectures_nums`
/// is on, the format becomes `01_02_title.mp4` (section + lecture).
pub fn build_lecture_filename(
    module_idx: usize,
    lesson_idx: usize,
    lecture_idx: usize,
    title: &str,
    ext: Option<&str>,
    opts: &CourseraOptions,
) -> String {
    let safe_title = clean_filename(title, opts.unrestricted_filenames);
    let extension = ext.unwrap_or("mp4");
    if opts.combined_section_lectures_nums {
        format!(
            "{:02}_{:02}_{:02}_{}.{}",
            module_idx, lesson_idx, lecture_idx, safe_title, extension
        )
    } else {
        format!("{:02}_{}.{}", lecture_idx, safe_title, extension)
    }
}

/// Build a section directory name like `01_module-name/00_section-name`.
pub fn build_section_dir_name(
    module_idx: usize,
    module_name: &str,
    lesson_idx: usize,
    lesson_name: &str,
    opts: &CourseraOptions,
) -> String {
    let safe_mod = clean_filename(module_name, opts.unrestricted_filenames);
    let safe_lesson = clean_filename(lesson_name, opts.unrestricted_filenames);
    if opts.verbose_dirs {
        format!(
            "{:02}_{}/{:02}_{}",
            module_idx, safe_mod, lesson_idx, safe_lesson
        )
    } else {
        format!(
            "{:02}_{}/{:02}_{}",
            module_idx, safe_mod, lesson_idx, safe_lesson
        )
    }
}

/// Build a filename for an inline asset like `_slides.pdf`.
pub fn build_resource_filename(title: &str, ext: &str, opts: &CourseraOptions) -> String {
    let safe = clean_filename(title, opts.unrestricted_filenames);
    format!("_{}.{}", safe, ext)
}

/// Join `root` with `parts` and reject any traversal attack.
///
/// Each part may contain `/` or `\` (section directories use `01_mod/01_lesson`).
/// Those separators are split into segments; `.`, `..`, empty, and drive-letter
/// segments are rejected. The result is always a descendant of `root`.
pub fn safe_join(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for part in parts {
        if part.is_empty() {
            return None;
        }
        if part.starts_with('/') || part.starts_with('\\') {
            return None;
        }
        // Windows absolute path detection: `C:` or `C:\` or `C:/`.
        if part.len() >= 2 && part.as_bytes()[1] == b':' {
            return None;
        }
        for segment in part.split(['/', '\\']) {
            if !is_allowed_segment(segment) {
                return None;
            }
            out.push(segment);
        }
    }
    if !out.starts_with(root) {
        return None;
    }
    Some(out)
}

fn is_allowed_segment(segment: &str) -> bool {
    if segment.is_empty() || segment == "." || segment == ".." {
        return false;
    }
    if segment.contains(':') {
        return false;
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return false;
    }
    let stem = segment.split('.').next().unwrap_or(segment);
    !matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Create the parent directory of `path` without panicking when `parent()` is `None`.
pub fn create_parent_dir(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn opts() -> CourseraOptions {
        CourseraOptions::default()
    }

    #[test]
    fn build_lecture_filename_default_format() {
        let name = build_lecture_filename(1, 1, 1, "Welcome to ML", Some("mp4"), &opts());
        assert_eq!(name, "01_Welcome_to_ML.mp4");
    }

    #[test]
    fn build_lecture_filename_combined_format() {
        let mut o = opts();
        o.combined_section_lectures_nums = true;
        let name = build_lecture_filename(1, 2, 3, "Intro", Some("mp4"), &o);
        assert_eq!(name, "01_02_03_Intro.mp4");
    }

    #[test]
    fn build_lecture_filename_handles_forbidden_chars() {
        let name = build_lecture_filename(1, 1, 1, "a<b>c", Some("mp4"), &opts());
        assert!(!name.contains('<'));
        assert!(!name.contains('>'));
    }

    #[test]
    fn build_section_dir_name_default() {
        let dir = build_section_dir_name(1, "Module 1", 1, "Welcome", &opts());
        assert!(dir.contains("01_Module_1"));
        assert!(dir.contains("01_Welcome"));
    }

    #[test]
    fn build_resource_filename_prefixes_underscore() {
        let name = build_resource_filename("slides", "pdf", &opts());
        assert_eq!(name, "_slides.pdf");
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["..", "etc", "passwd"]).is_none());
    }

    #[test]
    fn safe_join_rejects_absolute() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["/etc/passwd"]).is_none());
    }

    #[test]
    fn safe_join_rejects_empty_part() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["a", "", "b"]).is_none());
    }

    #[test]
    fn safe_join_accepts_normal_path() {
        let root = Path::new("/tmp/out");
        let p = safe_join(root, &["course", "module", "lesson"]).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/out/course/module/lesson"));
    }

    #[test]
    fn safe_join_accepts_section_dir_with_slash() {
        let root = Path::new("/tmp/out");
        let path = safe_join(root, &["ml-005", "01_Module/01_Welcome", "lecture.mp4"]).unwrap();
        assert!(path.ends_with("lecture.mp4"));
        assert!(path.starts_with(root));
    }

    #[test]
    fn safe_join_rejects_nested_traversal_in_one_part() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["course", "foo/../../etc", "passwd"]).is_none());
        assert!(safe_join(root, &["course", r"foo\..\..\etc", "passwd"]).is_none());
    }

    #[test]
    fn create_parent_dir_creates_nested_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a").join("b").join("c.html");
        create_parent_dir(&dest).unwrap();
        assert!(dest.parent().unwrap().is_dir());
        std::fs::write(&dest, "ok").unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "ok");
    }

    #[test]
    fn safe_join_rejects_windows_drive_filename() {
        let root = Path::new(r"C:\out");
        assert!(safe_join(root, &["course", r"D:\secret.txt"]).is_none());
    }

    #[test]
    fn safe_join_rejects_ntfs_ads_colon() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["course", "lecture.mp4:hidden"]).is_none());
    }

    #[test]
    fn safe_join_rejects_trailing_dot_and_space() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["course", "file.txt."]).is_none());
        assert!(safe_join(root, &["course", "file.txt "]).is_none());
    }

    #[test]
    fn safe_join_rejects_reserved_device_name() {
        let root = Path::new("/tmp/out");
        assert!(safe_join(root, &["course", "CON"]).is_none());
        assert!(safe_join(root, &["course", "nul.txt"]).is_none());
    }
}
