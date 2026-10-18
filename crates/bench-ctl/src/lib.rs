//! cache-bench-ctl: distribute cache-benchmark workers to multiple servers,
//! run them simultaneously, and collect merged results.
//!
//! Library facade so integration tests can exercise the orchestration logic
//! (host parsing, binary matching, report aggregation) directly.

pub mod hosts;
pub mod report;
pub mod transport;

#[cfg(feature = "russh")]
pub mod russh_transport;

use anyhow::{Result, anyhow};
use cb_histogram::HistogramData;
use std::path::{Path, PathBuf};

/// Worker JSON result (subset of cache_benchmark::BenchResult that we need;
/// kept separate so ctl does not depend on the worker crate).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkerResult {
    pub qps: f64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub connections: u64,
    pub count: u64,
    #[allow(dead_code)]
    pub elapsed_secs: f64,
    #[serde(default)]
    pub histogram: Option<HistogramData>,
    #[serde(default)]
    pub cluster_nodes: Vec<String>,
}

/// Normalize `uname -m` architecture names.
pub fn normalize_arch(arch: &str) -> Option<&'static str> {
    match arch.trim() {
        "x86_64" | "amd64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("aarch64"),
        _ => None,
    }
}

/// Pick the worker binary matching the host architecture.
/// Preferred layout: bin_dir/cache-benchmark-<arch> (e.g. cache-benchmark-x86_64).
/// Legacy names with a full target triple (e.g. cache-benchmark-x86_64-unknown-linux-musl)
/// are still accepted.
pub fn find_worker_binary(bin_dir: &Path, arch: &str) -> Result<PathBuf> {
    let arch = normalize_arch(arch).ok_or_else(|| anyhow!("unsupported remote architecture: {}", arch))?;
    let prefix = format!("cache-benchmark-{}", arch);

    // exact short name wins: cache-benchmark-x86_64
    let exact = bin_dir.join(&prefix);
    if exact.is_file() {
        return Ok(exact);
    }

    // otherwise any suffixed variant (legacy triple names etc.), files only
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(bin_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && entry.path().is_file() {
                candidates.push(entry.path());
            }
        }
    }
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no worker binary for arch {} found in {:?} (expected e.g. cache-benchmark-{})", arch, bin_dir, arch))
}

/// Quote a string for safe use as a single shell word.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------- clock offset compensation ----------

/// Result of probing one host's clock against the ctl machine's clock.
#[derive(Debug, Clone, Copy)]
pub struct ClockOffset {
    /// remote clock minus local clock, in seconds (positive = remote ahead)
    pub offset_secs: f64,
    /// round-trip time of the probe exec, in milliseconds
    pub rtt_ms: f64,
    /// whether the remote timestamp had sub-second resolution (`date +%s.%N`)
    pub precise: bool,
}

/// Parse a remote `date` output as fractional unix seconds.
/// Accepts `1792302394.123456789` (GNU `%s.%N`) and `1792302394` (`%s`).
pub fn parse_remote_timestamp(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || !s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// NTP-style offset estimate from one probe.
///
/// `t0`/`t1` are local clock readings immediately before/after the exec;
/// `remote` is the timestamp the host reported for some instant between them.
/// Assuming the exec latency is symmetric, the reading corresponds to the
/// local midpoint `(t0 + t1) / 2`.
pub fn offset_from_probe(t0: f64, t1: f64, remote: f64) -> (f64, f64) {
    let offset = remote - (t0 + t1) / 2.0;
    let rtt_ms = (t1 - t0) * 1000.0;
    (offset, rtt_ms)
}

/// Per-host `--start-at` value so every worker starts at the same true
/// instant, given the host's clock offset (remote = local + offset).
///
/// The worker's sleep deadline is on ITS OWN clock: it fires when
/// remote_clock == ts. We want that instant to be `reference` on the ctl
/// clock, i.e. when remote_clock == reference + offset. Millisecond
/// precision is kept (the worker accepts fractional timestamps).
pub fn compensated_start_at(reference: f64, offset_secs: f64) -> f64 {
    let v = reference + offset_secs;
    if !v.is_finite() || v <= 0.0 {
        return reference.max(0.001);
    }
    v
}
