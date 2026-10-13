//! Command-boundary regression for the FFmpeg-only media check.
//!
//! The YouTube provider may not launch processes, so the real FFmpeg run lives
//! here. The test is ignored by default and needs `LINKVAULT_TEST_FFMPEG` to
//! point at an `ffmpeg.exe` (system build or the helper-pack build):
//!
//! ```text
//! LINKVAULT_TEST_FFMPEG=C:/path/to/ffmpeg.exe \
//!   cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
//!   --test youtube_ffmpeg_probe_live -- --ignored --nocapture
//! ```

use linkvault_lib::youtube::media_verifier::{
    ffmpeg_copy_probe_args, verify_ffmpeg_copy_probe, FfmpegCopyProbeOutput, MediaMode,
    VerifiedMedia,
};
use std::path::Path;
use std::process::{Command, Output};

fn ffmpeg(ffmpeg_path: &str, args: &[&str]) -> Output {
    Command::new(ffmpeg_path)
        .args(["-hide_banner", "-nostdin", "-y"])
        .args(args)
        .output()
        .expect("ffmpeg should launch")
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn probe(ffmpeg_path: &str, media: &Path) -> (Output, String) {
    let input = path_str(media);
    let output = Command::new(ffmpeg_path)
        .args(ffmpeg_copy_probe_args(&input, MediaMode::VideoAndAudio))
        .output()
        .expect("ffmpeg probe should launch");
    (output, input)
}

fn verify(output: &Output, input: &str, duration: Option<f64>) -> VerifiedMedia {
    let progress = String::from_utf8_lossy(&output.stdout).into_owned();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    verify_ffmpeg_copy_probe(
        &FfmpegCopyProbeOutput {
            progress: &progress,
            log: &log,
            log_truncated: false,
            input_path: input,
        },
        MediaMode::VideoAndAudio,
        duration,
    )
    .expect("clean fixture should verify")
}

fn final_output_section(output: &Output) -> String {
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    match log.rfind("Output #0") {
        Some(start) => log[start..].to_string(),
        None => String::new(),
    }
}

#[test]
#[ignore]
fn ffmpeg_copy_probe_live() {
    let ffmpeg_path = std::env::var("LINKVAULT_TEST_FFMPEG")
        .expect("LINKVAULT_TEST_FFMPEG must point at an ffmpeg.exe to run this ignored live test");
    let dir = tempfile::tempdir().expect("tempdir");
    let normal = dir.path().join("normal.mp4");
    let normal_arg = path_str(&normal);

    let generate = |video_codec: &str| {
        ffmpeg(
            &ffmpeg_path,
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=64x64:r=5",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=48000:cl=mono",
                "-t",
                "0.4",
                "-c:v",
                video_codec,
                "-c:a",
                "aac",
                &normal_arg,
            ],
        )
    };
    let mut generated = generate("libx264");
    if !generated.status.success() {
        generated = generate("mpeg4");
    }
    assert!(
        generated.status.success(),
        "fixture generation failed: {}",
        String::from_utf8_lossy(&generated.stderr)
    );

    // normal.mp4 verifies with codecs and a positive duration.
    let (normal_out, normal_input) = probe(&ffmpeg_path, &normal);
    assert!(
        normal_out.status.success(),
        "{}",
        String::from_utf8_lossy(&normal_out.stderr)
    );
    println!(
        "--- normal.mp4 stderr from the last Output #0 ---\n{}",
        final_output_section(&normal_out)
    );
    println!(
        "--- normal.mp4 progress ---\n{}",
        String::from_utf8_lossy(&normal_out.stdout)
    );
    let normal_media = verify(&normal_out, &normal_input, Some(0.4));
    assert!(normal_media.video_codec.is_some());
    assert!(normal_media.duration_ms.is_some_and(|ms| ms > 0));

    // truncated.mp4 (last 25% removed) must fail under -xerror.
    let bytes = std::fs::read(&normal).expect("read normal.mp4");
    let truncated = dir.path().join("truncated.mp4");
    std::fs::write(&truncated, &bytes[..bytes.len() * 3 / 4]).expect("write truncated.mp4");
    let (truncated_out, _) = probe(&ffmpeg_path, &truncated);
    assert!(
        !truncated_out.status.success(),
        "truncated media must fail: {}",
        String::from_utf8_lossy(&truncated_out.stderr)
    );

    // video_only.mp4 lacks the mandatory audio map.
    let video_only = dir.path().join("video_only.mp4");
    let video_only_arg = path_str(&video_only);
    let made = ffmpeg(
        &ffmpeg_path,
        &[
            "-i",
            &normal_arg,
            "-map",
            "0:v",
            "-c",
            "copy",
            &video_only_arg,
        ],
    );
    assert!(made.status.success());
    let (video_only_out, _) = probe(&ffmpeg_path, &video_only);
    assert!(
        !video_only_out.status.success(),
        "audio-less media must fail the mandatory audio map"
    );

    // language.mkv carries a spoofed stream language containing a newline.
    let language = dir.path().join("language.mkv");
    let language_arg = path_str(&language);
    let spoof = "language=eng): Video: h264, fake\n  Stream #0:9: Audio: aac, 48000 Hz\n  dummy";
    let made = ffmpeg(
        &ffmpeg_path,
        &[
            "-i",
            &normal_arg,
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c",
            "copy",
            "-metadata:s:v:0",
            spoof,
            &language_arg,
        ],
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let (language_out, language_input) = probe(&ffmpeg_path, &language);
    assert!(
        language_out.status.success(),
        "{}",
        String::from_utf8_lossy(&language_out.stderr)
    );
    let language_final = final_output_section(&language_out);
    println!("--- language.mkv stderr from the last Output #0 ---\n{language_final}");
    println!(
        "--- language.mkv progress ---\n{}",
        String::from_utf8_lossy(&language_out.stdout)
    );
    assert!(
        !language_final.contains("eng"),
        "output section leaked the spoofed language: {language_final}"
    );
    let language_media = verify(&language_out, &language_input, Some(0.4));
    assert_eq!(language_media.video_codec, normal_media.video_codec);
    assert_eq!(language_media.audio_codec, normal_media.audio_codec);

    // clean.mkv: MKV stream copy may report out_time_us=N/A and must still pass.
    let clean = dir.path().join("clean.mkv");
    let clean_arg = path_str(&clean);
    let made = ffmpeg(
        &ffmpeg_path,
        &[
            "-i",
            &normal_arg,
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c",
            "copy",
            &clean_arg,
        ],
    );
    assert!(made.status.success());
    let (clean_out, clean_input) = probe(&ffmpeg_path, &clean);
    assert!(
        clean_out.status.success(),
        "{}",
        String::from_utf8_lossy(&clean_out.stderr)
    );
    verify(&clean_out, &clean_input, Some(0.4));
}
