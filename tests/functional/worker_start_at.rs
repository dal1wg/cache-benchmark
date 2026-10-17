//! Functional tests: --start-at synchronized-start behavior.

use cache_benchmark_tests::run_bin;
use std::time::{Duration, Instant};

fn start_mock_blocking() -> cache_benchmark_tests::MockRespHandle {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(async { cache_benchmark_tests::start_mock(Default::default()).await.unwrap() });
    std::mem::forget(rt);
    handle
}

#[test]
fn start_at_future_waits_before_first_command() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let start = Instant::now();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3; // start 3s out

    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "1", "-n", "10", "-q", "--json", "--start-at", &at.to_string(), "PING"],
    );
    let elapsed = start.elapsed();
    assert_eq!(code, Some(0), "out: {}", out);
    assert!(
        elapsed >= Duration::from_secs(2),
        "must wait for the start-at timestamp, waited {:?}",
        elapsed
    );
    assert!(server_cmd_count(&server) >= 10);
}

#[test]
fn start_at_past_starts_immediately() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let start = Instant::now();
    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "1", "-n", "10", "-q", "--json", "--start-at", "1000", "PING"],
    );
    let elapsed = start.elapsed();
    assert_eq!(code, Some(0), "out: {}", out);
    assert!(
        elapsed < Duration::from_secs(2),
        "past timestamp must start immediately, took {:?}",
        elapsed
    );
}

fn server_cmd_count(server: &cache_benchmark_tests::MockRespHandle) -> u64 {
    // dedicated short-lived runtime: the handle's counter is an async mutex
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { server.cmd_count().await })
}
