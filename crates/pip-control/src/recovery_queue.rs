//! With all execution units stopped, another case's frozen queue can stay put.
use pip_store::Store;
use std::{fs, path::Path};

pub(crate) fn validate(queue: &Path, store: &Store, case: Option<&str>) -> Result<(), String> {
    for directory in ["inbox", "results"] {
        for entry in fs::read_dir(queue.join(directory)).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let invalid = || {
                "direct queue must be drained for the recovery case; unknown entries are not allowed".to_string()
            };
            let case = case.ok_or_else(invalid)?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || entry.metadata().map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024
            {
                return Err(invalid());
            }
            let id = entry
                .file_name()
                .to_str()
                .and_then(|name| {
                    name.strip_prefix("attempt-")?
                        .strip_suffix(".json")?
                        .parse::<u64>()
                        .ok()
                })
                .ok_or_else(invalid)?;
            let attempt = store
                .direct_attempt(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(invalid)?;
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path()).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if attempt.case_key == case
                || value["schema_version"] != 1
                || value["attempt_id"].as_u64() != Some(id)
                || (directory == "inbox"
                    && (value["claimed"]["case_key"] != attempt.case_key
                        || value["claimed"]["effect_id"] != attempt.effect_id))
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pip_store::{EffectInput, EventInput, NewCase};
    use serde_json::json;

    #[test]
    fn stopped_peer_queue_is_preserved_but_own_unknown_and_symlink_entries_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("db")).unwrap();
        for folder in ["inbox", "results"] {
            fs::create_dir(dir.path().join(folder)).unwrap();
        }
        store
            .create_case(&NewCase {
                case_key: "repo:42#10@3".into(),
                repository_id: 42,
                issue_number: 10,
                workflow_version: 3,
                policy_revision: 1,
                initial_state: "READY_TO_BUILD".into(),
                observed_at: 1,
                event: EventInput {
                    event_id: "peer".into(),
                    event_type: "ISSUE_AUTHORIZED".into(),
                    payload: json!({}),
                },
                effects: vec![EffectInput {
                    effect_id: "job".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: json!({}),
                }],
            })
            .unwrap();
        let effect = store.claim_effect("worker", 2, 100).unwrap().unwrap();
        let id = store.begin_direct_attempt(&effect, "task", 2).unwrap();
        let file = dir.path().join(format!("inbox/attempt-{id}.json"));
        let bytes=serde_json::to_vec(&json!({"schema_version":1,"attempt_id":id,"claimed":{"case_key":"repo:42#10@3","effect_id":"job"}})).unwrap();
        fs::write(&file, &bytes).unwrap();
        assert!(validate(dir.path(), &store, Some("repo:42#9@3")).is_ok());
        assert!(validate(dir.path(), &store, Some("repo:42#10@3")).is_err());
        assert!(validate(dir.path(), &store, None).is_err());
        assert_eq!(fs::read(&file).unwrap(), bytes);
        let unknown = dir.path().join("results/attempt-9999.json");
        fs::write(&unknown, b"unknown").unwrap();
        assert!(validate(dir.path(), &store, Some("repo:42#9@3")).is_err());
        fs::remove_file(&unknown).unwrap();
        std::os::unix::fs::symlink(&file, dir.path().join(format!("results/attempt-{id}.json")))
            .unwrap();
        assert!(validate(dir.path(), &store, Some("repo:42#9@3")).is_err());
    }
}
