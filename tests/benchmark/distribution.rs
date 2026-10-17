use cache_benchmark::command::distribution::DistributionEnum;
use rand::thread_rng;

fn sample_n(d: &mut DistributionEnum, n: usize) -> Vec<u64> {
    let mut rng = thread_rng();
    (0..n).map(|_| d.sample(&mut rng)).collect()
}

#[test]
fn uniform_within_bounds() {
    let mut d = DistributionEnum::try_new("uniform", 100).unwrap();
    for v in sample_n(&mut d, 1000) {
        assert!(v < 100, "uniform out of range: {}", v);
    }
}

#[test]
fn uniform_covers_range() {
    let mut d = DistributionEnum::try_new("uniform", 10).unwrap();
    let seen: std::collections::HashSet<u64> = sample_n(&mut d, 2000).into_iter().collect();
    for k in 0..10u64 {
        assert!(seen.contains(&k), "uniform never produced {}", k);
    }
}

#[test]
fn sequence_ordered() {
    let mut d = DistributionEnum::try_new("sequence", 5).unwrap();
    let mut rng = thread_rng();
    assert_eq!(d.sample(&mut rng), 0);
    assert_eq!(d.sample(&mut rng), 1);
    assert_eq!(d.sample(&mut rng), 2);
    assert_eq!(d.sample(&mut rng), 3);
    assert_eq!(d.sample(&mut rng), 4);
    assert_eq!(d.sample(&mut rng), 0, "sequence must wrap");
}

#[test]
fn sequence_shared_across_clones() {
    // clones share the atomic counter (sequence generators must not repeat
    // values across threads)
    let mut a = DistributionEnum::try_new("sequence", 1000).unwrap();
    let mut b = a.clone();
    let mut rng = thread_rng();
    assert_eq!(a.sample(&mut rng), 0);
    assert_eq!(b.sample(&mut rng), 1);
    assert_eq!(a.sample(&mut rng), 2);
    assert_eq!(b.sample(&mut rng), 3);
}

#[test]
fn zipfian_within_bounds() {
    // rand_distr::Zipf samples [1, N] inclusive (inherited resp-benchmark semantics)
    let mut d = DistributionEnum::try_new("zipfian", 1000).unwrap();
    for v in sample_n(&mut d, 1000) {
        assert!(v >= 1 && v <= 1000, "zipfian out of [1,1000]: {}", v);
    }
}

#[test]
fn zipfian_skews_small() {
    // with exponent 1.03, low indices dominate; >80% of samples below 1/4 range
    let mut d = DistributionEnum::try_new("zipfian", 1000).unwrap();
    let samples = sample_n(&mut d, 2000);
    let low = samples.iter().filter(|v| **v < 250).count();
    assert!(
        low as f64 / samples.len() as f64 > 0.8,
        "zipfian not skewed enough: {}% below quarter",
        100.0 * low as f64 / samples.len() as f64
    );
}

#[test]
fn unknown_distribution_is_error() {
    let err = DistributionEnum::try_new("gaussian", 10).unwrap_err();
    assert!(err.contains("Unknown distribution"), "err: {}", err);
}
