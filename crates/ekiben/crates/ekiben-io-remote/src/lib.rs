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

/// The config readers and the mail parser, for fuzz/ (cargo fuzz): any text a config holds and any
/// bytes a mail server sends are read or refused, never a panic; an attachment keeps a bare name.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    /// The address, path, sender and mailbox readers of both inputs over one text.
    pub fn names(text: &str) {
        crate::ftp::fuzz_names(text);
        crate::imap::fuzz_names(text);
    }

    /// The attachments one raw mail queues.
    pub fn mail(raw: &[u8]) -> usize {
        crate::imap::fuzz_mail(raw)
    }
}
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
