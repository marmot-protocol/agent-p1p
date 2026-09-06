//! Explicit disposable build storage. Retirement is offline-only; results stay.
use crate::RepositoryPolicy;
use pip_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub fn verify_scratch_runtime_stopped<R: pip_hermes::CommandRunner>(
    runner: &R,
) -> Result<(), String> {
    let patterns = [
        "pip-hermes-gateway.service",
        "pip-controller@*.service",
        "pip-controller@*.timer",
        "pip-direct-worker@*.service",
        "pip-direct-worker@*.timer",
        "pip-webhook-consumer@*.service",
        "pip-webhook-consumer@*.timer",
        "pip-shadow-reconcile.service",
        "pip-shadow-reconcile.timer",
    ];
    for (command, filter) in [
        (
            "list-units",
            "--state=activating,active,deactivating,reloading",
        ),
        // Older systemd returns status 1 for an empty state-filtered listing.
        // Inspect the actual unit states instead of accepting a command failure.
        ("list-unit-files", "--full"),
    ] {
        let mut args = vec![
            command.into(),
            filter.into(),
            "--no-legend".into(),
            "--no-pager".into(),
            "--plain".into(),
        ];
        args.extend(patterns.iter().map(|pattern| (*pattern).into()));
        let output = runner
            .run(&pip_hermes::CommandSpec {
                program: "systemctl".into(),
                args,
                timeout: std::time::Duration::from_secs(10),
                max_output_bytes: 65536,
            })
            .map_err(failure)?;
        let quiescent = if command == "list-unit-files" {
            std::str::from_utf8(&output.stdout).is_ok_and(|listing| {
                listing
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .all(|line| {
                        let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
                        matches!(fields.len(), 2 | 3)
                            && matches!(
                                fields[1],
                                "disabled" | "static" | "masked" | "masked-runtime"
                            )
                    })
            })
        } else {
            output.stdout.iter().all(u8::is_ascii_whitespace)
        };
        if output.status != 0
            || output.timed_out
            || output.stdout.len() > 65536
            || output.stderr.len() > 65536
            || !quiescent
        {
            return Err("execution units are active, enabled, or uninspectable".into());
        }
    }
    let output = runner
        .run(&pip_hermes::CommandSpec {
            program: "systemctl".into(),
            args: vec![
                "show".into(),
                "pip-hermes-gateway.service".into(),
                "--property=MainPID".into(),
                "--value".into(),
            ],
            timeout: std::time::Duration::from_secs(10),
            max_output_bytes: 1024,
        })
        .map_err(failure)?;
    if output.status != 0 || output.timed_out || output.stdout != b"0\n" {
        return Err("Hermes execution process is not stopped".into());
    }
    Ok(())
}

fn failure(message: impl ToString) -> String {
    message.to_string()
}

fn real_directory(path: &Path, owner: u32) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(failure)?;
    if !meta.is_dir()
        || meta.uid() != owner
        || meta.mode() & 0o077 != 0
        || path.canonicalize().map_err(failure)? != path
    {
        return Err("scratch directory is not canonical, private and controller-owned".into());
    }
    Ok(())
}

fn bindings(
    policy: &RepositoryPolicy,
    store: &Store,
    body: &Value,
) -> Result<(PathBuf, Value, u32), String> {
    let configured = Path::new(
        policy
            .hermes_scratch_root
            .as_deref()
            .ok_or("missing scratch policy")?,
    );
    let owner = fs::metadata(store.path()).map_err(failure)?.uid();
    real_directory(configured, owner)?;
    let workspace = fs::metadata(&policy.workspace).map_err(failure)?;
    let scratch = fs::metadata(configured).map_err(failure)?;
    if workspace.dev() != scratch.dev()
        || (policy.workspace_storage.require_distinct_filesystem
            && scratch.dev() == fs::metadata(store.path()).map_err(failure)?.dev())
    {
        return Err("scratch is not on the policy workspace filesystem".into());
    }
    let case_key = body["case_key"].as_str().ok_or("missing case key")?;
    let case = store
        .case(case_key)
        .map_err(failure)?
        .ok_or("missing scratch case")?;
    if case.repository_id != policy.repository.id {
        return Err("foreign scratch repository".into());
    }
    let key = body["projection_key"]
        .as_str()
        .ok_or("missing projection key")?;
    if !key.starts_with(&format!("{case_key}:")) {
        return Err("foreign scratch projection".into());
    }
    let hash: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let root = configured.join(hash);
    let source = format!(
        "{}/repo-{}-issue-{}-workflow-{}",
        policy.workspace, case.repository_id, case.issue_number, case.workflow_version
    );
    let expected = json!({"schema_version":1,"root":root,"source":source,
        "cargo_target":root.join("disposable/target"),"cargo_home":root.join("disposable/cargo-home"),
        "temporary":root.join("disposable/tmp"),"results":root.join("results")});
    if body["storage"] != expected {
        return Err("scratch paths differ from the frozen projection".into());
    }
    Ok((
        root,
        json!({"schema_version":1,"case_key":case_key,"projection_key":key,"storage":expected}),
        owner,
    ))
}

fn read_marker(path: &Path, owner: u32) -> Result<Value, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(failure)?;
    let meta = file.metadata().map_err(failure)?;
    if !meta.is_file() || meta.uid() != owner || meta.mode() & 0o077 != 0 || meta.len() > 16384 {
        return Err("unsafe scratch marker".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() > 16384 {
        return Err("oversized scratch marker".into());
    }
    serde_json::from_slice(&bytes).map_err(failure)
}

fn write_marker(path: &Path, value: &Value) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(failure)?;
    file.write_all(&serde_json::to_vec(value).map_err(failure)?)
        .map_err(failure)?;
    file.sync_all().map_err(failure)
}

pub fn prepare_hermes_scratch(
    policy: &RepositoryPolicy,
    store: &Store,
    body: &Value,
) -> Result<(), String> {
    let (root, marker, owner) = bindings(policy, store, body)?;
    crate::workspace_lifecycle::ensure_workspace_storage_ready(policy, store.path())
        .map_err(failure)?;
    let marker_path = root.join(".pip-scratch.json");
    if root.try_exists().map_err(failure)? {
        real_directory(&root, owner)?;
        if read_marker(&marker_path, owner)? != marker {
            return Err("unrecognized scratch ownership".into());
        }
        if root
            .join(".pip-retired.json")
            .try_exists()
            .map_err(failure)?
        {
            return Err("retired scratch cannot be reused".into());
        }
    } else {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(failure)?;
        write_marker(&marker_path, &marker)?;
    }
    for child in [
        "disposable",
        "disposable/target",
        "disposable/cargo-home",
        "disposable/tmp",
        "results",
    ] {
        let path = root.join(child);
        if !path.try_exists().map_err(failure)? {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .map_err(failure)?;
        }
        real_directory(&path, owner)?;
    }
    Ok(())
}

// Reject nested mounts before any deletion. Symlinks are removed, never followed.
fn inspect_disposable(path: &Path, device: u64, count: &mut usize) -> Result<(), String> {
    *count += 1;
    if *count > 1_000_000 {
        return Err("scratch retirement inspection bound exceeded".into());
    }
    let meta = fs::symlink_metadata(path).map_err(failure)?;
    if meta.dev() != device {
        return Err("nested filesystem in disposable scratch".into());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(failure)? {
            inspect_disposable(&entry.map_err(failure)?.path(), device, count)?;
        }
    }
    Ok(())
}

pub fn retire_hermes_scratch(
    policy: &RepositoryPolicy,
    store: &Store,
    body: &Value,
    now: u64,
    runtime_stopped: bool,
) -> Result<(), String> {
    if !runtime_stopped {
        return Err("scratch retirement requires a stopped execution runtime".into());
    }
    let (root, marker, owner) = bindings(policy, store, body)?;
    let case = store
        .case(body["case_key"].as_str().ok_or("missing case")?)
        .map_err(failure)?
        .ok_or("missing case")?;
    let last_observed = store
        .immutable_history_for_case(&case.case_key)
        .map_err(failure)?
        .events
        .iter()
        .map(|event| event.observed_at)
        .max()
        .ok_or("missing case history")?;
    if !matches!(
        case.state.as_str(),
        "COMPLETED" | "ABANDONED" | "TAKEN_OVER"
    ) || now < last_observed.saturating_add(policy.workspace_storage.terminal_retention_seconds)
    {
        return Err("scratch case is active or retained".into());
    }
    real_directory(&root, owner)?;
    if read_marker(&root.join(".pip-scratch.json"), owner)? != marker {
        return Err("unrecognized scratch ownership".into());
    }
    let disposable = root.join("disposable");
    if disposable.try_exists().map_err(failure)? {
        real_directory(&disposable, owner)?;
        inspect_disposable(
            &disposable,
            fs::metadata(&root).map_err(failure)?.dev(),
            &mut 0,
        )?;
        fs::remove_dir_all(&disposable).map_err(failure)?;
    }
    let receipt = root.join(".pip-retired.json");
    if !receipt.try_exists().map_err(failure)? {
        write_marker(
            &receipt,
            &json!({"schema_version":1,"case_key":case.case_key,"retired_at":now,"results_retained":true}),
        )?;
    }
    Ok(())
}
