use cache_benchmark::shared_context::SharedContext;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn fetch_without_limit_returns_requested() {
    let ctx = SharedContext::new(0, 0, false);
    assert_eq!(ctx.fetch(16), 16);
    assert_eq!(ctx.fetch(1), 1);
}

#[test]
fn fetch_respects_max_count_exactly() {
    let ctx = SharedContext::new(100, 0, false);
    assert_eq!(ctx.fetch(60), 60);
    assert_eq!(ctx.fetch(60), 40, "second fetch clamped to remaining");
    assert_eq!(ctx.fetch(1), 0, "exhausted");
}

#[test]
fn fetch_reports_zero_after_max() {
    let ctx = SharedContext::new(10, 0, false);
    assert_eq!(ctx.fetch(10), 10);
    assert_eq!(ctx.fetch(10), 0);
    assert_eq!(ctx.fetch(1), 0);
}

#[test]
fn fetch_stops_when_flag_set() {
    let mut ctx = SharedContext::new(0, 0, false);
    assert_eq!(ctx.fetch(1), 1);
    ctx.stop();
    assert_eq!(ctx.fetch(1), 0);
    // idempotent
    ctx.stop();
    assert_eq!(ctx.fetch(1), 0);
}

#[test]
fn fetch_respects_seconds_limit() {
    let mut ctx = SharedContext::new(0, 1, false);
    ctx.start_timer();
    assert_eq!(ctx.fetch(1), 1, "within the 1s window");
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(ctx.fetch(1), 0, "after the window");
}

#[test]
fn fetch_unlimited_until_timer_started() {
    // seconds limit only applies once the timer started (auto-connection warmup)
    let ctx = SharedContext::new(0, 1, false);
    assert_eq!(ctx.fetch(1), 1);
}

#[test]
fn concurrent_fetch_never_overissues() {
    let ctx = SharedContext::new(1000, 0, false);
    let ctx = Arc::new(ctx);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let ctx = ctx.clone();
        handles.push(std::thread::spawn(move || {
            let mut got = 0u64;
            loop {
                let n = ctx.fetch(7);
                if n == 0 {
                    break;
                }
                got += n; // fetch may return a partial quota (fewer than asked)
            }
            got
        }));
    }
    let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(total, 1000, "concurrent fetch must issue exactly max_count");
}

#[test]
fn cluster_nodes_roundtrip() {
    let ctx = SharedContext::new(0, 0, false);
    assert!(ctx.take_cluster_nodes().is_empty());
    ctx.set_cluster_nodes(vec!["a:1".into(), "b:2".into()]);
    assert_eq!(ctx.take_cluster_nodes(), vec!["a:1".to_string(), "b:2".to_string()]);
    // take consumes
    assert!(ctx.take_cluster_nodes().is_empty());
}
