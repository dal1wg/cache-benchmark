use crate::hosts::Host;
use anyhow::Result;
use cb_histogram::HistogramData;
use colored::Colorize;
use serde::Serialize;

/// Per-host entry in the aggregate report.
#[derive(Serialize)]
pub struct HostReport {
    pub host: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p99_latency_ms: Option<f64>,
    pub count: u64,
    pub connections: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate report: sum of per-host QPS + bucket-wise merged latency histogram.
#[derive(Serialize)]
pub struct AggregateReport {
    pub total_qps: f64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub total_count: u64,
    pub hosts: Vec<HostReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cluster_nodes: Vec<String>,
}

pub(crate) fn print_report(
    results: &[(Host, anyhow::Result<crate::WorkerResult>)],
    output_file: &Option<String>,
) -> Result<()> {
    let mut hosts: Vec<HostReport> = Vec::new();
    let mut merged = HistogramData::empty();
    let mut total_qps = 0.0f64;
    let mut total_count = 0u64;
    let mut weighted_avg_ms = 0.0f64;
    let mut cluster_nodes: Vec<String> = Vec::new();
    let mut ok_cnt = 0usize;

    for (host, res) in results {
        let name = host.address.clone();
        match res {
            Ok(r) => {
                ok_cnt += 1;
                total_qps += r.qps;
                total_count += r.count;
                if let Some(h) = &r.histogram {
                    merged.merge(h);
                    // weighted average from buckets (more accurate than mean-of-means)
                    weighted_avg_ms = merged.avg_us() as f64 / 1_000.0;
                } else {
                    weighted_avg_ms += r.avg_latency_ms * r.count as f64;
                }
                for n in &r.cluster_nodes {
                    if !cluster_nodes.contains(n) {
                        cluster_nodes.push(n.clone());
                    }
                }
                hosts.push(HostReport {
                    host: name,
                    ok: true,
                    qps: Some(r.qps),
                    avg_latency_ms: Some(r.avg_latency_ms),
                    p99_latency_ms: Some(r.p99_latency_ms),
                    count: r.count,
                    connections: r.connections,
                    error: None,
                });
            }
            Err(e) => {
                hosts.push(HostReport {
                    host: name,
                    ok: false,
                    qps: None,
                    avg_latency_ms: None,
                    p99_latency_ms: None,
                    count: 0,
                    connections: 0,
                    error: Some(format!("{:#}", e)),
                });
            }
        }
    }

    // normalize weighted avg when histograms were absent
    if ok_cnt > 0 && merged.cnt() == 0 && total_count > 0 {
        weighted_avg_ms /= total_count as f64;
    }

    let report = AggregateReport {
        total_qps,
        avg_latency_ms: weighted_avg_ms,
        p50_latency_ms: merged.percentile_us(0.50) as f64 / 1_000.0,
        p95_latency_ms: merged.percentile_us(0.95) as f64 / 1_000.0,
        p99_latency_ms: merged.percentile_us(0.99) as f64 / 1_000.0,
        total_count,
        hosts,
        cluster_nodes,
    };

    println!("\n{}", "================ Aggregate Report ================".bold());
    println!("{:<20} {:>10} {:>10} {:>10} {:>10}", "host", "qps", "avg", "p99", "count");
    for h in &report.hosts {
        match (h.qps, h.avg_latency_ms, h.p99_latency_ms) {
            (Some(q), Some(a), Some(p)) => {
                println!("{:<20} {:>10.0} {:>8.2}ms {:>8.2}ms {:>10}", h.host, q, a, p, h.count);
            }
            _ => {
                println!("{:<20} {}", h.host, format!("FAILED: {}", h.error.clone().unwrap_or_default()).red());
            }
        }
    }
    println!("{}", "--------------------------------------------------".bold());
    println!(
        "{:<20} {:>10.0} {:>8.2}ms {:>8.2}ms {:>10}",
        format!("TOTAL ({} ok)", ok_cnt).green(), report.total_qps, report.avg_latency_ms, report.p99_latency_ms, report.total_count
    );
    if !report.cluster_nodes.is_empty() {
        println!("cluster masters: {}", report.cluster_nodes.join(", "));
    }
    println!("p50: {:.2}ms  p95: {:.2}ms  p99: {:.2}ms", report.p50_latency_ms, report.p95_latency_ms, report.p99_latency_ms);

    if let Some(path) = output_file {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(path, json)?;
        println!("report written to {}", path);
    }
    Ok(())
}
