// SPDX-License-Identifier: EUPL-1.2
//! Antares — NGSI-LD context broker (composition root).
//!
//! Config: ANTARES_* env vars only for v0 (antares.toml layering can land
//! later via figment). Unknown ANTARES_* keys are fatal.
#![cfg_attr(not(test), warn(clippy::expect_used))]

mod shutdown;
mod telemetry;
mod wiring;
#[cfg(feature = "ws")]
mod ws_relay;

use antares_api::AppState;

// ANTARES_DATABASE_URL: accepted — the ETSI compose wires one DB per broker —
// consumed by the postgres/timescale store modes.
const KNOWN_KEYS: &[&str] = &[
    "ANTARES_HTTP_PORT",
    "ANTARES_HOST_ALIAS",
    // 5.8.1.4 distributed subscriptions: the public base URL other brokers
    // reach this one at (the reduced-copy notification endpoint); defaults
    // to http://{host_alias}.
    "ANTARES_PUBLIC_URL",
    // Internal context sources (ADR-0025 rule 1): the URLs this deployment
    // answers under. Accepted in every build: one without the
    // `internal-csr` feature says once that it does not read it.
    "ANTARES_SELF_URLS",
    "ANTARES_ROLES",
    // The example policy engine's rules document; accepted only in a build
    // that carries the engine, so a release binary rejects it like any
    // other key it has no code for.
    #[cfg(feature = "plugin-example")]
    antares_plugin_example::RULES_ENV,
    "ANTARES_DATABASE_URL",
    "ANTARES_STORE",
    // Temporal driver: a store mode, or `none` — history off (temporal
    // reads answer OperationNotSupported, Table 6.3.2-1). Defaults to the
    // current-state store, so one instance serves both seams.
    "ANTARES_TEMPORAL",
    // History gate: `all` (default) records every changed instance;
    // `observed` records only instances carrying observedAt; `none`
    // auto-records nothing (temporal API + reads stay on).
    "ANTARES_TEMPORAL_RECORD",
    // behaviour profile (T-5497, docs/conformance/profiles.md)
    "ANTARES_NON_OBJECT_BODY",
    "ANTARES_FED_WRITE_SOURCE_404",
    "ANTARES_IMPLICIT_TENANTS",
    // History dedup on append (T-4507): `off` records an instance equal to the
    // newest one again; anything else (default) skips it.
    "ANTARES_TEMPORAL_DEDUP",
    // 6.3.10 ceiling: instances of one Attribute per temporal response
    // (default 1000; the ETSI brokers set 9 for the suite's 206 cases), and
    // the Entities of one query page (default 1000). T-4800.
    "ANTARES_TEMPORAL_INSTANCE_LIMIT",
    "ANTARES_MAX_LIMIT",
    // The defaults when a request names no lastN / limit (100 each, never
    // above the ceilings above). T-4804.
    "ANTARES_TEMPORAL_DEFAULT_INSTANCES",
    "ANTARES_DEFAULT_LIMIT",
    "ANTARES_DATA_DIR",
    // Egress: private-range destinations are ALLOWED by default (ADR-0010 —
    // brokers federate inside private networks); a hardened deployment sets
    // this to false to arm the SSRF wall.
    "ANTARES_EGRESS_ALLOW_PRIVATE",
    // Host names that may still resolve to a private address while the wall
    // is armed: the in-cluster peers (T-3908); never loopback or metadata.
    "ANTARES_EGRESS_PRIVATE_HOSTS",
    // Refuse to start when the DB role bypasses RLS (production gate;
    // default off so the dev/ETSI superuser stack still boots).
    "ANTARES_REQUIRE_RLS",
    // Temporal retention horizon in days; absent = keep forever (a
    // maintenance job must never default to dropping data).
    "ANTARES_TEMPORAL_RETENTION_DAYS",
    // T-4665: a value older than the current one goes to history only
    // (`on`); off keeps NGSI-LD's last-write-wins.
    "ANTARES_HISTORY_BACKFILL",
    // T-4665: the gateway's mesh identity; only it is believed in its
    // provenance headers. Unset, every write is `channel: api`. Read by the
    // `jc` build only (T-4854); vanilla ignores it.
    "ANTARES_PROVENANCE_FROM",
    // T-4875: the mesh identities (l5d-client-id) whose policy subject
    // headers are believed; from anyone else they are dropped. Unset,
    // every caller's are, as before.
    "ANTARES_POLICY_SUBJECT_FROM",
    // 4.22 GC interval (memory/file arm); default 900 s, the ETSI stack runs
    // at 2 s so the transient TPs (422_01) exercise the sweep itself.
    "ANTARES_SWEEP_SECS",
    // Batch entity-count cap; default 1000 — raised where a
    // trusted producer legitimately batches larger (the spec sets no ceiling).
    "ANTARES_MAX_BATCH_ITEMS",
    "ANTARES_MAX_BODY_BYTES",
    // The process-wide memory budget the caches and the buffered-answer pool
    // split (T-5895).
    "ANTARES_MEMORY_BUDGET",
    "ANTARES_CORS_ORIGINS",
    // The policy engine every operation is asked about, and how long it has
    // to answer before the seam denies (ADR-0020). The built-in allow-all
    // engine decides nothing and never waits; the header list is what an
    // engine is given to identify the caller by.
    "ANTARES_POLICY",
    "ANTARES_POLICY_SUBJECT_HEADERS",
    "ANTARES_POLICY_TIMEOUT_MS",
    // The HTTP surfaces mounted beside the NGSI-LD API root, comma-separated;
    // default `admin` (/q). An unknown name is fatal and names the shelf.
    "ANTARES_API_SURFACES",
    // The bridges inbound Channels bind on (ADR-0031, feature `channels`):
    // `name=uri` pairs, `uri` being `loopback` or an mqtt(s) base whose
    // credential stays in this variable. A malformed list is fatal.
    "ANTARES_CHANNEL_BROKERS",
    // The /q admin credential (T-3569): a file holding the bearer secret,
    // and whether /q/metrics answers without it (token | open).
    "ANTARES_ADMIN_TOKEN_FILE",
    "ANTARES_ADMIN_METRICS",
    // A second listener that serves /q/metrics and nothing else; set, the
    // API port answers /q/metrics 404 (T-5095). Unset = /q/metrics on the API port.
    "ANTARES_METRICS_PORT",
    // Drain: the LB notice window, and the ceiling on waiting for
    // in-flight requests once the listener has closed.
    "ANTARES_DRAIN_DELAY_MS",
    "ANTARES_DRAIN_DEADLINE_SECS",
    // Optional PEM bundle of extra TLS trust anchors (private CAs,
    // incomplete-chain servers). Never disables verification.
    "ANTARES_EXTRA_CA_FILE",
    // The bus seam: local (default, single process, all roles) or
    // nats (the JetStream spine — requires a postgres/timescale store and
    // ANTARES_NATS_URL).
    "ANTARES_BUS",
    "ANTARES_NATS_URL",
    // Stream/KV replication factor on a clustered JetStream (3 for
    // the reference manifests' R3; default 1 for single-node).
    "ANTARES_NATS_REPLICAS",
    // OTLP/HTTP span export endpoint (e.g. http://collector:4318/v1/traces);
    // unset = no OTLP anywhere.
    "ANTARES_OTLP_ENDPOINT",
    "ANTARES_TELEMETRY",
    // Outbox drain on this pod, on (default) | off. `off` is the
    // crash-drill lever (rows commit but this pod never publishes them —
    // another pod's drain must) and the knob for a dedicated-drainer split.
    "ANTARES_OUTBOX_DRAIN",
    // Notification delivery policy: total attempts (default 1 = 5.8.6 as
    // written), first-retry backoff, and the age after which no retry
    // starts. An exhausted policy leaves a dead letter (/q/dead-letters).
    "ANTARES_NOTIFY_ATTEMPTS",
    "ANTARES_NOTIFY_BACKOFF_MS",
    "ANTARES_NOTIFY_MAX_AGE_SECS",
    // Postgres pool size (max connections); default 20.
    "ANTARES_PG_POOL",
    // connections of that pool only batch writes take (T-5150); default 0.
    "ANTARES_PG_WRITE_RESERVE",
    "ANTARES_PG_STATEMENT_TIMEOUT_MS",
    // batch writes commit with synchronous_commit = off (T-5050); default off.
    "ANTARES_PG_BATCH_ASYNC_COMMIT",
    // T-5838: the key that seals Subscription receiverInfo at rest.
    "ANTARES_SECRET_KEY",
    // bus=local over a shared postgres/timescale store is refused — every
    // replica would run its own matcher and fire its own copy of each
    // notification. This opt-in states the deployment runs exactly ONE
    // broker process against that database.
    "ANTARES_ALLOW_SHARED_LOCAL",
    // HTTP/1 header read timeout in ms (default 10000): a connection that
    // never finishes its request headers is closed instead of holding a
    // slot forever.
    "ANTARES_HEADER_READ_TIMEOUT_MS",
    // Ceiling on concurrently served connections (default 10000);
    // connections accepted above it are dropped immediately.
    "ANTARES_MAX_CONNECTIONS",
    // 5.8.6 delivery concurrency: notifications in flight at once for the
    // whole broker, and the share of that width one tenant may hold.
    "ANTARES_DELIVERY_WIDTH",
    "ANTARES_DELIVERY_WIDTH_PER_TENANT",
    // Estimated heap every delivery lane may hold at once (T-4549).
    "ANTARES_DELIVERY_LANE_BYTES",
    // Batch write bodies in flight at once, in bytes (T-4562).
    "ANTARES_BATCH_INFLIGHT_BYTES",
    // Batch writes running at once, each holding a connection (T-4624).
    "ANTARES_BATCH_INFLIGHT",
    // 5.7.2.4 fan-out ceiling: how many matching registrations one
    // distributed operation may contact.
    "ANTARES_FED_FANOUT",
    "ANTARES_FED_INFLIGHT",
    // Ceiling on the body this broker will read back from a forwarded
    // request.
    "ANTARES_MAX_FED_RESPONSE_BYTES",
    // 5.7.5/5.7.6 discovery scan ceiling (types/attributes listing).
    "ANTARES_DISCOVERY_SCAN_MAX",
    // Run the DDL on this process; off keeps replicas from racing the
    // migration on boot.
    "ANTARES_MIGRATE",
];

/// The keys of each surface this binary was built with, listed by the crate
/// that reads them: a surface's new variable lands in its own crate, not in
/// this file, which every surface branch used to edit (T-3766).
const SURFACE_KEYS: &[&[&str]] = &[
    // the view table's reload interval and LinkML schema bound
    #[cfg(feature = "views")]
    antares_ops::views::ENV_KEYS,
    // the most views one read across views names (T-4494)
    #[cfg(feature = "views")]
    antares_views::ENV_KEYS,
    // hosts, origins, result budget, the confirmation key, tool endpoints
    #[cfg(feature = "mcp")]
    antares_mcp::ENV_KEYS,
    // how many files the OGC surface writes at once
    #[cfg(feature = "ogc")]
    antares_ogc::ENV_KEYS,
    // the WebSocket channel's bounds (ADR-0026 rules 6 to 10)
    #[cfg(feature = "ws")]
    antares_ws::ENV_KEYS,
];

/// Every config key this binary accepts.
fn known_keys() -> impl Iterator<Item = &'static str> {
    KNOWN_KEYS
        .iter()
        .chain(SURFACE_KEYS.iter().flat_map(|keys| keys.iter()))
        .copied()
}

/// Jemalloc with decay-based purging — RSS returns to ~live×1.2 when idle;
/// tune via MALLOC_CONF (e.g. dirty_decay_ms). Deliberately not glibc
/// malloc, whose arena fragmentation never gives memory back.
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Unknown-config-is-fatal, minus what the platform injects: a Service
/// named `antares*` makes kubelet write ANTARES_PORT,
/// ANTARES_PORT_9090_TCP*, ANTARES_SERVICE_* (and the antares-file /
/// antares-api variants) into every pod, and treating those as typos put the
/// shipped manifests into 100% CrashLoopBackOff. The manifests also set
/// enableServiceLinks: false — this check is the belt for clusters that
/// re-enable links or add their own Services.
fn unknown_config_key(key: &str) -> bool {
    if !key.starts_with("ANTARES_") || key.starts_with("ANTARES_TEST_") {
        return false;
    }
    if known_keys().any(|known| known == key) {
        return false;
    }
    // kubelet service-link shapes for the Services OUR manifests ship
    // (antares, antares-file, antares-api, antares-worker): {NAME}_PORT,
    // {NAME}_PORT_<n>_<proto>*, {NAME}_SERVICE_*. Only those exact name
    // infixes are exempt — an arbitrary ANTARES_*-shaped var stays a fatal
    // typo, and foreign Services are covered by
    // enableServiceLinks: false in the manifests.
    let rest = &key["ANTARES_".len()..];
    let injected = ["", "FILE_", "API_", "WORKER_"].iter().any(|infix| {
        rest.strip_prefix(infix)
            .is_some_and(|t| t == "PORT" || t.starts_with("PORT_") || t.starts_with("SERVICE_"))
    });
    !injected
}

/// ANTARES_SWEEP_SECS paces the 4.22 expiry sweep in every store mode. Absent
/// is the 15 min default; anything that is not a positive integer is fatal,
/// because a garbage cadence silently becoming the default one is exactly the
/// misconfiguration the unknown-key policy exists to catch.
fn parse_sweep_secs(raw: Option<&str>) -> Result<u64, String> {
    let Some(v) = raw else {
        return Ok(15 * 60);
    };
    match v.parse::<u64>() {
        Ok(0) | Err(_) => Err(format!(
            "ANTARES_SWEEP_SECS must be a positive integer number of seconds, got {v:?}"
        )),
        Ok(n) => Ok(n),
    }
}

/// An explicitly-off switch value, whatever the operator's spelling. Used by
/// the knobs where the DEFAULT is off, so that only an off value keeps them
/// off and a typo cannot silently disable a security control.
fn is_off(v: &str) -> bool {
    let v = v.trim();
    v.is_empty()
        || v == "0"
        || v.eq_ignore_ascii_case("false")
        || v.eq_ignore_ascii_case("off")
        || v.eq_ignore_ascii_case("no")
}

/// ANTARES_METRICS_PORT: unset or empty is no metrics listener; a port is
/// one that differs from the API port (both binds would fail, the second
/// one only after the store connected).
fn parse_metrics_port(raw: Option<&str>, api_port: u16) -> Result<Option<u16>, String> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    match raw.parse::<u16>() {
        Ok(p) if p != 0 && p != api_port => Ok(Some(p)),
        Ok(p) => Err(format!(
            "ANTARES_METRICS_PORT must be a port 1-65535 other than ANTARES_HTTP_PORT {api_port}, got {p}"
        )),
        Err(e) => Err(format!(
            "ANTARES_METRICS_PORT must be a port number 1-65535, got {raw:?} ({e})"
        )),
    }
}

/// One plain HTTP/1.0 GET of /q/health on the configured port; anything but
/// a 200 status line is an error, so `HEALTHCHECK` sees exit 1.
fn health_probe() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let port = std::env::var("ANTARES_HTTP_PORT").unwrap_or_else(|_| "9090".into());
    let timeout = std::time::Duration::from_secs(3);
    let addr = format!("127.0.0.1:{port}").parse::<std::net::SocketAddr>()?;
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.write_all(b"GET /q/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut head = [0u8; 16];
    s.read_exact(&mut head)?;
    if head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200") {
        Ok(())
    } else {
        Err(format!("health: {}", String::from_utf8_lossy(&head)).into())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --version answers without starting anything (a bare `antares
    // --version` used to boot a server).
    // `args()`/`vars()` PANIC on non-UTF-8; the *_os variants do not, and a
    // stray byte in the environment or argv must not kill the process.
    if std::env::args_os().any(|a| a == "--version" || a == "-V") {
        println!(
            "antares {} ({})",
            env!("CARGO_PKG_VERSION"),
            antares_api::GIT_HASH
        );
        return Ok(());
    }
    if std::env::args_os().any(|a| a == "--help" || a == "-h") {
        println!(
            "antares {} — NGSI-LD context broker (ETSI GS CIM 009 V1.9.1)\n\n\
             Usage: antares [--version | --health | --help]\n\n\
             Configuration is environment only; every accepted key is listed\n\
             below and documented with its default in docs/src/configuration.md\n\
             (an unknown ANTARES_* key is fatal at startup).\n",
            env!("CARGO_PKG_VERSION")
        );
        for key in known_keys() {
            println!("  {key}");
        }
        return Ok(());
    }
    // --health is the container health probe: the image has no shell or
    // curl, so the binary asks its own /q/health and exits 0 only on 200.
    if std::env::args_os().any(|a| a == "--health") {
        return health_probe();
    }
    // reqwest is built provider-less, so the FIRST client anything in this
    // process builds decides whether it panics — and the OTLP exporter
    // builds one inside `opentelemetry-http`, before any broker code runs.
    // Installed here, ahead of telemetry, rather than only in
    // `client_builder`, which a dependency does not call.
    antares_jsonld::install_crypto_provider();

    // Tracing (fmt + env-gated OTLP [+ console feature]) and, with the
    // `telemetry` feature, the Prometheus recorder rendering /q/metrics.
    let metrics_render = telemetry::init()?;

    // Unknown-config-is-fatal: catch typos before they become Scorpio's
    // $[quarkus.uuid} class of silent misconfiguration. ANTARES_TEST_* is the
    // reserved harness namespace (ANTARES_TEST_DATABASE_URL, ANTARES_TEST_MQTT_URL,
    // …) — CI exports those for the integration tests, and they land in the env of
    // any broker a test spawns. Reserving the prefix here beats making every
    // spawn site remember an env_remove allowlist.
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if unknown_config_key(&key) {
            let known: Vec<&str> = known_keys().collect();
            return Err(format!("unknown config key {key} (known: {known:?})").into());
        }
    }

    let port_raw = std::env::var("ANTARES_HTTP_PORT").unwrap_or_else(|_| "9090".into());
    let port: u16 = port_raw.parse().map_err(|e| {
        format!("ANTARES_HTTP_PORT must be a port number 0-65535, got {port_raw:?} ({e})")
    })?;
    let metrics_port =
        parse_metrics_port(std::env::var("ANTARES_METRICS_PORT").ok().as_deref(), port)?;
    // Every remaining config value is parsed HERE, before the runtime starts,
    // so a garbage window, cadence or switch fails the process instead of
    // silently running at its default.
    let sweep_secs = parse_sweep_secs(std::env::var("ANTARES_SWEEP_SECS").ok().as_deref())?;
    // Same: validated here, read again where the state is built.
    antares_api::DeliveryPolicy::from_env()?;
    let drain_delay = shutdown::drain_delay()?;
    let drain_deadline = shutdown::drain_deadline()?;
    // Validated here so a typo fails startup; the value itself is read again
    // where the drain task is wired, which is the only place it is used.
    wiring::outbox_drain_enabled()?;
    let host_alias = std::env::var("ANTARES_HOST_ALIAS").unwrap_or_else(|_| "antares".into());
    // 6.3.18 sends this as the Via pseudonym — an RFC 7230 token. `~` is
    // reserved as the tenant separator (federation::alias_for), so allowing
    // it in the configured alias would let `a~b` in the default tenant
    // collide with `a` in tenant `b` and cross-detect as a loop. Fatal at
    // startup, like every other bad config value.
    if host_alias.is_empty()
        || !host_alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|".contains(&b))
    {
        return Err(format!(
            "ANTARES_HOST_ALIAS {host_alias:?} is not a valid RFC 7230 token \
             (and may not contain '~', the tenant separator)"
        )
        .into());
    }
    let roles = std::env::var("ANTARES_ROLES").unwrap_or_else(|_| "all".into());
    // Unknown store backend is fatal BEFORE the runtime spins up, and the
    // message names the shelf this binary was built with — never a silent
    // fallback to memory.
    let store_name = std::env::var("ANTARES_STORE").unwrap_or_else(|_| "memory".into());
    if !store_shelf().contains(&store_name.as_str()) {
        return Err(format!(
            "unknown ANTARES_STORE {store_name:?}; built with {}",
            store_shelf().join("|")
        )
        .into());
    }
    // bus=local wires an in-process matcher into every process, so N
    // replicas over ONE shared database each fire their own copy of every
    // notification. Refused here — before any store connection is attempted
    // — unless the deployment states it runs exactly one broker process.
    // (Mirror of the nats arm's store check in `run`.)
    let bus_mode = std::env::var("ANTARES_BUS").unwrap_or_else(|_| "local".into());
    if bus_mode == "local"
        && shared_state(&store_name)
        && !std::env::var("ANTARES_ALLOW_SHARED_LOCAL")
            .is_ok_and(|v| matches!(v.as_str(), "1" | "true"))
    {
        return Err(format!(
            "ANTARES_BUS=local with ANTARES_STORE={store_name} double-fires notifications when \
             more than one broker process shares the database (each process runs its own \
             matcher). Use ANTARES_BUS=nats, or set ANTARES_ALLOW_SHARED_LOCAL=1 for a \
             strictly single-process deployment"
        )
        .into());
    }

    runtime()?.block_on(async {
        let drivers = build_drivers(&store_name).await?;
        run(
            port,
            host_alias,
            roles,
            drivers.store,
            drivers.temporal,
            store_name,
            drivers.temporal_name,
            drivers.maintenance,
            metrics_render,
            metrics_port,
            sweep_secs,
            drain_delay,
            drain_deadline,
        )
        .await
    })
}

/// ANTARES_PG_STATEMENT_TIMEOUT_MS → the per-session `statement_timeout`
/// every pooled connection carries (a runaway query is cancelled, 5.5.2
/// InternalError); absent = 30 000; not a positive integer = fatal.
fn parse_pg_statement_timeout(raw: Option<&str>) -> Result<std::time::Duration, String> {
    match raw {
        None => Ok(std::time::Duration::from_secs(30)),
        Some(v) => match v.parse::<u64>() {
            Ok(n) if n > 0 => Ok(std::time::Duration::from_millis(n)),
            _ => Err(format!(
                "ANTARES_PG_STATEMENT_TIMEOUT_MS must be a positive integer (got {v:?})"
            )),
        },
    }
}

/// ANTARES_PG_BATCH_ASYNC_COMMIT → batch writes commit asynchronously
/// (T-5050): absent or an off value = no; `1`/`true`/`on`/`yes` = yes;
/// anything else is fatal, so a typo never silently picks a durability.
fn parse_pg_batch_async_commit(raw: Option<&str>) -> Result<bool, String> {
    match raw.map(str::trim) {
        None => Ok(false),
        Some(v) if is_off(v) => Ok(false),
        Some(v)
            if v == "1"
                || ["true", "on", "yes"]
                    .iter()
                    .any(|o| v.eq_ignore_ascii_case(o)) =>
        {
            Ok(true)
        }
        Some(v) => Err(format!(
            "ANTARES_PG_BATCH_ASYNC_COMMIT must be true or false (got {v:?})"
        )),
    }
}

/// ANTARES_PG_POOL → pool size: absent defaults to 20; anything that is not
/// a positive integer is fatal (a misread size must never silently run with
/// a default, matching the unknown-key policy).
fn parse_pg_pool(raw: Option<&str>) -> Result<u32, String> {
    match raw {
        None => Ok(20),
        Some(v) => match v.parse::<u32>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!(
                "ANTARES_PG_POOL must be a positive integer (got {v:?})"
            )),
        },
    }
}

/// ANTARES_PG_WRITE_RESERVE → how many of the `pool` connections only the
/// batch writes take (T-5150): absent or 0 is off (one shared pool). A
/// reserve must leave the reads at least one connection; anything else is
/// fatal, like ANTARES_PG_POOL.
fn parse_pg_write_reserve(raw: Option<&str>, pool: u32) -> Result<u32, String> {
    let Some(v) = raw else { return Ok(0) };
    match v.parse::<u32>() {
        Ok(n) if n < pool => Ok(n),
        Ok(n) => Err(format!(
            "ANTARES_PG_WRITE_RESERVE ({n}) must be below ANTARES_PG_POOL ({pool}): \
             the reads need at least one connection"
        )),
        Err(_) => Err(format!(
            "ANTARES_PG_WRITE_RESERVE must be a non-negative integer (got {v:?})"
        )),
    }
}

/// What ANTARES_TEMPORAL resolved to, before anything is built.
#[derive(Debug, PartialEq, Eq)]
enum TemporalChoice {
    /// The current-state store records and serves history too (default).
    SameAsStore,
    /// History off: `NoTemporal`.
    None,
    /// A second store instance of this backend, used only through its
    /// temporal half.
    Second(String),
}

/// The current-state backends this binary can build: the built-ins, plus
/// any driver compiled in from outside this workspace. A list of NAMES and
/// not an enum, because a plugin's driver is not one of `StoreMode`'s arms
/// — adding a backend must never mean editing a core crate.
fn store_shelf() -> Vec<&'static str> {
    // Backends from outside this workspace, in selection order. Each is
    // compiled in by its own feature; without one the list is empty and the
    // shelf is exactly `StoreMode`.
    const PLUGINS: &[&str] = &[
        #[cfg(feature = "plugin-example")]
        antares_plugin_example::NAME,
    ];
    antares_sql::StoreMode::ALL
        .into_iter()
        .map(|m| m.as_str())
        .chain(PLUGINS.iter().copied())
        .collect()
}

/// Does this backend keep its state where several broker processes can
/// reach it? The precondition for `ANTARES_BUS=nats`, and the reason
/// `bus=local` over one shared database double-fires notifications. Only
/// the database backends qualify: a per-process store — memory, file, or a
/// driver from outside the workspace — never does.
fn shared_state(name: &str) -> bool {
    name.parse::<antares_sql::StoreMode>()
        .is_ok_and(|m| m.is_pg())
}

/// The shelf this binary was built with, rendered from the backend list
/// rather than spelled out: a backend reaches every message that names the
/// shelf without a second edit, whether it came from `StoreMode` or from a
/// crate outside this workspace.
fn built_with() -> String {
    format!("{} (temporal also: none)", store_shelf().join("|"))
}

/// ANTARES_TEMPORAL → driver choice. Absent or the store's own mode = one
/// instance for both seams; `none` = no history; any other backend name =
/// a second store. An unknown name is fatal and names the shelf.
fn temporal_choice(store_name: &str, raw: Option<&str>) -> Result<TemporalChoice, String> {
    match raw {
        None => Ok(TemporalChoice::SameAsStore),
        Some(m) if m == store_name => Ok(TemporalChoice::SameAsStore),
        Some("none") => Ok(TemporalChoice::None),
        Some(other) if store_shelf().contains(&other) => {
            Ok(TemporalChoice::Second(other.to_owned()))
        }
        Some(other) => Err(format!(
            "ANTARES_TEMPORAL: unknown backend {other:?}; built with {}",
            built_with()
        )),
    }
}

/// One entry of the surface shelf: the name a deployment selects it with,
/// and how to build it.
type SurfaceCtor = fn() -> Box<dyn antares_api::ApiSurface>;

/// The HTTP surfaces this binary was built with, outside the NGSI-LD API
/// root. Every one is compiled in, so the shelf is static; a feature-gated
/// surface would drop out here.
const SURFACE_SHELF: &[(&str, SurfaceCtor)] = &[
    ("admin", || Box::new(antares_api::Admin)),
    #[cfg(feature = "views")]
    ("views", || antares_ops::mount(antares_views::Views)),
    #[cfg(feature = "mcp")]
    ("mcp", || antares_ops::mount(antares_mcp::Mcp)),
    #[cfg(feature = "linkml")]
    ("linkml", || antares_ops::mount(antares_linkml::Linkml)),
    #[cfg(feature = "odata")]
    ("odata", || antares_ops::mount(antares_odata::OData)),
    #[cfg(feature = "ogc")]
    ("ogc", || antares_ops::mount(antares_ogc::Ogc)),
    #[cfg(feature = "sta")]
    ("sta", || antares_ops::mount(antares_sta::Sta)),
    #[cfg(feature = "ical")]
    ("ical", || antares_ops::mount(antares_ical::Ical)),
    #[cfg(feature = "geojson")]
    ("geojson", || antares_ops::mount(antares_geojson::GeoJson)),
    #[cfg(feature = "feed")]
    ("feed", || antares_ops::mount(antares_feed::Feed)),
    #[cfg(feature = "gbfs")]
    ("gbfs", || antares_ops::mount(antares_gbfs::Gbfs)),
    #[cfg(feature = "gtfsrt")]
    ("gtfsrt", || antares_ops::mount(antares_gtfsrt::GtfsRt)),
    #[cfg(feature = "jsonstat")]
    ("jsonstat", || {
        antares_ops::mount(antares_jsonstat::JsonStat)
    }),
    #[cfg(feature = "ws")]
    ("ws", || antares_ops::mount(antares_ws::Ws::process())),
    #[cfg(all(feature = "ws", feature = "views"))]
    ("ws-views", || {
        antares_ops::mount(antares_ws::ViewWs::process())
    }),
    #[cfg(all(feature = "ws", feature = "sta"))]
    ("sta-mqtt", || {
        antares_ops::mount(antares_ws::StaMqtt::process())
    }),
    #[cfg(feature = "channels")]
    ("channels", || {
        // `ANTARES_CHANNEL_BROKERS` was checked before the shelf is built
        Box::new(
            antares_api::channels::Channels::from_env()
                .unwrap_or_else(|_| antares_api::channels::Channels::new(Default::default())),
        )
    }),
    #[cfg(feature = "plugin-example")]
    ("example", || {
        Box::new(antares_plugin_example::ExampleSurface)
    }),
    // The joinedcontext engine's access surface reads the table the
    // engine of this process decides with (addon README section 4).
    #[cfg(all(feature = "jc-policy", feature = "views"))]
    (joinedcontext_policy::surface::NAME, || {
        antares_ops::mount(joinedcontext_policy::surface::Access::new(
            joinedcontext_policy::JoinedcontextPolicy::process(),
        ))
    }),
];

/// ANTARES_API_SURFACES → the surfaces mounted beside the NGSI-LD API,
/// comma-separated; absent = `admin`. An unknown name is fatal and names
/// the shelf, and so is a selection that ends up empty: health, readiness
/// and metrics are the admin surface, and a pod that serves none of them
/// can never report itself up.
fn api_surfaces(raw: Option<&str>) -> Result<Vec<(&'static str, SurfaceCtor)>, String> {
    let mut chosen = Vec::new();
    for name in raw
        .unwrap_or("admin")
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        let entry = SURFACE_SHELF
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| {
                let shelf: Vec<&str> = SURFACE_SHELF.iter().map(|(n, _)| *n).collect();
                format!(
                    "ANTARES_API_SURFACES: unknown surface {name:?}; built with {}",
                    shelf.join("|")
                )
            })?;
        chosen.push(*entry);
    }
    if chosen.is_empty() {
        return Err(
            "ANTARES_API_SURFACES selects no surface; /q (health, readiness, metrics) \
                    is the admin surface"
                .into(),
        );
    }
    Ok(chosen)
}

/// One entry of the policy shelf: the name a deployment selects the engine
/// with, and how to build it.
type PolicyCtor = fn() -> std::sync::Arc<dyn antares_api::policy::PolicyEngine>;

/// The policy engines this binary was built with (ADR-0020). `allow-all` is
/// the one the broker ships and the one conformance is asserted against; an
/// engine from outside this workspace joins the list behind its own
/// off-by-default feature, exactly as a store or a surface does, and is
/// absent from a release build.
const POLICY_SHELF: &[(&str, PolicyCtor)] = &[
    ("allow-all", || {
        std::sync::Arc::new(antares_api::policy::AllowAll)
    }),
    #[cfg(feature = "plugin-example")]
    (antares_plugin_example::POLICY_NAME, || {
        std::sync::Arc::new(antares_plugin_example::ExamplePolicy::from_env())
    }),
    #[cfg(feature = "jc-policy")]
    (joinedcontext_policy::POLICY_NAME, || {
        joinedcontext_policy::JoinedcontextPolicy::process()
    }),
];

/// ANTARES_POLICY → the engine every operation is asked about; absent =
/// `allow-all`. An unknown name is fatal and names the shelf: a deployment
/// that meant to run its engine and got a typo would otherwise serve every
/// request wide open and never know.
fn policy_engine(raw: Option<&str>) -> Result<(&'static str, PolicyCtor), String> {
    let name = raw
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("allow-all");
    POLICY_SHELF
        .iter()
        .find(|(n, _)| *n == name)
        .copied()
        .ok_or_else(|| {
            let shelf: Vec<&str> = POLICY_SHELF.iter().map(|(n, _)| *n).collect();
            format!(
                "ANTARES_POLICY: unknown policy engine {name:?}; built with {}",
                shelf.join("|")
            )
        })
}

/// The backend registry: the two driver seams from their configured names.
/// Every backend is one arm of `build_store`; the temporal driver is by
/// default the same instance (history recorded and served by the
/// current-state store), `none` turns history off (temporal reads answer
/// OperationNotSupported 422, Table 6.3.2-1; the recorder produces
/// nothing), and a different backend name builds a second store used only
/// through its temporal half.
async fn build_drivers(store_name: &str) -> Result<Drivers, Box<dyn std::error::Error>> {
    let built = build_store(store_name, false).await?;
    // Every pg half gets the maintenance job: partitions, retention and the
    // 4.22 reap belong to whichever database holds the history.
    let mut maintenance = Vec::from_iter(built.maintenance);
    let raw = std::env::var("ANTARES_TEMPORAL").ok();
    let (temporal, temporal_name) = match temporal_choice(store_name, raw.as_deref())? {
        TemporalChoice::SameAsStore => {
            let name = built.temporal.supported().then(|| store_name.to_owned());
            (built.temporal.clone(), name)
        }
        TemporalChoice::None => (
            std::sync::Arc::new(antares_store::NoTemporal)
                as std::sync::Arc<dyn antares_store::TemporalDriver>,
            None,
        ),
        TemporalChoice::Second(name) => {
            let second = build_store(&name, true).await?;
            maintenance.extend(second.maintenance);
            (second.temporal, Some(name))
        }
    };
    Ok(Drivers {
        store: built.store,
        temporal,
        maintenance,
        temporal_name,
    })
}

/// The two storage seams a running broker holds, and what the maintenance
/// job and `/q/health` need to know about them.
struct Drivers {
    store: std::sync::Arc<dyn antares_store::CurrentStateDriver>,
    temporal: std::sync::Arc<dyn antares_store::TemporalDriver>,
    maintenance: Vec<(
        antares_sql::sqlx::PgPool,
        antares_sql::store::pg::maintenance::TemporalBackend,
    )>,
    /// What `/q/health` calls the history backend; `None` = history off.
    temporal_name: Option<String>,
}

/// One built backend: the two driver seams of a single store instance, and
/// the Postgres handles its maintenance job needs.
struct Built {
    store: std::sync::Arc<dyn antares_store::CurrentStateDriver>,
    temporal: std::sync::Arc<dyn antares_store::TemporalDriver>,
    maintenance: Option<(
        antares_sql::sqlx::PgPool,
        antares_sql::store::pg::maintenance::TemporalBackend,
    )>,
}

/// One backend by name. `temporal_only` builds an instance whose
/// current-state half is never served — the second store of a split
/// `ANTARES_TEMPORAL`. An unknown name is an error naming the shelf.
async fn build_store(name: &str, temporal_only: bool) -> Result<Built, Box<dyn std::error::Error>> {
    #[cfg(feature = "plugin-example")]
    if name == antares_plugin_example::NAME {
        // A driver from outside `crates/` mounts through exactly the two
        // trait objects a built-in does; nothing downstream can tell them
        // apart, and its history half is its own — no `temporal_only` flag
        // to set, because the current-state half is simply never asked.
        let store = std::sync::Arc::new(antares_plugin_example::ExampleStore::new());
        return Ok(Built {
            store: store.clone(),
            temporal: store,
            maintenance: None,
        });
    }
    let mode: antares_sql::StoreMode = name.parse().map_err(|_| {
        format!(
            "unknown store backend {name:?}; built with {}",
            store_shelf().join("|")
        )
    })?;
    let (store, backend) = build_builtin(mode).await?;
    let store = if temporal_only {
        store.temporal_only()
    } else {
        store
    };
    let maintenance = match (&store, backend) {
        (antares_sql::store::any::AnyStore::Pg(p), Some(backend)) => {
            Some((p.docs.pool().clone(), backend))
        }
        _ => None,
    };
    let store = std::sync::Arc::new(store);
    Ok(Built {
        store: store.clone(),
        temporal: store,
        maintenance,
    })
}

/// ANTARES_STORE → store construction: `file` requires ANTARES_DATA_DIR
/// (never a default inside the image); postgres and timescale require
/// ANTARES_DATABASE_URL, connect ONE shared pool, run the embedded
/// migrations at start and serve from the Pg backend.
async fn build_builtin(
    mode: antares_sql::StoreMode,
) -> Result<
    (
        antares_sql::store::any::AnyStore,
        Option<antares_sql::store::pg::maintenance::TemporalBackend>,
    ),
    Box<dyn std::error::Error>,
> {
    use antares_sql::store::any::{AnyStore, PgBackend};
    use antares_sql::store::pg::maintenance::TemporalBackend;
    use antares_sql::store::Store;
    use antares_sql::StoreMode;
    match mode {
        StoreMode::Memory => Ok((AnyStore::Mem(Store::default()), None)),
        StoreMode::File => {
            let dir = std::env::var("ANTARES_DATA_DIR").map_err(|_| {
                "ANTARES_STORE=file requires ANTARES_DATA_DIR (a mounted volume — data \
                 must never live inside the image)"
            })?;
            let dir = std::path::PathBuf::from(dir);
            warn_if_not_mount_point(&dir);
            Ok((AnyStore::Mem(Store::open_file(&dir)?), None))
        }
        StoreMode::Postgres | StoreMode::Timescale => {
            // The URL carries the database password: held as a Secret so no
            // error or log line below can print it (T-3915).
            let url = std::env::var("ANTARES_DATABASE_URL")
                .map(antares_api::secret::Secret::new)
                .map_err(|_| format!("ANTARES_STORE={mode} requires ANTARES_DATABASE_URL"))?;
            let pool_size = parse_pg_pool(std::env::var("ANTARES_PG_POOL").ok().as_deref())?;
            let write_reserve = parse_pg_write_reserve(
                std::env::var("ANTARES_PG_WRITE_RESERVE").ok().as_deref(),
                pool_size,
            )?;
            let statement_timeout = parse_pg_statement_timeout(
                std::env::var("ANTARES_PG_STATEMENT_TIMEOUT_MS")
                    .ok()
                    .as_deref(),
            )?;
            let async_batch = parse_pg_batch_async_commit(
                std::env::var("ANTARES_PG_BATCH_ASYNC_COMMIT")
                    .ok()
                    .as_deref(),
            )?;
            // T-5838: read before the pool opens, so a bad key fails the boot
            // before any connection exists; never echoed.
            let mut secret_key = match std::env::var("ANTARES_SECRET_KEY") {
                Ok(raw) if !raw.trim().is_empty() => {
                    Some(antares_sql::store::pg::seal::SecretKey::from_base64(&raw)?)
                }
                _ => None,
            };
            // The DB container may still be booting — bounded retry, then die.
            let mut last = String::new();
            for _ in 0..30 {
                match antares_sql::store::pg::connect_with(
                    url.expose(),
                    pool_size.saturating_sub(write_reserve),
                    statement_timeout,
                )
                .await
                {
                    Ok(pool) => {
                        // The temporal backend is what the migrations actually
                        // BUILT, detected once from the catalog and pinned —
                        // the maintenance branch can never disagree with the
                        // DDL on disk, whatever happened to the extension since.
                        let backend =
                            antares_sql::store::pg::maintenance::detect_temporal_backend(&pool)
                                .await
                                .map_err(|e| format!("ANTARES_STORE={mode}: {e}"))?;
                        // Never silently fall back — timescale mode whose
                        // database is not hypertable-shaped is a config error,
                        // not a downgrade (extension missing at first boot, or
                        // installed only after the migrations ran).
                        if mode == StoreMode::Timescale && backend != TemporalBackend::Hypertable {
                            return Err(format!(
                                "timescale requested (ANTARES_STORE or ANTARES_TEMPORAL) but \
                                 attr_instances is {backend:?} — the timescaledb extension was \
                                 not CREATEd when the migrations first ran. Install it in a fresh \
                                 database (CREATE EXTENSION timescaledb before first boot) or use \
                                 postgres"
                            )
                            .into());
                        }
                        if mode == StoreMode::Postgres && backend == TemporalBackend::Hypertable {
                            tracing::info!(
                                "attr_instances is a hypertable (migrations ran with \
                                 the timescaledb extension present); the plain-mode partition \
                                 job stands down, retention runs via drop_chunks"
                            );
                        }
                        // RLS is a belt only when the role wears it —
                        // superuser/BYPASSRLS makes every policy inert. Warn
                        // always; in production set ANTARES_REQUIRE_RLS=1 to turn
                        // the warning into a hard refusal so a superuser DSN can
                        // never silently ship (dev/ETSI stacks leave it unset).
                        if antares_sql::store::pg::role_bypasses_rls(&pool).await {
                            // A gate that only understands two spellings
                            // fails OPEN on `TRUE`/`yes`/`on`: the operator
                            // believes RLS is enforced and the broker serves
                            // with a BYPASSRLS role. Anything but an explicit
                            // off value turns it on.
                            let strict =
                                std::env::var("ANTARES_REQUIRE_RLS").is_ok_and(|v| !is_off(&v));
                            if strict {
                                return Err(
                                    "ANTARES_REQUIRE_RLS=1 but the database role bypasses \
                                     row-level security (superuser or BYPASSRLS) — connect as a \
                                     non-superuser, non-BYPASSRLS role so the RLS tenant-isolation \
                                     backstop is enforced"
                                        .into(),
                                );
                            }
                            tracing::warn!(
                                "database role bypasses row-level security (superuser or \
                                 BYPASSRLS) — tenant isolation rests on the explicit \
                                 predicates only; use a non-superuser role in production \
                                 (set ANTARES_REQUIRE_RLS=1 to enforce)"
                            );
                        }
                        tracing::info!(
                            "ANTARES_STORE={mode}: pool up, migrations applied, serving \
                             from postgres (temporal backend: {backend:?})"
                        );
                        // Read once, here: /q/health is polled, so what it
                        // says about the server is captured at startup and
                        // never queried on the request.
                        let mut version = antares_sql::store::pg::version_info(&pool).await;
                        // the operator sized ANTARES_PG_POOL as a whole: the
                        // reads' pool holds poolSize - poolWriteReserve
                        if let Some(m) = version.as_object_mut() {
                            m.insert("poolSize".into(), pool_size.into());
                            m.insert("poolWriteReserve".into(), write_reserve.into());
                        }
                        let mut backend_pg = PgBackend::new(pool).with_version(version);
                        if write_reserve > 0 {
                            let reserve = antares_sql::store::pg::connect_reserve(
                                url.expose(),
                                write_reserve,
                                statement_timeout,
                            )
                            .await
                            .map_err(|e| {
                                format!("ANTARES_PG_WRITE_RESERVE: reserve pool not opened: {e}")
                            })?;
                            tracing::info!(
                                "ANTARES_PG_WRITE_RESERVE: {write_reserve} of {pool_size} \
                                 connections are kept for batch writes"
                            );
                            backend_pg = backend_pg.with_write_reserve(reserve);
                        }
                        let sealing = secret_key.is_some();
                        if let Some(key) = secret_key.take() {
                            backend_pg = backend_pg.with_secret_key(key);
                        }
                        let store = AnyStore::Pg(backend_pg);
                        if sealing {
                            let sealed = store.seal_plaintext_credentials().await.map_err(|e| {
                                format!(
                                    "ANTARES_SECRET_KEY: sealing stored receiverInfo failed: {e}"
                                )
                            })?;
                            tracing::info!(
                                "ANTARES_SECRET_KEY: receiverInfo is sealed at rest \
                                 ({sealed} row(s) written before the key sealed now)"
                            );
                        } else {
                            tracing::warn!(
                                "ANTARES_SECRET_KEY is not set: Subscription receiverInfo \
                                 (notification credentials) is stored in plaintext"
                            );
                        }
                        if async_batch {
                            tracing::info!(
                                "ANTARES_PG_BATCH_ASYNC_COMMIT: batch writes commit with \
                                 synchronous_commit = off"
                            );
                        }
                        store.set_async_batch_commit(async_batch);
                        return Ok((store, Some(backend)));
                    }
                    Err(e) if antares_sql::store::pg::is_schema_mismatch(&e) => {
                        // Waiting cannot make this database match: its
                        // migration history was written by a different
                        // release. Retried, the boot ends 30 s later blaming
                        // the network for a schema that will never fit.
                        return Err(format!(
                            "ANTARES_STORE={mode}: {e} — the database was migrated by a \
                             different release of the broker, so this binary cannot serve \
                             it. Point it at a database migrated by this release, or start \
                             from an empty one"
                        )
                        .into());
                    }
                    Err(e) => {
                        last = e.to_string();
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
            Err(format!("ANTARES_STORE={mode}: database not reachable after 30 s: {last}").into())
        }
    }
}

/// Warn when the data dir shares a device with its parent — i.e. it is
/// not a mount point, so the redb file dies with the container.
#[cfg(unix)]
fn warn_if_not_mount_point(dir: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let _ = std::fs::create_dir_all(dir);
    if let (Ok(md), Some(Ok(parent_md))) =
        (std::fs::metadata(dir), dir.parent().map(std::fs::metadata))
    {
        if md.dev() == parent_md.dev() {
            eprintln!(
                "WARN: ANTARES_DATA_DIR {} is not a mount point — data will be lost when \
                 the container is removed",
                dir.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_not_mount_point(_dir: &std::path::Path) {}

// Every parameter is one config value `main` parsed fatally before the
// runtime started; a struct would only rename the same ten values.
#[allow(clippy::too_many_arguments)]
async fn run(
    port: u16,
    host_alias: String,
    roles: String,
    store: std::sync::Arc<dyn antares_store::CurrentStateDriver>,
    temporal: std::sync::Arc<dyn antares_store::TemporalDriver>,
    store_name: String,
    temporal_name: Option<String>,
    maintenance: Vec<(
        antares_sql::sqlx::PgPool,
        antares_sql::store::pg::maintenance::TemporalBackend,
    )>,
    metrics_render: Option<telemetry::MetricsRender>,
    metrics_port: Option<u16>,
    sweep_secs: u64,
    drain_delay: std::time::Duration,
    drain_deadline: std::time::Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let roles = wiring::Roles::parse(&roles).map_err(|e| format!("ANTARES_ROLES: {e}"))?;
    // Bus seam: local (default) or nats. An unknown value is fatal.
    let bus_mode = std::env::var("ANTARES_BUS").unwrap_or_else(|_| "local".into());
    match bus_mode.as_str() {
        "local" => {
            // bus=local means ONE process running every role — a role
            // split without a shared bus would silently drop whole concerns.
            if !roles.all() {
                return Err(
                    "ANTARES_BUS=local requires all roles in one process (ANTARES_ROLES=all); \
                     role splits need ANTARES_BUS=nats"
                        .into(),
                );
            }
        }
        "nats" => {
            if !shared_state(&store_name) {
                return Err(format!(
                    "ANTARES_BUS=nats requires a shared store (ANTARES_STORE=postgres|timescale); \
                     {store_name} state is per-process and cannot back multiple instances"
                )
                .into());
            }
        }
        other => return Err(format!("unknown ANTARES_BUS={other} (local|nats)").into()),
    }
    tracing::info!(port, store = %store_name, %bus_mode, ?roles, "starting antares");

    // Trailing-slash tolerance: Table 6.2-1 spells collection resources with a
    // trailing '/'; normalize before routing.
    // T-5895: one process-wide memory budget, split over every cache before
    // the first of them is built and over the buffered-answer pool
    let budget = match std::env::var("ANTARES_MEMORY_BUDGET") {
        Ok(v) => antares_api::bounds::MemoryBudget::parse(&v)?,
        Err(_) => antares_api::bounds::MemoryBudget::default(),
    };
    budget.apply();
    #[cfg(feature = "odata")]
    antares_odata::set_model_cache_bytes(budget.odata_models(), budget.linkml_models());
    #[cfg(feature = "linkml")]
    antares_linkml::set_cache_bytes(
        usize::try_from(budget.linkml_renders()).unwrap_or(usize::MAX),
        budget.linkml_models(),
    );
    tracing::info!(bytes = budget.total, "memory budget");
    let mut state = AppState::with_drivers(host_alias, store, temporal, &store_name);
    budget.set_on(&mut state);
    state.temporal_name = temporal_name;
    state.delivery = antares_api::DeliveryPolicy::from_env().unwrap_or_default();
    state.temporal_record = std::env::var("ANTARES_TEMPORAL_RECORD")
        .as_deref()
        .unwrap_or("all")
        .parse()?;
    state.profile = antares_api::Profile::from_env()?;
    #[cfg(feature = "jc")]
    {
        state.provenance_from = std::env::var("ANTARES_PROVENANCE_FROM")
            .ok()
            .filter(|g| !g.trim().is_empty());
        // T-5032: a deployment that names who calls serves a caller
        // without a subject only with the admin bearer token.
        state.subject_required = !antares_api::policy::SUBJECT_HEADERS.is_empty();
    }
    // The surfaces mounted beside the NGSI-LD API root, from configuration
    // rather than from a hard-wired list: the selection replaces what the
    // default mounting put there, before the state is shared.
    let selected = api_surfaces(std::env::var("ANTARES_API_SURFACES").ok().as_deref())?;
    #[cfg(feature = "channels")]
    if selected.iter().any(|(n, _)| *n == "channels") {
        antares_api::channels::bridges_from(
            &std::env::var("ANTARES_CHANNEL_BROKERS").unwrap_or_default(),
        )?;
    }
    state = state.with_surfaces(selected.iter().map(|(_, build)| build()).collect())?;
    // The policy engine, from configuration and from the shelf this binary
    // was built with. Without one the broker asks `allow-all` and behaves
    // exactly as it did before the seam existed.
    let (policy_name, build_policy) =
        policy_engine(std::env::var("ANTARES_POLICY").ok().as_deref())?;
    tracing::info!("policy engine: {policy_name}");
    // The built-in engine is attached as no engine at all: it decides
    // nothing, and a gate with nothing to ask skips the whole apparatus
    // rather than boxing a future that always answers allow.
    if policy_name != antares_api::policy::BUILT_IN_NAME {
        state = state.with_policy(build_policy());
    }
    // A notification binding compiled in from outside the workspace mounts
    // exactly like the two shipped ones: one registration, no core-crate
    // edit. It is not in a release build — the feature is off by default,
    // so the shipped binary serves network schemes only. Every sink before
    // `views::install`: the registry is sealed once the state is shared.
    #[cfg(feature = "plugin-example")]
    {
        state = state.with_sink(Box::new(antares_plugin_example::MemorySink::new()));
    }
    // The WebSocket channel (ADR-0026): its sink beside HTTP and MQTT, over
    // the hub its two surfaces share. A mistyped bound stops the broker.
    #[cfg(feature = "ws")]
    {
        antares_ws::Limits::from_env(|k| std::env::var(k).ok())?;
        state = state.with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())));
    }
    // The MCP surface's resource subscriptions (MCP section 5): an
    // in-process sink that flags a URI on a stream this process holds.
    #[cfg(feature = "mcp")]
    {
        state = state.with_sink(Box::new(antares_mcp::Sink));
    }
    // Endpoint views (ADR-0023 rule 10): the stored views loaded into the
    // table every surface that serves through views opens on, `/q/views`
    // and `/q/policy` on the admin surface, and the reload that keeps every
    // pod's table within one interval of the store. After the engine: the
    // reload hands it its rule documents (ADR-0020 amendment).
    #[cfg(feature = "views")]
    antares_ops::views::install(&state).await?;
    // The MCP write decisions a deployment's decision page records
    // (`/q/mcp-confirmations/{id}`, T-5816), behind the admin credential.
    #[cfg(feature = "mcp")]
    state.views.add_admin(antares_mcp::confirmations());
    // Internal context sources (ADR-0025): a registration whose endpoint is
    // a self URL is served in process under the target's export grant, and
    // `/q/grants` joins the admin surface. No self URL, no transport: every
    // forward stays HTTP. An entry that is no self URL stops the broker.
    #[cfg(feature = "internal-csr")]
    {
        let selfs = antares_icsr::SelfUrls::parse(
            &std::env::var(antares_icsr::SELF_URLS_ENV).unwrap_or_default(),
        )?;
        if !selfs.is_empty() {
            state = antares_ops::with_forward_transport(state, antares_icsr::transport(selfs))?;
        }
    }
    #[cfg(not(feature = "internal-csr"))]
    if std::env::var_os("ANTARES_SELF_URLS").is_some() {
        tracing::warn!(
            "ANTARES_SELF_URLS is set and this build has no internal-csr feature: \
             every federation forward goes over HTTP"
        );
    }
    // The /q admin credential, before the router mounts the admin surface.
    // Unconfigured, /q serves as before and says so once, here.
    match antares_api::admin_auth::AdminAuth::from_config(
        std::env::var("ANTARES_ADMIN_TOKEN_FILE").ok().as_deref(),
        std::env::var("ANTARES_ADMIN_METRICS").ok().as_deref(),
    )? {
        Some(auth) => {
            tracing::info!("/q admin routes require the admin token ({auth:?})");
            state = state.with_admin_auth(auth);
        }
        None => tracing::warn!(
            "/q admin routes (tenants, dead letters, metrics) answer without a credential: \
             ANTARES_ADMIN_TOKEN_FILE is unset"
        ),
    }
    // /q/metrics renders through this closure (None without the
    // `telemetry` feature — the endpoint answers 404); the sampler feeds
    // the process-level gauges the whole run.
    state.metrics_render = metrics_render;
    // Heap stats on /q/health (allocated/resident bytes via jemalloc-ctl)
    state.mem_stats = Some(std::sync::Arc::new(|| {
        use tikv_jemalloc_ctl::{epoch, stats};
        let _ = epoch::advance();
        serde_json::json!({
            "allocatedBytes": stats::allocated::read().unwrap_or(0),
            "residentBytes": stats::resident::read().unwrap_or(0),
        })
    }));
    // 5.8.1.4 consumer half, whichever bus carries the delivery. A
    // distributed Subscription is served by an internal Context Source
    // Registration Subscription that notifies to `urn:antares:distsub:…`,
    // and the delivery path drops that notification unless this handler is
    // installed — the Subscription is accepted and no copy is ever
    // forwarded. `antares_api::wire` installs it with the in-process
    // matcher; the role-split fleet wires its matcher through `wire_nats`
    // and needs it installed here.
    antares_api::install_csource_notification(&mut state);
    if bus_mode == "nats" {
        // Outbox producer + drain, KV/registry mirrors, durable
        // consumers per role, topology asserted before traffic.
        let url = std::env::var("ANTARES_NATS_URL")
            .map_err(|_| "ANTARES_BUS=nats requires ANTARES_NATS_URL")?;
        let bus = wiring::wire_nats(&mut state, &url, roles).await?;
        // ADR-0026 rule 5: a notifier pod reaches the socket another pod
        // holds over core NATS request/reply.
        #[cfg(feature = "ws")]
        antares_ws::Hub::process().set_relay(std::sync::Arc::new(ws_relay::NatsRelay::new(bus)));
        #[cfg(not(feature = "ws"))]
        drop(bus);
    } else {
        antares_api::wire(&mut state).await; // in-process matcher + notifier + interval firing
    }
    telemetry::spawn_sampler(state.clone());

    // Boot preload — Cached rows persisted by the AppState write-through
    // re-seed the parsed-context cache on start, so expansion doesn't refetch
    // what a previous life already downloaded. (The writer itself is wired in
    // AppState::with_store — rows are the 5.13 source of truth.)
    {
        // Metadata first, then ONE body at a time. Reading every row whole
        // put up to MAX_CONTEXT_BYTES per row in memory at once, capped only
        // for Cached rows and not at all for the rest — a boot that a client
        // could make impossible by storing large @contexts.
        // No Tenant: the store then hands back the rows that belong to none
        // (ADR-0021), which is exactly the `Cached` set this warm wants.
        for row in state
            .store
            .context_list_meta(None)
            .await
            .unwrap_or_default()
        {
            if row.get("kind").and_then(|v| v.as_str()) != Some("Cached") {
                continue;
            }
            let (Some(url), Some(id), Some(created)) = (
                row.get("url").and_then(|v| v.as_str()),
                row.get("localId").and_then(|v| v.as_str()),
                row.get("createdAt").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let Some(full) = state.store.context_get(None, id).await.ok().flatten() else {
                continue;
            };
            if let Some(v) = full.pointer("/body/@context") {
                state.loader.seed_cached(url, id, created, v.clone()).await;
            }
        }
    }

    // 4.22 GC on the memory/file arm: reads already refuse expired entities;
    // this reaps them (spec-sanctioned lag). The Pg arm's sweep runs inside
    // the maintenance job below — one job per backend, mode-switched.
    // ANTARES_SWEEP_SECS paces 4.22 GC identically across ALL backends —
    // this loop and the Pg/Timescale maintenance job below both tick on it
    // (the ETSI stack runs at 2 s so transient TPs observe GC, not just the
    // read filter); parsed at startup, default 15 min. The loop runs for
    // every driver and asks it what it reaped: a backend whose GC lives
    // elsewhere — the Pg arm's is inside the maintenance job — answers 0.
    let sweeper = state.store.clone();
    let doc_sweeper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(sweep_secs));
        loop {
            tick.tick().await;
            let n = sweeper.sweep_expired().await;
            if n > 0 {
                tracing::debug!("4.22 sweep reaped {n} expired entities");
            }
            // Registrations, Snapshots and EntityMaps carry their own
            // expiry, and every read already refuses one that has passed;
            // this is what frees the row behind it.
            let n = antares_api::sweep_expired_docs(&doc_sweeper).await;
            if n > 0 {
                tracing::debug!("expiry sweep reaped {n} expired documents");
            }
        }
    });
    // Temporal maintenance — plain-mode partition pre-creation and the
    // (opt-in) retention horizon, single-winner via SKIP LOCKED.
    // One job per pg half (current state and/or history), PINNED to the
    // backend detected at startup; memory and file halves get none.
    if !maintenance.is_empty() {
        let retention: Option<i64> = std::env::var("ANTARES_TEMPORAL_RETENTION_DAYS")
            .ok()
            .map(|v| {
                v.parse::<i64>()
                    .map_err(|_| "ANTARES_TEMPORAL_RETENTION_DAYS must be an integer")
                    // A zero/negative horizon inverts `now() - make_interval(days)`
                    // and would reap all current + future history — a data-loss
                    // footgun. Retention is opt-in; a bad value must not silently
                    // delete. Cap at i32 too (bound as $1::int downstream).
                    .and_then(|d| {
                        (d > 0 && d <= i64::from(i32::MAX)).then_some(d).ok_or(
                            "ANTARES_TEMPORAL_RETENTION_DAYS must be between 1 and 2147483647",
                        )
                    })
            })
            .transpose()?;
        for (pool, backend) in maintenance {
            tokio::spawn(async move {
                // Same ANTARES_SWEEP_SECS cadence as the Mem arm — the job's 4.22
                // reap is the sweep here; the partition/retention steps riding on
                // the same tick are idempotent and SKIP LOCKED single-winner.
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(sweep_secs));
                loop {
                    tick.tick().await; // first tick is immediate: partitions at boot
                    match antares_sql::store::pg::maintenance::temporal_maintenance(
                        &pool, backend, retention,
                    )
                    .await
                    {
                        Ok(msg) => tracing::debug!("temporal maintenance: {msg}"),
                        Err(e) => tracing::warn!("temporal maintenance failed: {e}"),
                    }
                }
            });
        }
    }
    // Handles the drain needs, taken before `state` is consumed by the
    // router — the flag the health endpoint reads, and the store whose pools
    // close last.
    let draining = state.draining.clone();
    let store_for_drain = state.store.clone();
    // The temporal seam may be a second store with its own pool; the drain
    // closes both.
    let temporal_for_drain = state.temporal.clone();
    let pending_for_drain = state.pending_changes.clone();
    // Only the api role serves the NGSI-LD surface — a worker pod
    // exposes health/ready/metrics and nothing else (a subscription created
    // on a worker would bypass the roles.api KV sync and never notify).
    // ANTARES_METRICS_PORT: the renderer moves to the metrics listener, so
    // the API port's /q/metrics answers 404 and a scraper needs no path to
    // the API (T-5095).
    let metrics_app = metrics_port.map(|p| {
        let app = antares_api::Admin::metrics_router(state.clone());
        state.metrics_render = None;
        (p, app)
    });
    let routed = if roles.api {
        antares_api::router(state)
    } else {
        antares_api::ops_router(state)
    }
    .layer(antares_api::compression());
    let routed = match subject_from()? {
        Some(from) => {
            tracing::info!(
                ?from,
                "policy subject headers believed from these mesh identities alone"
            );
            routed.layer(tower::util::MapRequestLayer::new(
                move |mut req: axum::extract::Request| {
                    antares_api::policy::drop_subject_unless_from(
                        req.headers_mut(),
                        &antares_api::policy::SUBJECT_HEADERS,
                        &from,
                    );
                    req
                },
            ))
        }
        None => routed,
    };
    let app: App = tower::util::MapRequest::new(routed, antares_api::trim_trailing_slash);

    // A connection that never finishes its request headers must not hold a
    // slot forever; hyper closes it after this timeout.
    let header_read_timeout = std::env::var("ANTARES_HEADER_READ_TIMEOUT_MS")
        .ok()
        .map(|v| {
            v.parse::<u64>().map_err(|_| {
                format!("ANTARES_HEADER_READ_TIMEOUT_MS must be an integer (got {v:?})")
            })
        })
        .transpose()?
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_secs(10));
    // Ceiling on concurrently served connections: accepted streams beyond
    // it are dropped at once — refusing cheaply beats queueing work the box
    // cannot serve (each served connection is a spawned task).
    let max_connections = max_connections()?;
    let conn_permits = std::sync::Arc::new(tokio::sync::Semaphore::new(max_connections));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("listening on http://0.0.0.0:{port}");
    if let Some((p, app)) = metrics_app {
        let metrics_listener = tokio::net::TcpListener::bind(("0.0.0.0", p)).await?;
        tracing::info!("serving /q/metrics only on http://0.0.0.0:{p}");
        tokio::spawn(async move {
            if let Err(e) = axum::serve(metrics_listener, app).await {
                tracing::error!(error = %e, "metrics listener stopped");
            }
        });
    }
    // Count open connections so the drain can wait for them. Incremented
    // before the task is spawned — incrementing inside the task would race the
    // drain's first check and let a just-accepted connection be missed.
    // This counts CONNECTIONS, not requests, and stays that way on purpose:
    // hyper's graceful_shutdown below draws the distinction already (an idle
    // keep-alive closes at once, an active request finishes first), so a
    // request-layer counter would add a middleware without changing what the
    // drain waits for.
    let inflight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // The drain signal each connection listens for. On drain,
    // hyper's graceful_shutdown closes IDLE keep-alive connections immediately
    // (the LB holds one per backend — counting them as in-flight made every
    // api roll burn the full deadline) while an active request still finishes.
    let (drain_tx, drain_rx) = tokio::sync::watch::channel(false);
    // The signal future is created ONCE and polled by reference. Written
    // inline in the select, it would be dropped and re-created on every
    // accepted connection — and a SIGTERM landing in that drop-to-recreate
    // window is lost for good (tokio signal streams do not replay events from
    // before their creation). Under health-check polling that window is hit
    // constantly, which is exactly how the drain test caught it.
    let mut sigterm = std::pin::pin!(shutdown::signal());
    // A pod whose drain is switched off publishes nothing, so waiting for the
    // outbox to empty there would only burn the deadline.
    let flush_outbox = wiring::outbox_drain_enabled()?;
    // Manual serve loop: the ETSI suite reads response headers case-sensitively
    // ("Location"), so HTTP/1 responses are written with title-case headers.
    loop {
        let stream = tokio::select! {
            s = accept(&listener) => s,
            _ = &mut sigterm => {
                // 1+2: unhealthy FIRST, then keep serving for the LB's notice
                // window — still inside this select, so connections arriving
                // during it are accepted normally.
                shutdown::begin(&draining, drain_delay);
                let until = tokio::time::Instant::now() + drain_delay;
                loop {
                    tokio::select! {
                        stream = accept(&listener) => {
                            serve(stream, app.clone(), inflight.clone(), drain_rx.clone(),
                                  conn_permits.clone(), header_read_timeout);
                        }
                        _ = tokio::time::sleep_until(until) => break,
                    }
                }
                // 3–6: listener dropped, idle conns told to close (active
                // requests finish), in-flight drained, pools closed.
                drop(listener);
                let _ = drain_tx.send(true);
                shutdown::drain(
                    &inflight,
                    &pending_for_drain,
                    &*store_for_drain,
                    &*temporal_for_drain,
                    drain_deadline,
                    flush_outbox,
                )
                .await;
                tracing::info!("shutting down");
                return Ok(());
            }
        };
        serve(
            stream,
            app.clone(),
            inflight.clone(),
            drain_rx.clone(),
            conn_permits.clone(),
            header_read_timeout,
        );
    }
}

/// ANTARES_MAX_CONNECTIONS: the ceiling on concurrently served connections
/// (default 10 000).
fn max_connections() -> Result<usize, Box<dyn std::error::Error>> {
    Ok(std::env::var("ANTARES_MAX_CONNECTIONS")
        .ok()
        .map(|v| {
            v.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                format!("ANTARES_MAX_CONNECTIONS must be a positive integer (got {v:?})")
            })
        })
        .transpose()?
        .unwrap_or(10_000))
}

/// The request runtime. Store calls are futures the workers poll, so a
/// waiting caller holds no thread and the blocking pool carries only what
/// is genuinely blocking — the `file` mode's per-commit fsync, which redb
/// serializes behind its single writer. Tokio's own bounds hold.
fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

/// Accept the next connection. There is no failure value: `accept()`
/// propagates every non-`WouldBlock` errno from the syscall, and an
/// `ECONNABORTED` (a client resetting between SYN and accept), `EMFILE`/
/// `ENFILE` (the fd ceiling — the connection cap defaults above many
/// containers' `nofile`) or `ENOBUFS` must never take the broker down.
async fn accept(listener: &tokio::net::TcpListener) -> tokio::net::TcpStream {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                // Nagle held every multi-segment response for the peer's
                // delayed ACK: a fixed ~40 ms on any body over one segment,
                // 25 req/s per connection whatever the core count.
                let _ = stream.set_nodelay(true);
                return stream;
            }
            Err(e) => {
                tracing::warn!("accept failed, retrying: {e}");
                if accept_backoff(&e) {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// Whether an `accept()` error warrants a pause before the next attempt.
/// The per-connection failures retry at once (the next connection is
/// unaffected); a resource exhaustion would otherwise spin the loop at full
/// speed until the pressure clears. Neither is fatal.
fn accept_backoff(e: &std::io::Error) -> bool {
    !matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    )
}

/// `ANTARES_POLICY_SUBJECT_FROM`, comma-separated mesh identities; `None`
/// when unset, which believes every caller's subject headers. Set but naming
/// nobody is fatal: an operator who set it asked for deny by default, and a
/// blank value must not fall back to believing everyone.
fn subject_from() -> Result<Option<Vec<String>>, String> {
    let Ok(raw) = std::env::var("ANTARES_POLICY_SUBJECT_FROM") else {
        return Ok(None);
    };
    parse_subject_from(&raw).map(Some)
}

/// The identities of a set `ANTARES_POLICY_SUBJECT_FROM`.
fn parse_subject_from(raw: &str) -> Result<Vec<String>, String> {
    let from: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if from.is_empty() {
        return Err("ANTARES_POLICY_SUBJECT_FROM is set but names no mesh identity".to_owned());
    }
    Ok(from)
}

/// The served app: the router under trailing-slash normalization, which
/// leaves a view's OData paths as sent (T-3731).
type App =
    tower::util::MapRequest<axum::Router, fn(axum::extract::Request) -> axum::extract::Request>;

/// One accepted connection. Split out of the accept loop so the drain's
/// notice window serves connections with identical behaviour, and so the
/// in-flight counter is incremented in exactly one place.
fn serve(
    stream: tokio::net::TcpStream,
    app: App,
    inflight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    mut drain_rx: tokio::sync::watch::Receiver<bool>,
    conn_permits: std::sync::Arc<tokio::sync::Semaphore>,
    header_read_timeout: std::time::Duration,
) {
    // Over the connection cap: answer 503 instead of serving (`refuse`).
    // The permit rides in the connection task, so the slot frees exactly
    // when the connection ends — and never enters the inflight drain
    // accounting.
    let Ok(permit) = conn_permits.try_acquire_owned() else {
        refuse(stream, header_read_timeout);
        return;
    };
    inflight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tokio::spawn(async move {
        let _permit = permit;
        let svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
            let mut app = app.clone();
            async move { tower::Service::call(&mut app, req.map(axum::body::Body::new)).await }
        });
        // HTTP/1.1 only (hyper's http1 builder; the auto builder detects
        // h2c even with http1_only once upgrades are on): the header read
        // timeout and the one permit per connection are HTTP/1 bounds, and
        // no client of the broker speaks h2c. An HTTP/2 connection would
        // carry up to 200 streams under one permit with no header deadline.
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(header_read_timeout)
            .title_case_headers(true);
        // With upgrades: a WebSocket surface answers 101 and takes the
        // stream (ADR-0026); without an upgrading route nothing changes.
        let conn = builder
            .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
            .with_upgrades();
        let mut conn = std::pin::pin!(conn);
        // On drain, close an IDLE keep-alive connection immediately —
        // hyper finishes any active request first, then closes. Without this
        // the LB's idle keep-alives count as in-flight and every roll waits
        // out the entire drain deadline.
        tokio::select! {
            r = conn.as_mut() => { let _ = r; }
            _ = wait_drain(&mut drain_rx) => {
                conn.as_mut().graceful_shutdown();
                let _ = conn.as_mut().await;
            }
        }
        inflight.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    });
}

/// How many over-cap connections are answered at once. Past it the stream is
/// dropped as before, so a flood costs at most this many small tasks.
const REFUSE_AT_ONCE: usize = 256;
static REFUSING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(REFUSE_AT_ONCE);

/// A connection over ANTARES_MAX_CONNECTIONS reads `503` with
/// `Retry-After: 1` and `Connection: close` instead of a bare reset, so the
/// client can back off (T-6614, R1-030). Bounded twice: [`REFUSE_AT_ONCE`]
/// refusals in flight, each cut after five seconds; past that the stream is
/// dropped, the old behaviour.
fn refuse(stream: tokio::net::TcpStream, header_read_timeout: std::time::Duration) {
    let Ok(permit) = REFUSING.try_acquire() else {
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        let svc = hyper::service::service_fn(|_req: hyper::Request<hyper::body::Incoming>| async {
            let mut res = hyper::Response::new(axum::body::Body::empty());
            *res.status_mut() = hyper::StatusCode::SERVICE_UNAVAILABLE;
            res.headers_mut().insert(
                hyper::header::RETRY_AFTER,
                hyper::header::HeaderValue::from_static("1"),
            );
            Ok::<_, std::convert::Infallible>(res)
        });
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(header_read_timeout)
            .keep_alive(false);
        let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), conn).await;
    });
}

/// Resolves when the drain signal fires; pends forever once the sender is
/// gone (the connection future then completes on its own in the select).
async fn wait_drain(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod config_key_tests {
    use super::unknown_config_key;

    /// Kubelet-injected service links must never be fatal; real typos
    /// must stay fatal.
    #[test]
    fn kubelet_service_links_are_not_typos() {
        for k in [
            "ANTARES_PORT",
            "ANTARES_PORT_9090_TCP",
            "ANTARES_PORT_9090_TCP_ADDR",
            "ANTARES_SERVICE_HOST",
            "ANTARES_SERVICE_PORT",
            "ANTARES_API_SERVICE_HOST",
            "ANTARES_FILE_PORT_9090_TCP_PROTO",
        ] {
            assert!(!unknown_config_key(k), "{k} is platform-injected");
        }
        for k in [
            "ANTARES_HTTP_PORT",
            "ANTARES_STORE",
            "ANTARES_TEST_ANYTHING",
            "ANTARES_PG_POOL",
            "ANTARES_ALLOW_SHARED_LOCAL",
            "ANTARES_HEADER_READ_TIMEOUT_MS",
            "ANTARES_MAX_CONNECTIONS",
        ] {
            assert!(!unknown_config_key(k), "{k} is known/reserved");
        }
        assert!(
            unknown_config_key("ANTARES_STROE"),
            "a real typo stays fatal"
        );
        assert!(
            unknown_config_key("ANTARES_HTPT_PORT"),
            "a typo'd *_PORT var is NOT a service link"
        );
        assert!(unknown_config_key("ANTARES_BOGUS_FLAG"));
    }

    /// The exemption is narrow on purpose: only the exact kubelet shapes for
    /// the Services this repo ships. Near-misses stay fatal, and a non-ANTARES
    /// variable is never our business.
    #[test]
    fn the_service_link_exemption_does_not_over_reach() {
        for k in [
            "ANTARES_",
            "ANTARES_PORTAL",
            "ANTARES_SERVICEHOST",
            "ANTARES_DB_PORT",
            "ANTARES_WORKER_PROT",
        ] {
            assert!(unknown_config_key(k), "{k} must stay a fatal typo");
        }
        for k in ["PATH", "HOME", "antares_store", "ANTARE_STORE", ""] {
            assert!(!unknown_config_key(k), "{k} is not broker config");
        }
    }

    /// Unknown keys are FATAL, so every ANTARES_* variable the workspace
    /// actually reads has to be accepted — otherwise setting a documented
    /// deployment knob is a CrashLoopBackOff and the knob cannot be used at
    /// all. Scans the sources rather than restating the list, so the check
    /// keeps holding as crates add knobs.
    /// The MCP surface names its variables by constant, which the source
    /// scan below cannot see; a key missing here stops the broker at start.
    #[cfg(feature = "mcp")]
    #[test]
    fn known_keys_hold_the_mcp_surface_variables() {
        for key in [
            antares_mcp::ALLOWED_HOSTS_ENV,
            antares_mcp::ALLOWED_ORIGINS_ENV,
            antares_mcp::CONFIRM_URL_ENV,
            antares_mcp::MAX_RESULT_BYTES_ENV,
            antares_mcp::REQUEST_STATE_KEY_FILE_ENV,
            antares_mcp::SUBSCRIPTION_ENDPOINTS_ENV,
        ] {
            assert!(!unknown_config_key(key), "{key}");
        }
    }

    #[test]
    fn known_keys_cover_every_variable_the_workspace_reads() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .to_path_buf();
        let needles = [
            concat!("var(", "\"ANTARES_"),
            concat!("var_os(", "\"ANTARES_"),
        ];
        let mut missing: Vec<String> = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap_or_default();
                for needle in needles {
                    for (i, _) in src.match_indices(needle) {
                        let rest = &src[i + needle.len() - "\"ANTARES_".len() + 1..];
                        let Some(end) = rest.find('"') else { continue };
                        let key = &rest[..end];
                        if unknown_config_key(key) {
                            missing.push(format!("{key} (read in {})", path.display()));
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "KNOWN_KEYS is missing variables the workspace reads: {missing:#?}"
        );
    }

    /// The test above finds a key a crate reads as a literal; a surface reads
    /// through a `*_ENV` const instead, which the broker accepts only when
    /// the crate lists it in its `ENV_KEYS`. With every surface built, each
    /// such const under `crates/` has to be a key the broker knows.
    #[cfg(feature = "full")]
    #[test]
    fn every_env_const_of_a_surface_crate_is_a_known_key() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .to_path_buf();
        let mut declared = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap_or_default();
                    for line in src.lines() {
                        let Some(rest) = line.trim_start().strip_prefix("pub const ") else {
                            continue;
                        };
                        let Some((name, value)) = rest.split_once(": &str = \"") else {
                            continue;
                        };
                        if name.ends_with("_ENV") {
                            let key = value.trim_end_matches("\";").to_owned();
                            declared.push(format!("{key} ({name} in {})", path.display()));
                            if key.starts_with("ANTARES_") {
                                assert!(
                                    super::known_keys().any(|k| k == key),
                                    "{key} ({name} in {}) is no ENV_KEYS entry",
                                    path.display()
                                );
                            }
                        }
                    }
                }
            }
        }
        // the scan itself must find the surface consts it guards
        assert!(declared.len() >= 14, "found only {declared:#?}");
    }
}

#[cfg(test)]
mod accept_loop_tests {
    use super::{accept, accept_backoff};

    /// An accept() error must never end the serve loop: tokio propagates
    /// every non-WouldBlock errno straight from the syscall, so ECONNABORTED
    /// (a client resetting between SYN and accept), EMFILE/ENFILE (the fd
    /// ceiling) and ENOBUFS all used to take the whole broker down. `accept`
    /// has no failure value at all — the only decision left is whether to
    /// pause before retrying.
    #[test]
    fn no_accept_error_is_fatal_and_resource_errors_back_off() {
        use std::io::ErrorKind::*;
        for kind in [ConnectionAborted, ConnectionReset, Interrupted] {
            assert!(
                !accept_backoff(&std::io::Error::new(kind, "x")),
                "{kind:?} is per-connection — the next accept must run at once"
            );
        }
        for kind in [
            Other,
            OutOfMemory,
            PermissionDenied,
            InvalidInput,
            NotConnected,
        ] {
            assert!(
                accept_backoff(&std::io::Error::new(kind, "x")),
                "{kind:?} would spin the loop without a pause"
            );
        }
    }

    /// T-6614 (R1-030): a connection over the cap reads a 503 with
    /// Retry-After before the close, not a bare reset.
    #[tokio::test]
    async fn a_connection_over_the_cap_reads_503() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let client = tokio::spawn(async move {
            let mut c = tokio::net::TcpStream::connect(addr).await.expect("connect");
            c.write_all(b"GET /ngsi-ld/v1/entities HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("write");
            let mut answer = String::new();
            let _ = c.read_to_string(&mut answer).await;
            answer
        });
        let (stream, _) = listener.accept().await.expect("accept");
        super::refuse(stream, std::time::Duration::from_secs(2));
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), client)
            .await
            .expect("answered in time")
            .expect("client task");
        assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
        assert!(
            answer.to_ascii_lowercase().contains("retry-after: 1"),
            "{answer}"
        );
    }

    /// …and the healthy path still hands the loop its connection.
    #[tokio::test]
    async fn accept_yields_the_next_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let client = tokio::spawn(async move { tokio::net::TcpStream::connect(addr).await });
        let stream = tokio::time::timeout(std::time::Duration::from_secs(5), accept(&listener))
            .await
            .expect("a connection is accepted");
        assert!(stream.peer_addr().is_ok());
        let _ = client.await;
    }
}

#[cfg(test)]
mod switch_tests {
    use super::is_off;

    /// The knobs whose default is OFF (ANTARES_REQUIRE_RLS, ANTARES_TELEMETRY)
    /// must fail SAFE on a spelling they do not know: recognizing only
    /// `1|true` turned the RLS gate off on `TRUE`/`yes`/`on` while the
    /// operator believed it was enforced.
    #[test]
    fn only_an_explicit_off_value_reads_as_off() {
        for off in [
            "0", "false", "FALSE", "False", "off", "OFF", "no", "", " ", " 0\t",
        ] {
            assert!(is_off(off), "{off:?} must read as off");
        }
        for on in [
            "1", "true", "TRUE", "True", "on", "On", "yes", "YES", " 1 ", "enabled",
        ] {
            assert!(!is_off(on), "{on:?} must NOT read as off");
        }
    }
}

#[cfg(test)]
mod subject_from_tests {
    use super::parse_subject_from;

    /// A set key always names someone: blank refuses to start instead of
    /// believing every caller (T-6432).
    #[test]
    fn a_set_key_names_at_least_one_identity() {
        assert_eq!(
            parse_subject_from(" a.b , c.d,").expect("two identities"),
            vec!["a.b".to_owned(), "c.d".to_owned()]
        );
        for blank in ["", " ", " , "] {
            assert!(parse_subject_from(blank).is_err(), "{blank:?}");
        }
    }
}

#[cfg(test)]
mod sweep_secs_tests {
    use super::parse_sweep_secs;

    /// ANTARES_SWEEP_SECS paces the 4.22 GC in every store mode: absent is
    /// the 15 min default, and a value that is not a positive integer is
    /// fatal — a garbage cadence must never silently become the default one.
    #[test]
    fn sweep_secs_defaults_and_rejects() {
        assert_eq!(parse_sweep_secs(None).expect("default"), 900);
        assert_eq!(parse_sweep_secs(Some("2")).expect("explicit"), 2);
        for bad in ["0", "-1", "", "2s", "abc", "1.5", "99999999999999999999999"] {
            let err =
                parse_sweep_secs(Some(bad)).expect_err(&format!("SWEEP_SECS={bad:?} is fatal"));
            assert!(err.contains("ANTARES_SWEEP_SECS"), "{err}");
        }
    }
}

#[cfg(test)]
mod metrics_port_tests {
    use super::parse_metrics_port;

    /// ANTARES_METRICS_PORT (T-5095): unset or empty is no metrics listener,
    /// and a value that is no port, port 0 or the API port is fatal.
    #[test]
    fn metrics_port_parses_and_rejects() {
        assert_eq!(parse_metrics_port(None, 9090), Ok(None));
        assert_eq!(parse_metrics_port(Some(""), 9090), Ok(None));
        assert_eq!(parse_metrics_port(Some("9464"), 9090), Ok(Some(9464)));
        for bad in ["0", "9090", "-1", "abc", "70000"] {
            let err = parse_metrics_port(Some(bad), 9090)
                .expect_err(&format!("METRICS_PORT={bad:?} is fatal"));
            assert!(err.contains("ANTARES_METRICS_PORT"), "{err}");
        }
    }
}

#[cfg(test)]
mod driver_registry_tests {
    use super::{built_with, shared_state, store_shelf, temporal_choice, TemporalChoice};
    use antares_sql::StoreMode;

    /// The shelf in the message is rendered from the backend list, so a
    /// backend added to `StoreMode` — or compiled in from outside this
    /// workspace — cannot go missing from what the broker claims to have
    /// been built with.
    #[test]
    fn the_shelf_names_every_store_mode() {
        let shelf = built_with();
        for m in StoreMode::ALL {
            assert!(shelf.contains(m.as_str()), "{shelf} omits {m}");
        }
        assert!(
            shelf.contains("none"),
            "temporal `none` is part of the shelf: {shelf}"
        );
        for name in store_shelf() {
            assert!(shelf.contains(name), "{shelf} omits {name}");
        }
    }

    /// Absent, or the store's own name, means one instance serves both
    /// seams — no second store is ever built for the default.
    #[test]
    fn absent_or_same_name_shares_the_store() {
        assert_eq!(
            temporal_choice("postgres", None).expect("ok"),
            TemporalChoice::SameAsStore
        );
        assert_eq!(
            temporal_choice("postgres", Some("postgres")).expect("ok"),
            TemporalChoice::SameAsStore
        );
    }

    #[test]
    fn none_turns_history_off_and_other_names_build_a_second_store() {
        assert_eq!(
            temporal_choice("memory", Some("none")).expect("ok"),
            TemporalChoice::None
        );
        assert_eq!(
            temporal_choice("memory", Some("timescale")).expect("ok"),
            TemporalChoice::Second("timescale".to_owned())
        );
    }

    /// An unknown backend is fatal at startup and the message names the
    /// shelf this binary was built with — never a silent default.
    #[test]
    fn unknown_backend_is_fatal_and_lists_the_shelf() {
        let err = temporal_choice("memory", Some("mongo")).expect_err("must fail");
        assert!(err.contains("mongo"), "{err}");
        assert!(err.contains(&built_with()), "{err}");
        assert!(
            temporal_choice("memory", Some("")).is_err(),
            "an empty name is not a default"
        );
    }

    /// Shared state is what `ANTARES_BUS=nats` needs and what `bus=local`
    /// refuses: a per-process backend — the built-in memory and file arms,
    /// and any driver from outside the workspace — is never shared.
    #[test]
    fn only_the_database_backends_hold_shared_state() {
        assert!(shared_state("postgres"));
        assert!(shared_state("timescale"));
        assert!(!shared_state("memory"));
        assert!(!shared_state("file"));
        assert!(
            !shared_state("mongo"),
            "an unknown name is not shared state"
        );
    }

    /// A driver compiled in from outside `crates/` is selected by name
    /// exactly like a built-in, and serves both storage seams.
    #[cfg(feature = "plugin-example")]
    #[tokio::test]
    async fn a_plugin_backend_is_on_the_shelf_and_builds_both_seams() {
        let name = antares_plugin_example::NAME;
        assert!(
            store_shelf().contains(&name),
            "the shelf omits the compiled-in plugin: {:?}",
            store_shelf()
        );
        assert_eq!(
            temporal_choice(name, None).expect("ok"),
            TemporalChoice::SameAsStore
        );
        let drivers = super::build_drivers(name)
            .await
            .expect("the plugin driver builds");
        assert_eq!(drivers.temporal_name.as_deref(), Some(name));
        assert!(
            drivers.maintenance.is_empty(),
            "a plugin backend brings no Postgres maintenance job"
        );
        let tenant = antares_model::TenantId::default();
        drivers
            .store
            .create(
                &tenant,
                antares_store::Kind::Entity,
                "urn:ngsi-ld:Probe:1",
                serde_json::json!({"id": "urn:ngsi-ld:Probe:1"}),
            )
            .await
            .expect("create through the plugin driver");
        assert_eq!(
            drivers
                .store
                .list(&tenant, antares_store::Kind::Entity)
                .await
                .expect("list")
                .len(),
            1
        );
    }
}

#[cfg(test)]
mod api_surface_tests {
    use super::{api_surfaces, SURFACE_SHELF};

    fn names(raw: Option<&str>) -> Vec<&'static str> {
        api_surfaces(raw)
            .expect("selection")
            .iter()
            .map(|(n, _)| *n)
            .collect()
    }

    /// Absent means the operational surface, and nothing else: a pod that
    /// was never configured still answers its probes.
    #[test]
    fn absent_selects_admin() {
        assert_eq!(names(None), vec!["admin"]);
        assert_eq!(names(Some("admin")), vec!["admin"]);
        assert_eq!(
            names(Some(" admin , ")),
            vec!["admin"],
            "spacing is not a name"
        );
    }

    /// An unknown name is fatal at startup and the message names the shelf
    /// this binary was built with — never a silently ignored surface.
    #[test]
    fn an_unknown_surface_is_fatal_and_lists_the_shelf() {
        let err = api_surfaces(Some("admin,dashboard")).expect_err("must fail");
        assert!(err.contains("dashboard"), "{err}");
        for (name, _) in SURFACE_SHELF {
            assert!(err.contains(name), "the message lists {name}: {err}");
        }
    }

    /// A surface compiled in from outside `crates/` is selected by name
    /// beside the operational one, and claims its own prefix under `/x`.
    #[cfg(feature = "plugin-example")]
    #[test]
    fn a_plugin_surface_mounts_beside_admin() {
        assert_eq!(names(Some("admin,example")), vec!["admin", "example"]);
        let built: Vec<Box<dyn antares_api::ApiSurface>> = api_surfaces(Some("admin,example"))
            .expect("selection")
            .iter()
            .map(|(_, build)| build())
            .collect();
        let example = built
            .iter()
            .find(|s| s.name() == "example")
            .expect("the plugin surface is built");
        assert!(
            example.prefix().starts_with("/x/"),
            "a plugin surface lives under /x, never under the NGSI-LD root: {}",
            example.prefix()
        );
    }

    /// The view root is on the shelf of a `views` build, under `/x/views`.
    #[cfg(feature = "views")]
    #[test]
    fn a_views_build_shelves_the_view_root() {
        assert_eq!(names(Some("admin,views")), vec!["admin", "views"]);
        let root = api_surfaces(Some("views")).expect("selection")[0].1();
        assert_eq!(root.prefix(), "/x/views");
    }

    /// The LinkML surface shares the view root (ADR-0027 section 1).
    #[cfg(feature = "linkml")]
    #[test]
    fn a_linkml_build_shelves_the_linkml_surface_on_the_view_root() {
        assert_eq!(names(Some("views,linkml")), vec!["views", "linkml"]);
        let linkml = api_surfaces(Some("linkml")).expect("selection")[0].1();
        assert_eq!(linkml.prefix(), "/x/views");
    }

    /// The MCP surface shares the view root (ADR-0027 section 1).
    #[cfg(feature = "mcp")]
    #[test]
    fn an_mcp_build_shelves_the_mcp_surface_on_the_view_root() {
        assert_eq!(names(Some("views,mcp")), vec!["views", "mcp"]);
        let mcp = api_surfaces(Some("mcp")).expect("selection")[0].1();
        assert_eq!(mcp.prefix(), "/x/views");
    }

    /// An empty selection is fatal too: readiness, health and metrics are
    /// the admin surface, so a pod without one can never report itself up.
    #[test]
    fn an_empty_selection_is_fatal() {
        for raw in ["", " ", ",", " , "] {
            let err = api_surfaces(Some(raw)).expect_err(&format!("{raw:?} selects nothing"));
            assert!(err.contains("ANTARES_API_SURFACES"), "{err}");
        }
    }

    /// `full` is every surface in one binary (T-3636): the platform image
    /// selects them all by name.
    #[cfg(feature = "full")]
    #[test]
    fn the_full_build_shelves_every_surface() {
        let names = "admin,views,mcp,odata,ws,ws-views,sta,sta-mqtt";
        let picked = api_surfaces(Some(names)).expect("every surface is on the shelf");
        assert_eq!(picked.len(), names.split(',').count());
    }

    /// Rule M3, the route half (surfaces-threat-model 9.5 item 4): with no
    /// surface feature the shelf is the admin surface alone, and a path of
    /// every surface prefix (ADR-0023 to ADR-0027) is unrouted. A surface
    /// that lands without its feature gate turns this red.
    #[cfg(not(any(
        feature = "plugin-example",
        feature = "views",
        feature = "ws",
        feature = "channels"
    )))]
    #[tokio::test]
    async fn the_vanilla_shelf_is_admin_alone() {
        use tower::ServiceExt;

        let shelf: Vec<&str> = SURFACE_SHELF.iter().map(|(n, _)| *n).collect();
        assert_eq!(shelf, vec!["admin"]);
        let surfaces = api_surfaces(None)
            .expect("selection")
            .iter()
            .map(|(_, build)| build())
            .collect();
        let state = antares_api::AppState::new("antares-test".into())
            .with_surfaces(surfaces)
            .expect("the admin surface mounts");
        let app = antares_api::router(state);
        for path in [
            "/x/views",
            "/x/views/all/ngsi-ld/v1/entities",
            "/x/views/all/odata/v4/$metadata",
            "/x/views/all/mcp",
            "/x/views/all/linkml",
            "/x/views/all/ws",
            "/x/example/v1",
        ] {
            let request = axum::http::Request::get(path)
                .body(axum::body::Body::empty())
                .expect("request");
            let response = app.clone().oneshot(request).await.expect("answered");
            assert_eq!(
                response.status(),
                404,
                "{path} is routed in the vanilla build"
            );
        }
    }
}

#[cfg(test)]
mod policy_shelf_tests {
    use super::*;

    fn shelf() -> Vec<&'static str> {
        POLICY_SHELF.iter().map(|(n, _)| *n).collect()
    }

    /// ADR-0020: the broker ships one engine, and conformance is asserted
    /// against it. A release build carries no other — an engine reaches the
    /// shelf only through an off-by-default feature, so this assertion is
    /// what "no addon in the shipped build" means for the policy seam.
    #[cfg(not(any(feature = "plugin-example", feature = "jc-policy")))]
    #[test]
    fn the_shipped_build_carries_only_the_built_in_engine() {
        assert_eq!(shelf(), vec!["allow-all"]);
    }

    /// No selection is the built-in engine, which is the only default that
    /// keeps a broker without a policy behaving like a broker.
    #[test]
    fn no_selection_is_the_built_in_engine() {
        for raw in [None, Some(""), Some("  ")] {
            assert_eq!(policy_engine(raw).expect("default").0, "allow-all");
        }
    }

    /// A typo must not serve every request wide open, so it is fatal and
    /// the message names what this binary was built with.
    #[test]
    fn an_unknown_engine_is_fatal_and_names_the_shelf() {
        let err = policy_engine(Some("opa")).expect_err("unknown engine");
        assert!(err.contains("ANTARES_POLICY"), "{err}");
        for name in shelf() {
            assert!(err.contains(name), "the message lists {name}: {err}");
        }
    }

    /// An engine compiled in from outside `crates/` is selected by name
    /// exactly like the built-in one — and, given no rules to enforce,
    /// refuses every operation rather than allowing it (ADR-0020: a broken
    /// engine costs service, never access rules).
    #[cfg(feature = "plugin-example")]
    #[test]
    fn a_plugin_engine_is_on_the_shelf_and_fails_closed() {
        let name = antares_plugin_example::POLICY_NAME;
        assert!(shelf().contains(&name), "the shelf omits it: {:?}", shelf());
        let (selected, build) = policy_engine(Some(name)).expect("selection");
        assert_eq!(selected, name);
        assert_eq!(build().name(), name);
        assert!(
            std::env::var(antares_plugin_example::RULES_ENV).is_err(),
            "this test reads the engine built with no rules document"
        );
        assert!(
            antares_plugin_example::ExamplePolicy::from_env()
                .broken()
                .is_some(),
            "an engine selected without rules would otherwise allow everything"
        );
    }

    /// The access surface is on the shelf beside the platform's engine, and
    /// both are the one engine of the process: what the surface shows is
    /// the table the broker decides with (T-3638).
    #[cfg(all(feature = "jc-policy", feature = "views"))]
    #[test]
    fn the_access_surface_reads_the_engine_the_broker_decides_with() {
        let picked = api_surfaces(Some("admin,views,access")).expect("access is on the shelf");
        let access = picked[2].1();
        assert_eq!(access.name(), "access");
        assert_eq!(access.prefix(), "/x/views");
        let (_, build) = policy_engine(Some(joinedcontext_policy::POLICY_NAME)).expect("engine");
        let engine = build();
        let shared = joinedcontext_policy::JoinedcontextPolicy::process();
        engine
            .load("t3638", &serde_json::json!({"policies": []}))
            .expect("an empty document loads");
        assert!(
            shared.table("t3638").is_some(),
            "the surface's engine saw the load"
        );
        engine
            .load("t3638", &serde_json::Value::Null)
            .expect("a removal");
        assert!(shared.table("t3638").is_none());
    }

    /// The platform's engine is selected by name, and with no document
    /// loaded it denies a platform caller (T-3637).
    #[cfg(feature = "jc-policy")]
    #[tokio::test]
    async fn the_platform_engine_is_on_the_shelf_and_fails_closed() {
        let name = joinedcontext_policy::POLICY_NAME;
        let (selected, build) = policy_engine(Some(name)).expect("selection");
        assert_eq!(selected, name);
        let engine = build();
        assert!(engine.loads_documents());
        let subject = antares_api::policy::Subject {
            tenant: antares_model::TenantId::default(),
            headers: vec![("x-jc-subject".into(), "ana".into())],
        };
        let op = antares_api::policy::Operation::new("5.7.2");
        assert!(matches!(
            engine.decide(&subject, &op).await,
            antares_api::policy::Decision::Deny(_)
        ));
    }
}

#[cfg(test)]
mod pg_pool_tests {
    use super::{
        parse_pg_batch_async_commit, parse_pg_pool, parse_pg_statement_timeout,
        parse_pg_write_reserve, unknown_config_key,
    };

    /// ANTARES_PG_BATCH_ASYNC_COMMIT: off unless asked; a typo is fatal.
    #[test]
    fn pg_batch_async_commit_is_off_unless_asked() {
        assert_eq!(parse_pg_batch_async_commit(None), Ok(false));
        for off in ["", "0", "false", "OFF", "no"] {
            assert_eq!(parse_pg_batch_async_commit(Some(off)), Ok(false), "{off}");
        }
        for on in ["1", "true", "On", "yes"] {
            assert_eq!(parse_pg_batch_async_commit(Some(on)), Ok(true), "{on}");
        }
        assert!(parse_pg_batch_async_commit(Some("treu")).is_err());
    }

    /// ANTARES_PG_POOL: absent defaults to 20; a value that is not a
    /// positive integer is fatal — misconfiguration must never silently
    /// run with a default.
    #[test]
    fn pg_pool_parse_defaults_and_rejects() {
        assert_eq!(
            parse_pg_statement_timeout(None).expect("default"),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            parse_pg_statement_timeout(Some("1500")).expect("explicit"),
            std::time::Duration::from_millis(1500)
        );
        assert!(parse_pg_statement_timeout(Some("0")).is_err());
        assert!(parse_pg_statement_timeout(Some("30s")).is_err());
        assert_eq!(parse_pg_pool(None).expect("default"), 20);
        assert_eq!(parse_pg_pool(Some("7")).expect("explicit"), 7);
        assert!(
            parse_pg_pool(Some("abc")).is_err(),
            "non-numeric ANTARES_PG_POOL must be fatal"
        );
        assert!(
            parse_pg_pool(Some("0")).is_err(),
            "a zero-sized pool must be fatal"
        );
        assert!(parse_pg_pool(Some("-3")).is_err());
    }

    /// ANTARES_PG_WRITE_RESERVE (T-5150): absent is off, a reserve leaves
    /// the reads at least one connection, and a bad value is fatal.
    #[test]
    fn pg_write_reserve_parse_bounds() {
        assert_eq!(parse_pg_write_reserve(None, 16), Ok(0));
        assert_eq!(parse_pg_write_reserve(Some("0"), 16), Ok(0));
        assert_eq!(parse_pg_write_reserve(Some("4"), 16), Ok(4));
        assert_eq!(parse_pg_write_reserve(Some("15"), 16), Ok(15));
        assert!(parse_pg_write_reserve(Some("16"), 16).is_err());
        assert!(parse_pg_write_reserve(Some("-1"), 16).is_err());
        assert!(parse_pg_write_reserve(Some("four"), 16).is_err());
        assert!(!unknown_config_key("ANTARES_PG_WRITE_RESERVE"));
    }
}
