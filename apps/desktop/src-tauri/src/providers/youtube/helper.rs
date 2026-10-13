use crate::app::managed_process::{HelperKind, ManagedProcessOutput, ManagedProcessSpec};
use std::ffi::OsString;
use std::time::Duration;

/// Scan and transcript-inspection output limit.  One production video prints 11.2 MB of caption
/// fields alone (182 languages x 147 signed-URL entries), so 64 MiB leaves about 5x headroom.
/// Reads stay bounded: the supervisor stops capturing at this limit.
pub const MAX_DISCOVERY_STDOUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RECORD_STDOUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FFPROBE_STDOUT_BYTES: usize = 1024 * 1024;
pub const MAX_RETAINED_STDERR_BYTES: usize = 256 * 1024;
const HELPER_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MIN_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const UNKNOWN_DURATION_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DOWNLOAD_DURATION_MULTIPLIER: u64 = 3;
const DOWNLOAD_TIMEOUT_GRACE_SECONDS: u64 = 30 * 60;

/// Typed provider input for the workflow-owned helper port.  No executable
/// path is accepted from React or provider callers.
pub fn invocation(args: Vec<String>, stdout_limit: usize) -> ManagedProcessSpec {
    ManagedProcessSpec::youtube_ytdlp(
        args.into_iter().map(OsString::from).collect(),
        stdout_limit,
        MAX_RETAINED_STDERR_BYTES,
        HELPER_TIMEOUT,
    )
}

/// Long-form downloads use yt-dlp's fragment engine and can legitimately run
/// far longer than discovery or transcript inspection. The timeout scales
/// from the reviewed source duration, retains a bounded fallback when duration
/// is unknown, and still caps a wedged helper process.
pub fn download_invocation(
    args: Vec<String>,
    stdout_limit: usize,
    source_duration_seconds: Option<u64>,
) -> ManagedProcessSpec {
    ManagedProcessSpec::youtube_ytdlp(
        args.into_iter().map(OsString::from).collect(),
        stdout_limit,
        MAX_RETAINED_STDERR_BYTES,
        download_timeout(source_duration_seconds),
    )
}

fn download_timeout(source_duration_seconds: Option<u64>) -> Duration {
    let requested = source_duration_seconds.map_or(UNKNOWN_DURATION_DOWNLOAD_TIMEOUT, |seconds| {
        Duration::from_secs(
            seconds
                .saturating_mul(DOWNLOAD_DURATION_MULTIPLIER)
                .saturating_add(DOWNLOAD_TIMEOUT_GRACE_SECONDS),
        )
    });
    requested.clamp(MIN_DOWNLOAD_TIMEOUT, MAX_DOWNLOAD_TIMEOUT)
}

/// The FFmpeg media check (`-c copy -f null`) reads the whole file, so its
/// time grows with the file: the 5-minute base plus one minute per 2 GiB,
/// capped at six hours.  A 12-hour download of 22-43 GB (about 20-40 GiB) gets
/// roughly 15-25 minutes.
pub fn media_probe_timeout(file_size_bytes: u64) -> Duration {
    const BYTES_PER_EXTRA_MINUTE: u128 = 2 * 1024 * 1024 * 1024;
    const MAX_PROBE_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
    let extra_seconds = u128::from(file_size_bytes) * 60 / BYTES_PER_EXTRA_MINUTE;
    let extra = Duration::from_secs(u64::try_from(extra_seconds).unwrap_or(u64::MAX));
    HELPER_TIMEOUT.saturating_add(extra).min(MAX_PROBE_TIMEOUT)
}

/// `file_size_bytes` is the size of the verified media artifact being probed.
pub fn media_probe_invocation(args: Vec<String>, file_size_bytes: u64) -> ManagedProcessSpec {
    ManagedProcessSpec::youtube_ffmpeg(
        args.into_iter().map(OsString::from).collect(),
        MAX_FFPROBE_STDOUT_BYTES,
        MAX_RETAINED_STDERR_BYTES,
        media_probe_timeout(file_size_bytes),
    )
}

pub fn helper_kind() -> HelperKind {
    HelperKind::YouTubeYtDlp
}

pub fn output_error(output: &ManagedProcessOutput) -> String {
    if output.stderr_truncated {
        "yt-dlp helper failed (diagnostic output truncated)".to_string()
    } else if output.timed_out {
        "yt-dlp helper timed out".to_string()
    } else {
        "yt-dlp helper failed".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_probe_timeout_scales_with_the_file_and_stays_bounded() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(media_probe_timeout(0), HELPER_TIMEOUT);
        // One minute per 2 GiB, proportionally.
        assert_eq!(media_probe_timeout(GIB), HELPER_TIMEOUT + Duration::from_secs(30));
        assert_eq!(media_probe_timeout(2 * GIB), HELPER_TIMEOUT + Duration::from_secs(60));
        // A 12-hour file.
        assert_eq!(
            media_probe_timeout(43 * GIB),
            HELPER_TIMEOUT + Duration::from_secs(21 * 60 + 30)
        );
        // Capped at six hours: the cap is reached at 710 GiB and never exceeded.
        let cap = Duration::from_secs(6 * 60 * 60);
        assert_eq!(media_probe_timeout(710 * GIB), cap);
        assert!(media_probe_timeout(709 * GIB) < cap);
        assert_eq!(media_probe_timeout(10_000 * GIB), cap);
        assert_eq!(media_probe_timeout(u64::MAX), cap);
        // Never below the old fixed timeout, and non-decreasing.
        assert!(media_probe_timeout(1) >= HELPER_TIMEOUT);
        assert!(media_probe_timeout(5 * GIB) <= media_probe_timeout(6 * GIB));
    }

    #[test]
    fn discovery_and_inspection_keep_the_fixed_five_minute_timeout() {
        let spec = invocation(vec!["--version".to_string()], 1024);
        assert_eq!(spec.timeout, HELPER_TIMEOUT);
    }

    #[test]
    fn download_timeout_scales_for_long_form_media_and_stays_bounded() {
        assert_eq!(download_timeout(Some(0)), MIN_DOWNLOAD_TIMEOUT);
        assert_eq!(download_timeout(None), UNKNOWN_DURATION_DOWNLOAD_TIMEOUT);
        assert_eq!(
            download_timeout(Some(12 * 60 * 60)),
            Duration::from_secs((36 * 60 * 60) + DOWNLOAD_TIMEOUT_GRACE_SECONDS)
        );
        assert_eq!(download_timeout(Some(u64::MAX)), MAX_DOWNLOAD_TIMEOUT);
    }
}
