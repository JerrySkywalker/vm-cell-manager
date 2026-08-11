//! Strict, versioned input contract for reproducible execution jobs.
//!
//! A job specification is an untrusted, human-authored TOML document.  It is
//! deliberately not an authority token: parsing and validation do not inspect
//! state, probe a provider, acquire a lock, or create a cell.  The engine
//! continues to own all lifecycle authority.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::cell::{MAX_CPU_COUNT, MAX_MEMORY_MIB, MIN_MEMORY_MIB};
use super::guest::{MAX_ARTIFACT_FILE_BYTES, MAX_ARTIFACT_FILES, MAX_ARTIFACT_TOTAL_BYTES};
use super::image::ImageId;
use super::support::ProviderId;
use crate::guest::{
    DEFAULT_ACTION_TIMEOUT_SECONDS, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_READINESS_TIMEOUT_SECONDS,
    GuestCommand, GuestPath, MAX_ACTION_TIMEOUT_SECONDS, MAX_COPY_BYTES, MAX_OUTPUT_BYTES,
    OverwritePolicy,
};

/// The only job-spec schema accepted by v0.4.
pub const JOB_SPEC_SCHEMA_VERSION: u32 = 1;
/// The stable identifier for the parsed job-spec contract.
pub const JOB_SPEC_CONTRACT: &str = "vmcell.job-spec.v1";
/// Input documents are intentionally small enough to parse before any provider work.
pub const MAX_JOB_SPEC_BYTES: u64 = 64 * 1024;
/// A job has a deliberately bounded, one-cell input fan-in.
pub const MAX_JOB_COPY_IN_FILES: usize = 16;
const MIN_TTL_SECONDS: u64 = 1;
const MAX_TTL_SECONDS: u64 = 31_536_000;

/// A validated job specification.  All fields are normalized before this type
/// is returned, but it is still non-authorizing data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub schema_version: u32,
    pub image: ImageId,
    pub cpu_count: u16,
    pub memory_mib: u64,
    pub ttl_seconds: Option<u64>,
    pub provider: Option<ProviderId>,
    pub accelerator: Option<JobAccelerator>,
    pub allow_tcg: bool,
    pub command: JobCommandSpec,
    pub readiness_timeout_seconds: u64,
    pub action_timeout_seconds: u64,
    pub max_output_bytes: u64,
    pub cleanup: JobCleanupPolicy,
    pub copy_in: Vec<JobCopyInSpec>,
    pub artifacts: JobArtifactSpec,
}

/// A guest command, kept separate from its transport-level execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// Explicit QEMU accelerator choices.  Absence means native selection; `auto`
/// is deliberately not an input value so that the public contract stays
/// unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobAccelerator {
    Whpx,
    Kvm,
    Hvf,
    Tcg,
}

/// Cleanup policy for the one cell created by a job invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobCleanupPolicy {
    pub keep: bool,
    pub keep_on_failure: bool,
}

/// A declarative guest copy-in request.  The source is lexically constrained
/// here; the engine will later re-open and identity-check it immediately before
/// use to close TOCTOU races.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCopyInSpec {
    pub source: PathBuf,
    pub destination: GuestPath,
    pub overwrite: OverwritePolicy,
    pub timeout_seconds: u64,
    pub max_bytes: u64,
}

/// Optional grouped artifact request for the one job cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobArtifactSpec {
    pub sources: Vec<GuestPath>,
    pub timeout_seconds: u64,
    pub max_bytes_per_file: u64,
}

/// The canonicalized file and its validated contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedJobSpec {
    path: PathBuf,
    /// SHA-256 of the exact bounded source bytes.  It binds a later plan or
    /// result to an input document without serializing the document itself.
    source_sha256: String,
    spec: JobSpec,
}

impl LoadedJobSpec {
    /// Canonical path of the ordinary job document accepted by the loader.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// SHA-256 of the exact bounded source bytes accepted by the loader.
    #[must_use]
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    /// Validated, normalized non-authorizing job configuration.
    #[must_use]
    pub fn spec(&self) -> &JobSpec {
        &self.spec
    }

    #[cfg(test)]
    pub(crate) fn from_validated_parts_for_test(
        path: PathBuf,
        source_sha256: String,
        spec: JobSpec,
    ) -> Self {
        Self {
            path,
            source_sha256,
            spec,
        }
    }
}

#[derive(Debug, Error)]
pub enum JobSpecError {
    #[error("job specification file was not found")]
    NotFound,
    #[error("job specification path is not an ordinary private file")]
    UnsafePath,
    #[error("job specification exceeds the bounded size limit")]
    TooLarge,
    #[error("job specification is not valid UTF-8")]
    InvalidEncoding,
    #[error("job specification is not valid TOML")]
    Toml,
    #[error("unsupported job specification schema version {actual}; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error("job specification is outside the supported policy: {0}")]
    InvalidValue(&'static str),
    #[error("job specification I/O failed: {0}")]
    Io(#[source] std::io::Error),
}

impl JobSpecError {
    /// Stable classification for the automation error envelope added by later
    /// slices.  The prose error intentionally carries no parsed secrets.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "vmcell.job_spec.not_found",
            Self::UnsafePath => "vmcell.job_spec.unsafe_path",
            Self::TooLarge => "vmcell.job_spec.too_large",
            Self::InvalidEncoding | Self::Toml => "vmcell.job_spec.invalid_document",
            Self::UnsupportedSchema { .. } => "vmcell.job_spec.unsupported_schema",
            Self::InvalidValue(_) => "vmcell.job_spec.invalid_value",
            Self::Io(_) => "vmcell.job_spec.io",
        }
    }
}

/// Parse and validate an in-memory TOML document without accessing any host or
/// provider authority.
pub fn parse_job_spec(input: &str) -> Result<JobSpec, JobSpecError> {
    let raw = toml::from_str::<RawJobSpec>(input).map_err(|_| JobSpecError::Toml)?;
    JobSpec::try_from(raw)
}

/// Open, bound, parse, and validate an explicit job-spec file.  This follows
/// the same no-follow/private-file convention as the user configuration loader.
pub fn load_job_spec(path: &Path) -> Result<LoadedJobSpec, JobSpecError> {
    let path = absolute_existing_job_spec_path(path)?;
    let mut file = open_job_spec_file(&path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_JOB_SPEC_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(JobSpecError::Io)?;
    if bytes.len() as u64 > MAX_JOB_SPEC_BYTES {
        return Err(JobSpecError::TooLarge);
    }
    let source_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let input = std::str::from_utf8(&bytes).map_err(|_| JobSpecError::InvalidEncoding)?;
    let spec = parse_job_spec(input)?;
    Ok(LoadedJobSpec {
        path,
        source_sha256,
        spec,
    })
}

impl JobSpec {
    /// Convert the command input to the existing guest command shape only
    /// after validation has succeeded.  This does not authorize execution.
    #[must_use]
    pub fn guest_command(&self) -> GuestCommand {
        GuestCommand {
            program: self.command.program.clone(),
            args: self.command.args.clone(),
            timeout: std::time::Duration::from_secs(self.action_timeout_seconds),
            max_output_bytes: self.max_output_bytes,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobSpec {
    schema_version: u32,
    image: ImageId,
    cpu_count: u16,
    memory_mib: u64,
    #[serde(default)]
    ttl_seconds: Option<u64>,
    #[serde(default)]
    provider: Option<ProviderId>,
    #[serde(default)]
    accelerator: Option<JobAccelerator>,
    #[serde(default)]
    allow_tcg: bool,
    command: RawJobCommandSpec,
    #[serde(default = "default_readiness_timeout_seconds")]
    readiness_timeout_seconds: u64,
    #[serde(default = "default_action_timeout_seconds")]
    action_timeout_seconds: u64,
    #[serde(default = "default_max_output_bytes")]
    max_output_bytes: u64,
    cleanup: RawJobCleanupPolicy,
    #[serde(default)]
    copy_in: Vec<RawJobCopyInSpec>,
    #[serde(default)]
    artifacts: RawJobArtifactSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobCommandSpec {
    program: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobCleanupPolicy {
    #[serde(default)]
    keep: bool,
    #[serde(default)]
    keep_on_failure: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobCopyInSpec {
    source: PathBuf,
    destination: String,
    #[serde(default = "default_overwrite_policy")]
    overwrite: OverwritePolicy,
    #[serde(default = "default_action_timeout_seconds")]
    timeout_seconds: u64,
    #[serde(default = "default_max_copy_bytes")]
    max_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawJobArtifactSpec {
    sources: Vec<String>,
    #[serde(default = "default_action_timeout_seconds")]
    timeout_seconds: u64,
    #[serde(default = "default_max_artifact_file_bytes")]
    max_bytes_per_file: u64,
}

impl Default for RawJobArtifactSpec {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            timeout_seconds: default_action_timeout_seconds(),
            max_bytes_per_file: default_max_artifact_file_bytes(),
        }
    }
}

impl TryFrom<RawJobSpec> for JobSpec {
    type Error = JobSpecError;

    fn try_from(raw: RawJobSpec) -> Result<Self, Self::Error> {
        if raw.schema_version != JOB_SPEC_SCHEMA_VERSION {
            return Err(JobSpecError::UnsupportedSchema {
                expected: JOB_SPEC_SCHEMA_VERSION,
                actual: raw.schema_version,
            });
        }
        if !(1..=MAX_CPU_COUNT).contains(&raw.cpu_count) {
            return Err(JobSpecError::InvalidValue(
                "cpu_count is outside the engine-supported range",
            ));
        }
        if !(MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&raw.memory_mib) {
            return Err(JobSpecError::InvalidValue(
                "memory_mib is outside the engine-supported range",
            ));
        }
        if raw
            .ttl_seconds
            .is_some_and(|ttl| !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl))
        {
            return Err(JobSpecError::InvalidValue(
                "ttl_seconds is outside the engine-supported range",
            ));
        }
        validate_timeout(
            raw.readiness_timeout_seconds,
            "readiness_timeout_seconds is outside the engine-supported range",
        )?;
        validate_timeout(
            raw.action_timeout_seconds,
            "action_timeout_seconds is outside the engine-supported range",
        )?;
        if raw.max_output_bytes == 0 || raw.max_output_bytes > MAX_OUTPUT_BYTES {
            return Err(JobSpecError::InvalidValue(
                "max_output_bytes is outside the engine-supported range",
            ));
        }
        validate_accelerator_intent(raw.provider, raw.accelerator, raw.allow_tcg)?;

        let command = JobCommandSpec {
            program: raw.command.program,
            args: raw.command.args,
        };
        GuestCommand {
            program: command.program.clone(),
            args: command.args.clone(),
            timeout: std::time::Duration::from_secs(raw.action_timeout_seconds),
            max_output_bytes: raw.max_output_bytes,
        }
        .validate()
        .map_err(|_| JobSpecError::InvalidValue("command is outside the engine-supported range"))?;

        if raw.copy_in.len() > MAX_JOB_COPY_IN_FILES {
            return Err(JobSpecError::InvalidValue(
                "copy_in exceeds the engine-supported file count",
            ));
        }
        let mut destinations = BTreeSet::new();
        let mut copy_in = Vec::with_capacity(raw.copy_in.len());
        for input in raw.copy_in {
            validate_relative_input_path(&input.source)?;
            validate_timeout(
                input.timeout_seconds,
                "copy_in timeout_seconds is outside the engine-supported range",
            )?;
            if input.max_bytes == 0 || input.max_bytes > MAX_COPY_BYTES {
                return Err(JobSpecError::InvalidValue(
                    "copy_in max_bytes is outside the engine-supported range",
                ));
            }
            let destination = GuestPath::parse(&input.destination).map_err(|_| {
                JobSpecError::InvalidValue("copy_in destination is outside the supported policy")
            })?;
            if !destinations.insert(destination.as_str().to_owned()) {
                return Err(JobSpecError::InvalidValue(
                    "copy_in destinations must be unique",
                ));
            }
            copy_in.push(JobCopyInSpec {
                source: input.source,
                destination,
                overwrite: input.overwrite,
                timeout_seconds: input.timeout_seconds,
                max_bytes: input.max_bytes,
            });
        }

        if raw.artifacts.sources.len() > MAX_ARTIFACT_FILES {
            return Err(JobSpecError::InvalidValue(
                "artifact sources exceed the engine-supported file count",
            ));
        }
        validate_timeout(
            raw.artifacts.timeout_seconds,
            "artifacts timeout_seconds is outside the engine-supported range",
        )?;
        if raw.artifacts.max_bytes_per_file == 0
            || raw.artifacts.max_bytes_per_file > MAX_ARTIFACT_FILE_BYTES
        {
            return Err(JobSpecError::InvalidValue(
                "artifacts max_bytes_per_file is outside the engine-supported range",
            ));
        }
        let maximum_total = (raw.artifacts.sources.len() as u64)
            .checked_mul(raw.artifacts.max_bytes_per_file)
            .ok_or(JobSpecError::InvalidValue(
                "artifact size calculation overflow",
            ))?;
        if maximum_total > MAX_ARTIFACT_TOTAL_BYTES {
            return Err(JobSpecError::InvalidValue(
                "artifact total size exceeds the engine-supported range",
            ));
        }
        let mut artifact_paths = BTreeSet::new();
        let mut sources = Vec::with_capacity(raw.artifacts.sources.len());
        for source in raw.artifacts.sources {
            let source = GuestPath::parse(&source).map_err(|_| {
                JobSpecError::InvalidValue("artifact source is outside the supported policy")
            })?;
            if !artifact_paths.insert(source.as_str().to_owned()) {
                return Err(JobSpecError::InvalidValue(
                    "artifact sources must be unique",
                ));
            }
            sources.push(source);
        }

        Ok(Self {
            schema_version: raw.schema_version,
            image: raw.image,
            cpu_count: raw.cpu_count,
            memory_mib: raw.memory_mib,
            ttl_seconds: raw.ttl_seconds,
            provider: raw.provider,
            accelerator: raw.accelerator,
            allow_tcg: raw.allow_tcg,
            command,
            readiness_timeout_seconds: raw.readiness_timeout_seconds,
            action_timeout_seconds: raw.action_timeout_seconds,
            max_output_bytes: raw.max_output_bytes,
            cleanup: JobCleanupPolicy {
                keep: raw.cleanup.keep,
                keep_on_failure: raw.cleanup.keep_on_failure,
            },
            copy_in,
            artifacts: JobArtifactSpec {
                sources,
                timeout_seconds: raw.artifacts.timeout_seconds,
                max_bytes_per_file: raw.artifacts.max_bytes_per_file,
            },
        })
    }
}

fn validate_timeout(value: u64, message: &'static str) -> Result<(), JobSpecError> {
    if !(1..=MAX_ACTION_TIMEOUT_SECONDS).contains(&value) {
        return Err(JobSpecError::InvalidValue(message));
    }
    Ok(())
}

fn validate_accelerator_intent(
    provider: Option<ProviderId>,
    accelerator: Option<JobAccelerator>,
    allow_tcg: bool,
) -> Result<(), JobSpecError> {
    if provider == Some(ProviderId::Hyperv) && accelerator.is_some() {
        return Err(JobSpecError::InvalidValue(
            "accelerator may be selected only with the qemu provider",
        ));
    }
    if (accelerator == Some(JobAccelerator::Tcg)) != allow_tcg {
        return Err(JobSpecError::InvalidValue(
            "TCG requires accelerator = tcg and allow_tcg = true together",
        ));
    }
    Ok(())
}

fn validate_relative_input_path(path: &Path) -> Result<(), JobSpecError> {
    let rendered = path.to_string_lossy();
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || rendered.contains(['\\', ':'])
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(JobSpecError::InvalidValue(
            "copy_in source must be a non-empty relative path without dot segments",
        ));
    }
    Ok(())
}

fn absolute_existing_job_spec_path(path: &Path) -> Result<PathBuf, JobSpecError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(JobSpecError::Io)?
            .join(path)
    };
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            ensure_no_reparse_ancestors(&path)?;
            path.canonicalize().map_err(JobSpecError::Io)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(JobSpecError::NotFound),
        Err(error) => Err(JobSpecError::Io(error)),
    }
}

fn open_job_spec_file(path: &Path) -> Result<File, JobSpecError> {
    ensure_no_reparse_ancestors(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let file = options.open(path).map_err(JobSpecError::Io)?;
    let metadata = file.metadata().map_err(JobSpecError::Io)?;
    if !metadata.is_file() || path_is_reparse(path)? {
        return Err(JobSpecError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(JobSpecError::UnsafePath);
        }
    }
    Ok(file)
}

fn ensure_no_reparse_ancestors(path: &Path) -> Result<(), JobSpecError> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() || !ancestor.exists() {
            continue;
        }
        if path_is_reparse(ancestor)? {
            return Err(JobSpecError::UnsafePath);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn path_is_reparse(path: &Path) -> Result<bool, JobSpecError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let metadata = fs::symlink_metadata(path).map_err(JobSpecError::Io)?;
    Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn path_is_reparse(path: &Path) -> Result<bool, JobSpecError> {
    let metadata = fs::symlink_metadata(path).map_err(JobSpecError::Io)?;
    Ok(metadata.file_type().is_symlink())
}

const fn default_readiness_timeout_seconds() -> u64 {
    DEFAULT_READINESS_TIMEOUT_SECONDS
}

const fn default_action_timeout_seconds() -> u64 {
    DEFAULT_ACTION_TIMEOUT_SECONDS
}

const fn default_max_output_bytes() -> u64 {
    DEFAULT_MAX_OUTPUT_BYTES
}

const fn default_max_copy_bytes() -> u64 {
    MAX_COPY_BYTES
}

const fn default_max_artifact_file_bytes() -> u64 {
    MAX_ARTIFACT_FILE_BYTES
}

const fn default_overwrite_policy() -> OverwritePolicy {
    OverwritePolicy::Deny
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const VALID_SPEC: &str = r#"
schema_version = 1
image = "linux-qemu"
cpu_count = 2
memory_mib = 2048
ttl_seconds = 60
provider = "qemu"
accelerator = "kvm"

[command]
program = "/usr/bin/printf"
args = ["hello"]

[cleanup]
keep = false
keep_on_failure = true

[[copy_in]]
source = "inputs/message.txt"
destination = "inputs/message.txt"
overwrite = "deny"
timeout_seconds = 30
max_bytes = 1024

[artifacts]
sources = ["results/output.txt"]
timeout_seconds = 30
max_bytes_per_file = 4096
"#;

    #[test]
    fn parses_a_strict_versioned_toml_job_spec() {
        let spec = parse_job_spec(VALID_SPEC).unwrap();
        assert_eq!(spec.schema_version, JOB_SPEC_SCHEMA_VERSION);
        assert_eq!(spec.image.as_str(), "linux-qemu");
        assert_eq!(spec.cpu_count, 2);
        assert_eq!(spec.memory_mib, 2048);
        assert_eq!(spec.ttl_seconds, Some(60));
        assert_eq!(spec.provider, Some(ProviderId::Qemu));
        assert_eq!(spec.accelerator, Some(JobAccelerator::Kvm));
        assert!(!spec.allow_tcg);
        assert_eq!(spec.command.program, "/usr/bin/printf");
        assert_eq!(spec.command.args, ["hello"]);
        assert_eq!(spec.copy_in.len(), 1);
        assert_eq!(spec.copy_in[0].destination.as_str(), "inputs\\message.txt");
        assert_eq!(spec.artifacts.sources[0].as_str(), "results\\output.txt");
        assert!(spec.cleanup.keep_on_failure);
        assert_eq!(
            spec.guest_command().timeout,
            std::time::Duration::from_secs(DEFAULT_ACTION_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn rejects_unknown_authority_and_provisioning_fields() {
        for field in [
            "credential = \"secret\"",
            "password = \"secret\"",
            "provision = [\"apt install\"]",
            "network = \"bridge\"",
            "adopt_cell = \"foreign\"",
            "scheduler = \"later\"",
        ] {
            let input = format!("{VALID_SPEC}\n{field}\n");
            assert!(
                matches!(parse_job_spec(&input), Err(JobSpecError::Toml)),
                "{field}"
            );
        }

        let error = parse_job_spec("schema_version = 1\npassword = \"not-for-display").unwrap_err();
        assert_eq!(error.to_string(), "job specification is not valid TOML");
        assert!(!format!("{error:?}").contains("not-for-display"));
    }

    #[test]
    fn rejects_unknown_schema_and_unpaired_tcg_opt_in() {
        let future = VALID_SPEC.replacen("schema_version = 1", "schema_version = 2", 1);
        assert!(matches!(
            parse_job_spec(&future),
            Err(JobSpecError::UnsupportedSchema { actual: 2, .. })
        ));

        for replacement in [
            ("accelerator = \"kvm\"", "accelerator = \"tcg\""),
            (
                "accelerator = \"kvm\"",
                "accelerator = \"tcg\"\nallow_tcg = false",
            ),
            ("accelerator = \"kvm\"", "allow_tcg = true"),
        ] {
            let input = VALID_SPEC.replacen(replacement.0, replacement.1, 1);
            assert!(matches!(
                parse_job_spec(&input),
                Err(JobSpecError::InvalidValue(_))
            ));
        }

        let allowed = VALID_SPEC.replacen(
            "accelerator = \"kvm\"",
            "accelerator = \"tcg\"\nallow_tcg = true",
            1,
        );
        assert!(parse_job_spec(&allowed).is_ok());
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_copy_and_artifact_paths() {
        for replacement in [
            (
                "source = \"inputs/message.txt\"",
                "source = \"../message.txt\"",
            ),
            (
                "source = \"inputs/message.txt\"",
                "source = \"/host/message.txt\"",
            ),
            (
                "source = \"inputs/message.txt\"",
                "source = \"C:/host/message.txt\"",
            ),
            (
                "source = \"inputs/message.txt\"",
                "source = '..\\message.txt'",
            ),
            (
                "destination = \"inputs/message.txt\"",
                "destination = \"../message.txt\"",
            ),
            (
                "sources = [\"results/output.txt\"]",
                "sources = [\"results/output.txt\", \"results/output.txt\"]",
            ),
        ] {
            let input = VALID_SPEC.replacen(replacement.0, replacement.1, 1);
            assert!(matches!(
                parse_job_spec(&input),
                Err(JobSpecError::InvalidValue(_))
            ));
        }

        let duplicate_copy = format!(
            "{VALID_SPEC}\n[[copy_in]]\nsource = \"inputs/other.txt\"\ndestination = \"inputs/message.txt\"\n"
        );
        assert!(matches!(
            parse_job_spec(&duplicate_copy),
            Err(JobSpecError::InvalidValue(_))
        ));
    }

    #[test]
    fn rejects_resource_and_timeout_bounds_before_execution() {
        for replacement in [
            ("cpu_count = 2", "cpu_count = 0"),
            ("memory_mib = 2048", "memory_mib = 511"),
            ("ttl_seconds = 60", "ttl_seconds = 0"),
            ("timeout_seconds = 30", "timeout_seconds = 0"),
            ("max_bytes = 1024", "max_bytes = 0"),
            ("max_bytes_per_file = 4096", "max_bytes_per_file = 0"),
        ] {
            let input = VALID_SPEC.replacen(replacement.0, replacement.1, 1);
            assert!(matches!(
                parse_job_spec(&input),
                Err(JobSpecError::InvalidValue(_))
            ));
        }
    }

    #[test]
    fn optional_artifact_section_uses_bounded_defaults() {
        let spec = parse_job_spec(
            r#"
schema_version = 1
image = "linux-qemu"
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
        assert!(spec.artifacts.sources.is_empty());
        assert_eq!(
            spec.artifacts.timeout_seconds,
            DEFAULT_ACTION_TIMEOUT_SECONDS
        );
        assert_eq!(spec.artifacts.max_bytes_per_file, MAX_ARTIFACT_FILE_BYTES);
    }

    #[test]
    fn loader_is_required_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.toml");
        assert!(matches!(
            load_job_spec(&missing),
            Err(JobSpecError::NotFound)
        ));

        let path = directory.path().join("job.toml");
        fs::write(&path, VALID_SPEC).unwrap();
        set_private_test_permissions(&path);
        let loaded = load_job_spec(&path).unwrap();
        assert_eq!(loaded.spec.image.as_str(), "linux-qemu");
        assert_eq!(loaded.path, path.canonicalize().unwrap());
        assert_eq!(loaded.source_sha256.len(), 64);

        fs::write(&path, vec![b' '; MAX_JOB_SPEC_BYTES as usize + 1]).unwrap();
        assert!(matches!(load_job_spec(&path), Err(JobSpecError::TooLarge)));
    }

    #[cfg(unix)]
    #[test]
    fn unix_loader_requires_a_private_non_symlink_file() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("job.toml");
        fs::write(&path, VALID_SPEC).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            load_job_spec(&path),
            Err(JobSpecError::UnsafePath)
        ));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = directory.path().join("linked.toml");
        symlink(&path, &link).unwrap();
        assert!(matches!(
            load_job_spec(&link),
            Err(JobSpecError::UnsafePath)
        ));
    }

    #[cfg(unix)]
    fn set_private_test_permissions(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(windows)]
    fn set_private_test_permissions(_path: &Path) {}
}
