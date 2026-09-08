use pip_store::{ApplyResult, ConversationInput, Store};
use serde_json::json;

fn message() -> ConversationInput {
    ConversationInput {
        key: "repo-123-comment-456-version-1".into(),
        repository_id: 123,
        thread_number: 7,
        actor_id: 99,
        case_key: None,
        received_at: 100,
        payload: json!({"body":"Please explain this", "comment_id":456}),
    }
}

#[test]
fn discussion_is_durable_without_creating_a_workflow_or_replaying_effects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    let input = message();
    assert_eq!(
        store.record_conversation(&input).unwrap(),
        ApplyResult::Applied
    );
    assert_eq!(
        store.record_conversation(&input).unwrap(),
        ApplyResult::Replayed
    );
    let mut changed = input.clone();
    changed.payload = json!({"body":"changed"});
    assert!(store.record_conversation(&changed).is_err());
    assert!(store.status(101).unwrap().cases.is_empty());
    assert_eq!(store.outbox_count().unwrap(), 0);
    let spec = json!({"model":"configured-model","body":"frozen"});
    assert!(store.reserve_conversation(&input.key, &spec).unwrap());
    assert!(!store.reserve_conversation(&input.key, &spec).unwrap());
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.conversations(123, 10).unwrap()[0].spec, Some(spec));
    assert!(store.conversations(124, 10).unwrap().is_empty());
    store.bind_conversation_task(&input.key, "task-1").unwrap();
    assert!(store.bind_conversation_task(&input.key, "task-2").is_err());
    let answer = json!({"reply":"An explanation", "follow_up":"NONE"});
    store.answer_conversation(&input.key, &answer).unwrap();
    store
        .prepare_conversation_reply(&input.key, "Human-readable frozen reply")
        .unwrap();
    assert!(
        store
            .prepare_conversation_reply(&input.key, "Changed reply")
            .is_err()
    );
    assert_eq!(
        store
            .conversation(&input.key)
            .unwrap()
            .unwrap()
            .reply_body
            .as_deref(),
        Some("Human-readable frozen reply")
    );
    store.answer_conversation(&input.key, &answer).unwrap();
    assert!(
        store
            .answer_conversation(&input.key, &json!({"reply":"different"}))
            .is_err()
    );
    store
        .finish_conversation(&input.key, "PUBLISHED", Some(987))
        .unwrap();
    assert!(store.conversations(123, 10).unwrap().is_empty());
    assert_eq!(store.status(101).unwrap().runs, 0);
}

#[test]
fn ignored_conversation_cannot_be_dispatched_or_have_its_input_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    let input = message();
    store.record_conversation(&input).unwrap();
    store
        .finish_conversation(&input.key, "IGNORED", None)
        .unwrap();
    assert!(!store.reserve_conversation(&input.key, &json!({})).unwrap());
    let sql = rusqlite::Connection::open(&path).unwrap();
    assert!(
        sql.execute("UPDATE conversations SET payload_json='{}'", [])
            .is_err()
    );
    assert!(sql.execute("DELETE FROM conversations", []).is_err());
}

#[test]
fn schema_twelve_upgrades_without_rewriting_existing_case_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_case(&pip_store::NewCase {
            case_key: "repo:123#7@3".into(),
            repository_id: 123,
            issue_number: 7,
            workflow_version: 3,
            policy_revision: 7,
            initial_state: "WAITING_CI".into(),
            observed_at: 100,
            event: pip_store::EventInput {
                event_id: "retained-event".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"retain":"exact bytes"}),
            },
            effects: vec![],
        })
        .unwrap();
    let before =
        serde_json::to_value(store.immutable_history_for_case("repo:123#7@3").unwrap()).unwrap();
    drop(store);
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch("DROP TABLE conversations; DELETE FROM schema_migrations WHERE version=13; PRAGMA user_version=12;").unwrap();
    drop(sql);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 13);
    assert_eq!(
        serde_json::to_value(store.immutable_history_for_case("repo:123#7@3").unwrap()).unwrap(),
        before
    );
    assert!(store.conversations(123, 10).unwrap().is_empty());
}
