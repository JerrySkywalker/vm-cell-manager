use std::fs;
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

fn write_config(path: &std::path::Path, value: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
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
