use pip_github::{GitHubError, GitHubReader, ReadRequest, ReadResponse, ReadTransport};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
};

struct Transport {
    replies: RefCell<VecDeque<Value>>,
    requests: RefCell<Vec<ReadRequest>>,
}
impl ReadTransport for Transport {
    fn get(&self, request: ReadRequest) -> Result<ReadResponse, GitHubError> {
        self.requests.borrow_mut().push(request);
        let value = self
            .replies
            .borrow_mut()
            .pop_front()
            .expect("unexpected request");
        let body = value.as_str().map_or_else(
            || serde_json::to_vec(&value).unwrap(),
            |s| s.as_bytes().to_vec(),
        );
        Ok(ReadResponse {
            status: 200,
            headers: BTreeMap::new(),
            body,
        })
    }
}
fn fixtures(head: &str) -> VecDeque<Value> {
    VecDeque::from([
        json!({"id":7,"head_sha":head,"status":"completed","conclusion":"failure","details_url":"https://github.com/org/repo/actions/runs/9/job/11","output":{"summary":"failed","text":null}}),
        json!({"id":11,"run_id":9,"head_sha":head,"check_run_url":"https://api.github.com/repos/org/repo/check-runs/7"}),
        json!("error: contains LLVM bitcode segment\n"),
    ])
}
#[test]
fn diagnostic_log_is_bound_to_check_job_and_exact_head() {
    let head = "b".repeat(40);
    let transport = Transport {
        replies: RefCell::new(fixtures(&head)),
        requests: RefCell::new(vec![]),
    };
    let reader = GitHubReader::new(transport, "https://api.github.com", "secret", 4096, 2).unwrap();
    let result = reader.read_check_failure("org", "repo", 7, &head).unwrap();
    assert_eq!(result["job_id"], 11);
    assert_eq!(
        result["log_excerpt"],
        "error: contains LLVM bitcode segment\n"
    );
    assert_eq!(result["log_truncated"], false);
    assert_eq!(result["log_sha256"].as_str().unwrap().len(), 64);
}
#[test]
fn stale_head_or_foreign_job_cannot_supply_diagnostics() {
    let head = "b".repeat(40);
    for variant in 0..6 {
        let mut replies = fixtures(&head);
        match variant {
            0 => replies[0]["head_sha"] = json!("c".repeat(40)),
            1 => replies[0]["id"] = json!(8),
            2 => replies[1]["head_sha"] = json!("c".repeat(40)),
            3 => {
                replies[1]["check_run_url"] =
                    json!("https://api.github.com/repos/org/repo/check-runs/8")
            }
            4 => replies[0]["conclusion"] = json!("success"),
            _ => replies[0]["status"] = json!("in_progress"),
        }
        let reader = GitHubReader::new(
            Transport {
                replies: RefCell::new(replies),
                requests: RefCell::new(vec![]),
            },
            "https://api.github.com",
            "secret",
            4096,
            2,
        )
        .unwrap();
        assert!(reader.read_check_failure("org", "repo", 7, &head).is_err());
    }
}

#[test]
fn non_actions_checks_keep_bounded_summary_without_following_external_urls() {
    let head = "b".repeat(40);
    let mut replies = fixtures(&head);
    replies.truncate(1);
    replies[0]["details_url"] = json!("https://evil.test/actions/runs/9/job/11");
    replies[0]["output"]["summary"] = json!("é".repeat(4000));
    let reader = GitHubReader::new(
        Transport {
            replies: RefCell::new(replies),
            requests: RefCell::new(vec![]),
        },
        "https://api.github.com",
        "secret",
        16_384,
        2,
    )
    .unwrap();
    let result = reader.read_check_failure("org", "repo", 7, &head).unwrap();
    assert_eq!(result["availability"], "summary_only");
    assert_eq!(result["summary"].as_str().unwrap().len(), 4096);
}

#[test]
fn logs_are_response_bounded_and_excerpted_at_utf8_boundaries() {
    let head = "b".repeat(40);
    for oversized in [false, true] {
        let mut replies = fixtures(&head);
        replies[2] = json!("é".repeat(10_000));
        let reader = GitHubReader::new(
            Transport {
                replies: RefCell::new(replies),
                requests: RefCell::new(vec![]),
            },
            "https://api.github.com",
            "secret",
            if oversized { 4096 } else { 30_000 },
            2,
        )
        .unwrap();
        let result = reader.read_check_failure("org", "repo", 7, &head);
        if oversized {
            let result = result.unwrap();
            assert_eq!(result["availability"], "summary_only");
            assert_eq!(result["summary"], "failed");
            assert!(result["log_excerpt"].is_null());
        } else {
            let result = result.unwrap();
            assert_eq!(result["log_truncated"], true);
            assert_eq!(result["log_bytes"], 20_000);
            assert!(result["log_excerpt"].as_str().unwrap().len() <= 16_384);
        }
    }
}

#[test]
#[ignore = "read-only live API probe; requires explicit PIP_CI_PROBE_* environment"]
fn live_exact_head_failure_log_probe() {
    let env = |name: &str| {
        std::env::var(format!("PIP_CI_PROBE_{name}"))
            .unwrap_or_else(|_| panic!("manual probe requires PIP_CI_PROBE_{name}"))
    };
    let reader = GitHubReader::new(
        pip_github::UreqTransport::new(std::time::Duration::from_secs(20)),
        "https://api.github.com",
        env("TOKEN"),
        4 * 1024 * 1024,
        2,
    )
    .unwrap();
    let result = reader
        .read_check_failure(
            &env("OWNER"),
            &env("REPO"),
            env("CHECK").parse().unwrap(),
            &env("HEAD"),
        )
        .unwrap();
    assert!(
        result["log_excerpt"]
            .as_str()
            .unwrap()
            .contains(&env("EXPECTED_TEXT"))
    );
    eprintln!(
        "Verified head={}, job={}, bytes={}, truncated={}",
        result["head_sha"], result["job_id"], result["log_bytes"], result["log_truncated"]
    );
}
