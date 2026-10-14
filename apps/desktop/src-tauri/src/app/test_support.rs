//! Test-only helpers shared by provider tests that must not spawn processes
//! themselves (provider sources may not launch processes, even in tests).

use std::path::Path;

/// Make `link` point at the directory `target`: a junction on Windows (no
/// privilege needed) and a symlink elsewhere. Returns false when the platform
/// refuses, so callers can skip instead of failing.
pub(crate) fn link_dir(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
