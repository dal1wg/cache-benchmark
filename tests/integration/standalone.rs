//! Integration tests against a REAL standalone Redis.
//! Skipped unless `REDIS_HOST` (host[:port]) is set:
//!   REDIS_HOST=192.168.102.34:6379 cargo test --test integration_standalone -- --ignored
//!
//! `run_benchmark` is blocking; tests run it on the test thread and use a
//! helper runtime only for the redis verification connection.

use cache_benchmark_tests::{env_target, split_addr};

fn redis_target() -> Option<(String, u16)> {
    env_target("REDIS_HOST").map(|a| split_addr(&a, 6379))
}

fn client_cfg(host: &str, port: u16) -> cache_benchmark::client::ClientConfig {
    cache_benchmark::client::ClientConfig {
        cluster: false,
        address: format!("{}:{}", host, port),
        username: String::new(),
        password: String::new(),
        tls: false,
        timeout: 10,
    }
}

fn redis_client(host: &str, port: u16) -> redis::Client {
    redis::Client::open(format!("redis://{}:{}", host, port)).unwrap()
}

#[test]
#[ignore]
fn load_writes_verifiable_data() {
    let (host, port) = match redis_target() {
        Some(t) => t,
        None => {
            eprintln!("REDIS_HOST not set; skipping");
            return;
        }
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut conn = rt.block_on(async { redis_client(&host, port).get_multiplexed_async_connection().await.unwrap() });

    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator("SET {key sequence 200} {value 16}", false),
        connections: 2,
        count: 200,
        target: 0,
        seconds: 0,
        pipeline: 8,
        short_connection: false,
    };
    let result = cache_benchmark::run_benchmark(client_cfg(&host, port), vec![0], case, true, true);
    assert_eq!(result.count, 200);

    // verify with an independent connection: every key exists with a 16-byte value
    let checks: Vec<String> = (0..200u64).map(|i| format!("key_{:010}", i)).collect();
    for key in &checks {
        let v: Option<String> = rt.block_on(async {
            redis::cmd("GET").arg(key).query_async(&mut conn).await.unwrap()
        });
        assert_eq!(v.unwrap_or_default().len(), 16, "key {} value invalid", key);
    }

    // cleanup
    let _: u64 = rt.block_on(async { redis::cmd("DEL").arg(&checks).query_async(&mut conn).await.unwrap() });
}

#[test]
#[ignore]
fn sequence_keys_no_gaps() {
    let (host, port) = match redis_target() {
        Some(t) => t,
        None => {
            eprintln!("REDIS_HOST not set; skipping");
            return;
        }
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut conn = rt.block_on(async { redis_client(&host, port).get_multiplexed_async_connection().await.unwrap() });

    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator("SET {key sequence 300} {value 8}", false),
        connections: 4,
        count: 300,
        target: 0,
        seconds: 0,
        pipeline: 8,
        short_connection: false,
    };
    let result = cache_benchmark::run_benchmark(client_cfg(&host, port), vec![0], case, true, true);
    assert_eq!(result.count, 300, "exactly 300 commands issued");

    // load mode serializes generators with a lock: every key exactly once
    let keys: Vec<String> = (0..300u64).map(|i| format!("key_{:010}", i)).collect();
    for key in &keys {
        let exists: bool = rt.block_on(async {
            redis::cmd("EXISTS").arg(key).query_async(&mut conn).await.unwrap()
        });
        assert!(exists, "sequence key {} missing", key);
    }
    let _: u64 = rt.block_on(async { redis::cmd("DEL").arg(&keys).query_async(&mut conn).await.unwrap() });
}

#[test]
#[ignore]
fn get_benchmark_count_matches() {
    let (host, port) = match redis_target() {
        Some(t) => t,
        None => {
            eprintln!("REDIS_HOST not set; skipping");
            return;
        }
    };
    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator("GET {key uniform 100000}", false),
        connections: 8,
        count: 5000,
        target: 0,
        seconds: 0,
        pipeline: 16,
        short_connection: false,
    };
    let result = cache_benchmark::run_benchmark(client_cfg(&host, port), vec![0], case, false, true);
    assert_eq!(result.count, 5000);
    assert!(result.qps > 0.0);
}
