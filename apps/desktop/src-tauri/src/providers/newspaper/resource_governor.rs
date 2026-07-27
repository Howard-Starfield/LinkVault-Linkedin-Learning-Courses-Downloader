//! Adaptive CPU and memory admission for newspaper image workers.

use std::time::{Duration, Instant};

use super::models::{OptimizationRunOptions, OptimizationRuntimeStatus};

const HARD_WORKER_CEILING: u8 = 20;
const AUTO_START_WORKERS: u8 = 2;
const WORKER_MEMORY_BUDGET: u64 = 160 * 1024 * 1024;
const MINIMUM_MEMORY_RESERVE: u64 = 4 * 1024 * 1024 * 1024;

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
        let sample = system_sample(None);
        let memory_ceiling = memory_worker_limit(&sample).max(1);
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
        }
    }

    pub(super) fn refresh(&mut self) {
        self.sample = system_sample(self.previous_cpu);
        self.previous_cpu = self.sample.cpu_times;
        let memory_ceiling = memory_worker_limit(&self.sample).max(1);
        let safe_ceiling = self
            .requested
            .min(self.cpu_ceiling)
            .min(memory_ceiling)
            .max(1);
        self.limited_reason = limit_reason(self.requested, self.cpu_ceiling, memory_ceiling);

        if self.sample.cpu_percent.is_some_and(|value| value >= 90.0) {
            self.high_cpu_samples = self.high_cpu_samples.saturating_add(1);
        } else {
            self.high_cpu_samples = 0;
        }

        if self.admitted > safe_ceiling {
            self.admitted = safe_ceiling;
            self.last_adjustment = Instant::now();
            return;
        }
        if self.last_adjustment.elapsed() < Duration::from_secs(1) {
            return;
        }
        if self.high_cpu_samples >= 2 && self.admitted > 1 {
            self.admitted -= 1;
        } else if self.mode == "auto"
            && self.admitted < safe_ceiling
            && self.sample.cpu_percent.unwrap_or(0.0) < 80.0
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
            memory_safe: memory_worker_limit(&self.sample) >= self.admitted,
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

fn memory_worker_limit(sample: &SystemSample) -> u8 {
    let (Some(total), Some(available)) = (sample.total_memory, sample.available_memory) else {
        return HARD_WORKER_CEILING;
    };
    let reserve = MINIMUM_MEMORY_RESERVE.max(total / 10);
    let usable = available.saturating_sub(reserve);
    (usable / WORKER_MEMORY_BUDGET)
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
        assert_eq!(memory_worker_limit(&sample), 6);
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
            memory_worker_limit(&SystemSample::default()),
            HARD_WORKER_CEILING
        );
    }
}
