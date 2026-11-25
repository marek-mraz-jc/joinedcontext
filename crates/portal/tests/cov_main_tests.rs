//! The binary's entry point (main.rs) run as a process: each way it ends, and the clean stop of
//! a server that was asked to. The child keeps `LLVM_PROFILE_FILE`, so a coverage run counts it.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn binary(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_joinedcontext-portal"));
    command.args(args).env_clear();
    if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_blank_database_url_for_migrate_is_as_good_as_none() {
    let out = binary(&["migrate"])
        .env("JC_PORTAL_DATABASE_URL", "   ")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("JC_PORTAL_DATABASE_URL is not set"));
}

#[test]
fn migrate_without_a_database_url_names_the_variable() {
    let out = binary(&["migrate"]).output().expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("name the database to migrate"));
}

#[test]
fn migrate_names_an_installation_organization_the_hosts_do_not_hold() {
    let out = binary(&["migrate"])
        .env(
            "JC_PORTAL_DATABASE_URL",
            "postgres://nobody@127.0.0.1:1/none",
        )
        .env("JC_PORTAL_INSTALLATION_ORGANIZATION", "ghost")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let printed = stderr_of(&out);
    assert!(printed.contains("migrate:"), "{printed}");
    assert!(printed.contains("ghost"), "{printed}");
}

#[test]
fn migrate_against_a_database_that_is_down_fails_with_a_blank_migration_url() {
    let out = binary(&["migrate"])
        .env(
            "JC_PORTAL_DATABASE_URL",
            "postgres://nobody:pw-of-nobody@127.0.0.1:1/none",
        )
        .env("JC_PORTAL_MIGRATION_DATABASE_URL", "")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let printed = stderr_of(&out);
    assert!(printed.contains("migrate:"), "{printed}");
    assert!(!printed.contains("pw-of-nobody"), "{printed}");
}

#[test]
fn migrate_brings_a_database_to_this_releases_schema_and_exits_zero() {
    let Ok(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL") else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let out = binary(&["migrate"])
        .env("JC_PORTAL_DATABASE_URL", url)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
}

#[test]
fn validate_without_its_arguments_is_a_usage_error() {
    let out = binary(&["validate"]).output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).contains("usage: joinedcontext-portal validate"));
}

#[test]
fn a_bind_address_that_is_not_one_is_a_configuration_error() {
    let out = binary(&[])
        .env("JC_PORTAL_BIND", "not-an-address")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let printed = stderr_of(&out);
    assert!(printed.contains("Configuration error"), "{printed}");
    assert!(printed.contains("JC_PORTAL_BIND"), "{printed}");
}

#[test]
fn an_address_already_in_use_is_a_server_error() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let out = binary(&[])
        .env(
            "JC_PORTAL_BIND",
            taken.local_addr().expect("address").to_string(),
        )
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("Server error"));
}

#[test]
fn a_running_server_stops_cleanly_on_sigterm() {
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        probe.local_addr().expect("address").port()
    };
    let mut child = binary(&[])
        .env("JC_PORTAL_BIND", format!("127.0.0.1:{port}"))
        .env("JC_INTERNAL_BIND", "127.0.0.1:0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client");
    // An answer to a request means the server runs and listens for the signal.
    let started = Instant::now();
    let mut answered = false;
    while started.elapsed() < Duration::from_secs(60) {
        if client
            .get(format!("http://127.0.0.1:{port}/api/v1/ready"))
            .send()
            .is_ok()
        {
            answered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(client);
    if !answered {
        child.kill().expect("kill");
    }
    assert!(answered, "the server answered within a minute");
    std::thread::sleep(Duration::from_millis(500));
    let sent = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("kill runs");
    assert!(sent.success());
    let status = child.wait().expect("exit");
    assert_eq!(status.code(), Some(0));
}
