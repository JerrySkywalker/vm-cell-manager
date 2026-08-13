use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use vm_cell_manager::core::cell::CellId;
use vm_cell_manager::core::job_spec::{JobSpecError, parse_job_spec};
use vm_cell_manager::state::StateStore;

const LEGACY_CELL_ID: &str = "00000000-0000-0000-0000-000000000201";
const LEGACY_OPERATION_ID: &str = "00000000-0000-0000-0000-000000000202";
const V2_CELL_ID: &str = "00000000-0000-0000-0000-000000000301";
const V2_OPERATION_ID: &str = "00000000-0000-0000-0000-000000000302";
const LEGACY_SECRET_SENTINEL: &str = "credential-sentinel raw legacy provider detail";
const INVALID_SPEC_SECRET_SENTINEL: &str = "compatibility-credential-sentinel";
const MAX_REPOSITORY_RELATIVE_FIXTURE_PATH: usize = 100;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("compat")
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest() -> Value {
    read_json(fixture_root().join("manifest.json"))
}

fn json_path(path: &Path) -> String {
    serde_json::to_string(path.to_str().expect("fixture path must be UTF-8")).unwrap()
}

fn make_private_directory(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn make_private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn materialize_template(
    template: &Path,
    destination: &Path,
    base_path: &Path,
    path_replacements: &[(&str, &str)],
) {
    fn copy_entry(
        source: &Path,
        destination: &Path,
        replacements: &[(&str, String)],
        path_replacements: &[(&str, &str)],
    ) {
        let metadata = fs::symlink_metadata(source).unwrap();
        assert!(
            !metadata.file_type().is_symlink(),
            "fixture templates must not contain links"
        );
        if metadata.is_dir() {
            fs::create_dir_all(destination).unwrap();
            make_private_directory(destination);
            let mut entries = fs::read_dir(source)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            entries.sort();
            for entry in entries {
                let name = entry.file_name().unwrap().to_str().unwrap();
                let mut output_name = name.strip_suffix(".in").unwrap_or(name).to_owned();
                if output_name == "m.json" {
                    output_name = "manifest.json".to_owned();
                }
                for (token, replacement) in path_replacements {
                    output_name = output_name.replace(token, replacement);
                }
                copy_entry(
                    &entry,
                    &destination.join(output_name),
                    replacements,
                    path_replacements,
                );
            }
        } else {
            assert!(
                metadata.is_file(),
                "fixture templates must contain ordinary files only"
            );
            if source
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".in")
            {
                let mut text = fs::read_to_string(source).unwrap();
                for (token, replacement) in replacements {
                    text = text.replace(token, replacement);
                }
                assert!(!text.contains("{{"), "fixture template retained a token");
                fs::write(destination, text).unwrap();
            } else {
                fs::copy(source, destination).unwrap();
            }
            make_private_file(destination);
        }
    }

    let runtime = destination.join("runtime");
    let replacements = [
        ("{{BASE_PATH_JSON}}", json_path(base_path)),
        (
            "{{CONFIG_PATH_JSON}}",
            json_path(&runtime.join("fixture.vmcell.json")),
        ),
        (
            "{{OVERLAY_PATH_JSON}}",
            json_path(&runtime.join("fixture.overlay")),
        ),
    ];
    copy_entry(template, destination, &replacements, path_replacements);
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
                    "materialized fixture contains a special entry"
                );
                entries.push((relative, false, fs::read(path).unwrap()));
            }
        }
    }

    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries
}

fn run_state_check(state_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(state_root)
        .args(["state", "check"])
        .output()
        .expect("vmcell state check should start")
}

fn run_job_plan(state_root: &Path, spec: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vmcell"))
        .args(["--json", "--state-root"])
        .arg(state_root)
        .args(["job", "plan", "--spec"])
        .arg(spec)
        .output()
        .expect("vmcell job plan should start")
}

fn assert_json_subset(expected: &Value, actual: &Value, location: &str) {
    match expected {
        Value::Object(expected) => {
            let actual = actual
                .as_object()
                .unwrap_or_else(|| panic!("{location} was not an object"));
            for (key, value) in expected {
                let child = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{location} omitted {key}"));
                assert_json_subset(value, child, &format!("{location}.{key}"));
            }
        }
        _ => assert_eq!(expected, actual, "golden mismatch at {location}"),
    }
}

fn assert_no_path_or_secret(output: &Output, path: &Path, secrets: &[&str]) {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains(path.to_string_lossy().as_ref()));
    for secret in secrets {
        assert!(
            !combined.contains(secret),
            "compatibility output disclosed a sentinel"
        );
    }
}

#[test]
fn frozen_manifest_binds_exact_sources_state_specs_and_package_layouts() {
    let manifest = manifest();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(
        manifest["contract"],
        "vmcell.frozen-compatibility-fixtures.v1"
    );
    assert_eq!(
        manifest["base_dev_sha"],
        "27dcc1c56db91f8c8ce34bcb8d7e3ed667962158"
    );
    assert_eq!(
        manifest["authority"],
        "version_neutral_reader_and_contract_prework_only"
    );
    assert_eq!(manifest["owner_decision_issue"], 61);
    assert_eq!(manifest["owner_decision"], "pending");

    let expected = [
        (
            "v0.1.0",
            "release/v0.1.0",
            "32f4adad3881c5248c6c8c5d47982368b7b55799",
            "0.1.0",
            json!(["legacy-v1"]),
            Value::Null,
            json!([[
                "windows-x86_64",
                "vmcell-v0.1.0-windows-x86_64.zip",
                "windows-v1-core"
            ]]),
        ),
        (
            "v0.2.0",
            "release/v0.2.0",
            "ed2ed31ae2f0182fc1626321b81e86d09db378c2",
            "0.2.0",
            json!(["legacy-v1"]),
            Value::Null,
            json!([[
                "windows-x86_64",
                "vmcell-v0.2.0-windows-x86_64.zip",
                "windows-v2-metadata-completion"
            ]]),
        ),
        (
            "v0.3.0",
            "release/v0.3.0",
            "d0af04b2e84cf2226628173d2ed0d295aed01f2b",
            "0.3.0",
            json!(["legacy-v1"]),
            Value::Null,
            json!([
                [
                    "windows-x86_64",
                    "vmcell-v0.3.0-windows-x86_64.zip",
                    "windows-v2-metadata-completion"
                ],
                [
                    "linux-x86_64",
                    "vmcell-v0.3.0-linux-x86_64.tar.gz",
                    "linux-v1-user-portable"
                ]
            ]),
        ),
        (
            "v0.4.0",
            "release/v0.4.0",
            "c741be99ef4632b436f394f1c53b71ed57d0d2d9",
            "0.4.0",
            json!(["legacy-v1", "job-correlated-v2"]),
            json!("spec/v04-valid.toml"),
            json!([
                [
                    "windows-x86_64",
                    "vmcell-v0.4.0-windows-x86_64.zip",
                    "windows-v2-metadata-completion"
                ],
                [
                    "linux-x86_64",
                    "vmcell-v0.4.0-linux-x86_64.tar.gz",
                    "linux-v1-user-portable"
                ]
            ]),
        ),
    ];
    let candidates = manifest["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), expected.len());
    for (candidate, expected) in candidates.iter().zip(expected) {
        assert_eq!(candidate["release"], expected.0);
        assert_eq!(candidate["ref"], expected.1);
        assert_eq!(candidate["source_sha"], expected.2);
        assert_eq!(candidate["cargo_version"], expected.3);
        assert_eq!(candidate["rust_version"], "1.85.0");
        assert_eq!(candidate["disposition"], "retired_correction_required");
        assert_eq!(candidate["state_fixture_ids"], expected.4);
        assert_eq!(candidate["job_spec_fixture"], expected.5);
        let packages = candidate["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|package| {
                json!([
                    package["platform"],
                    package["archive"],
                    package["layout_revision"]
                ])
            })
            .collect::<Vec<_>>();
        assert_eq!(Value::Array(packages), expected.6);
        for package in candidate["packages"].as_array().unwrap() {
            assert_eq!(package["checksum"], "SHA256SUMS.txt");
        }
    }

    assert_eq!(
        manifest["forbidden_authority"],
        json!([
            "version_bump",
            "release_ref_or_tag",
            "package_publication",
            "support_promotion",
            "provider_or_guest_execution",
            "real_platform_acceptance",
            "owner_decision_selection"
        ])
    );
    assert_eq!(manifest["limitations"].as_array().unwrap().len(), 3);
}

#[test]
fn compatibility_inventory_preserves_evidence_and_authority_boundaries() {
    let inventory = include_str!("../docs/contract-compatibility-inventory.md");
    for required in [
        "vmcell.frozen-compatibility-fixtures.v1",
        "27dcc1c56db91f8c8ce34bcb8d7e3ed667962158",
        "RETIRED_CORRECTION_REQUIRED",
        "issue #61",
        "format-1",
        "format-2",
        "vmcell.state.upgrade_required",
        "vmcell.job_spec.unsupported_schema",
        "Package fixtures are metadata snapshots",
        "does not authorize",
        "fresh package identities",
        "new dedicated-host R5 receipts",
    ] {
        assert!(
            inventory.contains(required),
            "compatibility inventory omitted {required}"
        );
    }
    assert!(!inventory.contains("OWNER_DECISION=SELECTED_"));
    assert!(!inventory.contains("support is promoted"));
}

#[test]
fn compatibility_fixture_paths_preserve_windows_clone_margin() {
    fn collect(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            paths.push(path.strip_prefix(root).unwrap().to_path_buf());
            if path.is_dir() {
                collect(root, &path, paths);
            }
        }
    }

    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect(repository_root, &fixture_root(), &mut paths);
    assert!(!paths.is_empty());
    for path in paths {
        let length = path.to_string_lossy().len();
        assert!(
            length <= MAX_REPOSITORY_RELATIVE_FIXTURE_PATH,
            "compatibility fixture path exceeded the Windows clone budget: {length} > {MAX_REPOSITORY_RELATIVE_FIXTURE_PATH}: {}",
            path.display()
        );
    }
}

fn assert_compatible_fixture(
    template_name: &str,
    expected_format: u64,
) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("state");
    let base_path = directory.path().join("immutable-base.img");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    make_private_file(&base_path);
    let path_replacements = match template_name {
        "v1" => [
            ("CELL_ID", LEGACY_CELL_ID),
            ("OPERATION_ID", LEGACY_OPERATION_ID),
        ],
        "v2" => [("CELL_ID", V2_CELL_ID), ("OPERATION_ID", V2_OPERATION_ID)],
        _ => panic!("unknown compatible fixture template {template_name}"),
    };
    materialize_template(
        &fixture_root().join(template_name),
        &state_root,
        &base_path,
        &path_replacements,
    );
    let artifact_root = state_root
        .join("artifacts")
        .join(path_replacements[0].1)
        .join(path_replacements[1].1);
    assert!(artifact_root.join("manifest.json").is_file());
    assert!(artifact_root.join("files").join("0000.bin").is_file());
    assert!(!artifact_root.join("m.json").exists());
    let before = snapshot_tree(&state_root);
    let base_before = fs::read(&base_path).unwrap();
    let output = run_state_check(&state_root);
    assert!(
        output.status.success(),
        "state check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    let golden = read_json(
        fixture_root()
            .join("golden")
            .join("state-compatible-subset.json"),
    );
    assert_json_subset(&golden, &actual, "state-compatible");
    assert_eq!(actual["durable_state_format_version"], expected_format);
    assert_eq!(snapshot_tree(&state_root), before);
    assert_eq!(fs::read(&base_path).unwrap(), base_before);
    assert_no_path_or_secret(&output, directory.path(), &[LEGACY_SECRET_SENTINEL]);
    (directory, state_root)
}

#[test]
fn current_dev_reads_frozen_v1_state_without_rewrite_and_redacts_legacy_detail() {
    let (directory, state_root) = assert_compatible_fixture("v1", 1);
    let before = snapshot_tree(&state_root);
    let cell = StateStore::new(state_root.clone())
        .load_cell(LEGACY_CELL_ID.parse::<CellId>().unwrap())
        .unwrap();
    assert_eq!(cell.last_error.as_deref(), Some("vmcell.legacy.redacted"));
    assert_eq!(snapshot_tree(&state_root), before);
    assert!(!format!("{cell:?}").contains(LEGACY_SECRET_SENTINEL));
    drop(directory);
}

#[test]
fn current_dev_reads_frozen_v04_job_state_without_rewrite() {
    let (_directory, _state_root) = assert_compatible_fixture("v2", 2);
}

#[test]
fn current_dev_rejects_future_state_without_rewrite_or_path_disclosure() {
    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("future-state");
    let base_path = directory.path().join("immutable-base.img");
    fs::write(&base_path, b"immutable-base-sentinel").unwrap();
    make_private_file(&base_path);
    materialize_template(&fixture_root().join("future"), &state_root, &base_path, &[]);
    let before = snapshot_tree(&state_root);
    let base_before = fs::read(&base_path).unwrap();
    let output = run_state_check(&state_root);
    assert_eq!(output.status.code(), Some(9));
    assert!(output.stdout.is_empty());
    let actual: Value = serde_json::from_slice(&output.stderr).unwrap();
    let golden = read_json(
        fixture_root()
            .join("golden")
            .join("state-upgrade-required-subset.json"),
    );
    assert_json_subset(&golden, &actual, "state-upgrade-required");
    assert_eq!(snapshot_tree(&state_root), before);
    assert_eq!(fs::read(&base_path).unwrap(), base_before);
    assert_no_path_or_secret(&output, directory.path(), &[]);
}

#[test]
fn current_dev_reads_v04_spec_and_fails_closed_on_future_or_secret_like_input() {
    let root = fixture_root().join("spec");
    let valid_path = root.join("v04-valid.toml");
    let future_path = root.join("v04-future.toml");
    let invalid_path = root.join("secret-like-invalid.toml");
    let valid_before = fs::read(&valid_path).unwrap();
    let future_before = fs::read(&future_path).unwrap();
    let invalid_before = fs::read(&invalid_path).unwrap();

    let valid = parse_job_spec(std::str::from_utf8(&valid_before).unwrap()).unwrap();
    assert_eq!(valid.schema_version, 1);
    assert_eq!(valid.image.as_str(), "frozen-linux-image");
    assert_eq!(valid.command.program, "/usr/bin/printf");
    assert_eq!(valid.copy_in.len(), 1);
    assert_eq!(valid.artifacts.sources.len(), 1);
    assert!(valid.cleanup.keep_on_failure);

    assert!(matches!(
        parse_job_spec(std::str::from_utf8(&future_before).unwrap()),
        Err(JobSpecError::UnsupportedSchema { actual: 2, .. })
    ));

    let directory = tempfile::tempdir().unwrap();
    let state_root = directory.path().join("must-not-exist");
    let private_specs = directory.path().join("specs");
    fs::create_dir(&private_specs).unwrap();
    make_private_directory(&private_specs);
    let private_future = private_specs.join("future.toml");
    let private_invalid = private_specs.join("invalid.toml");
    fs::write(&private_future, &future_before).unwrap();
    fs::write(&private_invalid, &invalid_before).unwrap();
    make_private_file(&private_future);
    make_private_file(&private_invalid);

    let future = run_job_plan(&state_root, &private_future);
    assert_eq!(future.status.code(), Some(9));
    assert!(future.stdout.is_empty());
    assert_json_subset(
        &read_json(
            fixture_root()
                .join("golden")
                .join("job-spec-unsupported-subset.json"),
        ),
        &serde_json::from_slice(&future.stderr).unwrap(),
        "job-spec-unsupported",
    );
    assert!(!state_root.exists());
    assert_no_path_or_secret(&future, directory.path(), &[INVALID_SPEC_SECRET_SENTINEL]);

    let invalid = run_job_plan(&state_root, &private_invalid);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert_json_subset(
        &read_json(
            fixture_root()
                .join("golden")
                .join("job-spec-invalid-subset.json"),
        ),
        &serde_json::from_slice(&invalid.stderr).unwrap(),
        "job-spec-invalid",
    );
    assert!(!state_root.exists());
    assert_no_path_or_secret(&invalid, directory.path(), &[INVALID_SPEC_SECRET_SENTINEL]);

    assert_eq!(fs::read(valid_path).unwrap(), valid_before);
    assert_eq!(fs::read(future_path).unwrap(), future_before);
    assert_eq!(fs::read(invalid_path).unwrap(), invalid_before);
    assert_eq!(fs::read(private_future).unwrap(), future_before);
    assert_eq!(fs::read(private_invalid).unwrap(), invalid_before);
}
