//! Integration tests: Lua generator against a REAL Redis.
//! Skipped unless `REDIS_HOST` (host[:port]) is set.

use cache_benchmark_tests::{env_target, split_addr};

fn redis_target() -> Option<(String, u16)> {
    env_target("REDIS_HOST").map(|a| split_addr(&a, 6379))
}

#[test]
#[ignore]
fn lua_writes_verifiable_keys() {
    let (host, port) = match redis_target() {
        Some(t) => t,
        None => {
            eprintln!("REDIS_HOST not set; skipping");
            return;
        }
    };

    // sequence Lua generator over a small range: lua-it:<i>
    let script = r#"
        local seq = bench.key(50, 'sequence', 'lua-it')
        function generate()
            return {'SET', 'lua-it:' .. seq(), 'v'}
        end
    "#;
    let case = cache_benchmark::Case {
        command: cache_benchmark::make_generator(script, true),
        connections: 2,
        max_connections: 1024,
        count: 50,
        target: 0,
        seconds: 0,
        pipeline: 4,
        short_connection: false,
    };
    let cfg = cache_benchmark::client::ClientConfig {
        cluster: false,
        address: format!("{}:{}", host, port),
        username: String::new(),
        password: String::new(),
        tls: false,
        timeout: 10,
    };
    let result = cache_benchmark::run_benchmark(cfg, vec![0], case, true, true);
    assert_eq!(result.count, 50);

    // named sequence generators share state across clones: all 50 keys written
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = redis::Client::open(format!("redis://{}:{}", host, port)).unwrap();
    let mut conn = rt.block_on(async { client.get_multiplexed_async_connection().await.unwrap() });
    for i in 0..50u64 {
        let key = format!("lua-it:{}", i);
        let v: Option<String> = rt.block_on(async {
            redis::cmd("GET").arg(&key).query_async(&mut conn).await.unwrap()
        });
        assert_eq!(v.as_deref(), Some("v"), "key {} missing", key);
    }
    // cleanup
    let keys: Vec<String> = (0..50u64).map(|i| format!("lua-it:{}", i)).collect();
    let _: u64 = rt.block_on(async { redis::cmd("DEL").arg(&keys).query_async(&mut conn).await.unwrap() });
}
