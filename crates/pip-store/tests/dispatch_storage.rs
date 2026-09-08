use pip_store::{
    ApplyResult, DispatchIntent, DispatchTransport, EffectInput, EventInput, NewCase, Store,
    TaskProjectionInput,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn seed(store: &mut Store) {
    store
        .create_case(&NewCase {
            case_key: "repo:1#2@1".into(),
            repository_id: 1,
            issue_number: 2,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "REVIEWING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({}),
            },
            effects: vec![EffectInput {
                effect_id: "reviews".into(),
                effect_type: "DISPATCH_REVIEWERS".into(),
                payload: json!({}),
            }],
        })
        .unwrap();
}

fn intents() -> Vec<DispatchIntent> {
    [DispatchTransport::Hermes, DispatchTransport::Direct, DispatchTransport::Direct]
        .into_iter().enumerate().map(|(i, transport)| DispatchIntent {
            intent_id: format!("review-{i}"), transport,
            desired: json!({"source_effect_id":"reviews", "task_id":format!("review-{i}"),
                "model":format!("exact-model-{i}"), "body":{
                    "immutable_evidence_bundle":{"schema_version":2,"records": "retained-input-".repeat(2048)},
                    "evidence_focus":{"role":i}}}),
        }).collect()
}

fn hash(value: &Value) -> String {
    Sha256::digest(serde_json::to_vec(value).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn outputs(intents: &[DispatchIntent]) -> (TaskProjectionInput, Vec<EffectInput>) {
    (
        TaskProjectionInput {
            projection_id: intents[0].intent_id.clone(),
            effect_id: "reviews".into(),
            board: "repo-board".into(),
            task_id: "hermes-task".into(),
            desired: intents[0].desired.clone(),
            observed: json!({"id":"hermes-task"}),
        },
        intents[1..]
            .iter()
            .map(|intent| EffectInput {
                effect_id: format!("reviews:direct:{}", intent.intent_id),
                effect_type: "RUN_DIRECT_WORKER".into(),
                payload: intent.desired.clone(),
            })
            .collect(),
    )
}

#[test]
fn frozen_fanout_stores_one_bundle_and_outputs_reference_the_saved_definitions() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    seed(&mut store);
    let claim = store.claim_effect("controller", 2, 100).unwrap().unwrap();
    let intents = intents();
    store.freeze_dispatch_intents(&claim, &intents, 3).unwrap();
    let (projection, jobs) = outputs(&intents);
    assert_eq!(
        store
            .complete_dispatch_outputs(
                "reviews",
                std::slice::from_ref(&projection),
                &jobs,
                "controller",
                4,
                None
            )
            .unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store
            .complete_dispatch_outputs(
                "reviews",
                std::slice::from_ref(&projection),
                &jobs,
                "controller",
                5,
                None
            )
            .unwrap(),
        ApplyResult::Replayed
    );
    drop(store);
    let connection = Connection::open(&path).unwrap();
    let batch: String = connection
        .query_row("SELECT payload_json FROM dispatch_batches", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(batch.matches("retained-input-").count(), 2048);
    let expanded_bytes = serde_json::to_vec(&intents).unwrap().len()
        + intents
            .iter()
            .map(|intent| serde_json::to_vec(&intent.desired).unwrap().len())
            .sum::<usize>();
    let mut retained_bytes = batch.len();
    for query in [
        "SELECT desired_json FROM task_projections",
        "SELECT payload_json FROM outbox WHERE effect_type='RUN_DIRECT_WORKER'",
    ] {
        let mut statement = connection.prepare(query).unwrap();
        for raw in statement.query_map([], |r| r.get::<_, String>(0)).unwrap() {
            let raw = raw.unwrap();
            retained_bytes += raw.len();
            assert!(!raw.contains("retained-input-"));
            assert!(raw.len() < 512);
        }
    }
    assert!(retained_bytes * 4 < expanded_bytes);
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.dispatch_intents("reviews").unwrap().unwrap(), intents);
    assert_eq!(
        store.task_projection("review-0").unwrap().unwrap(),
        projection
    );
    assert_eq!(
        store.unconsumed_task_projections().unwrap(),
        vec![projection]
    );
    for job in jobs {
        let claimed = store.claim_effect("worker", 6, 100).unwrap().unwrap();
        assert_eq!(claimed.effect_id, job.effect_id);
        assert_eq!(claimed.payload, job.payload);
    }
}

#[test]
fn old_inline_batches_replay_without_rewriting_their_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    seed(&mut store);
    let claim = store.claim_effect("controller", 2, 100).unwrap().unwrap();
    let intents = intents();
    store.freeze_dispatch_intents(&claim, &intents, 3).unwrap();
    drop(store);
    let legacy = serde_json::to_value(&intents).unwrap();
    let bytes = serde_json::to_string(&legacy).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER dispatch_batches_no_update;
            DELETE FROM schema_migrations WHERE version > 9; PRAGMA user_version=9;",
        )
        .unwrap();
    connection
        .execute(
            "UPDATE dispatch_batches SET payload_json=?1, payload_sha256=?2",
            params![bytes, hash(&legacy)],
        )
        .unwrap();
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 11);
    assert_eq!(store.dispatch_intents("reviews").unwrap().unwrap(), intents);
    assert_eq!(
        store.freeze_dispatch_intents(&claim, &intents, 4).unwrap(),
        ApplyResult::Replayed
    );
    let mut changed = intents.clone();
    changed[1].desired["model"] = json!("substituted");
    assert!(store.freeze_dispatch_intents(&claim, &changed, 5).is_err());
    drop(store);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT payload_json FROM dispatch_batches", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        bytes
    );
}

#[test]
fn output_cannot_replace_a_frozen_job_with_different_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    seed(&mut store);
    let claim = store.claim_effect("controller", 2, 100).unwrap().unwrap();
    let intents = intents();
    store.freeze_dispatch_intents(&claim, &intents, 3).unwrap();
    let before = store.status(4).unwrap();
    let mut job = EffectInput {
        effect_id: "reviews:direct:review-1".into(),
        effect_type: "RUN_DIRECT_WORKER".into(),
        payload: intents[1].desired.clone(),
    };
    job.payload["model"] = json!("substituted");
    assert!(
        store
            .complete_dispatch_outputs("reviews", &[], &[job], "controller", 4, None)
            .is_err()
    );
    assert_eq!(store.status(4).unwrap(), before);
}

#[test]
fn historical_inline_outputs_replay_without_rewriting_or_rebinding() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    seed(&mut store);
    let claim = store.claim_effect("controller", 2, 100).unwrap().unwrap();
    let intents = intents();
    store.freeze_dispatch_intents(&claim, &intents, 3).unwrap();
    let (projection, jobs) = outputs(&intents);
    store
        .complete_dispatch_outputs(
            "reviews",
            std::slice::from_ref(&projection),
            &jobs,
            "controller",
            4,
            None,
        )
        .unwrap();
    drop(store);
    let connection = Connection::open(&path).unwrap();
    let desired = serde_json::to_string(&projection.desired).unwrap();
    connection
        .execute("UPDATE task_projections SET desired_json=?1", [&desired])
        .unwrap();
    for job in &jobs {
        connection
            .execute(
                "UPDATE outbox SET payload_json=?1, payload_sha256=?2 WHERE effect_id=?3",
                params![
                    serde_json::to_string(&job.payload).unwrap(),
                    hash(&job.payload),
                    job.effect_id
                ],
            )
            .unwrap();
    }
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .complete_dispatch_outputs(
                "reviews",
                std::slice::from_ref(&projection),
                &jobs,
                "controller",
                5,
                None
            )
            .unwrap(),
        ApplyResult::Replayed
    );
    assert_eq!(
        store.task_projection("review-0").unwrap().unwrap(),
        projection
    );
    drop(store);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT desired_json FROM task_projections", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        desired
    );
    for job in &jobs {
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_json FROM outbox WHERE effect_id=?1",
                    [&job.effect_id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            serde_json::to_string(&job.payload).unwrap()
        );
    }
}

#[test]
fn invalid_evidence_or_references_fail_before_a_lease_is_committed() {
    for corruption in [
        "bundle",
        "missing_bundle",
        "format",
        "digest",
        "intent",
        "transport",
        "foreign_case",
        "outer_digest",
        "outbox_digest",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ledger.db");
        let mut store = Store::open(&path).unwrap();
        seed(&mut store);
        let claim = store.claim_effect("controller", 2, 100).unwrap().unwrap();
        let intents = intents();
        store.freeze_dispatch_intents(&claim, &intents, 3).unwrap();
        let (projection, jobs) = outputs(&intents);
        store
            .complete_dispatch_outputs(
                "reviews",
                std::slice::from_ref(&projection),
                &jobs,
                "controller",
                4,
                None,
            )
            .unwrap();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("DROP TRIGGER dispatch_batches_no_update")
            .unwrap();
        if ["bundle", "missing_bundle", "format", "outer_digest"].contains(&corruption) {
            let raw: String = connection
                .query_row("SELECT payload_json FROM dispatch_batches", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let mut value: Value = serde_json::from_str(&raw).unwrap();
            match corruption {
                "bundle" => {
                    *value["evidence"]
                        .as_object_mut()
                        .unwrap()
                        .values_mut()
                        .next()
                        .unwrap() = json!({"changed":true})
                }
                "missing_bundle" => value["evidence"] = json!({}),
                "format" => value["format"] = json!(999),
                _ => {}
            }
            let digest = if corruption == "outer_digest" {
                "0".repeat(64)
            } else {
                hash(&value)
            };
            connection
                .execute(
                    "UPDATE dispatch_batches SET payload_json=?1, payload_sha256=?2",
                    params![serde_json::to_string(&value).unwrap(), digest],
                )
                .unwrap();
        } else {
            let raw: String = connection
                .query_row(
                    "SELECT payload_json FROM outbox WHERE effect_id=?1",
                    [&jobs[0].effect_id],
                    |r| r.get(0),
                )
                .unwrap();
            let mut value: Value = serde_json::from_str(&raw).unwrap();
            match corruption {
                "digest" => value["dispatch_intent_ref"]["sha256"] = json!("0".repeat(64)),
                "intent" => value["dispatch_intent_ref"]["intent_id"] = json!("missing"),
                "transport" => {
                    value["dispatch_intent_ref"]["intent_id"] = json!("review-0");
                    value["dispatch_intent_ref"]["sha256"] = json!(hash(&intents[0].desired));
                }
                "foreign_case" => {
                    connection
                        .execute(
                            "INSERT INTO cases(case_key,repository_id,issue_number,
                        workflow_version,state,state_revision,policy_revision,created_at,updated_at)
                        VALUES('repo:2#2@1',2,2,1,'REVIEWING',1,1,1,1)",
                            [],
                        )
                        .unwrap();
                    connection
                        .execute(
                            "UPDATE outbox SET case_key='repo:2#2@1' WHERE effect_id=?1",
                            [&jobs[0].effect_id],
                        )
                        .unwrap();
                }
                "outbox_digest" => {}
                _ => unreachable!(),
            }
            connection
                .execute(
                    "UPDATE outbox SET payload_json=?1, payload_sha256=?2 WHERE effect_id=?3",
                    params![
                        serde_json::to_string(&value).unwrap(),
                        if corruption == "outbox_digest" {
                            "0".repeat(64)
                        } else {
                            hash(&value)
                        },
                        jobs[0].effect_id
                    ],
                )
                .unwrap();
        }
        drop(connection);
        let mut store = Store::open(&path).unwrap();
        assert!(
            store.claim_effect("worker", 6, 100).is_err(),
            "{corruption}"
        );
        assert_eq!(store.status(6).unwrap().outbox_leased, 0, "{corruption}");
    }
}
