use super::{
    read_bounded, ManagedProcessError, ManagedProcessOutput, ManagedProcessSpec,
    TestFaultSelection, TransientRunControl,
};
use std::collections::BTreeMap;
use std::ffi::{c_void, OsStr, OsString};
use std::fs::{self, File};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

const CANCEL_EXIT_CODE: u32 = 0xC000_013A;
const START_FAILURE_EXIT_CODE: u32 = 0xC000_0142;
const WAIT_INTERVAL_MS: u32 = 20;
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const TREE_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE, label: &str) -> Result<Self, ManagedProcessError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(ManagedProcessError::Start(format!(
                "{label} failed with Windows error {}",
                unsafe { GetLastError() }
            )));
        }
        Ok(Self(handle))
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_raw(mut self) -> HANDLE {
        let raw = self.0;
        self.0 = null_mut();
        raw
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

struct AttributeList {
    _storage: Vec<usize>,
    list: *mut c_void,
}

impl AttributeList {
    fn with_inherited_handles(handles: &mut [HANDLE]) -> Result<Self, ManagedProcessError> {
        let mut size = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        }
        if size == 0 {
            return Err(last_start_error(
                "InitializeProcThreadAttributeList size query",
            ));
        }
        let word_count = size.div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; word_count];
        let list = storage.as_mut_ptr().cast::<c_void>();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(last_start_error("InitializeProcThreadAttributeList"));
        }
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                handles.as_mut_ptr().cast::<c_void>(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            unsafe {
                DeleteProcThreadAttributeList(list);
            }
            return Err(last_start_error("UpdateProcThreadAttribute(handle list)"));
        }
        Ok(Self {
            _storage: storage,
            list,
        })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.list.is_null() {
            unsafe {
                DeleteProcThreadAttributeList(self.list);
            }
        }
    }
}

struct ChildPipes {
    stdout_read: File,
    stdout_write: OwnedHandle,
    stderr_read: File,
    stderr_write: OwnedHandle,
    stdin_read: OwnedHandle,
}

type BoundedReadResult = std::io::Result<(Vec<u8>, bool)>;

struct ReaderTask {
    label: &'static str,
    receiver: Receiver<BoundedReadResult>,
    thread: JoinHandle<()>,
}

pub(super) fn run(
    executable: PathBuf,
    spec: ManagedProcessSpec,
    control: Option<&TransientRunControl>,
    discovery_cancel: Option<&AtomicBool>,
    fault: TestFaultSelection,
) -> Result<ManagedProcessOutput, ManagedProcessError> {
    ensure_absolute_executable(&executable)?;
    let job = create_kill_on_close_job()?;
    let pipes = create_child_pipes()?;
    let mut inherited = [
        pipes.stdin_read.raw(),
        pipes.stdout_write.raw(),
        pipes.stderr_write.raw(),
    ];
    let attributes = AttributeList::with_inherited_handles(&mut inherited)?;
    let temp_directory = process_temp_directory()?;
    let environment = build_environment_block(&temp_directory);
    let application = to_wide_nul(executable.as_os_str());
    let mut command_line = build_command_line(&executable, &spec.args);
    let current_directory = executable
        .parent()
        .map(|path| to_wide_nul(path.as_os_str()))
        .unwrap_or_else(|| vec![0]);
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = pipes.stdin_read.raw();
    startup.StartupInfo.hStdOutput = pipes.stdout_write.raw();
    startup.StartupInfo.hStdError = pipes.stderr_write.raw();
    startup.lpAttributeList = attributes.list;
    let mut process_info = PROCESS_INFORMATION::default();
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast::<c_void>(),
            current_directory.as_ptr(),
            &startup.StartupInfo,
            &mut process_info,
        )
    };
    drop(attributes);
    if created == 0 {
        return Err(last_start_error("CreateProcessW"));
    }

    let process = match OwnedHandle::new(
        process_info.hProcess,
        "CreateProcessW process handle",
    ) {
        Ok(process) => process,
        Err(error) => {
            close_if_valid(process_info.hThread);
            return Err(error);
        }
    };
    let thread_handle = match OwnedHandle::new(
        process_info.hThread,
        "CreateProcessW primary thread handle",
    ) {
        Ok(thread_handle) => thread_handle,
        Err(error) => {
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(
                    process.raw(),
                    START_FAILURE_EXIT_CODE,
                );
                WaitForSingleObject(process.raw(), 5_000);
            }
            return Err(error);
        }
    };

    if fault == TestFaultSelection::BeforeJobAssignment {
        terminate_suspended_child(&job, &process);
        return Err(ManagedProcessError::ProcessContainment(
            "injected failure before Job Object assignment".to_string(),
        ));
    }

    if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
        let error = last_containment_error("AssignProcessToJobObject");
        terminate_suspended_child(&job, &process);
        return Err(error);
    }
    let mut in_job = 0;
    if unsafe { IsProcessInJob(process.raw(), job.raw(), &mut in_job) } == 0 {
        let error = last_containment_error("IsProcessInJob verification");
        terminate_suspended_child(&job, &process);
        return Err(error);
    }
    if in_job == 0 {
        terminate_suspended_child(&job, &process);
        return Err(ManagedProcessError::ProcessContainment(
            "the suspended helper was not present in its Job Object".to_string(),
        ));
    }

    let ChildPipes {
        stdout_read,
        stdout_write,
        stderr_read,
        stderr_write,
        stdin_read,
    } = pipes;
    drop(stdout_write);
    drop(stderr_write);
    drop(stdin_read);

    if fault == TestFaultSelection::ReaderStartup {
        terminate_suspended_child(&job, &process);
        return Err(ManagedProcessError::Reader(
            "injected reader startup failure".to_string(),
        ));
    }

    let stdout_task = match spawn_reader("stdout", stdout_read, spec.stdout_limit) {
        Ok(task) => task,
        Err(error) => {
            terminate_suspended_child(&job, &process);
            return Err(error);
        }
    };
    let stderr_task = match spawn_reader("stderr", stderr_read, spec.stderr_limit) {
        Ok(task) => task,
        Err(error) => {
            terminate_suspended_child(&job, &process);
            let _ = finish_reader(stdout_task);
            return Err(error);
        }
    };

    let mut terminal_error = None;
    let mut timed_out = false;
    let mut cancelled = false;
    if fault == TestFaultSelection::Resume {
        terminal_error = Some(ManagedProcessError::Start(
            "injected primary-thread resume failure".to_string(),
        ));
        terminate_suspended_child(&job, &process);
    } else {
        let resume_result = unsafe { ResumeThread(thread_handle.raw()) };
        if resume_result == u32::MAX {
            terminal_error = Some(last_start_error("ResumeThread"));
            terminate_suspended_child(&job, &process);
        }
    }
    drop(thread_handle);

    if terminal_error.is_none() {
        let started = Instant::now();
        loop {
            if control.is_some_and(TransientRunControl::is_cancelled)
                || discovery_cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                cancelled = true;
                if let Err(error) = terminate_job(&job, CANCEL_EXIT_CODE) {
                    terminal_error = Some(error);
                }
                break;
            }
            if started.elapsed() >= spec.timeout {
                timed_out = true;
                if let Err(error) = terminate_job(&job, CANCEL_EXIT_CODE) {
                    terminal_error = Some(error);
                }
                break;
            }
            match unsafe { WaitForSingleObject(process.raw(), WAIT_INTERVAL_MS) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT => {}
                WAIT_FAILED => {
                    let error = last_wait_error("WaitForSingleObject");
                    let _ = terminate_job(&job, START_FAILURE_EXIT_CODE);
                    terminal_error = Some(error);
                    break;
                }
                value => {
                    let _ = terminate_job(&job, START_FAILURE_EXIT_CODE);
                    terminal_error = Some(ManagedProcessError::Wait(format!(
                        "unexpected wait result {value}"
                    )));
                    break;
                }
            }
        }
    }

    if let Err(error) = wait_for_job_empty(&job, TREE_JOIN_TIMEOUT) {
        let _ = terminate_job(&job, START_FAILURE_EXIT_CODE);
        let _ = wait_for_job_empty(&job, TREE_JOIN_TIMEOUT);
        if terminal_error.is_none() {
            terminal_error = Some(error);
        }
    }

    let stdout_result = finish_reader(stdout_task);
    let stderr_result = finish_reader(stderr_task);
    let (stdout_bytes, stdout_truncated) = stdout_result?;
    let (stderr_bytes, stderr_truncated) = stderr_result?;

    let mut exit_code = 0u32;
    if unsafe { GetExitCodeProcess(process.raw(), &mut exit_code) } == 0
        && terminal_error.is_none()
    {
        terminal_error = Some(last_wait_error("GetExitCodeProcess"));
    }
    if let Some(error) = terminal_error {
        return Err(error);
    }

    let stdout =
        String::from_utf8(stdout_bytes).map_err(|_| ManagedProcessError::InvalidUtf8)?;
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    Ok(ManagedProcessOutput {
        status: std::process::ExitStatus::from_raw(exit_code),
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
        cancelled,
    })
}

fn spawn_reader(
    label: &'static str,
    reader: File,
    limit: usize,
) -> Result<ReaderTask, ManagedProcessError> {
    let (sender, receiver) = mpsc::channel();
    let thread = thread::Builder::new()
        .name(format!("linkvault-youtube-{label}"))
        .spawn(move || {
            let _ = sender.send(read_bounded(reader, limit));
        })
        .map_err(|error| ManagedProcessError::Reader(error.to_string()))?;
    Ok(ReaderTask {
        label,
        receiver,
        thread,
    })
}

fn finish_reader(task: ReaderTask) -> Result<(Vec<u8>, bool), ManagedProcessError> {
    let ReaderTask {
        label,
        receiver,
        thread,
    } = task;
    let received = match receiver.recv_timeout(READER_JOIN_TIMEOUT) {
        Ok(result) => Ok(result),
        Err(RecvTimeoutError::Disconnected) => Err(ManagedProcessError::Reader(format!(
            "{label} reader disconnected before reporting a result"
        ))),
        Err(RecvTimeoutError::Timeout) => {
            unsafe {
                CancelSynchronousIo(thread.as_raw_handle() as HANDLE);
            }
            receiver.recv().map_err(|error| {
                ManagedProcessError::Reader(format!(
                    "{label} reader did not stop after I/O cancellation: {error}"
                ))
            })
        }
    };
    thread.join().map_err(|_| {
        ManagedProcessError::Reader(format!("{label} reader thread panicked"))
    })?;
    let result = received?;
    result.map_err(|error| ManagedProcessError::Reader(format!("{label}: {error}")))
}

fn close_if_valid(handle: HANDLE) {
    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(handle);
        }
    }
}

fn ensure_absolute_executable(path: &Path) -> Result<(), ManagedProcessError> {
    if !path.is_absolute() {
        return Err(ManagedProcessError::Start(
            "managed executable path was not absolute".to_string(),
        ));
    }
    Ok(())
}

fn create_kill_on_close_job() -> Result<OwnedHandle, ManagedProcessError> {
    let job =
        OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) }, "CreateJobObjectW")
            .map_err(|error| ManagedProcessError::ProcessContainment(error.to_string()))?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let applied = unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if applied == 0 {
        return Err(last_containment_error(
            "SetInformationJobObject(kill-on-close)",
        ));
    }
    Ok(job)
}

fn create_child_pipes() -> Result<ChildPipes, ManagedProcessError> {
    let (stdout_read, stdout_write) = create_inheritable_pipe("stdout")?;
    let (stderr_read, stderr_write) = create_inheritable_pipe("stderr")?;
    let mut security = inheritable_security_attributes();
    let nul = to_wide_nul(OsStr::new("NUL"));
    let stdin_read = OwnedHandle::new(
        unsafe {
            CreateFileW(
                nul.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &mut security,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        },
        "CreateFileW(NUL)",
    )?;
    Ok(ChildPipes {
        stdout_read,
        stdout_write,
        stderr_read,
        stderr_write,
        stdin_read,
    })
}

fn create_inheritable_pipe(
    label: &str,
) -> Result<(File, OwnedHandle), ManagedProcessError> {
    let mut security = inheritable_security_attributes();
    let mut read_handle: HANDLE = null_mut();
    let mut write_handle: HANDLE = null_mut();
    if unsafe { CreatePipe(&mut read_handle, &mut write_handle, &mut security, 0) } == 0 {
        return Err(last_start_error(&format!("CreatePipe({label})")));
    }
    let read = OwnedHandle::new(read_handle, &format!("{label} read handle"))?;
    let write = OwnedHandle::new(write_handle, &format!("{label} write handle"))?;
    if unsafe { SetHandleInformation(read.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(last_start_error(&format!(
            "SetHandleInformation({label} read)"
        )));
    }
    let raw = read.into_raw();
    let file = unsafe { File::from_raw_handle(raw as RawHandle) };
    Ok((file, write))
}

fn inheritable_security_attributes() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    }
}

fn terminate_suspended_child(job: &OwnedHandle, process: &OwnedHandle) {
    unsafe {
        TerminateJobObject(job.raw(), START_FAILURE_EXIT_CODE);
        windows_sys::Win32::System::Threading::TerminateProcess(
            process.raw(),
            START_FAILURE_EXIT_CODE,
        );
        WaitForSingleObject(process.raw(), 5_000);
    }
}

fn terminate_job(job: &OwnedHandle, exit_code: u32) -> Result<(), ManagedProcessError> {
    if unsafe { TerminateJobObject(job.raw(), exit_code) } == 0 {
        return Err(last_wait_error("TerminateJobObject"));
    }
    Ok(())
}

fn wait_for_job_empty(
    job: &OwnedHandle,
    timeout: Duration,
) -> Result<(), ManagedProcessError> {
    let started = Instant::now();
    loop {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let mut returned = 0u32;
        if unsafe {
            QueryInformationJobObject(
                job.raw(),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION)
                    .cast::<c_void>(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                &mut returned,
            )
        } == 0
        {
            return Err(last_wait_error(
                "QueryInformationJobObject(basic accounting)",
            ));
        }
        let _ = returned;
        if accounting.ActiveProcesses == 0 {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(ManagedProcessError::Wait(
                "managed process tree did not drain before the cleanup deadline".to_string(),
            ));
        }
        thread::sleep(Duration::from_millis(WAIT_INTERVAL_MS as u64));
    }
}

fn process_temp_directory() -> Result<PathBuf, ManagedProcessError> {
    let directory = std::env::temp_dir()
        .join("LinkVault")
        .join("youtube-helper")
        .join(std::process::id().to_string());
    fs::create_dir_all(&directory)
        .map_err(|error| ManagedProcessError::Start(error.to_string()))?;
    Ok(directory)
}

fn build_environment_block(temp_directory: &Path) -> Vec<u16> {
    let mut environment = BTreeMap::<String, OsString>::new();
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.to_string(), value);
        }
    }
    environment.insert(
        "TEMP".to_string(),
        temp_directory.as_os_str().to_os_string(),
    );
    environment.insert(
        "TMP".to_string(),
        temp_directory.as_os_str().to_os_string(),
    );
    environment.insert("NO_COLOR".to_string(), OsString::from("1"));
    environment.insert("PATH".to_string(), OsString::new());
    let mut block = Vec::new();
    for (key, value) in environment {
        block.extend(OsStr::new(&key).encode_wide());
        block.push('=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn build_command_line(executable: &Path, args: &[OsString]) -> Vec<u16> {
    let mut command = quote_windows_argument(executable.as_os_str());
    for argument in args {
        command.push(' ' as u16);
        command.extend(quote_windows_argument(argument));
    }
    command.push(0);
    command
}

fn quote_windows_argument(argument: &OsStr) -> Vec<u16> {
    let value: Vec<u16> = argument.encode_wide().collect();
    let requires_quotes = value.is_empty()
        || value
            .iter()
            .any(|unit| matches!(*unit, 0x20 | 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x22));
    if !requires_quotes {
        return value;
    }
    let mut output = Vec::with_capacity(value.len() + 2);
    output.push('"' as u16);
    let mut backslashes = 0usize;
    for unit in value {
        if unit == '\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == '"' as u16 {
            output.extend(std::iter::repeat('\\' as u16).take(backslashes * 2 + 1));
            output.push(unit);
            backslashes = 0;
            continue;
        }
        output.extend(std::iter::repeat('\\' as u16).take(backslashes));
        backslashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat('\\' as u16).take(backslashes * 2));
    output.push('"' as u16);
    output
}

fn to_wide_nul(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn last_start_error(operation: &str) -> ManagedProcessError {
    ManagedProcessError::Start(format!(
        "{operation} failed with Windows error {}",
        unsafe { GetLastError() }
    ))
}

fn last_containment_error(operation: &str) -> ManagedProcessError {
    ManagedProcessError::ProcessContainment(format!(
        "{operation} failed with Windows error {}",
        unsafe { GetLastError() }
    ))
}

fn last_wait_error(operation: &str) -> ManagedProcessError {
    ManagedProcessError::Wait(format!(
        "{operation} failed with Windows error {}",
        unsafe { GetLastError() }
    ))
}

#[cfg(test)]
mod tests {
    use super::{quote_windows_argument, to_wide_nul};
    use std::ffi::OsStr;

    fn decode(value: Vec<u16>) -> String {
        String::from_utf16(&value).expect("test value must remain valid UTF-16")
    }

    #[test]
    fn windows_argv_quoting_handles_empty_spaces_quotes_and_trailing_slashes() {
        assert_eq!(decode(quote_windows_argument(OsStr::new(""))), "\"\"");
        assert_eq!(
            decode(quote_windows_argument(OsStr::new("plain"))),
            "plain"
        );
        assert_eq!(
            decode(quote_windows_argument(OsStr::new("two words"))),
            "\"two words\""
        );
        assert_eq!(
            decode(quote_windows_argument(OsStr::new(r#"a\"b"#))),
            r#""a\\\"b""#
        );
        assert_eq!(
            decode(quote_windows_argument(OsStr::new(r#"C:\folder name\"#))),
            r#""C:\folder name\\""#,
        );
        assert_eq!(to_wide_nul(OsStr::new("x")), vec!['x' as u16, 0]);
    }
}
