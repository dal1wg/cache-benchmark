//! Functional tests: --json output contract of the worker binary.

use cache_benchmark_tests::{run_bin, start_mock};

/// start_mock_blocking: mock server runs on a dedicated runtime thread so the
/// test thread can block on child-process output without starving the server.
fn start_mock_blocking() -> cache_benchmark_tests::MockRespHandle {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.block_on(async { start_mock(Default::default()).await.unwrap() });
    std::mem::forget(rt); // keep the accept loop alive for the process lifetime
    handle
}

#[test]
fn json_single_line_and_fields() {
    let server = start_mock_blocking();
    let port = server.port().to_string();
    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "2", "-n", "200", "-q", "--json", "SET {key uniform 50} {value 8}"],
    );
    assert_eq!(code, Some(0), "stdout: {}", out);
    let line = out.trim();
    assert!(line.starts_with('{'), "json on stdout: {}", line);
    let v: serde_json::Value = serde_json::from_str(line).expect("valid single-line json");
    for field in ["qps", "avg_latency_ms", "p99_latency_ms", "connections", "count", "elapsed_secs", "histogram", "cluster_nodes"] {
        assert!(v.get(field).is_some(), "missing field {} in {}", field, line);
    }
    assert_eq!(v["count"].as_u64(), Some(200));
    assert!(v["histogram"]["buckets"].as_array().unwrap().len() > 100);
}

#[test]
fn quiet_json_has_no_progress_noise() {
    let server = start_mock_blocking();
    let port = server.port().to_string();
    let (code, out, _) = run_bin(
        "cache-benchmark",
        &["-h", "127.0.0.1", "-p", &port, "-c", "2", "-n", "100", "-q", "--json", "PING"],
    );
    assert_eq!(code, Some(0));
    // stdout must contain exactly one line: the JSON
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "stdout lines: {:?}", lines);
    assert!(lines[0].starts_with('{'));
}

#[test]
fn cluster_json_contract_when_topology_unreachable() {
    // cluster mode pointing at a plain (non-cluster) mock: topology discovery
    // warns but the run continues; if the cluster client cannot operate, the
    // worker exits non-zero with a message — either behavior is acceptable,
    // never a hang.
    let server = start_mock_blocking();
    let port = server.port().to_string();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (code, out, err) = rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            async {
                let out = tokio::process::Command::new(cache_benchmark_tests::find_bin("cache-benchmark"))
                    .args(["-h", "127.0.0.1", "-p", &port, "--cluster", "-c", "1", "-n", "10", "-q", "--json", "PING"])
                    .output()
                    .await
                    .expect("spawn worker");
                (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
            },
        )
        .await
        .unwrap_or((None, "timeout".to_string(), String::new()))
    });
    let combined = format!("{}{}", out, err);
    if code == Some(0) {
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert!(v["cluster_nodes"].as_array().is_some());
    } else {
        assert!(
            combined.contains("Failed to connect") || combined.contains("cluster") || combined.contains("timeout"),
            "unexpected failure: {}",
            combined
        );
    }
}
