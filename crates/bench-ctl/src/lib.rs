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
