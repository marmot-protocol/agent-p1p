use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use pip_github::{
    CheckConclusion, GitHubError, GitHubReader, ReadRequest, ReadResponse, ReadTransport,
    ReviewState, verify_webhook,
};

#[derive(Clone, Default)]
struct FakeTransport {
    responses: Rc<RefCell<VecDeque<Result<ReadResponse, GitHubError>>>>,
    requests: Rc<RefCell<Vec<ReadRequest>>>,
}

impl FakeTransport {
    fn push(&self, response: ReadResponse) {
        self.responses.borrow_mut().push_back(Ok(response));
    }
}

impl ReadTransport for FakeTransport {
    fn get(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        self.requests.borrow_mut().push(request);
        self.responses.borrow_mut().pop_front().unwrap()
    }
}

fn response(body: &str) -> ReadResponse {
    ReadResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn reader(transport: FakeTransport) -> GitHubReader<FakeTransport> {
    GitHubReader::new(
        transport,
        "https://api.github.test",
        "fixture-token",
        4096,
        3,
    )
    .unwrap()
}

#[test]
fn official_github_webhook_vector_verifies_in_constant_time_path() {
    let signature = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
    assert!(verify_webhook(
        b"It's a Secret to Everybody",
        b"Hello, World!",
        signature
    ));
    assert!(!verify_webhook(
        b"It's a Secret to Everybody",
        b"Hello, World?",
        signature
    ));
    assert!(!verify_webhook(b"secret", b"payload", "sha1=bad"));
}

#[test]
fn intake_read_uses_numeric_identity_authentication_and_bounded_pagination() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":984321,"full_name":"marmot-protocol/mdk","default_branch":"main"}"#,
    ));
    transport.push(response(
        r#"{"id":555,"number":1240,"state":"open","pull_request":null,"labels":[{"name":"bug"},{"name":"pip-ok"}]}"#,
    ));
    let mut first_events = response(
        r#"[{"id":1,"event":"labeled","actor":{"id":1001},"label":{"name":"pip-ok"},"created_at":"2026-08-20T00:00:00Z"}]"#,
    );
    first_events.headers.insert(
        "link".into(),
        r#"<https://api.github.test/repos/marmot-protocol/mdk/issues/1240/events?per_page=100&page=2>; rel="next""#.into(),
    );
    transport.push(first_events);
    transport.push(response(
        r#"[{"id":2,"event":"unlabeled","actor":{"id":1002},"label":{"name":"other"},"created_at":"2026-08-20T00:01:00Z"}]"#,
    ));

    let snapshot = reader(transport.clone())
        .read_intake("marmot-protocol", "mdk", 1240)
        .unwrap();
    assert_eq!(snapshot.repository.id, 984_321);
    assert_eq!(snapshot.issue.id, 555);
    assert!(snapshot.issue.labels.contains("pip-ok"));
    assert_eq!(snapshot.label_events.len(), 2);
    assert_eq!(snapshot.label_events[0].actor_id, 1001);

    let requests = transport.requests.borrow();
    assert_eq!(requests.len(), 4);
    assert!(requests.iter().all(|request| {
        request.headers.get("authorization").map(String::as_str) == Some("Bearer fixture-token")
            && request.headers.contains_key("x-github-api-version")
    }));
    assert!(requests.iter().all(|request| request.method == "GET"));
}

#[test]
fn generic_intake_discovery_lists_labeled_issues_without_a_canary_constant() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"[{"id":555,"number":1240,"state":"open","pull_request":null,"labels":[{"name":"pip-ok"}]},{"id":556,"number":1241,"state":"open","pull_request":{"url":"x"},"labels":[{"name":"pip-ok"}]}]"#,
    ));
    let issues = reader(transport.clone())
        .discover_open_issues("marmot-protocol", "mdk", "pip-ok")
        .unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].number, 1240);
    assert!(!issues[0].is_pull_request);
    assert!(issues[1].is_pull_request);
    let requests = transport.requests.borrow();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].url.contains(
        "/repos/marmot-protocol/mdk/issues?state=open&labels=pip-ok&per_page=100&page=1"
    ));
}

#[test]
fn oversized_malformed_and_non_success_responses_fail_closed() {
    let transport = FakeTransport::default();
    transport.push(ReadResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: vec![b'x'; 4097],
    });
    assert!(matches!(
        reader(transport).read_intake("owner", "repo", 1),
        Err(GitHubError::ResponseTooLarge)
    ));

    let transport = FakeTransport::default();
    transport.push(response("not-json"));
    assert!(matches!(
        reader(transport).read_intake("owner", "repo", 1),
        Err(GitHubError::MalformedJson(_))
    ));

    let transport = FakeTransport::default();
    transport.push(ReadResponse {
        status: 403,
        headers: BTreeMap::new(),
        body: b"{}".to_vec(),
    });
    assert!(matches!(
        reader(transport).read_intake("owner", "repo", 1),
        Err(GitHubError::HttpStatus(403))
    ));
}

#[test]
fn pagination_cannot_send_authorization_to_another_origin() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":984321,"full_name":"owner/repo","default_branch":"main"}"#,
    ));
    transport.push(response(
        r#"{"id":555,"number":1,"state":"open","labels":[]}"#,
    ));
    let mut events = response("[]");
    events.headers.insert(
        "link".into(),
        r#"<https://evil.example/steal>; rel="next""#.into(),
    );
    transport.push(events);

    assert!(matches!(
        reader(transport.clone()).read_intake("owner", "repo", 1),
        Err(GitHubError::UnsafePaginationUrl)
    ));
    assert_eq!(transport.requests.borrow().len(), 3);
}

#[test]
fn pagination_limit_blocks_unbounded_history() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":984321,"full_name":"owner/repo","default_branch":"main"}"#,
    ));
    transport.push(response(
        r#"{"id":555,"number":1,"state":"open","labels":[]}"#,
    ));
    for page in 1..=3 {
        let mut events = response("[]");
        events.headers.insert(
            "link".into(),
            format!(
                "<https://api.github.test/repos/owner/repo/issues/1/events?page={}>; rel=\"next\"",
                page + 1
            ),
        );
        transport.push(events);
    }

    assert!(matches!(
        reader(transport).read_intake("owner", "repo", 1),
        Err(GitHubError::PaginationLimit)
    ));
}

#[test]
fn pull_request_evidence_retains_all_attempts_and_exact_head_reviews() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":9001,"number":77,"state":"open","draft":true,"merged":false,"mergeable":true,"mergeable_state":"clean","user":{"id":1001},"head":{"ref":"pip/v2/repo-984321/issue-1240/workflow-1","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":{"id":984321,"full_name":"marmot-protocol/mdk"}},"base":{"ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
    ));
    let mut first_checks = response(
        r#"{"total_count":2,"check_runs":[{"id":1,"name":"ci","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"completed","conclusion":"failure","started_at":"2026-08-20T00:00:00Z","completed_at":"2026-08-20T00:01:00Z","app":{"id":10}}]}"#,
    );
    first_checks.headers.insert(
        "link".into(),
        r#"<https://api.github.test/repos/marmot-protocol/mdk/commits/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/check-runs?filter=all&per_page=100&page=2>; rel="next""#.into(),
    );
    transport.push(first_checks);
    transport.push(response(
        r#"{"total_count":2,"check_runs":[{"id":2,"name":"ci","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"completed","conclusion":"success","started_at":"2026-08-20T00:02:00Z","completed_at":"2026-08-20T00:03:00Z","app":{"id":10}}]}"#,
    ));
    transport.push(response(
        r#"{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","state":"success","statuses":[{"id":3,"context":"legacy-ci","state":"success","creator":{"id":1002},"created_at":"2026-08-20T00:03:00Z","updated_at":"2026-08-20T00:04:00Z"}]}"#,
    ));
    transport.push(response(
        r#"[{"id":4,"user":{"id":1003},"state":"APPROVED","commit_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","submitted_at":"2026-08-20T00:05:00Z","body":"looks good"},{"id":5,"user":{"id":1004},"state":"CHANGES_REQUESTED","commit_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","submitted_at":"2026-08-20T00:01:00Z","body":"stale"}]"#,
    ));

    let evidence = reader(transport.clone())
        .read_pull_request("marmot-protocol", "mdk", 984_321, 77)
        .unwrap();
    assert_eq!(evidence.pull_request.id, 9001);
    assert_eq!(evidence.pull_request.head_sha, "b".repeat(40));
    assert_eq!(evidence.check_runs.len(), 2);
    assert_eq!(
        evidence.check_runs[0].conclusion,
        Some(CheckConclusion::Failure)
    );
    assert_eq!(
        evidence.check_runs[1].conclusion,
        Some(CheckConclusion::Success)
    );
    assert_eq!(evidence.commit_statuses.len(), 1);
    assert_eq!(evidence.reviews[0].state, ReviewState::Approved);
    assert!(evidence.reviews[0].exact_head);
    assert!(!evidence.reviews[1].exact_head);

    let requests = transport.requests.borrow();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert!(requests[1].url.contains("filter=all"));
}

#[test]
fn pull_request_evidence_rejects_wrong_repository_head_and_duplicate_attempts() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":9001,"number":77,"state":"open","draft":true,"merged":false,"mergeable":true,"mergeable_state":"clean","user":{"id":1001},"head":{"ref":"pip/v2/case","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":{"id":111,"full_name":"foreign/repo"}},"base":{"ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
    ));
    assert!(matches!(
        reader(transport).read_pull_request("owner", "repo", 222, 77),
        Err(GitHubError::InvalidIdentity)
    ));

    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":9001,"number":77,"state":"open","draft":true,"merged":false,"mergeable":null,"mergeable_state":"unknown","user":{"id":1001},"head":{"ref":"pip/v2/case","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":{"id":222,"full_name":"owner/repo"}},"base":{"ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
    ));
    transport.push(response(
        r#"{"total_count":2,"check_runs":[{"id":1,"name":"ci","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"completed","conclusion":"success","started_at":null,"completed_at":null,"app":{"id":10}},{"id":1,"name":"ci","head_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"completed","conclusion":"success","started_at":null,"completed_at":null,"app":{"id":10}}]}"#,
    ));
    assert!(matches!(
        reader(transport).read_pull_request("owner", "repo", 222, 77),
        Err(GitHubError::InvalidIdentity)
    ));
}

#[test]
fn issue_comment_evidence_is_bound_to_numeric_actor_issue_and_content_digest() {
    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":10001,"user":{"id":1001},"issue_url":"https://api.github.test/repos/marmot-protocol/mdk/issues/1240","html_url":"https://github.test/marmot-protocol/mdk/issues/1240#issuecomment-10001","body":"immutable plan","created_at":"2026-08-20T00:00:00Z","updated_at":"2026-08-20T00:00:00Z"}"#,
    ));
    let comment = reader(transport.clone())
        .read_issue_comment("marmot-protocol", "mdk", 1240, 10001)
        .unwrap();
    assert_eq!(comment.id, 10001);
    assert_eq!(comment.actor_id, 1001);
    assert_eq!(comment.issue_number, 1240);
    assert_eq!(comment.body, "immutable plan");
    assert_eq!(comment.body_sha256.len(), 64);
    assert!(
        transport.requests.borrow()[0]
            .url
            .ends_with("/repos/marmot-protocol/mdk/issues/comments/10001")
    );

    let transport = FakeTransport::default();
    transport.push(response(
        r#"{"id":10001,"user":{"id":1001},"issue_url":"https://api.github.test/repos/marmot-protocol/mdk/issues/999","html_url":"https://github.test/comment","body":"wrong issue","created_at":"2026-08-20T00:00:00Z","updated_at":"2026-08-20T00:00:00Z"}"#,
    ));
    assert!(matches!(
        reader(transport).read_issue_comment("marmot-protocol", "mdk", 1240, 10001),
        Err(GitHubError::InvalidIdentity)
    ));
}
