use std::{
    env,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use image::GenericImageView;
use serde::Serialize;
use webp::{Encoder, WebPConfig};

const DEFAULT_QUALITY: u8 = 45;
const DEFAULT_WORKERS: usize = 1;
const DEFAULT_ENCODER_THREADS: i32 = 1;

#[derive(Debug)]
struct Arguments {
    inputs: Vec<PathBuf>,
    qualities: Vec<u8>,
    workers: Vec<usize>,
    encoder_threads: Vec<i32>,
    repetitions: usize,
    trials: usize,
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    generated_at: String,
    workload: &'static str,
    logical_processors: usize,
    inputs: Vec<String>,
    repetitions: usize,
    trials: usize,
    runs: Vec<RunReport>,
}

#[derive(Debug, Serialize)]
struct RunReport {
    trial: usize,
    quality: u8,
    workers: usize,
    encoder_threads: i32,
    tasks: usize,
    elapsed_ms: f64,
    pages_per_minute: f64,
    p50_page_ms: f64,
    p95_page_ms: f64,
    p50_read_ms: f64,
    p50_decode_convert_ms: f64,
    p50_encode_ms: f64,
    p50_write_ms: f64,
    p50_validate_ms: f64,
    p50_rename_ms: f64,
    p50_write_validate_ms: f64,
    cpu_core_equivalents: Option<f64>,
    cpu_percent_of_machine: Option<f64>,
    peak_working_set_bytes: Option<u64>,
    peak_private_bytes: Option<u64>,
    minimum_available_memory_bytes: Option<u64>,
    total_source_bytes: u64,
    total_output_bytes: u64,
    output_ratio: f64,
    dimensions: Vec<ImageDimensions>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ImageDimensions {
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct TaskResult {
    elapsed_ms: f64,
    read_ms: f64,
    decode_convert_ms: f64,
    encode_ms: f64,
    write_ms: f64,
    validate_ms: f64,
    rename_ms: f64,
    write_validate_ms: f64,
    source_bytes: u64,
    output_bytes: u64,
    dimensions: ImageDimensions,
}

#[derive(Debug, Default)]
struct ResourceSample {
    process_cpu_100ns: Option<u64>,
    working_set_bytes: Option<u64>,
    private_bytes: Option<u64>,
    available_memory_bytes: Option<u64>,
}

#[derive(Debug, Default)]
struct ResourcePeaks {
    peak_working_set_bytes: Option<u64>,
    peak_private_bytes: Option<u64>,
    minimum_available_memory_bytes: Option<u64>,
}

struct ResourceMonitor {
    stop: Arc<AtomicBool>,
    sampler: thread::JoinHandle<ResourcePeaks>,
    started_at: Instant,
    starting_cpu_100ns: Option<u64>,
}

impl ResourceMonitor {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let sampler_stop = Arc::clone(&stop);
        let starting_cpu_100ns = sample_resources().process_cpu_100ns;
        let sampler = thread::spawn(move || {
            let mut peaks = ResourcePeaks::default();
            while !sampler_stop.load(Ordering::Relaxed) {
                update_peaks(&mut peaks, sample_resources());
                thread::sleep(Duration::from_millis(100));
            }
            update_peaks(&mut peaks, sample_resources());
            peaks
        });
        Self {
            stop,
            sampler,
            started_at: Instant::now(),
            starting_cpu_100ns,
        }
    }

    fn finish(self, logical_processors: usize) -> (ResourcePeaks, Option<f64>, Option<f64>) {
        self.stop.store(true, Ordering::Relaxed);
        let elapsed_seconds = self.started_at.elapsed().as_secs_f64();
        let ending_cpu_100ns = sample_resources().process_cpu_100ns;
        let peaks = self.sampler.join().unwrap_or_default();
        let core_equivalents = match (self.starting_cpu_100ns, ending_cpu_100ns, elapsed_seconds) {
            (Some(start), Some(end), elapsed) if elapsed > 0.0 && end >= start => {
                Some(((end - start) as f64 / 10_000_000.0) / elapsed)
            }
            _ => None,
        };
        let percent_of_machine =
            core_equivalents.map(|cores| cores / logical_processors.max(1) as f64 * 100.0);
        (peaks, core_equivalents, percent_of_machine)
    }
}

fn main() -> Result<(), String> {
    let arguments = parse_arguments()?;
    let logical_processors = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1);
    let mut runs = Vec::new();

    for trial in 1..=arguments.trials {
        let mut qualities = arguments.qualities.clone();
        let mut encoder_thread_modes = arguments.encoder_threads.clone();
        let mut worker_counts = arguments.workers.clone();
        if trial % 2 == 0 {
            qualities.reverse();
            encoder_thread_modes.reverse();
            worker_counts.reverse();
        }
        for &quality in &qualities {
            for &encoder_threads in &encoder_thread_modes {
                for &workers in &worker_counts {
                    eprintln!(
                        "benchmarking trial={trial} quality={quality} workers={workers} encoder_threads={encoder_threads}"
                    );
                    runs.push(run_scenario(
                        &arguments.inputs,
                        arguments.repetitions,
                        trial,
                        quality,
                        workers,
                        encoder_threads,
                        logical_processors,
                    )?);
                }
            }
        }
    }

    let report = BenchmarkReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        workload: "read-decode-convert-encode-write-validate-rename",
        logical_processors,
        inputs: arguments
            .inputs
            .iter()
            .map(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("image")
                    .to_string()
            })
            .collect(),
        repetitions: arguments.repetitions,
        trials: arguments.trials,
        runs,
    };
    let json = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(output) = arguments.output {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&output, &json).map_err(|error| error.to_string())?;
        eprintln!("wrote {}", output.display());
    }
    println!("{json}");
    Ok(())
}

fn run_scenario(
    inputs: &[PathBuf],
    repetitions: usize,
    trial: usize,
    quality: u8,
    workers: usize,
    encoder_threads: i32,
    logical_processors: usize,
) -> Result<RunReport, String> {
    let tasks: Vec<PathBuf> = (0..repetitions)
        .flat_map(|_| inputs.iter().cloned())
        .collect();
    if tasks.is_empty() {
        return Err("at least one benchmark task is required".to_string());
    }
    let task_count = tasks.len();
    let tasks = Arc::new(tasks);
    let next_task = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::<Result<TaskResult, String>>::new()));
    let scratch = tempfile::Builder::new()
        .prefix("linkvault-newspaper-webp-bench-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let scratch_path = Arc::new(scratch.path().to_path_buf());
    let monitor = ResourceMonitor::start();
    let started_at = Instant::now();

    let handles: Vec<_> = (0..workers.min(task_count))
        .map(|_| {
            let tasks = Arc::clone(&tasks);
            let next_task = Arc::clone(&next_task);
            let results = Arc::clone(&results);
            let scratch_path = Arc::clone(&scratch_path);
            thread::spawn(move || loop {
                let index = next_task.fetch_add(1, Ordering::Relaxed);
                let Some(input) = tasks.get(index) else {
                    break;
                };
                let output = scratch_path.join(format!("task-{index}.webp"));
                let result = optimize_pipeline(input, &output, quality, encoder_threads);
                results.lock().expect("benchmark result lock").push(result);
            })
        })
        .collect();

    for handle in handles {
        handle
            .join()
            .map_err(|_| "a benchmark worker panicked".to_string())?;
    }

    let elapsed = started_at.elapsed();
    let (peaks, cpu_core_equivalents, cpu_percent_of_machine) = monitor.finish(logical_processors);
    let mut results = Arc::try_unwrap(results)
        .map_err(|_| "benchmark results are still shared".to_string())?
        .into_inner()
        .map_err(|_| "benchmark result lock was poisoned".to_string())?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    results.sort_by(|left, right| left.elapsed_ms.total_cmp(&right.elapsed_ms));

    let total_source_bytes = results.iter().map(|result| result.source_bytes).sum();
    let total_output_bytes = results.iter().map(|result| result.output_bytes).sum();
    let mut dimensions = results
        .iter()
        .map(|result| result.dimensions.clone())
        .collect::<Vec<_>>();
    dimensions.sort_by_key(|value| (value.width, value.height));
    dimensions.dedup();
    let elapsed_seconds = elapsed.as_secs_f64();

    Ok(RunReport {
        trial,
        quality,
        workers,
        encoder_threads,
        tasks: task_count,
        elapsed_ms: elapsed_seconds * 1_000.0,
        pages_per_minute: task_count as f64 / elapsed_seconds * 60.0,
        p50_page_ms: percentile(&results, 0.50, |result| result.elapsed_ms),
        p95_page_ms: percentile(&results, 0.95, |result| result.elapsed_ms),
        p50_read_ms: percentile(&results, 0.50, |result| result.read_ms),
        p50_decode_convert_ms: percentile(&results, 0.50, |result| result.decode_convert_ms),
        p50_encode_ms: percentile(&results, 0.50, |result| result.encode_ms),
        p50_write_ms: percentile(&results, 0.50, |result| result.write_ms),
        p50_validate_ms: percentile(&results, 0.50, |result| result.validate_ms),
        p50_rename_ms: percentile(&results, 0.50, |result| result.rename_ms),
        p50_write_validate_ms: percentile(&results, 0.50, |result| result.write_validate_ms),
        cpu_core_equivalents,
        cpu_percent_of_machine,
        peak_working_set_bytes: peaks.peak_working_set_bytes,
        peak_private_bytes: peaks.peak_private_bytes,
        minimum_available_memory_bytes: peaks.minimum_available_memory_bytes,
        total_source_bytes,
        total_output_bytes,
        output_ratio: total_output_bytes as f64 / total_source_bytes.max(1) as f64,
        dimensions,
    })
}

fn optimize_pipeline(
    input: &PathBuf,
    output: &PathBuf,
    quality: u8,
    encoder_threads: i32,
) -> Result<TaskResult, String> {
    if !(25..=95).contains(&quality) {
        return Err(format!(
            "quality must be between 25 and 95, received {quality}"
        ));
    }
    if !(0..=1).contains(&encoder_threads) {
        return Err(format!(
            "encoder threads must be 0 or 1, received {encoder_threads}"
        ));
    }
    let started_at = Instant::now();
    let read_started_at = Instant::now();
    let source = std::fs::read(input).map_err(|error| format!("{}: {error}", input.display()))?;
    let read_ms = read_started_at.elapsed().as_secs_f64() * 1_000.0;
    let decode_started_at = Instant::now();
    let image = image::load_from_memory(&source)
        .map_err(|error| format!("{}: {error}", input.display()))?;
    let dimensions = image.dimensions();
    let mut config =
        WebPConfig::new().map_err(|_| "could not initialize WebP config".to_string())?;
    config.quality = f32::from(quality);
    config.method = 2;
    config.thread_level = encoder_threads;
    let (encoded, decode_convert_ms, encode_ms) = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let decode_convert_ms = decode_started_at.elapsed().as_secs_f64() * 1_000.0;
        let encode_started_at = Instant::now();
        let encoded = Encoder::from_rgba(rgba.as_raw(), dimensions.0, dimensions.1)
            .encode_advanced(&config)
            .map_err(|error| format!("{}: {error:?}", input.display()))?;
        (
            encoded,
            decode_convert_ms,
            encode_started_at.elapsed().as_secs_f64() * 1_000.0,
        )
    } else {
        let rgb = image.to_rgb8();
        let decode_convert_ms = decode_started_at.elapsed().as_secs_f64() * 1_000.0;
        let encode_started_at = Instant::now();
        let encoded = Encoder::from_rgb(rgb.as_raw(), dimensions.0, dimensions.1)
            .encode_advanced(&config)
            .map_err(|error| format!("{}: {error:?}", input.display()))?;
        (
            encoded,
            decode_convert_ms,
            encode_started_at.elapsed().as_secs_f64() * 1_000.0,
        )
    };
    let write_validate_started_at = Instant::now();
    let part = output.with_extension("webp.part");
    let write_started_at = Instant::now();
    std::fs::write(&part, encoded.as_ref())
        .map_err(|error| format!("{}: {error}", part.display()))?;
    let write_ms = write_started_at.elapsed().as_secs_f64() * 1_000.0;
    let validate_started_at = Instant::now();
    let validated = image::load_from_memory(encoded.as_ref())
        .map_err(|error| format!("{}: {error}", part.display()))?;
    if validated.dimensions() != dimensions {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{}: encoded dimensions changed from {}x{} to {}x{}",
            input.display(),
            dimensions.0,
            dimensions.1,
            validated.width(),
            validated.height()
        ));
    }
    let validate_ms = validate_started_at.elapsed().as_secs_f64() * 1_000.0;
    let rename_started_at = Instant::now();
    std::fs::rename(&part, output).map_err(|error| format!("{}: {error}", output.display()))?;
    let rename_ms = rename_started_at.elapsed().as_secs_f64() * 1_000.0;
    let write_validate_ms = write_validate_started_at.elapsed().as_secs_f64() * 1_000.0;

    Ok(TaskResult {
        elapsed_ms: started_at.elapsed().as_secs_f64() * 1_000.0,
        read_ms,
        decode_convert_ms,
        encode_ms,
        write_ms,
        validate_ms,
        rename_ms,
        write_validate_ms,
        source_bytes: source.len() as u64,
        output_bytes: encoded.len() as u64,
        dimensions: ImageDimensions {
            width: dimensions.0,
            height: dimensions.1,
        },
    })
}

fn percentile(results: &[TaskResult], percentile: f64, metric: impl Fn(&TaskResult) -> f64) -> f64 {
    let mut values = results.iter().map(metric).collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let index = ((values.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1));
    values[index]
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut inputs = Vec::new();
    let mut qualities = vec![DEFAULT_QUALITY];
    let mut workers = vec![DEFAULT_WORKERS];
    let mut encoder_threads = vec![DEFAULT_ENCODER_THREADS];
    let mut repetitions = 1;
    let mut trials = 1;
    let mut output = None;
    let mut args = env::args().skip(1);

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--input" => inputs.push(PathBuf::from(next_value(&mut args, "--input")?)),
            "--qualities" => {
                qualities = parse_csv(&next_value(&mut args, "--qualities")?, "--qualities")?
            }
            "--workers" => workers = parse_csv(&next_value(&mut args, "--workers")?, "--workers")?,
            "--encoder-threads" => {
                encoder_threads = parse_csv(
                    &next_value(&mut args, "--encoder-threads")?,
                    "--encoder-threads",
                )?
            }
            "--repetitions" => {
                repetitions = next_value(&mut args, "--repetitions")?
                    .parse()
                    .map_err(|_| "--repetitions must be a positive integer".to_string())?
            }
            "--trials" => {
                trials = next_value(&mut args, "--trials")?
                    .parse()
                    .map_err(|_| "--trials must be a positive integer".to_string())?
            }
            "--output" => output = Some(PathBuf::from(next_value(&mut args, "--output")?)),
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            value if !value.starts_with('-') => inputs.push(PathBuf::from(value)),
            value => return Err(format!("unknown argument: {value}")),
        }
    }

    if inputs.is_empty() {
        return Err("at least one --input image is required".to_string());
    }
    if repetitions == 0 {
        return Err("--repetitions must be at least 1".to_string());
    }
    if trials == 0 {
        return Err("--trials must be at least 1".to_string());
    }
    if workers.iter().any(|&value| value == 0 || value > 20) {
        return Err("--workers values must be between 1 and 20".to_string());
    }
    if qualities.iter().any(|value| !(25..=95).contains(value)) {
        return Err("--qualities values must be between 25 and 95".to_string());
    }
    if encoder_threads.iter().any(|value| !(0..=1).contains(value)) {
        return Err("--encoder-threads values must be 0 or 1".to_string());
    }

    qualities.sort_unstable();
    qualities.dedup();
    workers.sort_unstable();
    workers.dedup();
    encoder_threads.sort_unstable();
    encoder_threads.dedup();

    Ok(Arguments {
        inputs,
        qualities,
        workers,
        encoder_threads,
        repetitions,
        trials,
        output,
    })
}

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_csv<T>(value: &str, name: &str) -> Result<Vec<T>, String>
where
    T: std::str::FromStr,
{
    let parsed = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<T>()
                .map_err(|_| format!("{name} contains an invalid value: {entry}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parsed.is_empty() {
        return Err(format!("{name} must contain at least one value"));
    }
    Ok(parsed)
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --example newspaper_webp_bench -- \\
         --input <image> [--input <image> ...] \\
         [--qualities 45,92] [--workers 1,2,4,8,12,16,20] \\
         [--encoder-threads 0,1] [--repetitions 2] [--trials 3] \\
         [--output report.json]"
    );
}

fn update_peaks(peaks: &mut ResourcePeaks, sample: ResourceSample) {
    peaks.peak_working_set_bytes =
        maximum_option(peaks.peak_working_set_bytes, sample.working_set_bytes);
    peaks.peak_private_bytes = maximum_option(peaks.peak_private_bytes, sample.private_bytes);
    peaks.minimum_available_memory_bytes = minimum_option(
        peaks.minimum_available_memory_bytes,
        sample.available_memory_bytes,
    );
}

fn maximum_option(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

fn minimum_option(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

#[cfg(windows)]
fn sample_resources() -> ResourceSample {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
    };

    unsafe {
        let process = GetCurrentProcess();
        let mut counters: PROCESS_MEMORY_COUNTERS = zeroed();
        let memory_ok = GetProcessMemoryInfo(
            process,
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ) != 0;

        let mut memory_status: MEMORYSTATUSEX = zeroed();
        memory_status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        let system_memory_ok = GlobalMemoryStatusEx(&mut memory_status) != 0;

        let mut creation: FILETIME = zeroed();
        let mut exit: FILETIME = zeroed();
        let mut kernel: FILETIME = zeroed();
        let mut user: FILETIME = zeroed();
        let times_ok =
            GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        let process_cpu_100ns = times_ok.then(|| filetime_value(kernel) + filetime_value(user));

        ResourceSample {
            process_cpu_100ns,
            working_set_bytes: memory_ok.then_some(counters.WorkingSetSize as u64),
            private_bytes: memory_ok.then_some(counters.PagefileUsage as u64),
            available_memory_bytes: system_memory_ok.then_some(memory_status.ullAvailPhys),
        }
    }
}

#[cfg(windows)]
fn filetime_value(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(not(windows))]
fn sample_resources() -> ResourceSample {
    ResourceSample::default()
}
