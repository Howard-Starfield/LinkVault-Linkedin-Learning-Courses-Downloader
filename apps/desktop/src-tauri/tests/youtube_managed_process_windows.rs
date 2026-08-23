#![cfg(windows)]

use linkvault_lib::workflow::transient::managed_process::{
    helper_identity, lock_test_executable, run, run_test, HelperKind, ManagedProcessError,
    ManagedProcessSpec, TestManagedProcessFault, TestManagedProcessSpec,
};
use linkvault_lib::workflow::transient::TransientRunControl;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_youtube_process_fixture"))
}

#[test]
#[ignore = "requires the reviewed YouTube helpers beside the test executable"]
fn reviewed_packaged_helpers_pass_the_production_identity_and_launch_path() {
    let identity = helper_identity(HelperKind::YouTubeYtDlp)
        .expect("the embedded ready lock and packaged helper bytes must agree");
    assert_eq!(
        identity.digest,
        "f2eb38349e71bd05b8da27807bc82e5eecb204cbf8b335952276bc1786527b7c"
    );

    let ytdlp = run(
        ManagedProcessSpec::youtube_ytdlp(
            vec![OsString::from("--version")],
            4096,
            4096,
            Duration::from_secs(30),
        ),
        None,
        None,
    )
    .expect("reviewed yt-dlp must launch through the production supervisor");
    assert!(ytdlp.status.success());
    assert_eq!(ytdlp.stdout.trim(), "2026.08.19");

    let ffprobe = run(
        ManagedProcessSpec::youtube_ffprobe(
            vec![OsString::from("-version")],
            64 * 1024,
            4096,
            Duration::from_secs(30),
        ),
        None,
        None,
    )
    .expect("reviewed FFprobe must launch through the production supervisor");
    assert!(ffprobe.status.success());
    assert!(ffprobe
        .stdout
        .lines()
        .next()
        .is_some_and(|line| line.contains("n9.0.1-6-g9d4ca21220-20260820")));
}

fn fixture_spec(args: &[&str], timeout: Duration) -> TestManagedProcessSpec {
    ManagedProcessSpec::for_test(
        fixture_path(),
        args.iter().map(OsString::from).collect(),
        256 * 1024,
        256 * 1024,
        timeout,
    )
}

fn fixture_spec_os(args: Vec<OsString>, timeout: Duration) -> TestManagedProcessSpec {
    ManagedProcessSpec::for_test(fixture_path(), args, 256 * 1024, 256 * 1024, timeout)
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while !path.exists() && started.elapsed() < timeout {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(path.exists(), "fixture did not create {}", path.display());
}

fn sha256(path: &Path) -> String {
    let bytes = fs::read(path).expect("fixture bytes");
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn verified_executable_handle_blocks_tamper_and_replacement() {
    let temp = tempdir().unwrap();
    let candidate = temp.path().join("trusted-helper.exe");
    fs::copy(fixture_path(), &candidate).unwrap();
    let digest = sha256(&candidate);
    assert!(matches!(
        lock_test_executable(candidate.clone(), &"0".repeat(64)),
        Err(ManagedProcessError::Integrity(_))
    ));

    let guard = lock_test_executable(candidate.clone(), &digest).unwrap();
    assert!(fs::write(&candidate, b"tampered").is_err());
    assert!(fs::rename(&candidate, temp.path().join("replacement.exe")).is_err());
    assert_eq!(sha256(&candidate), digest);
    drop(guard);
    fs::write(&candidate, b"released").unwrap();
}

#[test]
fn windows_argv_serializer_round_trips_real_process_arguments() {
    let expected = vec![
        "".to_string(),
        "plain".to_string(),
        "two words".to_string(),
        "a\\\"b".to_string(),
        "C:\\folder name\\".to_string(),
    ];
    let mut args = vec![OsString::from("echo_args")];
    args.extend(expected.iter().map(OsString::from));
    let output = run_test(fixture_spec_os(args, Duration::from_secs(10)), None, None)
        .expect("quoted fixture process should complete");
    assert!(output.status.success());
    let actual = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str::<String>(line).expect("fixture line must be JSON"))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn noisy_stdout_and_stderr_are_drained_and_bounded() {
    let mut spec = fixture_spec(&["noisy", "1048576"], Duration::from_secs(20));
    spec.spec.stdout_limit = 4096;
    spec.spec.stderr_limit = 3072;
    let output = run_test(spec, None, None).expect("noisy fixture should complete");
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 4096);
    assert_eq!(output.stderr.len(), 3072);
    assert!(output.stdout_truncated);
    assert!(output.stderr_truncated);
}

#[test]
fn invalid_utf8_machine_output_is_rejected() {
    let error = run_test(
        fixture_spec(&["invalid_utf8"], Duration::from_secs(10)),
        None,
        None,
    )
    .expect_err("invalid UTF-8 stdout must fail closed");
    assert_eq!(error, ManagedProcessError::InvalidUtf8);
}

#[test]
fn cancellation_terminates_direct_child_and_grandchild() {
    let temp = tempdir().expect("temporary directory should be available");
    let ready = temp.path().join("ready.txt");
    let survivor = temp.path().join("survivor.txt");
    let control = Arc::new(TransientRunControl::default());
    let worker_control = Arc::clone(&control);
    let spec = fixture_spec_os(
        vec![
            OsString::from("grandchild"),
            ready.as_os_str().to_os_string(),
            survivor.as_os_str().to_os_string(),
            OsString::from("1500"),
        ],
        Duration::from_secs(30),
    );
    let worker = thread::spawn(move || run_test(spec, Some(&worker_control), None));
    wait_for_path(&ready, Duration::from_secs(10));
    control.request_cancel();
    let output = worker
        .join()
        .expect("managed-process test thread should not panic")
        .expect("cancellation should be a typed output, not a supervisor failure");
    assert!(output.cancelled);
    assert!(!output.timed_out);
    thread::sleep(Duration::from_millis(1800));
    assert!(
        !survivor.exists(),
        "grandchild escaped the kill-on-close Job Object"
    );
}

#[test]
fn timeout_terminates_direct_child_and_grandchild() {
    let temp = tempdir().expect("temporary directory should be available");
    let ready = temp.path().join("ready.txt");
    let survivor = temp.path().join("survivor.txt");
    let output = run_test(
        fixture_spec_os(
            vec![
                OsString::from("grandchild"),
                ready.as_os_str().to_os_string(),
                survivor.as_os_str().to_os_string(),
                OsString::from("3500"),
            ],
            Duration::from_secs(2),
        ),
        None,
        None,
    )
    .expect("timeout should be a typed output, not a supervisor failure");
    assert!(output.timed_out);
    assert!(!output.cancelled);
    assert!(ready.exists(), "fixture grandchild was never admitted");
    thread::sleep(Duration::from_millis(3800));
    assert!(
        !survivor.exists(),
        "grandchild survived the timed-out Job Object"
    );
}

#[test]
fn injected_pre_assignment_reader_and_resume_failures_never_execute_child_code() {
    let cases = [
        (
            TestManagedProcessFault::BeforeJobAssignment,
            "before-assignment.txt",
            "containment",
        ),
        (
            TestManagedProcessFault::ReaderStartup,
            "reader-startup.txt",
            "reader",
        ),
        (TestManagedProcessFault::Resume, "resume.txt", "resume"),
    ];
    let temp = tempdir().expect("temporary directory should be available");
    for (fault, name, label) in cases {
        let marker = temp.path().join(name);
        let spec = fixture_spec_os(
            vec![
                OsString::from("write_marker"),
                marker.as_os_str().to_os_string(),
            ],
            Duration::from_secs(10),
        )
        .with_fault(fault);
        let error =
            run_test(spec, None, None).expect_err("injected supervisor failure must fail closed");
        match fault {
            TestManagedProcessFault::BeforeJobAssignment => {
                assert!(matches!(error, ManagedProcessError::ProcessContainment(_)));
            }
            TestManagedProcessFault::ReaderStartup => {
                assert!(matches!(error, ManagedProcessError::Reader(_)));
            }
            TestManagedProcessFault::Resume => {
                assert!(matches!(error, ManagedProcessError::Start(_)));
            }
            TestManagedProcessFault::None => unreachable!(),
        }
        thread::sleep(Duration::from_millis(100));
        assert!(!marker.exists(), "{label} fault resumed the child");
    }
}

#[test]
fn cancellation_completion_races_settle_without_supervisor_errors() {
    for iteration in 0..12 {
        let control = Arc::new(TransientRunControl::default());
        let worker_control = Arc::clone(&control);
        let spec = fixture_spec(&["sleep", "5"], Duration::from_secs(10));
        let worker = thread::spawn(move || run_test(spec, Some(&worker_control), None));
        if iteration % 3 == 0 {
            thread::yield_now();
        } else {
            thread::sleep(Duration::from_millis((iteration % 4) as u64));
        }
        control.request_cancel();
        let output = worker
            .join()
            .expect("race test thread should not panic")
            .expect("completion/cancellation race should settle as output");
        assert!(output.cancelled || output.status.success());
        assert!(!(output.cancelled && output.timed_out));
    }
}
