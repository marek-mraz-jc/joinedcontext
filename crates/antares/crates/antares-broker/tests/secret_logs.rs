// SPDX-License-Identifier: EUPL-1.2
//! T-3915 against the real binary: neither the `/q` admin token nor the
//! database password appears in any line the broker prints, at the most
//! verbose log level, through a refused admin call, an accepted one and a
//! database that refuses the connection.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;
use common::free_port;

const TOKEN: &str = "t3915-admin-token-0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "t3915-db-password-hunter2";

fn get(port: u16, path: &str, bearer: &str) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {bearer}\r\nConnection: close\r\n\r\n"
    );
    let _ = s.write_all(req.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// Everything the child printed on stdout and stderr.
fn output(mut child: std::process::Child) -> String {
    let _ = child.kill();
    let out = child.wait_with_output().expect("wait");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_admin_token_is_never_printed() {
    let dir = std::env::temp_dir().join(format!("antares-secret-logs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("token");
    std::fs::write(&file, format!("{TOKEN}\n")).expect("token file");
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_antares"))
        .env("ANTARES_HTTP_PORT", port.to_string())
        .env("ANTARES_ADMIN_TOKEN_FILE", &file)
        .env("RUST_LOG", "trace")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn antares");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !get(port, "/q/health", "").starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    // a near miss, the token itself, and the token where the scheme belongs
    let wrong = format!("{TOKEN}x");
    assert!(get(port, "/q/tenants", &wrong).starts_with("HTTP/1.1 401"));
    assert!(get(port, "/q/tenants", TOKEN).starts_with("HTTP/1.1 200"));
    assert!(get(port, &format!("/q/tenants?tenant={TOKEN}"), "").starts_with("HTTP/1.1 401"));
    let log = output(child);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !log.is_empty(),
        "the broker printed nothing at RUST_LOG=trace"
    );
    for line in log.lines() {
        assert!(
            !line.contains(TOKEN),
            "the admin token in a log line: {line}"
        );
    }
}

/// The database refuses the password: the broker retries for its 30 s and
/// exits, and its error names the failure, never the URL's password. Needs
/// the live test database (a closed port makes every attempt wait out the
/// pool's acquire timeout, minutes in all); skips loudly without one.
#[test]
fn the_database_password_is_never_printed() {
    let Some(host) = std::env::var("ANTARES_TEST_DATABASE_URL")
        .ok()
        .and_then(|u| u.split_once('@').map(|(_, host)| host.to_owned()))
    else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let url = format!("postgresql://antares:{PASSWORD}@{host}");
    let child = Command::new(env!("CARGO_BIN_EXE_antares"))
        .env("ANTARES_HTTP_PORT", free_port().to_string())
        .env("ANTARES_STORE", "postgres")
        .env("ANTARES_ALLOW_SHARED_LOCAL", "1")
        .env("ANTARES_DATABASE_URL", &url)
        .env("RUST_LOG", "trace")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn antares");
    let out = child.wait_with_output().expect("wait");
    assert!(
        !out.status.success(),
        "a refused database is a failed start"
    );
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(log.contains("database not reachable"), "{log}");
    for line in log.lines() {
        assert!(
            !line.contains(PASSWORD),
            "the database password in a log line: {line}"
        );
    }
}
