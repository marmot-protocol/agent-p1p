use std::{fs, path::Path, process::Command};

#[test]
fn product_namespace_has_no_obsolete_version_suffix() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tracked = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(tracked.status.success());
    let obsolete = [
        ["pip", "-v2"],
        ["Pip", " v2"],
        ["PIP", "_V2"],
        ["pip", "_v2"],
        ["pip", "/v2/"],
    ]
    .map(|parts| parts.concat());
    let offenders = tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            let relative = std::str::from_utf8(path).unwrap();
            let path = root.join(relative);
            if !path.is_file() {
                return None;
            }
            let bytes = fs::read(path).unwrap();
            obsolete
                .iter()
                .any(|token| {
                    bytes
                        .windows(token.len())
                        .any(|part| part == token.as_bytes())
                })
                .then_some(relative.to_owned())
        })
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "obsolete product names in {offenders:?}"
    );
}
