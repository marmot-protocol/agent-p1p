use std::cell::RefCell;
use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pip_github::{
    GitHubAppCredentials, GitHubError, ReadRequest, ReadResponse, ReadTransport,
    mint_installation_token,
};

const TEST_KEY: &[u8] = include_bytes!("fixtures/github-app-test-key.pem");

#[derive(Default)]
struct FakeTransport {
    requests: RefCell<Vec<ReadRequest>>,
    response: RefCell<Option<Result<ReadResponse, GitHubError>>>,
}

impl FakeTransport {
    fn returning(response: ReadResponse) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            response: RefCell::new(Some(Ok(response))),
        }
    }
}

impl ReadTransport for FakeTransport {
    fn get(&self, _request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        panic!("installation token minting must not issue GET")
    }

    fn post(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        self.requests.borrow_mut().push(request);
        self.response.borrow_mut().take().unwrap()
    }
}

#[test]
fn app_credentials_mint_a_repository_scoped_installation_token() {
    let transport = FakeTransport::returning(response(
        201,
        br#"{"token":"ghs_fixture.stateless-token","expires_at":"2026-09-01T20:00:00Z"}"#,
    ));
    let credentials = credentials();

    let token = mint_installation_token(
        &transport,
        "https://api.github.com",
        &credentials,
        1_788_290_400,
    )
    .unwrap();

    assert_eq!(token.token, "ghs_fixture.stateless-token");
    assert_eq!(token.expires_at, "2026-09-01T20:00:00Z");
    let requests = transport.requests.borrow();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url,
        "https://api.github.com/app/installations/987654/access_tokens"
    );
    assert_eq!(request.max_bytes, 64 * 1024);
    assert_eq!(
        request.headers.get("accept").map(String::as_str),
        Some("application/vnd.github+json")
    );
    assert_eq!(
        request
            .headers
            .get("x-github-api-version")
            .map(String::as_str),
        Some("2022-11-28")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({"repository_ids": [1_055_628_515_u64]})
    );

    let authorization = request.headers.get("authorization").unwrap();
    let jwt = authorization.strip_prefix("Bearer ").unwrap();
    let parts = jwt.split('.').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    assert!(!parts[2].is_empty());
    let header: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    let claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(header["alg"], "RS256");
    assert_eq!(header["typ"], "JWT");
    assert_eq!(claims["iss"], "123456");
    assert_eq!(claims["iat"], 1_788_290_340_u64);
    assert_eq!(claims["exp"], 1_788_290_940_u64);
}

#[test]
fn app_token_minting_fails_closed_on_configuration_or_response_drift() {
    for credentials in [
        GitHubAppCredentials {
            app_id: 0,
            ..credentials()
        },
        GitHubAppCredentials {
            installation_id: 0,
            ..credentials()
        },
        GitHubAppCredentials {
            repository_id: 0,
            ..credentials()
        },
        GitHubAppCredentials {
            private_key_pem: b"not a PEM",
            ..credentials()
        },
    ] {
        let transport = FakeTransport::returning(response(201, valid_response()));
        assert_eq!(
            mint_installation_token(
                &transport,
                "https://api.github.com",
                &credentials,
                1_788_290_400,
            ),
            Err(GitHubError::InvalidConfiguration)
        );
        assert!(transport.requests.borrow().is_empty());
    }

    for (status, body) in [
        (200, valid_response()),
        (201, br#"{}"#),
        (201, br#"{"token":"bad token","expires_at":"soon"}"#),
        (201, br#"{"token":"ghs_ok","expires_at":""}"#),
    ] {
        let transport = FakeTransport::returning(response(status, body));
        assert!(
            mint_installation_token(
                &transport,
                "https://api.github.com",
                &credentials(),
                1_788_290_400,
            )
            .is_err()
        );
    }
}

fn credentials() -> GitHubAppCredentials<'static> {
    GitHubAppCredentials {
        app_id: 123456,
        installation_id: 987654,
        repository_id: 1_055_628_515,
        private_key_pem: TEST_KEY,
    }
}

fn response(status: u16, body: &[u8]) -> ReadResponse {
    ReadResponse {
        status,
        headers: BTreeMap::new(),
        body: body.to_vec(),
    }
}

fn valid_response() -> &'static [u8] {
    br#"{"token":"ghs_fixture.stateless-token","expires_at":"2026-09-01T20:00:00Z"}"#
}
