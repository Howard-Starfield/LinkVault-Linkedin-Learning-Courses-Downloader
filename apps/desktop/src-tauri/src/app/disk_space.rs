//! Free-disk-space probe and the pure decision built on it.

use std::io;
use std::path::Path;

const MIB: u64 = 1024 * 1024;
/// Smallest safety margin kept free on top of the bytes still to download.
const MIN_MARGIN_BYTES: u64 = 256 * MIB;

/// Bytes available to the current user on the volume holding `path`.
#[cfg(windows)]
pub fn free_space_bytes(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the two null
    // out-pointers are documented as optional; `available` is a valid u64.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(not(windows))]
pub fn free_space_bytes(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "free disk space is only probed on Windows",
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceDecision {
    /// Nothing to compare (size or free space unknown): do not block.
    Skip,
    Enough,
    Insufficient { needed: u64, free: u64 },
}

/// Margin kept free beyond the remaining download: 5% of the expected size,
/// never below 256 MiB.
pub fn margin_for(expected_total: u64) -> u64 {
    (expected_total / 20).max(MIN_MARGIN_BYTES)
}

/// Decide whether a download can proceed.
///
/// `expected_total` is the size of the item being downloaded (`None` when
/// unknown, which skips the check), `already_present` the `.part` bytes a
/// resume keeps, and `free` the volume's free bytes (`None` when it could not
/// be read, which also skips the check rather than blocking on a probe
/// failure).
pub fn decide_space(
    expected_total: Option<u64>,
    already_present: u64,
    free: Option<u64>,
) -> SpaceDecision {
    let (Some(expected), Some(free)) = (expected_total, free) else {
        return SpaceDecision::Skip;
    };
    let remaining = expected.saturating_sub(already_present);
    let needed = remaining.saturating_add(margin_for(expected));
    if free >= needed {
        SpaceDecision::Enough
    } else {
        SpaceDecision::Insufficient { needed, free }
    }
}

/// Human wording for the Insufficient case, e.g. `"about 1.2 GB"`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn unknown_size_or_unknown_free_space_skips_the_check() {
        assert_eq!(decide_space(None, 0, Some(GIB)), SpaceDecision::Skip);
        assert_eq!(decide_space(Some(GIB), 0, None), SpaceDecision::Skip);
    }

    #[test]
    fn existing_part_bytes_reduce_what_is_needed() {
        // 10 GiB item, 9 GiB already on disk: 1 GiB + 512 MiB margin.
        let needed = GIB + margin_for(10 * GIB);
        assert_eq!(
            decide_space(Some(10 * GIB), 9 * GIB, Some(needed)),
            SpaceDecision::Enough
        );
        assert_eq!(
            decide_space(Some(10 * GIB), 9 * GIB, Some(needed - 1)),
            SpaceDecision::Insufficient {
                needed,
                free: needed - 1
            }
        );
    }

    #[test]
    fn margin_has_a_floor_and_scales_with_size() {
        assert_eq!(margin_for(10 * MIB), 256 * MIB);
        assert_eq!(margin_for(100 * GIB), 5 * GIB);
    }

    #[test]
    fn a_complete_part_file_needs_only_the_margin() {
        assert_eq!(
            decide_space(Some(GIB), 2 * GIB, Some(margin_for(GIB))),
            SpaceDecision::Enough
        );
    }

    #[test]
    fn formats_bytes_for_the_pause_reason() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(3 * GIB / 2), "1.5 GB");
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_probe_reads_a_real_volume() {
        let free = free_space_bytes(&std::env::temp_dir()).unwrap();
        assert!(free > 0);
    }
}
