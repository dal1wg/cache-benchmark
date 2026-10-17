use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use rand::distributions::Distribution;

#[derive(Clone, Debug)]
pub enum DistributionEnum {
    Uniform(rand::distributions::Uniform<u64>),
    Zipfian(rand_distr::Zipf<f64>),
    Sequence(SequenceDistribution),
}

impl DistributionEnum {
    pub fn new(s: &str, range: u64) -> Self {
        match Self::try_new(s, range) {
            Ok(d) => d,
            Err(msg) => panic!("{}", msg),
        }
    }
    /// Testable variant: returns an error message instead of panicking.
    pub fn try_new(s: &str, range: u64) -> Result<Self, String> {
        match s {
            "uniform" => Ok(Self::Uniform(rand::distributions::Uniform::new(0, range))),
            "zipfian" => rand_distr::Zipf::new(range, 1.03)
                .map(Self::Zipfian)
                .map_err(|e| format!("invalid zipfian distribution for range {}: {}", range, e)),
            "sequence" => Ok(Self::Sequence(SequenceDistribution::new(range))),
            other => Err(format!("Unknown distribution: {}", other)),
        }
    }
    pub fn sample(&mut self, rng: &mut impl rand::Rng) -> u64 {
        match self {
            Self::Uniform(d) => d.sample(rng),
            Self::Zipfian(d) => d.sample(rng) as u64,
            Self::Sequence(d) => d.sample(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SequenceDistribution {
    range: u64,
    current: Arc<AtomicU64>,
}

impl SequenceDistribution {
    fn new(range: u64) -> Self {
        Self { range, current: Arc::new(AtomicU64::new(0)) }
    }
    fn sample(&mut self) -> u64 {
        let ret = self.current.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ret % self.range
    }
}
