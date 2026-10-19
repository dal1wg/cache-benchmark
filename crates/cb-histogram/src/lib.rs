//! Shared latency histogram.
//!
//! Ported from resp-benchmark `src/histogram.rs`. Lives in its own crate so
//! that `cache-benchmark` (serialization) and `cache-bench-ctl` (bucket-wise
//! merging) share a single source of truth for bucket semantics.
//!
//! PRECISION:
//! - 0-99:   10us  < 1ms
//! - 100-199: 100us < 10ms
//! - 200-299: 1ms  < 100ms
//! - 300-399: 10ms < 1s
//! - 400-499: 100ms < 10s
//! - 500:    >= 10s

use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering};

pub const BUCKET_COUNT: usize = 501;

#[derive(Debug)]
pub struct Histogram {
    cnt: AtomicU64,
    buckets: Vec<AtomicU64>,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub fn new() -> Histogram {
        let mut buckets = Vec::with_capacity(BUCKET_COUNT);
        for _ in 0..BUCKET_COUNT {
            buckets.push(AtomicU64::new(0));
        }
        Histogram {
            cnt: AtomicU64::new(0),
            buckets,
        }
    }

    pub fn record(&self, latency_us: u64) {
        self.record_n(latency_us, 1);
    }

    /// All commands in a pipeline share its completion latency. Update the
    /// counters once per batch instead of contending on them for every command.
    pub fn record_n(&self, latency_us: u64, count: u64) {
        let index = Self::bucket_index(latency_us);
        self.cnt.fetch_add(count, Ordering::Relaxed);
        self.buckets[index].fetch_add(count, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn clear(&self) {
        self.cnt.store(0, Ordering::Relaxed);
        for i in 0..self.buckets.len() {
            self.buckets[i].store(0, Ordering::Relaxed);
        }
    }

    #[allow(dead_code)]
    pub fn un_record(&self, latency_us: u64) {
        let index = Self::bucket_index(latency_us);
        self.cnt.fetch_sub(1, Ordering::Relaxed);
        self.buckets[index].fetch_sub(1, Ordering::Relaxed);
    }

    pub fn bucket_index(latency_us: u64) -> usize {
        match latency_us {
            0..=999 => (latency_us / 10) as usize,                            // <1ms precision 10us
            1_000..=9_999 => 100 + (latency_us / 100) as usize,               // <10ms precision 100us
            10_000..=99_999 => 200 + (latency_us / 1_000) as usize,           // <100ms precision 1ms
            100_000..=999_999 => 300 + (latency_us / 10_000) as usize,        // <1s precision 10ms
            1_000_000..=9_999_999 => 400 + (latency_us / 100_000) as usize,   // <10s precision 100ms
            _ => 500,                                                         // >=10s
        }
    }

    fn bucket_unit_us(index: u64) -> u64 {
        match index {
            0..=99 => index * 10,                 // precision 10us
            100..=199 => (index - 100) * 100,     // precision 100us
            200..=299 => (index - 200) * 1_000,   // precision 1ms
            300..=399 => (index - 300) * 10_000,  // precision 10ms
            400..=499 => (index - 400) * 100_000, // precision 100ms
            _ => 10_000_000,                      // >=10s
        }
    }

    pub fn cnt(&self) -> u64 {
        self.cnt.load(Ordering::Relaxed)
    }

    pub fn avg(&self) -> u64 {
        let cnt = self.cnt();
        if cnt == 0 {
            return 0;
        }
        let mut sum = 0;
        for i in 0..self.buckets.len() {
            sum += Histogram::bucket_unit_us(i as u64) * self.buckets[i].load(Ordering::Relaxed);
        }
        sum / cnt
    }

    pub fn percentile(&self, percentile: f64) -> u64 {
        let cnt = self.cnt();
        if cnt == 0 {
            return 0;
        }
        let mut sum = 0;
        let target = (cnt as f64 * percentile) as u64;
        for i in 0..self.buckets.len() {
            sum += self.buckets[i].load(Ordering::Relaxed);
            if sum > 0 && sum >= target {
                return Histogram::bucket_unit_us(i as u64);
            }
        }
        0
    }

    pub fn humanize_us(latency_us: u64) -> String {
        match latency_us {
            0 => "<0.01ms".to_string(),
            1..=999 => format!("{:.2}ms", latency_us as f64 / 1_000.0),
            1_000..=9_999 => format!("{:.1}ms", latency_us as f64 / 1_000.0),
            10_000..=99_999 => format!("{:.0}ms", latency_us as f64 / 1_000.0),
            100_000..=999_999 => format!("{:.2}s", latency_us as f64 / 1_000_000.0),
            1_000_000..=9_999_999 => format!("{:.1}s", latency_us as f64 / 1_000_000.0),
            _ => ">10s".to_string(),
        }
    }
}

impl Display for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cnt = self.cnt();
        if cnt == 0 {
            return write!(f, "no data");
        }
        let avg = self.avg();
        let p99 = self.percentile(0.99);

        write!(f, "cnt: {}, avg: {}, p99: {}", cnt, Histogram::humanize_us(avg), Histogram::humanize_us(p99))
    }
}

/// Serializable snapshot of a histogram: plain bucket counts that can be
/// merged bucket-wise by `cache-bench-ctl` to recompute aggregate percentiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramData {
    pub buckets: Vec<u64>,
}

impl HistogramData {
    pub fn subtract(&mut self, baseline: &Self) {
        for (count, before) in self.buckets.iter_mut().zip(&baseline.buckets) {
            *count = count.saturating_sub(*before);
        }
    }
    pub fn from_histogram(h: &Histogram) -> Self {
        HistogramData {
            buckets: h.buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
        }
    }

    pub fn empty() -> Self {
        HistogramData {
            buckets: vec![0; BUCKET_COUNT],
        }
    }

    /// Merge another histogram into this one (bucket-wise addition).
    pub fn merge(&mut self, other: &HistogramData) {
        for (a, b) in self.buckets.iter_mut().zip(other.buckets.iter()) {
            *a += b;
        }
    }

    pub fn cnt(&self) -> u64 {
        self.buckets.iter().sum()
    }

    pub fn avg_us(&self) -> u64 {
        let cnt = self.cnt();
        if cnt == 0 {
            return 0;
        }
        let mut sum = 0;
        for (i, &c) in self.buckets.iter().enumerate() {
            sum += Histogram::bucket_unit_us(i as u64) * c;
        }
        sum / cnt
    }

    pub fn percentile_us(&self, percentile: f64) -> u64 {
        let cnt = self.cnt();
        if cnt == 0 {
            return 0;
        }
        let mut sum = 0;
        let target = (cnt as f64 * percentile) as u64;
        for (i, &c) in self.buckets.iter().enumerate() {
            sum += c;
            if sum > 0 && sum >= target {
                return Histogram::bucket_unit_us(i as u64);
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::Histogram;

    #[test]
    fn test() {
        let histogram = Histogram::new();
        for i in 0..1000 {
            histogram.record(i * 1000);
        }
        println!("{}", histogram);

        // PRECISION:
        // 0-99: 10us <1ms
        // 100-199: 100us <10ms
        // 200-299: 1ms <100ms
        // 300-399: 10ms <1s
        // 400-499: 100ms <10s
        // 500: >=10s
        let src = [0, 9, 10, 99, 100, 999, 1_000, 9_999, 10_000, 99_999, 100_000, 999_999, 1_000_000, 9_999_999, 10_000_000, 99_999_999, 100_000_000];
        let dst = [0, 0, 10, 90, 100, 990, 1_000, 9_900, 10_000, 99_000, 100_000, 990_000, 1_000_000, 9_900_000, 10_000_000, 10_000_000, 10_000_000];
        for i in 0..src.len() {
            let histogram = Histogram::new();
            for _ in 0..1000 {
                histogram.record(src[i]);
            }

            println!("src: {} dst: {}", src[i], dst[i]);
            println!("{}", histogram);
            assert_eq!(histogram.cnt(), 1000);
            assert_eq!(histogram.avg(), dst[i]);
            assert_eq!(histogram.percentile(0.0), dst[i]);
            assert_eq!(histogram.percentile(0.5), dst[i]);
            assert_eq!(histogram.percentile(0.99), dst[i]);
            assert_eq!(histogram.percentile(0.999), dst[i]);
            assert_eq!(histogram.percentile(1.0), dst[i]);
        }
    }

    use super::HistogramData;

    #[test]
    fn test_merge_and_percentiles() {
        // two nodes: node A always 1ms, node B always 3ms
        let mut a = HistogramData::empty();
        let mut b = HistogramData::empty();
        for _ in 0..100 {
            a.buckets[Histogram::bucket_index(1_000)] += 1; // 1ms
            b.buckets[Histogram::bucket_index(3_000)] += 1; // 3ms
        }
        a.merge(&b);
        assert_eq!(a.cnt(), 200);
        // p50 should land in the 1ms bucket (first 100 samples)
        assert_eq!(a.percentile_us(0.5), 1_000);
        // p99 should land in the 3ms bucket
        assert_eq!(a.percentile_us(0.99), 3_000);
        // avg should be 2ms
        assert_eq!(a.avg_us(), 2_000);
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn batch_recording_and_warmup_subtraction() {
        let h = Histogram::new();
        h.record_n(123, 64);
        let baseline = HistogramData::from_histogram(&h);
        h.record_n(567, 128);
        let mut measured = HistogramData::from_histogram(&h);
        measured.subtract(&baseline);
        assert_eq!(h.cnt(), 192);
        assert_eq!(measured.cnt(), 128);
        assert_eq!(measured.avg_us(), 560);
        assert_eq!(measured.percentile_us(0.99), 560);
    }
}
