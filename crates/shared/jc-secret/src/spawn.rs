//! The one place a `jc` module starts a child process (git, a build, an App).
//!
//! The child gets an empty environment plus the variables in [`PASSED`] the parent has, so a
//! secret in `jc`'s environment (`JC_*`, `JCBIN_*`, `ANTARES_*`) never reaches it. A caller adds
//! what the child needs with [`std::process::Command::env`] on the returned command.

use std::ffi::{OsStr, OsString};
use std::process::Command;

/// The variables a child inherits: where programs are, the home directory (git reads its config
/// there), the locale, the time zone, the temp directory and the trust store.
pub const PASSED: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// A command for `program` with a cleared environment and [`PASSED`] carried over from this
/// process.
#[must_use]
#[allow(clippy::disallowed_methods)] // Reads the allow-listed names only, for the child.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    command_with(program, |name| std::env::var_os(name))
}

/// As [`command`], with `parent` standing for this process's environment.
#[must_use]
pub fn command_with(
    program: impl AsRef<OsStr>,
    parent: impl Fn(&str) -> Option<OsString>,
) -> Command {
    #[allow(clippy::disallowed_methods)] // This is the spawn helper.
    let mut command = Command::new(program);
    command.env_clear();
    for name in PASSED {
        if let Some(value) = parent(name) {
            command.env(name, value);
        }
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn child_env(mut command: Command) -> String {
        let out = command.output().expect("env runs");
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn a_child_sees_only_the_passed_variables() {
        // Cargo sets CARGO_MANIFEST_DIR in this test process's real environment: a child that
        // inherits the environment would print it.
        let shown = child_env(command("env"));
        assert!(!shown.contains("CARGO_MANIFEST_DIR="), "{shown}");
        assert!(shown.contains("PATH="), "{shown}");
        let names: Vec<&str> = shown
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .collect();
        for name in names {
            assert!(PASSED.contains(&name), "{name} leaked to the child");
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // The test stands in for the parent environment.
    fn a_secret_named_by_the_parent_is_not_passed() {
        let parent = |name: &str| match name {
            "PATH" => std::env::var_os("PATH"),
            "JCBIN_EKIBEN_TOKEN" => Some("leak-me".into()),
            _ => None,
        };
        let shown = child_env(command_with("env", parent));
        assert!(!shown.contains("leak-me"), "{shown}");
    }
}
