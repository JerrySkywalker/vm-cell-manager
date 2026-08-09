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

pub trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

pub(crate) fn connect_endpoint(
    endpoint: &ControlEndpoint,
    timeout: Duration,
) -> Result<Box<dyn ReadWrite>, ProviderError> {
    let deadline = Instant::now() + timeout;
    loop {
        let result: std::io::Result<Box<dyn ReadWrite>> = match endpoint {
            ControlEndpoint::Unix(path) => connect_unix(path),
            ControlEndpoint::WindowsPipe(path) => connect_windows_pipe(path),
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
fn connect_unix(path: &Path) -> std::io::Result<Box<dyn ReadWrite>> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(Box::new(stream))
}

#[cfg(not(unix))]
fn connect_unix(_path: &Path) -> std::io::Result<Box<dyn ReadWrite>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix sockets are unavailable",
    ))
}

#[cfg(target_os = "windows")]
fn connect_windows_pipe(path: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    Ok(Box::new(file))
}

#[cfg(not(target_os = "windows"))]
fn connect_windows_pipe(_path: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Windows named pipes are unavailable",
    ))
}

pub(crate) struct JsonLineProtocol<S> {
    stream: S,
}

impl<S: Read + Write> JsonLineProtocol<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self { stream }
    }

    pub(crate) fn send(&mut self, value: &Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
        if bytes.len() > PROTOCOL_MESSAGE_LIMIT {
            return Err(ProviderError::InvalidResponse(
                "QEMU protocol request exceeded the message limit".to_owned(),
            ));
        }
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .and_then(|_| self.stream.flush())
            .map_err(|error| ProviderError::Command(format!("QEMU protocol write failed: {error}")))
    }

    pub(crate) fn receive(&mut self) -> Result<Value, ProviderError> {
        let mut bytes = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
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
}

impl<S: Read + Write> QmpClient<S> {
    pub(crate) fn negotiate(stream: S) -> Result<Self, ProviderError> {
        let mut client = Self {
            protocol: JsonLineProtocol::new(stream),
            next_id: 1,
        };
        let greeting = client.protocol.receive()?;
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
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"execute": command, "id": id});
        if let Some(arguments) = arguments {
            request["arguments"] = arguments;
        }
        self.protocol.send(&request)?;
        loop {
            let response = self.protocol.receive()?;
            if response.get("event").is_some() {
                continue;
            }
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                return Err(ProviderError::InvalidResponse(
                    "QMP response id did not match the request".to_owned(),
                ));
            }
            if let Some(error) = response.get("error") {
                return Err(ProviderError::Command(format!(
                    "QMP command {command} failed: {}",
                    error
                        .get("desc")
                        .and_then(Value::as_str)
                        .unwrap_or("redacted QMP error")
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
        let mut client = QmpClient::negotiate(stream).unwrap();
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
            JsonLineProtocol::new(stream).receive(),
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
            QmpClient::negotiate(wrong_id),
            Err(ProviderError::InvalidResponse(_))
        ));
        let truncated = ScriptedStream {
            reads: b"{\"QMP\":".iter().copied().collect(),
            writes: Vec::new(),
        };
        assert!(matches!(
            QmpClient::negotiate(truncated),
            Err(ProviderError::InvalidResponse(_))
        ));
    }
}
