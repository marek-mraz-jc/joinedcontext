//! `joinedcontext-portal migrate` (T-2802, OPS-12): the upgrade drill runs the next release's
//! migrations against a copy of dev's database with nothing but the database URL.

use std::process::Command;

fn migrate(url: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_joinedcontext-portal"));
    command.arg("migrate").env_clear();
    if let Some(url) = url {
        command.env("JC_PORTAL_DATABASE_URL", url);
    }
    command.output().expect("run the binary")
}

#[test]
fn without_a_database_it_says_which_variable_and_fails() {
    let out = migrate(None);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("JC_PORTAL_DATABASE_URL is not set"));
}

#[test]
fn a_database_it_cannot_reach_fails_without_printing_the_password() {
    let out = migrate(Some(
        "postgres://portal:s3cret-of-the-drill@127.0.0.1:1/portal",
    ));
    assert_eq!(out.status.code(), Some(1));
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains("s3cret-of-the-drill"), "{printed}");
    assert!(printed.contains("migrate:"), "{printed}");
}

/// Twice against a real database: the second run finds nothing to do (T-2802's upgrade drill).
#[test]
fn a_database_is_migrated_and_a_second_run_is_a_no_op() {
    let Ok(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL") else {
        return;
    };
    for _ in 0..2 {
        let out = migrate(Some(&url));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
