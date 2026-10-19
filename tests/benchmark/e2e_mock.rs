//! End-to-end: run a real benchmark against the in-process mock RESP server.
//!
//! `run_benchmark` is a blocking API (called from main in production); tests
//! must NOT call it from inside a tokio runtime ("cannot start a runtime from
//! within a runtime"). The mock server is started on a helper runtime first,
//! then the blocking benchmark call runs on the test thread.

use cache_benchmark::{BenchResult, Case, client::ClientConfig, make_generator, run_benchmark};

fn config(addr: &str) -> ClientConfig {
    ClientConfig {
        cluster: false,
        address: addr.to_string(),
        username: String::new(),
        password: String::new(),
        tls: false,
        timeout: 5,
    }
}

fn case(count: u64, command: &str) -> Case {
    Case {
        command: make_generator(command, false),
        connections: 4,
        max_connections: 1024,
        count,
        target: 0,
        seconds: 0,
        pipeline: 1,
        short_connection: false,
    }
}

/// Start a mock server on a dedicated helper runtime thread; returns the
/// handle once the listener is bound.
fn start_mock_blocking() -> cache_benchmark_tests::MockRespHandle {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(async { cache_benchmark_tests::start_mock(Default::default()).await.unwrap() });
    // keep the runtime alive for the process lifetime: leak it intentionally
    std::mem::forget(rt);
    handle
}

/// Tolerated connection-setup overhead per connection (redis-rs may send
/// HELLO / CLIENT SETINFO during MultiplexedConnection handshake).
const HANDSHAKE_SLOP: u64 = 4;

#[test]
fn benchmark_count_based_against_mock() {
    let server = start_mock_blocking();
    let result = run_benchmark(config(&server.addr()), vec![0], case(1000, "SET {key uniform 100} {value 8}"), false, true);
    assert_eq!(result.count, 1000, "count-based run must issue exactly N commands");
    assert!(result.qps > 0.0);
    assert!(result.p99_latency_ms >= result.avg_latency_ms, "p99 >= avg");
    assert_eq!(result.connections, 4);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mock_count = rt.block_on(async { server.cmd_count().await });
    assert!(
        (1000..=1000 + 4 * HANDSHAKE_SLOP).contains(&mock_count),
        "mock saw {} commands (expected 1000 + handshake slop)",
        mock_count
    );
}

#[test]
fn benchmark_pipeline_against_mock() {
    let server = start_mock_blocking();
    let mut c = case(320, "GET {key uniform 1000}");
    c.pipeline = 16;
    let result = run_benchmark(config(&server.addr()), vec![0], c, false, true);
    assert_eq!(result.count, 320);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mock_count = rt.block_on(async { server.cmd_count().await });
    assert!(
        (320..=320 + 4 * HANDSHAKE_SLOP).contains(&mock_count),
        "mock saw {} commands",
        mock_count
    );
}

#[test]
fn load_mode_against_mock() {
    let server = start_mock_blocking();
    let result = run_benchmark(config(&server.addr()), vec![0], case(500, "SET {key sequence 500} {value 8}"), true, true);
    assert_eq!(result.count, 500);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mock_count = rt.block_on(async { server.cmd_count().await });
    assert!(
        (500..=500 + 2 * HANDSHAKE_SLOP).contains(&mock_count),
        "mock saw {} commands",
        mock_count
    );
}

#[test]
fn json_result_roundtrip() {
    let server = start_mock_blocking();
    let result = run_benchmark(config(&server.addr()), vec![0], case(100, "PING"), false, true);
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"qps\""));
    assert!(json.contains("\"histogram\""));
    assert!(!json.contains('\n'), "--json contract: single line");
    let back: BenchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back.count, result.count);
    assert_eq!(back.histogram.cnt(), 100);
    assert!(back.cluster_nodes.is_empty());
}

#[test]
fn lua_generator_e2e_against_mock() {
    let server = start_mock_blocking();
    let mut c = case(50, "irrelevant");
    c.command = make_generator(
        "local k = bench.key(10, 'sequence', 'e2e'); function generate() return {'SET', k(), 'v'} end",
        true,
    );
    let result = run_benchmark(config(&server.addr()), vec![0], c, false, true);
    assert_eq!(result.count, 50);
}

#[test]
#[should_panic(expected = "count must be greater than 0 when loading")]
fn load_requires_count() {
    let server = start_mock_blocking();
    let c = case(0, "SET {key sequence 10} {value 8}");
    let _ = run_benchmark(config(&server.addr()), vec![0], c, true, true);
}

#[test]
#[should_panic(expected = "short_connection mode does not support pipeline")]
fn short_connection_rejects_pipeline() {
    let server = start_mock_blocking();
    let mut c = case(10, "PING");
    c.pipeline = 4;
    c.short_connection = true;
    let _ = run_benchmark(config(&server.addr()), vec![0], c, false, true);
}

#[test]
fn auto_count_only_reports_nonzero_qps() {
    let server = start_mock_blocking();
    let mut c = case(128, "GET {key uniform 1000}");
    c.connections = 0;
    c.pipeline = 16;
    let result = run_benchmark(config(&server.addr()), vec![0], c, false, true);
    assert_eq!(result.count, 128);
    assert!(result.qps > 0.0);
    assert_eq!(result.histogram.cnt(), result.count);
    assert!((result.qps * result.elapsed_secs - result.count as f64).abs() < 0.01);
}

#[test]
fn pipeline_rate_limit_counts_commands_not_batches() {
    let server = start_mock_blocking();
    let mut c = case(0, "GET {key uniform 1000}");
    c.connections = 1;
    c.pipeline = 16;
    c.seconds = 2;
    c.target = 100;
    let result = run_benchmark(config(&server.addr()), vec![0], c, false, true);
    assert!(result.qps <= 115.0, "-t 100 must limit commands, not batches: {}", result.qps);
    assert!(result.qps >= 50.0, "rate limiting should still make progress: {}", result.qps);
}

#[test]
fn auto_timed_count_latency_and_qps_share_a_window() {
    let server = start_mock_blocking();
    let mut c = case(0, "GET {key uniform 1000}");
    c.connections = 0;
    c.max_connections = 2;
    c.pipeline = 16;
    c.seconds = 1;
    let result = run_benchmark(config(&server.addr()), vec![0], c, false, true);
    assert!(result.count > 0);
    assert_eq!(result.count, result.histogram.cnt());
    assert!((result.qps * result.elapsed_secs - result.count as f64).abs() < 0.01);
}
