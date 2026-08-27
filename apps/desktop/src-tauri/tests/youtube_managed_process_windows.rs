#![cfg(windows)]

use linkvault_lib::managed_process::{
    helper_identity, lock_test_executable, run, run_test, run_test_in_parent, DiscoveryOperation,
    HelperKind, ManagedProcessContext, ManagedProcessError, ManagedProcessOutput,
    ManagedProcessSpec, TestManagedProcessFault, TestManagedProcessSpec, TransientRunControl,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
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

fn stage_reviewed_helpers_beside_current_exe() {
    let binaries = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
    let install = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .expect("test executable directory must be available");
    for name in ["yt-dlp", "deno", "ffmpeg"] {
        let source = binaries.join(format!("{name}-{TARGET_TRIPLE}.exe"));
        let destination = install.join(format!("{name}.exe"));
        if destination.is_file() {
            continue;
        }
        if fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination)
                .unwrap_or_else(|error| panic!("copy {name} sidecar: {error}"));
        }
    }
}

#[test]
#[ignore = "requires the reviewed YouTube helpers beside the test executable"]
fn reviewed_packaged_helpers_pass_the_production_identity_and_launch_path() {
    stage_reviewed_helpers_beside_current_exe();
    let identity = helper_identity(HelperKind::YouTubeYtDlp)
        .expect("the embedded ready lock and packaged helper bytes must agree");
    assert_eq!(
        identity.digest,
        "389e8126d37c83bf1221172cf4b4d9f0fd0c9103e7490e18a62586cdf66407f3"
    );

    let control = TransientRunControl::default();
    let ytdlp = run(
        ManagedProcessSpec::youtube_ytdlp(
            vec![OsString::from("--version")],
            4096,
            4096,
            Duration::from_secs(30),
        ),
        ManagedProcessContext::Run(&control),
    )
    .expect("reviewed yt-dlp must launch through the production supervisor");
    assert!(ytdlp.status.success());
    assert_eq!(ytdlp.stdout.trim(), "2026.08.19");

    let ffmpeg = run(
        ManagedProcessSpec::youtube_ffmpeg(
            vec![OsString::from("-version")],
            64 * 1024,
            4096,
            Duration::from_secs(30),
        ),
        ManagedProcessContext::Run(&control),
    )
    .expect("reviewed FFmpeg must launch through the production supervisor");
    assert!(ffmpeg.status.success());
    assert!(ffmpeg
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

fn helper_launch_root(parent: &Path) -> PathBuf {
    let helper_parent = parent.join("youtube-helper");
    fs::read_dir(&helper_parent)
        .expect("helper capability parent must exist")
        .map(|entry| entry.expect("helper capability entry").path())
        .next()
        .expect("faulted helper capability must retain its launch root")
}

fn run_fault_through_control(
    fault: TestManagedProcessFault,
) -> (
    tempfile::TempDir,
    TransientRunControl,
    PathBuf,
    Option<ManagedProcessError>,
) {
    let temp = tempdir().expect("isolated parent should be available");
    fs::write(temp.path().join("outside-sentinel.txt"), b"keep me")
        .expect("outside sentinel should be writable");
    let control = TransientRunControl::default();
    let observed_error = run_test_in_parent(
        fixture_spec(&["quick"], Duration::from_secs(10)).with_fault(fault),
        temp.path(),
        Some(ManagedProcessContext::Run(&control)),
    )
    .err();
    let root = helper_launch_root(temp.path());
    (temp, control, root, observed_error)
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

fn environment_from(output: &ManagedProcessOutput) -> BTreeMap<String, String> {
    serde_json::from_str(output.stdout.trim()).expect("fixture environment must be JSON")
}

fn assert_environment_scoped(parent: &Path, output: &ManagedProcessOutput) -> PathBuf {
    let values = environment_from(output);
    let root = PathBuf::from(values.get("TEMP").expect("TEMP must be present"));
    assert!(
        root.is_absolute(),
        "TEMP must be absolute: {}",
        root.display()
    );
    assert_eq!(
        values.get("TMP"),
        values.get("TEMP"),
        "TEMP and TMP must use the same capability root"
    );
    assert_eq!(values.get("PATH").map(String::as_str), Some(""));

    let helper_parent = parent.join("youtube-helper");
    assert!(
        root.starts_with(parent),
        "TEMP escaped the explicit test parent: {}",
        root.display()
    );
    assert_eq!(
        root.parent(),
        Some(helper_parent.as_path()),
        "TEMP must be the direct unpredictable launch root"
    );
    assert!(
        root.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("run-")),
        "TEMP must use an unpredictable launch-root name: {}",
        root.display()
    );

    for key in [
        "DENO_DIR",
        "XDG_CACHE_HOME",
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
    ] {
        let value = Path::new(
            values
                .get(key)
                .expect("required helper path must be present"),
        );
        assert!(
            value.starts_with(&root),
            "{key} escaped the capability root: {}",
            value.display()
        );
    }
    root
}

fn assert_launch_roots_clean(parent: &Path) {
    let helper_parent = parent.join("youtube-helper");
    assert!(
        helper_parent.is_dir(),
        "capability parent was not created: {}",
        helper_parent.display()
    );
    let remaining = fs::read_dir(&helper_parent)
        .expect("helper capability parent must remain inspectable after cleanup")
        .map(|entry| entry.expect("helper directory entry").path())
        .collect::<Vec<_>>();
    assert!(
        remaining.is_empty(),
        "helper launch roots leaked: {remaining:?}"
    );
}

#[test]
fn helper_environment_is_capability_scoped_and_normal_cleanup_removes_root() {
    let temp = tempdir().expect("isolated parent should be available");
    let outside_sentinel = temp.path().join("outside-sentinel.txt");
    fs::write(&outside_sentinel, b"keep me").expect("outside sentinel should be writable");

    let output = run_test_in_parent(
        fixture_spec(&["report_environment"], Duration::from_secs(10)),
        temp.path(),
        None,
    )
    .expect("environment fixture should complete through the managed supervisor");
    let root = assert_environment_scoped(temp.path(), &output);
    assert!(
        !root.exists(),
        "managed root should be removed after success"
    );
    assert_launch_roots_clean(temp.path());
    assert_eq!(
        fs::read(&outside_sentinel).expect("outside sentinel must survive cleanup"),
        b"keep me"
    );
}

#[test]
fn concurrent_runs_receive_distinct_capability_roots_and_clean_them() {
    let temp = tempdir().expect("isolated parent should be available");
    let mut workers = Vec::new();
    for _ in 0..4 {
        let parent = temp.path().to_path_buf();
        workers.push(thread::spawn(move || {
            run_test_in_parent(
                fixture_spec(&["report_environment"], Duration::from_secs(10)),
                &parent,
                None,
            )
            .expect("concurrent environment fixture should complete")
        }));
    }

    let mut roots = HashSet::new();
    for worker in workers {
        let output = worker.join().expect("concurrent worker must not panic");
        let root = assert_environment_scoped(temp.path(), &output);
        assert!(roots.insert(root), "concurrent runs reused a launch root");
    }
    assert_launch_roots_clean(temp.path());
}

#[test]
fn injected_startup_faults_clean_capability_roots_and_preserve_outside_sentinel() {
    let cases = [
        (
            TestManagedProcessFault::BeforeJobAssignment,
            "before-assignment",
        ),
        (TestManagedProcessFault::ReaderStartup, "reader-startup"),
        (TestManagedProcessFault::Resume, "resume"),
    ];

    for (fault, label) in cases {
        let temp = tempdir().expect("isolated parent should be available");
        let outside_sentinel = temp.path().join(format!("{label}-sentinel.txt"));
        fs::write(&outside_sentinel, b"keep me").expect("outside sentinel should be writable");
        let spec = fixture_spec_os(
            vec![
                OsString::from("write_marker"),
                outside_sentinel.as_os_str().to_os_string(),
            ],
            Duration::from_secs(10),
        )
        .with_fault(fault);

        let error = run_test_in_parent(spec, temp.path(), None)
            .expect_err("injected startup fault must fail closed");
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
            TestManagedProcessFault::None
            | TestManagedProcessFault::CreationAfterRoot
            | TestManagedProcessFault::CreationBeforeRoot
            | TestManagedProcessFault::CleanupAfterSupervisor => unreachable!(),
        }
        assert_eq!(
            fs::read(&outside_sentinel).expect("outside sentinel must remain present"),
            b"keep me",
            "{label} fault allowed child code to overwrite the outside sentinel"
        );
        assert_launch_roots_clean(temp.path());
    }
}

#[test]
fn cleanup_faults_without_runtime_context_are_rejected_before_admission() {
    let faults = [
        TestManagedProcessFault::CreationAfterRoot,
        TestManagedProcessFault::CreationBeforeRoot,
        TestManagedProcessFault::CleanupAfterSupervisor,
    ];

    for fault in faults {
        let temp = tempdir().expect("isolated parent should be available");
        let error = run_test_in_parent(
            fixture_spec(&["quick"], Duration::from_secs(10)).with_fault(fault),
            temp.path(),
            None,
        )
        .expect_err("cleanup fault without an owner must fail before admission");
        assert!(matches!(error, ManagedProcessError::Integrity(_)));
        assert!(
            !temp.path().join("youtube-helper").exists(),
            "context-less {fault:?} must not create a helper root"
        );

        let error = run_test(
            fixture_spec(&["quick"], Duration::from_secs(10)).with_fault(fault),
            None,
        )
        .expect_err("parentless cleanup fault must fail before capability creation");
        assert!(matches!(error, ManagedProcessError::Integrity(_)));
    }
}

#[test]
fn post_root_creation_cleanup_failure_registers_identity_held_recovery_for_run() {
    let (temp, control, root, observed_error) =
        run_fault_through_control(TestManagedProcessFault::CreationAfterRoot);
    let sentinel = temp.path().join("outside-sentinel.txt");

    assert!(matches!(
        observed_error,
        Some(ManagedProcessError::HelperTemp(_))
    ));
    assert!(root.exists(), "faulted creation root must remain for retry");
    assert!(
        fs::rename(&root, root.with_file_name("replaced-root")).is_err(),
        "the retained verifier must hold the admitted root identity"
    );
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");

    assert!(control.retry_cleanup_verifiers());
    assert!(
        !root.exists(),
        "the later exit-equivalent retry must clean root"
    );
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");
}

#[test]
fn pre_root_creation_cleanup_uncertainty_registers_permanent_unproven_run() {
    let (temp, control, root, observed_error) =
        run_fault_through_control(TestManagedProcessFault::CreationBeforeRoot);
    let sentinel = temp.path().join("outside-sentinel.txt");

    assert!(matches!(
        observed_error,
        Some(ManagedProcessError::HelperTemp(_))
    ));
    assert!(
        root.exists(),
        "unproven creation root must not be rediscovered"
    );
    assert!(!control.retry_cleanup_verifiers());
    assert!(
        root.exists(),
        "permanent uncertainty must never authorize cleanup"
    );
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");
    assert!(!control.retry_cleanup_verifiers());
    assert!(
        root.exists(),
        "a repeated exit-equivalent retry remains fail closed"
    );
}

#[test]
fn post_supervisor_cleanup_failure_registers_recovery_through_real_run_path() {
    let (temp, control, root, observed_error) =
        run_fault_through_control(TestManagedProcessFault::CleanupAfterSupervisor);
    let sentinel = temp.path().join("outside-sentinel.txt");

    assert!(matches!(
        observed_error,
        Some(ManagedProcessError::HelperTemp(_))
    ));
    assert!(
        root.exists(),
        "post-supervisor cleanup fault must retain root"
    );
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");
    assert!(control.retry_cleanup_verifiers());
    assert!(!root.exists(), "retry must remove only the owned root");
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");
}

#[test]
fn post_supervisor_cleanup_failure_registers_and_retries_through_discovery_path() {
    let temp = tempdir().expect("isolated parent should be available");
    let sentinel = temp.path().join("outside-sentinel.txt");
    fs::write(&sentinel, b"keep me").expect("outside sentinel should be writable");
    let operation = DiscoveryOperation::new();
    let output = run_test_in_parent(
        fixture_spec(&["quick"], Duration::from_secs(10))
            .with_fault(TestManagedProcessFault::CleanupAfterSupervisor),
        temp.path(),
        Some(ManagedProcessContext::Discovery(&operation)),
    )
    .expect("completed discovery must keep helper output when only cleanup is unproven");
    assert!(output.status.success());
    assert!(!operation.cancellation_requested());
    let root = helper_launch_root(temp.path());
    assert!(root.exists());

    assert!(operation.retry_cleanup_verifiers());
    assert!(!root.exists(), "discovery retry must remove the owned root");
    assert_eq!(fs::read(&sentinel).unwrap_or_default(), b"keep me");
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
    let output = run_test(fixture_spec_os(args, Duration::from_secs(10)), None)
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
    let output = run_test(spec, None).expect("noisy fixture should complete");
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
    let worker =
        thread::spawn(move || run_test(spec, Some(ManagedProcessContext::Run(&worker_control))));
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
        let error = run_test(spec, None).expect_err("injected supervisor failure must fail closed");
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
            TestManagedProcessFault::None
            | TestManagedProcessFault::CreationAfterRoot
            | TestManagedProcessFault::CreationBeforeRoot
            | TestManagedProcessFault::CleanupAfterSupervisor => unreachable!(),
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
        let worker = thread::spawn(move || {
            run_test(spec, Some(ManagedProcessContext::Run(&worker_control)))
        });
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
