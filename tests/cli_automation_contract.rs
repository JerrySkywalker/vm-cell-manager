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

    let human_stderr = String::from_utf8(human.stderr).unwrap();
    assert!(human_stderr.starts_with("vmcell: "));
}
