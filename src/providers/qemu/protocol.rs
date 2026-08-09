use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::providers::ProviderError;

pub(crate) const PROTOCOL_MESSAGE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ControlEndpoint {
    Unix(PathBuf),
    WindowsPipe(String),
}

impl ControlEndpoint {
    pub(crate) fn qmp(configuration_path: &Path, id: &str) -> Self {
        #[cfg(target_os = "windows")]
        {
            let _ = configuration_path;
            Self::WindowsPipe(format!(r"\\.\pipe\vmcell-qmp-{id}"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = id;
            Self::Unix(configuration_path.join("qmp.sock"))
        }
    }

    pub(crate) fn qga(configuration_path: &Path, id: &str) -> Self {
        #[cfg(target_os = "windows")]
        {
            let _ = configuration_path;
            Self::WindowsPipe(format!(r"\\.\pipe\vmcell-qga-{id}"))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = id;
            Self::Unix(configuration_path.join("qga.sock"))
        }
    }

    pub(crate) fn qemu_argument(&self) -> String {
        match self {
            Self::Unix(path) => format!("unix:{},server=on,wait=off", path.display()),
            Self::WindowsPipe(path) => {
                let name = path.strip_prefix(r"\\.\pipe\").unwrap_or(path);
                format!("pipe:{name},server=on,wait=off")
            }
        }
    }
}

pub trait ReadWrite: Read + Write + Send {
    fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()>;
}

impl ReadWrite for Box<dyn ReadWrite> {
    fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
        (**self).set_operation_deadline(deadline)
    }
}

pub(crate) fn connect_endpoint(
    endpoint: &ControlEndpoint,
    timeout: Duration,
) -> Result<Box<dyn ReadWrite>, ProviderError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = remaining_duration(deadline).map_err(|error| {
            ProviderError::Command(format!("QEMU control endpoint unavailable: {error}"))
        })?;
        let result: std::io::Result<Box<dyn ReadWrite>> = match endpoint {
            ControlEndpoint::Unix(path) => connect_unix(path, remaining),
            ControlEndpoint::WindowsPipe(path) => connect_windows_pipe(path, remaining),
        };
        match result {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(ProviderError::Command(format!(
                    "QEMU control endpoint unavailable: {error}"
                )));
            }
        }
    }
}

#[cfg(unix)]
fn connect_unix(path: &Path, timeout: Duration) -> std::io::Result<Box<dyn ReadWrite>> {
    UnixControlStream::connect(path, timeout).map(|stream| Box::new(stream) as Box<dyn ReadWrite>)
}

#[cfg(not(unix))]
fn connect_unix(_path: &Path, _timeout: Duration) -> std::io::Result<Box<dyn ReadWrite>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix sockets are unavailable",
    ))
}

#[cfg(target_os = "windows")]
fn connect_windows_pipe(path: &str, timeout: Duration) -> std::io::Result<Box<dyn ReadWrite>> {
    WindowsPipeStream::connect(path, timeout).map(|stream| Box::new(stream) as Box<dyn ReadWrite>)
}

#[cfg(not(target_os = "windows"))]
fn connect_windows_pipe(_path: &str, _timeout: Duration) -> std::io::Result<Box<dyn ReadWrite>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Windows named pipes are unavailable",
    ))
}

#[cfg(target_os = "windows")]
struct WindowsPipeStream {
    handle: windows_sys::Win32::Foundation::HANDLE,
    timeout_ms: u32,
}

#[cfg(unix)]
struct UnixControlStream(std::os::unix::net::UnixStream);

#[cfg(unix)]
impl UnixControlStream {
    fn connect(path: &Path, timeout: Duration) -> std::io::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(path)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Self(stream))
    }
}

#[cfg(unix)]
impl Read for UnixControlStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(bytes)
    }
}

#[cfg(unix)]
impl Write for UnixControlStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(unix)]
impl ReadWrite for UnixControlStream {
    fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
        let remaining = remaining_duration(deadline)?;
        self.0.set_read_timeout(Some(remaining))?;
        self.0.set_write_timeout(Some(remaining))
    }
}

#[cfg(target_os = "windows")]
unsafe impl Send for WindowsPipeStream {}

#[cfg(target_os = "windows")]
impl WindowsPipeStream {
    fn connect(path: &str, timeout: Duration) -> std::io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        };
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

        let wide = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let timeout_ms = duration_millis(timeout);
        if unsafe { WaitNamedPipeW(wide.as_ptr(), timeout_ms) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { handle, timeout_ms })
    }

    fn transfer(&self, bytes: &mut [u8], write: bool) -> std::io::Result<usize> {
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GetLastError,
        };
        use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
        use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResultEx, OVERLAPPED};
        use windows_sys::Win32::System::Threading::CreateEventW;

        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut overlapped = Box::new(OVERLAPPED {
            hEvent: event,
            ..OVERLAPPED::default()
        });
        let mut transferred = 0_u32;
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let started = unsafe {
            if write {
                WriteFile(
                    self.handle,
                    bytes.as_ptr(),
                    length,
                    &mut transferred,
                    overlapped.as_mut(),
                )
            } else {
                ReadFile(
                    self.handle,
                    bytes.as_mut_ptr(),
                    length,
                    &mut transferred,
                    overlapped.as_mut(),
                )
            }
        };
        let result = if started != 0 {
            Ok(transferred as usize)
        } else if unsafe { GetLastError() } != ERROR_IO_PENDING {
            Err(std::io::Error::last_os_error())
        } else if unsafe {
            GetOverlappedResultEx(
                self.handle,
                overlapped.as_ref(),
                &mut transferred,
                self.timeout_ms,
                0,
            )
        } != 0
        {
            Ok(transferred as usize)
        } else {
            let operation_error = std::io::Error::last_os_error();
            unsafe {
                CancelIoEx(self.handle, overlapped.as_ref());
            }
            let cancellation_result = unsafe {
                GetOverlappedResultEx(self.handle, overlapped.as_ref(), &mut transferred, 100, 0)
            };
            let cancellation_settled =
                cancellation_result != 0 || unsafe { GetLastError() } == ERROR_OPERATION_ABORTED;
            if !cancellation_settled {
                // The kernel may still reference OVERLAPPED after a failed
                // cancellation. Leak this tiny timeout-only allocation/event
                // instead of blocking or returning a dangling stack pointer;
                // dropping the stream closes the pipe handle.
                let _ = Box::into_raw(overlapped);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "QEMU named-pipe cancellation did not settle",
                ));
            }
            Err(operation_error)
        };
        unsafe { CloseHandle(event) };
        result
    }
}

#[cfg(target_os = "windows")]
impl Read for WindowsPipeStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.transfer(bytes, false)
    }
}

#[cfg(target_os = "windows")]
impl Write for WindowsPipeStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut owned = bytes.to_vec();
        self.transfer(&mut owned, true)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl ReadWrite for WindowsPipeStream {
    fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
        self.timeout_ms = duration_millis(remaining_duration(deadline)?);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsPipeStream {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[cfg(target_os = "windows")]
fn duration_millis(timeout: Duration) -> u32 {
    timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32
}

fn remaining_duration(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "QEMU protocol operation exceeded its deadline",
            )
        })
}

pub(crate) struct JsonLineProtocol<S> {
    stream: S,
}

impl<S: ReadWrite> JsonLineProtocol<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self { stream }
    }

    pub(crate) fn send(&mut self, value: &Value, deadline: Instant) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
        if bytes.len() > PROTOCOL_MESSAGE_LIMIT {
            return Err(ProviderError::InvalidResponse(
                "QEMU protocol request exceeded the message limit".to_owned(),
            ));
        }
        bytes.push(b'\n');
        self.stream
            .set_operation_deadline(deadline)
            .and_then(|_| self.stream.write_all(&bytes))
            .and_then(|_| self.stream.flush())
            .map_err(|error| ProviderError::Command(format!("QEMU protocol write failed: {error}")))
    }

    pub(crate) fn receive(&mut self, deadline: Instant) -> Result<Value, ProviderError> {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            self.stream
                .set_operation_deadline(deadline)
                .map_err(|error| {
                    ProviderError::Command(format!("QEMU protocol read failed: {error}"))
                })?;
            let read = self.stream.read(&mut byte).map_err(|error| {
                ProviderError::Command(format!("QEMU protocol read failed: {error}"))
            })?;
            if read == 0 {
                return Err(ProviderError::InvalidResponse(
                    "QEMU protocol closed before a complete response".to_owned(),
                ));
            }
            if byte[0] == b'\n' {
                if bytes.is_empty() {
                    continue;
                }
                return serde_json::from_slice(&bytes)
                    .map_err(|error| ProviderError::InvalidResponse(error.to_string()));
            }
            if bytes.len() >= PROTOCOL_MESSAGE_LIMIT {
                return Err(ProviderError::InvalidResponse(
                    "QEMU protocol response exceeded the message limit".to_owned(),
                ));
            }
            bytes.push(byte[0]);
        }
    }
}

pub(crate) struct QmpClient<S> {
    protocol: JsonLineProtocol<S>,
    next_id: u64,
    deadline: Instant,
}

impl<S: ReadWrite> QmpClient<S> {
    pub(crate) fn negotiate(stream: S, timeout: Duration) -> Result<Self, ProviderError> {
        let mut client = Self {
            protocol: JsonLineProtocol::new(stream),
            next_id: 1,
            deadline: Instant::now() + timeout,
        };
        let greeting = client.protocol.receive(client.deadline)?;
        if greeting.get("QMP").is_none() {
            return Err(ProviderError::InvalidResponse(
                "QMP greeting was missing".to_owned(),
            ));
        }
        client.execute("qmp_capabilities", None)?;
        Ok(client)
    }

    pub(crate) fn execute(
        &mut self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<Value, ProviderError> {
        if Instant::now() >= self.deadline {
            return Err(ProviderError::Command(
                "QMP operation exceeded its deadline".to_owned(),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"execute": command, "id": id});
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        self.protocol.send(&request, self.deadline)?;
        let mut event_count = 0_u16;
        loop {
            if Instant::now() >= self.deadline {
                return Err(ProviderError::Command(
                    "QMP operation exceeded its deadline".to_owned(),
                ));
            }
            let response = self.protocol.receive(self.deadline)?;
            if response.get("event").is_some() {
                event_count += 1;
                if event_count > 64 {
                    return Err(ProviderError::InvalidResponse(
                        "QMP emitted too many events before the response".to_owned(),
                    ));
                }
                continue;
            }
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(ProviderError::InvalidResponse(
                    "QMP response id did not match the request".to_owned(),
                ));
            }
            if let Some(error) = response.get("error") {
                let _ = error;
                return Err(ProviderError::Command(format!(
                    "QMP command {command} failed"
                )));
            }
            return response.get("return").cloned().ok_or_else(|| {
                ProviderError::InvalidResponse("QMP response omitted return".to_owned())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct ScriptedStream {
        reads: VecDeque<u8>,
        writes: Vec<u8>,
    }

    impl Read for ScriptedStream {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            let count = target.len().min(self.reads.len());
            for slot in target.iter_mut().take(count) {
                *slot = self.reads.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl Write for ScriptedStream {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl ReadWrite for ScriptedStream {
        fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
            remaining_duration(deadline).map(|_| ())
        }
    }

    struct DripStream {
        reads: VecDeque<u8>,
        delay: Duration,
    }

    impl Read for DripStream {
        fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.delay);
            let Some(byte) = self.reads.pop_front() else {
                return Ok(0);
            };
            target[0] = byte;
            Ok(1)
        }
    }

    impl Write for DripStream {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl ReadWrite for DripStream {
        fn set_operation_deadline(&mut self, deadline: Instant) -> std::io::Result<()> {
            remaining_duration(deadline).map(|_| ())
        }
    }

    #[test]
    fn qmp_negotiation_correlates_ids_and_ignores_events() {
        let input = concat!(
            "{\"QMP\":{\"version\":{}}}\n",
            "{\"event\":\"RESET\"}\n",
            "{\"return\":{},\"id\":1}\n",
            "{\"return\":{\"status\":\"running\"},\"id\":2}\n"
        );
        let stream = ScriptedStream {
            reads: input.bytes().collect(),
            writes: Vec::new(),
        };
        let mut client = QmpClient::negotiate(stream, Duration::from_secs(1)).unwrap();
        assert_eq!(
            client.execute("query-status", None).unwrap()["status"],
            "running"
        );
    }

    #[test]
    fn protocol_rejects_oversized_response() {
        let stream = ScriptedStream {
            reads: std::iter::repeat_n(b'x', PROTOCOL_MESSAGE_LIMIT + 1).collect(),
            writes: Vec::new(),
        };
        assert!(matches!(
            // This fixture intentionally consumes a full 1 MiB byte-by-byte;
            // leave enough wall-clock budget for a saturated debug test run so
            // the assertion exercises the size limit rather than the deadline.
            JsonLineProtocol::new(stream).receive(Instant::now() + Duration::from_secs(30)),
            Err(ProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn qmp_rejects_wrong_response_id_and_truncated_json() {
        let wrong_id = ScriptedStream {
            reads: concat!("{\"QMP\":{}}\n", "{\"return\":{},\"id\":9}\n")
                .bytes()
                .collect(),
            writes: Vec::new(),
        };
        assert!(matches!(
            QmpClient::negotiate(wrong_id, Duration::from_secs(1)),
            Err(ProviderError::InvalidResponse(_))
        ));
        let truncated = ScriptedStream {
            reads: b"{\"QMP\":".iter().copied().collect(),
            writes: Vec::new(),
        };
        assert!(matches!(
            QmpClient::negotiate(truncated, Duration::from_secs(1)),
            Err(ProviderError::InvalidResponse(_))
        ));
    }

    #[test]
    fn qmp_event_flood_and_expired_deadline_fail_closed() {
        let mut input = concat!("{\"QMP\":{}}\n", "{\"return\":{},\"id\":1}\n").to_owned();
        for _ in 0..65 {
            input.push_str("{\"event\":\"STOP\"}\n");
        }
        input.push_str("{\"return\":{},\"id\":2}\n");
        let stream = ScriptedStream {
            reads: input.bytes().collect(),
            writes: Vec::new(),
        };
        let mut client = QmpClient::negotiate(stream, Duration::from_secs(1)).unwrap();
        assert!(matches!(
            client.execute("query-status", None),
            Err(ProviderError::InvalidResponse(_))
        ));

        let expired = ScriptedStream {
            reads: concat!("{\"QMP\":{}}\n", "{\"return\":{},\"id\":1}\n")
                .bytes()
                .collect(),
            writes: Vec::new(),
        };
        assert!(matches!(
            QmpClient::negotiate(expired, Duration::ZERO),
            Err(ProviderError::Command(_))
        ));

        let drip = DripStream {
            reads: concat!("{\"QMP\":{}}\n", "{\"return\":{},\"id\":1}\n")
                .bytes()
                .collect(),
            delay: Duration::from_millis(2),
        };
        assert!(matches!(
            QmpClient::negotiate(drip, Duration::from_millis(5)),
            Err(ProviderError::Command(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_qmp_socket_round_trip_is_deadline_bound() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("qmp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"{\"QMP\":{}}\n").unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            stream.write_all(b"{\"return\":{},\"id\":1}\n").unwrap();
            request.clear();
            reader.read_line(&mut request).unwrap();
            stream
                .write_all(b"{\"return\":{\"status\":\"running\"},\"id\":2}\n")
                .unwrap();
        });
        let stream =
            connect_endpoint(&ControlEndpoint::Unix(path), Duration::from_secs(1)).unwrap();
        let mut client = QmpClient::negotiate(stream, Duration::from_secs(1)).unwrap();
        assert_eq!(
            client.execute("query-status", None).unwrap()["status"],
            "running"
        );
        server.join().unwrap();
    }
}
