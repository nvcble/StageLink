//! Process and machine resource sampling.
//!
//! Deliberately cheap and deliberately off the audio path: `sysinfo` walks OS
//! structures and allocates, so this is only ever called from the telemetry
//! thread, never from capture or packetisation.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// A resource snapshot. Every field is `Option` because any of them can be
/// unavailable depending on OS permissions.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetrics {
    /// This process's CPU use, as a percentage of one core (so >100 is normal
    /// for a multithreaded service).
    pub process_cpu_percent: Option<f64>,
    /// Whole-machine CPU use, 0–100.
    pub system_cpu_percent: Option<f64>,
    /// Resident set size of this process, bytes.
    pub process_memory_bytes: Option<u64>,
    pub system_memory_total_bytes: Option<u64>,
    pub system_memory_available_bytes: Option<u64>,
}

/// CPU percentages need two samples separated in time to mean anything.
/// Sampling faster than this returns the previous figures rather than noise.
const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(400);

/// Samples process and system resources.
///
/// Holds a `sysinfo::System` across calls, which is required for CPU
/// percentages (they are computed from the delta between refreshes).
pub struct SystemProbe {
    inner: Mutex<ProbeState>,
    pid: Pid,
}

struct ProbeState {
    system: System,
    last_sampled: Option<Instant>,
    last_metrics: SystemMetrics,
}

impl SystemProbe {
    #[must_use]
    pub fn new() -> Self {
        let pid = Pid::from_u32(std::process::id());
        Self {
            inner: Mutex::new(ProbeState {
                system: System::new(),
                last_sampled: None,
                last_metrics: SystemMetrics::default(),
            }),
            pid,
        }
    }

    /// Returns a fresh snapshot, or the previous one if called too soon.
    pub fn sample(&self) -> SystemMetrics {
        let mut state = self.inner.lock();

        if let Some(last) = state.last_sampled {
            if last.elapsed() < MIN_SAMPLE_INTERVAL {
                return state.last_metrics;
            }
        }

        state.system.refresh_memory();
        state.system.refresh_cpu_usage();
        state.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );

        let (process_cpu_percent, process_memory_bytes) = match state.system.process(self.pid) {
            Some(process) => (
                Some(f64::from(process.cpu_usage())),
                Some(process.memory()),
            ),
            None => (None, None),
        };

        let cpus = state.system.cpus();
        let system_cpu_percent = if cpus.is_empty() {
            None
        } else {
            let total: f32 = cpus.iter().map(sysinfo::Cpu::cpu_usage).sum();
            Some(f64::from(total / cpus.len() as f32))
        };

        let metrics = SystemMetrics {
            process_cpu_percent,
            system_cpu_percent,
            process_memory_bytes,
            system_memory_total_bytes: Some(state.system.total_memory()),
            system_memory_available_bytes: Some(state.system.available_memory()),
        };

        // The very first refresh has no previous sample to diff against, so its
        // CPU figures are 0 by construction. Report them as unknown instead of
        // telling the operator the service is using no CPU.
        let metrics = if state.last_sampled.is_none() {
            SystemMetrics {
                process_cpu_percent: None,
                system_cpu_percent: None,
                ..metrics
            }
        } else {
            metrics
        };

        state.last_sampled = Some(Instant::now());
        state.last_metrics = metrics;
        metrics
    }
}

impl Default for SystemProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sample_reports_memory_but_not_cpu() {
        let probe = SystemProbe::new();
        let metrics = probe.sample();
        // Memory is an absolute reading and is available immediately.
        assert!(
            metrics.process_memory_bytes.is_some_and(|bytes| bytes > 0),
            "this process must report a resident set size"
        );
        assert!(metrics.system_memory_total_bytes.is_some_and(|b| b > 0));
        // CPU needs two refreshes, so it must be reported as unknown, not 0.
        assert_eq!(
            metrics.process_cpu_percent, None,
            "a first-sample CPU figure of 0 would be a fabrication"
        );
    }

    #[test]
    fn a_second_sample_after_the_interval_reports_cpu() {
        let probe = SystemProbe::new();
        probe.sample();
        std::thread::sleep(MIN_SAMPLE_INTERVAL + Duration::from_millis(100));
        let metrics = probe.sample();
        assert!(metrics.process_cpu_percent.is_some());
        assert!(
            metrics
                .system_cpu_percent
                .is_some_and(|cpu| (0.0..=100.0).contains(&cpu)),
            "system CPU must be a percentage, got {:?}",
            metrics.system_cpu_percent
        );
    }

    #[test]
    fn sampling_too_fast_reuses_the_previous_snapshot_instead_of_hammering_the_os() {
        let probe = SystemProbe::new();
        let first = probe.sample();
        let second = probe.sample();
        assert_eq!(first, second);
    }

    #[test]
    fn available_memory_never_exceeds_total() {
        let probe = SystemProbe::new();
        let metrics = probe.sample();
        if let (Some(total), Some(available)) = (
            metrics.system_memory_total_bytes,
            metrics.system_memory_available_bytes,
        ) {
            assert!(available <= total);
        }
    }
}
