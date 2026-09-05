#![cfg(unix)]

use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

#[test]
fn shared_rust_proxy_pins_installation_but_preserves_repo_and_cache_selection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("rust");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/pip-rust-tool");
    let script = std::fs::read_to_string(source).expect("packaged Rust proxy exists");
    let wrapper = temp.path().join("pip-rust-tool");
    std::fs::write(
        &wrapper,
        script.replace("/opt/pip/rust", root.to_str().unwrap()),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    for tool in [
        "cargo",
        "rustc",
        "rustfmt",
        "cargo-fmt",
        "clippy-driver",
        "cargo-clippy",
        "rustdoc",
        "rustup",
    ] {
        let fake = root.join("bin").join(tool);
        std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$RUSTUP_HOME\" \"$RUSTUP_AUTO_INSTALL\" \"$CARGO_HOME\" \"${CARGO_TARGET_DIR-unset}\" \"${RUSTUP_TOOLCHAIN-unset}\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let proxy = temp.path().join(tool);
        symlink(&wrapper, &proxy).unwrap();
        let output = Command::new(proxy)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("RUSTUP_HOME", "/untrusted")
            .env("RUSTUP_AUTO_INSTALL", "1")
            .env("CARGO_HOME", "/worker/cache")
            .args(["+1.97.1", "argument with spaces"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}/rustup\n0\n/worker/cache\nunset\nunset\n+1.97.1\nargument with spaces\n",
                root.display()
            )
        );
    }
    let bad = Command::new(&wrapper).output().unwrap();
    assert!(
        !bad.status.success(),
        "unknown invocation must not select a tool"
    );
}
