use cache_benchmark::auto_connection::{AutoConnection, ConnLimiter};
use cb_histogram::Histogram;
use std::sync::Arc;

#[test]
fn limiter_allows_up_to_target_immediately() {
    let l = ConnLimiter::new(4, 2);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        l.wait_new_conn().await; // 1
        l.wait_new_conn().await; // 2
    });
    assert_eq!(l.get_active_conn(), 2);
    assert_eq!(l.get_target_conn(), 2);
}

#[test]
fn limiter_blocks_beyond_target_until_add() {
    let l = Arc::new(ConnLimiter::new(4, 1));

    // consume the single allowed connection first (fast path)
    {
        let l2 = l.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async { l2.wait_new_conn().await; });
    }
    assert_eq!(l.get_active_conn(), 1);

    // second waiter must block at target=1
    let l2 = l.clone();
    let blocked = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            l2.wait_new_conn().await;
        });
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!blocked.is_finished(), "second wait should block at target=1");

    l.add_conn(); // raises target to 2, notifies the waiter
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !blocked.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(blocked.is_finished(), "waiter must finish after add_conn");
    assert_eq!(l.get_target_conn(), 2);
    assert_eq!(l.get_active_conn(), 2);
}

#[test]
fn limiter_add_caps_at_total() {
    let l = ConnLimiter::new(2, 0);
    l.add_conn();
    l.add_conn();
    assert_eq!(l.get_target_conn(), 2);
    l.add_conn(); // no-op beyond total
    assert_eq!(l.get_target_conn(), 2);
}

#[test]
fn auto_connection_fixed_mode_is_ready() {
    let a = AutoConnection::new(8, 4);
    assert!(a.ready, "explicit connection count => ready immediately");
    assert_eq!(a.active_conn(), 0, "connections activate lazily");
}

#[test]
fn auto_connection_auto_mode_adjust_locks_when_qps_stalls() {
    let mut a = AutoConnection::new(0, 2);
    assert!(!a.ready, "auto mode starts not-ready");
    let h = Histogram::new();

    // simulate some traffic so qps > 0
    for _ in 0..1000 {
        h.record(100);
    }

    // t0: elapsed >= 0.5s, last_qps=0 -> adds connections, records qps
    std::thread::sleep(std::time::Duration::from_millis(600));
    a.adjust(&h);
    assert!(!a.ready);

    // no new traffic: qps stays equal, growth <= 30% -> lock in
    std::thread::sleep(std::time::Duration::from_millis(3100));
    a.adjust(&h);
    assert!(a.ready, "auto mode must lock in when qps stops growing");
}

#[test]
fn auto_connection_auto_mode_grows_when_qps_doubles() {
    let mut a = AutoConnection::new(0, 2);
    let h = Histogram::new();
    for _ in 0..1000 {
        h.record(100);
    }
    std::thread::sleep(std::time::Duration::from_millis(600));
    a.adjust(&h); // baseline: last_qps set, at least 1 connection added
    let after_baseline = a.target_conn();
    assert!(after_baseline >= 1, "first adjust must add at least one connection");

    // double the traffic: qps > last * 1.3 -> another growth round
    for _ in 0..3000 {
        h.record(100);
    }
    std::thread::sleep(std::time::Duration::from_millis(700));
    a.adjust(&h);
    assert!(!a.ready, "still growing: not ready");
}
