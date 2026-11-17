#[path = "support/adversarial_case_report.rs"]
mod adversarial_case_report;
#[path = "support/quality/mod.rs"]
mod quality;

use std::cell::Cell;
use std::{collections::BTreeMap, env};

use adversarial_case_report::{
    CaseOrigin, CaseReport, SyntheticUpdateCase, SyntheticUrlCase, ValidityClass,
};
use proptest::{
    prelude::*,
    test_runner::{TestCaseError, TestError},
};
use quality::{PROPERTY_CASES, fixed_runner, replay_seed};

fn url_case_report(case_id: &'static str, seed: u64, accepted: u32, rejected: u32) -> CaseReport {
    CaseReport {
        case_id,
        invariant_id: "URL-01",
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PropertyGenerated,
        evidence_source: None,
        seed: Some(seed),
        generated: accepted.saturating_add(rejected),
        accepted,
        rejected,
    }
}

fn property_case_report(
    case_id: &'static str,
    invariant_id: &'static str,
    seed: u64,
    accepted: u32,
    rejected: u32,
) -> CaseReport {
    CaseReport {
        case_id,
        invariant_id,
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PropertyGenerated,
        evidence_source: None,
        seed: Some(seed),
        generated: accepted.saturating_add(rejected),
        accepted,
        rejected,
    }
}

fn failure_summary(case_id: &str, seed: u64, error: impl std::fmt::Display) -> String {
    let sha = env::var("GITHUB_SHA")
        .or_else(|_| env::var("NEDOBOT_TEST_SHA"))
        .unwrap_or_else(|_| "local-unpinned".to_owned());
    format!(
        "case_id={case_id} seed={seed} sha={sha} replay=sh scripts/replay_quality_case.sh {case_id} {seed}: {error}"
    )
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
        case_id: "public-url-corpus",
        invariant_id: "URL-02",
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PublicSyntheticCorpus,
        evidence_source: None,
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
        .unwrap_or_else(|error| panic!("{}", failure_summary("seeded-url-property", seed, error)));

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
    assert_eq!(
        first,
        second,
        "{}",
        failure_summary(
            "deliberate-shrink-fixture",
            seed,
            "same seed produced a different minimized counterexample"
        )
    );
    assert_eq!(
        first.0,
        512,
        "{}",
        failure_summary(
            "deliberate-shrink-fixture",
            seed,
            "synthetic mutant did not shrink to its boundary"
        )
    );

    let report = CaseReport {
        case_id: "deliberate-shrink-fixture",
        invariant_id: "SYS-01",
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::MinimizedReplay,
        evidence_source: None,
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

#[test]
fn seeded_scope_and_utf16_generators_respect_their_contracts() {
    use quality::generators::{scoped_ids, utf16_entity_case};

    let seed = replay_seed();
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let accepted = Cell::new(0_u32);
    runner
        .run(&(scoped_ids(), utf16_entity_case()), |(ids, entity)| {
            prop_assert!(!ids.instance.is_empty());
            prop_assert!(ids.chat_id != 0);
            prop_assert!(ids.user_id > 0);
            prop_assert!(ids.message_id > 0);
            prop_assert!(!entity.text.is_empty());
            prop_assert!(entity.length > 0);

            let boundaries = utf16_boundaries(&entity.text);
            prop_assert!(boundaries.contains(&(entity.offset as usize)));
            prop_assert!(boundaries.contains(&((entity.offset + entity.length) as usize)));
            prop_assert!(
                entity.offset + entity.length <= entity.text.encode_utf16().count() as u32
            );
            accepted.set(accepted.get() + 1);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("{}", failure_summary("utf16-generator", seed, error)));
    let report = property_case_report("typed-generators", "TEXT-UTF16-01", seed, accepted.get(), 0);
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);
}

#[test]
fn schema_valid_telegram_update_strategy_deserializes_as_teloxide_update() {
    use quality::generators::schema_valid_update;

    let seed = replay_seed();
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let accepted = Cell::new(0_u32);
    runner
        .run(&schema_valid_update(), |case| {
            let value = case.to_json();
            let update: teloxide::types::Update = serde_json::from_value(value)
                .map_err(|error| TestCaseError::fail(format!("V2 update rejected: {error}")))?;
            prop_assert_eq!(update.id.0, case.update_id);
            match update.kind {
                teloxide::types::UpdateKind::Message(message) => {
                    prop_assert_eq!(message.id.0, case.ids.message_id);
                    prop_assert_eq!(message.text().unwrap_or_default(), case.text.text);
                    let entity = message
                        .entities()
                        .and_then(|entities| entities.first())
                        .expect("one generated entity");
                    prop_assert_eq!(entity.offset as u32, case.text.offset);
                    prop_assert_eq!(entity.length as u32, case.text.length);
                }
                kind => prop_assert!(false, "expected a message update, got {kind:?}"),
            }
            accepted.set(accepted.get() + 1);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("{}", failure_summary("telegram-update", seed, error)));
    let report = property_case_report(
        "typed-generators",
        "UPDATE-SCHEMA-01",
        seed,
        accepted.get(),
        0,
    );
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);

    let malformed_v3 = serde_json::json!({"update_id": -1, "message": {}});
    assert!(serde_json::from_value::<teloxide::types::Update>(malformed_v3).is_err());
    let malformed_report = CaseReport {
        case_id: "invalid-update-v3",
        invariant_id: "UPDATE-SCHEMA-01",
        validity_class: ValidityClass::V3,
        origin: CaseOrigin::MinimizedReplay,
        evidence_source: None,
        seed: Some(seed),
        generated: 1,
        accepted: 0,
        rejected: 1,
    };
    assert!(malformed_report.is_well_formed());
}

#[test]
fn seeded_update_stream_replays_identically() {
    use std::cell::RefCell;

    use quality::generators::schema_valid_update;

    fn capture(seed: u64) -> Vec<serde_json::Value> {
        let mut runner = fixed_runner(seed, 64);
        let values = RefCell::new(Vec::new());
        runner
            .run(&schema_valid_update(), |case| {
                values.borrow_mut().push(case.to_json());
                Ok(())
            })
            .unwrap_or_else(|error| {
                panic!("{}", failure_summary("seeded-update-replay", seed, error))
            });
        values.into_inner()
    }

    let seed = replay_seed();
    let first = capture(seed);
    let second = capture(seed);
    assert_eq!(
        first,
        second,
        "{}",
        failure_summary(
            "seeded-update-replay",
            seed,
            "fixed seed produced different typed Update traces"
        )
    );
    assert_eq!(first.len(), 64);
}

#[test]
fn reference_models_reject_stale_job_completion_and_unauthorized_actions() {
    use quality::{
        generators::{ActorRole, Capability, JobOperation},
        model::{ReferenceJob, reference_allows},
    };

    let mut job = ReferenceJob::pending();
    assert!(job.apply(JobOperation::Claim));
    let claimed = job;
    assert!(!job.apply(JobOperation::Complete { generation: 0 }));
    assert_eq!(job, claimed);
    assert!(job.apply(JobOperation::Complete { generation: 1 }));
    assert!(!job.apply(JobOperation::Claim));

    for (actor, capability, expected) in [
        (ActorRole::Owner, Capability::Write, true),
        (ActorRole::Admin, Capability::Read, true),
        (ActorRole::Admin, Capability::Write, false),
        (ActorRole::Member, Capability::Read, true),
        (ActorRole::Member, Capability::Moderate, false),
        (ActorRole::Anonymous, Capability::Read, false),
    ] {
        assert_eq!(reference_allows(actor, capability), expected);
    }
}

#[test]
fn typed_authorization_and_ordered_job_strategies_round_trip() {
    use quality::generators::{authorization_case, job_phase, ordered_job_operations};

    let seed = replay_seed();
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let accepted = Cell::new(0_u32);
    runner
        .run(
            &(authorization_case(), job_phase(), ordered_job_operations()),
            |(authorization, phase, operations)| {
                prop_assert!(operations.len() <= 32);

                let encoded = serde_json::to_vec(&(authorization, phase, &operations))
                    .expect("typed synthetic case serializes");
                let decoded: (
                    quality::generators::AuthorizationCase,
                    quality::generators::JobPhase,
                    Vec<quality::generators::JobOperation>,
                ) = serde_json::from_slice(&encoded).expect("typed synthetic case deserializes");
                prop_assert_eq!(decoded.0, authorization);
                prop_assert_eq!(decoded.1, phase);
                prop_assert_eq!(decoded.2, operations);
                accepted.set(accepted.get() + 1);
                Ok(())
            },
        )
        .unwrap_or_else(|error| {
            panic!("{}", failure_summary("typed-state-generators", seed, error))
        });
    let report = property_case_report(
        "typed-state-generators",
        "HARNESS-DATA-01",
        seed,
        accepted.get(),
        0,
    );
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);
}

#[test]
fn durable_job_reference_model_preserves_fences_and_uncertain_delivery() {
    use quality::{
        durable_jobs::{DurableJobModel, DurableJobStatus},
        generators::{DurableJobOperation as Op, durable_job_trace},
    };

    let stale_worker_trace = [
        Op::Claim { worker: 0 },
        Op::AdvanceClock { seconds: 31 },
        Op::Claim { worker: 1 },
        Op::FinalizeOk { worker: 0 },
    ];
    let mut job = DurableJobModel::default();
    for op in stale_worker_trace {
        job.apply(op);
        assert!(job.invariant_holds(), "state after {op:?}: {job:?}");
    }
    assert_eq!(job.status, DurableJobStatus::Processing);
    assert_eq!(job.owner, Some(1));
    assert_eq!(job.fence, 2);
    assert_eq!(job.attempts, 2);
    assert_eq!(job.confirmed_effects, 0);

    assert!(job.apply(Op::BeginExternalEffect { worker: 1 }));
    assert!(job.apply(Op::MarkDeliveryUnknown { worker: 1 }));
    let unknown = job.clone();
    for op in [Op::Restart, Op::Claim { worker: 2 }, Op::DuplicateIngress] {
        assert!(!job.apply(op), "terminal unknown job accepted {op:?}");
        assert_eq!(job, unknown, "terminal unknown job changed after {op:?}");
    }
    assert!(job.apply(Op::AdvanceClock { seconds: 40 }));
    assert!(!job.apply(Op::Claim { worker: 2 }));
    assert_eq!(job.status, DurableJobStatus::DeliveryUnknown);
    assert_eq!(job.confirmed_effects, 0);

    let seed = replay_seed();
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let traces = Cell::new(0_u32);
    runner
        .run(&durable_job_trace(), |trace| {
            let mut model = DurableJobModel::default();
            let mut previous_fence = 0;
            let mut previous_attempts = 0;
            for op in &trace {
                model.apply(*op);
                prop_assert!(
                    model.invariant_holds(),
                    "trace={trace:?} operation={op:?} state={model:?}"
                );
                prop_assert!(model.fence >= previous_fence);
                prop_assert!(model.attempts >= previous_attempts);
                previous_fence = model.fence;
                previous_attempts = model.attempts;
            }
            traces.set(traces.get() + 1);
            Ok(())
        })
        .unwrap_or_else(|error| {
            panic!(
                "{}",
                failure_summary("durable-job-state-trace", seed, error)
            )
        });
    assert_eq!(traces.get(), PROPERTY_CASES);
}

#[test]
fn ingress_reference_model_keeps_only_current_revision_and_media_evidence() {
    use quality::{
        generators::{
            EvidenceRef, EvidenceSource, IngressMessageKey, IngressOperation, ingress_message_key,
            ingress_trace,
        },
        model::{ReferenceIngress, ReferenceIngressStore},
    };

    let mut state = ReferenceIngress::default();
    let receive = IngressOperation::Receive {
        revision: 1,
        media_generation: 10,
        evidence: vec![
            EvidenceRef {
                target: 1,
                source: EvidenceSource::Text,
            },
            EvidenceRef {
                target: 1,
                source: EvidenceSource::Entity,
            },
        ],
    };
    state.apply(&receive);
    assert_eq!(state.unique_target_count(), 1);
    assert_eq!(state.message_evidence.len(), 2);

    state.apply(&IngressOperation::AttachQr {
        media_generation: 10,
        targets: vec![2],
    });
    assert_eq!(state.unique_target_count(), 2);
    let with_current_qr = state.clone();
    state.apply(&IngressOperation::RetryOldJob {
        media_generation: 9,
        targets: vec![3],
    });
    assert_eq!(state, with_current_qr, "stale QR retry must be a no-op");

    state.apply(&IngressOperation::RefreshShadow { revision: 1 });
    assert_eq!(state.shadow_revision, Some(1));
    assert_eq!(state.shadow_evidence, with_current_qr.current_evidence());

    state.apply(&IngressOperation::Edit {
        revision: 2,
        media_generation: 11,
        evidence: vec![EvidenceRef {
            target: 4,
            source: EvidenceSource::Caption,
        }],
    });
    assert_eq!(state.unique_target_count(), 1);
    assert_eq!(state.current_evidence().iter().next().unwrap().target, 4);
    let after_edit = state.clone();
    state.apply(&IngressOperation::Edit {
        revision: 1,
        media_generation: 10,
        evidence: vec![EvidenceRef {
            target: 5,
            source: EvidenceSource::Text,
        }],
    });
    assert_eq!(
        state, after_edit,
        "late old edit must not restore stale evidence"
    );
    state.apply(&IngressOperation::RemoveLink { revision: 3 });
    assert!(state.current_evidence().is_empty());
    state.apply(&IngressOperation::RefreshShadow { revision: 3 });
    assert!(state.shadow_evidence.is_empty());

    // Identical Telegram identifiers in separate instances are different keys.
    let key_a = IngressMessageKey {
        instance: "community_a".into(),
        chat_id: -1001,
        user_id: 42,
        message_id: 7,
    };
    let key_b = IngressMessageKey {
        instance: "community_b".into(),
        ..key_a.clone()
    };
    let mut isolated = ReferenceIngressStore::default();
    isolated.apply(key_a.clone(), &receive);
    assert!(isolated.message(&key_b).is_none());
    isolated.apply(
        key_b.clone(),
        &IngressOperation::Receive {
            revision: 1,
            media_generation: 10,
            evidence: vec![EvidenceRef {
                target: 8,
                source: EvidenceSource::Text,
            }],
        },
    );
    assert_eq!(isolated.message(&key_a).unwrap().unique_target_count(), 1);
    assert_eq!(
        isolated
            .message(&key_b)
            .unwrap()
            .current_evidence()
            .iter()
            .next()
            .unwrap()
            .target,
        8
    );

    let seed = replay_seed();
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let accepted = Cell::new(0_u32);
    runner
        .run(&(ingress_message_key(), ingress_trace()), |(key, trace)| {
            let mut store = ReferenceIngressStore::default();
            for operation in &trace {
                let before = store.message(&key).cloned().unwrap_or_default();
                let should_be_ignored = match operation {
                    IngressOperation::Duplicate { .. } => true,
                    IngressOperation::Edit { revision, .. }
                    | IngressOperation::RemoveLink { revision }
                    | IngressOperation::ReplaceMedia { revision, .. } => {
                        before.revision.is_some_and(|current| *revision <= current)
                    }
                    IngressOperation::AttachQr {
                        media_generation, ..
                    }
                    | IngressOperation::RetryOldJob {
                        media_generation, ..
                    } => before.media_generation != Some(*media_generation),
                    IngressOperation::RefreshShadow { revision } => {
                        before.revision != Some(*revision)
                    }
                    IngressOperation::Receive { revision, .. } => {
                        before.revision.is_some_and(|current| *revision <= current)
                    }
                    IngressOperation::DeleteStoredMessage => false,
                };
                store.apply(key.clone(), operation);
                let after = store.message(&key).cloned().unwrap_or_default();
                if should_be_ignored {
                    prop_assert_eq!(&after, &before, "stale or duplicate op: {:?}", operation);
                }
                prop_assert!(after.unique_target_count() <= after.current_evidence().len());
            }
            accepted.set(accepted.get() + 1);
            Ok(())
        })
        .unwrap_or_else(|error| {
            panic!(
                "{}",
                failure_summary("ingress-reference-trace", seed, error)
            )
        });
    let report = property_case_report(
        "ingress-reference-trace",
        "INGRESS-CURRENT-REVISION-01",
        seed,
        accepted.get(),
        0,
    );
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);
}

#[test]
fn typed_http_outcomes_preserve_uncertain_delivery_and_bounds() {
    use quality::{
        fixed_runner,
        generators::{HttpOutcome, http_outcome},
        model::delivery_outcome_is_ambiguous,
    };

    let seed = replay_seed();
    for outcome in [
        HttpOutcome::Response {
            status: 200,
            body_bytes: 32,
        },
        HttpOutcome::Timeout,
        HttpOutcome::DisconnectedBeforeResponse,
        HttpOutcome::DisconnectedAfterAcceptance,
        HttpOutcome::Cancelled,
    ] {
        let ambiguous = delivery_outcome_is_ambiguous(outcome);
        assert_eq!(
            ambiguous,
            matches!(
                outcome,
                HttpOutcome::Timeout | HttpOutcome::DisconnectedAfterAcceptance
            ),
            "{}",
            failure_summary(
                "http-outcomes",
                seed,
                "typed outcome classification changed"
            )
        );
    }
    let mut runner = fixed_runner(seed, PROPERTY_CASES);
    let accepted = Cell::new(0_u32);
    runner
        .run(&http_outcome(), |outcome| {
            match outcome {
                HttpOutcome::Response { status, body_bytes } => {
                    prop_assert!((100..=599).contains(&status));
                    prop_assert!(body_bytes <= 4096);
                    prop_assert!(!delivery_outcome_is_ambiguous(outcome));
                }
                HttpOutcome::Timeout | HttpOutcome::DisconnectedAfterAcceptance => {
                    prop_assert!(delivery_outcome_is_ambiguous(outcome));
                }
                HttpOutcome::DisconnectedBeforeResponse | HttpOutcome::Cancelled => {
                    prop_assert!(!delivery_outcome_is_ambiguous(outcome));
                }
            }
            accepted.set(accepted.get() + 1);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("{}", failure_summary("http-outcomes", seed, error)));
    let report = property_case_report("http-outcomes", "HARNESS-FAULT-01", seed, accepted.get(), 0);
    assert!(report.is_well_formed());
    assert_eq!(report.accepted, PROPERTY_CASES);
}

#[test]
fn reports_omit_payload_and_require_evidence_for_v1() {
    use quality::report::{CaseOrigin, CaseReport, FailureInput, ValidityClass};

    let payload = "synthetic-secret-payload-must-not-be-logged";
    let report = CaseReport {
        case_id: "case-safe-report",
        invariant_id: "SYS-REPORT-01",
        validity_class: ValidityClass::V2,
        origin: CaseOrigin::PropertyGenerated,
        evidence_source: None,
        seed: Some(replay_seed()),
        generated: 3,
        accepted: 2,
        rejected: 1,
    };
    let mut unseeded = report.clone();
    unseeded.seed = None;
    assert!(
        !unseeded.is_well_formed(),
        "{}",
        failure_summary(
            "safe-report",
            replay_seed(),
            "property report without a replay seed was accepted"
        )
    );
    let safe = FailureInput {
        raw_payload: payload,
        report,
    };
    let serialized = serde_json::to_string(&safe.safe_json()).unwrap();
    assert!(
        !serialized.contains(payload),
        "{}",
        failure_summary("safe-report", replay_seed(), "raw payload was serialized")
    );
    assert!(
        !serialized.contains("raw_payload"),
        "{}",
        failure_summary("safe-report", replay_seed(), "payload field was serialized")
    );
    assert_eq!(
        safe.safe_json()["validity_class"],
        "v2",
        "{}",
        failure_summary("safe-report", replay_seed(), "validity metadata was lost")
    );

    let unsupported_v1 = CaseReport {
        case_id: "case-unproven-v1",
        invariant_id: "SYS-REPORT-01",
        validity_class: ValidityClass::V1,
        origin: CaseOrigin::PropertyGenerated,
        evidence_source: None,
        seed: Some(replay_seed()),
        generated: 1,
        accepted: 1,
        rejected: 0,
    };
    assert!(!unsupported_v1.is_well_formed());
}

fn utf16_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries = vec![0];
    let mut offset = 0;
    for character in text.chars() {
        offset += character.len_utf16();
        boundaries.push(offset);
    }
    boundaries
}
