//! Why a parked case stopped, what to tell the human, and where it resumes.
use pip_core::CaseState;
use pip_store::{ParkingEvent, StoredCase};
use serde_json::Value;

use crate::publication_text::prose;

/// Where `resume` restarts a case, chosen from the reason it parked.
pub(crate) fn resume_target(park: &ParkingEvent) -> CaseState {
    let previous = park.previous_state.as_str();
    match park.event_type.as_str() {
        // Out of remediation rounds: the human granted more, so build again.
        "CI_FAILED" | "REQUEST_CHANGES" | "RETURN_TO_BUILD" => CaseState::Remediating,
        "RETURN_TO_REVIEW" => CaseState::WaitingCi,
        "RETURN_TO_PLANNING" | "WAIT_FOR_ISSUE_CREATOR" | "HUMAN_FEEDBACK_RECEIVED" => {
            CaseState::Planning
        }
        _ => match previous {
            "PLANNING" | "WAITING_HUMAN" => CaseState::Planning,
            "READY_TO_BUILD" | "BUILDING" => CaseState::ReadyToBuild,
            "REMEDIATING" => CaseState::Remediating,
            _ => CaseState::WaitingCi,
        },
    }
}

/// Whether the case parked because it used up its remediation rounds. These
/// events escalate only at the round limit; otherwise they hold or continue.
pub(crate) fn rounds_exhausted(park: &ParkingEvent, state: &str) -> bool {
    state == "ESCALATED"
        && matches!(
            park.event_type.as_str(),
            "CI_FAILED"
                | "REQUEST_CHANGES"
                | "RETURN_TO_BUILD"
                | "RETURN_TO_REVIEW"
                | "RETURN_TO_PLANNING"
                | "WAIT_FOR_ISSUE_CREATOR"
                | "HUMAN_FEEDBACK_RECEIVED"
        )
}

/// Human-readable stage names for comments.
pub(crate) fn stage(state: &str) -> &'static str {
    match state {
        "PLANNING" | "WAITING_HUMAN" => "planning",
        "READY_TO_BUILD" | "BUILDING" => "building",
        "REMEDIATING" => "fixing review or CI feedback",
        "WAITING_CI" => "waiting for CI",
        "REVIEWING" => "review",
        "FINAL_REVIEW" => "final review",
        "SHADOW_READY" | "READY_TO_MERGE" | "MERGING" => "readiness checks",
        _ => "an unknown stage",
    }
}

/// One or two sentences explaining what stopped the case.
pub(crate) fn explain(park: &ParkingEvent, case: &StoredCase) -> String {
    let payload = &park.payload;
    let details = &payload["details"];
    let text = |value: &Value| value.as_str().map(prose).unwrap_or_default();
    let number = |value: &Value| value.as_u64().unwrap_or_default();
    let error = text(&details["error"]);
    let with_error = |sentence: String| {
        if error.is_empty() {
            sentence
        } else {
            format!("{sentence} Last error: {error}")
        }
    };
    let worker = {
        let role = text(&details["role"]);
        if role.is_empty() {
            "worker".to_string()
        } else {
            role
        }
    };
    match park.event_type.as_str() {
        "OPERATIONAL_BOUND_REACHED" => match payload["bound"].as_str().unwrap_or_default() {
            "PROVIDER_FAILURES" => with_error(format!(
                "The {worker} step failed {} times in a row without producing a usable result.",
                number(&payload["observed"])
            )),
            "PROVIDER_UNAVAILABLE" => with_error(format!(
                "The worker provider has been unavailable for {} (limit {}).",
                duration(number(&payload["observed"])),
                duration(number(&payload["limit"]))
            )),
            "MODEL_UNAVAILABLE" => with_error(
                "The pinned model is not available to the provider account. Pip never substitutes another model.".into(),
            ),
            "WORKER_BLOCKED" => {
                format!("The {worker} stopped and reported: {error}")
            }
            "ELAPSED_TIME" => format!(
                "The case has been open for {} without finishing (limit {}).",
                duration(number(&payload["observed"])),
                duration(number(&payload["limit"]))
            ),
            "NO_PROGRESS" => {
                let pending = details["pending"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(|item| item.to_ascii_lowercase().replace('_', " "))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|items| !items.is_empty())
                    .map(|items| format!(" Waiting on: {}.", prose(&items)))
                    .unwrap_or_default();
                format!(
                    "Nothing has happened on this case for {} (limit {}).{pending}",
                    duration(number(&payload["observed"])),
                    duration(number(&payload["limit"]))
                )
            }
            "POLICY_CHANGED" => format!(
                "Pip's configuration changed (revision {} to {}) in a way this case did not accept, such as a different model or reviewer. Resuming continues under the new configuration.",
                number(&payload["limit"]),
                number(&payload["observed"])
            ),
            "REPEATED_FINDING_FINGERPRINT" => format!(
                "A reviewer raised the same finding on {} successive revisions; the builder has not resolved it.",
                number(&payload["observed"])
            ),
            other => format!("Pip reached an operational limit ({}).", prose(other)),
        },
        "CI_FAILED" | "REQUEST_CHANGES" | "RETURN_TO_BUILD" | "RETURN_TO_REVIEW"
        | "RETURN_TO_PLANNING" | "WAIT_FOR_ISSUE_CREATOR" | "HUMAN_FEEDBACK_RECEIVED"
            if rounds_exhausted(park, &case.state) =>
        {
            format!(
                "The case used all of its remediation rounds ({} so far) without reaching a ready PR.",
                case.remediation_round
            )
        }
        "WAIT_FOR_ISSUE_CREATOR" => {
            "The final reviewer needs input from the issue author before this can be ready.".into()
        }
        "WAITING_FOR_ISSUE_CREATOR" | "NEEDS_HUMAN_SCOPE_DECISION" | "ROOT_CAUSE_DIFFERENT_SCOPE" => {
            "The planner needs a human decision before work can continue; see its plan comment above.".into()
        }
        "CROSS_REPO_DEPENDENCY" => {
            "The planner found that the fix depends on another repository; see its plan comment above.".into()
        }
        "BLOCKED_UNEXPECTED_MODEL" => {
            "A worker reported that it was not running the pinned model.".into()
        }
        "BLOCKED" => "A worker reported that it could not do its job; see its result for the reason.".into(),
        other => format!("The case stopped after `{}`.", prose(other)),
    }
}

/// The comment Pip posts when it parks a case.
pub(crate) fn comment(park: &ParkingEvent, case: &StoredCase, login: Option<&str>) -> String {
    let head = case
        .head_sha
        .as_deref()
        .map(|head| format!(" on `{head}`"))
        .unwrap_or_default();
    let commands = login.map_or_else(
        || {
            "mention Pip's GitHub account followed by `resume`, `replan` or `abandon`. \
             Lines after the command are passed on as guidance."
                .to_string()
        },
        |login| {
            format!(
                "reply to this issue with one of:\n\n- `@{login} resume` to pick up where it stopped (add any guidance on the following lines)\n- `@{login} replan` to start again from planning with your guidance\n- `@{login} abandon` to stop working on this issue"
            )
        },
    );
    format!(
        "## Pip paused this case\n\n{reason}\n\nPip stopped during {stage}{head}. To continue, {commands}\n\nOnly maintainers trusted to authorize Pip can do this.",
        reason = explain(park, case),
        stage = stage(&park.previous_state),
    )
}

fn duration(seconds: u64) -> String {
    match seconds {
        0..120 => format!("{seconds} seconds"),
        120..7_200 => format!("{} minutes", seconds / 60),
        7_200..172_800 => format!("{} hours", seconds / 3_600),
        _ => format!("{} days", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn park(event_type: &str, previous: &str, payload: Value) -> ParkingEvent {
        ParkingEvent {
            event_type: event_type.into(),
            previous_state: previous.into(),
            state_revision: 9,
            observed_at: 1_000,
            payload,
        }
    }

    fn case() -> StoredCase {
        StoredCase {
            case_key: "repo:1#2@3".into(),
            repository_id: 1,
            issue_number: 2,
            workflow_version: 3,
            state: "ESCALATED".into(),
            state_revision: 9,
            policy_revision: 1,
            remediation_round: 10,
            plan_version: 1,
            pr_number: Some(77),
            head_sha: Some("b".repeat(40)),
        }
    }

    #[test]
    fn resume_restarts_where_the_cause_points() {
        for (event, previous, expected) in [
            ("CI_FAILED", "WAITING_CI", CaseState::Remediating),
            ("REQUEST_CHANGES", "REVIEWING", CaseState::Remediating),
            ("RETURN_TO_REVIEW", "FINAL_REVIEW", CaseState::WaitingCi),
            (
                "NEEDS_HUMAN_SCOPE_DECISION",
                "PLANNING",
                CaseState::Planning,
            ),
            ("OPERATIONAL_BOUND_REACHED", "PLANNING", CaseState::Planning),
            (
                "OPERATIONAL_BOUND_REACHED",
                "BUILDING",
                CaseState::ReadyToBuild,
            ),
            (
                "OPERATIONAL_BOUND_REACHED",
                "REMEDIATING",
                CaseState::Remediating,
            ),
            (
                "OPERATIONAL_BOUND_REACHED",
                "REVIEWING",
                CaseState::WaitingCi,
            ),
            (
                "OPERATIONAL_BOUND_REACHED",
                "FINAL_REVIEW",
                CaseState::WaitingCi,
            ),
            ("BLOCKED", "REMEDIATING", CaseState::Remediating),
        ] {
            assert_eq!(
                resume_target(&park(event, previous, json!({}))),
                expected,
                "{event} from {previous}"
            );
        }
    }

    #[test]
    fn comments_say_what_stopped_and_how_to_continue() {
        let failures = park(
            "OPERATIONAL_BOUND_REACHED",
            "REVIEWING",
            json!({"bound":"PROVIDER_FAILURES","observed":3,"limit":3,
                   "details":{"role":"reviewer-general","error":"unknown field `x` <script>"}}),
        );
        let body = comment(&failures, &case(), Some("agent-p1p"));
        assert!(
            body.contains("The reviewer-general step failed 3 times"),
            "{body}"
        );
        assert!(
            body.contains("unknown field \\`x\\` &lt;script&gt;"),
            "{body}"
        );
        assert!(body.contains("during review on `bbbb"), "{body}");
        assert!(body.contains("`@agent-p1p resume`"), "{body}");
        assert!(body.contains("`@agent-p1p abandon`"), "{body}");

        let outage = park(
            "OPERATIONAL_BOUND_REACHED",
            "BUILDING",
            json!({"bound":"PROVIDER_UNAVAILABLE","observed":21_700,"limit":21_600,
                   "details":{"error":"The provided API key is invalid."}}),
        );
        assert!(explain(&outage, &case()).contains("unavailable for 6 hours"));

        let rounds = park("CI_FAILED", "WAITING_CI", json!({}));
        assert!(rounds_exhausted(&rounds, "ESCALATED"));
        assert!(!rounds_exhausted(
            &park("WAIT_FOR_ISSUE_CREATOR", "FINAL_REVIEW", json!({})),
            "WAITING_HUMAN"
        ));
        assert!(explain(&rounds, &case()).contains("used all of its remediation rounds"));
        assert!(comment(&rounds, &case(), None).contains("mention Pip's GitHub account"));
    }
}
