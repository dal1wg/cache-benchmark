//! cache-benchmark: a high-performance benchmark tool for Redis, Valkey, Tair
//! and any RESP-compatible database. Pure Rust port of resp-benchmark (Python
//! bindings removed).

mod async_flag;
pub mod auto_connection;
mod bench;
mod cluster_pipeline;
pub mod client;
pub mod command;
pub mod shared_context;

use crate::command::{LuaGenerator, PlaceholderGenerator};
use cb_histogram::HistogramData;
use ctrlc;
use serde::{Deserialize, Serialize};

/// Parse a cores specification: "0,1,2" or "0-3,7" (mixed ranges and single
/// cores). Result is sorted and deduplicated. Exits the process on invalid
/// input (CLI semantics); tests use `parse_cores_checked` instead.
pub fn parse_cores(cores: &str) -> Vec<u16> {
    match parse_cores_checked(cores) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Invalid cores range: {}: {}", cores, e);
            std::process::exit(1);
        }
    }
}

/// Pure variant of [`parse_cores`] returning a Result (testable).
pub fn parse_cores_checked(cores: &str) -> anyhow::Result<Vec<u16>> {
    let mut result = Vec::new();
    for part in cores.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let range: Vec<&str> = part.split('-').collect();
        match range.len() {
            1 => {
                let c = range[0].parse::<u16>().map_err(|e| anyhow::anyhow!("{}", e))?;
                result.push(c);
            }
            2 => {
                let start = range[0].parse::<u16>().map_err(|e| anyhow::anyhow!("{}", e))?;
                let end = range[1].parse::<u16>().map_err(|e| anyhow::anyhow!("{}", e))?;
                if end < start {
                    anyhow::bail!("descending range {}-{}", start, end);
                }
                for c in start..=end {
                    result.push(c);
                }
            }
            _ => anyhow::bail!("invalid range part {:?}", part),
        }
    }
    result.sort();
    result.dedup();
    if result.is_empty() {
        anyhow::bail!("no cores specified");
    }
    Ok(result)
}


/// Everything needed to run one benchmark case.
pub struct Case {
    pub command: Box<dyn command::CommandGenerator>,
    pub connections: u64,
    pub max_connections: u64,
    pub count: u64,
    pub target: u64,
    pub seconds: u64,
    pub pipeline: u64,
    pub short_connection: bool,
}

impl Clone for Case {
    fn clone(&self) -> Self {
        Self {
            command: self.command.clone_box(),
            connections: self.connections,
            max_connections: self.max_connections,
            count: self.count,
            target: self.target,
            seconds: self.seconds,
            pipeline: self.pipeline,
            short_connection: self.short_connection,
        }
    }
}

/// Overall result of a benchmark run. `histogram` carries the full bucket
/// distribution so that cache-bench-ctl can merge results from multiple
/// workers bucket-wise and recompute aggregate percentiles correctly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub qps: f64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub connections: u64,
    pub count: u64,
    pub elapsed_secs: f64,
    pub histogram: HistogramData,
    /// Master node addresses when benchmarking a cluster (empty otherwise).
    pub cluster_nodes: Vec<String>,
}

impl Default for BenchResult {
    fn default() -> Self {
        BenchResult {
            qps: 0.0,
            avg_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            connections: 0,
            count: 0,
            elapsed_secs: 0.0,
            histogram: HistogramData::empty(),
            cluster_nodes: Vec::new(),
        }
    }
}

/// Pure Rust entry point (replaces the old PyO3 `benchmark` function).
pub fn run_benchmark(
    client_config: client::ClientConfig,
    cores: Vec<u16>,
    case: Case,
    load: bool,
    quiet: bool,
) -> BenchResult {
    assert!(!cores.is_empty(), "cores must not be empty");
    if load {
        assert_ne!(case.count, 0, "count must be greater than 0 when loading");
    }
    if case.short_connection {
        assert!(!load, "short_connection mode does not support loading");
        assert_eq!(case.pipeline, 1, "short_connection mode does not support pipeline (pipeline must be 1)");
    }

    let _ = ctrlc::set_handler(move || {
        std::process::exit(0);
    });

    bench::do_benchmark(client_config, cores, case, load, quiet)
}

/// Build a command generator from a command string.
pub fn make_generator(command: &str, use_lua: bool) -> Box<dyn command::CommandGenerator> {
    if use_lua {
        Box::new(LuaGenerator::new(command))
    } else {
        Box::new(PlaceholderGenerator::new(command))
    }
}
