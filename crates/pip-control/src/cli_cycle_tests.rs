use super::*;
use pip_github::{GitHubError, MutationRequest, ReadRequest, ReadResponse};
use pip_store::{EffectInput, EventInput, NewCase};

#[derive(Clone)]
struct OfflineGitHub {
    healthy: Option<crate::RepositoryPolicy>,
}

impl pip_github::ReadTransport for OfflineGitHub {
    fn get(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        if let Some(policy) = &self.healthy {
            let root = format!(
                "https://api.github.com/repos/{}",
                policy.repository.full_name()
            );
            let body = if request.url == root {
                json!({"id":policy.repository.id,"full_name":policy.repository.full_name(),"default_branch":policy.repository.default_branch})
            } else if request.url == format!("{root}/issues/78") {
                json!({"id":78,"number":78,"state":"open","labels":[{"name":policy.intake.label}],
                    "user":{"id":1001},"title":"Healthy issue","body":"Fixture","created_at":"2026-09-07T00:00:00Z","updated_at":"2026-09-07T00:00:00Z"})
            } else if request.url == format!("{root}/issues/78/events?per_page=100&page=1") {
                json!([{"id":1,"event":"labeled","actor":{"id":policy.intake.trusted_actor_ids.first().unwrap()},
                    "label":{"name":policy.intake.label},"created_at":"2026-09-07T00:00:00Z"}])
            } else if request.url == format!("{root}/issues/78/comments?per_page=100&page=1") {
                json!([])
            } else {
                return Err(GitHubError::Transport("fixture issue outage".into()));
            };
            return Ok(ReadResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: serde_json::to_vec(&body).unwrap(),
            });
        }
        Err(GitHubError::Transport("fixture GitHub outage".into()))
    }
}

impl pip_github::MutationTransport for OfflineGitHub {
    fn send(&self, _: MutationRequest) -> Result<ReadResponse, GitHubError> {
        panic!("no external mutation is authorized by this fixture")
    }
}

#[test]
fn capability_failures_are_reported_without_aborting_unrelated_controller_phases() {
    for fault in [
        "none",
        "intake",
        "workspace_lifecycle",
        "direct_worker",
        "plan_publication",
        "authorization",
        "peer_authorization",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut policy = crate::load_repository_policy(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        policy.intake.enabled = fault == "intake";
        policy.intake.paused = false;
        policy.dispatch_enabled = true;
        policy.workspace = root.join("workspace").to_str().unwrap().into();
        policy.workspace_storage.require_distinct_filesystem = false;
        policy.workspace_storage.minimum_free_bytes = 1;
        if fault != "workspace_lifecycle" {
            fs::create_dir(&policy.workspace).unwrap();
        }
        let queue = root.join("queue");
        if fault != "direct_worker" {
            for name in ["inbox", "results", "archive"] {
                fs::create_dir_all(queue.join(name)).unwrap();
            }
        }
        let database = root.join("ledger.db");
        let mut store = Store::open(&database).unwrap();
        if matches!(
            fault,
            "plan_publication" | "authorization" | "peer_authorization"
        ) {
            store
                .create_case(&NewCase {
                    case_key: format!("repo:{}#77@3", policy.repository.id),
                    repository_id: policy.repository.id,
                    issue_number: 77,
                    workflow_version: 3,
                    policy_revision: policy.revision,
                    initial_state: if matches!(fault, "authorization" | "peer_authorization") {
                        "PLANNING"
                    } else {
                        "ABANDONED"
                    }
                    .into(),
                    observed_at: 100,
                    event: EventInput {
                        event_id: "initial".into(),
                        event_type: "ISSUE_AUTHORIZED".into(),
                        payload: json!({}),
                    },
                    effects: vec![EffectInput {
                        effect_id: "pending-plan".into(),
                        effect_type: "PUBLISH_PLAN".into(),
                        payload: json!({}),
                    }],
                })
                .unwrap();
        }
        if matches!(fault, "peer_authorization" | "workspace_lifecycle") {
            store
                .create_case(&NewCase {
                    case_key: format!("repo:{}#78@3", policy.repository.id),
                    repository_id: policy.repository.id,
                    issue_number: 78,
                    workflow_version: 3,
                    policy_revision: policy.revision,
                    initial_state: "PLANNING".into(),
                    observed_at: 100,
                    event: EventInput {
                        event_id: "healthy-intake".into(),
                        event_type: "ISSUE_AUTHORIZED".into(),
                        payload: json!({}),
                    },
                    effects: vec![],
                })
                .unwrap();
        }
        let before = store.status(100).unwrap();
        let policy_path = root.join("policy.json");
        fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
        let token = root.join("token");
        fs::write(&token, "fixture-not-a-secret").unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        let commit = root.join("commit");
        fs::write(&commit, "a".repeat(40)).unwrap();
        let askpass = root.join("askpass");
        fs::write(&askpass, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&askpass, fs::Permissions::from_mode(0o700)).unwrap();
        let mut arguments = Vec::new();
        for (name, path) in [
            ("--policy", policy_path.as_path()),
            ("--database", database.as_path()),
            ("--github-token", token.as_path()),
            ("--skills-commit-file", commit.as_path()),
            ("--direct-queue", queue.as_path()),
            ("--git-askpass", askpass.as_path()),
        ] {
            arguments.extend([name.into(), path.to_str().unwrap().into()]);
        }
        for name in [
            "--commit-signing-identity",
            "--commit-signing-key",
            "--github-reviewer-general-app",
            "--github-reviewer-general-key",
            "--github-reviewer-secperf-app",
            "--github-reviewer-secperf-key",
            "--hermes",
        ] {
            arguments.extend([name.into(), root.join("missing").to_str().unwrap().into()]);
        }
        arguments.extend([
            "--owner".into(),
            "test-controller".into(),
            "--now".into(),
            "100".into(),
        ]);
        let report = controller_cycle_with_transport(
            &arguments,
            OfflineGitHub {
                healthy: matches!(fault, "peer_authorization" | "workspace_lifecycle")
                    .then(|| policy.clone()),
            },
        )
        .unwrap_or_else(|error| panic!("{fault} aborted the cycle: {error}"));
        assert_eq!(report["ok"], fault == "none", "{fault}: {report}");
        assert_eq!(report["report_format"], 2);
        if fault == "peer_authorization" {
            let cases = report["cases"]
                .as_array()
                .expect("controller must report independently scoped cases");
            let healthy = cases
                .iter()
                .find(|case| case["case_key"] == format!("repo:{}#78@3", policy.repository.id))
                .unwrap();
            assert_eq!(healthy["authorization"]["result"], "authorized");
            assert_eq!(healthy["dispatch"]["result"], "idle");
            let blocked = cases
                .iter()
                .find(|case| case["case_key"] == format!("repo:{}#77@3", policy.repository.id))
                .unwrap();
            assert_eq!(blocked["authorization"]["result"], "blocked");
            assert_eq!(blocked["dispatch"]["result"], "authorization_blocked");
            assert_eq!(store.status(100).unwrap().cases, before.cases);
            assert_eq!(store.status(100).unwrap().outbox_leased, 0);
            continue;
        }
        let phases = &report["cases"][0];
        if fault == "authorization" {
            assert_eq!(phases[fault]["result"], "blocked");
            assert_eq!(
                phases[fault]["cases"][0]["blockers"],
                json!(["EVIDENCE_UNAVAILABLE"])
            );
            assert!(
                phases[fault]["cases"][0]["error"]
                    .as_str()
                    .unwrap()
                    .contains("fixture GitHub outage")
            );
        } else if fault == "plan_publication" {
            assert_eq!(phases[fault]["result"], "error", "{report}");
        } else if fault == "direct_worker" {
            assert_eq!(report["collection"][fault]["result"], "error", "{report}");
        } else if fault != "none" {
            assert_eq!(report[fault]["result"], "error", "{report}");
        }
        if !phases.is_null() {
            assert_eq!(phases["operational_bounds"]["result"], "idle");
            assert_eq!(
                phases["disposition"]["result"],
                if fault == "authorization" {
                    "authorization_blocked"
                } else {
                    "idle"
                }
            );
        }
        if matches!(fault, "authorization" | "workspace_lifecycle") {
            assert_eq!(phases["dispatch"]["result"], "authorization_blocked");
        }
        let after = store.status(100).unwrap();
        assert_eq!(after.cases, before.cases);
        assert_eq!(after.events, before.events);
        assert_eq!(after.outbox_delivered, 0);
        assert_eq!(after.outbox_leased, 0);
    }
}
