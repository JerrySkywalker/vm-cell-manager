pub mod powershell_direct;
pub mod qga;
pub mod ssh;

use std::fmt;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::core::cell::{CellId, CellRecord};
use crate::core::guest::GuestOperationId;
use crate::core::image::GuestOs;
use crate::providers::{ProviderPowerState, ProviderVm};
use crate::state::{CellRuntimeGuard, InstallationAuthority, MutationGuard};

pub const DEFAULT_READINESS_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_ACTION_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1_048_576;
pub const DEFAULT_MAX_COPY_BYTES: u64 = 67_108_864;
pub const MAX_ACTION_TIMEOUT_SECONDS: u64 = 3_600;
pub const MAX_OUTPUT_BYTES: u64 = 16_777_216;
pub const MAX_COPY_BYTES: u64 = 67_108_864;

pub struct GuestCredentials {
    username: String,
    password: Zeroizing<String>,
}

impl GuestCredentials {
    pub fn new(username: String, password: String) -> Result<Self, GuestIoError> {
        let password = Zeroizing::new(password);
        if username.trim().is_empty()
            || username.len() > 256
            || username.contains('\0')
            || username.chars().any(char::is_control)
        {
            return Err(GuestIoError::InvalidRequest(
                "guest username is outside the supported bounds",
            ));
        }
        if password.is_empty() || password.len() > 4096 || password.contains('\0') {
            return Err(GuestIoError::InvalidRequest(
                "guest password is outside the supported bounds",
            ));
        }
        Ok(Self { username, password })
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn password(&self) -> &str {
        self.password.as_str()
    }

    #[must_use]
    pub fn not_required() -> Self {
        Self {
            username: String::new(),
            password: Zeroizing::new(String::new()),
        }
    }
}

impl fmt::Debug for GuestCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GuestCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestPath(String);

impl GuestPath {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, GuestIoError> {
        let value = value.as_ref();
        if value.is_empty()
            || value.starts_with(['\\', '/'])
            || value.contains(':')
            || value.contains('\0')
            || value.chars().any(char::is_control)
        {
            return Err(GuestIoError::PathViolation);
        }
        let normalized = value.replace('/', "\\");
        let segments = normalized.split('\\').collect::<Vec<_>>();
        if segments.iter().any(|segment| {
            segment.is_empty()
                || matches!(*segment, "." | "..")
                || segment.ends_with(['.', ' '])
                || is_windows_device_name(segment)
        }) {
            return Err(GuestIoError::PathViolation);
        }
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('\\').next().unwrap_or(self.0.as_str())
    }

    #[must_use]
    pub fn as_posix(&self) -> String {
        self.0.replace('\\', "/")
    }
}

impl fmt::Display for GuestPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for GuestPath {
    type Err = GuestIoError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

fn is_windows_device_name(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    matches!(
        base.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverwritePolicy {
    Deny,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessPolicy {
    pub timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for ReadinessPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_READINESS_TIMEOUT_SECONDS),
            poll_interval: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestReadiness {
    Ready,
    GuestNotReady,
    AuthenticationFailed,
    SessionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestCommand {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: u64,
}

impl GuestCommand {
    pub fn validate(&self) -> Result<(), GuestIoError> {
        if self.program.trim().is_empty()
            || self.program.contains('\0')
            || self.args.iter().any(|argument| argument.contains('\0'))
        {
            return Err(GuestIoError::InvalidRequest(
                "guest program and arguments must contain no NUL",
            ));
        }
        if self.timeout.is_zero() || self.timeout > Duration::from_secs(MAX_ACTION_TIMEOUT_SECONDS)
        {
            return Err(GuestIoError::InvalidRequest(
                "guest action timeout is outside the supported bounds",
            ));
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > MAX_OUTPUT_BYTES {
            return Err(GuestIoError::InvalidRequest(
                "guest output limit is outside the supported bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub encoding: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub truncated: bool,
}

pub struct GuestActionAuthority<'a> {
    install_id: uuid::Uuid,
    cell_id: CellId,
    provider_id: &'a str,
    provider: &'a str,
    provider_name: &'a str,
    provider_marker: &'a str,
    configuration_path: &'a Path,
    overlay_path: &'a Path,
    cpu_count: u16,
    memory_mib: u64,
    _installation: &'a InstallationAuthority,
    _runtime: &'a CellRuntimeGuard,
    _mutation: &'a MutationGuard,
}

impl<'a> GuestActionAuthority<'a> {
    pub(crate) fn new(
        record: &'a CellRecord,
        expected: &'a ProviderVm,
        installation: &'a InstallationAuthority,
        runtime: &'a CellRuntimeGuard,
        mutation: &'a MutationGuard,
    ) -> Result<Self, GuestIoError> {
        let provider_id = record
            .provider_object
            .as_ref()
            .ok_or(GuestIoError::OwnershipChanged)?
            .id
            .as_str();
        let authority = Self {
            install_id: record.ownership.install_id,
            cell_id: record.id,
            provider_id,
            provider: &record.provider,
            provider_name: &record.ownership.provider_object_name,
            provider_marker: &record.ownership.provider_marker,
            configuration_path: &record.ownership.configuration_path,
            overlay_path: &record.ownership.overlay_path,
            cpu_count: record.spec.cpu_count,
            memory_mib: record.spec.memory_mib,
            _installation: installation,
            _runtime: runtime,
            _mutation: mutation,
        };
        authority.validate(expected)?;
        Ok(authority)
    }

    pub(crate) fn validate(&self, expected: &ProviderVm) -> Result<(), GuestIoError> {
        self._mutation
            .validate_filesystem_identity()
            .map_err(|_| GuestIoError::OwnershipChanged)?;
        if self.install_id != self._installation.record().install_id
            || self.cell_id != self._runtime.cell_id()
            || !matches!(self.provider, "hyperv" | "qemu")
            || expected.id != self.provider_id
            || expected.name != self.provider_name
            || expected.ownership_marker != self.provider_marker
            || !path_identity_equal(&expected.configuration_path, self.configuration_path)
            || expected.attached_disks.len() != 1
            || !path_identity_equal(&expected.attached_disks[0], self.overlay_path)
            || expected.network_adapter_count != 0
            || expected.cpu_count != self.cpu_count
            || expected.memory_mib != self.memory_mib
            || expected.power_state != ProviderPowerState::Running
        {
            return Err(GuestIoError::OwnershipChanged);
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn cell_id(&self) -> CellId {
        self.cell_id
    }

    #[must_use]
    pub(crate) fn provider(&self) -> &str {
        self.provider
    }

    #[must_use]
    pub(crate) fn provider_id(&self) -> &str {
        self.provider_id
    }

    #[must_use]
    pub(crate) fn configuration_path(&self) -> &Path {
        self.configuration_path
    }
}

fn path_identity_equal(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let mut value = path.to_string_lossy().replace('/', "\\");
        if value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
        {
            value = format!(r"\\{}", &value[8..]);
        } else if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
        {
            value = value[4..].to_owned();
        }
        while value.len() > 3 && value.ends_with('\\') {
            value.pop();
        }
        value
    };
    normalize(left).eq_ignore_ascii_case(&normalize(right))
}

pub trait GuestTransport: Send + Sync {
    fn name(&self) -> &'static str;

    fn supports(&self, _provider: &str, _guest_os: GuestOs) -> bool {
        false
    }

    fn probe_ready(
        &self,
        _authority: &GuestActionAuthority<'_>,
        _expected: &ProviderVm,
        _credentials: &GuestCredentials,
        _timeout: Duration,
    ) -> Result<GuestReadiness, GuestIoError> {
        Err(GuestIoError::NotImplemented(self.name()))
    }

    fn exec(
        &self,
        _authority: &GuestActionAuthority<'_>,
        _expected: &ProviderVm,
        _credentials: &GuestCredentials,
        _command: &GuestCommand,
    ) -> Result<GuestCommandResult, GuestIoError> {
        Err(GuestIoError::NotImplemented(self.name()))
    }

    fn copy_in(
        &self,
        _authority: &GuestActionAuthority<'_>,
        _expected: &ProviderVm,
        _credentials: &GuestCredentials,
        _action: GuestCopyInAction<'_>,
    ) -> Result<(), GuestIoError> {
        Err(GuestIoError::NotImplemented(self.name()))
    }

    fn copy_out(
        &self,
        _authority: &GuestActionAuthority<'_>,
        _expected: &ProviderVm,
        _credentials: &GuestCredentials,
        _action: GuestCopyOutAction<'_>,
    ) -> Result<Vec<u8>, GuestIoError> {
        Err(GuestIoError::NotImplemented(self.name()))
    }
}

pub struct GuestCopyInAction<'a> {
    pub operation_id: GuestOperationId,
    pub destination: &'a GuestPath,
    pub content: &'a [u8],
    pub overwrite: OverwritePolicy,
    pub timeout: Duration,
}

pub struct GuestCopyOutAction<'a> {
    pub operation_id: GuestOperationId,
    pub source: &'a GuestPath,
    pub max_bytes: u64,
    pub timeout: Duration,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GuestIoError {
    #[error("guest transport is not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("invalid guest operation: {0}")]
    InvalidRequest(&'static str),

    #[error("guest path violates the CellId-scoped workspace policy")]
    PathViolation,

    #[error("guest ownership precondition changed")]
    OwnershipChanged,

    #[error("guest is not ready for PowerShell Direct")]
    GuestNotReady,

    #[error("guest authentication failed")]
    AuthenticationFailed,

    #[error("guest session failed")]
    SessionFailed,

    #[error("guest transport timed out; guest side effects are unknown")]
    Timeout,

    #[error("guest output exceeded the configured limit")]
    OutputLimit,

    #[error("guest transport returned invalid structured data")]
    InvalidResponse,

    #[error("guest copy was interrupted; destination state is not assumed")]
    PartialCopy,

    #[error("guest transport failed with a redacted error")]
    Transport,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cell::{CellPhase, CellState};
    use crate::core::ownership::ProviderObjectIdentity;
    use crate::providers::{ProviderVm, ProviderVmIdentity};

    #[test]
    fn credential_debug_is_redacted() {
        let credentials =
            GuestCredentials::new("Administrator".to_owned(), "credential-sentinel".to_owned())
                .unwrap();
        let output = format!("{credentials:?}");
        assert!(!output.contains("credential-sentinel"));
        assert!(output.contains("<redacted>"));
    }

    #[test]
    fn guest_path_rejects_escape_ads_devices_and_ambiguity() {
        for path in [
            "",
            "..\\secret",
            ".\\secret",
            "C:\\secret",
            "\\\\server\\share",
            "safe\\..\\secret",
            "safe\\file:stream",
            "safe\\NUL.txt",
            "safe\\COM¹.txt",
            "safe\\LPT³",
            "safe\\trailing. ",
            "safe//double",
        ] {
            assert!(
                GuestPath::parse(path).is_err(),
                "accepted unsafe path: {path}"
            );
        }
        assert_eq!(
            GuestPath::parse("results/output.txt").unwrap().as_str(),
            "results\\output.txt"
        );
    }

    #[test]
    fn guest_authority_rejects_cpu_and_memory_drift() {
        let (_directory, state, installation, runtime, mut record) =
            crate::providers::test_mutation_fixture();
        let identity = ProviderVmIdentity {
            id: uuid::Uuid::new_v4().to_string(),
            name: record.ownership.provider_object_name.clone(),
        };
        record.provider_object = Some(ProviderObjectIdentity {
            id: identity.id.clone(),
            name: identity.name.clone(),
        });
        record.state = CellState::Running;
        record.phase = CellPhase::Ready;
        let expected = ProviderVm {
            id: identity.id,
            name: identity.name,
            power_state: ProviderPowerState::Running,
            ownership_marker: record.ownership.provider_marker.clone(),
            configuration_path: record.ownership.configuration_path.clone(),
            attached_disks: vec![record.ownership.overlay_path.clone()],
            network_adapter_count: 0,
            cpu_count: record.spec.cpu_count,
            memory_mib: record.spec.memory_mib,
        };
        let mutation = state.acquire_mutation_lock().unwrap();
        let authority =
            GuestActionAuthority::new(&record, &expected, &installation, &runtime, &mutation)
                .unwrap();

        let mut drifted = expected.clone();
        drifted.cpu_count += 1;
        assert_eq!(
            authority.validate(&drifted),
            Err(GuestIoError::OwnershipChanged)
        );
        drifted = expected.clone();
        drifted.memory_mib += 1;
        assert_eq!(
            authority.validate(&drifted),
            Err(GuestIoError::OwnershipChanged)
        );
    }
}
