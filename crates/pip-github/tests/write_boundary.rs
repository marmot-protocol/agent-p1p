use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use pip_github::{
    CommentSpec, GitHubError, GitHubWriter, MergeModePolicy, MergeSpec, MutationRequest,
    MutationResult, MutationTransport, ReadResponse,
};

#[derive(Clone, Default)]
struct FakeTransport {
    responses: Rc<RefCell<VecDeque<Result<ReadResponse, GitHubError>>>>,
    requests: Rc<RefCell<Vec<MutationRequest>>>,
}

impl FakeTransport {
    fn push(&self, status: u16, body: &str) {
        self.responses.borrow_mut().push_back(Ok(ReadResponse {
            status,
            headers: BTreeMap::new(),
            body: body.as_bytes().to_vec(),
        }));
    }
}

impl MutationTransport for FakeTransport {
    fn send(&self, request: MutationRequest) -> Result<ReadResponse, GitHubError> {
        self.requests.borrow_mut().push(request);
        self.responses.borrow_mut().pop_front().unwrap()
    }
}

fn writer(transport: FakeTransport) -> GitHubWriter<FakeTransport> {
    GitHubWriter::new(
        transport,
        "https://api.github.test",
        "fixture-token",
        4096,
        3,
    )
    .unwrap()
}

fn comment() -> CommentSpec {
    CommentSpec {
        owner: "marmot-protocol".into(),
        repository: "mdk".into(),
        issue_number: 1240,
        effect_id: "effect-plan-comment-1".into(),
        expected_actor_id: 1001,
        body: "Structured planner evidence.".into(),
    }
}

#[test]
fn issue_comment_is_created_once_with_provenance_marker() {
    let transport = FakeTransport::default();
    transport.push(200, "[]");
    transport.push(
        201,
        r#"{"id":55,"user":{"id":1001},"body":"<!-- pip-control:v1 effect=effect-plan-comment-1 sha256=79957938c7c187cee5e26af1933f7995db2401b4e44e18f33c2b44fb4f24ebcf -->\nStructured planner evidence.","html_url":"https://github.test/comment/55"}"#,
    );
    assert_eq!(
        writer(transport.clone())
            .ensure_issue_comment(&comment())
            .unwrap(),
        MutationResult::Created(55)
    );
    let requests = transport.requests.borrow();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[1].method, "POST");
    let posted: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(
        posted["body"]
            .as_str()
            .unwrap()
            .starts_with("<!-- pip-control:v1 effect=effect-plan-comment-1 sha256=")
    );
    assert!(requests.iter().all(|request| {
        request.headers.get("authorization").map(String::as_str) == Some("Bearer fixture-token")
    }));
}

#[test]
fn exact_existing_comment_is_a_noop_and_conflicts_fail_closed() {
    let transport = FakeTransport::default();
    transport.push(
        200,
        r#"[{"id":55,"user":{"id":1001},"body":"<!-- pip-control:v1 effect=effect-plan-comment-1 sha256=79957938c7c187cee5e26af1933f7995db2401b4e44e18f33c2b44fb4f24ebcf -->\nStructured planner evidence.","html_url":"https://github.test/comment/55"}]"#,
    );
    assert_eq!(
        writer(transport.clone())
            .ensure_issue_comment(&comment())
            .unwrap(),
        MutationResult::Existing(55)
    );
    assert_eq!(transport.requests.borrow().len(), 1);

    let transport = FakeTransport::default();
    transport.push(
        200,
        r#"[{"id":55,"user":{"id":1001},"body":"<!-- pip-control:v1 effect=effect-plan-comment-1 sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -->\nchanged","html_url":"https://github.test/comment/55"}]"#,
    );
    assert!(matches!(
        writer(transport).ensure_issue_comment(&comment()),
        Err(GitHubError::IdempotencyConflict)
    ));
}

#[test]
fn duplicate_or_foreign_comment_ownership_blocks_without_writing() {
    let exact = r#"<!-- pip-control:v1 effect=effect-plan-comment-1 sha256=79957938c7c187cee5e26af1933f7995db2401b4e44e18f33c2b44fb4f24ebcf -->\nStructured planner evidence."#;
    let transport = FakeTransport::default();
    transport.push(
        200,
        &format!(
            r#"[{{"id":55,"user":{{"id":1001}},"body":"{exact}","html_url":"x"}},{{"id":56,"user":{{"id":1001}},"body":"{exact}","html_url":"y"}}]"#
        ),
    );
    assert!(matches!(
        writer(transport).ensure_issue_comment(&comment()),
        Err(GitHubError::DuplicateOwnership)
    ));

    let transport = FakeTransport::default();
    transport.push(
        200,
        &format!(r#"[{{"id":55,"user":{{"id":9999}},"body":"{exact}","html_url":"x"}}]"#),
    );
    assert!(matches!(
        writer(transport).ensure_issue_comment(&comment()),
        Err(GitHubError::OwnershipConflict)
    ));
}

#[test]
fn shadow_or_disabled_policy_cannot_send_a_merge_request() {
    let transport = FakeTransport::default();
    let merge = MergeSpec {
        owner: "marmot-protocol".into(),
        repository: "mdk".into(),
        pull_request_number: 77,
        expected_head_sha: "b".repeat(40),
        commit_title: "Pip shadow result".into(),
        method: "squash".into(),
    };
    assert!(matches!(
        writer(transport.clone()).merge_pull_request(
            &merge,
            MergeModePolicy {
                guarded: false,
                autonomous_merge: false,
            },
        ),
        Err(GitHubError::MutationDisabled)
    ));
    assert!(transport.requests.borrow().is_empty());
}
