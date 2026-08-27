use crate::app::managed_process::{HelperKind, ManagedProcessOutput, ManagedProcessSpec};
use std::ffi::OsString;
use std::time::Duration;

pub const MAX_DISCOVERY_STDOUT_BYTES: usize = 32 * 1024 * 1024;
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

pub fn media_probe_invocation(args: Vec<String>) -> ManagedProcessSpec {
    ManagedProcessSpec::youtube_ffmpeg(
        args.into_iter().map(OsString::from).collect(),
        MAX_FFPROBE_STDOUT_BYTES,
        MAX_RETAINED_STDERR_BYTES,
        HELPER_TIMEOUT,
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
