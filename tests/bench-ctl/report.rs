use cache_bench_ctl::hosts::Host;
use cache_bench_ctl::report::aggregate;
use cache_bench_ctl::WorkerResult;
use cb_histogram::{Histogram, HistogramData};

fn host(addr: &str) -> Host {
    Host {
        address: addr.to_string(),
        port: 22,
        user: "root".to_string(),
        arch: String::new(),
        remote_dir: "/tmp/cache-benchmark".to_string(),
        password: String::new(),
    }
}

fn result_with_histogram(tag: &str, qps: f64, latencies_us: &[u64]) -> WorkerResult {
    let h = Histogram::new();
    for l in latencies_us {
        h.record(*l);
    }
    let histogram = HistogramData::from_histogram(&h);
    let avg_ms = histogram.avg_us() as f64 / 1000.0;
    let p99_ms = histogram.percentile_us(0.99) as f64 / 1000.0;
    let count = latencies_us.len() as u64;
    let _ = tag;
    WorkerResult {
        qps,
        avg_latency_ms: avg_ms,
        p99_latency_ms: p99_ms,
        connections: 8,
        count,
        elapsed_secs: 10.0,
        histogram: Some(histogram),
        cluster_nodes: vec![],
    }
}

#[test]
fn qps_is_summed() {
    let results = vec![
        (host("a"), Ok(result_with_histogram("a", 100.0, &[100]))),
        (host("b"), Ok(result_with_histogram("b", 250.5, &[100]))),
    ];
    let report = aggregate(&results);
    assert!((report.total_qps - 350.5).abs() < 1e-9);
    assert_eq!(report.total_count, 2);
    assert_eq!(report.ok_count, 2);
}

#[test]
fn percentiles_from_merged_histogram() {
    // host A: 100 samples at 1ms; host B: 100 samples at 3ms
    let a: Vec<u64> = vec![1000; 100];
    let b: Vec<u64> = vec![3000; 100];
    let results = vec![
        (host("a"), Ok(result_with_histogram("a", 10.0, &a))),
        (host("b"), Ok(result_with_histogram("b", 10.0, &b))),
    ];
    let report = aggregate(&results);
    // lower half of merged samples is 1ms -> p50 = 1ms
    assert_eq!(report.p50_latency_ms, 1.0);
    // upper 1% is in the 3ms bucket
    assert_eq!(report.p99_latency_ms, 3.0);
    // average is exactly 2ms (bucket-wise)
    assert_eq!(report.avg_latency_ms, 2.0);
}

#[test]
fn single_host_percentiles_passthrough() {
    let lat: Vec<u64> = vec![500; 10];
    let results = vec![(host("a"), Ok(result_with_histogram("a", 5.0, &lat)))];
    let report = aggregate(&results);
    assert_eq!(report.p50_latency_ms, 0.5);
    assert_eq!(report.p99_latency_ms, 0.5);
}

#[test]
fn failed_host_marked_but_others_aggregate() {
    let ok: Vec<u64> = vec![1000; 50];
    let results = vec![
        (host("good"), Ok(result_with_histogram("good", 100.0, &ok))),
        (host("bad"), Err(anyhow::anyhow!("ssh refused"))),
    ];
    let report = aggregate(&results);
    assert_eq!(report.ok_count, 1);
    assert!((report.total_qps - 100.0).abs() < 1e-9, "failed host contributes no qps");
    assert_eq!(report.hosts.len(), 2);
    let bad = report.hosts.iter().find(|h| h.host == "bad").unwrap();
    assert!(!bad.ok);
    assert!(bad.error.as_deref().unwrap().contains("ssh refused"));
    assert!(bad.qps.is_none());
}

#[test]
fn cluster_nodes_deduplicated() {
    let mut r1 = result_with_histogram("a", 1.0, &[100]);
    r1.cluster_nodes = vec!["10.0.0.1:6379".into(), "10.0.0.2:6379".into()];
    let mut r2 = result_with_histogram("b", 1.0, &[100]);
    r2.cluster_nodes = vec!["10.0.0.2:6379".into(), "10.0.0.3:6379".into()];
    let results = vec![
        (host("a"), Ok(r1)),
        (host("b"), Ok(r2)),
    ];
    let report = aggregate(&results);
    assert_eq!(report.cluster_nodes, vec!["10.0.0.1:6379", "10.0.0.2:6379", "10.0.0.3:6379"]);
}

#[test]
fn all_failed_report_is_coherent() {
    let results = vec![(host("x"), Err(anyhow::anyhow!("boom")))];
    let report = aggregate(&results);
    assert_eq!(report.ok_count, 0);
    assert_eq!(report.total_qps, 0.0);
    assert_eq!(report.total_count, 0);
    assert_eq!(report.p50_latency_ms, 0.0);
}

#[test]
fn report_json_serializable() {
    let ok: Vec<u64> = vec![1000; 10];
    let results = vec![(host("a"), Ok(result_with_histogram("a", 5.0, &ok)))];
    let report = aggregate(&results);
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains("\"total_qps\""));
    assert!(json.contains("\"ok_count\""));
    let _back: serde_json::Value = serde_json::from_str(&json).unwrap();
}

#[test]
fn missing_histogram_falls_back_to_weighted_avg() {
    // worker result without histogram (older worker): avg weighted by count
    let r1 = WorkerResult {
        qps: 10.0,
        avg_latency_ms: 1.0,
        p99_latency_ms: 2.0,
        connections: 4,
        count: 100,
        elapsed_secs: 10.0,
        histogram: None,
        cluster_nodes: vec![],
    };
    let r2 = WorkerResult {
        qps: 20.0,
        avg_latency_ms: 3.0,
        p99_latency_ms: 6.0,
        connections: 4,
        count: 300,
        elapsed_secs: 10.0,
        histogram: None,
        cluster_nodes: vec![],
    };
    let results = vec![(host("a"), Ok(r1)), (host("b"), Ok(r2))];
    let report = aggregate(&results);
    // (1.0*100 + 3.0*300) / 400 = 2.5
    assert!((report.avg_latency_ms - 2.5).abs() < 1e-9);
}
