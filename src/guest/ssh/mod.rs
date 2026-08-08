use crate::guest::GuestTransport;

pub struct SshTransport;

impl GuestTransport for SshTransport {
    fn name(&self) -> &'static str {
        "ssh"
    }
}
