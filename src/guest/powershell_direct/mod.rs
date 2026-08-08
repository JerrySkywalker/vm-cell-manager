use crate::guest::GuestTransport;

pub struct PowerShellDirectTransport;

impl GuestTransport for PowerShellDirectTransport {
    fn name(&self) -> &'static str {
        "powershell-direct"
    }
}
