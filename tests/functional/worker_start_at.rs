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
        .as_secs_f64()
        + 3.0; // start 3s out

    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "1", "-n", "10", "-q", "--json", "--start-at", &format!("{:.3}", at)],
    );
    let elapsed = start.elapsed();
    assert_eq!(code, Some(0), "out: {}", out);
    assert!(
        elapsed >= Duration::from_millis(2900),
        "must wait for the fractional start-at timestamp, waited {:?}",
        elapsed
    );
    assert!(server_cmd_count(&server) >= 10);
}

#[test]
fn start_at_millisecond_precision() {
    // fractional part must be honored: 1.3s out means ~1.3s wait, not 1s or 2s
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let start = Instant::now();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 1.3;

    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "1", "-n", "5", "-q", "--json", "--start-at", &format!("{:.3}", at)],
    );
    let elapsed = start.elapsed();
    assert_eq!(code, Some(0), "out: {}", out);
    assert!(
        elapsed >= Duration::from_millis(1200) && elapsed <= Duration::from_millis(2000),
        "1.3s deadline must wait ~1.3s, waited {:?}",
        elapsed
    );
}

#[test]
fn start_at_past_starts_immediately() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let start = Instant::now();
    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "1", "-n", "10", "-q", "--json", "--start-at", "1000.5"],
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
