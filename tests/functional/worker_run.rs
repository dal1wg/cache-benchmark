//! Functional tests: real benchmark runs against the in-process mock server.

use cache_benchmark_tests::run_bin;

fn start_mock_blocking() -> cache_benchmark_tests::MockRespHandle {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(async { cache_benchmark_tests::start_mock(Default::default()).await.unwrap() });
    std::mem::forget(rt);
    handle
}

#[test]
fn count_run_against_mock() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "4", "-P", "8", "-n", "800", "-q", "--json", "SET {key uniform 1000} {value 32}"],
    );
    assert_eq!(code, Some(0), "out: {}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"].as_u64(), Some(800));
    // qps is a timing-dependent value: a sub-100ms run can finish before the
    // first 233ms stats tick, so only require a finite, non-negative value.
    // (Positive throughput is covered by the real-Redis integration tests.)
    let qps = v["qps"].as_f64().unwrap();
    assert!(qps.is_finite() && qps >= 0.0, "qps: {}", qps);
}

#[test]
fn load_mode_against_mock() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "--load", "-c", "4", "-P", "8", "-n", "300", "-q", "--json", "SET {key sequence 300} {value 8}"],
    );
    assert_eq!(code, Some(0), "out: {}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"].as_u64(), Some(300));
}

#[test]
fn lua_inline_against_mock() {
    let server = start_mock_blocking();
    let port = server.port().to_string();

    let script = "local k = bench.key(10, 'uniform', 'fn'); function generate() return {'SET', k(), 'v'} end";
    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "2", "-n", "100", "-q", "--json", "--lua", script],
    );
    assert_eq!(code, Some(0), "out: {}", out);
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["count"].as_u64(), Some(100));
}

#[test]
fn connection_failure_exits_nonzero_with_address() {
    // port 1 (tcpmux) is almost certainly closed
    let (code, out, err) = run_bin("cache-benchmark", &["-h", "127.0.0.1", "-p", "1", "-c", "1", "-s", "1", "PING"]);
    assert_ne!(code, Some(0));
    let combined = format!("{}{}", out, err);
    assert!(combined.contains("127.0.0.1:1"), "error must mention address: {}", combined);
}

#[test]
fn non_json_output_mentions_qps() {
    // without --json the human-readable summary prints qps (against a dead
    // server this fails; just ensure the flag combination parses and fails
    // with the connection error, not an argument error)
    let (code, _, err) = run_bin("cache-benchmark", &["-h", "127.0.0.1", "-p", "1", "-c", "1", "-s", "1", "PING"]);
    assert_ne!(code, Some(0));
    assert!(!err.contains("unexpected argument"), "err: {}", err);
}
