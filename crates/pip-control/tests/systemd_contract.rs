#[test]
fn installed_shadow_unit_is_hardened_credential_bound_and_never_dispatches() {
    let service = include_str!("../../../packaging/systemd/pip-v2-shadow-reconcile.service");
    let timer = include_str!("../../../packaging/systemd/pip-v2-shadow-reconcile.timer");
    assert!(service.contains("User=pip-v2-control"));
    assert!(service.contains("LoadCredential=github.token:/etc/pip-v2/github.token"));
    assert!(service.contains("shadow-reconcile"));
    assert!(service.contains("ProtectSystem=strict"));
    assert!(service.contains("NoNewPrivileges=yes"));
    assert!(service.contains("CapabilityBoundingSet=\n"));
    let command = service
        .lines()
        .find(|line| line.starts_with("ExecStart="))
        .unwrap();
    assert!(command.contains("shadow-reconcile"));
    assert!(!command.contains("route-consumer"));
    assert!(!command.contains(" merge"));
    assert!(timer.contains("OnUnitActiveSec=5min"));
}
