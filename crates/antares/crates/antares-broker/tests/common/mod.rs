// SPDX-License-Identifier: EUPL-1.2
//! Helpers shared by the broker's end-to-end tests.

/// A TCP port no other test process of the same run will hand out.
///
/// A kernel-chosen ephemeral port is only free at the moment it is drawn,
/// and a closed listening socket never enters TIME_WAIT, so the next draw
/// can repeat it. Every test runs in its own process, so a record of what
/// has already been handed out cannot live in memory: two brokers then get
/// the same port, race for it, the loser exits, and the survivor answers
/// for the wrong role — an API pod returning 201 Created where a worker
/// pod's 404 was asserted. The reservation is therefore a file created
/// exclusively under the directory every test process of one run shares.
/// A reservation older than an hour is left over from a run that was
/// killed before it could clean up, and is taken over.
pub fn free_port() -> u16 {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ports");
    std::fs::create_dir_all(&dir).expect("port reservation directory");
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let claim = dir.join(port.to_string());
        let stale = std::fs::metadata(&claim)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > std::time::Duration::from_secs(3600))
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&claim);
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&claim)
        {
            Ok(_) => return port,
            // Someone else holds this port: draw another one.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // Anything else is the directory itself refusing writes, and
            // retrying cannot fix it — say so instead of spinning forever.
            Err(e) => panic!("port reservation {}: {e}", claim.display()),
        }
    }
}

/// Stops a spawned broker the way an orchestrator does: SIGTERM, the drain,
/// a clean exit; a SIGKILL only when it is still running after `GRACE`.
///
/// A killed child never reaches its exit, so its coverage profile is never
/// written and every line only a spawned broker runs (the Postgres boot, the
/// NATS wiring, the drain itself) read as untested (T-7829). A broker that
/// exited already (a refused boot, a test's own SIGKILL) is only reaped.
pub fn stop(child: &mut std::process::Child) {
    /// The drain's notice window (2 s) plus its deadline (20 s), and slack.
    const GRACE: std::time::Duration = std::time::Duration::from_secs(25);
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let term = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    if term.is_ok_and(|s| s.success()) {
        let deadline = std::time::Instant::now() + GRACE;
        while std::time::Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
