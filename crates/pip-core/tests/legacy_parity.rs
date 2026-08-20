use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFixture {
    fixture_format: u32,
    cases: Vec<LegacyCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCase {
    name: String,
    #[serde(rename = "from")]
    from_state: String,
    outcome: String,
    remediation_round: Option<u32>,
    max_rounds: Option<u32>,
    #[serde(rename = "to")]
    to_state: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetFixture {
    fixture_format: u32,
    cases: Vec<TargetCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetCase {
    name: String,
    #[serde(rename = "from")]
    from_state: String,
    event: String,
    remediation_round: Option<u32>,
    max_rounds: Option<u32>,
    merge_mode: Option<String>,
    #[serde(rename = "to")]
    to_state: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParityFixture {
    fixture_format: u32,
    comparisons: Vec<Comparison>,
    target_only: Vec<TargetOnly>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Classification {
    Equivalent,
    IntentionalDifference,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparison {
    legacy_name: String,
    target_name: String,
    classification: Classification,
    rationale: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetOnly {
    target_name: String,
    rationale: String,
}

#[test]
fn every_frozen_python_transition_is_evaluated_or_intentionally_superseded() {
    let legacy: LegacyFixture = serde_json::from_str(include_str!(
        "../../../migration/legacy-v1/transitions.json"
    ))
    .unwrap();
    let target: TargetFixture = serde_json::from_str(include_str!(
        "../../../migration/target-v1/transitions.json"
    ))
    .unwrap();
    let parity: ParityFixture = serde_json::from_str(include_str!(
        "../../../migration/parity-v1/transitions.json"
    ))
    .unwrap();
    assert_eq!(
        (
            legacy.fixture_format,
            target.fixture_format,
            parity.fixture_format
        ),
        (1, 1, 1)
    );

    let legacy = unique_by_name(legacy.cases, |case| &case.name);
    let target = unique_by_name(target.cases, |case| &case.name);
    let mut compared_legacy = BTreeSet::new();
    let mut compared_target = BTreeSet::new();
    for comparison in parity.comparisons {
        assert!(!comparison.rationale.trim().is_empty());
        assert!(compared_legacy.insert(comparison.legacy_name.clone()));
        assert!(compared_target.insert(comparison.target_name.clone()));
        let old = legacy.get(&comparison.legacy_name).unwrap();
        let new = target.get(&comparison.target_name).unwrap();
        let same = old.from_state == new.from_state
            && old.outcome == new.event
            && old.remediation_round == new.remediation_round
            && old.max_rounds == new.max_rounds
            && old.to_state == new.to_state
            && new.merge_mode.as_deref().unwrap_or("shadow") == "shadow";
        match comparison.classification {
            Classification::Equivalent => assert!(same, "{}", comparison.legacy_name),
            Classification::IntentionalDifference => {
                assert!(!same, "{}", comparison.legacy_name)
            }
        }
    }

    let mut target_only = BTreeSet::new();
    for entry in parity.target_only {
        assert!(!entry.rationale.trim().is_empty());
        assert!(target.contains_key(&entry.target_name));
        assert!(target_only.insert(entry.target_name));
    }
    assert_eq!(compared_legacy, legacy.keys().cloned().collect());
    let accounted_target: BTreeSet<String> = compared_target.union(&target_only).cloned().collect();
    assert_eq!(accounted_target, target.keys().cloned().collect());
}

fn unique_by_name<T>(values: Vec<T>, name: impl Fn(&T) -> &String) -> BTreeMap<String, T> {
    let mut result = BTreeMap::new();
    for value in values {
        let key = name(&value).clone();
        assert!(result.insert(key, value).is_none());
    }
    result
}
