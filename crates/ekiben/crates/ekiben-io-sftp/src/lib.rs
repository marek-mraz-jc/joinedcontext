#![forbid(unsafe_code)]
//! `ekiben-io-sftp`: Bento's `sftp` input (`internal/impl/sftp/input.go`) on russh and
//! russh-sftp.
//!
//! The server goes through the organization's outbound rule (ekiben-io-http
//! `outbound::resolve_host_checked`) and the TCP connection is opened to the checked address.
//! Its host key must match `host_key_fingerprint` (ekiben, required: the `SHA256:...` line of
//! `ssh-keyscan host | ssh-keygen -lf -`); nothing is trusted on first use. Credentials are a
//! password or an OpenSSH private key (`credentials.private_key`, an ekiben field the author
//! interpolates from a secret); both stay `SecretString`. Host and user keys are Ed25519 or
//! ECDSA: the `rsa` crate (RUSTSEC-2023-0071) is not linked.
//!
//! `paths` are absolute; a glob is matched in the file name (`/upload/*.csv`). Each file is read
//! (at most `max_file_bytes`, default and ceiling 256 MiB), split by `scanner` (`to_the_end` by
//! default) and handed on as one batch with `sftp_path` set. `delete_on_finish` removes a file
//! once its batch is acked. Without `watcher` the input lists once and ends; with it, it lists
//! again every `poll_interval`, takes files older than `minimum_age` and records each taken
//! path in the `watcher.cache` resource (dropped again on a nack, so the file comes back).

mod input;

pub use input::{connect, SftpInput, SftpTarget};

use ekiben_core::Registry;

pub fn register_all(registry: &Registry) {
    registry.register_input("sftp", |conf, ctx| {
        Ok(Box::new(SftpInput::from_config(conf, ctx)?))
    });
    registry.register_lint("input", "sftp", input::lint);
}
