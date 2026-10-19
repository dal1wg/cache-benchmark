use crate::async_flag::AsyncFlag;
use cb_histogram::Histogram;
use std::option::Option;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone)]
pub struct SharedContext {
    pub is_loading: bool,
    // limit by max_count
    current_count: Arc<AtomicU64>,
    max_count: u64,

    // limit by max_seconds
    instant: Arc<OnceLock<Instant>>,
    max_seconds: u64,

    // stop flag
    stop_flag: AsyncFlag,

    // histogram
    pub histogram: Arc<Histogram>,

    // cluster topology (filled once at startup, read at the end)
    cluster_nodes: Arc<Mutex<Option<Vec<String>>>>,
}

impl SharedContext {
    pub fn new(max_count: u64, max_seconds: u64, is_loading: bool) -> Self {
        SharedContext {
            is_loading,
            current_count: Arc::new(AtomicU64::new(0)),
            max_count,
            instant: Arc::new(OnceLock::new()),
            max_seconds,
            stop_flag: AsyncFlag::new(),

            histogram: Arc::new(Histogram::new()),

            cluster_nodes: Arc::new(Mutex::new(None)),
        }
    }

    pub fn stop(&mut self) {
        self.stop_flag.set_flag();
    }

    pub async fn wait_stop(&mut self) {
        self.stop_flag.wait_flag().await;
    }

    pub fn start_timer(&mut self) {
        let _ = self.instant.set(Instant::now());
    }

    pub fn set_cluster_nodes(&self, nodes: Vec<String>) {
        let mut guard = self.cluster_nodes.lock().unwrap();
        *guard = Some(nodes);
    }

    pub fn take_cluster_nodes(&self) -> Vec<String> {
        self.cluster_nodes.lock().unwrap().take().unwrap_or_default()
    }

    pub fn fetch(&self, count: u64) -> u64 {
        let mut result = count;
        if self.max_count != 0 {
            // atomic claim: loop until we secure min(count, remaining) without
            // over-issuing under concurrency (fetch_add-then-check races)
            loop {
                let current = self.current_count.load(std::sync::atomic::Ordering::Relaxed);
                if current >= self.max_count {
                    return 0;
                }
                let want = count.min(self.max_count - current);
                match self.current_count.compare_exchange(
                    current,
                    current + want,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        result = want;
                        break;
                    }
                    Err(_) => continue, // another thread claimed; retry
                }
            }
        }

        if self.max_seconds != 0
            && self.instant.get().is_some_and(|start| start.elapsed().as_secs() >= self.max_seconds)
        {
            return 0;
        }

        if self.stop_flag.flag() {
            return 0;
        }

        result
    }
}
