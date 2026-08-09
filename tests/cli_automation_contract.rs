use std::process::Command;

const MISSING_CELL_ID: &str = "00000000-0000-0000-0000-000000000001";

fn inspect_missing_cell(state_root: &std::path::Path, json: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vmcell"));
    if json {
        command.arg("--json");
    }
    command
        .arg("--state-root")
        .arg(state_root)
        .arg("inspect")
        .arg(MISSING_CELL_ID)
        .output()
        .expect("vmcell CLI should start")
}

fn run_vmcell(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(arguments)
        .output()
        .expect("vmcell CLI should start")
}

fn invalid_run(state_root: &std::path::Path, json: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vmcell"));
    if json {
        command.arg("--json");
    }
    command
        .arg("--state-root")
        .arg(state_root)
        .args([
            "run",
            "--image",
            "windows-dev",
            "--provider",
            "qemu",
            "--cpu",
            "0",
            "--",
            "echo",
        ])
        .output()
        .expect("vmcell CLI should start")
}

#[test]
fn json_and_human_modes_share_the_same_stable_exit_classification() {
    let state = tempfile::tempdir().unwrap();
    let json = inspect_missing_cell(state.path(), true);
    let human = inspect_missing_cell(state.path(), false);

    assert_eq!(json.status.code(), Some(3));
    assert_eq!(human.status.code(), Some(3));
    assert!(json.stdout.is_empty());
    assert!(human.stdout.is_empty());

    let envelope: serde_json::Value = serde_json::from_slice(&json.stderr).unwrap();
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["error"]["code"], "vmcell.state.not_found");
    assert_eq!(envelope["error"]["category"], "not_found");
    assert_eq!(envelope["error"]["retryable"], false);
    assert_eq!(envelope["error"]["exit_code"], 3);
    assert_eq!(
        envelope["error"]["message"],
        "requested state object was not found"
    );
    assert!(!String::from_utf8_lossy(&json.stderr).contains(&state.path().display().to_string()));

    let human_stderr = String::from_utf8(human.stderr).unwrap();
    assert_eq!(
        human_stderr.trim(),
        "vmcell: vmcell.state.not_found: requested state object was not found"
    );
}

#[test]
fn json_parse_failures_are_versioned_redacted_and_migrate_legacy_provider_list() {
    for arguments in [
        vec!["--json", "provider-list"],
        vec!["--json", "--credential-sentinel"],
    ] {
        let output = run_vmcell(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["error"]["code"], "vmcell.invalid_input");
        assert_eq!(envelope["error"]["category"], "invalid_input");
        assert_eq!(envelope["error"]["retryable"], false);
        assert_eq!(envelope["error"]["exit_code"], 2);
        let serialized = String::from_utf8(output.stderr).unwrap();
        assert!(!serialized.contains("provider-list"));
        assert!(!serialized.contains("credential-sentinel"));
    }

    let human = run_vmcell(&["provider-list"]);
    assert_eq!(human.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&human.stderr).contains("provider"));

    let guest_argument = run_vmcell(&[
        "exec",
        "not-a-cell-id",
        "--username",
        "fixture",
        "--password-stdin",
        "--",
        "--json",
    ]);
    assert_eq!(guest_argument.status.code(), Some(2));
    assert!(serde_json::from_slice::<serde_json::Value>(&guest_argument.stderr).is_err());
}

#[test]
fn run_failures_preserve_stage_cell_and_cleanup_in_json_and_human_modes() {
    let state = tempfile::tempdir().unwrap();
    let json = invalid_run(state.path(), true);
    let human = invalid_run(state.path(), false);

    assert_eq!(json.status.code(), Some(2));
    assert_eq!(human.status.code(), Some(2));
    assert!(json.stdout.is_empty());
    assert!(human.stdout.is_empty());

    let envelope: serde_json::Value = serde_json::from_slice(&json.stderr).unwrap();
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["error"]["code"], "vmcell.invalid_input");
    assert_eq!(envelope["run"]["schema_version"], 1);
    assert_eq!(envelope["run"]["cell_id"], serde_json::Value::Null);
    assert_eq!(envelope["run"]["operation_id"], serde_json::Value::Null);
    assert_eq!(envelope["run"]["stage"], "request_validation");
    assert_eq!(envelope["run"]["cleanup"], "nothing_created");
    assert_eq!(
        envelope["run"]["cleanup_error_code"],
        serde_json::Value::Null
    );
    assert!(
        !String::from_utf8_lossy(&json.stderr).contains(state.path().to_string_lossy().as_ref())
    );

    let human_stderr = String::from_utf8(human.stderr).unwrap();
    assert!(human_stderr.contains("run stage=request_validation"));
    assert!(human_stderr.contains("cell=none"));
    assert!(human_stderr.contains("operation=none"));
    assert!(human_stderr.contains("cleanup=nothing_created"));
    assert!(human_stderr.contains("cleanup_error=none"));
}
