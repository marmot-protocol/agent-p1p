from __future__ import annotations

from enum import Enum


class CaseState(str, Enum):
    PLANNING = "PLANNING"
    WAITING_HUMAN = "WAITING_HUMAN"
    BUILDING = "BUILDING"
    REVIEWING = "REVIEWING"
    FINAL_REVIEW = "FINAL_REVIEW"
    SHADOW_READY = "SHADOW_READY"
    COMPLETED = "COMPLETED"
    BLOCKED = "BLOCKED"
    ESCALATED = "ESCALATED"
    ABANDONED = "ABANDONED"


class TransitionError(ValueError):
    """A result cannot advance the case from its current state."""


_STATIC_TRANSITIONS: dict[tuple[CaseState, str], CaseState] = {
    (CaseState.PLANNING, "PROCEED"): CaseState.BUILDING,
    (CaseState.PLANNING, "WAITING_FOR_ISSUE_CREATOR"): CaseState.WAITING_HUMAN,
    (CaseState.PLANNING, "NEEDS_HUMAN_SCOPE_DECISION"): CaseState.WAITING_HUMAN,
    (CaseState.PLANNING, "ROOT_CAUSE_DIFFERENT_SCOPE"): CaseState.WAITING_HUMAN,
    (CaseState.PLANNING, "CROSS_REPO_DEPENDENCY"): CaseState.BLOCKED,
    (CaseState.PLANNING, "ALREADY_FIXED"): CaseState.COMPLETED,
    (CaseState.PLANNING, "NOT_REPRODUCIBLE"): CaseState.COMPLETED,
    (CaseState.PLANNING, "DUPLICATE"): CaseState.COMPLETED,
    (CaseState.PLANNING, "ABANDON"): CaseState.ABANDONED,
    (CaseState.PLANNING, "HUMAN_NARROWED_SCOPE"): CaseState.PLANNING,
    (CaseState.PLANNING, "HUMAN_APPROVED_STALE_BASE"): CaseState.PLANNING,
    (CaseState.WAITING_HUMAN, "HUMAN_CLARIFIED"): CaseState.PLANNING,
    (CaseState.BUILDING, "REVIEW_READY"): CaseState.REVIEWING,
    (CaseState.BUILDING, "RETURN_TO_PLANNING"): CaseState.PLANNING,
    (CaseState.BUILDING, "BLOCKED"): CaseState.BLOCKED,
    (CaseState.BUILDING, "HUMAN_REAFFIRMED_SCOPE"): CaseState.BUILDING,
    (CaseState.BUILDING, "ABANDON"): CaseState.ABANDONED,
    (CaseState.REVIEWING, "REVIEWS_APPROVED"): CaseState.FINAL_REVIEW,
    (CaseState.REVIEWING, "BLOCKED"): CaseState.BLOCKED,
    (CaseState.FINAL_REVIEW, "HUMAN_REVIEW_REQUIRED"): CaseState.SHADOW_READY,
    (CaseState.FINAL_REVIEW, "RETURN_TO_REVIEW"): CaseState.REVIEWING,
    (CaseState.FINAL_REVIEW, "RETURN_TO_PLANNING"): CaseState.PLANNING,
    (CaseState.FINAL_REVIEW, "WAIT_FOR_ISSUE_CREATOR"): CaseState.WAITING_HUMAN,
    (CaseState.FINAL_REVIEW, "BLOCKED"): CaseState.BLOCKED,
    (CaseState.FINAL_REVIEW, "ABANDON"): CaseState.ABANDONED,
    (CaseState.SHADOW_READY, "HUMAN_MERGED"): CaseState.COMPLETED,
    (CaseState.SHADOW_READY, "BLOCKED"): CaseState.BLOCKED,
    (CaseState.BLOCKED, "PREREQUISITE_RESOLVED"): CaseState.PLANNING,
}


def transition(
    state: CaseState,
    outcome: str,
    *,
    remediation_round: int = 0,
    max_rounds: int = 3,
) -> CaseState:
    if type(remediation_round) is not int or remediation_round < 0:
        raise TransitionError("remediation round must be a non-negative integer")
    if type(max_rounds) is not int or max_rounds < 1:
        raise TransitionError("max rounds must be a positive integer")
    if state in {
        CaseState.COMPLETED,
        CaseState.ESCALATED,
        CaseState.ABANDONED,
    }:
        raise TransitionError(f"terminal state cannot transition: {state.value}")
    if outcome == "BLOCKED_UNEXPECTED_MODEL":
        return CaseState.BLOCKED
    if state is CaseState.REVIEWING and outcome == "REQUEST_CHANGES":
        return (
            CaseState.ESCALATED
            if remediation_round >= max_rounds
            else CaseState.BUILDING
        )
    final_returns = {
        "RETURN_TO_BUILD": CaseState.BUILDING,
        "RETURN_TO_REVIEW": CaseState.REVIEWING,
        "RETURN_TO_PLANNING": CaseState.PLANNING,
        "WAIT_FOR_ISSUE_CREATOR": CaseState.WAITING_HUMAN,
    }
    if state is CaseState.FINAL_REVIEW and outcome in final_returns:
        return (
            CaseState.ESCALATED
            if remediation_round >= max_rounds
            else final_returns[outcome]
        )
    try:
        return _STATIC_TRANSITIONS[(state, outcome)]
    except KeyError as exc:
        raise TransitionError(f"invalid transition: {state.value} + {outcome}") from exc
