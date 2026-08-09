mod executor;

use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;

use crate::guest::{
    GuestActionAuthority, GuestCommand, GuestCommandResult, GuestCopyInAction, GuestCopyOutAction,
    GuestCredentials, GuestIoError, GuestReadiness, GuestTransport,
};
use crate::providers::ProviderVm;

pub use executor::PowerShellDirectExecutor;
pub(crate) use executor::{PowerShellDirectAction, PowerShellDirectCommandExecutor};

pub struct PowerShellDirectTransport<E = PowerShellDirectExecutor> {
    executor: E,
}

impl PowerShellDirectTransport<PowerShellDirectExecutor> {
    #[must_use]
    pub fn system() -> Self {
        Self::new(PowerShellDirectExecutor)
    }
}

impl<E> PowerShellDirectTransport<E> {
    #[must_use]
    pub(crate) fn new(executor: E) -> Self {
        Self { executor }
    }
}

fn execute_transport<E: PowerShellDirectCommandExecutor, T: serde::de::DeserializeOwned>(
    transport: &PowerShellDirectTransport<E>,
    action: PowerShellDirectAction,
    credentials: &GuestCredentials,
    payload: Option<&[u8]>,
    timeout: Duration,
) -> Result<T, GuestIoError> {
    let value = transport
        .executor
        .execute(&action, credentials, payload, timeout)?;
    serde_json::from_value(value).map_err(|_| GuestIoError::InvalidResponse)
}

impl<E: PowerShellDirectCommandExecutor> GuestTransport for PowerShellDirectTransport<E> {
    fn name(&self) -> &'static str {
        "powershell-direct"
    }

    fn supports(&self, provider: &str, guest_os: crate::core::image::GuestOs) -> bool {
        provider == "hyperv" && guest_os == crate::core::image::GuestOs::Windows
    }

    fn probe_ready(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        credentials: &GuestCredentials,
        timeout: Duration,
    ) -> Result<GuestReadiness, GuestIoError> {
        authority.validate(expected)?;
        let response: ReadinessResponse = execute_transport(
            self,
            PowerShellDirectAction::ProbeReady {
                cell_id: authority.cell_id(),
                expected: expected.clone(),
            },
            credentials,
            None,
            timeout,
        )?;
        Ok(response.status)
    }

    fn exec(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        credentials: &GuestCredentials,
        command: &GuestCommand,
    ) -> Result<GuestCommandResult, GuestIoError> {
        authority.validate(expected)?;
        command.validate()?;
        let command_line = windows_command_line(&command.args);
        if command.program.encode_utf16().count() > 32_766
            || command_line.encode_utf16().count() > 32_766
        {
            return Err(GuestIoError::InvalidRequest(
                "guest command exceeds the Windows process command-line limit",
            ));
        }
        let response: ExecResponse = execute_transport(
            self,
            PowerShellDirectAction::Exec {
                cell_id: authority.cell_id(),
                expected: expected.clone(),
                program: command.program.clone(),
                command_line,
                timeout_ms: duration_millis(command.timeout)?,
                max_output_bytes: command.max_output_bytes,
            },
            credentials,
            None,
            command.timeout + Duration::from_secs(5),
        )?;
        if response.timed_out {
            return Err(GuestIoError::Timeout);
        }
        Ok(GuestCommandResult {
            exit_code: response.exit_code,
            stdout: response.stdout,
            stderr: response.stderr,
            encoding: "utf-8".to_owned(),
            stdout_bytes: response.stdout_bytes,
            stderr_bytes: response.stderr_bytes,
            truncated: false,
        })
    }

    fn copy_in(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        credentials: &GuestCredentials,
        action: GuestCopyInAction<'_>,
    ) -> Result<(), GuestIoError> {
        authority.validate(expected)?;
        let response: EmptyResponse = execute_transport(
            self,
            PowerShellDirectAction::CopyIn {
                cell_id: authority.cell_id(),
                operation_id: action.operation_id,
                expected: expected.clone(),
                destination: action.destination.as_str().to_owned(),
                overwrite: action.overwrite,
            },
            credentials,
            Some(action.content),
            action.timeout,
        )?;
        if response.ok {
            Ok(())
        } else {
            Err(GuestIoError::InvalidResponse)
        }
    }

    fn copy_out(
        &self,
        authority: &GuestActionAuthority<'_>,
        expected: &ProviderVm,
        credentials: &GuestCredentials,
        action: GuestCopyOutAction<'_>,
    ) -> Result<Vec<u8>, GuestIoError> {
        authority.validate(expected)?;
        let response: CopyOutResponse = execute_transport(
            self,
            PowerShellDirectAction::CopyOut {
                cell_id: authority.cell_id(),
                operation_id: action.operation_id,
                expected: expected.clone(),
                source: action.source.as_str().to_owned(),
                max_bytes: action.max_bytes,
            },
            credentials,
            None,
            action.timeout,
        )?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(response.content_base64)
            .map_err(|_| GuestIoError::InvalidResponse)?;
        if bytes.len() as u64 != response.size || bytes.len() as u64 > action.max_bytes {
            return Err(GuestIoError::InvalidResponse);
        }
        Ok(bytes)
    }
}

fn duration_millis(duration: Duration) -> Result<u64, GuestIoError> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        GuestIoError::InvalidRequest("guest timeout cannot be represented in milliseconds")
    })
}

fn windows_command_line(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|value| value.is_whitespace() || value == '"')
    {
        return argument.to_owned();
    }
    let mut output = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            output.push_str(&"\\".repeat(backslashes * 2 + 1));
            output.push('"');
            backslashes = 0;
        } else {
            output.push_str(&"\\".repeat(backslashes));
            backslashes = 0;
            output.push(character);
        }
    }
    output.push_str(&"\\".repeat(backslashes * 2));
    output.push('"');
    output
}

#[derive(Deserialize)]
struct ReadinessResponse {
    status: GuestReadiness,
}

#[derive(Deserialize)]
struct ExecResponse {
    exit_code: i32,
    stdout: String,
    stderr: String,
    stdout_bytes: u64,
    stderr_bytes: u64,
    timed_out: bool,
}

#[derive(Deserialize)]
struct CopyOutResponse {
    content_base64: String,
    size: u64,
}

#[derive(Deserialize)]
struct EmptyResponse {
    ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_argument_quoting_preserves_spaces_quotes_and_backslashes() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument(""), "\"\"");
        assert_eq!(quote_windows_argument("two words"), "\"two words\"");
        assert_eq!(quote_windows_argument("a\"b"), "\"a\\\"b\"");
        assert_eq!(
            quote_windows_argument(r"C:\path with space\"),
            r#""C:\path with space\\""#
        );
    }
}
