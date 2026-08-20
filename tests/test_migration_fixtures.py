from __future__ import annotations

import copy
import json
import tomllib
from pathlib import Path

import pytest

from pip_agent.contracts import (
    ContractError,
    assert_exact_head_evidence,
    validate_contract,
)
from pip_agent.state_machine import CaseState, transition


ROOT = Path(__file__).parents[1]
FIXTURES = ROOT / "migration" / "legacy-v1"


def _json(name: str) -> dict[str, object]:
    return json.loads((FIXTURES / name).read_text())


def test_happy_path_fixture_validates_all_contracts_and_exact_head_join() -> None:
    fixture = _json("happy-path.json")
    results = fixture["results"]
    assert isinstance(results, dict)
    for key, contract in {
        "planner": "planner-result",
        "builder": "builder-result",
        "general_review": "review-result",
        "secperf_review": "review-result",
        "final_review": "final-result",
    }.items():
        payload = results[key]
        assert isinstance(payload, dict)
        validate_contract(contract, payload)

    join = fixture["join"]
    assert isinstance(join, dict)
    evidence = join["evidence"]
    assert isinstance(evidence, dict)
    evidence = {
        **evidence,
        "builder_result": results["builder"],
        "general_review": results["general_review"],
        "secperf_review": results["secperf_review"],
    }
    assert_exact_head_evidence(
        str(join["case_id"]),
        int(join["pr_number"]),
        int(join["plan_version"]),
        str(join["head_sha"]),
        evidence,
    )


def test_invalid_contract_recipes_fail_for_the_documented_reason() -> None:
    happy = _json("happy-path.json")
    results = happy["results"]
    recipes = _json("invalid-contracts.json")["cases"]
    assert isinstance(results, dict) and isinstance(recipes, list)
    for recipe in recipes:
        assert isinstance(recipe, dict)
        payload = copy.deepcopy(results[recipe["source"]])
        assert isinstance(payload, dict)
        for key, value in recipe.get("set", {}).items():
            payload[key] = value
        for key in recipe.get("remove", []):
            payload.pop(key, None)
        with pytest.raises(ContractError, match=str(recipe["error_contains"])):
            validate_contract(str(recipe["contract"]), payload)


def test_transition_fixture_is_a_language_neutral_state_machine_oracle() -> None:
    cases = _json("transitions.json")["cases"]
    assert isinstance(cases, list)
    for case in cases:
        assert isinstance(case, dict)
        observed = transition(
            CaseState(str(case["from"])),
            str(case["outcome"]),
            remediation_round=int(case.get("remediation_round", 0)),
            max_rounds=int(case.get("max_rounds", 3)),
        )
        assert observed.value == case["to"], case["name"]


def test_classification_accounts_for_every_legacy_test_module() -> None:
    classification = tomllib.loads((FIXTURES / "test-classification.toml").read_text())
    modules = classification["module"]
    assert isinstance(modules, list)
    by_path = {item["path"]: item for item in modules}
    expected = {
        str(path.relative_to(ROOT))
        for path in (ROOT / "tests").glob("test_*.py")
        if path.name != "test_migration_fixtures.py"
    }
    assert set(by_path) == expected
    assert all(
        item["disposition"] in {"port", "replace", "legacy-only", "split"}
        and isinstance(item["reason"], str)
        and item["reason"]
        for item in modules
    )
