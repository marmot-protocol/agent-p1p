use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentifierError {
    kind: &'static str,
    value: String,
}

impl IdentifierError {
    fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for IdentifierError {}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepositorySlug(String);

impl FromStr for RepositorySlug {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository = parts.next().unwrap_or_default();
        if !valid_slug(owner) || !valid_slug(repository) || parts.next().is_some() {
            return Err(IdentifierError::new("repository slug", value));
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for RepositorySlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

macro_rules! nonzero_id {
    ($name:ident, $inner:ty, $primitive:ty) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($inner);

        impl $name {
            #[must_use]
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> $primitive {
                self.0.get()
            }
        }
    };
}

nonzero_id!(IssueNumber, NonZeroU64, u64);
nonzero_id!(RepositoryId, NonZeroU64, u64);
nonzero_id!(ActorId, NonZeroU64, u64);
nonzero_id!(PullRequestNumber, NonZeroU64, u64);
nonzero_id!(WorkflowVersion, NonZeroU32, u32);
nonzero_id!(PolicyRevision, NonZeroU64, u64);
nonzero_id!(StateRevision, NonZeroU64, u64);
nonzero_id!(PlanVersion, NonZeroU32, u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CaseId {
    repository: RepositoryId,
    issue: IssueNumber,
    workflow: WorkflowVersion,
}

impl CaseId {
    #[must_use]
    pub const fn new(
        repository: RepositoryId,
        issue: IssueNumber,
        workflow: WorkflowVersion,
    ) -> Self {
        Self {
            repository,
            issue,
            workflow,
        }
    }

    #[must_use]
    pub const fn repository(&self) -> &RepositoryId {
        &self.repository
    }

    #[must_use]
    pub const fn issue(&self) -> IssueNumber {
        self.issue
    }

    #[must_use]
    pub const fn workflow(&self) -> WorkflowVersion {
        self.workflow
    }
}

impl fmt::Display for CaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "repo:{}#{}@{}",
            self.repository.get(),
            self.issue.get(),
            self.workflow.get()
        )
    }
}

macro_rules! textual_id {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl FromStr for $name {
            type Err = IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if !valid_slug(value) {
                    return Err(IdentifierError::new($kind, value));
                }
                Ok(Self(value.to_owned()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

textual_id!(RunId, "run ID");
textual_id!(FindingId, "finding ID");
textual_id!(EventId, "event ID");

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObservedAt(u64);

impl ObservedAt {
    #[must_use]
    pub const fn new(unix_seconds: u64) -> Self {
        Self(unix_seconds)
    }

    #[must_use]
    pub const fn unix_seconds(self) -> u64 {
        self.0
    }
}

impl StateRevision {
    #[must_use]
    pub fn checked_next(self) -> Option<Self> {
        self.get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self::new)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GitSha([u8; 40]);

impl FromStr for GitSha {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 40
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(IdentifierError::new("Git SHA", value));
        }
        let mut bytes = [0; 40];
        bytes.copy_from_slice(value.as_bytes());
        Ok(Self(bytes))
    }
}

impl fmt::Display for GitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = std::str::from_utf8(&self.0).expect("validated SHA is ASCII");
        formatter.write_str(value)
    }
}
