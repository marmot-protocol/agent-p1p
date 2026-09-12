//! Per-projection exact-head review copies, independent of mutable builder Git state.
use pip_executor::{GitCommand, GitRunner, ProcessGitRunner, sanitized_environment};
use serde_json::Value;
use std::fs::{self, File};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Validate every path from frozen task data against deterministic policy paths.
pub(crate) fn source_for(body: &Value, source: &str, key: &str) -> Result<String, String> {
    let Some(snapshot) = body.get("review_snapshot") else {
        return Ok(source.into());
    };
    if !matches!(
        body["role"].as_str(),
        Some("reviewer-general" | "reviewer-secperf" | "final-reviewer")
    ) {
        return Err("snapshot on non-review task".into());
    }
    let head = body["expected_head_sha"]
        .as_str()
        .ok_or("missing review head")?;
    if head.len() != 40
        || !head
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || *snapshot != pip_controller::review_snapshot(source, key, head)
    {
        return Err("review snapshot differs from the frozen projection".into());
    }
    Ok(format!(
        "{}/source",
        snapshot["root"].as_str().ok_or("missing snapshot root")?
    ))
}

pub(crate) fn prepare(body: &Value, workspace: &str, key: &str) -> Result<(), String> {
    let Some(snapshot) = body.get("review_snapshot") else {
        return Ok(());
    };
    let repo = body["repository_id"].as_u64().ok_or("missing repository")?;
    let issue = body["issue_number"].as_u64().ok_or("missing issue")?;
    let workflow = body["workflow_version"]
        .as_u64()
        .ok_or("missing workflow")?;
    let source = format!(
        "{}/repo-{repo}-issue-{issue}-workflow-{workflow}",
        workspace.trim_end_matches('/')
    );
    source_for(body, &source, key)?;
    materialize(snapshot)
}

pub(crate) fn direct_target(
    body: &Value,
    checkout: &Path,
    key: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(snapshot) = body.get("review_snapshot") else {
        return Ok(None);
    };
    let source = snapshot["source"].as_str().ok_or("missing source")?;
    if Path::new(&source_for(body, source, key)?) != checkout {
        return Err("foreign review checkout".into());
    }
    let build = checkout
        .parent()
        .ok_or("missing review root")?
        .join("build");
    if fs::symlink_metadata(&build)
        .map_err(error)?
        .file_type()
        .is_symlink()
        || build.canonicalize().map_err(error)? != build
        || !build.is_dir()
    {
        return Err("invalid review build directory".into());
    }
    Ok(Some(build.join("target")))
}

fn directory(path: &Path, owner: u32) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(error)?;
    if !meta.is_dir()
        || meta.uid() != owner
        || meta.mode() & 0o022 != 0
        || meta.mode() & 0o055 != 0o055
    {
        return Err("review directory is not controller-owned and protected".into());
    }
    Ok(())
}

fn managed_directory(path: &Path, owner: u32) -> Result<(), String> {
    if !path.try_exists().map_err(error)? {
        fs::DirBuilder::new()
            .mode(0o755)
            .create(path)
            .map_err(error)?;
        // The controller runs with umask 0077; the worker is a different UID.
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(error)?;
    }
    directory(path, owner)
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let mut environment = sanitized_environment();
    environment.insert("GIT_NO_REPLACE_OBJECTS".into(), "1".into());
    environment.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    environment.insert("GIT_CONFIG_COUNT".into(), "1".into());
    environment.insert("GIT_CONFIG_KEY_0".into(), "safe.directory".into());
    environment.insert(
        "GIT_CONFIG_VALUE_0".into(),
        path.to_string_lossy().into_owned(),
    );
    let output = ProcessGitRunner
        .run(&GitCommand {
            program: "git".into(),
            cwd: path.into(),
            args: args.iter().map(|s| (*s).into()).collect(),
            timeout: Duration::from_secs(60),
            max_output_bytes: 4 * 1024 * 1024,
            environment,
        })
        .map_err(error)?;
    if output.status != 0 || output.timed_out {
        return Err("review snapshot Git operation failed".into());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(error)
}

fn materialize(snapshot: &Value) -> Result<(), String> {
    let source = Path::new(snapshot["source"].as_str().ok_or("missing source")?);
    if source.canonicalize().map_err(error)? != source {
        return Err("noncanonical source".into());
    }
    let owner = fs::metadata(source).map_err(error)?.uid();
    let root = Path::new(snapshot["root"].as_str().ok_or("missing root")?);
    let parent = root.parent().ok_or("missing parent")?;
    let reviews = parent.parent().ok_or("missing reviews root")?;
    if reviews
        != source
            .parent()
            .ok_or("missing source parent")?
            .join(".reviews")
        || parent.file_name() != source.file_name()
    {
        return Err("foreign review root".into());
    }
    managed_directory(reviews, owner)?;
    let lock = File::open(reviews).map_err(error)?;
    lock.lock().map_err(error)?;
    managed_directory(parent, owner)?;
    let head = snapshot["head_sha"].as_str().ok_or("missing head")?;
    if root.try_exists().map_err(error)? {
        directory(root, owner)?;
        let marker = root.join("ownership.json");
        let meta = fs::symlink_metadata(&marker).map_err(error)?;
        if !meta.is_file()
            || meta.uid() != owner
            || meta.mode() & 0o077 != 0
            || meta.len() > 16384
            || serde_json::from_slice::<Value>(&fs::read(marker).map_err(error)?).map_err(error)?
                != *snapshot
        {
            return Err("unrecognized review snapshot".into());
        }
        verify(&root.join("source"), head, owner)?;
        return Ok(());
    }
    with_staging(parent, |staging| {
        let checkout = staging.join("source");
        fs::create_dir(&checkout).map_err(error)?;
        git(&checkout, &["init", "--quiet", "--template="])?;
        let reference = format!("+{head}:refs/pip/review-head");
        git(
            &checkout,
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--",
                source.to_str().ok_or("invalid source")?,
                &reference,
            ],
        )?;
        git(&checkout, &["checkout", "--quiet", "--detach", head])?;
        protect(&checkout, false)?;
        verify(&checkout, head, owner)?;
        let build = staging.join("build");
        fs::create_dir(&build).map_err(error)?;
        // Both service identities use pip-control as their shared group. Do not
        // request setgid: the controller deliberately has RestrictSUIDSGID=yes.
        fs::set_permissions(&build, fs::Permissions::from_mode(0o770)).map_err(error)?;
        let marker = staging.join("ownership.json");
        fs::write(&marker, serde_json::to_vec(snapshot).map_err(error)?).map_err(error)?;
        fs::set_permissions(marker, fs::Permissions::from_mode(0o600)).map_err(error)?;
        fs::set_permissions(staging, fs::Permissions::from_mode(0o755)).map_err(error)?;
        fs::rename(staging, root).map_err(error)?;
        Ok(())
    })
}

fn with_staging(
    parent: &Path,
    action: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let staging = tempfile::Builder::new()
        .prefix(".preparing-")
        .tempdir_in(parent)
        .map_err(error)?;
    let result = action(staging.path());
    // Published snapshots have been atomically renamed out of staging. On
    // failure, TempDir alone cannot remove a read-only tree as this UID.
    if staging.path().try_exists().map_err(error)? {
        protect(staging.path(), true)
            .map_err(|cleanup| format!("{result:?}; staging cleanup: {cleanup}"))?;
        staging
            .close()
            .map_err(|cleanup| format!("{result:?}; staging cleanup: {cleanup}"))?;
    }
    result
}

fn verify(path: &Path, head: &str, owner: u32) -> Result<(), String> {
    directory(path, owner)?;
    directory(&path.join(".git"), owner)?;
    if git(path, &["rev-parse", "HEAD"])? != head
        || !git(path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
        || path.join(".git/objects/info/alternates").exists()
        || !git(path, &["remote"])?.is_empty()
    {
        return Err("review snapshot is not clean and independent at its assigned head".into());
    }
    Ok(())
}

// Never follow repository symlinks while changing permissions or retiring.
fn protect(path: &Path, writable: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(error)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        if writable {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(error)?;
        }
        for entry in fs::read_dir(path).map_err(error)? {
            protect(&entry.map_err(error)?.path(), writable)?;
        }
    }
    let mode = if writable {
        if meta.is_dir() { 0o700 } else { 0o600 }
    } else if meta.is_dir() || meta.mode() & 0o111 != 0 {
        0o555
    } else {
        0o444
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(error)
}

/// Called only by the existing quiescent terminal-workspace retirement gate.
pub(crate) fn retire(source: &Path) -> Result<(), String> {
    let parent = source.parent().ok_or("missing case parent")?;
    let reviews = parent.join(".reviews");
    if !reviews.try_exists().map_err(error)? {
        return Ok(());
    }
    let owner = fs::metadata(parent).map_err(error)?.uid();
    directory(&reviews, owner)?;
    let lock = File::open(&reviews).map_err(error)?;
    lock.lock().map_err(error)?;
    let case_root = reviews.join(source.file_name().ok_or("missing case name")?);
    if !case_root.try_exists().map_err(error)? {
        return Ok(());
    }
    directory(&case_root, owner)?;
    let roots: Vec<PathBuf> = fs::read_dir(&case_root)
        .map_err(error)?
        .map(|entry| entry.map(|e| e.path()).map_err(error))
        .collect::<Result<_, _>>()?;
    for root in &roots {
        directory(root, owner)?;
        let marker = root.join("ownership.json");
        let meta = fs::symlink_metadata(&marker).map_err(error)?;
        if !meta.is_file() || meta.uid() != owner || meta.mode() & 0o077 != 0 || meta.len() > 16384
        {
            return Err("unrecognized review retirement marker".into());
        }
        let snapshot: Value =
            serde_json::from_slice(&fs::read(marker).map_err(error)?).map_err(error)?;
        let key = snapshot["projection_key"]
            .as_str()
            .ok_or("missing projection")?;
        let head = snapshot["head_sha"].as_str().ok_or("missing head")?;
        if snapshot
            != pip_controller::review_snapshot(source.to_str().ok_or("invalid source")?, key, head)
            || snapshot["root"].as_str() != root.to_str()
        {
            return Err("foreign review retirement".into());
        }
    }
    for root in roots {
        protect(&root, true)?;
        fs::remove_dir_all(root).map_err(error)?;
    }
    fs::remove_dir(case_root).map_err(error)
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn failed_read_only_snapshot_is_removed_without_following_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("snapshots");
        fs::create_dir(&parent).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, "keep").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o640)).unwrap();
        for _ in 0..3 {
            let result = with_staging(&parent, |staging| {
                let source = staging.join("source");
                fs::create_dir(&source).unwrap();
                fs::write(source.join("file"), "retained only until failure").unwrap();
                std::os::unix::fs::symlink(&outside, source.join("link")).unwrap();
                protect(&source, false).unwrap();
                Err("injected failure after protecting source".into())
            });
            assert!(result.unwrap_err().contains("injected failure"));
            assert_eq!(
                fs::read_dir(&parent).unwrap().count(),
                0,
                "failed attempts must not accumulate read-only snapshots"
            );
            assert_eq!(fs::metadata(&outside).unwrap().mode() & 0o777, 0o640);
            assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
        }
    }

    fn git_at(path: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn snapshots_survive_builder_changes_and_are_independent_read_only_copies() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("repo-1-issue-2-workflow-3");
        std::fs::create_dir(&source).unwrap();
        let source = source.canonicalize().unwrap();
        git_at(&source, &["init", "-q"]);
        std::fs::write(source.join("file"), "first").unwrap();
        git_at(&source, &["add", "file"]);
        git_at(
            &source,
            &["-c", "commit.gpgsign=false", "commit", "-qm", "first"],
        );
        let head = git_at(&source, &["rev-parse", "HEAD"]);
        let first =
            pip_controller::review_snapshot(source.to_str().unwrap(), "case:general", &head);
        let second =
            pip_controller::review_snapshot(source.to_str().unwrap(), "case:security", &head);
        materialize(&first).unwrap();
        materialize(&second).unwrap();
        let root = std::path::Path::new(first["root"].as_str().unwrap());
        let checkout = root.join("source");
        assert_eq!(
            std::fs::metadata(root.join("build")).unwrap().mode() & 0o7777,
            0o770,
            "controller sandbox forbids setgid; its primary group already owns build output"
        );
        for parent in [
            root.parent().unwrap(),
            root.parent().unwrap().parent().unwrap(),
        ] {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(parent).unwrap().mode() & 0o055,
                0o055,
                "managed review ancestors must be traversable under the controller's private umask"
            );
        }
        let body = serde_json::json!({"role":"reviewer-general","expected_head_sha":head,"review_snapshot":first});
        assert_eq!(
            direct_target(&body, &checkout, "case:general").unwrap(),
            Some(root.join("build/target"))
        );
        assert!(!checkout.join(".git/objects/info/alternates").exists());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(checkout.join("file")).unwrap().mode() & 0o222,
            0
        );
        assert_eq!(std::fs::metadata(root).unwrap().mode() & 0o022, 0);
        std::fs::write(source.join("file"), "second").unwrap();
        git_at(
            &source,
            &["-c", "commit.gpgsign=false", "commit", "-qam", "second"],
        );
        materialize(&first).unwrap();
        assert_eq!(git_at(&checkout, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(checkout.join("file")).unwrap(),
            "first"
        );
        assert_eq!(git_at(&checkout, &["remote"]), "");
        retire(&source).unwrap();
        assert!(!root.exists());
        assert!(source.exists());
    }

    #[test]
    fn snapshot_refuses_symlinks_and_unowned_existing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("case");
        std::fs::create_dir(&source).unwrap();
        let snapshot =
            pip_controller::review_snapshot(source.to_str().unwrap(), "key", &"a".repeat(40));
        std::os::unix::fs::symlink(temp.path(), temp.path().join(".reviews")).unwrap();
        assert!(materialize(&snapshot).is_err());
    }
}
