use std::io::{Read, Write};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::guest::{
    GuestActionAuthority, GuestCommand, GuestCommandResult, GuestCopyInAction, GuestCopyOutAction,
    GuestCredentials, GuestIoError, GuestReadiness, GuestTransport, OverwritePolicy,
};
use crate::providers::qemu::protocol::{ControlEndpoint, QmpClient, ReadWrite, connect_endpoint};
use crate::providers::{ProviderError, ProviderVm};

const QGA_FRAME_LIMIT: usize = 24 * 1024 * 1024;
const QGA_FILE_CHUNK: usize = 48 * 1024;

pub(crate) trait QgaCommandExecutor: Send + Sync {
    fn prove_vm(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        timeout: Duration,
    ) -> Result<(), GuestIoError>;
    fn probe(&self, endpoint: &ControlEndpoint, timeout: Duration) -> Result<(), GuestIoError>;
    fn exec(
        &self,
        endpoint: &ControlEndpoint,
        command: &GuestCommand,
    ) -> Result<GuestCommandResult, GuestIoError>;
    fn copy_in(
        &self,
        endpoint: &ControlEndpoint,
        cell_id: crate::core::cell::CellId,
        action: GuestCopyInAction<'_>,
    ) -> Result<(), GuestIoError>;
    fn copy_out(
        &self,
        endpoint: &ControlEndpoint,
        cell_id: crate::core::cell::CellId,
        action: GuestCopyOutAction<'_>,
    ) -> Result<Vec<u8>, GuestIoError>;
}

pub struct SystemQgaExecutor;

impl QgaCommandExecutor for SystemQgaExecutor {
    fn prove_vm(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        timeout: Duration,
    ) -> Result<(), GuestIoError> {
        let endpoint =
            ControlEndpoint::qmp(authority.configuration_path(), authority.provider_id());
        let deadline = Instant::now() + timeout;
        let stream = connect_endpoint(
            &endpoint,
            remaining_guest_duration(deadline)?.min(Duration::from_secs(5)),
        )
        .map_err(provider_to_guest)?;
        let mut qmp = QmpClient::negotiate(stream, remaining_guest_duration(deadline)?)
            .map_err(provider_to_guest)?;
        prove_qmp_snapshot(&mut qmp, expected)
    }

    fn probe(&self, endpoint: &ControlEndpoint, timeout: Duration) -> Result<(), GuestIoError> {
        let mut client = QgaClient::connect(endpoint, timeout)?;
        client.execute("guest-info", None)?;
        Ok(())
    }

    fn exec(
        &self,
        endpoint: &ControlEndpoint,
        command: &GuestCommand,
    ) -> Result<GuestCommandResult, GuestIoError> {
        command.validate()?;
        let mut client = QgaClient::connect(endpoint, command.timeout)?;
        client.exec(command)
    }

    fn copy_in(
        &self,
        endpoint: &ControlEndpoint,
        cell_id: crate::core::cell::CellId,
        action: GuestCopyInAction<'_>,
    ) -> Result<(), GuestIoError> {
        let mut client = QgaClient::connect(endpoint, action.timeout)?;
        let root = workspace_root(cell_id);
        let destination = format!("{root}/{}", action.destination.as_posix());
        let parent = destination
            .rsplit_once('/')
            .map(|value| value.0)
            .unwrap_or(&root);
        client.exec_simple("/bin/mkdir", &["-p", parent], action.timeout)?;
        let temporary = format!("{destination}.vmcell-{}.tmp", action.operation_id);
        let handle = client.file_open(&temporary, "w")?;
        let write_result = client.file_write_all(handle, action.content);
        let close_result = client.file_close(handle);
        write_result?;
        close_result?;
        client.commit_copy_in(&temporary, &destination, action.overwrite, action.timeout)
    }

    fn copy_out(
        &self,
        endpoint: &ControlEndpoint,
        cell_id: crate::core::cell::CellId,
        action: GuestCopyOutAction<'_>,
    ) -> Result<Vec<u8>, GuestIoError> {
        let mut client = QgaClient::connect(endpoint, action.timeout)?;
        let source = format!("{}/{}", workspace_root(cell_id), action.source.as_posix());
        let handle = client.file_open(&source, "r")?;
        let read_result = client.file_read_all(handle, action.max_bytes);
        let close_result = client.file_close(handle);
        let bytes = read_result?;
        close_result?;
        Ok(bytes)
    }
}

pub struct QemuGuestAgentTransport<E = SystemQgaExecutor> {
    executor: E,
}

impl QemuGuestAgentTransport<SystemQgaExecutor> {
    #[must_use]
    pub fn system() -> Self {
        Self::new(SystemQgaExecutor)
    }
}

impl<E> QemuGuestAgentTransport<E> {
    pub(crate) fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: QgaCommandExecutor> GuestTransport for QemuGuestAgentTransport<E> {
    fn name(&self) -> &'static str {
        "qga"
    }

    fn supports(&self, provider: &str, guest_os: crate::core::image::GuestOs) -> bool {
        provider == "qemu" && guest_os == crate::core::image::GuestOs::Linux
    }

    fn probe_ready(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        _credentials: &GuestCredentials,
        timeout: Duration,
    ) -> Result<GuestReadiness, GuestIoError> {
        authority.validate(expected)?;
        if authority.provider() != "qemu" {
            return Err(GuestIoError::OwnershipChanged);
        }
        self.executor.prove_vm(authority, expected, timeout)?;
        let endpoint =
            ControlEndpoint::qga(authority.configuration_path(), authority.provider_id());
        match self.executor.probe(&endpoint, timeout) {
            Ok(()) => Ok(GuestReadiness::Ready),
            Err(GuestIoError::GuestNotReady | GuestIoError::Transport) => {
                Ok(GuestReadiness::GuestNotReady)
            }
            Err(error) => Err(error),
        }
    }

    fn exec(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        _credentials: &GuestCredentials,
        command: &GuestCommand,
    ) -> Result<GuestCommandResult, GuestIoError> {
        authority.validate(expected)?;
        self.executor
            .prove_vm(authority, expected, command.timeout)?;
        let endpoint =
            ControlEndpoint::qga(authority.configuration_path(), authority.provider_id());
        self.executor.exec(&endpoint, command)
    }

    fn copy_in(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        _credentials: &GuestCredentials,
        action: GuestCopyInAction<'_>,
    ) -> Result<(), GuestIoError> {
        authority.validate(expected)?;
        self.executor
            .prove_vm(authority, expected, action.timeout)?;
        let endpoint =
            ControlEndpoint::qga(authority.configuration_path(), authority.provider_id());
        self.executor
            .copy_in(&endpoint, authority.cell_id(), action)
    }

    fn copy_out(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        _credentials: &GuestCredentials,
        action: GuestCopyOutAction<'_>,
    ) -> Result<Vec<u8>, GuestIoError> {
        authority.validate(expected)?;
        self.executor
            .prove_vm(authority, expected, action.timeout)?;
        let endpoint =
            ControlEndpoint::qga(authority.configuration_path(), authority.provider_id());
        self.executor
            .copy_out(&endpoint, authority.cell_id(), action)
    }
}

struct QgaClient {
    stream: Box<dyn ReadWrite>,
    deadline: Instant,
}

impl QgaClient {
    fn connect(endpoint: &ControlEndpoint, timeout: Duration) -> Result<Self, GuestIoError> {
        let deadline = Instant::now() + timeout;
        let mut client = Self {
            stream: connect_endpoint(
                endpoint,
                remaining_guest_duration(deadline)?.min(Duration::from_secs(5)),
            )
            .map_err(provider_to_guest)?,
            deadline,
        };
        client
            .stream
            .set_operation_deadline(client.deadline)
            .map_err(|_| GuestIoError::Timeout)?;
        client
            .stream
            .write_all(&[0xff])
            .map_err(|_| GuestIoError::Transport)?;
        let sync_id = Uuid::new_v4().as_u128() as u64;
        let response = client.execute("guest-sync-delimited", Some(json!({"id": sync_id})))?;
        if response.as_u64() != Some(sync_id) {
            return Err(GuestIoError::InvalidResponse);
        }
        Ok(client)
    }

    fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value, GuestIoError> {
        if Instant::now() >= self.deadline {
            return Err(GuestIoError::Timeout);
        }
        let mut request = json!({"execute": command});
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        let mut bytes = serde_json::to_vec(&request).map_err(|_| GuestIoError::InvalidResponse)?;
        if bytes.len() > QGA_FRAME_LIMIT {
            return Err(GuestIoError::OutputLimit);
        }
        bytes.push(b'\n');
        self.stream
            .set_operation_deadline(self.deadline)
            .map_err(|_| GuestIoError::Timeout)?;
        self.stream
            .write_all(&bytes)
            .and_then(|_| self.stream.flush())
            .map_err(|_| GuestIoError::Transport)?;
        let response = self.receive()?;
        if Instant::now() >= self.deadline {
            return Err(GuestIoError::Timeout);
        }
        if response.get("error").is_some() {
            return Err(GuestIoError::Transport);
        }
        response
            .get("return")
            .cloned()
            .ok_or(GuestIoError::InvalidResponse)
    }

    fn receive(&mut self) -> Result<Value, GuestIoError> {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        let mut started = false;
        let mut skipped = 0_usize;
        loop {
            self.stream
                .set_operation_deadline(self.deadline)
                .map_err(|_| GuestIoError::Timeout)?;
            if self
                .stream
                .read(&mut byte)
                .map_err(|_| GuestIoError::Transport)?
                == 0
            {
                return Err(GuestIoError::InvalidResponse);
            }
            if !started {
                if byte[0] != b'{' {
                    skipped += 1;
                    if skipped >= QGA_FRAME_LIMIT {
                        return Err(GuestIoError::OutputLimit);
                    }
                    continue;
                }
                started = true;
            }
            if byte[0] == b'\n' {
                return serde_json::from_slice(&bytes).map_err(|_| GuestIoError::InvalidResponse);
            }
            if bytes.len() >= QGA_FRAME_LIMIT {
                return Err(GuestIoError::OutputLimit);
            }
            bytes.push(byte[0]);
        }
    }

    fn exec(&mut self, command: &GuestCommand) -> Result<GuestCommandResult, GuestIoError> {
        let response = self.execute(
            "guest-exec",
            Some(json!({
                "path": command.program,
                "arg": command.args,
                "capture-output": true
            })),
        )?;
        let pid = response
            .get("pid")
            .and_then(Value::as_u64)
            .ok_or(GuestIoError::InvalidResponse)?;
        let deadline = Instant::now() + command.timeout;
        loop {
            let status = self.execute("guest-exec-status", Some(json!({"pid": pid})))?;
            if status.get("exited").and_then(Value::as_bool) == Some(true) {
                for field in ["out-truncated", "err-truncated"] {
                    if !matches!(status.get(field), None | Some(Value::Bool(false))) {
                        return Err(GuestIoError::InvalidResponse);
                    }
                }
                let stdout = decode_output(&status, "out-data", command.max_output_bytes)?;
                let stderr = decode_output(&status, "err-data", command.max_output_bytes)?;
                if (stdout.len() + stderr.len()) as u64 > command.max_output_bytes {
                    return Err(GuestIoError::OutputLimit);
                }
                let stdout =
                    String::from_utf8(stdout).map_err(|_| GuestIoError::InvalidResponse)?;
                let stderr =
                    String::from_utf8(stderr).map_err(|_| GuestIoError::InvalidResponse)?;
                return Ok(GuestCommandResult {
                    exit_code: status
                        .get("exitcode")
                        .and_then(Value::as_i64)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or(GuestIoError::InvalidResponse)?,
                    stdout_bytes: stdout.len() as u64,
                    stderr_bytes: stderr.len() as u64,
                    stdout,
                    stderr,
                    encoding: "utf-8".to_owned(),
                    truncated: false,
                });
            }
            if Instant::now() >= deadline {
                return Err(GuestIoError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn exec_simple(
        &mut self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<(), GuestIoError> {
        if self.exec_exit_code(program, args, timeout)? == 0 {
            Ok(())
        } else {
            Err(GuestIoError::Transport)
        }
    }

    fn commit_copy_in(
        &mut self,
        temporary: &str,
        destination: &str,
        overwrite: OverwritePolicy,
        timeout: Duration,
    ) -> Result<(), GuestIoError> {
        if overwrite == OverwritePolicy::Deny {
            let committed =
                self.exec_exit_code("/bin/ln", &["-T", "--", temporary, destination], timeout)?;
            if committed != 0 {
                self.exec_simple("/bin/rm", &["--", temporary], timeout)?;
                return Err(GuestIoError::PathViolation);
            }
            self.exec_simple("/bin/rm", &["--", temporary], timeout)
        } else {
            self.exec_simple("/bin/mv", &["-f", "--", temporary, destination], timeout)
        }
    }

    fn exec_exit_code(
        &mut self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<i32, GuestIoError> {
        Ok(self
            .exec(&GuestCommand {
                program: program.to_owned(),
                args: args.iter().map(|value| (*value).to_owned()).collect(),
                timeout,
                max_output_bytes: 64 * 1024,
            })?
            .exit_code)
    }

    fn file_open(&mut self, path: &str, mode: &str) -> Result<i64, GuestIoError> {
        self.execute("guest-file-open", Some(json!({"path": path, "mode": mode})))?
            .as_i64()
            .ok_or(GuestIoError::InvalidResponse)
    }

    fn file_close(&mut self, handle: i64) -> Result<(), GuestIoError> {
        self.execute("guest-file-close", Some(json!({"handle": handle})))
            .map(|_| ())
    }

    fn file_write_all(&mut self, handle: i64, content: &[u8]) -> Result<(), GuestIoError> {
        for chunk in content.chunks(QGA_FILE_CHUNK) {
            let response = self.execute(
                "guest-file-write",
                Some(json!({
                    "handle": handle,
                    "buf-b64": base64::engine::general_purpose::STANDARD.encode(chunk)
                })),
            )?;
            if response.get("count").and_then(Value::as_u64) != Some(chunk.len() as u64) {
                return Err(GuestIoError::PartialCopy);
            }
        }
        self.execute("guest-file-flush", Some(json!({"handle": handle})))
            .map(|_| ())
    }

    fn file_read_all(&mut self, handle: i64, max_bytes: u64) -> Result<Vec<u8>, GuestIoError> {
        let mut output = Vec::new();
        loop {
            let response = self.execute(
                "guest-file-read",
                Some(json!({"handle": handle, "count": QGA_FILE_CHUNK})),
            )?;
            let chunk = response
                .get("buf-b64")
                .and_then(Value::as_str)
                .map(|value| {
                    base64::engine::general_purpose::STANDARD
                        .decode(value)
                        .map_err(|_| GuestIoError::InvalidResponse)
                })
                .transpose()?
                .unwrap_or_default();
            let count = response
                .get("count")
                .and_then(Value::as_u64)
                .ok_or(GuestIoError::InvalidResponse)?;
            let eof = response
                .get("eof")
                .and_then(Value::as_bool)
                .ok_or(GuestIoError::InvalidResponse)?;
            if count != chunk.len() as u64 || count > QGA_FILE_CHUNK as u64 {
                return Err(GuestIoError::PartialCopy);
            }
            output.extend_from_slice(&chunk);
            if output.len() as u64 > max_bytes {
                return Err(GuestIoError::OutputLimit);
            }
            if eof {
                return Ok(output);
            }
            if chunk.is_empty() {
                return Err(GuestIoError::InvalidResponse);
            }
        }
    }
}

fn prove_qmp_snapshot<S: ReadWrite>(
    qmp: &mut QmpClient<S>,
    expected: &ProviderVm,
) -> Result<(), GuestIoError> {
    let uuid = qmp.execute("query-uuid", None).map_err(provider_to_guest)?;
    let name = qmp.execute("query-name", None).map_err(provider_to_guest)?;
    let cpus = qmp
        .execute("query-cpus-fast", None)
        .map_err(provider_to_guest)?;
    let memory = qmp
        .execute("query-memory-size-summary", None)
        .map_err(provider_to_guest)?;
    let network = qmp
        .execute("query-rx-filter", None)
        .map_err(provider_to_guest)?;
    let blocks = qmp
        .execute("query-block", None)
        .map_err(provider_to_guest)?;
    let status = qmp
        .execute("query-status", None)
        .map_err(provider_to_guest)?;
    let block = blocks
        .as_array()
        .and_then(|items| (items.len() == 1).then(|| &items[0]));
    let attached = block
        .and_then(|item| item.get("inserted"))
        .and_then(|item| item.get("file"))
        .and_then(Value::as_str);
    if uuid.get("UUID").and_then(Value::as_str) != Some(expected.id.as_str())
        || name.get("name").and_then(Value::as_str) != Some(expected.name.as_str())
        || cpus.as_array().map(Vec::len) != Some(expected.cpu_count as usize)
        || memory.get("base-memory").and_then(Value::as_u64)
            != Some(expected.memory_mib * 1024 * 1024)
        || network.as_array().is_none_or(|items| !items.is_empty())
        || block
            .and_then(|item| item.get("inserted"))
            .and_then(|item| item.get("drv"))
            .and_then(Value::as_str)
            != Some("qcow2")
        || block
            .and_then(|item| item.get("inserted"))
            .and_then(|item| item.get("backing_file_depth"))
            .and_then(Value::as_u64)
            != Some(1)
        || attached.is_none_or(|path| {
            expected.attached_disks.len() != 1
                || !qga_provider_path_equal(std::path::Path::new(path), &expected.attached_disks[0])
        })
        || status.get("status").and_then(Value::as_str) != Some("running")
    {
        return Err(GuestIoError::OwnershipChanged);
    }
    Ok(())
}

fn qga_provider_path_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    let canonical = |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.into());
    if cfg!(target_os = "windows") {
        canonical(left)
            .to_string_lossy()
            .eq_ignore_ascii_case(&canonical(right).to_string_lossy())
    } else {
        canonical(left) == canonical(right)
    }
}

fn decode_output(status: &Value, field: &str, limit: u64) -> Result<Vec<u8>, GuestIoError> {
    let Some(value) = status.get(field).and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    if value.len() as u64 > limit.saturating_mul(2) {
        return Err(GuestIoError::OutputLimit);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| GuestIoError::InvalidResponse)?;
    if bytes.len() as u64 > limit {
        return Err(GuestIoError::OutputLimit);
    }
    Ok(bytes)
}

fn workspace_root(cell_id: crate::core::cell::CellId) -> String {
    format!("/var/lib/vmcell/{cell_id}")
}

fn provider_to_guest(error: ProviderError) -> GuestIoError {
    match error {
        ProviderError::OwnershipChanged(_) | ProviderError::Authority(_) => {
            GuestIoError::OwnershipChanged
        }
        ProviderError::InvalidResponse(_) => GuestIoError::InvalidResponse,
        _ => GuestIoError::GuestNotReady,
    }
}

fn remaining_guest_duration(deadline: Instant) -> Result<Duration, GuestIoError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(GuestIoError::Timeout)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    use super::*;

    struct FakeQgaStream {
        reads: VecDeque<u8>,
        writes: Vec<u8>,
    }

    impl FakeQgaStream {
        fn scripted(lines: &[Value]) -> Self {
            let mut reads = VecDeque::new();
            for line in lines {
                reads.extend(serde_json::to_vec(line).unwrap());
                reads.push_back(b'\n');
            }
            Self {
                reads,
                writes: Vec::new(),
            }
        }
    }

    impl Read for FakeQgaStream {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            let count = target.len().min(self.reads.len());
            for slot in target.iter_mut().take(count) {
                *slot = self.reads.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl Write for FakeQgaStream {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl ReadWrite for FakeQgaStream {
        fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
            if Instant::now() >= deadline {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "scripted QGA deadline expired",
                ))
            } else {
                Ok(())
            }
        }
    }

    struct FakeQga {
        calls: Mutex<Vec<&'static str>>,
    }
    impl QgaCommandExecutor for FakeQga {
        fn prove_vm(
            &self,
            _: &GuestActionAuthority<'_>,
            _: &ProviderVm,
            _: Duration,
        ) -> Result<(), GuestIoError> {
            self.calls.lock().unwrap().push("prove_vm");
            Ok(())
        }
        fn probe(&self, _: &ControlEndpoint, _: Duration) -> Result<(), GuestIoError> {
            self.calls.lock().unwrap().push("probe");
            Ok(())
        }
        fn exec(
            &self,
            _: &ControlEndpoint,
            _: &GuestCommand,
        ) -> Result<GuestCommandResult, GuestIoError> {
            Err(GuestIoError::Transport)
        }
        fn copy_in(
            &self,
            _: &ControlEndpoint,
            _: crate::core::cell::CellId,
            _: GuestCopyInAction<'_>,
        ) -> Result<(), GuestIoError> {
            Err(GuestIoError::Transport)
        }
        fn copy_out(
            &self,
            _: &ControlEndpoint,
            _: crate::core::cell::CellId,
            _: GuestCopyOutAction<'_>,
        ) -> Result<Vec<u8>, GuestIoError> {
            Err(GuestIoError::Transport)
        }
    }

    #[test]
    fn qga_transport_is_credentialless_and_provider_scoped() {
        let transport = QemuGuestAgentTransport::new(FakeQga {
            calls: Mutex::new(Vec::new()),
        });
        assert!(transport.supports("qemu", crate::core::image::GuestOs::Linux));
        assert!(!transport.supports("qemu", crate::core::image::GuestOs::Windows));
        assert!(!transport.supports("hyperv", crate::core::image::GuestOs::Linux));
        let credentials = GuestCredentials::not_required();
        assert!(!format!("{credentials:?}").contains("credential-sentinel"));
    }

    #[test]
    fn qga_exec_is_typed_bounded_and_strict_utf8() {
        let stdout = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let stream = FakeQgaStream::scripted(&[
            json!({"return": {"pid": 42}}),
            json!({"return": {"exited": true, "exitcode": 7, "out-data": stdout}}),
        ]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let result = client
            .exec(&GuestCommand {
                program: "/usr/bin/printf".to_owned(),
                args: vec!["hello".to_owned()],
                timeout: Duration::from_secs(1),
                max_output_bytes: 64,
            })
            .unwrap();
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.stdout, "hello");

        let invalid = base64::engine::general_purpose::STANDARD.encode([0xff]);
        let stream = FakeQgaStream::scripted(&[
            json!({"return": {"pid": 43}}),
            json!({"return": {"exited": true, "exitcode": 0, "out-data": invalid}}),
        ]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            client.exec(&GuestCommand {
                program: "/bin/true".to_owned(),
                args: Vec::new(),
                timeout: Duration::from_secs(1),
                max_output_bytes: 64,
            }),
            Err(GuestIoError::InvalidResponse)
        );

        let stream = FakeQgaStream::scripted(&[
            json!({"return": {"pid": 44}}),
            json!({"return": {
                "exited": true,
                "exitcode": 0,
                "out-truncated": true
            }}),
        ]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            client.exec(&GuestCommand {
                program: "/bin/true".to_owned(),
                args: Vec::new(),
                timeout: Duration::from_secs(1),
                max_output_bytes: 64,
            }),
            Err(GuestIoError::InvalidResponse)
        );
    }

    #[test]
    fn qga_file_protocol_rejects_partial_and_oversized_results() {
        let stream = FakeQgaStream::scripted(&[json!({"return": {"count": 1}})]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            client.file_write_all(7, b"more than one byte"),
            Err(GuestIoError::PartialCopy)
        );

        let payload = base64::engine::general_purpose::STANDARD.encode(b"too-large");
        let stream = FakeQgaStream::scripted(&[json!({
            "return": {"buf-b64": payload, "count": 9, "eof": true}
        })]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(client.file_read_all(8, 2), Err(GuestIoError::OutputLimit));

        let payload = base64::engine::general_purpose::STANDARD.encode(b"short");
        let stream = FakeQgaStream::scripted(&[json!({
            "return": {"buf-b64": payload, "count": 4, "eof": true}
        })]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(client.file_read_all(8, 64), Err(GuestIoError::PartialCopy));

        let stream = FakeQgaStream::scripted(&[json!({"return": {"count": 1}})]);
        let mut expired = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() - Duration::from_millis(1),
        };
        assert_eq!(
            expired.file_write_all(9, b"deadline"),
            Err(GuestIoError::Timeout)
        );
    }

    #[test]
    fn qga_copy_in_deny_never_treats_no_clobber_collision_as_success() {
        let stream = FakeQgaStream::scripted(&[
            json!({"return": {"pid": 1}}),
            json!({"return": {"exited": true, "exitcode": 1}}),
            json!({"return": {"pid": 2}}),
            json!({"return": {"exited": true, "exitcode": 0}}),
        ]);
        let mut client = QgaClient {
            stream: Box::new(stream),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            client.commit_copy_in(
                "/run/vmcell/temp",
                "/run/vmcell/destination",
                OverwritePolicy::Deny,
                Duration::from_secs(1),
            ),
            Err(GuestIoError::PathViolation)
        );
    }

    #[test]
    fn qga_action_qmp_snapshot_binds_running_networkless_vm() {
        let expected = ProviderVm {
            id: Uuid::nil().to_string(),
            name: format!("vmcell-{}", Uuid::nil()),
            power_state: crate::providers::ProviderPowerState::Running,
            ownership_marker: "marker".to_owned(),
            configuration_path: std::path::PathBuf::from("runtime/qemu"),
            attached_disks: vec![std::path::PathBuf::from("runtime/cell.qcow2")],
            network_adapter_count: 0,
            cpu_count: 1,
            memory_mib: 1024,
        };
        let lines = [
            json!({"QMP": {}}),
            json!({"return": {}, "id": 1}),
            json!({"return": {"UUID": expected.id}, "id": 2}),
            json!({"return": {"name": expected.name}, "id": 3}),
            json!({"return": [{"cpu-index": 0}], "id": 4}),
            json!({"return": {"base-memory": 1073741824_u64}, "id": 5}),
            json!({"return": [], "id": 6}),
            json!({"return": [{"inserted": {
                "drv": "qcow2",
                "file": expected.attached_disks[0],
                "backing_file_depth": 1
            }}], "id": 7}),
            json!({"return": {"status": "running"}, "id": 8}),
        ];
        let stream = FakeQgaStream::scripted(&lines);
        let mut qmp = QmpClient::negotiate(stream, Duration::from_secs(1)).unwrap();
        assert!(prove_qmp_snapshot(&mut qmp, &expected).is_ok());

        let mut drifted = lines;
        drifted[6] = json!({"return": [{"name": "foreign-nic"}], "id": 6});
        let stream = FakeQgaStream::scripted(&drifted);
        let mut qmp = QmpClient::negotiate(stream, Duration::from_secs(1)).unwrap();
        assert_eq!(
            prove_qmp_snapshot(&mut qmp, &expected),
            Err(GuestIoError::OwnershipChanged)
        );
    }
}
