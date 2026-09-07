//! Root-managed public signing identity; the private key never enters Rust memory.

use pip_executor::{CommitSigningIdentity, PublicationError};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

pub(crate) fn load_identity(
    path: &Path,
    actor: u64,
) -> Result<CommitSigningIdentity, PublicationError> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| PublicationError::Filesystem(error.to_string()))?;
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| PublicationError::Filesystem(error.to_string()))?;
    // Older systemd versions chown the read-only credential copy to the
    // service UID; newer versions retain root ownership and grant read access.
    // A service-owned ordinary file is not an operator-owned signing identity.
    let readonly_credential = std::env::var_os("CREDENTIALS_DIRECTORY").is_some_and(|directory| {
        Path::new(&directory).is_absolute() && path.parent() == Some(Path::new(&directory))
    }) && rustix::fs::fstatvfs(&file)
        .is_ok_and(|stat| stat.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY));
    if !metadata.is_file()
        || metadata.nlink() != 1
        || !trusted_identity_permissions(
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o7777,
            rustix::process::geteuid().as_raw(),
            readonly_credential,
        )
        || metadata.len() == 0
        || metadata.len() > 4096
    {
        return Err(PublicationError::InvalidConfiguration);
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| PublicationError::Filesystem(error.to_string()))?;
    if bytes.len() > 4096 {
        return Err(PublicationError::InvalidConfiguration);
    }
    parse_identity(&bytes, actor)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SigningIdentity {
    schema_version: u32,
    actor_id: u64,
    name: String,
    email: String,
    public_key: String,
}

fn trusted_identity_permissions(
    uid: u32,
    gid: u32,
    mode: u32,
    service_uid: u32,
    readonly_credential: bool,
) -> bool {
    gid == 0
        && ((uid == 0 && matches!(mode, 0o400 | 0o440 | 0o600))
            || (uid == service_uid && mode == 0o400 && readonly_credential))
}

fn parse_identity(bytes: &[u8], actor: u64) -> Result<CommitSigningIdentity, PublicationError> {
    let value: SigningIdentity =
        serde_json::from_slice(bytes).map_err(|_| PublicationError::InvalidConfiguration)?;
    if value.schema_version != 1 || actor == 0 || value.actor_id != actor {
        return Err(PublicationError::InvalidConfiguration);
    }
    Ok(CommitSigningIdentity {
        name: value.name,
        email: value.email,
        public_key: value.public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_root_files_or_readonly_service_credential_copies_are_trusted() {
        assert!(trusted_identity_permissions(0, 0, 0o600, 997, false));
        assert!(trusted_identity_permissions(0, 0, 0o440, 997, true));
        assert!(trusted_identity_permissions(997, 0, 0o400, 997, true));
        assert!(!trusted_identity_permissions(997, 0, 0o400, 997, false));
        assert!(!trusted_identity_permissions(998, 0, 0o400, 997, true));
        assert!(!trusted_identity_permissions(997, 997, 0o400, 997, true));
        assert!(!trusted_identity_permissions(997, 0, 0o600, 997, true));
        assert!(!trusted_identity_permissions(0, 0, 0o644, 997, true));
    }

    #[test]
    #[ignore = "requires the disposable systemd signing fixture"]
    fn service_signing_credential_boundary() {
        use pip_core::{CaseId, IssueNumber, RepositoryId, WorkflowVersion};
        use pip_executor::{
            GitPublicationSpec, IsolatedWorkspace, ProcessGitRunner, WorktreeSpec, sign_commit,
        };
        use std::fs;
        use std::num::{NonZeroU32, NonZeroU64};
        use std::path::PathBuf;
        use std::process::Command;
        use std::time::Duration;
        assert!(!rustix::process::geteuid().is_root());
        let root = PathBuf::from(std::env::var("PIP_SIGNING_FIXTURE_ROOT").unwrap());
        let credentials = PathBuf::from(std::env::var("CREDENTIALS_DIRECTORY").unwrap());
        let identity_file = credentials.join("pip-signing-fixture-identity");
        let identity = load_identity(&identity_file, 42).unwrap();
        assert!(load_identity(&identity_file, 43).is_err());
        assert_eq!(identity.name, "Fixture Bot");
        let git = |path: &Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(path)
                .args(args)
                .env_clear()
                .envs(pip_executor::workspace_git_environment(
                    path,
                    pip_executor::sanitized_environment(),
                ))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let repo = root.join("repository");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(root.join("worktrees")).unwrap();
        git(
            &repo,
            &["init", "--quiet", "--template=", "--initial-branch=main"],
        );
        let commit = [
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ];
        git(&repo, &commit);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        let case = CaseId::new(
            RepositoryId::new(NonZeroU64::new(42).unwrap()),
            IssueNumber::new(NonZeroU64::new(1).unwrap()),
            WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
        );
        let spec = WorktreeSpec::new(
            &repo,
            root.join("worktrees"),
            "pip/",
            case,
            base.parse().unwrap(),
        )
        .unwrap();
        let allocator = IsolatedWorkspace::new(
            ProcessGitRunner,
            "git",
            Duration::from_secs(30),
            1024 * 1024,
        )
        .unwrap();
        allocator
            .allocate(&spec, "https://github.com/example/fixture.git")
            .unwrap();
        fs::write(spec.path().join("implementation"), "accepted build\n").unwrap();
        git(spec.path(), &["add", "implementation"]);
        git(spec.path(), &commit);
        let source = git(spec.path(), &["rev-parse", "HEAD"]);
        let tree = git(spec.path(), &["rev-parse", "HEAD^{tree}"]);
        let publication = GitPublicationSpec::new_scoped(
            spec.root(),
            spec.path(),
            "origin",
            "https://github.com/example/fixture.git",
            spec.branch(),
            source.parse().unwrap(),
            None,
        )
        .unwrap();
        let signed = sign_commit(
            ProcessGitRunner,
            "git",
            &publication,
            base.parse().unwrap(),
            &identity,
            &credentials.join("pip-signing-fixture-key"),
        )
        .unwrap();
        assert_ne!(signed.head.to_string(), source);
        assert_eq!(signed.source_head.to_string(), source);
        assert_eq!(signed.tree.to_string(), tree);
        assert_eq!(signed.parent.to_string(), base);
        assert_eq!(git(spec.path(), &["rev-parse", "HEAD"]), source);
        println!("CONTROLLER_SIGNING_CREDENTIAL_SANDBOX_OK");
    }

    #[test]
    fn signing_identity_is_explicit_versioned_and_bound_to_the_automation_actor() {
        let value = json!({"schema_version":1, "actor_id":42, "name":"Fixture Bot",
            "email":"42+fixture@users.noreply.github.com", "public_key":"ssh-ed25519 fixture"});
        let identity = parse_identity(&serde_json::to_vec(&value).unwrap(), 42).unwrap();
        assert_eq!(identity.name, "Fixture Bot");
        assert_eq!(identity.email, "42+fixture@users.noreply.github.com");
        assert_eq!(identity.public_key, "ssh-ed25519 fixture");
        for (field, replacement) in [
            ("schema_version", json!(2)),
            ("actor_id", json!(0)),
            ("actor_id", json!(43)),
            ("extra", json!(true)),
        ] {
            let mut invalid = value.clone();
            invalid[field] = replacement;
            assert!(parse_identity(&serde_json::to_vec(&invalid).unwrap(), 42).is_err());
        }
        assert!(parse_identity(&serde_json::to_vec(&value).unwrap(), 0).is_err());
    }

    #[test]
    fn missing_or_user_owned_signing_identity_is_not_trusted() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_identity(&temp.path().join("absent"), 42).is_err());
        if !rustix::process::geteuid().is_root() {
            let path = temp.path().join("identity.json");
            std::fs::write(&path, br#"{"schema_version":1,"actor_id":42,"name":"Fixture","email":"fixture@example.invalid","public_key":"ssh-ed25519 fixture"}"#).unwrap();
            assert!(load_identity(&path, 42).is_err());
        }
    }
}
