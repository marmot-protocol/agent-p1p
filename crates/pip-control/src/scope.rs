//! Explicit selection for one case cycle, not policy or stored workflow state.
use crate::RepositoryPolicy;
use pip_store::{ClaimedEffect, Store, StoreError, StoredCase};

pub(crate) fn execution_policy(
    policy: &RepositoryPolicy,
    store: &Store,
    case: &StoredCase,
) -> RepositoryPolicy {
    if policy.revision == case.policy_revision {
        return policy.clone();
    }
    store
        .accepted_policy(case.repository_id, case.policy_revision)
        .ok()
        .and_then(|value| serde_json::to_vec(&value).ok())
        .and_then(|bytes| crate::load_repository_policy(&bytes).ok())
        .and_then(|accepted| policy.execution_policy_for(&accepted))
        // Keep the revision mismatch fence on missing/incompatible authority.
        .unwrap_or_else(|| policy.clone())
}

#[derive(Clone, Copy)]
pub struct RepositoryScope<'a> {
    pub(crate) policy: &'a RepositoryPolicy,
    pub(crate) case_key: Option<&'a str>,
}

impl<'a> From<&'a RepositoryPolicy> for RepositoryScope<'a> {
    fn from(policy: &'a RepositoryPolicy) -> Self {
        Self {
            policy,
            case_key: None,
        }
    }
}

impl<'a> RepositoryScope<'a> {
    /// Scope selection only. The caller must still check authorization, ownership
    /// and bounds; the scope never grants permission to advance a workflow.
    #[must_use]
    pub fn case(policy: &'a RepositoryPolicy, case_key: &'a str) -> Self {
        Self {
            policy,
            case_key: Some(case_key),
        }
    }

    pub(crate) fn matches(self, case: &StoredCase) -> bool {
        case.repository_id == self.policy.repository.id
            && self.case_key.is_none_or(|key| key == case.case_key)
    }

    pub(crate) fn claim(
        self,
        store: &mut Store,
        owner: &str,
        now: u64,
        lease_seconds: u64,
        types: &[&str],
    ) -> Result<Option<ClaimedEffect>, StoreError> {
        store.claim_repository_case_effect_matching(
            self.policy.repository.id,
            self.case_key,
            owner,
            now,
            lease_seconds,
            types,
        )
    }
}
