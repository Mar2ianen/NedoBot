#[path = "support/adversarial_case_report.rs"]
mod adversarial_case_report;

use std::{collections::BTreeMap, env};

use adversarial_case_report::{
    CaseOrigin, CaseReport, SyntheticUpdateCase, SyntheticUrlCase, ValidityClass,
};
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestCaseError, TestError, TestRunner},
};

const DEFAULT_REPLAY_SEED: u64 = 0x01_02_03_04_05_06_07_08;
const PROPERTY_CASES: u32 = 256;

fn replay_seed() -> u64 {
    env::var("NEDOBOT_PROPTEST_SEED")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("NEDOBOT_PROPTEST_SEED must be an unsigned integer")
        })
        .unwrap_or(DEFAULT_REPLAY_SEED)
}

fn fixed_runner(seed: u64, cases: u32) -> TestRunner {
    TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    })
}

fn url_case_report(case_id: &str, seed: u64, accepted: u32, rejected: u32) -> CaseReport {
    CaseReport {
        case_id: case_id.to_owned(),
        invariant_id: "URL-01".to_owned(),
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PropertyGenerated,
        seed: Some(seed),
        generated: accepted.saturating_add(rejected),
        accepted,
        rejected,
    }
}

#[test]
fn synthetic_public_corpora_load_with_explicit_validity() {
    let urls = include_str!("../testing/corpus/public/synthetic_urls.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<SyntheticUrlCase>(line).unwrap())
        .collect::<Vec<_>>();
    let updates = include_str!("../testing/corpus/public/synthetic_updates.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<SyntheticUpdateCase>(line).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(urls.len(), 24);
    assert_eq!(updates.len(), 6);
    assert!(urls.iter().all(|case| case.validity == "V2_synthetic"));
    assert!(updates.iter().all(|case| {
        case.validity == "V2_schema_synthetic"
            && case.note.contains("not asserted observed in wild")
            && case.update.is_object()
    }));
    assert!(updates.iter().all(|case| !case.id.is_empty()));
    assert!(urls.iter().all(|case| {
        !case.id.is_empty() && !case.class.is_empty() && !case.oracle_hint.is_empty()
    }));

    let indexed = urls
        .iter()
        .map(|case| (case.id.as_str(), case.input.as_str()))
        .collect::<BTreeMap<_, _>>();
    let parse = |id: &str| {
        teloxide_url::parse_url(indexed[id])
            .unwrap_or_else(|| panic!("synthetic URL case {id} was rejected"))
    };
    assert_eq!(parse("url-01").as_str(), parse("url-02").as_str());
    assert_ne!(parse("url-01").as_str(), parse("url-03").as_str());
    assert_ne!(parse("url-01").as_str(), parse("url-04").as_str());
    assert_eq!(
        urls.iter().find(|case| case.id == "url-19").unwrap().class,
        "mention"
    );

    let summary = CaseReport {
        case_id: "public-url-corpus".to_owned(),
        invariant_id: "URL-02".to_owned(),
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PublicSyntheticCorpus,
        seed: None,
        generated: urls.len() as u32,
        accepted: (urls.len() - 1) as u32,
        rejected: 1,
    };
    assert!(summary.is_well_formed());
}

#[test]
fn url_property_generator_replays_the_fixed_seed() {
    let seed = replay_seed();
    let strategy = ("[a-z]{1,12}", "[A-Za-z0-9_-]{1,18}", "[A-Za-z0-9]{1,18}");
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    runner
        .run(&strategy, |(host, path, query)| {
            let raw = format!("https://{host}.example.org/{path}?ref={query}");
            let parsed = teloxide_url::parse_url(&raw)
                .ok_or_else(|| TestCaseError::fail("generated valid URL was rejected"))?;
            let independent = url::Url::parse(&raw)
                .map_err(|_| TestCaseError::fail("reference URL parser rejected generated URL"))?;
            let expected_host = format!("{host}.example.org");
            let expected_path = format!("/{path}");
            let expected_query = format!("ref={query}");
            prop_assert_eq!(parsed.host_str(), Some(expected_host.as_str()));
            prop_assert_eq!(parsed.path(), expected_path.as_str());
            prop_assert_eq!(parsed.query(), Some(expected_query.as_str()));
            prop_assert_eq!(parsed.as_str(), independent.as_str());
            prop_assert_eq!(teloxide_url::parse_url(parsed.as_str()).unwrap(), parsed);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("seeded URL property failed: {error}"));

    let report = url_case_report("seeded-url-property", seed, PROPERTY_CASES, 0);
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);
}

fn run_deliberately_broken_fixture(seed: u64) -> (u32, String) {
    let mut runner = fixed_runner(seed, 128);
    match runner.run(&(0_u32..=1024), |value| {
        if value >= 512 {
            Err(TestCaseError::fail("deliberately broken harness fixture"))
        } else {
            Ok(())
        }
    }) {
        Err(TestError::Fail(reason, minimal)) => (minimal, reason.to_string()),
        Err(TestError::Abort(reason)) => panic!("fixture runner aborted: {reason}"),
        Ok(()) => panic!("deliberately broken fixture unexpectedly passed"),
    }
}

#[test]
fn deliberate_failure_shrinks_and_replays_identically() {
    let seed = replay_seed();
    let first = run_deliberately_broken_fixture(seed);
    let second = run_deliberately_broken_fixture(seed);
    assert_eq!(first, second);
    assert_eq!(first.0, 512);

    let report = CaseReport {
        case_id: "deliberate-shrink-fixture".to_owned(),
        invariant_id: "SYS-01".to_owned(),
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::MinimizedReplay,
        seed: Some(seed),
        generated: 1,
        accepted: 0,
        rejected: 1,
    };
    assert!(report.is_well_formed());
    assert_eq!(first.1, "deliberately broken harness fixture");
}

#[derive(serde::Deserialize)]
struct MinimalUrlTrace {
    trace_id: String,
    seed: u64,
    validity_class: String,
    origin: String,
    invariant_id: String,
    operation: String,
    first: String,
    second: String,
    expected_identity: String,
}

#[test]
fn minimal_operation_trace_replays_with_independent_identity() {
    let trace: MinimalUrlTrace =
        serde_json::from_str(include_str!("../testing/replays/minimal_url_trace.json")).unwrap();
    assert_eq!(trace.trace_id, "synthetic-url-host-case-001");
    assert_eq!(trace.validity_class, "V2");
    assert_eq!(trace.origin, "public_synthetic_corpus");
    assert_eq!(trace.invariant_id, "URL-01");
    assert_eq!(trace.operation, "parse_url_pair");

    let first = teloxide_url::parse_url(&trace.first).unwrap();
    let second = teloxide_url::parse_url(&trace.second).unwrap();
    let expected = url::Url::parse(&trace.first).unwrap().to_string();
    assert_eq!(first.as_str(), trace.expected_identity);
    assert_eq!(second.as_str(), trace.expected_identity);
    assert_eq!(first.as_str(), expected);
    assert_eq!(first, second);
    assert!(trace.seed > 0);
}
