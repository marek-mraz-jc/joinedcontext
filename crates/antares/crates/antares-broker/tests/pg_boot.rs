// SPDX-License-Identifier: EUPL-1.2
//! The Postgres boot of the real binary in the split-role shape a production
//! deployment runs (T-7310, T-7307, T-5838): the owner migrates, the request
//! role serves and owns nothing, the maintenance role holds the cross-Tenant
//! escape, credentials are sealed. Each refusal the boot owes an operator is
//! asked for once: a request role owning the tables, a maintenance role
//! outside `antares_maintenance` or bypassing RLS under ANTARES_REQUIRE_RLS, a
//! database another release migrated, a key that is not 32 bytes.
//!
//! Every test runs in a database of its own (roles are server-wide, grants
//! and `_sqlx_migrations` are not), so nothing here touches the shared one.
//! Skips loudly without ANTARES_TEST_DATABASE_URL (same rule as
//! store_combos.rs).
#![cfg(unix)]

use antares_sql::sqlx;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod common;
use common::free_port;

/// The roles' password: a fixture of the per-test server, like antares:antares.
const PASSWORD: &str = "boot";
/// A request role: the rows' grants, no table, bound by row-level security.
const REQUEST: &str = "antares_boot_request";
/// A maintenance role: a member of `antares_maintenance`, bound by RLS.
const MAINTENANCE: &str = "antares_boot_maintenance";
/// A role that may log in and read, outside `antares_maintenance`.
const PLAIN: &str = "antares_boot_plain";
/// A request role allowed two connections: the boot probes take both, a
/// second pool cannot open.
const LIMITED: &str = "antares_boot_limited";
/// A request role allowed one connection: too few for the boot's probes.
const SINGLE: &str = "antares_boot_single";

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// A database of this test's own, migrated by this release, with the three
/// roles granted on its tables.
struct Db {
    /// The superuser URL of the own database (the tables' owner).
    owner: String,
    /// The own database's URL with `{role}` where the user goes.
    base: String,
    name: String,
    admin: String,
}

impl Db {
    fn new(tag: &str) -> Option<Self> {
        let Ok(admin) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
            eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
            return None;
        };
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        let name = format!("antares_boot_{tag}_{run}");
        let (server, _) = admin.rsplit_once('/').expect("a database URL");
        let owner = format!("{server}/{name}");
        rt().block_on(async {
            let pool = sqlx::PgPool::connect(&admin).await.expect("admin pool");
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
                .execute(&pool)
                .await
                .expect("own database");
            pool.close().await;
            let own = antares_sql::store::pg::connect(&owner, 2)
                .await
                .expect("connect+migrate the own database");
            let mut tx = own.begin().await.expect("begin");
            sqlx::query("SET LOCAL lock_timeout = '60s'")
                .execute(&mut *tx)
                .await
                .expect("lock timeout");
            // The role setup lock every pg test takes (tests/pg.rs app_role):
            // CREATE ROLE races another process's on the same server.
            sqlx::query("SELECT pg_advisory_xact_lock(4334)")
                .execute(&mut *tx)
                .await
                .expect("role setup lock");
            let mut stmts = vec!["DO $$ BEGIN CREATE ROLE antares_maintenance NOLOGIN;
                 EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
                .to_owned()];
            for role in [REQUEST, MAINTENANCE, PLAIN, LIMITED, SINGLE] {
                stmts.push(format!(
                    "DO $$ BEGIN CREATE ROLE {role} LOGIN PASSWORD '{PASSWORD}';
                     EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
                ));
                stmts.push(format!("GRANT USAGE ON SCHEMA public TO {role}"));
                stmts.push(format!(
                    "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {role}"
                ));
                stmts.push(format!(
                    "GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {role}"
                ));
            }
            stmts.push(format!("GRANT antares_maintenance TO {MAINTENANCE}"));
            stmts.push(format!("ALTER ROLE {LIMITED} CONNECTION LIMIT 2"));
            stmts.push(format!("ALTER ROLE {SINGLE} CONNECTION LIMIT 1"));
            for stmt in stmts {
                sqlx::query(sqlx::AssertSqlSafe(stmt.clone()))
                    .execute(&mut *tx)
                    .await
                    .expect(&stmt);
            }
            tx.commit().await.expect("commit");
            own.close().await;
        });
        let base = server
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('@'))
            .map(|(_, host)| format!("postgresql://{{role}}:{PASSWORD}@{host}/{name}"))
            .expect("a URL with credentials");
        Some(Self {
            owner,
            base,
            name,
            admin,
        })
    }

    /// The own database as `role`.
    fn role(&self, role: &str) -> String {
        self.base.replace("{role}", role)
    }

    /// Runs `sql` in the own database as its owner.
    fn exec(&self, sql: &str) {
        rt().block_on(async {
            let pool = sqlx::PgPool::connect(&self.owner)
                .await
                .expect("owner pool");
            sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
                .execute(&pool)
                .await
                .expect(sql);
            pool.close().await;
        });
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let (admin, name) = (self.admin.clone(), self.name.clone());
        rt().block_on(async move {
            if let Ok(pool) = sqlx::PgPool::connect(&admin).await {
                let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                )))
                .execute(&pool)
                .await;
                pool.close().await;
            }
        });
    }
}

/// A broker on the own database: `ANTARES_STORE=postgres` with `envs` on top.
fn broker(port: u16, envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_antares"));
    cmd.env("ANTARES_HTTP_PORT", port.to_string())
        .env("ANTARES_STORE", "postgres")
        .env("ANTARES_ALLOW_SHARED_LOCAL", "1")
        .env("ANTARES_DRAIN_DELAY_MS", "0")
        .envs(envs.iter().copied());
    cmd
}

fn get(port: u16, path: &str) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let _ = s.write_all(req.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// The boot `envs` describe fails: its exit is not a success and comes before
/// the 30 s connect retry would end. What it printed, both streams.
fn refused(envs: &[(&str, &str)]) -> String {
    let mut child = broker(free_port(), envs)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn antares");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(25) {
            common::stop(&mut child);
            panic!("the boot did not stop on its own within 25 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut log = String::new();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read>),
    ]
    .into_iter()
    .flatten()
    {
        let mut pipe = pipe;
        let _ = pipe.read_to_string(&mut log);
    }
    assert!(!status.success(), "the boot was meant to fail: {log}");
    assert!(
        !log.contains(&format!(":{PASSWORD}@")),
        "a database URL's password in the output: {log}"
    );
    log
}

/// The production shape boots, serves, reports its pools and stops clean on
/// SIGTERM: owner migrates, request role serves under ANTARES_REQUIRE_RLS, the
/// maintenance role holds the escape, a write reserve and the sealing key on.
#[test]
fn the_split_role_boot_serves_and_stops_clean() {
    let Some(db) = Db::new("split") else { return };
    let request = db.role(REQUEST);
    let maintenance = db.role(MAINTENANCE);
    let port = free_port();
    let mut child = broker(
        port,
        &[
            ("ANTARES_MIGRATION_DATABASE_URL", &db.owner),
            ("ANTARES_DATABASE_URL", &request),
            ("ANTARES_MAINTENANCE_DATABASE_URL", &maintenance),
            ("ANTARES_REQUIRE_RLS", "1"),
            ("ANTARES_PG_POOL", "4"),
            ("ANTARES_PG_WRITE_RESERVE", "1"),
            ("ANTARES_PG_BATCH_ASYNC_COMMIT", "1"),
            (
                "ANTARES_SECRET_KEY",
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            ),
            ("ANTARES_TEMPORAL_RETENTION_DAYS", "30"),
            // every background job ticks twice before the stop, at debug
            ("ANTARES_SWEEP_SECS", "1"),
            ("RUST_LOG", "debug"),
        ],
    )
    .spawn()
    .expect("spawn antares");
    let deadline = Instant::now() + Duration::from_secs(60);
    let health = loop {
        let r = get(port, "/q/health");
        if r.starts_with("HTTP/1.1 200") {
            break r;
        }
        if let Some(status) = child.try_wait().expect("try_wait") {
            panic!("the split-role boot exited with {status}");
        }
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(health.contains("\"poolWriteReserve\":1"), "{health}");
    assert!(health.contains("\"poolSize\":4"), "{health}");
    std::thread::sleep(Duration::from_millis(2500));
    let tenants = get(port, "/q/tenants");
    assert!(
        tenants.starts_with("HTTP/1.1 401"),
        "no admin token: /q stays shut: {tenants}"
    );
    common::stop(&mut child);
    let status = child.try_wait().expect("reaped").expect("exited");
    assert!(
        status.success(),
        "SIGTERM is a clean stop, not a failure: {status}"
    );
}

/// With an owner's pool, a request role that owns the tables could switch
/// their row-level security off: the boot refuses it.
#[test]
fn a_request_role_owning_the_tables_is_refused() {
    let Some(db) = Db::new("owns") else { return };
    let log = refused(&[
        ("ANTARES_MIGRATION_DATABASE_URL", &db.owner),
        ("ANTARES_DATABASE_URL", &db.owner),
    ]);
    assert!(log.contains("role owns a table of the schema"), "{log}");
}

/// A request role that bypasses RLS is a warning, and a refusal under
/// ANTARES_REQUIRE_RLS.
#[test]
fn a_request_role_bypassing_rls_is_refused_under_require_rls() {
    let Some(db) = Db::new("rls") else { return };
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_REQUIRE_RLS", "true"),
    ]);
    assert!(
        log.contains("ANTARES_REQUIRE_RLS=1 but the database role bypasses"),
        "{log}"
    );
}

/// A maintenance role outside `antares_maintenance` would drain and reap
/// nothing, silently: the boot refuses it and says which GRANT is missing.
#[test]
fn a_maintenance_role_outside_antares_maintenance_is_refused() {
    let Some(db) = Db::new("plain") else { return };
    let plain = db.role(PLAIN);
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_MAINTENANCE_DATABASE_URL", &plain),
    ]);
    assert!(
        log.contains("GRANT antares_maintenance TO it"),
        "the refusal names the fix: {log}"
    );
}

/// Under ANTARES_REQUIRE_RLS the maintenance role is held to the same rule
/// as the request role: a superuser there is refused.
#[test]
fn a_maintenance_role_bypassing_rls_is_refused_under_require_rls() {
    let Some(db) = Db::new("mrls") else { return };
    let request = db.role(REQUEST);
    let log = refused(&[
        ("ANTARES_MIGRATION_DATABASE_URL", &db.owner),
        ("ANTARES_DATABASE_URL", &request),
        ("ANTARES_MAINTENANCE_DATABASE_URL", &db.owner),
        ("ANTARES_REQUIRE_RLS", "1"),
    ]);
    assert!(
        log.contains("ANTARES_MAINTENANCE_DATABASE_URL role bypasses row-level security"),
        "{log}"
    );
}

/// A maintenance pool that cannot open stops the boot with its own key in
/// the message, never the URL's password.
#[test]
fn a_maintenance_pool_that_cannot_open_stops_the_boot() {
    let Some(db) = Db::new("mgone") else { return };
    let gone = format!("{}_gone", db.role(MAINTENANCE));
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_MAINTENANCE_DATABASE_URL", &gone),
    ]);
    assert!(
        log.contains("ANTARES_MAINTENANCE_DATABASE_URL: pool not opened"),
        "{log}"
    );
}

/// A database whose migration history another release wrote never fits this
/// binary: the boot stops at once instead of retrying for 30 s.
#[test]
fn a_database_another_release_migrated_stops_the_boot_at_once() {
    let Some(db) = Db::new("release") else { return };
    db.exec(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (0, 'another release', true, '\\x00', 0)",
    );
    let started = Instant::now();
    let log = refused(&[("ANTARES_DATABASE_URL", &db.owner)]);
    assert!(
        log.contains("the database was migrated by a different release"),
        "{log}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a schema mismatch is no reason to retry: {:?}",
        started.elapsed()
    );
}

/// A database server that accepts every connection and hangs up on it never
/// lets a pool open: the boot retries for its 30 s, then stops and says the
/// database is not reachable. Needs no database of its own.
#[test]
fn a_database_that_hangs_up_stops_the_boot_after_the_retry_window() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    let url = format!("postgresql://antares:{PASSWORD}@{addr}/antares");
    let started = Instant::now();
    let mut child = broker(free_port(), &[("ANTARES_DATABASE_URL", &url)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn antares");
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(90) {
            common::stop(&mut child);
            panic!("the boot did not give up on the database within 90 s");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut log = String::new();
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read>),
    ]
    .into_iter()
    .flatten()
    {
        let mut pipe = pipe;
        let _ = pipe.read_to_string(&mut log);
    }
    assert!(!status.success(), "an unreachable database fails the boot");
    assert!(log.contains("database not reachable after 30 s"), "{log}");
    assert!(
        started.elapsed() >= Duration::from_secs(29),
        "the boot gave up after {:?}, before its retry window",
        started.elapsed()
    );
    assert!(
        !log.contains(&format!(":{PASSWORD}@")),
        "the password in the output: {log}"
    );
}

/// The sealing key is read before any pool opens: one that is not 32 bytes
/// stops the boot with the key's name and the expected shape, never echoed.
#[test]
fn a_secret_key_that_is_not_32_bytes_stops_the_boot() {
    let Some(db) = Db::new("key") else { return };
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_SECRET_KEY", "not-a-key"),
    ]);
    assert!(log.contains("ANTARES_SECRET_KEY must be 32 bytes"), "{log}");
    assert!(!log.contains("not-a-key"), "the key echoed: {log}");
}

/// With ANTARES_SECRET_KEY set the boot seals every stored plaintext credential before it
/// serves; a database that refuses the seal's bookkeeping stops the boot with the key's name and
/// the database's reason, and never with the key.
#[test]
fn a_boot_whose_credential_sealing_fails_stops() {
    let Some(db) = Db::new("sealfail") else {
        return;
    };
    rt().block_on(async {
        let pool = sqlx::PgPool::connect(&db.owner).await.expect("owner pool");
        for sql in [
            "CREATE FUNCTION refuse_seal() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'seal walk refused by the test'; END $$",
            "CREATE TRIGGER refuse_seal BEFORE INSERT ON maintenance_jobs FOR EACH ROW
             WHEN (NEW.name = 'plaintext_seal_walk') EXECUTE FUNCTION refuse_seal()",
        ] {
            sqlx::query(sql).execute(&pool).await.expect(sql);
        }
        pool.close().await;
    });
    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_SECRET_KEY", KEY),
    ]);
    assert!(
        log.contains("ANTARES_SECRET_KEY: sealing stored credentials failed")
            && log.contains("seal walk refused by the test"),
        "{log}"
    );
    assert!(!log.contains(KEY), "the key echoed: {log}");
}

/// A retention horizon of zero or below would reap all history, current and
/// future: the boot refuses it, as it refuses one that is not a number.
#[test]
fn a_retention_horizon_outside_1_to_i32_max_stops_the_boot() {
    let Some(db) = Db::new("retain") else { return };
    for (bad, says) in [
        ("0", "must be between 1 and 2147483647"),
        ("2147483648", "must be between 1 and 2147483647"),
        ("a month", "must be an integer"),
    ] {
        let log = refused(&[
            ("ANTARES_DATABASE_URL", &db.owner),
            ("ANTARES_TEMPORAL_RETENTION_DAYS", bad),
        ]);
        assert!(log.contains(says), "{bad:?}: {log}");
    }
}

/// A NATS bus with no server to reach is a boot that cannot do its job:
/// refused before anything serves, naming the missing key.
#[test]
fn a_nats_bus_without_its_url_stops_the_boot() {
    let Some(db) = Db::new("nats") else { return };
    let log = refused(&[("ANTARES_DATABASE_URL", &db.owner), ("ANTARES_BUS", "nats")]);
    assert!(
        log.contains("ANTARES_BUS=nats requires ANTARES_NATS_URL"),
        "{log}"
    );
}

/// A maintenance role on a database this release never migrated cannot
/// answer the escape probe: the boot stops and says the probe failed.
#[test]
fn a_maintenance_database_without_the_schema_stops_the_boot() {
    let Some(db) = Db::new("probe") else { return };
    let unmigrated = db
        .role(MAINTENANCE)
        .replace(&format!("/{}", db.name), "/postgres");
    let log = refused(&[
        ("ANTARES_DATABASE_URL", &db.owner),
        ("ANTARES_MAINTENANCE_DATABASE_URL", &unmigrated),
    ]);
    assert!(
        log.contains("ANTARES_MAINTENANCE_DATABASE_URL: escape probe failed"),
        "{log}"
    );
}

/// A write reserve its role cannot open (the main pool holds the role's
/// connections) stops the boot naming the reserve.
#[test]
fn a_write_reserve_that_cannot_open_stops_the_boot() {
    let Some(db) = Db::new("reserve") else { return };
    let limited = db.role(LIMITED);
    let log = refused(&[
        ("ANTARES_MIGRATION_DATABASE_URL", &db.owner),
        ("ANTARES_DATABASE_URL", &limited),
        ("ANTARES_PG_POOL", "4"),
        ("ANTARES_PG_WRITE_RESERVE", "1"),
    ]);
    assert!(
        log.contains("ANTARES_PG_WRITE_RESERVE: reserve pool not opened"),
        "{log}"
    );
}

/// A request role the server allows a single connection cannot serve the
/// boot's probes: the boot stops with the pool's answer under its store mode.
#[test]
fn a_request_role_with_one_connection_stops_the_boot() {
    let Some(db) = Db::new("single") else { return };
    let single = db.role(SINGLE);
    let log = refused(&[
        ("ANTARES_MIGRATION_DATABASE_URL", &db.owner),
        ("ANTARES_DATABASE_URL", &single),
    ]);
    assert!(
        log.contains("ANTARES_STORE=postgres: pool timed out"),
        "{log}"
    );
}
