#[test]
fn monotonic_timers_ignore_legacy_persistent_stamps() {
    for timer in [
        include_str!("../../../packaging/systemd/pip-shadow-reconcile.timer"),
        include_str!("../../../packaging/systemd/pip-controller@.timer"),
        include_str!("../../../packaging/systemd/pip-direct-worker@.timer"),
        include_str!("../../../packaging/systemd/pip-webhook-consumer@.timer"),
    ] {
        assert!(timer.contains("Persistent=false"));
        assert!(!timer.lines().any(|line| line == "Persistent=true"));
    }
}

#[test]
fn installed_shadow_unit_is_hardened_credential_bound_and_never_dispatches() {
    let service = include_str!("../../../packaging/systemd/pip-shadow-reconcile.service");
    let timer = include_str!("../../../packaging/systemd/pip-shadow-reconcile.timer");
    assert!(service.contains("User=pip-control"));
    assert!(service.contains("LoadCredential=github.token:/etc/pip/github.token"));
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
    assert!(timer.contains("OnUnitInactiveSec=5min"));
    assert!(!timer.contains("OnUnitActiveSec="));
}

#[test]
fn staged_active_controller_template_uses_a_dedicated_shared_hermes_root() {
    let service = include_str!("../../../packaging/systemd/pip-controller@.service");
    let timer = include_str!("../../../packaging/systemd/pip-controller@.timer");

    assert!(service.contains("User=pip-control"));
    assert!(service.contains("RequiresMountsFor=/var/lib/pip/worktrees"));
    assert!(service.contains("ConditionPathIsMountPoint=/var/lib/pip/worktrees"));
    assert!(service.contains("Environment=HERMES_HOME=/var/lib/pip/hermes"));
    assert!(service.contains("Environment=HERMES_KANBAN_HOME=/var/lib/pip/hermes"));
    assert!(service.contains("controller-cycle"));
    assert!(service.contains("--policy /etc/pip/repositories/%i.json"));
    assert!(service.contains("--skills-commit-file /opt/pip/current/SOURCE.COMMIT"));
    assert!(service.contains("--direct-queue /var/lib/pip/direct-queue"));
    assert!(service.contains("--git-askpass /opt/pip/current/bin/pip-control"));
    for (name, source) in [
        ("github.token", "pip-github-token"),
        ("github-reviewer-general.app", "pip-reviewer-general-app"),
        ("github-reviewer-general.pem", "pip-reviewer-general-key"),
        ("github-reviewer-secperf.app", "pip-reviewer-secperf-app"),
        ("github-reviewer-secperf.pem", "pip-reviewer-secperf-key"),
    ] {
        assert!(
            service
                .lines()
                .any(|line| line == format!("LoadCredential={name}:{source}"))
        );
    }
    assert!(
        !service
            .lines()
            .any(|line| line.starts_with("LoadCredential=") && line.contains(":/"))
    );
    assert!(service.contains("--github-reviewer-general-app %d/github-reviewer-general.app"));
    assert!(service.contains("--github-reviewer-general-key %d/github-reviewer-general.pem"));
    assert!(service.contains("--github-reviewer-secperf-app %d/github-reviewer-secperf.app"));
    assert!(service.contains("--github-reviewer-secperf-key %d/github-reviewer-secperf.pem"));
    assert!(!service.contains("github-reviewer-general.token"));
    assert!(!service.contains("github-reviewer-secperf.token"));
    assert!(!service.contains("/home/jeff"));
    assert!(timer.contains("OnUnitInactiveSec=15s"));
    assert!(!timer.contains("OnUnitActiveSec="));
}

#[test]
fn signing_credentials_are_optional_startup_capabilities_only_for_the_controller() {
    let service = include_str!("../../../packaging/systemd/pip-controller@.service");
    for name in ["pip-commit-signing", "pip-commit-signing-identity"] {
        assert!(
            service
                .lines()
                .any(|line| line == format!("LoadCredential={name}"))
        );
        assert!(
            !include_str!("../../../packaging/systemd/pip-direct-worker@.service").contains(name)
        );
        assert!(
            !include_str!("../../../packaging/systemd/pip-hermes-gateway.service").contains(name)
        );
    }
    assert!(service.contains("--commit-signing-identity %d/pip-commit-signing-identity"));
    assert!(service.contains("--commit-signing-key %d/pip-commit-signing"));
}

#[test]
fn direct_worker_template_has_provider_state_but_no_controller_credentials() {
    let service = include_str!("../../../packaging/systemd/pip-direct-worker@.service");
    let timer = include_str!("../../../packaging/systemd/pip-direct-worker@.timer");

    assert!(service.contains("User=pip-worker"));
    assert!(service.contains("Group=pip-control"));
    assert!(service.contains("UMask=0007"));
    assert!(service.contains("MemoryDenyWriteExecute=no"));
    assert!(service.contains("NoNewPrivileges=yes"));
    assert!(service.contains("ProtectSystem=strict"));
    assert!(service.contains("RestrictNamespaces=yes"));
    assert!(service.contains("RequiresMountsFor=/var/lib/pip/worktrees"));
    assert!(service.contains("ConditionPathIsMountPoint=/var/lib/pip/worktrees"));
    assert!(service.contains("Environment=HOME=/var/lib/pip/provider-home"));
    assert!(service.contains("direct-worker-cycle"));
    assert!(service.contains("--direct-queue /var/lib/pip/direct-queue"));
    assert!(!service.contains("--database"));
    assert!(service.contains("--cursor cursor-agent"));
    assert!(service.contains("--skills-root /opt/pip/current/share/pip/skills"));
    assert!(!service.contains("LoadCredential="));
    assert!(!service.contains("github.token"));
    assert!(service.contains("InaccessiblePaths=/var/lib/pip/ledger.db"));
    assert!(!service.contains("--hermes"));
    assert!(service.contains("/var/lib/pip/hermes"));
    assert!(timer.contains("OnUnitInactiveSec=15s"));
    assert!(!timer.contains("OnUnitActiveSec="));
    assert!(timer.contains("AccuracySec=1s"));
}

#[test]
fn hermes_gateway_owns_dispatch_without_controller_credentials_or_ledger_access() {
    let service = include_str!("../../../packaging/systemd/pip-hermes-gateway.service");

    assert!(service.contains("User=pip-control"));
    assert!(service.contains("PrivateUsers=yes"));
    assert!(service.contains("TemporaryFileSystem=/var/lib/pip:ro"));
    assert!(service.contains("MemoryDenyWriteExecute=no"));
    assert!(service.contains("NoNewPrivileges=yes"));
    assert!(service.contains("ProtectSystem=strict"));
    assert!(service.contains("RequiresMountsFor=/var/lib/pip/worktrees"));
    assert!(service.contains("ConditionPathIsMountPoint=/var/lib/pip/worktrees"));
    assert!(service.contains("Environment=HERMES_HOME=/var/lib/pip/hermes"));
    assert!(service.contains("Environment=HERMES_KANBAN_HOME=/var/lib/pip/hermes"));
    assert!(service.contains("ExecStart=/usr/local/bin/hermes gateway run --external-supervisor"));
    assert!(!service.contains("--no-supervise"));
    assert!(
        service.contains("BindPaths=/var/lib/pip/hermes /var/lib/pip/worktrees/hermes-scratch")
    );
    assert!(
        include_str!("../../../scripts/install-rust-control-plane.sh").contains(
            "ensure_directory /var/lib/pip/worktrees/hermes-scratch pip-control pip-control 700"
        )
    );
    assert!(service.contains("BindReadOnlyPaths=/var/lib/pip/repositories /var/lib/pip/worktrees"));
    assert!(!service.contains("LoadCredential="));
    assert!(!service.contains("github.token"));
}

#[test]
fn webhook_ingress_is_loopback_only_token_free_and_ledger_blind() {
    let service = include_str!("../../../packaging/systemd/pip-webhook-ingress.service");

    assert!(service.contains("User=pip-ingress"));
    assert!(service.contains("Group=pip-ingress"));
    assert!(
        service.contains("LoadCredential=github-webhook.secret:/etc/pip/github-webhook.secret")
    );
    assert!(service.contains("webhook-serve --listen 127.0.0.1:8787"));
    assert!(service.contains("--spool /var/spool/pip-webhooks"));
    assert!(service.contains("ProtectSystem=strict"));
    assert!(service.contains("IPAddressDeny=any"));
    assert!(service.contains("IPAddressAllow=localhost"));
    assert!(service.contains("ReadWritePaths=/var/spool/pip-webhooks"));
    assert!(!service.contains(
        "ReadWritePaths=/var/spool/pip-webhooks/receipts /var/spool/pip-webhooks/pending"
    ));
    assert!(service.contains("InaccessiblePaths=/var/lib/pip"));
    assert!(!service.contains("github.token"));
    assert!(!service.contains("ledger.db"));
    assert!(!service.contains("HERMES_HOME"));
}

#[test]
fn webhook_consumer_is_controller_owned_bounded_and_credential_scoped() {
    let service = include_str!("../../../packaging/systemd/pip-webhook-consumer@.service");
    let timer = include_str!("../../../packaging/systemd/pip-webhook-consumer@.timer");

    assert!(service.contains("User=pip-control"));
    assert!(service.contains("Group=pip-control"));
    assert!(service.contains("LoadCredential=github.token:/etc/pip/github.token"));
    assert!(
        service.contains("LoadCredential=github-webhook.secret:/etc/pip/github-webhook.secret")
    );
    assert!(service.contains("webhook-spool-cycle"));
    assert!(service.contains("--policy /etc/pip/repositories/%i.json"));
    assert!(service.contains("--database /var/lib/pip/ledger.db"));
    assert!(service.contains("--spool /var/spool/pip-webhooks"));
    assert!(service.contains("ProtectSystem=strict"));
    assert!(service.contains("ReadWritePaths=/var/lib/pip /var/spool/pip-webhooks"));
    assert!(!service.contains(
        "ReadWritePaths=/var/lib/pip /var/spool/pip-webhooks/pending /var/spool/pip-webhooks/processed"
    ));
    assert!(service.contains(
        "InaccessiblePaths=/var/lib/pip/hermes /var/lib/pip/repositories /var/lib/pip/worktrees /var/lib/pip/artifacts /var/lib/pip/provider-home /var/lib/pip/direct-queue"
    ));
    assert!(!service.contains("github-reviewer-general"));
    assert!(!service.contains("github-reviewer-secperf"));
    assert!(!service.contains("HERMES_HOME"));
    assert!(timer.contains("OnUnitInactiveSec=5s"));
    assert!(!timer.contains("OnUnitActiveSec="));
}
