use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const MISSING_CELL_ID: &str = "00000000-0000-0000-0000-000000000001";
const CORRELATED_CELL_ID: &str = "00000000-0000-0000-0000-000000000124";
const CORRELATED_OPERATION_ID: &str = "00000000-0000-0000-0000-000000000125";
const CORRELATED_JOB_ID: &str = "00000000-0000-0000-0000-000000000126";
const TRANSPORT_ACTIVE_OPERATION_ID: &str = "00000000-0000-0000-0000-000000000127";

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

#[test]
fn version_help_and_shell_completions_are_stable_and_state_free() {
    let version = run_vmcell(&["--version"]);
    assert!(version.status.success());
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.4.0");
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("vmcell {}\n", env!("CARGO_PKG_VERSION"))
    );

    let help = run_vmcell(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in [
        "doctor",
        "status",
        "state",
        "completion",
        "job",
        "run",
        "shell",
    ] {
        assert!(help.contains(command), "root help omitted {command}");
    }

    let run_help = run_vmcell(&["run", "--help"]);
    assert!(run_help.status.success());
    let run_help = String::from_utf8(run_help.stdout).unwrap();
    assert!(run_help.contains("--spec <PATH>"));
    assert!(run_help.contains("--plan-only"));

    let job_help = run_vmcell(&["job", "--help"]);
    assert!(job_help.status.success());
    assert!(String::from_utf8(job_help.stdout).unwrap().contains("plan"));

    let job_plan_help = run_vmcell(&["job", "plan", "--help"]);
    assert!(job_plan_help.status.success());
    assert!(
        String::from_utf8(job_plan_help.stdout)
            .unwrap()
            .contains("--spec <PATH>")
    );

    let directory = tempfile::tempdir().unwrap();
    let absent_state = directory.path().join("must-not-exist");
    let state = absent_state.to_string_lossy().into_owned();
    for (shell, marker) in [
        ("bash", "complete"),
        ("powershell", "Register-ArgumentCompleter"),
        ("zsh", "#compdef vmcell"),
    ] {
        let first = run_vmcell(&["--state-root", state.as_str(), "completion", shell]);
        let second = run_vmcell(&["--state-root", state.as_str(), "completion", shell]);
        assert!(first.status.success());
        assert!(second.status.success());
        assert_eq!(first.stdout, second.stdout);
        assert!(first.stderr.is_empty());
        assert!(String::from_utf8_lossy(&first.stdout).contains(marker));
        assert!(!absent_state.exists());
    }

    let json = run_vmcell(&[
        "--json",
        "--state-root",
        state.as_str(),
        "completion",
        "powershell",
    ]);
    assert_eq!(json.status.code(), Some(2));
    assert!(json.stdout.is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&json.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "vmcell.invalid_input");
    assert!(!absent_state.exists());
}

fn write_config(path: &std::path::Path, value: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, bool, Vec<u8>)> {
    fn collect(root: &Path, directory: &Path, entries: &mut Vec<(PathBuf, bool, Vec<u8>)>) {
        let mut paths = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                entries.push((relative, true, Vec::new()));
                collect(root, &path, entries);
            } else {
                assert!(
                    metadata.is_file(),
                    "fixture state tree unexpectedly contains a non-ordinary entry: {path:?}"
                );
                entries.push((relative, false, fs::read(path).unwrap()));
            }
        }
    }

    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries
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

fn write_image_fixture(state_root: &std::path::Path, base_path: &std::path::Path) {
    let images = state_root.join("images");
    fs::create_dir_all(&images).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(state_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&images, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let manifest = images.join("daily-image.json");
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "id": "daily-image",
            "guest_os": "windows",
            "guest_arch": "x86_64",
            "variants": [{
                "provider": "hyperv",
                "disk_format": "vhdx",
                "path": base_path,
                "sha256": "fixture-hash-never-read",
                "file_size": 23
            }],
            "registered_at": "2026-08-10T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_active_cell_fixture(state_root: &std::path::Path, base_path: &std::path::Path) {
    let cells = state_root.join("cells");
    fs::create_dir_all(&cells).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(state_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&cells, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let cell_id = "00000000-0000-0000-0000-000000000123";
    let manifest = cells.join(format!("{cell_id}.json"));
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "id": cell_id,
            "provider": "hyperv",
            "spec": {
                "image": "daily-image",
                "provider": "hyperv",
                "cpu_count": 2,
                "memory_mib": 4096,
                "ttl_seconds": null,
                "accelerator": null,
                "allow_tcg": false
            },
            "image": {
                "image_id": "daily-image",
                "guest_os": "windows",
                "provider": "hyperv",
                "disk_format": "vhdx",
                "path": base_path,
                "sha256": "fixture-hash-never-read",
                "file_size": 23
            },
            "ownership": {
                "schema_version": 1,
                "install_id": "00000000-0000-0000-0000-000000000001",
                "operation_id": "00000000-0000-0000-0000-000000000002",
                "provider_object_name": "vmcell-fixture",
                "provider_marker": "vmcell:v1:fixture",
                "configuration_path": state_root.join("runtime").join("fixture.vmcell.json"),
                "overlay_path": state_root.join("runtime").join("fixture.vhdx")
            },
            "provider_object": null,
            "state": "running",
            "phase": "ready",
            "created_at": "2026-08-10T00:00:00Z",
            "updated_at": "2026-08-10T00:00:00Z",
            "expires_at": null,
            "last_error": null
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_transport_active_operation_fixture(state_root: &std::path::Path) -> std::path::PathBuf {
    let operations = state_root.join("operations");
    fs::create_dir_all(&operations).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&operations, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let operation_path = operations.join(format!("{TRANSPORT_ACTIVE_OPERATION_ID}.json"));
    write_config(
        &operation_path,
        &serde_json::json!({
            "schema_version": 1,
            "id": TRANSPORT_ACTIVE_OPERATION_ID,
            "cell_id": "00000000-0000-0000-0000-000000000123",
            "kind": "copy_in",
            "phase": "transport_active",
            "created_at": "2026-08-12T00:00:00Z",
            "updated_at": "2026-08-12T00:00:01Z",
            "completed_at": null,
            "failure": "timeout",
            "exit_code": null,
            "stdout_bytes": null,
            "stderr_bytes": null,
            "artifact_id": null,
            "artifact_pruned_at": null
        }),
    );
    operation_path
}

fn write_v2_correlated_operation_and_artifact_fixture(state_root: &std::path::Path) {
    let cells = state_root.join("cells");
    let operations = state_root.join("operations");
    let artifacts = state_root.join("artifacts");
    let artifact_cell = artifacts.join(CORRELATED_CELL_ID);
    let artifact_root = artifact_cell.join(CORRELATED_OPERATION_ID);
    let files = artifact_root.join("files");
    fs::create_dir_all(&cells).unwrap();
    fs::create_dir_all(&operations).unwrap();
    fs::create_dir_all(&files).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for directory in [
            state_root,
            &cells,
            &operations,
            &artifacts,
            &artifact_cell,
            &artifact_root,
            &files,
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    let started_at = "2026-08-11T00:00:00Z";
    let job_spec_sha256 = "a".repeat(64);
    let artifact_bytes = b"job-artifact";
    let artifact_relative_path =
        format!("artifacts/{CORRELATED_CELL_ID}/{CORRELATED_OPERATION_ID}/files/0000.bin");
    let cell = serde_json::json!({
        "schema_version": 2,
        "id": CORRELATED_CELL_ID,
        "provider": "hyperv",
        "spec": {
            "image": "daily-image",
            "provider": "hyperv",
            "cpu_count": 2,
            "memory_mib": 4096,
            "ttl_seconds": null,
            "accelerator": null,
            "allow_tcg": false
        },
        "image": {
            "image_id": "daily-image",
            "guest_os": "windows",
            "provider": "hyperv",
            "disk_format": "vhdx",
            "path": state_root.join("base.vhdx"),
            "sha256": "fixture-hash-never-read",
            "file_size": 23
        },
        "ownership": {
            "schema_version": 1,
            "install_id": "00000000-0000-0000-0000-000000000001",
            "operation_id": "00000000-0000-0000-0000-000000000002",
            "provider_object_name": "vmcell-fixture",
            "provider_marker": "vmcell:v1:fixture",
            "configuration_path": state_root.join("runtime").join("fixture.vmcell.json"),
            "overlay_path": state_root.join("runtime").join("fixture.vhdx")
        },
        "provider_object": null,
        "state": "stopped",
        "phase": "ready",
        "created_at": started_at,
        "updated_at": started_at,
        "expires_at": null,
        "last_error": null,
        "job": {
            "job_id": CORRELATED_JOB_ID,
            "job_spec_sha256": job_spec_sha256,
            "started_at": started_at
        }
    });
    let operation = serde_json::json!({
        "schema_version": 2,
        "id": CORRELATED_OPERATION_ID,
        "cell_id": CORRELATED_CELL_ID,
        "kind": "artifact_collect",
        "phase": "completed",
        "created_at": started_at,
        "updated_at": started_at,
        "completed_at": started_at,
        "failure": null,
        "exit_code": null,
        "stdout_bytes": null,
        "stderr_bytes": null,
        "artifact_id": CORRELATED_OPERATION_ID,
        "artifact_pruned_at": null,
        "job_id": CORRELATED_JOB_ID
    });
    let artifact = serde_json::json!({
        "schema_version": 2,
        "id": CORRELATED_OPERATION_ID,
        "cell_id": CORRELATED_CELL_ID,
        "created_at": started_at,
        "entries": [{
            "guest_path": "results/output.bin",
            "host_relative_path": artifact_relative_path,
            "sha256": format!("{:x}", Sha256::digest(artifact_bytes)),
            "size": artifact_bytes.len()
        }],
        "job_id": CORRELATED_JOB_ID
    });
    let cell_path = cells.join(format!("{CORRELATED_CELL_ID}.json"));
    let operation_path = operations.join(format!("{CORRELATED_OPERATION_ID}.json"));
    let artifact_path = artifact_root.join("manifest.json");
    let artifact_file = files.join("0000.bin");
    fs::write(&cell_path, serde_json::to_vec_pretty(&cell).unwrap()).unwrap();
    fs::write(
        &operation_path,
        serde_json::to_vec_pretty(&operation).unwrap(),
    )
    .unwrap();
    fs::write(
        &artifact_path,
        serde_json::to_vec_pretty(&artifact).unwrap(),
    )
    .unwrap();
    fs::write(&artifact_file, artifact_bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for file in [&cell_path, &operation_path, &artifact_path, &artifact_file] {
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}

#[test]
fn transport_active_reconcile_is_fresh_process_read_only_and_nonreplaying() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("state");
    let base_path = directory.path().join("base.vhdx");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    write_active_cell_fixture(&state_root, &base_path);
    let operation_path = write_transport_active_operation_fixture(&state_root);
    let before = fs::read(&operation_path).unwrap();
    let state_before = snapshot_tree(&state_root);
    let base_before = fs::read(&base_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .arg("--json")
        .arg("--state-root")
        .arg(&state_root)
        .arg("operation")
        .arg("reconcile")
        .arg(TRANSPORT_ACTIVE_OPERATION_ID)
        .output()
        .expect("vmcell CLI should start");

    assert!(
        output.status.success(),
        "fresh-process reconciliation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["disposition"], "recovery_required");
    assert_eq!(report["required_action"], "manual_review");
    assert_eq!(report["changed"], false);
    assert_eq!(report["operation"]["id"], TRANSPORT_ACTIVE_OPERATION_ID);
    assert_eq!(report["operation"]["phase"], "transport_active");
    assert_eq!(report["operation"]["failure"], "timeout");
    assert_eq!(fs::read(&operation_path).unwrap(), before);
    assert_eq!(snapshot_tree(&state_root), state_before);
    assert_eq!(fs::read(&base_path).unwrap(), base_before);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(state_root.to_string_lossy().as_ref()));
    assert!(!stdout.contains(base_path.to_string_lossy().as_ref()));
}

#[test]
fn job_correlated_durable_json_records_are_v2_inside_v1_envelopes() {
    let directory = tempfile::tempdir().unwrap();
    write_v2_correlated_operation_and_artifact_fixture(directory.path());
    let state = directory.path().to_string_lossy().into_owned();

    let operation = run_vmcell(&[
        "--json",
        "--state-root",
        state.as_str(),
        "operation",
        "inspect",
        CORRELATED_OPERATION_ID,
    ]);
    assert!(operation.status.success());
    let operation: serde_json::Value = serde_json::from_slice(&operation.stdout).unwrap();
    assert_eq!(operation["schema_version"], 2);
    assert_eq!(operation["job_id"], CORRELATED_JOB_ID);

    let list = run_vmcell(&[
        "--json",
        "--state-root",
        state.as_str(),
        "operation",
        "list",
    ]);
    assert!(list.status.success());
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["schema_version"], 1);
    assert_eq!(list["items"][0]["schema_version"], 2);

    let artifact = run_vmcell(&[
        "--json",
        "--state-root",
        state.as_str(),
        "artifact",
        "inspect",
        CORRELATED_CELL_ID,
        CORRELATED_OPERATION_ID,
    ]);
    assert!(artifact.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&artifact.stdout).unwrap();
    assert_eq!(artifact["schema_version"], 2);
    assert_eq!(artifact["job_id"], CORRELATED_JOB_ID);
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
fn config_state_root_is_used_and_cli_state_root_wins() {
    let directory = tempfile::tempdir().unwrap();
    let configured_root = directory.path().join("configured-state");
    let override_root = directory.path().join("override-state");
    let base_path = directory.path().join("base.vhdx");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    write_image_fixture(&configured_root, &base_path);
    let config = directory.path().join("config.json");
    write_config(
        &config,
        &serde_json::json!({
            "schema_version": 1,
            "defaults": {
                "state_root": configured_root,
                "provider": "qemu",
                "cpu_count": 4,
                "memory_mib": 8192
            }
        }),
    );

    let configured = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .args(["image", "list"])
        .output()
        .unwrap();
    assert!(configured.status.success());
    let configured_json: serde_json::Value = serde_json::from_slice(&configured.stdout).unwrap();
    assert_eq!(configured_json["items"].as_array().unwrap().len(), 1);

    let overridden = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("--state-root")
        .arg(&override_root)
        .args(["image", "list"])
        .output()
        .unwrap();
    assert!(overridden.status.success());
    let overridden_json: serde_json::Value = serde_json::from_slice(&overridden.stdout).unwrap();
    assert_eq!(overridden_json["items"], serde_json::json!([]));
}

#[test]
fn state_check_is_read_only_versioned_and_rejects_future_state() {
    let directory = tempfile::tempdir().unwrap();
    let empty_root = directory.path().join("empty-state");
    let empty = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(&empty_root)
        .args(["state", "check"])
        .output()
        .unwrap();
    assert!(empty.status.success());
    let report: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["contract"], "vmcell.state-compatibility.v1");
    assert_eq!(report["durable_state_format_version"], 1);
    assert_eq!(report["status"], "empty");
    assert!(!empty_root.exists());

    let state_root = directory.path().join("future-state");
    let base_path = directory.path().join("base.vhdx");
    write_image_fixture(&state_root, &base_path);
    let manifest = state_root.join("images").join("daily-image.json");
    let mut future: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    future["schema_version"] = serde_json::json!(2);
    fs::write(&manifest, serde_json::to_vec_pretty(&future).unwrap()).unwrap();
    let before = fs::read(&manifest).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(&state_root)
        .args(["state", "check"])
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(9));
    let error: serde_json::Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "vmcell.state.upgrade_required");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("stop mutation")
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
}

#[test]
fn malformed_or_unsupported_config_fails_before_state_access_and_is_redacted() {
    let directory = tempfile::tempdir().unwrap();
    let forbidden_root = directory.path().join("must-not-exist");
    let config = directory.path().join("credential-sentinel-config.json");
    write_config(
        &config,
        &serde_json::json!({
            "schema_version": 1,
            "defaults": {
                "state_root": forbidden_root,
                "password": "credential-sentinel"
            }
        }),
    );
    let malformed = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(2));
    assert!(malformed.stdout.is_empty());
    let malformed_json: serde_json::Value = serde_json::from_slice(&malformed.stderr).unwrap();
    assert_eq!(malformed_json["error"]["code"], "vmcell.config.invalid");
    let serialized = String::from_utf8(malformed.stderr).unwrap();
    assert!(!serialized.contains("credential-sentinel"));
    assert!(!serialized.contains(directory.path().to_string_lossy().as_ref()));
    assert!(!forbidden_root.exists());

    write_config(
        &config,
        &serde_json::json!({"schema_version": 2, "defaults": {}}),
    );
    let unsupported = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(unsupported.status.code(), Some(9));
    let unsupported_json: serde_json::Value = serde_json::from_slice(&unsupported.stderr).unwrap();
    assert_eq!(
        unsupported_json["error"]["code"],
        "vmcell.config.unsupported_schema"
    );
}

#[test]
fn job_plan_rejects_secret_like_input_before_state_or_provider_access() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("must-not-exist");
    let config = directory.path().join("config.json");
    write_config(
        &config,
        &serde_json::json!({"schema_version": 1, "defaults": {}}),
    );
    let spec = directory.path().join("job.toml");
    fs::write(
        &spec,
        "schema_version = 1\npassword = \"job-spec-secret-sentinel",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&spec, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("--state-root")
        .arg(&state_root)
        .args(["job", "plan", "--spec"])
        .arg(&spec)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!state_root.exists());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        envelope["error"]["code"],
        "vmcell.job_spec.invalid_document"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("job-spec-secret-sentinel"));
}

#[test]
fn run_spec_rejects_secret_like_input_before_state_or_lifecycle_access() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("must-not-exist");
    let config = directory.path().join("config.json");
    write_config(
        &config,
        &serde_json::json!({"schema_version": 1, "defaults": {}}),
    );
    let spec = directory.path().join("job.toml");
    fs::write(
        &spec,
        "schema_version = 1\npassword = \"run-spec-secret-sentinel",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&spec, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("--state-root")
        .arg(&state_root)
        .args(["run", "--spec"])
        .arg(&spec)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!state_root.exists());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        envelope["error"]["code"],
        "vmcell.job_spec.invalid_document"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("run-spec-secret-sentinel"));
    assert!(!stderr.contains(directory.path().to_string_lossy().as_ref()));
}

#[test]
fn job_plan_missing_image_is_read_only() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("must-not-exist");
    let config = directory.path().join("config.json");
    write_config(
        &config,
        &serde_json::json!({"schema_version": 1, "defaults": {}}),
    );
    let spec = directory.path().join("job.toml");
    fs::write(
        &spec,
        r#"
schema_version = 1
image = "missing-image"
cpu_count = 2
memory_mib = 2048

[command]
program = "echo"

[cleanup]
keep = false
keep_on_failure = false
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&spec, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("--state-root")
        .arg(&state_root)
        .args(["job", "plan", "--spec"])
        .arg(&spec)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(!state_root.exists());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["error"]["code"], "vmcell.state.not_found");
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

#[test]
fn image_dependency_and_unregister_cli_are_provider_neutral_and_metadata_only() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("state");
    let base_path = directory.path().join("base.vhdx");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    write_image_fixture(&state_root, &base_path);

    let dependencies = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(&state_root)
        .args(["image", "dependencies", "daily-image"])
        .output()
        .unwrap();
    assert!(dependencies.status.success());
    let dependencies_json: serde_json::Value =
        serde_json::from_slice(&dependencies.stdout).unwrap();
    assert_eq!(
        dependencies_json["contract"],
        "vmcell.image-dependencies.v1"
    );
    assert_eq!(dependencies_json["can_unregister"], true);
    assert_eq!(dependencies_json["dependencies"], serde_json::json!([]));

    let unregister = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(&state_root)
        .args(["image", "unregister", "daily-image"])
        .output()
        .unwrap();
    assert!(unregister.status.success());
    let unregister_json: serde_json::Value = serde_json::from_slice(&unregister.stdout).unwrap();
    assert_eq!(unregister_json["contract"], "vmcell.image-unregister.v1");
    assert_eq!(unregister_json["metadata_removed"], true);
    assert_eq!(unregister_json["bytes_deleted"], false);
    assert!(!state_root.join("images").join("daily-image.json").exists());
    assert_eq!(fs::read(&base_path).unwrap(), b"immutable-base-sentinel");

    let repeated = Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(&state_root)
        .args(["image", "unregister", "daily-image"])
        .output()
        .unwrap();
    assert!(repeated.status.success());
    let repeated_json: serde_json::Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated_json["metadata_removed"], false);
    assert_eq!(repeated_json["bytes_deleted"], false);
    assert_eq!(fs::read(&base_path).unwrap(), b"immutable-base-sentinel");
}

#[test]
fn image_unregister_conflict_is_stable_redacted_and_non_mutating() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("state");
    let base_path = directory.path().join("credential-sentinel-base.vhdx");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    write_image_fixture(&state_root, &base_path);
    write_active_cell_fixture(&state_root, &base_path);

    for json in [true, false] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vmcell"));
        if json {
            command.arg("--json");
        }
        let output = command
            .arg("--state-root")
            .arg(&state_root)
            .args(["image", "unregister", "daily-image"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("vmcell.image.in_use"));
        assert!(!stderr.contains("credential-sentinel"));
        assert!(!stderr.contains(state_root.to_string_lossy().as_ref()));
    }

    assert!(state_root.join("images").join("daily-image.json").exists());
    assert_eq!(fs::read(&base_path).unwrap(), b"immutable-base-sentinel");
}
