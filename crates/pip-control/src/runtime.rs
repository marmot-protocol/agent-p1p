//! Policy-to-Hermes runtime bootstrap mapping.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use pip_hermes::{
    BootstrapError, BootstrapOutcome, CommandRunner, HermesBootstrap, ProfileBootstrapSpec,
    RuntimeBootstrapSpec,
};

use crate::RepositoryPolicy;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeBootstrapError {
    InvalidPolicy,
    Bootstrap(BootstrapError),
}

impl fmt::Display for RuntimeBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy => formatter.write_str("repository has no Hermes-native roles"),
            Self::Bootstrap(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RuntimeBootstrapError {}

impl From<BootstrapError> for RuntimeBootstrapError {
    fn from(error: BootstrapError) -> Self {
        Self::Bootstrap(error)
    }
}

pub fn bootstrap_hermes_runtime_with<R: CommandRunner + Clone>(
    policy: &RepositoryPolicy,
    root: &Path,
    skills_root: &Path,
    auth_source: &Path,
    hermes_program: &str,
    runner: R,
) -> Result<BootstrapOutcome, RuntimeBootstrapError> {
    let mut profiles = policy
        .roles
        .iter()
        .filter(|role| role.is_hermes())
        .map(
            |role| -> Result<ProfileBootstrapSpec, RuntimeBootstrapError> {
                Ok(ProfileBootstrapSpec {
                    name: role.profile.clone(),
                    provider: role.provider.clone(),
                    model: role.model.clone(),
                    reasoning_effort: role
                        .reasoning_effort
                        .clone()
                        .ok_or(RuntimeBootstrapError::InvalidPolicy)?,
                    skills: role.skills.clone(),
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    if profiles.is_empty() {
        return Err(RuntimeBootstrapError::InvalidPolicy);
    }
    if policy.conversations_enabled {
        let planner = policy
            .roles
            .iter()
            .find(|role| role.role == pip_contracts::WorkerRole::Planner && role.is_hermes())
            .ok_or(RuntimeBootstrapError::InvalidPolicy)?;
        profiles.push(ProfileBootstrapSpec {
            name: "conversation".into(),
            provider: planner.provider.clone(),
            model: planner.model.clone(),
            reasoning_effort: planner
                .reasoning_effort
                .clone()
                .ok_or(RuntimeBootstrapError::InvalidPolicy)?,
            skills: vec!["conversation".into(), "workflow-contract".into()],
        });
    }
    let repository = policy.repository.full_name();
    HermesBootstrap::new(
        runner,
        hermes_program,
        Duration::from_secs(20),
        4 * 1024 * 1024,
    )?
    .apply(&RuntimeBootstrapSpec {
        root: root.into(),
        skills_root: skills_root.into(),
        auth_source: auth_source.into(),
        board: policy.board.clone(),
        board_name: format!("Pip - {repository}"),
        board_description: format!("Pip controlled shadow workflow for {repository}"),
        profiles,
    })
    .map_err(Into::into)
}
