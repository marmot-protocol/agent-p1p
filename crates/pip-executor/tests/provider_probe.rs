use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use pip_executor::{
    BoundedProcessRunner, CursorHealthProbe, HealthAssurance, ProcessError, ProcessOutput,
    ProcessRunner, ProcessSpec, ProviderProbeError,
};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<ProcessOutput, ProcessError>>>>,
    commands: Rc<RefCell<Vec<ProcessSpec>>>,
}

impl FakeRunner {
    fn push(&self, status: i32, stdout: &str) {
        self.outputs.borrow_mut().push_back(Ok(ProcessOutput {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            timed_out: false,
        }));
    }
}

impl ProcessRunner for FakeRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
        self.commands.borrow_mut().push(spec.clone());
        self.outputs.borrow_mut().pop_front().unwrap()
    }
}

fn probe(runner: FakeRunner) -> CursorHealthProbe<FakeRunner> {
    CursorHealthProbe::new(
        runner,
        "/opt/pip/bin/agent",
        PathBuf::from("/opt/pip/probe"),
        BTreeMap::from([
            ("HOME".into(), "/var/lib/pip-provider".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]),
        Duration::from_secs(5),
        16_384,
    )
    .unwrap()
}

#[test]
fn exact_advertised_model_and_authenticated_cli_are_ready() {
    let runner = FakeRunner::default();
    runner.push(0, "2026.01.08-fixture\n");
    runner.push(0, "Commands:\n  models  List available models\n  status\n");
    runner.push(0, "Authenticated as fixture@example.invalid\n");
    runner.push(0, "composer-2.5\nclaude-opus-4-8-thinking-high\n");

    let health = probe(runner.clone()).probe("composer-2.5").unwrap();
    assert_eq!(health.provider, "cursor");
    assert_eq!(health.model, "composer-2.5");
    assert_eq!(health.version, "2026.01.08-fixture");
    assert_eq!(health.assurance, HealthAssurance::AdvertisedExact);
    assert_eq!(
        runner
            .commands
            .borrow()
            .iter()
            .map(|command| command.args.clone())
            .collect::<Vec<_>>(),
        [
            vec!["--version"],
            vec!["--help"],
            vec!["status"],
            vec!["models"],
        ]
    );
}

#[test]
fn incompatible_cli_stops_before_auth_or_model_commands() {
    let runner = FakeRunner::default();
    runner.push(0, "old-version\n");
    runner.push(0, "Commands:\n  status\n");
    assert_eq!(
        probe(runner.clone()).probe("composer-2.5"),
        Err(ProviderProbeError::IncompatibleCli)
    );
    assert_eq!(runner.commands.borrow().len(), 2);
}

#[test]
fn authentication_and_model_availability_fail_closed_without_substitution() {
    let runner = FakeRunner::default();
    runner.push(0, "version\n");
    runner.push(0, "models\nstatus\n");
    runner.push(0, "Not authenticated\n");
    assert_eq!(
        probe(runner).probe("composer-2.5"),
        Err(ProviderProbeError::Unauthenticated)
    );

    let runner = FakeRunner::default();
    runner.push(0, "version\n");
    runner.push(0, "models\nstatus\n");
    runner.push(0, "Authenticated\n");
    runner.push(0, "auto\nclaude-opus\n");
    assert_eq!(
        probe(runner).probe("composer-2.5"),
        Err(ProviderProbeError::ModelNotAdvertised)
    );

    let runner = FakeRunner::default();
    runner.push(0, "version\n");
    runner.push(0, "models\nstatus\n");
    runner.push(0, "Authenticated\n");
    runner.push(0, "composer-2.5\ncomposer-2.5\n");
    assert_eq!(
        probe(runner).probe("composer-2.5"),
        Err(ProviderProbeError::AmbiguousModel)
    );
}

#[test]
fn bounded_runner_enforces_timeout_output_limit_and_explicit_environment() {
    let runner = BoundedProcessRunner;
    let started = Instant::now();
    let timeout = runner
        .run(&ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30 & wait".into()],
            cwd: PathBuf::from("/"),
            environment: BTreeMap::new(),
            timeout: Duration::from_millis(50),
            max_output_bytes: 1024,
        })
        .unwrap();
    assert!(timeout.timed_out);
    assert!(started.elapsed() < Duration::from_secs(3));

    let oversized = runner
        .run(&ProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf %02048d 0".into()],
            cwd: PathBuf::from("/"),
            environment: BTreeMap::new(),
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        })
        .unwrap();
    assert_eq!(oversized.stdout.len(), 1025);

    let environment = runner
        .run(&ProcessSpec {
            program: "/usr/bin/env".into(),
            args: Vec::new(),
            cwd: PathBuf::from("/"),
            environment: BTreeMap::from([("ONLY_SAFE_FIXTURE".into(), "present".into())]),
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        })
        .unwrap();
    assert_eq!(environment.stdout, b"ONLY_SAFE_FIXTURE=present\n");
}
