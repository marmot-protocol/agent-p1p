use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use pip_github::{
    GitHubError, GitHubReader, ReadRequest, ReadResponse, ReadTransport, verify_webhook,
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
