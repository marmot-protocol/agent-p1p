use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use pip_github::{
    CommentSpec, GitHubError, GitHubWriter, MergeModePolicy, MergeSpec, MutationRequest,
    MutationResult, MutationTransport, PullRequestReadySpec, PullRequestSpec, ReadResponse,
    ReviewEvent, ReviewMutationSpec,
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

#[test]
fn owned_exact_head_draft_is_marked_ready_through_graphql_once() {
    let transport = FakeTransport::default();
    let draft = serde_json::json!({
        "id": 9001,
        "node_id": "PR_kwDOFixture",
        "number": 77,
        "state": "open",
        "draft": true,
        "title": "Fix issue 1240",
        "body": "owned",
        "html_url": "https://github.test/pr/77",
        "user": {"id": 1001},
        "head": {"ref": "pip/repo-984321/issue-1240/workflow-1", "sha": "b".repeat(40), "repo": {"id": 984321}},
        "base": {"ref": "main"}
    });
    let mut ready = draft.clone();
    ready["draft"] = serde_json::json!(false);
    transport.push(200, &draft.to_string());
    transport.push(
        200,
        r#"{"data":{"markPullRequestReadyForReview":{"pullRequest":{"id":"PR_kwDOFixture","isDraft":false}}}}"#,
    );
    transport.push(200, &ready.to_string());

    assert_eq!(
        writer(transport.clone())
            .mark_pull_request_ready(&PullRequestReadySpec {
                owner: "marmot-protocol".into(),
                repository: "mdk".into(),
                repository_id: 984_321,
                pull_request_number: 77,
                expected_actor_id: 1001,
                expected_head_branch: "pip/repo-984321/issue-1240/workflow-1".into(),
                expected_head_sha: "b".repeat(40),
                expected_base_branch: "main".into(),
                client_mutation_id: "repo:984321#1240@1:ready".into(),
            })
            .unwrap(),
        MutationResult::Updated(77)
    );
    let requests = transport.requests.borrow();
    assert_eq!(
        [requests[0].method, requests[1].method, requests[2].method],
        ["GET", "POST", "GET"]
    );
    assert!(requests[1].url.ends_with("/graphql"));
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        body["variables"]["input"]["pullRequestId"],
        "PR_kwDOFixture"
    );
}

fn pull_request() -> PullRequestSpec {
    PullRequestSpec {
        owner: "marmot-protocol".into(),
        repository: "mdk".into(),
        repository_id: 984_321,
        effect_id: "effect-draft-pr-1".into(),
        expected_actor_id: 1001,
        title: "Fix issue 1240".into(),
        body: "Implements the accepted plan.".into(),
        head_branch: "pip/repo-984321/issue-1240/workflow-1".into(),
        head_sha: "b".repeat(40),
        base_branch: "main".into(),
    }
}

#[test]
fn draft_pull_request_is_created_once_on_the_owned_exact_head() {
    let transport = FakeTransport::default();
    transport.push(200, "[]");
    let mut response = serde_json::json!({
        "id": 9001,
        "number": 77,
        "state": "open",
        "draft": true,
        "title": "Fix issue 1240",
        "body": "marker replaced below",
        "html_url": "https://github.test/pr/77",
        "user": {"id": 1001},
        "head": {
            "ref": "pip/repo-984321/issue-1240/workflow-1",
            "sha": "b".repeat(40),
            "repo": {"id": 984321}
        },
        "base": {"ref": "main"}
    });
    let writer = writer(transport.clone());
    let expected_body = writer.render_pull_request_body(&pull_request()).unwrap();
    response["body"] = serde_json::json!(expected_body);
    transport.push(201, &response.to_string());

    assert_eq!(
        writer.ensure_draft_pull_request(&pull_request()).unwrap(),
        MutationResult::Created(77)
    );
    let requests = transport.requests.borrow();
    assert_eq!([requests[0].method, requests[1].method], ["GET", "POST"]);
    let posted: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(posted["draft"], true);
    assert_eq!(posted["head"], pull_request().head_branch);
    assert!(
        posted["body"]
            .as_str()
            .unwrap()
            .contains("effect=effect-draft-pr-1")
    );
}

#[test]
fn existing_owned_draft_is_reused_but_foreign_branch_ownership_blocks() {
    let transport = FakeTransport::default();
    let github = writer(transport.clone());
    let body = github.render_pull_request_body(&pull_request()).unwrap();
    transport.push(
        200,
        &serde_json::json!([{
            "id": 9001,
            "number": 77,
            "state": "open",
            "draft": true,
            "title": "Fix issue 1240",
            "body": body,
            "html_url": "https://github.test/pr/77",
            "user": {"id": 1001},
            "head": {"ref": pull_request().head_branch, "sha": "b".repeat(40), "repo": {"id": 984321}},
            "base": {"ref": "main"}
        }])
        .to_string(),
    );
    assert_eq!(
        github.ensure_draft_pull_request(&pull_request()).unwrap(),
        MutationResult::Existing(77)
    );
    assert_eq!(transport.requests.borrow().len(), 1);

    let transport = FakeTransport::default();
    transport.push(
        200,
        &serde_json::json!([{
            "id": 9002,
            "number": 78,
            "state": "open",
            "draft": true,
            "title": "Human PR",
            "body": "no marker",
            "html_url": "https://github.test/pr/78",
            "user": {"id": 2002},
            "head": {"ref": pull_request().head_branch, "sha": "b".repeat(40), "repo": {"id": 984321}},
            "base": {"ref": "main"}
        }])
        .to_string(),
    );
    assert!(matches!(
        writer(transport).ensure_draft_pull_request(&pull_request()),
        Err(GitHubError::OwnershipConflict)
    ));
}

#[test]
fn owned_draft_content_is_updated_without_changing_its_head() {
    let transport = FakeTransport::default();
    let github = writer(transport.clone());
    let desired = pull_request();
    let mut previous = desired.clone();
    previous.title = "Earlier title".into();
    previous.body = "Earlier body.".into();
    let previous_body = github.render_pull_request_body(&previous).unwrap();
    let desired_body = github.render_pull_request_body(&desired).unwrap();
    transport.push(
        200,
        &serde_json::json!([{
            "id": 9001,
            "number": 77,
            "state": "open",
            "draft": true,
            "title": previous.title,
            "body": previous_body,
            "html_url": "https://github.test/pr/77",
            "user": {"id": 1001},
            "head": {"ref": desired.head_branch, "sha": desired.head_sha, "repo": {"id": 984321}},
            "base": {"ref": "main"}
        }])
        .to_string(),
    );
    transport.push(
        200,
        &serde_json::json!({
            "id": 9001,
            "number": 77,
            "state": "open",
            "draft": true,
            "title": desired.title,
            "body": desired_body,
            "html_url": "https://github.test/pr/77",
            "user": {"id": 1001},
            "head": {"ref": desired.head_branch, "sha": desired.head_sha, "repo": {"id": 984321}},
            "base": {"ref": "main"}
        })
        .to_string(),
    );

    assert_eq!(
        github.ensure_draft_pull_request(&desired).unwrap(),
        MutationResult::Updated(77)
    );
    let requests = transport.requests.borrow();
    assert_eq!(requests[1].method, "PATCH");
    let patched: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(patched["title"], desired.title);
    assert!(patched.get("head").is_none());
}

#[test]
fn exact_head_review_is_published_once_with_role_provenance() {
    let spec = ReviewMutationSpec {
        owner: "marmot-protocol".into(),
        repository: "mdk".into(),
        pull_request_number: 77,
        effect_id: "effect-review-secperf-r1".into(),
        expected_actor_id: 1001,
        expected_head_sha: "b".repeat(40),
        body: "No blocking security findings.".into(),
        event: ReviewEvent::Approve,
    };
    let transport = FakeTransport::default();
    transport.push(200, "[]");
    let writer = writer(transport.clone());
    let expected_body = writer.render_review_body(&spec).unwrap();
    transport.push(
        200,
        &serde_json::json!({
            "id": 81,
            "user": {"id": 1001},
            "body": expected_body,
            "state": "APPROVED",
            "commit_id": "b".repeat(40),
            "html_url": "https://github.test/pr/77#review-81"
        })
        .to_string(),
    );
    assert_eq!(
        writer.ensure_pull_request_review(&spec).unwrap(),
        MutationResult::Created(81)
    );
    let requests = transport.requests.borrow();
    assert_eq!([requests[0].method, requests[1].method], ["GET", "POST"]);
    let posted: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(posted["commit_id"], "b".repeat(40));
    assert_eq!(posted["event"], "APPROVE");
}

#[test]
fn exact_review_replay_is_a_noop_but_head_drift_conflicts() {
    let spec = ReviewMutationSpec {
        owner: "marmot-protocol".into(),
        repository: "mdk".into(),
        pull_request_number: 77,
        effect_id: "effect-review-general-r1".into(),
        expected_actor_id: 1001,
        expected_head_sha: "b".repeat(40),
        body: "No blocking general findings.".into(),
        event: ReviewEvent::Approve,
    };
    let transport = FakeTransport::default();
    let github = writer(transport.clone());
    let body = github.render_review_body(&spec).unwrap();
    transport.push(
        200,
        &serde_json::json!([{
            "id": 82,
            "user": {"id": 1001},
            "body": body,
            "state": "APPROVED",
            "commit_id": "b".repeat(40),
            "html_url": "https://github.test/pr/77#review-82"
        }])
        .to_string(),
    );
    assert_eq!(
        github.ensure_pull_request_review(&spec).unwrap(),
        MutationResult::Existing(82)
    );
    assert_eq!(transport.requests.borrow().len(), 1);

    let transport = FakeTransport::default();
    let github = writer(transport.clone());
    let body = github.render_review_body(&spec).unwrap();
    transport.push(
        200,
        &serde_json::json!([{
            "id": 82,
            "user": {"id": 1001},
            "body": body,
            "state": "APPROVED",
            "commit_id": "c".repeat(40),
            "html_url": "https://github.test/pr/77#review-82"
        }])
        .to_string(),
    );
    assert!(matches!(
        github.ensure_pull_request_review(&spec),
        Err(GitHubError::IdempotencyConflict)
    ));
}
