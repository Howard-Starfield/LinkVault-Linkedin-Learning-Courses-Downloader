use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{self, Command};
use std::thread;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("youtube process fixture failed: {error}");
        process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let _program = args.next();
    let mode = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| "missing fixture mode".to_string())?;
    let remaining = args.collect::<Vec<_>>();
    match mode.as_str() {
        "quick" => Ok(()),
        "sleep" => {
            let duration = parse_duration(&remaining, 0)?;
            thread::sleep(duration);
            Ok(())
        }
        "write_marker" => {
            let path = path_argument(&remaining, 0)?;
            fs::write(path, b"executed").map_err(|error| error.to_string())
        }
        "report_environment" => report_environment(),
        "grandchild" => run_grandchild_parent(&remaining),
        "survivor" => run_survivor(&remaining),
        "noisy" => run_noisy(&remaining),
        "trickle" => run_trickle(&remaining),
        "many_lines" => run_many_lines(&remaining),
        "long_line" => run_long_line(),
        "invalid_utf8" => {
            io::stdout()
                .write_all(&[0xff, 0xfe, 0xfd])
                .map_err(|error| error.to_string())?;
            io::stdout().flush().map_err(|error| error.to_string())
        }
        "echo_args" => {
            let mut stdout = io::stdout().lock();
            for argument in remaining {
                let value = argument
                    .into_string()
                    .map_err(|_| "echo argument was not valid Unicode".to_string())?;
                let encoded = serde_json::to_string(&value).map_err(|error| error.to_string())?;
                writeln!(stdout, "{encoded}").map_err(|error| error.to_string())?;
            }
            stdout.flush().map_err(|error| error.to_string())
        }
        other => Err(format!("unknown fixture mode {other}")),
    }
}

fn report_environment() -> Result<(), String> {
    let keys = [
        "TEMP",
        "TMP",
        "DENO_DIR",
        "XDG_CACHE_HOME",
        "HOME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "PATH",
    ];
    let mut values = BTreeMap::new();
    for key in keys {
        let value = env::var_os(key)
            .ok_or_else(|| format!("missing required environment variable {key}"))?;
        values.insert(key, value.to_string_lossy().into_owned());
    }
    println!(
        "{}",
        serde_json::to_string(&values).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn run_grandchild_parent(args: &[std::ffi::OsString]) -> Result<(), String> {
    let ready_path = path_argument(args, 0)?;
    let survivor_path = path_argument(args, 1)?;
    let delay = parse_duration(args, 2)?;
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let child = Command::new(executable)
        .arg("survivor")
        .arg(&survivor_path)
        .arg(delay.as_millis().to_string())
        .spawn()
        .map_err(|error| error.to_string())?;
    fs::write(
        ready_path,
        format!("parent={} child={}", process::id(), child.id()),
    )
    .map_err(|error| error.to_string())?;
    drop(child);
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn run_survivor(args: &[std::ffi::OsString]) -> Result<(), String> {
    let survivor_path = path_argument(args, 0)?;
    let delay = parse_duration(args, 1)?;
    thread::sleep(delay);
    fs::write(survivor_path, b"survived job termination").map_err(|error| error.to_string())
}

fn run_noisy(args: &[std::ffi::OsString]) -> Result<(), String> {
    let bytes = args
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or("1048576")
        .parse::<usize>()
        .map_err(|error| format!("invalid noisy byte count: {error}"))?;
    let stdout_chunk = vec![b'O'; bytes];
    let stderr_chunk = vec![b'E'; bytes];
    let stdout = thread::spawn(move || -> io::Result<()> {
        let mut stream = io::stdout().lock();
        stream.write_all(&stdout_chunk)?;
        stream.flush()
    });
    let stderr = thread::spawn(move || -> io::Result<()> {
        let mut stream = io::stderr().lock();
        stream.write_all(&stderr_chunk)?;
        stream.flush()
    });
    stdout
        .join()
        .map_err(|_| "stdout fixture writer panicked".to_string())?
        .map_err(|error| error.to_string())?;
    stderr
        .join()
        .map_err(|_| "stderr fixture writer panicked".to_string())?
        .map_err(|error| error.to_string())
}

/// Writes lines incrementally, splitting one across two writes and ending with
/// an unterminated line, so the supervisor's line framing can be observed
/// while the process is still alive.
fn run_trickle(args: &[std::ffi::OsString]) -> Result<(), String> {
    let pause = parse_duration(args, 0)?;
    let mut stdout = io::stdout().lock();
    for chunk in [&b"alpha\r\n"[..], &b"be"[..], &b"ta\n"[..], &b"gamma"[..]] {
        stdout.write_all(chunk).map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
        thread::sleep(pause);
    }
    Ok(())
}

fn run_many_lines(args: &[std::ffi::OsString]) -> Result<(), String> {
    let count = args
        .first()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "missing line count".to_string())?
        .parse::<usize>()
        .map_err(|error| format!("invalid line count: {error}"))?;
    let mut stdout = io::stdout().lock();
    for index in 0..count {
        writeln!(stdout, "line-{index}").map_err(|error| error.to_string())?;
    }
    stdout.flush().map_err(|error| error.to_string())
}

fn run_long_line() -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&vec![b'L'; 64 * 1024])
        .map_err(|error| error.to_string())?;
    stdout
        .write_all(b"\nafter\n")
        .map_err(|error| error.to_string())?;
    stdout.flush().map_err(|error| error.to_string())
}

fn parse_duration(args: &[std::ffi::OsString], index: usize) -> Result<Duration, String> {
    let millis = args
        .get(index)
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("missing millisecond argument {index}"))?
        .parse::<u64>()
        .map_err(|error| format!("invalid millisecond argument {index}: {error}"))?;
    Ok(Duration::from_millis(millis))
}

fn path_argument(args: &[std::ffi::OsString], index: usize) -> Result<PathBuf, String> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing path argument {index}"))
}
