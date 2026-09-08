use hmac::{Hmac, KeyInit, Mac};
use pip_control::{IntakeSource, WebhookEnvelope, ingest_webhook, load_repository_policy};
use pip_github::{DiscussionComment, GitHubError, IntakeSnapshot, IssueSnapshot};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError, TaskSnapshot};
use pip_store::Store;
use serde_json::json;
use sha2::Sha256;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Default)]
struct Queue(Rc<RefCell<Vec<TaskSnapshot>>>, Rc<std::cell::Cell<bool>>);
impl CommandRunner for Queue {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput, HermesError> {
        let args = &command.args;
        let value = match args[3].as_str() {
            "list" => serde_json::to_value(&*self.0.borrow()).unwrap(),
            "create" => {
                let arg = |name| args[args.iter().position(|s| s == name).unwrap() + 1].clone();
                let task: TaskSnapshot = serde_json::from_value(json!({
                    "id":"task-conversation", "title":args[4],"status":"ready",
                    "assignee":arg("--assignee"),"created_by":"pip-controller",
                    "body":arg("--body"),"workspace_kind":"scratch","workspace_path":null,
                    "skills":["conversation","workflow-contract"],"provider_override":arg("--provider"),
                    "model_override":arg("--model"),"max_retries":1,"priority":10
                })).unwrap();
                self.0.borrow_mut().push(task.clone());
                serde_json::to_value(task).unwrap()
            }
            "show" => {
                let task = self.0.borrow()[0].clone();
                let body: serde_json::Value = serde_json::from_str(&task.body).unwrap();
                json!({"task":task,"parents":[],"runs":[{"outcome":"completed","profile":"conversation",
                    "metadata":{"schema_version":1,"message_key":body["message_key"],"reply":"Here is the explanation.","follow_up":if self.1.get() {"REPLAN"} else {"NONE"},
                    "requested_model":format!("{}/{}",body["provider"].as_str().unwrap(),body["model"].as_str().unwrap()),
                    "actual_model":format!("{}/{}",body["provider"].as_str().unwrap(),body["model"].as_str().unwrap()),
                    "skills_repository_commit":body["skills_repository_commit"]}}]})
            }
            other => panic!("unexpected Hermes command: {other}"),
        };
        Ok(CommandOutput {
            status: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: vec![],
            timed_out: false,
        })
    }
}
#[derive(Default)]
struct Replies(RefCell<Vec<String>>, std::cell::Cell<bool>);
impl pip_control::DispositionWriter for Replies {
    fn ensure_comment(
        &self,
        spec: &pip_github::CommentSpec,
    ) -> Result<pip_github::MutationResult, GitHubError> {
        if !self.0.borrow().contains(&spec.body) {
            self.0.borrow_mut().push(spec.body.clone());
        }
        if self.1.replace(false) {
            return Err(GitHubError::Transport(
                "uncertain publication response".into(),
            ));
        }
        Ok(pip_github::MutationResult::Created(600))
    }
    fn mark_ready(
        &self,
        _: &pip_github::PullRequestReadySpec,
    ) -> Result<pip_github::MutationResult, GitHubError> {
        panic!("conversation cannot mark a PR ready")
    }
}

#[test]
fn conversation_runs_once_and_publishes_without_creating_a_case() {
    conversation_roundtrip(false);
}

#[test]
fn clarification_replans_once_preserving_history_and_label_authority() {
    conversation_roundtrip(true);
}

fn conversation_roundtrip(follow_up: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut path = dir.path().join("ledger.db");
    let mut store = Store::open(&path).unwrap();
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.conversations_enabled = true;
    policy.dispatch_enabled = true;
    policy.intake.paused = false;
    policy.github.automation_actor_id = Some(88);
    policy.intake.trusted_actor_ids = vec![99];
    let source = Source {
        comment: DiscussionComment {
            id: 500,
            actor_id: 99,
            human: true,
            body: "@pip-renamed explain this".into(),
            updated_at: "2026-09-08T12:00:00Z".into(),
            reply_to: None,
            context: serde_json::Value::Null,
        },
    };
    let key = "conversation-123";
    store
        .record_conversation(&pip_store::ConversationInput {
            key: key.into(),
            repository_id: policy.repository.id,
            thread_number: 42,
            actor_id: 99,
            case_key: None,
            received_at: 100,
            payload: json!({"kind":"issue_comment","comment":source.comment}),
        })
        .unwrap();
    let queue = Queue::default();
    if follow_up {
        queue.1.set(true);
        let case_key = format!("repo:{}#42@3", policy.repository.id);
        store
            .create_case(&pip_store::NewCase {
                case_key: case_key.clone(),
                repository_id: policy.repository.id,
                issue_number: 42,
                workflow_version: 3,
                policy_revision: policy.revision,
                initial_state: "WAITING_HUMAN".into(),
                observed_at: 90,
                event: pip_store::EventInput {
                    event_id: "authorized-before-conversation".into(),
                    event_type: "ISSUE_AUTHORIZED".into(),
                    payload: json!({}),
                },
                effects: vec![],
            })
            .unwrap();
        // Bind a distinct immutable input, rather than modifying the first one.
        store.finish_conversation(key, "IGNORED", None).unwrap();
        store
            .record_conversation(&pip_store::ConversationInput {
                key: "feedback-123".into(),
                repository_id: policy.repository.id,
                thread_number: 42,
                actor_id: 99,
                case_key: Some(case_key),
                received_at: 100,
                payload: json!({"kind":"issue_comment","comment":source.comment}),
            })
            .unwrap();
    }
    let writer = Replies::default();
    let run = |store: &mut Store| {
        pip_control::reconcile_conversation_once(
            &AuthorizedSource(&source, follow_up),
            &writer,
            &policy,
            store,
            queue.clone(),
            ("hermes", &"a".repeat(40)),
            101,
        )
    };
    assert_eq!(run(&mut store).unwrap()["result"], "queued");
    assert_eq!(queue.0.borrow().len(), 1);
    if !follow_up {
        let message = store.conversation(key).unwrap().unwrap();
        path = dir.path().join("crash-before-binding.db");
        let mut recovered = Store::open(&path).unwrap();
        recovered.record_conversation(&message.input).unwrap();
        recovered
            .reserve_conversation(key, &message.spec.unwrap())
            .unwrap();
        assert_eq!(run(&mut recovered).unwrap()["result"], "waiting");
        assert_eq!(queue.0.borrow().len(), 1);
        store = recovered;
    }
    assert_eq!(run(&mut store).unwrap()["result"], "waiting");
    queue.0.borrow_mut()[0].status = "done".into();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    writer.1.set(true);
    assert!(run(&mut store).is_err());
    assert_eq!(run(&mut store).unwrap()["result"], "published");
    assert_eq!(run(&mut store).unwrap()["result"], "idle");
    assert_eq!(writer.0.borrow().len(), 1);
    assert!(writer.0.borrow()[0].contains("Here is the explanation."));
    if follow_up {
        let case = &store.status(102).unwrap().cases[0];
        assert_eq!(case.state, "PLANNING");
        assert_eq!(case.state_revision, 2);
        assert_eq!(case.remediation_round, 1);
        let history = store.immutable_history_for_case(&case.case_key).unwrap();
        assert_eq!(history.events.len(), 2);
        assert_eq!(history.evidence[0].kind, "HUMAN_DISCUSSION");
    } else {
        assert!(store.status(102).unwrap().cases.is_empty());
        assert_eq!(store.outbox_count().unwrap(), 0);
    }
}

struct AuthorizedSource<'a>(&'a Source, bool);
impl IntakeSource for AuthorizedSource<'_> {
    fn discover(&self, a: &str, b: &str, c: &str) -> Result<Vec<IssueSnapshot>, GitHubError> {
        self.0.discover(a, b, c)
    }
    fn actor_login(&self, id: u64) -> Result<String, GitHubError> {
        self.0.actor_login(id)
    }
    fn discussion_comment(
        &self,
        a: &str,
        b: &str,
        n: u64,
        k: &str,
        id: u64,
    ) -> Result<DiscussionComment, GitHubError> {
        self.0.discussion_comment(a, b, n, k, id)
    }
    fn intake(&self, a: &str, b: &str, n: u64) -> Result<IntakeSnapshot, GitHubError> {
        let mut snapshot = self.0.intake(a, b, n)?;
        if self.1 {
            snapshot.issue.labels.insert("pip-ok".into());
            snapshot.label_events.push(pip_github::LabelEvent {
                id: 1,
                actor_id: 99,
                label: "pip-ok".into(),
                labeled: true,
                created_at: "2026-09-08".into(),
            });
        }
        Ok(snapshot)
    }
}

struct Source {
    comment: DiscussionComment,
}
impl IntakeSource for Source {
    fn discover(&self, _: &str, _: &str, _: &str) -> Result<Vec<IssueSnapshot>, GitHubError> {
        panic!("comments never discover build candidates")
    }
    fn intake(&self, _: &str, _: &str, _: u64) -> Result<IntakeSnapshot, GitHubError> {
        Ok(IntakeSnapshot {
            repository: pip_github::RepositorySnapshot {
                id: 1055628515,
                full_name: "marmot-protocol/mdk".into(),
                default_branch: "master".into(),
            },
            issue: IssueSnapshot {
                id: 123,
                number: 42,
                open: true,
                is_pull_request: false,
                labels: Default::default(),
            },
            issue_content: pip_github::IssueContentSnapshot {
                author_id: 99,
                title: "Question".into(),
                body: String::new(),
                created_at: "2026-09-08".into(),
                updated_at: "2026-09-08".into(),
            },
            label_events: vec![],
            comments: vec![],
        })
    }
    fn actor_login(&self, id: u64) -> Result<String, GitHubError> {
        assert_eq!(id, 88);
        Ok("pip-renamed".into())
    }
    fn discussion_comment(
        &self,
        _: &str,
        _: &str,
        n: u64,
        _: &str,
        id: u64,
    ) -> Result<DiscussionComment, GitHubError> {
        assert_eq!((n, id), (42, 500));
        Ok(self.comment.clone())
    }
}

#[test]
fn signed_mentions_route_without_a_label_or_workflow_and_ignore_untrusted_bots() {
    for (actor, human, body, expected) in [
        (99, true, "@pip-renamed explain this", 1),
        (77, true, "@pip-renamed explain this", 0),
        (88, true, "@pip-renamed explain this", 0),
        (99, false, "@pip-renamed explain this", 0),
        (99, true, "@pip-renamed-extra explain this", 0),
        (99, true, "> @pip-renamed quoted text", 0),
        (99, true, "`@pip-renamed` code example", 0),
        (99, true, "```\n@pip-renamed\n```", 0),
        (99, true, "Hi @PIP-RENAMED, explain this", 1),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        let mut policy = load_repository_policy(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        policy.conversations_enabled = true;
        policy.github.automation_actor_id = Some(88);
        policy.intake.trusted_actor_ids = vec![99];
        let source = Source {
            comment: DiscussionComment {
                id: 500,
                actor_id: actor,
                human,
                body: body.into(),
                updated_at: "2026-09-08T12:00:00Z".into(),
                reply_to: None,
                context: serde_json::Value::Null,
            },
        };
        let bytes=serde_json::to_vec(&json!({"action":"created","repository":{"id":policy.repository.id,"full_name":policy.repository.full_name()},"issue":{"id":123,"number":42},"comment":{"id":500,"body":body,"user":{"id":actor}},"sender":{"id":actor}})).unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(&bytes);
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        for _ in 0..2 {
            ingest_webhook(
                &source,
                &policy,
                &mut store,
                WebhookEnvelope {
                    delivery_id: "01234567-89ab-cdef-0123-456789abcdef",
                    event_name: "issue_comment",
                    signature: &signature,
                    payload: &bytes,
                    received_at: 100,
                },
                b"secret",
                101,
                false,
            )
            .unwrap();
        }
        assert_eq!(
            store.conversations(policy.repository.id, 10).unwrap().len(),
            expected
        );
        assert!(store.status(101).unwrap().cases.is_empty());
        assert_eq!(store.outbox_count().unwrap(), 0);
    }
}
