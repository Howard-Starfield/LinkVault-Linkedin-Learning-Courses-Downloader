use std::path::{Component, Path};

pub fn is_safe_relative_archive_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_archive_member() {
        assert!(!is_safe_relative_archive_path(Path::new("../outside.txt")));
        assert!(!is_safe_relative_archive_path(Path::new(
            "course/../../outside.txt"
        )));
    }

    #[test]
    fn accepts_simple_relative_archive_member() {
        assert!(is_safe_relative_archive_path(Path::new(
            "course/exercise/file.txt"
        )));
    }
}
