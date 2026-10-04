#![cfg(feature = "dev-tools")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_STATE: AtomicUsize = AtomicUsize::new(0);

struct TestState(PathBuf);

impl TestState {
    fn new() -> Self {
        let sequence = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ekmp-cli-test-{}-{sequence}", std::process::id()));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path.join("state.json"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        if let Some(directory) = self.0.parent() {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

fn ekmp(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ekmp"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run ekmp")
}

struct ServiceProcess(std::process::Child);

impl ServiceProcess {
    fn child_mut(&mut self) -> &mut std::process::Child {
        &mut self.0
    }

    fn stop(&mut self) {
        if self
            .0
            .try_wait()
            .expect("inspect refresh service")
            .is_none()
        {
            self.0.kill().expect("terminate refresh service");
        }
        self.0.wait().expect("reap refresh service");
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn service_process(state: &TestState) -> ServiceProcess {
    let mut arguments = scenario_args(state);
    arguments.extend([
        "service".into(),
        "run".into(),
        "--interval".into(),
        "1h".into(),
    ]);
    ServiceProcess(
        Command::new(env!("CARGO_BIN_EXE_ekmp"))
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start refresh service"),
    )
}

fn assert_service_is_running(child: &mut std::process::Child) {
    for _ in 0..20 {
        if let Some(status) = child.try_wait().expect("inspect refresh service") {
            let mut diagnostics = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                stderr
                    .read_to_string(&mut diagnostics)
                    .expect("read service stderr");
            }
            panic!("refresh service ended early with {status}: {diagnostics}");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn scenario_args(state: &TestState) -> Vec<String> {
    scenario_args_for(state, "mixed")
}

fn scenario_args_for(state: &TestState, scenario: &str) -> Vec<String> {
    vec![
        "--scenario".into(),
        scenario.into(),
        "--dev-state".into(),
        state.path().display().to_string(),
    ]
}

fn run_scenario(state: &TestState, args: &[&str]) -> Output {
    run_named_scenario(state, "mixed", args)
}

fn run_named_scenario(state: &TestState, scenario: &str, args: &[&str]) -> Output {
    let mut complete = scenario_args_for(state, scenario);
    complete.extend(args.iter().map(|argument| (*argument).into()));
    let borrowed: Vec<_> = complete.iter().map(String::as_str).collect();
    ekmp(&borrowed)
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    stdout(&output)
}

#[test]
fn help_and_invalid_noninteractive_post_have_expected_exit_behavior() {
    let help = ekmp(&[]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout(&help).contains("Usage:"));

    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));
    let post = run_scenario(&state, &["post", "9001"]);
    assert_eq!(post.status.code(), Some(1));
    assert!(stderr(&post).contains("confirmation requires a terminal"));
}

#[test]
fn simulator_cli_workflow_uses_json_and_never_exposes_hashes() {
    let state = TestState::new();

    let refresh = success(run_scenario(&state, &["--json", "refresh"]));
    assert!(refresh.contains("fetched_killmails"));

    let characters = success(run_scenario(&state, &["--json", "characters", "list"]));
    assert!(characters.contains("Simulated Alpha"));

    let hidden = success(run_scenario(&state, &["--json", "list"]));
    assert!(hidden.contains("9001"));
    assert!(!hidden.contains("9002"));
    assert!(!hidden.contains("synthetic-hash"));

    let protected = success(run_scenario(
        &state,
        &["--json", "--show-protected", "show", "9002"],
    ));
    assert!(protected.contains("9002"));
    assert!(protected.contains("\"protected\":true"));
    assert!(!protected.contains("synthetic-hash"));

    success(run_scenario(
        &state,
        &["--json", "protect", "add", "character", "Lookup Character"],
    ));
    let protection = success(run_scenario(&state, &["--json", "protect", "list"]));
    assert!(protection.contains("Lookup Character"));

    success(run_scenario(
        &state,
        &["--json", "config", "set", "refresh-interval", "10m"],
    ));
    let config = success(run_scenario(
        &state,
        &["--json", "config", "get", "refresh-interval"],
    ));
    assert!(config.contains("600"));
}

#[test]
fn protected_post_requires_override_and_bulk_excludes_it() {
    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));

    let denied = run_scenario(&state, &["post", "9002", "--yes"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(stderr(&denied).contains("protected"));

    let individual = success(run_scenario(
        &state,
        &["--json", "post", "9002", "--post-anyway", "--yes"],
    ));
    assert!(individual.contains("9002"));

    let bulk = success(run_scenario(&state, &["--json", "post", "--all", "--yes"]));
    assert!(bulk.contains("9001"));
    assert!(!bulk.contains("9003"));
    assert!(!bulk.contains("9004"));
}

#[test]
fn json_post_errors_do_not_expose_the_killmail_hash() {
    let state = TestState::new();
    success(run_named_scenario(&state, "errors", &["refresh"]));
    // Model the unchanged upstream query cache across an uncertain submission.
    let mut before: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    let observed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(1);
    before["zkill_pages"]["kills:1101:1"] =
        serde_json::json!({"entries":[],"observed_at":observed_at,"valid_until":observed_at+3600});
    fs::write(state.path(), serde_json::to_vec(&before).unwrap()).unwrap();

    let output = run_named_scenario(&state, "errors", &["--json", "post", "9101", "--yes"]);
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(!combined.contains("synthetic-error-hash"));
    assert!(!combined.contains("killmail/add"));
    assert_eq!(output.status.code(), Some(1));
    let failed: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    let attempt = failed["zkill_status"]["9101"].clone();
    assert_eq!(attempt["state"], "post_attempted", "durable attempt marker");
    let retried = run_named_scenario(&state, "errors", &["--json", "post", "9101", "--yes"]);
    assert_eq!(retried.status.code(), Some(1));
    assert!(stderr(&retried).contains("could not be confirmed"));
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    assert_eq!(after["zkill_status"]["9101"], attempt);
}

#[test]
fn service_lifetime_lock_rejects_duplicates_and_is_released_after_a_crash() {
    let state = TestState::new();
    let mut first = service_process(&state);
    assert_service_is_running(first.child_mut());

    let duplicate = run_scenario(&state, &["service", "run", "--interval", "1h"]);
    assert_eq!(
        duplicate.status.code(),
        Some(3),
        "stderr: {}",
        stderr(&duplicate)
    );

    first.stop();

    let mut replacement = service_process(&state);
    assert_service_is_running(replacement.child_mut());
    replacement.stop();
}

#[test]
fn corrupt_sensitive_state_errors_do_not_repeat_secret_values() {
    let state = TestState::new();
    fs::write(
        state.path(),
        r#"{"characters":[{"id":"sentinel-private-token","name":"Pilot"}]}"#,
    )
    .unwrap();
    for flags in [vec!["list"], vec!["--json", "list"]] {
        let output = run_scenario(&state, &flags);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            !format!("{}{}", stdout(&output), stderr(&output)).contains("sentinel-private-token")
        );
    }
}

#[test]
fn configuration_invalid_values_are_invocation_errors() {
    let state = TestState::new();
    for args in [
        vec!["config", "set", "refresh-interval", "0m"],
        vec!["config", "set", "show-protected-killmails", "maybe"],
    ] {
        let output = run_scenario(&state, &args);
        assert_eq!(output.status.code(), Some(2));
    }
}

/// Sends `signal` to the service and returns its exit code and stderr.
#[cfg(unix)]
fn stop_with_signal(service: &mut ServiceProcess, signal: &str) -> (Option<i32>, String) {
    assert!(Command::new("kill")
        .args([signal, &service.child_mut().id().to_string()])
        .status()
        .unwrap()
        .success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = service.child_mut().try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "service did not stop after {signal}"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let mut diagnostics = String::new();
    if let Some(mut stderr) = service.child_mut().stderr.take() {
        use std::io::Read;
        stderr.read_to_string(&mut diagnostics).unwrap();
    }
    (status.code(), diagnostics)
}

#[cfg(unix)]
#[test]
fn service_stops_cleanly_on_sigint_and_sigterm_and_releases_its_lock() {
    for signal in ["-INT", "-TERM"] {
        let state = TestState::new();
        let mut service = service_process(&state);
        assert_service_is_running(service.child_mut());

        let (code, diagnostics) = stop_with_signal(&mut service, signal);

        assert_eq!(code, Some(0), "{signal}: {diagnostics}");
        assert!(diagnostics.contains("Refresh service stopped."));
        let status = success(run_scenario(&state, &["--json", "status"]));
        let status: serde_json::Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status["service_running"], false);
    }
}

#[cfg(unix)]
#[test]
fn service_without_characters_waits_instead_of_exiting() {
    let state = TestState::new();
    success(run_scenario(
        &state,
        &["characters", "remove", "1001", "--yes"],
    ));
    let mut service = service_process(&state);
    assert_service_is_running(service.child_mut());

    let (code, diagnostics) = stop_with_signal(&mut service, "-TERM");

    assert_eq!(code, Some(0));
    assert_eq!(
        diagnostics
            .matches("No characters are authenticated")
            .count(),
        1,
        "{diagnostics}"
    );
}

#[test]
fn competing_processes_cannot_read_or_mutate_during_an_operation() {
    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));
    let original = fs::read(state.path()).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.path().with_file_name("state.json.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    for args in [
        vec!["--json", "list"],
        vec!["protect", "add", "killmail", "9001"],
        vec!["post", "9001", "--yes"],
    ] {
        let output = run_scenario(&state, &args);
        assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    }
    assert_eq!(fs::read(state.path()).unwrap(), original);
    drop(lock);
    success(run_scenario(&state, &["--json", "list"]));
}

#[test]
fn uncertain_submission_survives_restart_and_override_cannot_bypass_evidence() {
    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));
    let mut store: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    let attempt = serde_json::json!({"state": "post_attempted", "attempted_at": future});
    store["zkill_status"]["9002"] = attempt.clone();
    fs::write(state.path(), serde_json::to_vec(&store).unwrap()).unwrap();
    let result = run_scenario(
        &state,
        &["--json", "post", "9002", "--post-anyway", "--yes"],
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("could not be confirmed"));
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    assert_eq!(after["zkill_status"]["9002"], attempt);
}

#[test]
fn characters_can_be_added_and_removed_with_explicit_confirmation() {
    let state = TestState::new();
    let added = success(run_scenario(
        &state,
        &["--json", "characters", "add", "--no-browser"],
    ));
    assert!(added.contains("Simulated Beta"));
    let denied = run_scenario(&state, &["characters", "remove", "1002"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(success(run_scenario(&state, &["characters", "list"])).contains("Simulated Beta"));
    success(run_scenario(
        &state,
        &["characters", "remove", "1002", "--yes"],
    ));
    assert!(!success(run_scenario(&state, &["characters", "list"])).contains("Simulated Beta"));
}

#[test]
fn persisted_cooldown_blocks_lookup_and_submission_in_new_processes() {
    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));
    let mut store: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    store["api_cooldowns"] = serde_json::json!([{"source":"zkillboard","until":future}]);
    fs::write(state.path(), serde_json::to_vec(&store).unwrap()).unwrap();
    let known = run_scenario(&state, &["--json", "post", "9001", "--yes"]);
    assert_eq!(known.status.code(), Some(1));
    assert!(stdout(&known).contains("cooldown"));
    store["zkill_status"]
        .as_object_mut()
        .unwrap()
        .remove("9001");
    store["zkill_pages"] = serde_json::json!({});
    fs::write(state.path(), serde_json::to_vec(&store).unwrap()).unwrap();
    let unknown = run_scenario(&state, &["--json", "post", "9001", "--yes"]);
    assert_eq!(unknown.status.code(), Some(1));
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(state.path()).unwrap()).unwrap();
    assert!(after["zkill_pages"].as_object().unwrap().is_empty());
    assert!(after["zkill_status"]["9001"].is_null());
}

#[test]
fn default_output_is_readable_text_and_json_is_opt_in() {
    let state = TestState::new();
    success(run_scenario(&state, &["refresh"]));

    let list = success(run_scenario(&state, &["list"]));
    let header = list.lines().next().unwrap();
    assert!(header.starts_with("ID"), "{list}");
    assert!(header.contains("STATUS") && header.contains("BULK"));
    assert!(list.contains("9001"));
    assert!(list.contains("protected killmails are hidden; use --show-protected"));
    assert!(!list.contains("fixture-hash") && !list.contains('{'));

    let shown = success(run_scenario(&state, &["--show-protected", "show", "9002"]));
    assert!(shown.contains("Protected:"));
    assert!(shown.contains("Eligible for bulk posting: no"));

    let status = success(run_scenario(&state, &["status"]));
    assert!(status.contains("Unreported killmails:"));

    let json = success(run_scenario(&state, &["--json", "list"]));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed
        .as_array()
        .unwrap()
        .iter()
        .all(|mail| mail["id"].is_u64()));
}
