pub mod protocol;

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use self::protocol::{ControlEndpoint, QmpClient, connect_endpoint};
use crate::core::capability::ProviderCapabilities;
use crate::process::{ProcessError, run_bounded};
use crate::providers::{
    ClaimVmRequest, ConfigureVmRequest, CreateOverlayRequest, CreateVmRequest, LocalVmProvider,
    ProviderError, ProviderImageInfo, ProviderMutationAuthority, ProviderPowerState, ProviderProbe,
    ProviderProbeStatus, ProviderVm, ProviderVmIdentity, QemuDefinition, VmLookup,
};

const QEMU_CONFIG_SCHEMA: u32 = 1;
const QEMU_CONFIG_MAX_BYTES: usize = 256 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_LIMIT: usize = 1024 * 1024;
#[cfg(unix)]
const UNIX_CONTROL_ENDPOINT_LIMIT: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KvmDeviceStatus {
    #[cfg(not(target_os = "linux"))]
    NotApplicable,
    #[cfg(any(target_os = "linux", test))]
    Usable,
    #[cfg(any(target_os = "linux", test))]
    Missing,
    #[cfg(any(target_os = "linux", test))]
    PermissionDenied,
    #[cfg(any(target_os = "linux", test))]
    NotCharacterDevice,
    #[cfg(any(target_os = "linux", test))]
    IdentityChanged,
    #[cfg(any(target_os = "linux", test))]
    Unavailable,
}

impl KvmDeviceStatus {
    fn usable(self) -> bool {
        match self {
            #[cfg(not(target_os = "linux"))]
            Self::NotApplicable => true,
            #[cfg(any(target_os = "linux", test))]
            Self::Usable => true,
            #[cfg(any(target_os = "linux", test))]
            _ => false,
        }
    }

    fn diagnostic(self) -> &'static str {
        match self {
            #[cfg(not(target_os = "linux"))]
            Self::NotApplicable => "not applicable on this host",
            #[cfg(any(target_os = "linux", test))]
            Self::Usable => "read-write usable by the current identity",
            #[cfg(any(target_os = "linux", test))]
            Self::Missing => "/dev/kvm is missing",
            #[cfg(any(target_os = "linux", test))]
            Self::PermissionDenied => "/dev/kvm is not read-write usable by the current identity",
            #[cfg(any(target_os = "linux", test))]
            Self::NotCharacterDevice => "/dev/kvm is not an ordinary character device",
            #[cfg(any(target_os = "linux", test))]
            Self::IdentityChanged => "/dev/kvm identity changed while it was opened",
            #[cfg(any(target_os = "linux", test))]
            Self::Unavailable => "/dev/kvm could not be opened read-write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QemuCommandOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QemuSpawnReceipt {
    pub process_id: u32,
    pub process_start_token: u64,
    pub executable_sha256: String,
}

pub trait QemuCommandExecutor: Send + Sync {
    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        timeout: Duration,
        output_limit: usize,
    ) -> Result<QemuCommandOutput, ProviderError>;

    fn spawn_vm(
        &self,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<QemuSpawnReceipt, ProviderError>;

    fn process_matches(
        &self,
        process_id: u32,
        start_token: u64,
        program: &OsStr,
        command_sha256: &str,
        executable_sha256: &str,
    ) -> bool;

    fn process_absence_proven(&self, process_id: u32, start_token: u64) -> bool;

    fn process_group_absence_proven(&self, process_group_id: u32) -> bool;

    fn connect_qmp(
        &self,
        endpoint: &ControlEndpoint,
        timeout: Duration,
    ) -> Result<Box<dyn protocol::ReadWrite>, ProviderError>;
}

pub struct SystemQemuExecutor;

impl QemuCommandExecutor for SystemQemuExecutor {
    fn run(
        &self,
        program: &OsStr,
        args: &[OsString],
        timeout: Duration,
        output_limit: usize,
    ) -> Result<QemuCommandOutput, ProviderError> {
        let mut command = Command::new(program);
        command.args(args);
        let output = run_bounded(&mut command, &[], timeout, output_limit)
            .map_err(process_error_to_provider)?;
        Ok(QemuCommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn spawn_vm(
        &self,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<QemuSpawnReceipt, ProviderError> {
        let executable_sha256 = ordinary_file_sha256(Path::new(program))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached_process(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| ProviderError::Command(format!("failed to start QEMU: {error}")))?;
        let process_id = child.id();
        let Some(process_start_token) = process_start_token(process_id).filter(|token| *token != 0)
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::Command(
                "QEMU process start identity was unavailable".to_owned(),
            ));
        };
        if !process_matches(
            process_id,
            process_start_token,
            program,
            &argument_digest(args),
            &executable_sha256,
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::OwnershipChanged(
                "spawned QEMU process identity did not match its executable and start instance"
                    .to_owned(),
            ));
        }
        start_process_reaper(child)?;
        Ok(QemuSpawnReceipt {
            process_id,
            process_start_token,
            executable_sha256,
        })
    }

    fn process_matches(
        &self,
        process_id: u32,
        start_token: u64,
        program: &OsStr,
        command_sha256: &str,
        executable_sha256: &str,
    ) -> bool {
        process_matches(
            process_id,
            start_token,
            program,
            command_sha256,
            executable_sha256,
        )
    }

    fn process_absence_proven(&self, process_id: u32, start_token: u64) -> bool {
        process_absence_proven(process_id, start_token)
    }

    fn process_group_absence_proven(&self, process_group_id: u32) -> bool {
        process_group_absence_proven(process_group_id)
    }

    fn connect_qmp(
        &self,
        endpoint: &ControlEndpoint,
        timeout: Duration,
    ) -> Result<Box<dyn protocol::ReadWrite>, ProviderError> {
        connect_endpoint(endpoint, timeout)
    }
}

pub struct QemuProvider<E = SystemQemuExecutor> {
    executor: E,
    runtime_root: PathBuf,
    system_binary: OsString,
    image_binary: OsString,
}

impl QemuProvider<SystemQemuExecutor> {
    #[must_use]
    pub fn system(state_root: impl Into<PathBuf>) -> Self {
        Self::new(
            SystemQemuExecutor,
            state_root.into().join("runtime"),
            resolve_executable(system_binary_name()),
            resolve_executable(OsString::from("qemu-img")),
        )
    }

    #[must_use]
    pub fn probe_only() -> Self {
        Self::system(PathBuf::from("."))
    }
}

impl<E> QemuProvider<E> {
    pub(crate) fn new(
        executor: E,
        runtime_root: PathBuf,
        system_binary: OsString,
        image_binary: OsString,
    ) -> Self {
        Self {
            executor,
            runtime_root,
            system_binary,
            image_binary,
        }
    }
}

impl<E: QemuCommandExecutor> QemuProvider<E> {
    fn run_checked(
        &self,
        program: &OsStr,
        args: &[OsString],
    ) -> Result<QemuCommandOutput, ProviderError> {
        let output = self
            .executor
            .run(program, args, COMMAND_TIMEOUT, PROBE_OUTPUT_LIMIT)?;
        if output.success {
            Ok(output)
        } else {
            Err(ProviderError::Command(redacted_command_failure()))
        }
    }

    fn accelerator_inventory(
        &self,
    ) -> Result<(Vec<String>, Option<KvmDeviceStatus>, bool), ProviderError> {
        let output = self.run_checked(
            &self.system_binary,
            &[OsString::from("-accel"), OsString::from("help")],
        )?;
        let text = std::str::from_utf8(&output.stdout).map_err(|_| {
            ProviderError::InvalidResponse("QEMU accelerator output was not UTF-8".to_owned())
        })?;
        let compiled = text
            .lines()
            .map(str::trim)
            .filter(|line| matches!(*line, "whpx" | "kvm" | "hvf" | "tcg"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let kvm_advertised = compiled.iter().any(|value| value == "kvm");
        let kvm_status = kvm_advertised.then(kvm_device_status);
        Ok((
            filter_usable_accelerators(compiled, kvm_status.is_none_or(KvmDeviceStatus::usable)),
            kvm_status,
            kvm_advertised,
        ))
    }

    fn accelerators(&self) -> Result<Vec<String>, ProviderError> {
        self.accelerator_inventory()
            .map(|(accelerators, _, _)| accelerators)
    }

    fn select_accelerator(&self, request: &CreateVmRequest) -> Result<String, ProviderError> {
        let available = self.accelerators()?;
        let requested = request.accelerator.as_deref().unwrap_or("auto");
        let hardware = host_accelerator();
        let selected = match requested {
            "auto" if available.iter().any(|value| value == hardware) => hardware,
            "auto" => {
                return Err(ProviderError::Unsupported {
                    provider: "qemu",
                    operation: "hardware_acceleration_or_explicit_tcg",
                });
            }
            "tcg" if request.allow_tcg && available.iter().any(|value| value == "tcg") => "tcg",
            value if value == hardware && available.iter().any(|item| item == value) => value,
            _ => {
                return Err(ProviderError::Command(
                    "requested QEMU accelerator is unavailable or TCG was not explicitly allowed"
                        .to_owned(),
                ));
            }
        };
        Ok(selected.to_owned())
    }

    fn config_path(configuration_path: &Path) -> PathBuf {
        configuration_path.join("vm.json")
    }

    fn config_for_lookup(&self, lookup: &VmLookup) -> Result<Option<QemuVmConfig>, ProviderError> {
        let id = match lookup {
            VmLookup::Id(id) => Uuid::parse_str(id).ok(),
            VmLookup::Name(name) => name
                .strip_prefix("vmcell-")
                .and_then(|value| Uuid::parse_str(value).ok()),
        };
        let Some(id) = id else {
            return Ok(None);
        };
        let path = self
            .runtime_root
            .join(id.to_string())
            .join("qemu")
            .join("vm.json");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let config = read_config(&path)?;
                let matches = match lookup {
                    VmLookup::Id(value) => {
                        Uuid::parse_str(value).ok() == Uuid::parse_str(&config.id).ok()
                    }
                    VmLookup::Name(value) => value == &config.name,
                };
                if matches { Ok(Some(config)) } else { Ok(None) }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ProviderError::Command(format!(
                "failed to read QEMU configuration: {error}"
            ))),
        }
    }

    fn inspect_config(&self, config: &QemuVmConfig) -> Result<ProviderVm, ProviderError> {
        let qmp_deadline = std::time::Instant::now() + CONTROL_TIMEOUT;
        let qmp = self
            .executor
            .connect_qmp(&config.qmp, Duration::from_millis(100));
        let power_state = match qmp {
            Ok(stream) => {
                self.validate_live_process_receipt(config)?;
                let mut qmp =
                    QmpClient::negotiate(stream, remaining_provider_duration(qmp_deadline)?)?;
                validate_qmp_identity(&mut qmp, config)?;
                let status = qmp.execute("query-status", None)?;
                match status.get("status").and_then(Value::as_str) {
                    Some("running") => ProviderPowerState::Running,
                    Some("paused" | "prelaunch" | "inmigrate") => ProviderPowerState::Paused,
                    Some(value) => ProviderPowerState::Other(value.to_owned()),
                    None => {
                        return Err(ProviderError::InvalidResponse(
                            "QMP status response was invalid".to_owned(),
                        ));
                    }
                }
            }
            Err(_) => {
                if config.spawn_pending {
                    return Err(ProviderError::OwnershipChanged(
                        "QEMU launch may have started before its process receipt was persisted"
                            .to_owned(),
                    ));
                }
                if let (Some(pid), Some(token)) = (config.process_id, config.process_start_token) {
                    if self.executor.process_matches(
                        pid,
                        token,
                        &self.system_binary,
                        &config.command_sha256,
                        config.process_executable_sha256.as_deref().unwrap_or(""),
                    ) {
                        return Err(ProviderError::OwnershipChanged(
                            "recorded QEMU process is alive but its QMP identity is unavailable"
                                .to_owned(),
                        ));
                    }
                    if !self.executor.process_absence_proven(pid, token) {
                        return Err(ProviderError::OwnershipChanged(
                            "recorded QEMU process absence cannot be proven".to_owned(),
                        ));
                    }
                    if !self.executor.process_group_absence_proven(pid) {
                        return Err(ProviderError::OwnershipChanged(
                            "recorded QEMU process group is not empty".to_owned(),
                        ));
                    }
                }
                ensure_control_endpoints_absent(config)?;
                ProviderPowerState::Off
            }
        };
        Ok(config.snapshot(power_state))
    }

    fn validate_live_process_receipt(
        &self,
        config: &QemuVmConfig,
    ) -> Result<(u32, u64), ProviderError> {
        if config.spawn_pending {
            return Err(ProviderError::OwnershipChanged(
                "QEMU launch intent has no durable process receipt".to_owned(),
            ));
        }
        let (process_id, process_start_token) = config
            .process_id
            .zip(config.process_start_token)
            .ok_or_else(|| {
            ProviderError::OwnershipChanged(
                "QEMU control requires a durable process receipt".to_owned(),
            )
        })?;
        let executable_sha256 = config.process_executable_sha256.as_deref().ok_or_else(|| {
            ProviderError::OwnershipChanged(
                "QEMU control requires a durable executable receipt".to_owned(),
            )
        })?;
        if !self.executor.process_matches(
            process_id,
            process_start_token,
            &self.system_binary,
            &config.command_sha256,
            executable_sha256,
        ) {
            return Err(ProviderError::OwnershipChanged(
                "QEMU process identity drifted from its durable receipt".to_owned(),
            ));
        }
        Ok((process_id, process_start_token))
    }

    fn validate_overlay_chain(&self, config: &QemuVmConfig) -> Result<(), ProviderError> {
        let overlay = self.inspect_image(config.overlay_path.clone())?;
        let base = self.inspect_image(config.parent_path.clone())?;
        if overlay.disk_format != "qcow2"
            || overlay.disk_type != "overlay"
            || overlay
                .parent_path
                .as_ref()
                .is_none_or(|path| !provider_path_equal(path, &config.parent_path))
            || base.disk_format != "qcow2"
            || base.parent_path.is_some()
        {
            return Err(ProviderError::OwnershipChanged(
                "QEMU QCOW2 backing chain is not exactly one overlay over the immutable base"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

impl<E: QemuCommandExecutor> LocalVmProvider for QemuProvider<E> {
    fn name(&self) -> &'static str {
        "qemu"
    }

    fn probe(&self) -> ProviderProbe {
        let version = self.executor.run(
            &self.system_binary,
            &[OsString::from("--version")],
            Duration::from_secs(10),
            PROBE_OUTPUT_LIMIT,
        );
        let version = match version {
            Ok(version) => version,
            Err(ProviderError::Command(_)) => {
                return ProviderProbe {
                    name: "qemu",
                    status: ProviderProbeStatus::Unavailable,
                    available: false,
                    detail: "QEMU system binary was not found".to_owned(),
                    capabilities: ProviderCapabilities::unavailable(),
                };
            }
            Err(_) => {
                return ProviderProbe {
                    name: "qemu",
                    status: ProviderProbeStatus::ProbeFailed,
                    available: false,
                    detail: "QEMU system binary did not complete its bounded probe".to_owned(),
                    capabilities: ProviderCapabilities::unavailable(),
                };
            }
        };
        if !version.success {
            return ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::ProbeFailed,
                available: false,
                detail: "QEMU version probe failed".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            };
        }
        let Some(version_line) = probe_version_line(&version.stdout, "QEMU emulator version")
        else {
            return ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::ProbeFailed,
                available: false,
                detail: "QEMU system version output was invalid".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            };
        };
        let image_version = self.executor.run(
            &self.image_binary,
            &[OsString::from("--version")],
            Duration::from_secs(10),
            PROBE_OUTPUT_LIMIT,
        );
        let image_version = match image_version {
            Ok(version) => version,
            Err(ProviderError::Command(_)) => {
                return ProviderProbe {
                    name: "qemu",
                    status: ProviderProbeStatus::Unavailable,
                    available: false,
                    detail: "QEMU image binary was not found".to_owned(),
                    capabilities: ProviderCapabilities::unavailable(),
                };
            }
            Err(_) => {
                return ProviderProbe {
                    name: "qemu",
                    status: ProviderProbeStatus::ProbeFailed,
                    available: false,
                    detail: "QEMU image binary did not complete its bounded probe".to_owned(),
                    capabilities: ProviderCapabilities::unavailable(),
                };
            }
        };
        if !image_version.success {
            return ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::ProbeFailed,
                available: false,
                detail: "QEMU image version probe failed".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            };
        }
        if probe_version_line(&image_version.stdout, "qemu-img version").is_none() {
            return ProviderProbe {
                name: "qemu",
                status: ProviderProbeStatus::ProbeFailed,
                available: false,
                detail: "QEMU image version output was invalid".to_owned(),
                capabilities: ProviderCapabilities::unavailable(),
            };
        }
        let (accelerators, kvm_status, kvm_advertised) = match self.accelerator_inventory() {
            Ok(inventory) => inventory,
            Err(_) => {
                return ProviderProbe {
                    name: "qemu",
                    status: ProviderProbeStatus::ProbeFailed,
                    available: false,
                    detail: "QEMU accelerator discovery failed".to_owned(),
                    capabilities: ProviderCapabilities::unavailable(),
                };
            }
        };
        let hardware = host_accelerator();
        let hardware_available = accelerators.iter().any(|value| value == hardware);
        let native_accelerator = if hardware_available {
            "available".to_owned()
        } else if hardware == "kvm" && kvm_advertised {
            format!(
                "unavailable ({})",
                kvm_status
                    .expect("advertised KVM always has a device admission result")
                    .diagnostic()
            )
        } else if hardware == "kvm" {
            "unavailable (QEMU did not advertise KVM)".to_owned()
        } else {
            "unavailable".to_owned()
        };
        ProviderProbe {
            name: "qemu",
            status: ProviderProbeStatus::Ready,
            available: true,
            detail: format!("{version_line}; native accelerator {hardware} {native_accelerator}"),
            capabilities: ProviderCapabilities {
                schema_version: crate::core::automation::AUTOMATION_SCHEMA_VERSION,
                full_system_vm: true,
                cow_overlay: true,
                hardware_acceleration: hardware_available,
                accelerators,
                guest_os: vec!["windows".to_owned(), "linux".to_owned()],
                guest_arch: vec![std::env::consts::ARCH.to_owned()],
                guest_transports: vec!["qga".to_owned()],
                networkless_guest_exec: true,
            },
        }
    }

    fn inspect_image(&self, path: PathBuf) -> Result<ProviderImageInfo, ProviderError> {
        if !qemu_argument_path_is_safe(&path) {
            return Err(ProviderError::Authority(
                "QEMU image path contains an unsafe argument or Windows alias".to_owned(),
            ));
        }
        let output = self.run_checked(
            &self.image_binary,
            &[
                OsString::from("info"),
                OsString::from("--output=json"),
                path.as_os_str().to_owned(),
            ],
        )?;
        let info: QemuImgInfo = serde_json::from_slice(&output.stdout).map_err(|_| {
            ProviderError::InvalidResponse("qemu-img info JSON was invalid".to_owned())
        })?;
        Ok(ProviderImageInfo {
            path: path.clone(),
            disk_format: info.format,
            disk_type: if info.backing_filename.is_some() {
                "overlay"
            } else {
                "base"
            }
            .to_owned(),
            parent_path: info
                .full_backing_filename
                .or(info.backing_filename.map(PathBuf::from)),
            file_size: fs::metadata(path).map(|value| value.len()).unwrap_or(0),
            virtual_size: info.virtual_size,
        })
    }

    fn create_overlay(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &CreateOverlayRequest,
    ) -> Result<ProviderImageInfo, ProviderError> {
        authority.validate_overlay_request(request)?;
        if !qemu_argument_path_is_safe(&request.parent_path) {
            return Err(ProviderError::Authority(
                "QEMU parent image path contains an unsafe argument or Windows alias".to_owned(),
            ));
        }
        prove_path_absent(&request.overlay_path, "QEMU overlay")?;
        let parent = request.overlay_path.parent().ok_or_else(|| {
            ProviderError::Authority("QEMU overlay path has no runtime parent".to_owned())
        })?;
        let file_name = request
            .overlay_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                ProviderError::Authority("QEMU overlay filename is invalid".to_owned())
            })?;
        let staged_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        prove_path_absent(&staged_path, "QEMU staged overlay")?;
        let create_result = self.run_checked(
            &self.image_binary,
            &[
                OsString::from("create"),
                OsString::from("-f"),
                OsString::from("qcow2"),
                OsString::from("-F"),
                OsString::from("qcow2"),
                OsString::from("-b"),
                request.parent_path.as_os_str().to_owned(),
                staged_path.as_os_str().to_owned(),
            ],
        );
        if let Err(error) = create_result {
            cleanup_staged_overlay(&staged_path);
            return Err(error);
        }
        authority.validate_overlay_request(request)?;
        let staged_file = open_private_qemu_file(&staged_path)?;
        let staged = match self.inspect_image(staged_path.clone()) {
            Ok(staged) => staged,
            Err(error) => {
                cleanup_exact_qemu_file(&staged_path, staged_file);
                return Err(error);
            }
        };
        if staged.disk_format != "qcow2"
            || staged.disk_type != "overlay"
            || staged
                .parent_path
                .as_ref()
                .is_none_or(|path| !provider_path_equal(path, &request.parent_path))
        {
            cleanup_exact_qemu_file(&staged_path, staged_file);
            return Err(ProviderError::OwnershipChanged(
                "staged QEMU overlay did not bind the exact immutable parent".to_owned(),
            ));
        }
        ensure_private_qemu_file(&staged_path, &staged_file)?;
        prove_path_absent(&request.overlay_path, "QEMU overlay")?;
        if let Err(error) = fs::hard_link(&staged_path, &request.overlay_path) {
            cleanup_exact_qemu_file(&staged_path, staged_file);
            return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                ProviderError::Collision("QEMU overlay already exists".to_owned())
            } else {
                ProviderError::Command(format!(
                    "failed to publish QEMU overlay without replacement: {error}"
                ))
            });
        }
        ensure_private_qemu_file(&request.overlay_path, &staged_file)?;
        authority.validate_overlay_request(request)?;
        remove_exact_qemu_file(&staged_path, staged_file)?;
        sync_parent_directory(&request.overlay_path).map_err(|error| {
            ProviderError::Command(format!(
                "failed to persist QEMU overlay publication: {error}"
            ))
        })?;
        self.inspect_image(request.overlay_path.clone())
    }

    fn create_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &CreateVmRequest,
    ) -> Result<ProviderVmIdentity, ProviderError> {
        authority.validate_create_request(request)?;
        let id = request
            .name
            .strip_prefix("vmcell-")
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| {
                ProviderError::Authority("QEMU VM name did not contain its CellId".to_owned())
            })?
            .to_string();
        create_ordinary_directory(&request.configuration_path)?;
        let config_path = Self::config_path(&request.configuration_path);
        if config_path.exists() {
            return Err(ProviderError::Collision(
                "QEMU provider configuration already exists".to_owned(),
            ));
        }
        let accelerator = self.select_accelerator(request)?;
        let mut config = QemuVmConfig {
            schema_version: QEMU_CONFIG_SCHEMA,
            id: id.clone(),
            name: request.name.clone(),
            ownership_marker: String::new(),
            configuration_path: request.configuration_path.clone(),
            overlay_path: request.overlay_path.clone(),
            parent_path: request.parent_path.clone(),
            cpu_count: request.cpu_count,
            memory_mib: request.memory_mib,
            accelerator,
            qmp: ControlEndpoint::qmp(&request.configuration_path, &id),
            qga: ControlEndpoint::qga(&request.configuration_path, &id),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: None,
            process_start_token: None,
            process_executable_sha256: None,
        };
        config.command_sha256 = launch_digest(&config);
        config.validate(&config_path)?;
        write_config_new(&config_path, &config)?;
        Ok(ProviderVmIdentity {
            id,
            name: request.name.clone(),
        })
    }

    fn claim_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &ClaimVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        authority.validate_claim_request(request)?;
        let path = Self::config_path(&request.expected.configuration_path);
        let snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete)?;
        let mut config = snapshot.config.clone();
        authorize_config(authority, &config, false)?;
        validate_config_snapshot(&config, &request.expected, false)?;
        if !config.ownership_marker.is_empty()
            && config.ownership_marker != request.ownership_marker
        {
            return Err(ProviderError::OwnershipChanged(
                "QEMU configuration marker changed".to_owned(),
            ));
        }
        config.ownership_marker = request.ownership_marker.clone();
        replace_config(&path, snapshot, &config)?;
        self.inspect_config(&config)
    }

    fn configure_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        request: &ConfigureVmRequest,
    ) -> Result<ProviderVm, ProviderError> {
        authority.validate_configure_request(request)?;
        let path = Self::config_path(&request.expected.configuration_path);
        let snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete)?;
        let mut config = snapshot.config.clone();
        authorize_config(authority, &config, true)?;
        validate_config_snapshot(&config, &request.expected, true)?;
        config.cpu_count = request.cpu_count;
        config.command_sha256 = launch_digest(&config);
        replace_config(&path, snapshot, &config)?;
        self.inspect_config(&config)
    }

    fn inspect_vm(&self, lookup: &VmLookup) -> Result<Option<ProviderVm>, ProviderError> {
        self.config_for_lookup(lookup)?
            .map(|config| self.inspect_config(&config))
            .transpose()
    }

    fn start_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        let path = Self::config_path(&expected.configuration_path);
        let snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete)?;
        let mut config = snapshot.config.clone();
        authorize_config(authority, &config, true)?;
        validate_config_snapshot(&config, expected, true)?;
        self.validate_overlay_chain(&config)?;
        if config.command_sha256 != launch_digest(&config) {
            return Err(ProviderError::OwnershipChanged(
                "QEMU launch configuration digest changed".to_owned(),
            ));
        }
        if expected.power_state == ProviderPowerState::Off {
            ensure_control_endpoints_absent(&config)?;
            config.spawn_pending = true;
            replace_config(&path, snapshot, &config)?;
            let args = launch_args(&config);
            let receipt = self.executor.spawn_vm(&self.system_binary, &args)?;
            let snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete)?;
            if snapshot.bytes_sha256 != qemu_config_bytes_sha256(&encode_qemu_config(&config)?) {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU launch intent changed before its process receipt was recorded".to_owned(),
                ));
            }
            config.process_id = Some(receipt.process_id);
            config.process_start_token = Some(receipt.process_start_token);
            config.process_executable_sha256 = Some(receipt.executable_sha256);
            config.spawn_pending = false;
            replace_config(&path, snapshot, &config)?;
        } else if expected.power_state != ProviderPowerState::Paused {
            return Err(ProviderError::OwnershipChanged(
                "QEMU start expected an off or prelaunch VM".to_owned(),
            ));
        }
        self.validate_live_process_receipt(&config)?;
        let qmp_deadline = std::time::Instant::now() + CONTROL_TIMEOUT;
        let stream = self.executor.connect_qmp(&config.qmp, CONTROL_TIMEOUT)?;
        let mut qmp = QmpClient::negotiate(stream, remaining_provider_duration(qmp_deadline)?)?;
        validate_qmp_identity(&mut qmp, &config)?;
        qmp.execute("cont", None)?;
        let status = qmp.execute("query-status", None)?;
        if status.get("status").and_then(Value::as_str) != Some("running") {
            return Err(ProviderError::InvalidResponse(
                "QEMU did not enter running state".to_owned(),
            ));
        }
        Ok(())
    }

    fn stop_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        let path = Self::config_path(&expected.configuration_path);
        let snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete)?;
        let mut config = snapshot.config.clone();
        authorize_config(authority, &config, true)?;
        validate_config_snapshot(&config, expected, true)?;
        let (process_id, process_start_token) = self.validate_live_process_receipt(&config)?;
        let qmp_deadline = std::time::Instant::now() + CONTROL_TIMEOUT;
        let stream = self.executor.connect_qmp(&config.qmp, CONTROL_TIMEOUT)?;
        let mut qmp = QmpClient::negotiate(stream, remaining_provider_duration(qmp_deadline)?)?;
        validate_qmp_identity(&mut qmp, &config)?;
        qmp.execute("quit", None)?;
        let deadline = std::time::Instant::now() + CONTROL_TIMEOUT;
        while std::time::Instant::now() < deadline {
            let endpoint_absent = self
                .executor
                .connect_qmp(&config.qmp, Duration::from_millis(25))
                .is_err();
            if endpoint_absent
                && self
                    .executor
                    .process_absence_proven(process_id, process_start_token)
            {
                if !self.executor.process_group_absence_proven(process_id) {
                    return Err(ProviderError::OwnershipChanged(
                        "QEMU leader exited while its owned process group remained live".to_owned(),
                    ));
                }
                ensure_control_endpoints_absent(&config)?;
                config.process_id = None;
                config.process_start_token = None;
                config.process_executable_sha256 = None;
                config.spawn_pending = false;
                replace_config(&path, snapshot, &config)?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(ProviderError::Timeout(
            "QEMU did not terminate within the bounded stop window".to_owned(),
        ))
    }

    fn remove_vm(
        &self,
        authority: &ProviderMutationAuthority<'_>,
        expected: &ProviderVm,
    ) -> Result<(), ProviderError> {
        authority.validate_vm(expected)?;
        if expected.power_state != ProviderPowerState::Off {
            return Err(ProviderError::OwnershipChanged(
                "QEMU remove expected an off VM".to_owned(),
            ));
        }
        let path = Self::config_path(&expected.configuration_path);
        let mut snapshot = read_config_snapshot(&path, QemuFileSharePolicy::DenyWrite)?;
        let config = snapshot.config.clone();
        authorize_config(authority, &config, true)?;
        validate_config_snapshot(&config, expected, true)?;
        if config.spawn_pending {
            return Err(ProviderError::OwnershipChanged(
                "QEMU launch intent has no durable process receipt".to_owned(),
            ));
        }
        if self
            .executor
            .connect_qmp(&config.qmp, Duration::from_millis(50))
            .is_ok()
        {
            return Err(ProviderError::OwnershipChanged(
                "QEMU control endpoint is still live".to_owned(),
            ));
        }
        if let (Some(pid), Some(token)) = (config.process_id, config.process_start_token) {
            if self.executor.process_matches(
                pid,
                token,
                &self.system_binary,
                &config.command_sha256,
                config.process_executable_sha256.as_deref().unwrap_or(""),
            ) {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU process is still live".to_owned(),
                ));
            }
            if !self.executor.process_absence_proven(pid, token) {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU process absence cannot be proven".to_owned(),
                ));
            }
            if !self.executor.process_group_absence_proven(pid) {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU process group is still live".to_owned(),
                ));
            }
        }
        ensure_control_endpoints_absent(&config)?;
        validate_pinned_config_snapshot(&path, &mut snapshot)?;
        remove_exact_qemu_file(&path, snapshot.file)?;
        sync_parent_directory(&path).map_err(|error| {
            ProviderError::Command(format!(
                "failed to persist QEMU configuration removal: {error}"
            ))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QemuVmConfig {
    schema_version: u32,
    id: String,
    name: String,
    ownership_marker: String,
    configuration_path: PathBuf,
    overlay_path: PathBuf,
    parent_path: PathBuf,
    cpu_count: u16,
    memory_mib: u64,
    accelerator: String,
    qmp: ControlEndpoint,
    qga: ControlEndpoint,
    command_sha256: String,
    #[serde(default)]
    spawn_pending: bool,
    process_id: Option<u32>,
    process_start_token: Option<u64>,
    #[serde(default)]
    process_executable_sha256: Option<String>,
}

struct QemuConfigSnapshot {
    config: QemuVmConfig,
    bytes_sha256: String,
    file: File,
}

struct QemuConfigWrite {
    bytes_sha256: String,
    file: File,
}

impl QemuVmConfig {
    fn validate(&self, path: &Path) -> Result<(), ProviderError> {
        if self.schema_version != QEMU_CONFIG_SCHEMA
            || Uuid::parse_str(&self.id).is_err()
            || self.name != format!("vmcell-{}", self.id)
            || !Self::config_path_matches(path, &self.configuration_path)
            || self.overlay_path.parent() != self.configuration_path.parent()
            || self.qmp != ControlEndpoint::qmp(&self.configuration_path, &self.id)
            || self.qga != ControlEndpoint::qga(&self.configuration_path, &self.id)
            || self.cpu_count == 0
            || self.memory_mib == 0
            || !matches!(self.accelerator.as_str(), "whpx" | "kvm" | "hvf" | "tcg")
            || !qemu_argument_path_is_safe(&self.configuration_path)
            || !qemu_argument_path_is_safe(&self.overlay_path)
            || !qemu_argument_path_is_safe(&self.parent_path)
            || !control_endpoint_is_safe(&self.qmp)
            || !control_endpoint_is_safe(&self.qga)
            || self.command_sha256 != launch_digest(self)
            || self.process_id.is_some() != self.process_start_token.is_some()
            || self.process_id.is_some() != self.process_executable_sha256.is_some()
            || self.process_start_token == Some(0)
            || self
                .process_executable_sha256
                .as_ref()
                .is_some_and(|value| {
                    value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        {
            return Err(ProviderError::InvalidResponse(
                "QEMU configuration invariants failed".to_owned(),
            ));
        }
        Ok(())
    }

    fn config_path_matches(path: &Path, configuration_path: &Path) -> bool {
        path == configuration_path.join("vm.json")
    }

    fn snapshot(&self, power_state: ProviderPowerState) -> ProviderVm {
        ProviderVm {
            id: self.id.clone(),
            name: self.name.clone(),
            power_state,
            ownership_marker: self.ownership_marker.clone(),
            configuration_path: self.configuration_path.clone(),
            attached_disks: vec![self.overlay_path.clone()],
            network_adapter_count: 0,
            cpu_count: self.cpu_count,
            memory_mib: self.memory_mib,
        }
    }
}

#[derive(Deserialize)]
struct QemuImgInfo {
    format: String,
    #[serde(rename = "virtual-size")]
    virtual_size: u64,
    #[serde(rename = "backing-filename")]
    backing_filename: Option<String>,
    #[serde(rename = "full-backing-filename")]
    full_backing_filename: Option<PathBuf>,
}

fn prove_path_absent(path: &Path, label: &str) -> Result<(), ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ProviderError::Collision(format!(
            "{label} path already exists"
        ))),
        Err(_) => Err(ProviderError::OwnershipChanged(format!(
            "{label} absence could not be proven"
        ))),
    }
}

fn open_private_qemu_file(path: &Path) -> Result<File, ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    let current = fs::symlink_metadata(path).map_err(|_| {
        ProviderError::OwnershipChanged(
            "QEMU staged overlay path metadata could not be pinned".to_owned(),
        )
    })?;
    if !metadata_is_ordinary_file(&current) {
        return Err(ProviderError::OwnershipChanged(
            "QEMU staged overlay path is not an ordinary file".to_owned(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    configure_qemu_file_options(&mut options, false, QemuFileSharePolicy::DenyWrite);
    let file = options.open(path).map_err(|error| {
        ProviderError::OwnershipChanged(format!("QEMU staged overlay could not be pinned: {error}"))
    })?;
    let metadata = file.metadata().map_err(|_| {
        ProviderError::OwnershipChanged(
            "QEMU staged overlay metadata could not be pinned".to_owned(),
        )
    })?;
    if !metadata_is_ordinary_file(&metadata) {
        return Err(ProviderError::OwnershipChanged(
            "QEMU staged overlay is not an ordinary file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| {
                ProviderError::OwnershipChanged(
                    "QEMU staged overlay privacy could not be enforced".to_owned(),
                )
            })?;
    }
    ensure_private_qemu_file(path, &file)?;
    Ok(file)
}

fn cleanup_staged_overlay(path: &Path) {
    if let Ok(file) = open_private_qemu_file(path) {
        cleanup_exact_qemu_file(path, file);
    }
}

fn cleanup_exact_qemu_file(path: &Path, file: File) {
    let _ = remove_exact_qemu_file(path, file);
}

fn remove_exact_qemu_file(path: &Path, file: File) -> Result<(), ProviderError> {
    ensure_private_qemu_file(path, &file)?;
    #[cfg(windows)]
    {
        let delete_file = open_private_qemu_delete_file(path, &file)?;
        drop(file);
        remove_open_qemu_file(&delete_file)?;
        drop(delete_file);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path).map_err(|error| {
            ProviderError::Command(format!("failed to remove exact QEMU file: {error}"))
        })
    }
}

fn create_ordinary_directory(path: &Path) -> Result<(), ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !qemu_metadata_is_reparse(&metadata) => {
            ensure_private_qemu_directory(path)
        }
        Ok(_) => Err(ProviderError::Authority(
            "QEMU configuration path is not an ordinary directory".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            let result = {
                use std::os::unix::fs::DirBuilderExt;

                fs::DirBuilder::new().mode(0o700).create(path)
            };
            #[cfg(not(unix))]
            let result = fs::create_dir(path);
            result.map_err(|error| {
                ProviderError::Command(format!(
                    "failed to create QEMU configuration directory: {error}"
                ))
            })?;
            ensure_private_qemu_directory(path)
        }
        Err(error) => Err(ProviderError::Command(format!(
            "failed to inspect QEMU configuration directory: {error}"
        ))),
    }
}

fn read_config(path: &Path) -> Result<QemuVmConfig, ProviderError> {
    Ok(read_config_snapshot(path, QemuFileSharePolicy::DenyWriteAndDelete)?.config)
}

fn read_config_snapshot(
    path: &Path,
    share_policy: QemuFileSharePolicy,
) -> Result<QemuConfigSnapshot, ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    configure_qemu_file_options(&mut options, false, share_policy);
    let mut file = options.open(path).map_err(|error| {
        ProviderError::Command(format!("failed to read QEMU configuration: {error}"))
    })?;
    ensure_private_qemu_file(path, &file)?;
    let bytes = read_qemu_config_bytes(&mut file)?;
    let config = decode_qemu_config(path, &bytes)?;
    let bytes_sha256 = qemu_config_bytes_sha256(&bytes);
    Ok(QemuConfigSnapshot {
        config,
        bytes_sha256,
        file,
    })
}

fn read_qemu_config_bytes(file: &mut File) -> Result<Vec<u8>, ProviderError> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        ProviderError::Command(format!("failed to seek QEMU configuration: {error}"))
    })?;
    let mut bytes = Vec::new();
    file.take((QEMU_CONFIG_MAX_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ProviderError::Command(format!("failed to read QEMU configuration: {error}"))
        })?;
    if bytes.len() > QEMU_CONFIG_MAX_BYTES {
        return Err(ProviderError::InvalidResponse(
            "QEMU configuration exceeds the maximum size".to_owned(),
        ));
    }
    Ok(bytes)
}

fn decode_qemu_config(path: &Path, bytes: &[u8]) -> Result<QemuVmConfig, ProviderError> {
    let config: QemuVmConfig = serde_json::from_slice(bytes).map_err(|_| {
        ProviderError::InvalidResponse("QEMU configuration JSON is invalid".to_owned())
    })?;
    config.validate(path)?;
    Ok(config)
}

fn encode_qemu_config(config: &QemuVmConfig) -> Result<Vec<u8>, ProviderError> {
    let bytes = serde_json::to_vec_pretty(config).map_err(|_| {
        ProviderError::InvalidResponse("QEMU configuration could not be encoded".to_owned())
    })?;
    if bytes.len() > QEMU_CONFIG_MAX_BYTES {
        return Err(ProviderError::InvalidResponse(
            "QEMU configuration exceeds the maximum size".to_owned(),
        ));
    }
    Ok(bytes)
}

fn qemu_config_bytes_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_pinned_config_snapshot(
    path: &Path,
    snapshot: &mut QemuConfigSnapshot,
) -> Result<(), ProviderError> {
    let bytes =
        validate_pinned_qemu_config_bytes(path, &mut snapshot.file, &snapshot.bytes_sha256)?;
    decode_qemu_config(path, &bytes)?;
    Ok(())
}

fn validate_pinned_qemu_config_bytes(
    path: &Path,
    file: &mut File,
    expected_sha256: &str,
) -> Result<Vec<u8>, ProviderError> {
    ensure_private_qemu_file(path, file)?;
    let bytes = read_qemu_config_bytes(file)?;
    if qemu_config_bytes_sha256(&bytes) != expected_sha256 {
        return Err(ProviderError::OwnershipChanged(
            "QEMU configuration changed while it was pinned".to_owned(),
        ));
    }
    Ok(bytes)
}

fn write_config_new_pinned(
    path: &Path,
    config: &QemuVmConfig,
) -> Result<QemuConfigWrite, ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    let bytes = encode_qemu_config(config)?;
    let bytes_sha256 = qemu_config_bytes_sha256(&bytes);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    configure_qemu_file_options(&mut options, true, QemuFileSharePolicy::DenyWriteAndDelete);
    let mut file = options.open(path).map_err(|error| {
        ProviderError::Command(format!("failed to create QEMU configuration: {error}"))
    })?;
    ensure_private_qemu_file(path, &file)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            ProviderError::Command(format!("failed to persist QEMU configuration: {error}"))
        })?;
    sync_parent_directory(path).map_err(|error| {
        ProviderError::Command(format!(
            "failed to persist QEMU configuration directory: {error}"
        ))
    })?;
    Ok(QemuConfigWrite { bytes_sha256, file })
}

fn write_config_new(path: &Path, config: &QemuVmConfig) -> Result<(), ProviderError> {
    drop(write_config_new_pinned(path, config)?);
    Ok(())
}

fn replace_config(
    path: &Path,
    mut snapshot: QemuConfigSnapshot,
    config: &QemuVmConfig,
) -> Result<(), ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    let temp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let mut temp_write = write_config_new_pinned(&temp, config)?;
    if let Err(error) = validate_pinned_config_snapshot(path, &mut snapshot) {
        // The generated path may have been replaced after a failed identity check.
        // Retain it rather than deleting by pathname without exact ownership proof.
        drop(temp_write);
        return Err(error);
    }
    let expected_sha256 = temp_write.bytes_sha256.clone();
    if let Err(error) =
        validate_pinned_qemu_config_bytes(&temp, &mut temp_write.file, &expected_sha256)
    {
        // A replacement or content drift makes the generated path untrusted.
        // Retaining it is safer than a pathname-based cleanup attempt.
        drop(temp_write);
        return Err(error);
    }
    drop(snapshot);
    drop(temp_write);
    replace_file_atomic(&temp, path).map_err(|error| {
        ProviderError::Command(format!("failed to replace QEMU configuration: {error}"))
    })?;
    let written = read_config_snapshot(path, QemuFileSharePolicy::DenyWriteAndDelete)?;
    if written.bytes_sha256 != expected_sha256 {
        return Err(ProviderError::OwnershipChanged(
            "QEMU configuration replacement did not bind the intended bytes".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)?;
    sync_parent_directory(destination)
}

#[cfg(target_os = "windows")]
fn replace_file_atomic(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn validate_config_snapshot(
    config: &QemuVmConfig,
    expected: &ProviderVm,
    require_marker: bool,
) -> Result<(), ProviderError> {
    if config.id != expected.id
        || config.name != expected.name
        || config.configuration_path != expected.configuration_path
        || expected.attached_disks != [config.overlay_path.clone()]
        || config.cpu_count != expected.cpu_count
        || config.memory_mib != expected.memory_mib
        || expected.network_adapter_count != 0
        || (require_marker && config.ownership_marker != expected.ownership_marker)
        || (!require_marker
            && !config.ownership_marker.is_empty()
            && config.ownership_marker != expected.ownership_marker)
    {
        return Err(ProviderError::OwnershipChanged(
            "QEMU configuration no longer matches the expected snapshot".to_owned(),
        ));
    }
    Ok(())
}

fn authorize_config(
    authority: &ProviderMutationAuthority<'_>,
    config: &QemuVmConfig,
    require_marker: bool,
) -> Result<(), ProviderError> {
    authority.validate_qemu_definition(&QemuDefinition {
        provider_id: &config.id,
        provider_name: &config.name,
        provider_marker: &config.ownership_marker,
        configuration_path: &config.configuration_path,
        overlay_path: &config.overlay_path,
        parent_path: &config.parent_path,
        accelerator: &config.accelerator,
        cpu_count: config.cpu_count,
        memory_mib: config.memory_mib,
        require_marker,
    })
}

fn provider_path_equal(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if cfg!(target_os = "windows") {
        canonical(left)
            .to_string_lossy()
            .eq_ignore_ascii_case(&canonical(right).to_string_lossy())
    } else {
        canonical(left) == canonical(right)
    }
}

fn validate_qmp_identity<S: protocol::ReadWrite>(
    qmp: &mut QmpClient<S>,
    config: &QemuVmConfig,
) -> Result<(), ProviderError> {
    let uuid = qmp.execute("query-uuid", None)?;
    let name = qmp.execute("query-name", None)?;
    if uuid.get("UUID").and_then(Value::as_str) != Some(config.id.as_str())
        || name.get("name").and_then(Value::as_str) != Some(config.name.as_str())
    {
        return Err(ProviderError::OwnershipChanged(
            "QMP UUID/name identity changed".to_owned(),
        ));
    }

    let cpus = qmp.execute("query-cpus-fast", None)?;
    let memory = qmp.execute("query-memory-size-summary", None)?;
    let network = qmp.execute("query-rx-filter", None)?;
    let blocks = qmp.execute("query-block", None)?;
    let expected_memory = config.memory_mib * 1024 * 1024;
    let block = blocks
        .as_array()
        .and_then(|items| (items.len() == 1).then(|| &items[0]));
    let inserted = block.and_then(|item| item.get("inserted"));
    let attached_path = inserted
        .and_then(|item| item.get("file"))
        .and_then(Value::as_str)
        .map(Path::new);
    if cpus.as_array().map(Vec::len) != Some(config.cpu_count as usize)
        || memory.get("base-memory").and_then(Value::as_u64) != Some(expected_memory)
        || network.as_array().is_none_or(|items| !items.is_empty())
        || inserted
            .and_then(|item| item.get("drv"))
            .and_then(Value::as_str)
            != Some("qcow2")
        || inserted
            .and_then(|item| item.get("backing_file_depth"))
            .and_then(Value::as_u64)
            != Some(1)
        || attached_path.is_none_or(|path| !provider_path_equal(path, &config.overlay_path))
    {
        return Err(ProviderError::OwnershipChanged(
            "QMP CPU, memory, network, or disk definition changed".to_owned(),
        ));
    }
    Ok(())
}

fn launch_args(config: &QemuVmConfig) -> Vec<OsString> {
    vec![
        "-S".into(),
        "-nodefaults".into(),
        "-display".into(),
        "none".into(),
        "-serial".into(),
        "none".into(),
        "-monitor".into(),
        "none".into(),
        "-no-reboot".into(),
        "-nic".into(),
        "none".into(),
        "-name".into(),
        config.name.clone().into(),
        "-uuid".into(),
        config.id.clone().into(),
        "-machine".into(),
        format!("accel={}", config.accelerator).into(),
        "-m".into(),
        config.memory_mib.to_string().into(),
        "-smp".into(),
        config.cpu_count.to_string().into(),
        "-drive".into(),
        format!(
            "file={},if=virtio,format=qcow2,cache=none",
            config.overlay_path.display()
        )
        .into(),
        "-qmp".into(),
        config.qmp.qemu_argument().into(),
        "-chardev".into(),
        format!("socket,id=qga0,{}", config.qga.qemu_argument()).into(),
        "-device".into(),
        "virtio-serial".into(),
        "-device".into(),
        "virtserialport,chardev=qga0,name=org.qemu.guest_agent.0".into(),
    ]
}

fn qemu_argument_path_is_safe(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        !value
            .chars()
            .any(|character| matches!(character, ',' | '\n' | '\r' | '\0'))
    }) && !crate::state::windows_path_has_stream_or_device_ambiguity(path)
}

fn control_endpoint_is_safe(endpoint: &ControlEndpoint) -> bool {
    match endpoint {
        ControlEndpoint::Unix(path) => unix_control_endpoint_is_safe(path),
        ControlEndpoint::WindowsPipe(value) => {
            value.starts_with(r"\\.\pipe\vmcell-")
                && value.len() <= 256
                && !value.chars().any(char::is_control)
        }
    }
}

#[cfg(unix)]
fn unix_control_endpoint_is_safe(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    qemu_argument_path_is_safe(path)
        && !path.as_os_str().as_bytes().is_empty()
        && path.as_os_str().as_bytes().len() <= UNIX_CONTROL_ENDPOINT_LIMIT
}

#[cfg(not(unix))]
fn unix_control_endpoint_is_safe(path: &Path) -> bool {
    qemu_argument_path_is_safe(path)
}

fn ensure_control_endpoints_absent(config: &QemuVmConfig) -> Result<(), ProviderError> {
    for endpoint in [&config.qmp, &config.qga] {
        let ControlEndpoint::Unix(path) = endpoint else {
            continue;
        };
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU control endpoint path already exists; stale or foreign state requires manual review"
                        .to_owned(),
                ));
            }
            Err(_) => {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU control endpoint absence could not be proven".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn remaining_provider_duration(deadline: std::time::Instant) -> Result<Duration, ProviderError> {
    deadline
        .checked_duration_since(std::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| ProviderError::Timeout("QEMU operation exceeded its deadline".to_owned()))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    File::open(path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "QEMU configuration has no parent directory",
        )
    })?)?
    .sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[derive(Clone, Copy)]
enum QemuFileSharePolicy {
    AllowAll,
    DenyWrite,
    DenyWriteAndDelete,
}

fn configure_qemu_file_options(
    options: &mut OpenOptions,
    create: bool,
    share_policy: QemuFileSharePolicy,
) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        if create {
            options.mode(0o600);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        let _ = create;
        let share_mode = match share_policy {
            QemuFileSharePolicy::AllowAll => FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            QemuFileSharePolicy::DenyWrite => FILE_SHARE_READ | FILE_SHARE_DELETE,
            QemuFileSharePolicy::DenyWriteAndDelete => FILE_SHARE_READ,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(share_mode);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (options, create, share_policy);
    }
}

#[cfg(windows)]
fn configure_qemu_delete_access(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    const GENERIC_READ: u32 = 0x8000_0000;
    const DELETE: u32 = 0x0001_0000;
    options.access_mode(GENERIC_READ | DELETE);
}

#[cfg(windows)]
fn open_private_qemu_delete_file(path: &Path, read_file: &File) -> Result<File, ProviderError> {
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    configure_qemu_file_options(&mut options, false, QemuFileSharePolicy::DenyWriteAndDelete);
    configure_qemu_delete_access(&mut options);
    let delete_file = options.open(path).map_err(|error| {
        ProviderError::OwnershipChanged(format!(
            "QEMU private file could not be upgraded for exact cleanup: {error}"
        ))
    })?;
    if qemu_file_identity(read_file)? != qemu_file_identity(&delete_file)? {
        return Err(ProviderError::OwnershipChanged(
            "QEMU private file identity changed before exact cleanup".to_owned(),
        ));
    }
    ensure_private_qemu_file(path, &delete_file)?;
    Ok(delete_file)
}

fn ensure_private_qemu_directory(path: &Path) -> Result<(), ProviderError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ProviderError::Authority("QEMU configuration directory identity is unavailable".to_owned())
    })?;
    if !metadata.is_dir() || qemu_metadata_is_reparse(&metadata) {
        return Err(ProviderError::Authority(
            "QEMU configuration directory is not private and ordinary".to_owned(),
        ));
    }
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(ProviderError::Authority(
                "QEMU configuration directory is not private and ordinary".to_owned(),
            ));
        }
    }
    Ok(())
}

fn ensure_private_qemu_file(path: &Path, file: &File) -> Result<(), ProviderError> {
    let open = file.metadata().map_err(|_| {
        ProviderError::Authority("QEMU configuration file identity is unavailable".to_owned())
    })?;
    let current = fs::symlink_metadata(path).map_err(|_| {
        ProviderError::Authority("QEMU configuration file path is unavailable".to_owned())
    })?;
    if !open.is_file()
        || !current.is_file()
        || qemu_metadata_is_reparse(&open)
        || qemu_metadata_is_reparse(&current)
    {
        return Err(ProviderError::Authority(
            "QEMU configuration file is not private, ordinary, and safe to use".to_owned(),
        ));
    }
    ensure_existing_qemu_ancestors_are_ordinary(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if open.uid() != unsafe { libc::geteuid() } || open.mode() & 0o077 != 0 {
            return Err(ProviderError::Authority(
                "QEMU configuration file is not private, ordinary, and safe to use".to_owned(),
            ));
        }
        if open.dev() != current.dev() || open.ino() != current.ino() {
            return Err(ProviderError::OwnershipChanged(
                "QEMU configuration file identity changed while it was opened".to_owned(),
            ));
        }
    }
    #[cfg(windows)]
    {
        let mut options = OpenOptions::new();
        options.read(true);
        configure_qemu_file_options(&mut options, false, QemuFileSharePolicy::AllowAll);
        let current_file = options.open(path).map_err(|error| {
            ProviderError::OwnershipChanged(format!(
                "QEMU configuration file path identity is unavailable: {error}"
            ))
        })?;
        let current_open = current_file.metadata().map_err(|_| {
            ProviderError::OwnershipChanged(
                "QEMU configuration file path identity is unavailable".to_owned(),
            )
        })?;
        if qemu_metadata_is_reparse(&current_open)
            || qemu_file_identity(file)? != qemu_file_identity(&current_file)?
        {
            return Err(ProviderError::OwnershipChanged(
                "QEMU configuration file identity changed while it was opened".to_owned(),
            ));
        }
    }
    Ok(())
}

fn ensure_existing_qemu_ancestors_are_ordinary(path: &Path) -> Result<(), ProviderError> {
    if !qemu_argument_path_is_safe(path) {
        return Err(ProviderError::Authority(
            "QEMU path contains an unsafe argument or Windows alias".to_owned(),
        ));
    }
    for (index, ancestor) in path.ancestors().enumerate() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if qemu_metadata_is_reparse(&metadata) || (index > 0 && !metadata.is_dir()) {
                    return Err(ProviderError::OwnershipChanged(
                        "QEMU path contains a reparse or non-directory ancestor".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && index == 0 => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProviderError::OwnershipChanged(
                    "QEMU path ancestor is unavailable".to_owned(),
                ));
            }
            Err(error) => {
                return Err(ProviderError::Command(format!(
                    "failed to inspect QEMU path ancestry: {error}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn qemu_metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn qemu_metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn qemu_file_identity(file: &File) -> Result<(u32, u64), ProviderError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(ProviderError::OwnershipChanged(
            "QEMU configuration file identity is unavailable".to_owned(),
        ));
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((information.dwVolumeSerialNumber, index))
}

#[cfg(windows)]
fn remove_open_qemu_file(file: &File) -> Result<(), ProviderError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let size = u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>()).map_err(|_| {
        ProviderError::Command("QEMU exact-file deletion metadata was invalid".to_owned())
    })?;
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size,
        )
    } == 0
    {
        return Err(ProviderError::Command(format!(
            "failed to remove exact QEMU file: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn launch_digest(config: &QemuVmConfig) -> String {
    let args = launch_args(config);
    argument_digest(&args)
}

fn argument_digest(args: &[OsString]) -> String {
    let mut hash = Sha256::new();
    for argument in args {
        let bytes = argument.to_string_lossy();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn ordinary_file_sha256(path: &Path) -> Result<String, ProviderError> {
    let requested_metadata = fs::symlink_metadata(path).map_err(|_| {
        ProviderError::OwnershipChanged("QEMU executable identity was unavailable".to_owned())
    })?;
    if !metadata_is_ordinary_file(&requested_metadata) {
        return Err(ProviderError::OwnershipChanged(
            "QEMU executable is not an ordinary file".to_owned(),
        ));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ProviderError::OwnershipChanged("QEMU executable path could not be resolved".to_owned())
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        ProviderError::OwnershipChanged("QEMU executable identity was unavailable".to_owned())
    })?;
    if !metadata_is_ordinary_file(&metadata) {
        return Err(ProviderError::OwnershipChanged(
            "QEMU executable is not an ordinary file".to_owned(),
        ));
    }
    let mut file = File::open(&canonical).map_err(|_| {
        ProviderError::OwnershipChanged("QEMU executable could not be pinned".to_owned())
    })?;
    opened_file_sha256(&mut file)
}

fn opened_file_sha256(file: &mut File) -> Result<String, ProviderError> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            ProviderError::OwnershipChanged("QEMU executable could not be hashed".to_owned())
        })?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn metadata_is_ordinary_file(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn redacted_command_failure() -> String {
    "QEMU command exited unsuccessfully".to_owned()
}

fn probe_version_line(bytes: &[u8], expected_prefix: &str) -> Option<String> {
    let line = std::str::from_utf8(bytes).ok()?.lines().next()?.trim();
    (!line.is_empty()
        && line.len() <= 256
        && line.starts_with(expected_prefix)
        && !line.chars().any(char::is_control))
    .then(|| line.to_owned())
}

fn process_error_to_provider(error: ProcessError) -> ProviderError {
    match error {
        ProcessError::Spawn => {
            ProviderError::Command("QEMU command could not be spawned".to_owned())
        }
        ProcessError::Io => ProviderError::Command("QEMU command I/O failed".to_owned()),
        ProcessError::Timeout => ProviderError::Timeout("QEMU command timed out".to_owned()),
        ProcessError::OutputLimit => {
            ProviderError::OutputLimit("QEMU command output exceeded the limit".to_owned())
        }
    }
}

fn system_binary_name() -> OsString {
    if std::env::consts::ARCH == "aarch64" {
        "qemu-system-aarch64".into()
    } else {
        "qemu-system-x86_64".into()
    }
}

fn host_accelerator() -> &'static str {
    match std::env::consts::OS {
        "windows" => "whpx",
        "linux" => "kvm",
        "macos" => "hvf",
        _ => "",
    }
}

fn filter_usable_accelerators(values: Vec<String>, kvm_usable: bool) -> Vec<String> {
    let mut values = values
        .into_iter()
        .filter(|value| value != "kvm" || kvm_usable)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

#[cfg(target_os = "linux")]
fn kvm_device_status() -> KvmDeviceStatus {
    probe_kvm_device(Path::new("/dev/kvm"))
}

#[cfg(not(target_os = "linux"))]
fn kvm_device_status() -> KvmDeviceStatus {
    KvmDeviceStatus::NotApplicable
}

#[cfg(target_os = "linux")]
fn probe_kvm_device(path: &Path) -> KvmDeviceStatus {
    use std::os::unix::fs::OpenOptionsExt;

    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return classify_kvm_error(error.kind()),
    };
    let Some(before_identity) = kvm_device_identity(&before) else {
        return KvmDeviceStatus::NotCharacterDevice;
    };
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => return classify_kvm_error(error.kind()),
    };
    let opened = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return KvmDeviceStatus::Unavailable,
    };
    let current = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return KvmDeviceStatus::IdentityChanged,
    };
    let opened_identity = kvm_device_identity(&opened);
    let current_identity = kvm_device_identity(&current);
    if !kvm_device_identity_is_stable(before_identity, opened_identity, current_identity) {
        return KvmDeviceStatus::IdentityChanged;
    }
    KvmDeviceStatus::Usable
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KvmDeviceIdentity {
    device: u64,
    inode: u64,
    raw_device: u64,
}

#[cfg(target_os = "linux")]
fn kvm_device_identity(metadata: &fs::Metadata) -> Option<KvmDeviceIdentity> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    (!metadata.file_type().is_symlink() && metadata.file_type().is_char_device()).then_some(
        KvmDeviceIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            raw_device: metadata.rdev(),
        },
    )
}

#[cfg(target_os = "linux")]
fn kvm_device_identity_is_stable(
    before: KvmDeviceIdentity,
    opened: Option<KvmDeviceIdentity>,
    current: Option<KvmDeviceIdentity>,
) -> bool {
    opened == Some(before) && current == Some(before)
}

#[cfg(any(target_os = "linux", test))]
fn classify_kvm_error(kind: std::io::ErrorKind) -> KvmDeviceStatus {
    match kind {
        std::io::ErrorKind::NotFound => KvmDeviceStatus::Missing,
        std::io::ErrorKind::PermissionDenied => KvmDeviceStatus::PermissionDenied,
        _ => KvmDeviceStatus::Unavailable,
    }
}

#[cfg(unix)]
fn resolve_executable(name: OsString) -> OsString {
    use std::os::unix::fs::PermissionsExt;

    let path = PathBuf::from(&name);
    let candidates = if path.components().count() > 1 {
        vec![path]
    } else {
        std::env::var_os("PATH")
            .map(|value| {
                std::env::split_paths(&value)
                    .map(|directory| directory.join(&path))
                    .collect()
            })
            .unwrap_or_default()
    };
    candidates
        .into_iter()
        .filter_map(|candidate| candidate.canonicalize().ok())
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .map(PathBuf::into_os_string)
        .unwrap_or(name)
}

#[cfg(target_os = "windows")]
fn resolve_executable(name: OsString) -> OsString {
    let search_paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    resolve_windows_executable(&name, &search_paths).unwrap_or(name)
}

#[cfg(target_os = "windows")]
fn resolve_windows_executable(name: &OsStr, search_paths: &[PathBuf]) -> Option<OsString> {
    let requested = PathBuf::from(name);
    let candidates = if requested.components().count() > 1 {
        vec![requested]
    } else {
        search_paths
            .iter()
            .map(|directory| directory.join(&requested))
            .collect::<Vec<_>>()
    };
    candidates
        .into_iter()
        .map(|candidate| {
            if candidate.extension().is_none() {
                candidate.with_extension("exe")
            } else {
                candidate
            }
        })
        .filter(|candidate| {
            candidate
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        })
        .find_map(|candidate| {
            fs::symlink_metadata(&candidate)
                .ok()
                .filter(metadata_is_ordinary_file)?;
            let canonical = candidate.canonicalize().ok()?;
            fs::symlink_metadata(&canonical)
                .ok()
                .filter(metadata_is_ordinary_file)?;
            Some(canonical)
        })
        .map(PathBuf::into_os_string)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn resolve_executable(name: OsString) -> OsString {
    name
}

#[cfg(unix)]
fn configure_detached_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

fn start_process_reaper(child: std::process::Child) -> Result<(), ProviderError> {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let waiter = std::sync::Arc::clone(&shared);
    match std::thread::Builder::new()
        .name("vmcell-qemu-reaper".to_owned())
        .spawn(move || {
            let child = waiter.lock().ok().and_then(|mut guard| guard.take());
            if let Some(mut child) = child {
                let _ = child.wait();
            }
        }) {
        Ok(_) => Ok(()),
        Err(_) => {
            let child = shared.lock().ok().and_then(|mut guard| guard.take());
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(ProviderError::Command(
                "QEMU process reaper could not be started".to_owned(),
            ))
        }
    }
}

#[cfg(target_os = "windows")]
fn configure_detached_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0000_0200 | 0x0800_0000);
}

#[cfg(not(any(unix, target_os = "windows")))]
fn configure_detached_process(_command: &mut Command) {}

#[cfg(target_os = "linux")]
fn process_start_token(process_id: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
    text.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn process_group_id(process_id: u32) -> Option<u32> {
    let text = fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
    text.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(2)?
        .parse()
        .ok()
}

#[cfg(target_os = "windows")]
fn process_start_token(process_id: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if handle.is_null() {
            return None;
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        ok.then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn process_start_token(_process_id: u32) -> Option<u64> {
    Some(0)
}

#[cfg(target_os = "linux")]
fn process_matches(
    process_id: u32,
    start_token: u64,
    program: &OsStr,
    command_sha256: &str,
    executable_sha256: &str,
) -> bool {
    use std::os::unix::ffi::OsStringExt;

    if process_start_token(process_id) != Some(start_token)
        || process_group_id(process_id) != Some(process_id)
    {
        return false;
    }
    let executable = match fs::read_link(format!("/proc/{process_id}/exe")) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let expected_program = Path::new(program);
    let executable_matches = if expected_program.is_absolute() {
        executable.canonicalize().ok() == expected_program.canonicalize().ok()
    } else {
        executable.file_name() == expected_program.file_name()
    };
    if !executable_matches {
        return false;
    }
    if process_executable_sha256(process_id).as_deref() != Some(executable_sha256)
        || process_start_token(process_id) != Some(start_token)
    {
        return false;
    }
    let command_line = match fs::read(format!("/proc/{process_id}/cmdline")) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let arguments = command_line
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .skip(1)
        .map(|value| OsString::from_vec(value.to_vec()))
        .collect::<Vec<_>>();
    argument_digest(&arguments) == command_sha256
        && process_start_token(process_id) == Some(start_token)
}

#[cfg(target_os = "linux")]
fn process_executable_sha256(process_id: u32) -> Option<String> {
    let mut file = File::open(format!("/proc/{process_id}/exe")).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    opened_file_sha256(&mut file).ok()
}

#[cfg(target_os = "linux")]
fn process_absence_proven(process_id: u32, start_token: u64) -> bool {
    if start_token == 0 {
        return false;
    }
    let process_root = PathBuf::from(format!("/proc/{process_id}"));
    match fs::symlink_metadata(&process_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(_) => process_start_token(process_id).is_some_and(|actual| actual != start_token),
    }
}

#[cfg(target_os = "windows")]
fn process_group_absence_proven(_process_group_id: u32) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn process_group_absence_proven(process_group_id: u32) -> bool {
    if process_group_id == 0 {
        return false;
    }
    let entries = match fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return false,
        };
        let Some(_process_id) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        match fs::read_to_string(entry.path().join("stat")) {
            Ok(text) => {
                let group = text
                    .rsplit_once(')')
                    .and_then(|(_, fields)| fields.split_whitespace().nth(2))
                    .and_then(|value| value.parse::<u32>().ok());
                if group == Some(process_group_id) {
                    return false;
                }
                if group.is_none() {
                    return false;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn process_matches(
    process_id: u32,
    start_token: u64,
    program: &OsStr,
    command_sha256: &str,
    executable_sha256: &str,
) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        WaitForSingleObject,
    };

    let _ = command_sha256;
    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | 0x0010_0000,
            0,
            process_id,
        );
        if handle.is_null() {
            return false;
        }
        let mut length = 32_768_u32;
        let mut buffer = vec![0_u16; length as usize];
        let image_ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) != 0;
        let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        CloseHandle(handle);
        if !image_ok || !alive || process_start_token(process_id) != Some(start_token) {
            return false;
        }
        buffer.truncate(length as usize);
        let actual = PathBuf::from(OsString::from_wide(&buffer));
        let expected = Path::new(program);
        let path_matches = if expected.is_absolute() {
            actual.canonicalize().ok() == expected.canonicalize().ok()
        } else {
            actual
                .file_name()
                .zip(expected.file_name())
                .is_some_and(|(left, right)| {
                    left.to_string_lossy()
                        .eq_ignore_ascii_case(&right.to_string_lossy())
                })
        };
        path_matches && ordinary_file_sha256(&actual).ok().as_deref() == Some(executable_sha256)
    }
}

#[cfg(target_os = "windows")]
fn process_absence_proven(process_id: u32, start_token: u64) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    if start_token == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(0x0010_0000, 0, process_id);
        if handle.is_null() {
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let result = WaitForSingleObject(handle, 0) == 0;
        CloseHandle(handle);
        result
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn process_matches(
    _process_id: u32,
    _start_token: u64,
    _program: &OsStr,
    _command_sha256: &str,
    _executable_sha256: &str,
) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn process_absence_proven(_process_id: u32, _start_token: u64) -> bool {
    false
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn process_group_absence_proven(_process_group_id: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    struct FakeQmpStream {
        reads: VecDeque<u8>,
        writes: Vec<u8>,
    }

    impl std::io::Read for FakeQmpStream {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            let count = target.len().min(self.reads.len());
            for slot in target.iter_mut().take(count) {
                *slot = self.reads.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl std::io::Write for FakeQmpStream {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl protocol::ReadWrite for FakeQmpStream {
        fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
            if Instant::now() >= deadline {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "scripted QMP deadline expired",
                ))
            } else {
                Ok(())
            }
        }
    }

    struct FakeExecutor {
        accelerators: Vec<String>,
        calls: Mutex<Vec<String>>,
        failed_program: Option<String>,
        backing: Option<PathBuf>,
        qmp_sessions: Mutex<VecDeque<Vec<u8>>>,
        process_matches: AtomicBool,
    }

    impl QemuCommandExecutor for FakeExecutor {
        fn run(
            &self,
            program: &OsStr,
            args: &[OsString],
            _timeout: Duration,
            _limit: usize,
        ) -> Result<QemuCommandOutput, ProviderError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{} {:?}", program.to_string_lossy(), args));
            if self.failed_program.as_deref() == program.to_str() {
                return Err(ProviderError::Command(
                    "scripted command failure".to_owned(),
                ));
            }
            let stdout = if args == [OsString::from("-accel"), OsString::from("help")] {
                self.accelerators.join("\n").into_bytes()
            } else if args == [OsString::from("--version")] {
                if program.to_string_lossy().contains("qemu-img") {
                    b"qemu-img version test\n".to_vec()
                } else {
                    b"QEMU emulator version test\n".to_vec()
                }
            } else if args.first() == Some(&OsString::from("info")) {
                let path = PathBuf::from(args.last().unwrap());
                let overlay = self
                    .backing
                    .as_ref()
                    .is_some_and(|backing| !provider_path_equal(&path, backing));
                serde_json::to_vec(&serde_json::json!({
                    "format": "qcow2",
                    "virtual-size": 1048576,
                    "backing-filename": overlay.then(|| self.backing.as_ref().unwrap().to_string_lossy().to_string()),
                    "full-backing-filename": overlay.then(|| self.backing.as_ref().unwrap())
                }))
                .unwrap()
            } else if args.first() == Some(&OsString::from("create")) {
                fs::write(PathBuf::from(args.last().unwrap()), b"overlay").unwrap();
                Vec::new()
            } else {
                Vec::new()
            };
            Ok(QemuCommandOutput {
                success: true,
                stdout,
                stderr: Vec::new(),
            })
        }
        fn spawn_vm(
            &self,
            _program: &OsStr,
            _args: &[OsString],
        ) -> Result<QemuSpawnReceipt, ProviderError> {
            Ok(QemuSpawnReceipt {
                process_id: 4242,
                process_start_token: 7,
                executable_sha256: "a".repeat(64),
            })
        }
        fn process_matches(
            &self,
            _process_id: u32,
            _start_token: u64,
            _program: &OsStr,
            _command_sha256: &str,
            _executable_sha256: &str,
        ) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push("process_matches".to_owned());
            self.process_matches.load(Ordering::SeqCst)
        }
        fn process_absence_proven(&self, _process_id: u32, _start_token: u64) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push("process_absence_proven".to_owned());
            true
        }
        fn process_group_absence_proven(&self, _process_group_id: u32) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push("process_group_absence_proven".to_owned());
            true
        }
        fn connect_qmp(
            &self,
            _: &ControlEndpoint,
            _: Duration,
        ) -> Result<Box<dyn protocol::ReadWrite>, ProviderError> {
            self.qmp_sessions
                .lock()
                .unwrap()
                .pop_front()
                .map(|bytes| {
                    Box::new(FakeQmpStream {
                        reads: bytes.into(),
                        writes: Vec::new(),
                    }) as Box<dyn protocol::ReadWrite>
                })
                .ok_or_else(|| ProviderError::Command("no scripted QMP session".to_owned()))
        }
    }

    #[test]
    fn probe_reports_hardware_and_tcg_without_silent_fallback() {
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec![host_accelerator().to_owned(), "tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: None,
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            PathBuf::from("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        let probe = provider.probe();
        assert!(probe.available);
        assert_eq!(probe.status, ProviderProbeStatus::Ready);
        assert_eq!(
            probe.capabilities.schema_version,
            crate::core::automation::AUTOMATION_SCHEMA_VERSION
        );
        let hardware_expected = host_accelerator() != "kvm" || kvm_device_status().usable();
        assert_eq!(probe.capabilities.hardware_acceleration, hardware_expected);
        assert!(probe.capabilities.accelerators.contains(&"tcg".to_owned()));
        assert_eq!(probe.capabilities.guest_transports, ["qga"]);
        assert!(probe.detail.contains(&format!(
            "native accelerator {} {}",
            host_accelerator(),
            if hardware_expected {
                "available"
            } else {
                "unavailable"
            }
        )));
        let calls = provider.executor.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with("qemu-system-test [\"--version\"]"))
        );
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with("qemu-img [\"--version\"]"))
        );
    }

    #[test]
    fn probe_distinguishes_missing_qemu_system_and_native_accelerator() {
        let missing_system = QemuProvider::new(
            FakeExecutor {
                accelerators: Vec::new(),
                calls: Mutex::new(Vec::new()),
                failed_program: Some("qemu-system-test".to_owned()),
                backing: None,
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            PathBuf::from("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        )
        .probe();
        assert_eq!(missing_system.status, ProviderProbeStatus::Unavailable);
        assert_eq!(missing_system.detail, "QEMU system binary was not found");

        let missing_native = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: None,
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            PathBuf::from("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        )
        .probe();
        assert_eq!(missing_native.status, ProviderProbeStatus::Ready);
        assert!(missing_native.available);
        assert!(!missing_native.capabilities.hardware_acceleration);
        assert!(missing_native.detail.contains(&format!(
            "native accelerator {} unavailable",
            host_accelerator()
        )));

        assert_eq!(
            probe_version_line(b"QEMU emulator version 9.2.0\n", "QEMU emulator version"),
            Some("QEMU emulator version 9.2.0".to_owned())
        );
        assert!(probe_version_line(b"unexpected tool\n", "QEMU emulator version").is_none());
        assert!(probe_version_line(&[0xff], "QEMU emulator version").is_none());
    }

    #[test]
    fn probe_is_unavailable_when_qemu_img_is_missing() {
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec![host_accelerator().to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: Some("qemu-img".to_owned()),
                backing: None,
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            PathBuf::from("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );

        let probe = provider.probe();

        assert!(!probe.available);
        assert_eq!(probe.status, ProviderProbeStatus::Unavailable);
        assert!(!probe.capabilities.cow_overlay);
        assert_eq!(probe.detail, "QEMU image binary was not found");
    }

    #[test]
    fn compiled_kvm_is_not_selectable_without_a_usable_device() {
        assert_eq!(
            filter_usable_accelerators(vec!["kvm".to_owned(), "tcg".to_owned()], false),
            vec!["tcg"]
        );
        assert_eq!(
            filter_usable_accelerators(
                vec!["tcg".to_owned(), "kvm".to_owned(), "tcg".to_owned()],
                true
            ),
            vec!["kvm", "tcg"]
        );
    }

    #[test]
    fn kvm_open_errors_have_stable_admission_diagnostics() {
        assert_eq!(
            classify_kvm_error(std::io::ErrorKind::NotFound),
            KvmDeviceStatus::Missing
        );
        assert_eq!(
            classify_kvm_error(std::io::ErrorKind::PermissionDenied),
            KvmDeviceStatus::PermissionDenied
        );
        assert_eq!(
            classify_kvm_error(std::io::ErrorKind::Other),
            KvmDeviceStatus::Unavailable
        );
        assert_eq!(
            KvmDeviceStatus::PermissionDenied.diagnostic(),
            "/dev/kvm is not read-write usable by the current identity"
        );
        assert!(KvmDeviceStatus::Usable.usable());
        assert_eq!(
            KvmDeviceStatus::IdentityChanged.diagnostic(),
            "/dev/kvm identity changed while it was opened"
        );
        assert_eq!(
            KvmDeviceStatus::NotCharacterDevice.diagnostic(),
            "/dev/kvm is not an ordinary character device"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kvm_probe_rejects_missing_non_device_and_symlink_paths() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-kvm");
        assert_eq!(probe_kvm_device(&missing), KvmDeviceStatus::Missing);

        let ordinary = directory.path().join("ordinary-kvm");
        fs::write(&ordinary, b"not a device").unwrap();
        assert_eq!(
            probe_kvm_device(&ordinary),
            KvmDeviceStatus::NotCharacterDevice
        );

        let linked = directory.path().join("linked-kvm");
        symlink(&ordinary, &linked).unwrap();
        assert_eq!(
            probe_kvm_device(&linked),
            KvmDeviceStatus::NotCharacterDevice
        );

        let exact = KvmDeviceIdentity {
            device: 1,
            inode: 2,
            raw_device: 3,
        };
        assert!(kvm_device_identity_is_stable(
            exact,
            Some(exact),
            Some(exact)
        ));
        assert!(!kvm_device_identity_is_stable(
            exact,
            Some(exact),
            Some(KvmDeviceIdentity { inode: 4, ..exact })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_reaper_makes_exited_child_absence_provable() {
        let child = Command::new("sh")
            .args(["-c", "sleep 0.05"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let process_id = child.id();
        let start_token = process_start_token(process_id).unwrap();
        start_process_reaper(child).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !process_absence_proven(process_id, start_token)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(process_absence_proven(process_id, start_token));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn leader_exit_does_not_prove_the_owned_process_group_empty() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & sleep 0.2"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached_process(&mut command);
        let child = command.spawn().unwrap();
        let process_id = child.id();
        let start_token = process_start_token(process_id).unwrap();
        assert_eq!(process_group_id(process_id), Some(process_id));
        start_process_reaper(child).unwrap();

        let leader_deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !process_absence_proven(process_id, start_token)
            && std::time::Instant::now() < leader_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(process_absence_proven(process_id, start_token));
        assert!(!process_group_absence_proven(process_id));

        unsafe {
            libc::kill(-(process_id as i32), libc::SIGTERM);
        }
        let group_deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !process_group_absence_proven(process_id)
            && std::time::Instant::now() < group_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(process_group_absence_proven(process_id));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn running_executable_hash_uses_proc_handle_not_replaced_path() {
        use std::os::unix::fs::PermissionsExt;

        let sleep = ["/usr/bin/sleep", "/bin/sleep"]
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .unwrap();
        let true_program = ["/usr/bin/true", "/bin/true"]
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("qemu-system-fixture");
        let retired = directory.path().join("qemu-system-fixture.retired");
        fs::copy(sleep, &program).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let launched_sha256 = ordinary_file_sha256(&program).unwrap();
        let mut child = Command::new(&program)
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let process_id = child.id();
        let start_token = process_start_token(process_id).unwrap();

        fs::rename(&program, &retired).unwrap();
        fs::copy(true_program, &program).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let replacement_sha256 = ordinary_file_sha256(&program).unwrap();
        let running_sha256 = process_executable_sha256(process_id).unwrap();

        assert_eq!(running_sha256, launched_sha256);
        assert_ne!(running_sha256, replacement_sha256);
        assert_eq!(process_start_token(process_id), Some(start_token));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_control_endpoint_paths_are_bounded_and_collisions_are_retained() {
        let directory = tempfile::tempdir().unwrap();
        let qemu = directory.path().join("qemu");
        fs::create_dir(&qemu).unwrap();
        let id = Uuid::nil().to_string();
        let mut config = QemuVmConfig {
            schema_version: QEMU_CONFIG_SCHEMA,
            id: id.clone(),
            name: format!("vmcell-{id}"),
            ownership_marker: "marker".to_owned(),
            configuration_path: qemu.clone(),
            overlay_path: directory.path().join("cell.qcow2"),
            parent_path: directory.path().join("base.qcow2"),
            cpu_count: 2,
            memory_mib: 1024,
            accelerator: "kvm".to_owned(),
            qmp: ControlEndpoint::qmp(&qemu, &id),
            qga: ControlEndpoint::qga(&qemu, &id),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: None,
            process_start_token: None,
            process_executable_sha256: None,
        };
        config.command_sha256 = launch_digest(&config);
        assert!(control_endpoint_is_safe(&config.qmp));
        assert!(ensure_control_endpoints_absent(&config).is_ok());

        let ControlEndpoint::Unix(qmp_path) = &config.qmp else {
            unreachable!();
        };
        fs::write(qmp_path, b"stale").unwrap();
        assert!(matches!(
            ensure_control_endpoints_absent(&config),
            Err(ProviderError::OwnershipChanged(message))
                if message.contains("stale or foreign state")
        ));

        let long_path = PathBuf::from("x".repeat(UNIX_CONTROL_ENDPOINT_LIMIT + 1));
        assert!(!control_endpoint_is_safe(&ControlEndpoint::Unix(long_path)));
    }

    #[test]
    fn launch_is_networkless_and_digest_binds_every_argument() {
        let config = QemuVmConfig {
            schema_version: 1,
            id: Uuid::nil().to_string(),
            name: format!("vmcell-{}", Uuid::nil()),
            ownership_marker: "marker".to_owned(),
            configuration_path: PathBuf::from("runtime/qemu"),
            overlay_path: PathBuf::from("runtime/cell.qcow2"),
            parent_path: PathBuf::from("base.qcow2"),
            cpu_count: 2,
            memory_mib: 1024,
            accelerator: "tcg".to_owned(),
            qmp: ControlEndpoint::Unix(PathBuf::from("qmp.sock")),
            qga: ControlEndpoint::Unix(PathBuf::from("qga.sock")),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: None,
            process_start_token: None,
            process_executable_sha256: None,
        };
        let args = launch_args(&config);
        let rendered = args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(rendered.windows(2).any(|pair| pair == ["-nic", "none"]));
        assert!(!launch_digest(&config).is_empty());
    }

    #[test]
    fn concrete_provider_definition_claim_configure_and_remove_are_authority_bound() {
        let (_directory, state, installation, runtime, mut record) =
            crate::providers::test_mutation_fixture_for(
                "qemu",
                "qcow2",
                crate::core::image::GuestOs::Linux,
            );
        let mutation = state.acquire_mutation_lock().unwrap();
        fs::write(&record.image.path, b"base").unwrap();
        fs::write(&record.ownership.overlay_path, b"overlay").unwrap();
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: Some(record.image.path.clone()),
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            state.root().join("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        let request = CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: record.ownership.configuration_path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
            parent_path: record.image.path.clone(),
            memory_mib: record.spec.memory_mib,
            cpu_count: record.spec.cpu_count,
            accelerator: Some("tcg".to_owned()),
            allow_tcg: true,
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
        let identity = provider.create_vm(&authority, &request).unwrap();
        record.provider_object = Some(crate::core::ownership::ProviderObjectIdentity {
            id: identity.id.clone(),
            name: identity.name,
        });
        record.phase = crate::core::cell::CellPhase::ProviderObjectCreated;
        let initial = provider
            .inspect_vm(&VmLookup::Id(identity.id))
            .unwrap()
            .unwrap();
        assert_eq!(initial.power_state, ProviderPowerState::Off);
        assert!(initial.ownership_marker.is_empty());
        let claim = ClaimVmRequest {
            expected: initial,
            ownership_marker: record.ownership.provider_marker.clone(),
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
        let claimed = provider.claim_vm(&authority, &claim).unwrap();
        record.phase = crate::core::cell::CellPhase::ProviderObjectClaimed;
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
        let configured = provider
            .configure_vm(
                &authority,
                &ConfigureVmRequest {
                    expected: claimed,
                    cpu_count: record.spec.cpu_count,
                },
            )
            .unwrap();
        record.phase = crate::core::cell::CellPhase::Ready;
        record.state = crate::core::cell::CellState::Stopped;
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
        assert!(provider.start_vm(&authority, &configured).is_err());
        let persisted = read_config(&QemuProvider::<FakeExecutor>::config_path(
            &configured.configuration_path,
        ))
        .unwrap();
        assert_eq!(persisted.process_id, Some(4242));
        assert_eq!(persisted.process_executable_sha256, Some("a".repeat(64)));
        provider
            .executor
            .qmp_sessions
            .lock()
            .unwrap()
            .push_back(qmp_status_session(&configured, "paused"));
        provider
            .executor
            .process_matches
            .store(false, Ordering::SeqCst);
        assert!(matches!(
            provider.inspect_vm(&VmLookup::Id(configured.id.clone())),
            Err(ProviderError::OwnershipChanged(_))
        ));
        provider
            .executor
            .process_matches
            .store(true, Ordering::SeqCst);
        provider
            .executor
            .qmp_sessions
            .lock()
            .unwrap()
            .push_back(qmp_status_session(&configured, "paused"));
        let paused = provider
            .inspect_vm(&VmLookup::Id(configured.id.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(paused.power_state, ProviderPowerState::Paused);
        provider
            .executor
            .qmp_sessions
            .lock()
            .unwrap()
            .push_back(qmp_start_session(&paused));
        provider
            .executor
            .process_matches
            .store(false, Ordering::SeqCst);
        assert!(matches!(
            provider.start_vm(&authority, &paused),
            Err(ProviderError::OwnershipChanged(_))
        ));
        assert_eq!(provider.executor.qmp_sessions.lock().unwrap().len(), 1);
        provider
            .executor
            .process_matches
            .store(true, Ordering::SeqCst);
        provider.start_vm(&authority, &paused).unwrap();
        let mut running = paused;
        running.power_state = ProviderPowerState::Running;
        provider
            .executor
            .qmp_sessions
            .lock()
            .unwrap()
            .push_back(qmp_stop_session(&running));
        provider
            .executor
            .process_matches
            .store(false, Ordering::SeqCst);
        assert!(matches!(
            provider.stop_vm(&authority, &running),
            Err(ProviderError::OwnershipChanged(_))
        ));
        assert_eq!(provider.executor.qmp_sessions.lock().unwrap().len(), 1);
        provider
            .executor
            .process_matches
            .store(true, Ordering::SeqCst);
        provider.stop_vm(&authority, &running).unwrap();
        assert!(
            provider
                .executor
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call == "process_absence_proven")
        );
        let stopped = provider
            .inspect_vm(&VmLookup::Id(running.id.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(stopped.power_state, ProviderPowerState::Off);
        let stopped_config = read_config(&QemuProvider::<FakeExecutor>::config_path(
            &stopped.configuration_path,
        ))
        .unwrap();
        assert!(stopped_config.process_executable_sha256.is_none());
        #[cfg(unix)]
        {
            let ControlEndpoint::Unix(qga_path) = &stopped_config.qga else {
                unreachable!();
            };
            fs::write(qga_path, b"foreign").unwrap();
            assert!(matches!(
                provider.remove_vm(&authority, &stopped),
                Err(ProviderError::OwnershipChanged(_))
            ));
            assert_eq!(fs::read(qga_path).unwrap(), b"foreign");
            fs::remove_file(qga_path).unwrap();
        }
        provider.remove_vm(&authority, &stopped).unwrap();
        assert!(
            provider
                .inspect_vm(&VmLookup::Id(record.provider_object.unwrap().id))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_vm_rejects_overlong_control_endpoints_before_persisting_config() {
        let (_directory, state, installation, runtime, mut record) =
            crate::providers::test_mutation_fixture_for(
                "qemu",
                "qcow2",
                crate::core::image::GuestOs::Linux,
            );
        drop(runtime);
        let configuration_path = state
            .cell_runtime_root(record.id)
            .join("q".repeat(UNIX_CONTROL_ENDPOINT_LIMIT + 1));
        record.ownership.configuration_path = configuration_path.clone();
        let runtime = state
            .prepare_cell_runtime_for(
                record.id,
                configuration_path.clone(),
                record.ownership.overlay_path.clone(),
            )
            .unwrap();
        let mutation = state.acquire_mutation_lock().unwrap();
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: Some(record.image.path.clone()),
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            state.root().join("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        let request = CreateVmRequest {
            name: record.ownership.provider_object_name.clone(),
            configuration_path: configuration_path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
            parent_path: record.image.path.clone(),
            memory_mib: record.spec.memory_mib,
            cpu_count: record.spec.cpu_count,
            accelerator: Some("tcg".to_owned()),
            allow_tcg: true,
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);

        assert!(matches!(
            provider.create_vm(&authority, &request),
            Err(ProviderError::InvalidResponse(_))
        ));
        assert!(!configuration_path.join("vm.json").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn overlay_creation_uses_staging_and_preserves_final_path_boundaries() {
        let (_directory, state, installation, runtime, record) =
            crate::providers::test_mutation_fixture_for(
                "qemu",
                "qcow2",
                crate::core::image::GuestOs::Linux,
            );
        let mutation = state.acquire_mutation_lock().unwrap();
        fs::write(&record.image.path, b"base").unwrap();
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: Some(record.image.path.clone()),
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            state.root().join("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        let request = CreateOverlayRequest {
            parent_path: record.image.path.clone(),
            overlay_path: record.ownership.overlay_path.clone(),
        };
        let authority = ProviderMutationAuthority::new(&record, &installation, &runtime, &mutation);
        let overlay = provider.create_overlay(&authority, &request).unwrap();
        assert_eq!(overlay.path, request.overlay_path);
        assert!(request.overlay_path.is_file());
        assert!(
            fs::read_dir(request.overlay_path.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::remove_file(&request.overlay_path).unwrap();
            let external = state.root().join("external.qcow2");
            fs::write(&external, b"retain").unwrap();
            symlink(&external, &request.overlay_path).unwrap();
            assert!(matches!(
                provider.create_overlay(&authority, &request),
                Err(ProviderError::Collision(_))
            ));
            assert_eq!(fs::read(&external).unwrap(), b"retain");
        }
    }

    #[test]
    fn tcg_is_never_selected_without_explicit_opt_in() {
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: None,
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            PathBuf::from("runtime"),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        let request = CreateVmRequest {
            name: format!("vmcell-{}", Uuid::nil()),
            configuration_path: PathBuf::from("runtime/qemu"),
            overlay_path: PathBuf::from("runtime/cell.qcow2"),
            parent_path: PathBuf::from("base.qcow2"),
            memory_mib: 1024,
            cpu_count: 1,
            accelerator: Some("tcg".to_owned()),
            allow_tcg: false,
        };
        assert!(provider.select_accelerator(&request).is_err());

        let auto = CreateVmRequest {
            accelerator: Some("auto".to_owned()),
            allow_tcg: true,
            ..request
        };
        assert!(provider.select_accelerator(&auto).is_err());
    }

    fn qmp_start_session(expected: &ProviderVm) -> Vec<u8> {
        qmp_lifecycle_session(
            expected,
            "{\"return\":{},\"id\":8}\n{\"return\":{\"status\":\"running\"},\"id\":9}\n",
        )
    }

    fn qmp_stop_session(expected: &ProviderVm) -> Vec<u8> {
        qmp_lifecycle_session(expected, "{\"return\":{},\"id\":8}\n")
    }

    fn qmp_status_session(expected: &ProviderVm, status: &str) -> Vec<u8> {
        qmp_lifecycle_session(
            expected,
            &format!("{{\"return\":{{\"status\":\"{status}\"}},\"id\":8}}\n"),
        )
    }

    fn qmp_lifecycle_session(expected: &ProviderVm, final_responses: &str) -> Vec<u8> {
        let cpus = (0..expected.cpu_count)
            .map(|index| serde_json::json!({"cpu-index": index}))
            .collect::<Vec<_>>();
        let memory = expected.memory_mib * 1024 * 1024;
        format!(
            concat!(
                "{{\"QMP\":{{}}}}\n",
                "{{\"return\":{{}},\"id\":1}}\n",
                "{{\"return\":{{\"UUID\":\"{id}\"}},\"id\":2}}\n",
                "{{\"return\":{{\"name\":\"{name}\"}},\"id\":3}}\n",
                "{{\"return\":{cpus},\"id\":4}}\n",
                "{{\"return\":{{\"base-memory\":{memory}}},\"id\":5}}\n",
                "{{\"return\":[],\"id\":6}}\n",
                "{{\"return\":[{{\"inserted\":{{\"drv\":\"qcow2\",\"file\":\"{overlay}\",\"backing_file_depth\":1}}}}],\"id\":7}}\n",
                "{final_responses}"
            ),
            id = expected.id,
            name = expected.name,
            cpus = serde_json::to_string(&cpus).unwrap(),
            memory = memory,
            overlay = expected.attached_disks[0]
                .to_string_lossy()
                .replace('\\', "\\\\"),
            final_responses = final_responses,
        )
        .into_bytes()
    }

    #[test]
    fn qmp_runtime_definition_drift_fails_closed() {
        let config = QemuVmConfig {
            schema_version: 1,
            id: Uuid::nil().to_string(),
            name: format!("vmcell-{}", Uuid::nil()),
            ownership_marker: "marker".to_owned(),
            configuration_path: PathBuf::from("runtime/qemu"),
            overlay_path: PathBuf::from("runtime/cell.qcow2"),
            parent_path: PathBuf::from("base.qcow2"),
            cpu_count: 1,
            memory_mib: 1024,
            accelerator: "tcg".to_owned(),
            qmp: ControlEndpoint::Unix(PathBuf::from("qmp.sock")),
            qga: ControlEndpoint::Unix(PathBuf::from("qga.sock")),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: Some(42),
            process_start_token: Some(7),
            process_executable_sha256: Some("a".repeat(64)),
        };
        let valid = qmp_start_session(&config.snapshot(ProviderPowerState::Paused));
        let drifted = String::from_utf8(valid)
            .unwrap()
            .replace(
                "{\"return\":[],\"id\":6}",
                "{\"return\":[{\"name\":\"foreign-nic\"}],\"id\":6}",
            )
            .into_bytes();
        let stream = FakeQmpStream {
            reads: drifted.into(),
            writes: Vec::new(),
        };
        let mut qmp = QmpClient::negotiate(stream, CONTROL_TIMEOUT).unwrap();
        assert!(matches!(
            validate_qmp_identity(&mut qmp, &config),
            Err(ProviderError::OwnershipChanged(_))
        ));
    }

    #[test]
    fn persisted_qemu_configuration_is_bounded_and_schema_strict() {
        let directory = tempfile::tempdir().unwrap();
        let configuration_path = directory.path().join("qemu");
        create_ordinary_directory(&configuration_path).unwrap();
        let id = Uuid::new_v4().to_string();
        let mut config = QemuVmConfig {
            schema_version: QEMU_CONFIG_SCHEMA,
            id: id.clone(),
            name: format!("vmcell-{id}"),
            ownership_marker: "marker".to_owned(),
            configuration_path: configuration_path.clone(),
            overlay_path: directory.path().join("cell.qcow2"),
            parent_path: directory.path().join("base.qcow2"),
            cpu_count: 1,
            memory_mib: 1024,
            accelerator: "tcg".to_owned(),
            qmp: ControlEndpoint::qmp(&configuration_path, &id),
            qga: ControlEndpoint::qga(&configuration_path, &id),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: None,
            process_start_token: None,
            process_executable_sha256: None,
        };
        config.command_sha256 = launch_digest(&config);
        let path = configuration_path.join("vm.json");
        write_config_new(&path, &config).unwrap();
        assert!(read_config(&path).is_ok());

        #[cfg(unix)]
        {
            let snapshot =
                read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete).unwrap();
            let mut external = snapshot.config.clone();
            external.ownership_marker = "external-marker".to_owned();
            let external_path = configuration_path.join("external.json");
            write_config_new(&external_path, &external).unwrap();
            fs::rename(&external_path, &path).unwrap();
            let mut replacement = snapshot.config.clone();
            replacement.ownership_marker = "replacement-marker".to_owned();
            assert!(matches!(
                replace_config(&path, snapshot, &replacement),
                Err(ProviderError::OwnershipChanged(message))
                    if message == "QEMU configuration file identity changed while it was opened"
            ));
            assert_eq!(
                read_config(&path).unwrap().ownership_marker,
                "external-marker"
            );
        }

        #[cfg(not(unix))]
        {
            let snapshot =
                read_config_snapshot(&path, QemuFileSharePolicy::DenyWriteAndDelete).unwrap();
            #[cfg(windows)]
            assert!(OpenOptions::new().write(true).open(&path).is_err());
            let mut replacement = snapshot.config.clone();
            replacement.ownership_marker = "replacement-marker".to_owned();
            replace_config(&path, snapshot, &replacement).unwrap();
            assert_eq!(
                read_config(&path).unwrap().ownership_marker,
                "replacement-marker"
            );
        }

        let mut unknown_field = serde_json::to_value(&config).unwrap();
        unknown_field["unexpected"] = serde_json::json!(true);
        fs::write(&path, serde_json::to_vec(&unknown_field).unwrap()).unwrap();
        assert!(matches!(
            read_config(&path),
            Err(ProviderError::InvalidResponse(message))
                if message == "QEMU configuration JSON is invalid"
        ));

        fs::write(&path, vec![b' '; QEMU_CONFIG_MAX_BYTES + 1]).unwrap();
        assert!(matches!(
            read_config(&path),
            Err(ProviderError::InvalidResponse(message))
                if message == "QEMU configuration exceeds the maximum size"
        ));

        let mut oversized_config = config.clone();
        oversized_config.ownership_marker = "x".repeat(QEMU_CONFIG_MAX_BYTES);
        let oversized_path = configuration_path.join("oversized.json");
        assert!(matches!(
            write_config_new(&oversized_path, &oversized_config),
            Err(ProviderError::InvalidResponse(message))
                if message == "QEMU configuration exceeds the maximum size"
        ));
        assert!(!oversized_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_qemu_paths_reject_aliases_before_config_or_overlay_access() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        assert!(qemu_argument_path_is_safe(Path::new(
            r"C:\vmcell\runtime\cell.qcow2"
        )));
        for unsafe_path in [
            r"C:\vmcell\runtime\cell.qcow2:stream",
            r"C:\vmcell\runtime\NUL.qcow2",
            r"\\.\NUL",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\cell.qcow2",
        ] {
            assert!(!qemu_argument_path_is_safe(Path::new(unsafe_path)));
        }

        let directory = tempfile::tempdir().unwrap();
        let target_directory = directory.path().join("target-qemu");
        fs::create_dir(&target_directory).unwrap();
        let aliased_directory = directory.path().join("aliased-qemu");
        if symlink_dir(&target_directory, &aliased_directory).is_err() {
            return;
        }
        assert!(matches!(
            create_ordinary_directory(&aliased_directory),
            Err(ProviderError::OwnershipChanged(_))
        ));

        let configuration_path = directory.path().join("qemu");
        create_ordinary_directory(&configuration_path).unwrap();
        let target_config = directory.path().join("target.json");
        fs::write(&target_config, b"{}").unwrap();
        let config_path = configuration_path.join("vm.json");
        if symlink_file(&target_config, &config_path).is_ok() {
            assert!(matches!(
                read_config(&config_path),
                Err(ProviderError::OwnershipChanged(_))
            ));
        }

        let target_overlay = directory.path().join("target.qcow2");
        fs::write(&target_overlay, b"foreign").unwrap();
        let overlay_path = configuration_path.join("cell.qcow2");
        if symlink_file(&target_overlay, &overlay_path).is_ok() {
            assert!(matches!(
                open_private_qemu_file(&overlay_path),
                Err(ProviderError::OwnershipChanged(_))
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_staged_overlay_cleanup_can_remove_the_pinned_file() {
        let directory = tempfile::tempdir().unwrap();
        let staged_path = directory.path().join("staged.qcow2");
        fs::write(&staged_path, b"overlay").unwrap();
        let staged_file = open_private_qemu_file(&staged_path).unwrap();
        cleanup_exact_qemu_file(&staged_path, staged_file);
        assert!(!staged_path.exists());
    }

    #[test]
    fn config_endpoint_digest_and_spawn_receipt_gates_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let configuration_path = directory.path().join("qemu");
        let id = Uuid::new_v4().to_string();
        let mut config = QemuVmConfig {
            schema_version: QEMU_CONFIG_SCHEMA,
            id: id.clone(),
            name: format!("vmcell-{id}"),
            ownership_marker: "marker".to_owned(),
            configuration_path: configuration_path.clone(),
            overlay_path: directory.path().join("cell.qcow2"),
            parent_path: directory.path().join("base.qcow2"),
            cpu_count: 1,
            memory_mib: 1024,
            accelerator: "tcg".to_owned(),
            qmp: ControlEndpoint::qmp(&configuration_path, &id),
            qga: ControlEndpoint::qga(&configuration_path, &id),
            command_sha256: String::new(),
            spawn_pending: false,
            process_id: None,
            process_start_token: None,
            process_executable_sha256: None,
        };
        config.command_sha256 = launch_digest(&config);
        let path = configuration_path.join("vm.json");
        assert!(config.validate(&path).is_ok());
        config.overlay_path = directory.path().join("foreign,format=raw.qcow2");
        config.command_sha256 = launch_digest(&config);
        assert!(config.validate(&path).is_err());
        config.overlay_path = directory.path().join("cell.qcow2");
        config.command_sha256 = launch_digest(&config);
        config.process_id = Some(42);
        config.process_start_token = Some(0);
        assert!(config.validate(&path).is_err());
        config.process_start_token = Some(7);
        assert!(config.validate(&path).is_err());
        config.process_executable_sha256 = Some("not-a-hash".to_owned());
        assert!(config.validate(&path).is_err());
        config.process_executable_sha256 = Some("b".repeat(64));
        assert!(config.validate(&path).is_ok());
        config.process_id = None;
        config.process_start_token = None;
        config.process_executable_sha256 = None;
        config.qmp = ControlEndpoint::WindowsPipe("foreign".to_owned());
        assert!(config.validate(&path).is_err());
        config.qmp = ControlEndpoint::qmp(&configuration_path, &id);
        config.command_sha256 = "tampered".to_owned();
        assert!(config.validate(&path).is_err());

        config.command_sha256 = launch_digest(&config);
        config.spawn_pending = true;
        let provider = QemuProvider::new(
            FakeExecutor {
                accelerators: vec!["tcg".to_owned()],
                calls: Mutex::new(Vec::new()),
                failed_program: None,
                backing: Some(config.parent_path.clone()),
                qmp_sessions: Mutex::new(VecDeque::new()),
                process_matches: AtomicBool::new(true),
            },
            directory.path().to_path_buf(),
            "qemu-system-test".into(),
            "qemu-img".into(),
        );
        assert!(matches!(
            provider.inspect_config(&config),
            Err(ProviderError::OwnershipChanged(_))
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn process_absence_rejects_unknown_and_live_instance_receipts() {
        let process_id = std::process::id();
        let start_token = process_start_token(process_id).unwrap();
        assert_ne!(start_token, 0);
        assert!(!process_absence_proven(process_id, 0));
        assert!(!process_absence_proven(process_id, start_token));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_executable_resolution_and_process_receipt_bind_exact_identity() {
        let directory = tempfile::tempdir().unwrap();
        let candidate = directory.path().join("qemu-system-x86_64.exe");
        fs::write(&candidate, b"fixture-executable").unwrap();
        let resolved = resolve_windows_executable(
            OsStr::new("qemu-system-x86_64"),
            &[directory.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(PathBuf::from(resolved), candidate.canonicalize().unwrap());
        let wrong_extension = directory.path().join("qemu-system-x86_64.com");
        fs::write(&wrong_extension, b"fixture-executable").unwrap();
        assert!(
            resolve_windows_executable(
                wrong_extension.as_os_str(),
                &[directory.path().to_path_buf()]
            )
            .is_none()
        );

        let current = std::env::current_exe().unwrap().canonicalize().unwrap();
        let process_id = std::process::id();
        let start_token = process_start_token(process_id).unwrap();
        let executable_sha256 = ordinary_file_sha256(&current).unwrap();
        assert!(process_matches(
            process_id,
            start_token,
            current.as_os_str(),
            "not-observable-on-windows",
            &executable_sha256,
        ));
        assert!(!process_matches(
            process_id,
            start_token,
            current.as_os_str(),
            "not-observable-on-windows",
            &"0".repeat(64),
        ));
        assert!(!process_matches(
            process_id,
            start_token.saturating_add(1),
            current.as_os_str(),
            "not-observable-on-windows",
            &executable_sha256,
        ));
    }
}
