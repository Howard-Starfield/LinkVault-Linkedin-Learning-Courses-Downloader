use crate::workflow::transient::managed_process::{
    HelperKind, ManagedProcessOutput, ManagedProcessSpec,
};
use std::ffi::OsString;
use std::time::Duration;

pub const MAX_DISCOVERY_STDOUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_RECORD_STDOUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FFPROBE_STDOUT_BYTES: usize = 1024 * 1024;
pub const MAX_RETAINED_STDERR_BYTES: usize = 256 * 1024;
const HELPER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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

pub fn ffprobe_invocation(args: Vec<String>) -> ManagedProcessSpec {
    ManagedProcessSpec::youtube_ffprobe(
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
