//! Shared desktop-shell helpers.

use std::path::Path;
use std::process::Command;

pub fn open_folder_in_explorer(folder: &Path) -> Result<(), String> {
    if !folder.is_dir() {
        return Err(format!(
            "Folder does not exist yet: {}",
            folder.to_string_lossy()
        ));
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(folder)
            .spawn()
            .map_err(|error| format!("Failed to open File Explorer: {error}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("open")
            .arg(folder)
            .spawn()
            .or_else(|_| Command::new("xdg-open").arg(folder).spawn())
            .map_err(|error| format!("Failed to open folder: {error}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::open_folder_in_explorer;

    #[test]
    fn missing_directory_is_rejected_without_spawning() {
        let path = std::env::temp_dir().join(format!(
            "linkvault-missing-folder-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let error = open_folder_in_explorer(&path).unwrap_err();
        assert!(error.contains("does not exist yet"));
    }
}
