use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Component, Path};

use serde::Serialize;
use thiserror::Error;

use super::image::{Architecture, GuestOs};

pub const SUPPORT_MATRIX_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostOs {
    Windows,
    Linux,
    Macos,
}

impl HostOs {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Macos => "macos",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Hyperv,
    Qemu,
}

impl ProviderId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hyperv => "hyperv",
            Self::Qemu => "qemu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accelerator {
    HyperV,
    Whpx,
    Kvm,
    Hvf,
    Tcg,
}

impl Accelerator {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HyperV => "hyper-v",
            Self::Whpx => "whpx",
            Self::Kvm => "kvm",
            Self::Hvf => "hvf",
            Self::Tcg => "tcg",
        }
    }

    #[must_use]
    pub const fn is_hardware(self) -> bool {
        !matches!(self, Self::Tcg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuestTransportId {
    PowerShellDirect,
    Qga,
    Ssh,
}

impl GuestTransportId {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PowerShellDirect => "powershell-direct",
            Self::Qga => "qga",
            Self::Ssh => "ssh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportStatus {
    Supported,
    Experimental,
    DevelopmentOnly,
    Untested,
    Unsupported,
}

impl SupportStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Experimental => "experimental",
            Self::DevelopmentOnly => "development-only",
            Self::Untested => "untested",
            Self::Unsupported => "unsupported",
        }
    }

    const fn requires_real_platform_evidence(self) -> bool {
        matches!(self, Self::Supported | Self::Experimental)
    }
}

pub const SUPPORT_STATUS_VOCABULARY: [SupportStatus; 5] = [
    SupportStatus::Supported,
    SupportStatus::Experimental,
    SupportStatus::DevelopmentOnly,
    SupportStatus::Untested,
    SupportStatus::Unsupported,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceScope {
    RepositoryLocal,
    RealPlatform,
}

impl AcceptanceScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryLocal => "repository-local",
            Self::RealPlatform => "real-platform",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AcceptanceEvidence {
    pub scope: AcceptanceScope,
    pub reference: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SupportKey {
    pub host_os: HostOs,
    pub host_architecture: Architecture,
    pub provider: ProviderId,
    pub accelerator: Accelerator,
    pub guest_os: GuestOs,
    pub guest_architecture: Architecture,
    pub guest_transport: GuestTransportId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SupportMatrixEntry {
    #[serde(flatten)]
    pub key: SupportKey,
    pub status: SupportStatus,
    pub acceptance_evidence: &'static [AcceptanceEvidence],
}

const NO_EVIDENCE: &[AcceptanceEvidence] = &[];

pub const SUPPORT_MATRIX: &[SupportMatrixEntry] = &[
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Hyperv,
            accelerator: Accelerator::HyperV,
            guest_os: GuestOs::Windows,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::PowerShellDirect,
        },
        status: SupportStatus::Untested,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Hyperv,
            accelerator: Accelerator::Whpx,
            guest_os: GuestOs::Windows,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::PowerShellDirect,
        },
        status: SupportStatus::Unsupported,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Whpx,
            guest_os: GuestOs::Windows,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::Unsupported,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Whpx,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::Untested,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Whpx,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Ssh,
        },
        status: SupportStatus::Unsupported,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Windows,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::DevelopmentOnly,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Kvm,
            guest_os: GuestOs::Windows,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::Unsupported,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Kvm,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::Untested,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Kvm,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Ssh,
        },
        status: SupportStatus::Unsupported,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::DevelopmentOnly,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Macos,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Hvf,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::Untested,
        acceptance_evidence: NO_EVIDENCE,
    },
    SupportMatrixEntry {
        key: SupportKey {
            host_os: HostOs::Macos,
            host_architecture: Architecture::X86_64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Tcg,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::X86_64,
            guest_transport: GuestTransportId::Qga,
        },
        status: SupportStatus::DevelopmentOnly,
        acceptance_evidence: NO_EVIDENCE,
    },
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupportMatrixError {
    #[error("support matrix is empty")]
    Empty,
    #[error("support matrix contains a duplicate or conflicting key: {0:?}")]
    DuplicateKey(SupportKey),
    #[error("support matrix entries are not in canonical order at: {0:?}")]
    NonCanonicalOrder(SupportKey),
    #[error("support matrix combination may only be declared unsupported: {0:?}")]
    ImpossibleCombination(SupportKey),
    #[error("support status requires declared real-platform acceptance evidence: {0:?}")]
    MissingRealPlatformEvidence(SupportKey),
    #[error("acceptance evidence reference is invalid: {0}")]
    InvalidEvidenceReference(String),
    #[error("acceptance evidence is missing or not an ordinary file: {0}")]
    MissingEvidenceFile(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupportLookupError {
    #[error("support combination is undocumented")]
    UndocumentedCombination,
}

pub fn validate_support_matrix(entries: &[SupportMatrixEntry]) -> Result<(), SupportMatrixError> {
    if entries.is_empty() {
        return Err(SupportMatrixError::Empty);
    }

    let mut keys = BTreeSet::new();
    let mut previous_rank = None;
    for entry in entries {
        let rank = support_key_rank(&entry.key);
        if previous_rank.is_some_and(|previous| previous >= rank) {
            if keys.contains(&rank) {
                return Err(SupportMatrixError::DuplicateKey(entry.key));
            }
            return Err(SupportMatrixError::NonCanonicalOrder(entry.key));
        }
        if !keys.insert(rank) {
            return Err(SupportMatrixError::DuplicateKey(entry.key));
        }
        previous_rank = Some(rank);

        if !combination_is_possible(&entry.key) && entry.status != SupportStatus::Unsupported {
            return Err(SupportMatrixError::ImpossibleCombination(entry.key));
        }

        for evidence in entry.acceptance_evidence {
            validate_evidence_reference(evidence)?;
        }
        if entry.status.requires_real_platform_evidence()
            && !entry
                .acceptance_evidence
                .iter()
                .any(|evidence| evidence.scope == AcceptanceScope::RealPlatform)
        {
            return Err(SupportMatrixError::MissingRealPlatformEvidence(entry.key));
        }
    }
    Ok(())
}

pub fn validate_support_matrix_evidence(
    entries: &[SupportMatrixEntry],
    repository_root: &Path,
) -> Result<(), SupportMatrixError> {
    validate_support_matrix(entries)?;
    for evidence in entries.iter().flat_map(|entry| entry.acceptance_evidence) {
        let path = repository_root.join(evidence.reference);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| SupportMatrixError::MissingEvidenceFile(evidence.reference.to_owned()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SupportMatrixError::MissingEvidenceFile(
                evidence.reference.to_owned(),
            ));
        }
    }
    Ok(())
}

pub fn support_for(key: &SupportKey) -> Result<&'static SupportMatrixEntry, SupportLookupError> {
    SUPPORT_MATRIX
        .iter()
        .find(|entry| entry.key == *key)
        .ok_or(SupportLookupError::UndocumentedCombination)
}

#[must_use]
pub fn render_support_matrix_markdown() -> String {
    let mut output = String::new();
    writeln!(output, "# Platform Support Matrix").expect("writing to a String cannot fail");
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(
        output,
        "This document is rendered from the typed `SUPPORT_MATRIX` in `src/core/support.rs`."
    )
    .expect("writing to a String cannot fail");
    writeln!(
        output,
        "Repository-local tests validate the source and require this file to match it byte for byte."
    )
    .expect("writing to a String cannot fail");
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(output, "## Status vocabulary").expect("writing to a String cannot fail");
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(output, "| Status | Meaning |").expect("writing to a String cannot fail");
    writeln!(output, "|---|---|").expect("writing to a String cannot fail");
    for (status, meaning) in status_definitions() {
        writeln!(output, "| `{}` | {meaning} |", status.as_str())
            .expect("writing to a String cannot fail");
    }
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(output, "## Declared combinations").expect("writing to a String cannot fail");
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(output, "| Host OS | Host architecture | Provider | Accelerator | Guest OS | Guest architecture | Guest transport | Status | Acceptance evidence |").expect("writing to a String cannot fail");
    writeln!(output, "|---|---|---|---|---|---|---|---|---|")
        .expect("writing to a String cannot fail");
    for entry in SUPPORT_MATRIX {
        let evidence = if entry.acceptance_evidence.is_empty() {
            "none".to_owned()
        } else {
            entry
                .acceptance_evidence
                .iter()
                .map(|item| format!("{}: `{}`", item.scope.as_str(), item.reference))
                .collect::<Vec<_>>()
                .join("<br>")
        };
        writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} | {} | `{}` | {} |",
            entry.key.host_os.as_str(),
            architecture_name(entry.key.host_architecture),
            entry.key.provider.as_str(),
            entry.key.accelerator.as_str(),
            guest_os_name(entry.key.guest_os),
            architecture_name(entry.key.guest_architecture),
            entry.key.guest_transport.as_str(),
            entry.status.as_str(),
            evidence,
        )
        .expect("writing to a String cannot fail");
    }
    writeln!(output).expect("writing to a String cannot fail");
    writeln!(
        output,
        "An absent combination is undocumented and must fail closed; it never inherits support from a similar row."
    )
    .expect("writing to a String cannot fail");
    writeln!(
        output,
        "No current row is `supported` or `experimental`. Repository CI, mocks, fake protocols, and WSL2 development evidence cannot promote a row; those statuses require declared real-platform acceptance evidence."
    )
    .expect("writing to a String cannot fail");
    output
}

fn validate_evidence_reference(evidence: &AcceptanceEvidence) -> Result<(), SupportMatrixError> {
    let path = Path::new(evidence.reference);
    let safe_components = !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    let receipt_path = evidence.scope != AcceptanceScope::RealPlatform
        || (evidence.reference.starts_with("docs/receipts/")
            && evidence.reference.ends_with(".md"));
    if evidence.reference.is_empty() || !safe_components || !receipt_path {
        return Err(SupportMatrixError::InvalidEvidenceReference(
            evidence.reference.to_owned(),
        ));
    }
    Ok(())
}

fn combination_is_possible(key: &SupportKey) -> bool {
    let provider_accelerator = match key.provider {
        ProviderId::Hyperv => {
            key.host_os == HostOs::Windows && key.accelerator == Accelerator::HyperV
        }
        ProviderId::Qemu => match key.accelerator {
            Accelerator::HyperV => false,
            Accelerator::Whpx => key.host_os == HostOs::Windows,
            Accelerator::Kvm => key.host_os == HostOs::Linux,
            Accelerator::Hvf => key.host_os == HostOs::Macos,
            Accelerator::Tcg => true,
        },
    };
    let architecture =
        !key.accelerator.is_hardware() || key.host_architecture == key.guest_architecture;
    let transport = matches!(
        (key.provider, key.guest_os, key.guest_transport),
        (
            ProviderId::Hyperv,
            GuestOs::Windows,
            GuestTransportId::PowerShellDirect
        ) | (ProviderId::Qemu, GuestOs::Linux, GuestTransportId::Qga)
    );
    provider_accelerator && architecture && transport
}

fn support_key_rank(key: &SupportKey) -> (u8, u8, u8, u8, u8, u8, u8) {
    (
        host_os_rank(key.host_os),
        architecture_rank(key.host_architecture),
        provider_rank(key.provider),
        accelerator_rank(key.accelerator),
        guest_os_rank(key.guest_os),
        architecture_rank(key.guest_architecture),
        guest_transport_rank(key.guest_transport),
    )
}

const fn host_os_rank(value: HostOs) -> u8 {
    match value {
        HostOs::Windows => 0,
        HostOs::Linux => 1,
        HostOs::Macos => 2,
    }
}

const fn architecture_rank(value: Architecture) -> u8 {
    match value {
        Architecture::X86_64 => 0,
        Architecture::Aarch64 => 1,
    }
}

const fn provider_rank(value: ProviderId) -> u8 {
    match value {
        ProviderId::Hyperv => 0,
        ProviderId::Qemu => 1,
    }
}

const fn accelerator_rank(value: Accelerator) -> u8 {
    match value {
        Accelerator::HyperV => 0,
        Accelerator::Whpx => 1,
        Accelerator::Kvm => 2,
        Accelerator::Hvf => 3,
        Accelerator::Tcg => 4,
    }
}

const fn guest_os_rank(value: GuestOs) -> u8 {
    match value {
        GuestOs::Windows => 0,
        GuestOs::Linux => 1,
        GuestOs::Macos => 2,
    }
}

const fn guest_transport_rank(value: GuestTransportId) -> u8 {
    match value {
        GuestTransportId::PowerShellDirect => 0,
        GuestTransportId::Qga => 1,
        GuestTransportId::Ssh => 2,
    }
}

const fn architecture_name(value: Architecture) -> &'static str {
    match value {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
    }
}

const fn guest_os_name(value: GuestOs) -> &'static str {
    match value {
        GuestOs::Windows => "windows",
        GuestOs::Linux => "linux",
        GuestOs::Macos => "macos",
    }
}

const fn status_definitions() -> [(SupportStatus, &'static str); 5] {
    [
        (
            SupportStatus::Supported,
            "Accepted for the named release by declared real-platform evidence.",
        ),
        (
            SupportStatus::Experimental,
            "Real-platform evidence exists, but the path is not a release guarantee.",
        ),
        (
            SupportStatus::DevelopmentOnly,
            "Repository, mock, fake-protocol, or WSL2 evidence only; not real-platform acceptance.",
        ),
        (
            SupportStatus::Untested,
            "The path is intended, but its required real-platform acceptance is absent.",
        ),
        (
            SupportStatus::Unsupported,
            "The combination is rejected or not implemented and must not be selected.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPOSITORY_EVIDENCE: &[AcceptanceEvidence] = &[AcceptanceEvidence {
        scope: AcceptanceScope::RepositoryLocal,
        reference: "docs/roadmap.md",
    }];
    const MISSING_REAL_EVIDENCE: &[AcceptanceEvidence] = &[AcceptanceEvidence {
        scope: AcceptanceScope::RealPlatform,
        reference: "docs/receipts/missing-real-platform-acceptance.md",
    }];

    #[test]
    fn support_status_vocabulary_is_exact() {
        assert_eq!(
            SUPPORT_STATUS_VOCABULARY.map(SupportStatus::as_str),
            [
                "supported",
                "experimental",
                "development-only",
                "untested",
                "unsupported",
            ]
        );
        assert_eq!(
            serde_json::to_value(SUPPORT_STATUS_VOCABULARY).unwrap(),
            serde_json::json!([
                "supported",
                "experimental",
                "development-only",
                "untested",
                "unsupported",
            ])
        );
    }

    #[test]
    fn production_matrix_is_valid_conservative_and_documented_from_one_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        validate_support_matrix_evidence(SUPPORT_MATRIX, root).unwrap();
        assert!(SUPPORT_MATRIX.iter().all(|entry| !matches!(
            entry.status,
            SupportStatus::Supported | SupportStatus::Experimental
        )));
        assert_eq!(
            include_str!("../../docs/support-matrix.md"),
            render_support_matrix_markdown()
        );
    }

    #[test]
    fn duplicate_conflict_and_noncanonical_order_fail_validation() {
        let mut duplicate = SUPPORT_MATRIX.to_vec();
        duplicate.insert(1, duplicate[0]);
        assert!(matches!(
            validate_support_matrix(&duplicate),
            Err(SupportMatrixError::DuplicateKey(_))
        ));

        let mut reversed = SUPPORT_MATRIX.to_vec();
        reversed.swap(0, 1);
        assert!(matches!(
            validate_support_matrix(&reversed),
            Err(SupportMatrixError::NonCanonicalOrder(_))
        ));
    }

    #[test]
    fn impossible_or_unimplemented_combinations_cannot_be_promoted() {
        let mut entry = SUPPORT_MATRIX[1];
        assert_eq!(entry.status, SupportStatus::Unsupported);
        entry.status = SupportStatus::Untested;
        assert!(matches!(
            validate_support_matrix(&[entry]),
            Err(SupportMatrixError::ImpossibleCombination(_))
        ));

        let mut ssh = SUPPORT_MATRIX[4];
        assert_eq!(ssh.key.guest_transport, GuestTransportId::Ssh);
        ssh.status = SupportStatus::DevelopmentOnly;
        assert!(matches!(
            validate_support_matrix(&[ssh]),
            Err(SupportMatrixError::ImpossibleCombination(_))
        ));
    }

    #[test]
    fn repository_local_or_missing_evidence_cannot_claim_support() {
        let mut repository_only = SUPPORT_MATRIX[0];
        repository_only.status = SupportStatus::Supported;
        repository_only.acceptance_evidence = REPOSITORY_EVIDENCE;
        assert!(matches!(
            validate_support_matrix(&[repository_only]),
            Err(SupportMatrixError::MissingRealPlatformEvidence(_))
        ));

        let mut missing = SUPPORT_MATRIX[0];
        missing.status = SupportStatus::Supported;
        missing.acceptance_evidence = MISSING_REAL_EVIDENCE;
        assert!(validate_support_matrix(&[missing]).is_ok());
        assert!(matches!(
            validate_support_matrix_evidence(&[missing], Path::new(env!("CARGO_MANIFEST_DIR"))),
            Err(SupportMatrixError::MissingEvidenceFile(_))
        ));
    }

    #[test]
    fn undocumented_combination_is_never_inferred() {
        let key = SupportKey {
            host_os: HostOs::Linux,
            host_architecture: Architecture::Aarch64,
            provider: ProviderId::Qemu,
            accelerator: Accelerator::Kvm,
            guest_os: GuestOs::Linux,
            guest_architecture: Architecture::Aarch64,
            guest_transport: GuestTransportId::Qga,
        };
        assert_eq!(
            support_for(&key),
            Err(SupportLookupError::UndocumentedCombination)
        );
    }
}
