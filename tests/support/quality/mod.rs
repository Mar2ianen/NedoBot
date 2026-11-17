pub mod generators;
pub mod model;
pub mod report;

use proptest::test_runner::{Config, RngSeed, TestRunner};

pub const DEFAULT_REPLAY_SEED: u64 = 0x01_02_03_04_05_06_07_08;
pub const PROPERTY_CASES: u32 = 256;

pub fn replay_seed() -> u64 {
    std::env::var("NEDOBOT_PROPTEST_SEED")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("NEDOBOT_PROPTEST_SEED must be an unsigned integer")
        })
        .unwrap_or(DEFAULT_REPLAY_SEED)
}

pub fn fixed_runner(seed: u64, cases: u32) -> TestRunner {
    TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    })
}
