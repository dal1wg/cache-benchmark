//! cache-benchmark: a high-performance benchmark tool for Redis, Valkey, Tair
//! and any RESP-compatible database. Pure Rust port of resp-benchmark (Python
//! bindings removed).

mod async_flag;
mod auto_connection;
mod bench;
pub mod client;
mod command;
mod shared_context;

use crate::command::{LuaGenerator, PlaceholderGenerator};
use cb_histogram::HistogramData;
use ctrlc;
use serde::{Deserialize, Serialize};

/// Everything needed to run one benchmark case.
pub struct Case {
    pub command: Box<dyn command::CommandGenerator>,
    pub connections: u64,
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
