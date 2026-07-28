//! Adaptive CPU and memory admission for newspaper image workers.

use std::time::{Duration, Instant};

use super::models::{OptimizationRunOptions, OptimizationRuntimeStatus};

const HARD_WORKER_CEILING: u8 = 20;
const AUTO_START_WORKERS: u8 = 2;
/// Default memory budget per worker. Used when the caller does not pass
/// `worker_memory_budget_mb`. Sized for typical 4K image decode/encode peaks.
const DEFAULT_WORKER_MEMORY_BUDGET_BYTES: u64 = 160 * 1024 * 1024;
/// Default memory reserve kept free for the rest of the OS, the LinkVault UI,
/// and concurrent download workers. The actual reserve is the larger of this
/// constant and 10% of total RAM, matching the original conservative floor.
const DEFAULT_MEMORY_RESERVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Minimum bytes for a user-configured `worker_memory_budget_mb`. Anything
/// smaller would not fit a single decoded newspaper page and would just
/// trigger swap.
const MIN_WORKER_MEMORY_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum bytes for a user-configured `worker_memory_budget_mb`. Larger
/// values risk one worker hogging the entire system, defeating the
/// worker's purpose as the unit of admission.
const MAX_WORKER_MEMORY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
/// Minimum bytes for a user-configured `memory_reserve_bytes`. Smaller
/// values would leave the OS and the rest of the app starved.
const MIN_MEMORY_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum bytes for a user-configured `memory_reserve_bytes`. Larger
/// values would cap the optimization at a single worker on most machines.
const MAX_MEMORY_RESERVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// Auto mode target cap. The governor treats the system as "at cap" once
/// CPU rises above this, and refuses to admit more workers until the
/// load drops. A small deadband (10 percentage points) is applied around
/// this value so single-sample jitter does not flip the decision.
pub(super) const AUTO_CPU_TARGET_PERCENT: f32 = 50.0;
/// Auto mode will only scale up when the system is comfortably below the
/// target. Defaults to `AUTO_CPU_TARGET_PERCENT - 10`.
pub(super) const AUTO_CPU_SCALE_UP_PERCENT: f32 = AUTO_CPU_TARGET_PERCENT - 10.0;
/// Auto mode immediately scales down when the system reaches the target.
pub(super) const AUTO_CPU_SCALE_DOWN_PERCENT: f32 = AUTO_CPU_TARGET_PERCENT;
/// Auto mode must observe the high-CPU signal this many times in a row
/// before it tears a worker down. Slows oscillation and lets short spikes
/// pass through.
const AUTO_HIGH_CPU_SAMPLE_THRESHOLD: u8 = 2;
/// Minimum gap between admission adjustments in auto mode. Default is
/// intentionally slower than the previous 1 s pacing to avoid oscillating
/// around the 50% target.
const AUTO_SCALE_INTERVAL: Duration = Duration::from_secs(3);

pub(super) struct ResourceGovernor {
    mode: String,
    requested: u8,
    cpu_ceiling: u8,
    admitted: u8,
    last_adjustment: Instant,
    high_cpu_samples: u8,
    previous_cpu: Option<CpuTimes>,
    sample: SystemSample,
    limited_reason: Option<String>,
    worker_memory_budget_bytes: u64,
    memory_reserve_bytes: u64,
}

impl ResourceGovernor {
    pub(super) fn new(options: OptimizationRunOptions) -> Self {
        let mode = if options.mode.eq_ignore_ascii_case("manual") {
            "manual"
        } else {
            "auto"
        }
        .to_string();
        let requested = options.worker_ceiling.clamp(2, HARD_WORKER_CEILING);
        let logical_processors = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(2)
            .min(usize::from(HARD_WORKER_CEILING)) as u8;
        let cpu_ceiling = if mode == "auto" {
            logical_processors.max(2)
        } else {
            HARD_WORKER_CEILING
        };
        let worker_memory_budget_bytes = options
            .worker_memory_budget_mb
            .map(|megabytes| {
                (u64::from(megabytes) * 1024 * 1024)
                    .clamp(MIN_WORKER_MEMORY_BUDGET_BYTES, MAX_WORKER_MEMORY_BUDGET_BYTES)
            })
            .unwrap_or(DEFAULT_WORKER_MEMORY_BUDGET_BYTES);
        let memory_reserve_bytes = options
            .memory_reserve_bytes
            .unwrap_or(DEFAULT_MEMORY_RESERVE_BYTES)
            .clamp(MIN_MEMORY_RESERVE_BYTES, MAX_MEMORY_RESERVE_BYTES);
        let sample = system_sample(None);
        let memory_ceiling = memory_worker_limit(&sample, worker_memory_budget_bytes, memory_reserve_bytes)
            .max(1);
        let safe_ceiling = requested.min(cpu_ceiling).min(memory_ceiling);
        let admitted = if mode == "auto" {
            AUTO_START_WORKERS.min(safe_ceiling).max(1)
        } else {
            safe_ceiling.max(1)
        };
        let limited_reason = limit_reason(requested, cpu_ceiling, memory_ceiling);
        Self {
            mode,
            requested,
            cpu_ceiling,
            admitted,
            last_adjustment: Instant::now(),
            high_cpu_samples: 0,
            previous_cpu: sample.cpu_times,
            sample,
            limited_reason,
            worker_memory_budget_bytes,
            memory_reserve_bytes,
        }
    }

    pub(super) fn refresh(&mut self) {
        self.sample = system_sample(self.previous_cpu);
        self.previous_cpu = self.sample.cpu_times;
        let memory_ceiling = memory_worker_limit(
            &self.sample,
            self.worker_memory_budget_bytes,
            self.memory_reserve_bytes,
        )
        .max(1);
        let safe_ceiling = self
            .requested
            .min(self.cpu_ceiling)
            .min(memory_ceiling)
            .max(1);
        self.limited_reason = limit_reason(self.requested, self.cpu_ceiling, memory_ceiling);

        if self.sample.cpu_percent.is_some_and(|value| value >= AUTO_CPU_SCALE_DOWN_PERCENT) {
            self.high_cpu_samples = self.high_cpu_samples.saturating_add(1);
        } else {
            self.high_cpu_samples = 0;
        }

        if self.admitted > safe_ceiling {
            self.admitted = safe_ceiling;
            self.last_adjustment = Instant::now();
            return;
        }
        if self.last_adjustment.elapsed() < AUTO_SCALE_INTERVAL {
            return;
        }
        if self.high_cpu_samples >= AUTO_HIGH_CPU_SAMPLE_THRESHOLD && self.admitted > 1 {
            self.admitted -= 1;
            self.high_cpu_samples = 0;
        } else if self.mode == "auto"
            && self.admitted < safe_ceiling
            && self.sample.cpu_percent.unwrap_or(0.0) < AUTO_CPU_SCALE_UP_PERCENT
        {
            self.admitted += 1;
        }
        self.last_adjustment = Instant::now();
    }

    pub(super) fn admitted_workers(&self) -> usize {
        usize::from(self.admitted)
    }

    pub(super) fn runtime_status(&self, active_workers: usize) -> OptimizationRuntimeStatus {
        OptimizationRuntimeStatus {
            active: true,
            mode: self.mode.clone(),
            requested_workers: self.requested,
            admitted_workers: self.admitted,
            active_workers: active_workers.min(usize::from(u8::MAX)) as u8,
            cpu_percent: self.sample.cpu_percent,
            available_memory_bytes: self.sample.available_memory,
            memory_safe: memory_worker_limit(
                &self.sample,
                self.worker_memory_budget_bytes,
                self.memory_reserve_bytes,
            ) >= self.admitted,
            limited_reason: self.limited_reason.clone(),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SystemSample {
    cpu_percent: Option<f32>,
    cpu_times: Option<CpuTimes>,
    total_memory: Option<u64>,
    available_memory: Option<u64>,
}

#[derive(Clone, Copy)]
struct CpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

fn memory_worker_limit(
    sample: &SystemSample,
    worker_memory_budget_bytes: u64,
    memory_reserve_bytes: u64,
) -> u8 {
    let (Some(total), Some(available)) = (sample.total_memory, sample.available_memory) else {
        return HARD_WORKER_CEILING;
    };
    let reserve = memory_reserve_bytes.max(total / 10);
    let usable = available.saturating_sub(reserve);
    (usable / worker_memory_budget_bytes)
        .min(u64::from(HARD_WORKER_CEILING))
        .max(1) as u8
}

fn limit_reason(requested: u8, cpu_ceiling: u8, memory_ceiling: u8) -> Option<String> {
    let admitted = requested.min(cpu_ceiling).min(memory_ceiling);
    if admitted >= requested {
        None
    } else if memory_ceiling <= cpu_ceiling {
        Some(format!(
            "Limited to {admitted} workers to preserve the memory reserve."
        ))
    } else {
        Some(format!(
            "Limited to {admitted} workers for the available logical processors."
        ))
    }
}

#[cfg(windows)]
fn system_sample(previous_cpu: Option<CpuTimes>) -> SystemSample {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
            Threading::GetSystemTimes,
        },
    };

    let mut memory = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    let memory_ok = unsafe { GlobalMemoryStatusEx(&mut memory) } != 0;
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let cpu_ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0;
    let cpu_times = cpu_ok.then(|| CpuTimes {
        idle: filetime_value(idle),
        kernel: filetime_value(kernel),
        user: filetime_value(user),
    });
    let cpu_percent = previous_cpu.zip(cpu_times).and_then(|(previous, current)| {
        let idle_delta = current.idle.saturating_sub(previous.idle);
        let kernel_delta = current.kernel.saturating_sub(previous.kernel);
        let user_delta = current.user.saturating_sub(previous.user);
        let total = kernel_delta.saturating_add(user_delta);
        (total > 0)
            .then(|| ((total.saturating_sub(idle_delta)) as f64 * 100.0 / total as f64) as f32)
    });
    SystemSample {
        cpu_percent,
        cpu_times,
        total_memory: memory_ok.then_some(memory.ullTotalPhys),
        available_memory: memory_ok.then_some(memory.ullAvailPhys),
    }
}

#[cfg(windows)]
fn filetime_value(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(not(windows))]
fn system_sample(_previous_cpu: Option<CpuTimes>) -> SystemSample {
    SystemSample::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_guard_preserves_the_required_reserve() {
        let sample = SystemSample {
            total_memory: Some(16 * 1024 * 1024 * 1024),
            available_memory: Some(5 * 1024 * 1024 * 1024),
            ..SystemSample::default()
        };
        assert_eq!(
            memory_worker_limit(
                &sample,
                DEFAULT_WORKER_MEMORY_BUDGET_BYTES,
                DEFAULT_MEMORY_RESERVE_BYTES
            ),
            6
        );
    }

    #[test]
    fn manual_twenty_is_a_ceiling_not_an_unsafe_guarantee() {
        let reason = limit_reason(20, 16, 8).unwrap();
        assert!(reason.contains("8 workers"));
        assert!(reason.contains("memory"));
    }

    #[test]
    fn unknown_memory_does_not_disable_the_cpu_governor() {
        assert_eq!(
            memory_worker_limit(
                &SystemSample::default(),
                DEFAULT_WORKER_MEMORY_BUDGET_BYTES,
                DEFAULT_MEMORY_RESERVE_BYTES
            ),
            HARD_WORKER_CEILING
        );
    }

    #[test]
    fn configurable_memory_budget_scales_worker_limit() {
        // 6 GB available, 2 GB reserve, 256 MB per worker => 16 workers.
        let sample = SystemSample {
            total_memory: Some(16 * 1024 * 1024 * 1024),
            available_memory: Some(6 * 1024 * 1024 * 1024),
            ..SystemSample::default()
        };
        let budget = 256 * 1024 * 1024;
        let reserve = 2 * 1024 * 1024 * 1024;
        assert_eq!(
            memory_worker_limit(&sample, budget, reserve),
            16,
            "((6 GB - max(2 GB, 1.6 GB)) / 256 MB = 16)"
        );
    }

    #[test]
    fn configurable_memory_budget_clamps_to_safe_floor_and_ceiling() {
        let options = OptimizationRunOptions {
            mode: "auto".to_string(),
            worker_ceiling: 16,
            worker_memory_budget_mb: Some(8), // below 64 MB minimum
            memory_reserve_bytes: Some(64 * 1024 * 1024 * 1024), // above 32 GB maximum
        };
        let governor = ResourceGovernor::new(options);
        assert_eq!(governor.worker_memory_budget_bytes, MIN_WORKER_MEMORY_BUDGET_BYTES);
        assert_eq!(governor.memory_reserve_bytes, MAX_MEMORY_RESERVE_BYTES);
    }

    #[test]
    fn auto_mode_cpu_target_is_fifty_percent() {
        assert_eq!(AUTO_CPU_TARGET_PERCENT, 50.0);
        assert!(AUTO_CPU_SCALE_UP_PERCENT < AUTO_CPU_TARGET_PERCENT);
        assert!(AUTO_CPU_SCALE_DOWN_PERCENT <= AUTO_CPU_TARGET_PERCENT);
    }

    #[test]
    fn auto_mode_scale_interval_is_slower_than_one_second() {
        assert!(AUTO_SCALE_INTERVAL >= Duration::from_secs(2));
    }
}
