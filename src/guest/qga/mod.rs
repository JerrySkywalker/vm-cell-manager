use crate::guest::GuestTransport;

pub struct QemuGuestAgentTransport;

impl GuestTransport for QemuGuestAgentTransport {
    fn name(&self) -> &'static str {
        "qga"
    }
}
