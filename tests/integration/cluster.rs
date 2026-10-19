//! Integration tests against a REAL Redis Cluster.
//! Skipped unless `CLUSTER_SEED` (host[:port] of any cluster node) is set:
//!   CLUSTER_SEED=192.168.102.34:16385 cargo test --test integration_cluster -- --ignored

use cache_benchmark_tests::{env_target, split_addr};

fn cluster_target() -> Option<(String, u16)> {
    env_target("CLUSTER_SEED").map(|a| split_addr(&a, 6379))
}

fn cluster_cfg(host: &str, port: u16) -> cache_benchmark::client::ClientConfig {
    cache_benchmark::client::ClientConfig {
        cluster: true,
        address: format!("{}:{}", host, port),
        username: String::new(),
        password: String::new(),
        tls: false,
        timeout: 10,
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}

async fn node_stats(host: &str, port: u16) -> Option<u64> {
    let client = redis::Client::open(format!("redis://{}:{}", host, port)).ok()?;
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    let info: String = redis::cmd("INFO").arg("stats").query_async(&mut conn).await.ok()?;
    for line in info.lines() {
        if let Some(v) = line.strip_prefix("total_commands_processed:") {
            return v.trim().parse().ok();
        }
    }
    None
}

#[test]
#[ignore]
fn topology_discovery_matches_cluster_nodes() {
    let (host, port) = match cluster_target() {
        Some(t) => t,
        None => {
            eprintln!("CLUSTER_SEED not set; skipping");
            return;
        }
    };
    let cfg = cluster_cfg(&host, port);
    let (masters, replicas, covered) = rt()
        .block_on(async { cfg.discover_cluster_topology().await })
        .expect("topology discovery must succeed against a real cluster");
    assert!(masters.len() >= 1, "at least one master");
    assert!(covered >= 16384, "slots fully covered, got {}", covered);
    let _ = replicas;

    // masters from CLUSTER SLOTS must actually be reachable
    for m in &masters {
        let (h, p) = split_addr(m, 6379);
        let ok = rt().block_on(async { node_stats(&h, p).await }).is_some();
        assert!(ok, "master {} not reachable", m);
    }
}

#[test]
#[ignore]
fn cluster_pipeline_no_crosslot_error() {
    // regression: cluster + pipeline used to fail with CrossSlot
    let (host, port) = match cluster_target() {
        Some(t) => t,
        None => {
            eprintln!("CLUSTER_SEED not set; skipping");
            return;
        }
    };
    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator("SET {key uniform 100000} {value 8}", false),
        connections: 8,
        max_connections: 1024,
        count: 2000,
        target: 0,
        seconds: 0,
        pipeline: 16, // the previously-failing configuration
        short_connection: false,
    };
    let result = cache_benchmark::run_benchmark(cluster_cfg(&host, port), vec![0], case, false, true);
    assert_eq!(result.count, 2000, "cluster pipeline must complete all commands");
    assert!(!result.cluster_nodes.is_empty(), "cluster_nodes populated");
}

#[test]
#[ignore]
fn load_spreads_evenly_across_masters() {
    let (host, port) = match cluster_target() {
        Some(t) => t,
        None => {
            eprintln!("CLUSTER_SEED not set; skipping");
            return;
        }
    };
    let cfg = cluster_cfg(&host, port);
    let (masters, _, _) = rt()
        .block_on(async { cfg.discover_cluster_topology().await })
        .expect("topology");
    assert!(masters.len() >= 2, "this check needs >= 2 masters");

    let rth = rt();
    // snapshot ops before
    let mut before = Vec::new();
    for m in &masters {
        let (h, p) = split_addr(m, 6379);
        before.push(rth.block_on(async { node_stats(&h, p).await }).expect("stats before"));
    }

    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator("SET {key uniform 100000} {value 8}", false),
        connections: 8,
        max_connections: 1024,
        count: 6000,
        target: 0,
        seconds: 0,
        pipeline: 16,
        short_connection: false,
    };
    let result = cache_benchmark::run_benchmark(cluster_cfg(&host, port), vec![0], case, false, true);
    assert_eq!(result.count, 6000);

    // snapshot ops after; deltas per master must be within 15% of even share
    let mut deltas = Vec::new();
    for (i, m) in masters.iter().enumerate() {
        let (h, p) = split_addr(m, 6379);
        let after = rth.block_on(async { node_stats(&h, p).await }).expect("stats after");
        deltas.push(after.saturating_sub(before[i]));
    }
    let total: u64 = deltas.iter().sum();
    let even = total as f64 / deltas.len() as f64;
    for d in &deltas {
        let dev = (*d as f64 - even).abs() / even.max(1.0);
        assert!(
            dev < 0.15,
            "load uneven: deltas {:?}, even share {:.0}, deviation {:.1}%",
            deltas,
            even,
            dev * 100.0
        );
    }
}
