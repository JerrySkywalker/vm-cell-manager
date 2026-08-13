const LEDGER: &str = include_str!("../docs/v041-corrective-acceptance-ledger.md");
const QUALIFICATION: &str = include_str!("../docs/v041-frozen-qualification.md");
const RELEASE_REHEARSAL: &str = include_str!("../docs/v041-release-rehearsal.json");
const RUNBOOK: &str = include_str!("../docs/receipts/v041-r5-dedicated-host-runbook.md");
const CARGO_LOCK: &str = include_str!("../Cargo.lock");
const README: &str = include_str!("../README.md");
const CHANGELOG: &str = include_str!("../CHANGELOG.md");
const WINDOWS_INSTALL: &str = include_str!("../docs/windows-install-upgrade-remove.md");
const LINUX_INSTALL: &str = include_str!("../docs/linux-install-upgrade-remove.md");
const WINDOWS_DAILY_DRIVER: &str = include_str!("../docs/windows-daily-driver.md");

#[test]
fn corrective_ledger_binds_selected_strategy_floor_and_historical_retirement() {
    for required in [
        "vmcell.v0.4.1-corrective-acceptance-ledger.v1",
        "OWNER_DECISION=SELECTED_B",
        "0e7fcf37f4310562d318f9d5c709ddf8e8ca1637",
        "18c2e81acc4db57e2275175b138d31049df000da",
        "PROMOTION_ELIGIBLE_PENDING_R5",
        "f10ea52a9dddf62adf115225ae0f9d83b5f298da",
        "0217fa3d42addb95008e0190124d50ed4383d0ba",
        "f539cbc8aa0d4438df21256ebed3590c187824b1",
        "06934a5b8ce93b80f5fb2b1fc7353a070751e784",
        "97e5054f4c7a7d231c2c97c9c6615574f7b83299",
        "e50e1759cb3a1c003230d1de45a5a64c6f6283ce",
        "090fe32f7e8df1291e076efa8567c7f6b643d4ee",
        "4a97888b2a964cabd2ba5d32a967674421c0ae2d",
        "27dcc1c56db91f8c8ce34bcb8d7e3ed667962158",
        "51a8fdfb2410c3ac00bc56ac30b3ff4bc61a77e1",
        "390880e4d24c57da62d23ed958cd91e3ee77b1bb",
        "9da83b767659557f4a2438a5c63d31dacec34c95",
        "RETIRED_CORRECTION_REQUIRED",
        "release/v0.1.0@32f4adad3881c5248c6c8c5d47982368b7b55799",
        "release/v0.2.0@ed2ed31ae2f0182fc1626321b81e86d09db378c2",
        "release/v0.3.0@d0af04b2e84cf2226628173d2ed0d295aed01f2b",
        "release/v0.4.0@c741be99ef4632b436f394f1c53b71ed57d0d2d9",
        "vmcell-v0.4.1-windows-x86_64.zip",
        "vmcell-v0.4.1-linux-x86_64.tar.gz",
        "V041-R5-HYPERV-PSD-V1",
        "V041-R5-WHPX-QGA-V1",
        "V041-R5-KVM-QGA-V1",
        "V041-R5-JOBSPEC-OVERLAY-V1",
        "R4 remains optional shared-host performance/diagnostic evidence",
    ] {
        assert!(LEDGER.contains(required), "ledger omitted {required}");
    }

    for prohibited in [
        "support_status: supported",
        "real_platform_acceptance: completed",
        "result: PASS",
        "tag: v0.4.1",
        "package_publication: authorized",
    ] {
        assert!(
            !LEDGER.contains(prohibited),
            "ledger claimed forbidden authority: {prohibited}"
        );
    }
}

#[test]
fn qualification_and_release_rehearsal_stay_exact_candidate_and_non_authorizing() {
    for required in [
        "vmcell.v041-frozen-qualification.v1",
        "PROMOTION_ELIGIBLE_PENDING_R5",
        "NOT_COMPLETED",
        "TECHNICAL_FAILURE",
        "binary reproducibility",
        "R5 is `NOT_EXECUTED`",
        "support remains `untested`",
        "31730186128",
        "31744783947",
        "3802a045148849c2dc7a385e2fee43865336dbd3d12ea64347503713230324b7",
        "0a258f4f838f38ed632e80a2aec8e2ae6526de6656b21ddde77aeb13efa2999b",
    ] {
        assert!(
            QUALIFICATION.contains(required),
            "qualification omitted {required}"
        );
    }

    for prohibited in [
        "support_status: supported",
        "R5 is `PASS`",
        "publication_performed: true",
    ] {
        assert!(
            !QUALIFICATION.contains(prohibited),
            "qualification claimed forbidden authority: {prohibited}"
        );
    }

    let rehearsal: serde_json::Value =
        serde_json::from_str(RELEASE_REHEARSAL).expect("release rehearsal must be JSON");
    assert_eq!(rehearsal["contract"], "vmcell.v041-release-rehearsal.v1");
    assert_eq!(rehearsal["authorizing"], false);
    assert_eq!(rehearsal["publication_performed"], false);
    assert_eq!(
        rehearsal["candidate_disposition"],
        "PROMOTION_ELIGIBLE_PENDING_R5"
    );
    assert_eq!(
        rehearsal["candidate"]["sha"],
        "0e7fcf37f4310562d318f9d5c709ddf8e8ca1637"
    );
    assert_eq!(
        rehearsal["candidate"]["tree"],
        "18c2e81acc4db57e2275175b138d31049df000da"
    );
    assert_eq!(rehearsal["r5"]["result"], "NOT_EXECUTED");
    assert_eq!(rehearsal["tag_plan"]["created"], false);
    assert_eq!(rehearsal["main_promotion"]["performed"], false);
    assert_eq!(rehearsal["support_rendering"]["changed"], false);
    assert_eq!(
        rehearsal["assets"].as_array().map(Vec::len),
        Some(2),
        "release rehearsal must bind both platform assets"
    );
    for asset in rehearsal["assets"]
        .as_array()
        .expect("release assets must be an array")
    {
        assert_eq!(
            asset["source_commit"],
            "0e7fcf37f4310562d318f9d5c709ddf8e8ca1637"
        );
        assert_eq!(asset["source_date_epoch"], 1786641485);
    }
}

#[test]
fn r5_runbook_is_tuple_specific_fail_closed_and_non_authorizing() {
    for required in [
        "vmcell.v0.4.1-r5-dedicated-host-runbook.v1",
        "authorizing: false",
        "real_platform_acceptance: pending",
        "result: NOT_EXECUTED",
        "support_status: untested",
        "OWNER_DECISION_REQUIRED",
        "BLOCKED_EXTERNAL",
        "PREFLIGHT_PASS",
        "authorized-real-run",
        "V041-R5-HYPERV-PSD-V1",
        "V041-R5-WHPX-QGA-V1",
        "PROC_THREAD_ATTRIBUTE_JOB_LIST",
        "ActiveProcesses == 0",
        "V041-R5-KVM-QGA-V1",
        "native non-WSL2, non-container host",
        "performs no ioctl, VM",
        "creation, QEMU launch",
        "V041-R5-JOBSPEC-OVERLAY-V1",
        "Execute the identical spec twice",
        "fresh job, cell, operation, and artifact IDs",
        "no replay",
        "exact-owned",
        "Raw host evidence stays outside Git",
        "R5 completion still does not authorize a",
    ] {
        assert!(RUNBOOK.contains(required), "R5 runbook omitted {required}");
    }

    for prohibited in [
        "support_status: supported",
        "real_platform_acceptance: completed",
        "result: PASS",
        "password:",
        "credential:",
        "guest_output:",
        "command_argv:",
    ] {
        assert!(
            !RUNBOOK.contains(prohibited),
            "R5 runbook claimed authority or disclosure field: {prohibited}"
        );
    }
}

#[test]
fn repository_package_and_current_docs_share_the_v041_candidate_identity() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.4.1");
    assert!(CARGO_LOCK.contains("name = \"vm-cell-manager\"\nversion = \"0.4.1\""));
    for (name, document) in [
        ("README", README),
        ("changelog", CHANGELOG),
        ("Windows install", WINDOWS_INSTALL),
        ("Linux install", LINUX_INSTALL),
        ("Windows daily driver", WINDOWS_DAILY_DRIVER),
    ] {
        assert!(document.contains("0.4.1"), "{name} omitted candidate 0.4.1");
    }
    for (name, current_document) in [
        ("Windows install", WINDOWS_INSTALL),
        ("Linux install", LINUX_INSTALL),
        ("Windows daily driver", WINDOWS_DAILY_DRIVER),
    ] {
        assert!(
            !current_document.contains("0.4.0"),
            "{name} retained the retired current-install example"
        );
    }
    assert!(README.contains("consolidated corrective"));
    assert!(CHANGELOG.contains("candidate-only"));
    assert!(LEDGER.contains("release/v0.4.1@0e7fcf37f4310562d318f9d5c709ddf8e8ca1637"));
    assert!(LEDGER.contains("18c2e81acc4db57e2275175b138d31049df000da"));
}
