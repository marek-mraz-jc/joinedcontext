//! Scheduled inputs that collect files from a remote mailbox or file server (ekiben extensions,
//! T-7087): `imap` (attachments of the mails in a mailbox, IMAPS only) and `ftp` (files under a
//! path glob, explicit FTPS unless the organization's `allow_plain_ftp` quota is on). Both read
//! what is there when a run starts, end the run, and change the server (seen, moved, deleted or
//! renamed) only for what the pipeline acknowledged.
//!
//! Every connection goes through the organization's outbound rule
//! (`ekiben_io_http::outbound::resolve_host_checked`) and to the address it checked, never a
//! second lookup; an FTP data connection only to the control connection's own peer.

mod acks;
mod ftp;
mod imap;
mod tls;

use ekiben_core::Registry;

pub use ftp::FtpInput;
pub use imap::ImapInput;

/// Registers the `ftp` and `imap` inputs.
pub fn register_all(registry: &Registry) {
    registry.register_input("ftp", |conf, ctx| {
        Ok(Box::new(FtpInput::from_config(conf, ctx)?))
    });
    registry.register_input("imap", |conf, ctx| {
        Ok(Box::new(ImapInput::from_config(conf, ctx)?))
    });
}
