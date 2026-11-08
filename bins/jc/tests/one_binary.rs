//! End-to-end: `jc` started in-process on ephemeral ports with in-memory stores, one discovery or
//! health route of every mounted module called through one-domain slug mode and through host mode;
//! unknown slugs and hosts answer 404; overlapping mounts refuse to start.

use jc::config::{Cli, Config, FileConfig, MountOverride};
use jc::{Module, MountMode, StartError};
use reqwest::StatusCode;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

struct Fixture {
    _dir: tempfile::TempDir,
    kek: PathBuf,
    realms: PathBuf,
    ekiben_token: PathBuf,
    ekiben_root: PathBuf,
    antares_token: PathBuf,
    admin_token: PathBuf,
}

/// Antares keeps process-wide statics (one broker per process), and these tests share one process:
/// every test that starts jc holds this lock.
static ONE_JC_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const EKIBEN_TOKEN: &str = "ekiben-test-token";
const ANTARES_TOKEN: &str = "antares-admin-test-token-0123456789abcdef";
const ADMIN_TOKEN: &str = "jc-admin-metrics-test-token";

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let kek = dir.path().join("kek");
    std::fs::write(
        &kek,
        "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff\n",
    )
    .expect("kek");
    let realms = dir.path().join("realms");
    std::fs::create_dir(&realms).expect("realms dir");
    std::fs::write(
        realms.join("jc.json"),
        r#"{"realm":"jc","enabled":true,"displayName":"jc test realm"}"#,
    )
    .expect("realm");
    let ekiben_token = dir.path().join("ekiben-token");
    std::fs::write(&ekiben_token, format!("{EKIBEN_TOKEN}\n")).expect("token");
    let ekiben_root = dir.path().join("ekiben-files");
    let antares_token = dir.path().join("antares-admin-token");
    std::fs::write(&antares_token, format!("{ANTARES_TOKEN}\n")).expect("antares token");
    let admin_token = dir.path().join("admin-token");
    std::fs::write(&admin_token, format!("{ADMIN_TOKEN}\n")).expect("admin token");
    Fixture {
        _dir: dir,
        kek,
        realms,
        ekiben_token,
        ekiben_root,
        antares_token,
        admin_token,
    }
}

fn config(fx: &Fixture, mode: MountMode, mounts: BTreeMap<Module, MountOverride>) -> Config {
    let mut cfg = Config::merge(
        Cli {
            role: Some("all".into()),
            listen: Some("127.0.0.1:0".parse().unwrap()),
            admin_listen: Some("127.0.0.1:0".parse().unwrap()),
            admin_token_file: Some(fx.admin_token.clone()),
            base_host: Some("city.test".into()),
            scheme: Some("http".into()),
            kaisatsu_kek_file: Some(fx.kek.clone()),
            kaisatsu_import_dir: Some(fx.realms.clone()),
            ekiben_token_file: Some(fx.ekiben_token.clone()),
            ekiben_file_root: Some(fx.ekiben_root.clone()),
            antares_admin_token_file: Some(fx.antares_token.clone()),
            ..Cli::default()
        },
        FileConfig::default(),
    )
    .expect("config");
    cfg.mode = mode;
    cfg.mounts = mounts;
    cfg
}

/// The discovery or health route of each module (relative to its mount) and a string its answer
/// must contain.
fn probe(module: Module) -> (&'static str, &'static str) {
    match module {
        Module::Kaisatsu => ("/realms/jc/.well-known/openid-configuration", "\"issuer\""),
        Module::Dataroom => ("/api/3/action/status_show", "\"success\""),
        Module::Daiya => ("/api/health", "\"database\""),
        Module::Ekiben => ("/ping", "pong"),
        Module::Antares => ("/ngsi-ld/v1/info/sourceIdentity", "antares"),
        Module::Portal => ("/api/v1/health", "\"status\":\"ok\""),
    }
}

async fn get(addr: SocketAddr, host: &str, path: &str) -> (StatusCode, String) {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}{path}"))
        .header("host", host)
        .send()
        .await
        .expect("request");
    let status = resp.status();
    (status, resp.text().await.unwrap_or_default())
}

/// Sends a request line exactly as given (no client-side path normalization) and returns the
/// status code.
async fn raw_get_status(addr: SocketAddr, host: &str, path: &str) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    let text = String::from_utf8_lossy(&buf);
    text.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line")
}

#[tokio::test]
async fn slug_mode_reaches_every_module_and_404s_unknown_slugs() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let addr = running.addr;
    for &module in &cfg.roles {
        let (path, needle) = probe(module);
        let url = format!("/{}{path}", module.slug());
        let (status, body) = get(addr, "city.test", &url).await;
        assert_eq!(status, StatusCode::OK, "{module} {url}: {body}");
        assert!(body.contains(needle), "{module} {url}: {body}");
    }
    // Antares' /q admin surface needs its token inside jc; health stays open.
    let (status, _) = get(addr, "city.test", "/broker/q/health").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get(addr, "city.test", "/broker/q/tenants").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/broker/q/tenants"))
        .header("host", "city.test")
        .bearer_auth(ANTARES_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // kaisatsu's issuer carries the slug: the module was built with its mount.
    let (_, body) = get(
        addr,
        "city.test",
        "/id/realms/jc/.well-known/openid-configuration",
    )
    .await;
    assert!(body.contains("http://city.test/id/realms/jc"), "{body}");

    for unknown in [
        "/",
        "/nope/realms/jc",
        "/idx/realms/jc",
        "/%69d/realms/jc",
        "/ID/realms/jc",
    ] {
        let (status, body) = get(addr, "city.test", unknown).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}: {body}");
    }
    // A dot-segment never crosses from one module's mount into another's (sent raw: an HTTP
    // client would resolve it before sending).
    let status = raw_get_status(addr, "city.test", "/dashboards/../id/realms/jc").await;
    assert_eq!(status, 404);
    let status = raw_get_status(addr, "city.test", "/dashboards/%2e%2e/id/realms/jc").await;
    assert_eq!(status, 404);

    // The admin listener is separate: metrics (behind the admin token: they carry Antares'
    // series, which the broker serves only behind its own token) and open liveness there, never
    // on the public listener.
    let admin = running.admin_addr.expect("admin listener");
    let (status, body) = get(admin, "localhost", "/healthz").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));
    let (status, body) = get(admin, "localhost", "/metrics").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.is_empty(), "{body}");
    let client = reqwest::Client::new();
    for wrong in [
        "wrong",
        "jc-admin-metrics-test-tokem",
        "jc-admin-metrics-test-token-",
    ] {
        let resp = client
            .get(format!("http://{admin}/metrics"))
            .bearer_auth(wrong)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{wrong}");
    }
    let resp = client
        .get(format!("http://{admin}/metrics"))
        .header("authorization", format!("Basic {ADMIN_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let resp = client
        .get(format!("http://{admin}/metrics"))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert!(!text.contains(ADMIN_TOKEN));
    let (status, _) = get(addr, "city.test", "/metrics").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    running.stop().await.expect("clean stop");
}

#[tokio::test]
async fn host_mode_reaches_every_module_and_404s_unknown_hosts() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    jc::runtime::init_tracing(jc::config::LogFormat::Text);
    let fx = fixture();
    let mut cfg = config(&fx, MountMode::Host, BTreeMap::new());
    // Behind a trusted proxy the forwarded host chooses the module; from anyone else it is ignored.
    cfg.trusted_proxies = vec!["127.0.0.0/8".parse().unwrap()];
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let addr = running.addr;
    for &module in &cfg.roles {
        let (path, needle) = probe(module);
        let host = format!("{}.city.test", module.host_label());
        let (status, body) = get(addr, &host, path).await;
        assert_eq!(status, StatusCode::OK, "{module} {host}{path}: {body}");
        assert!(body.contains(needle), "{module} {host}{path}: {body}");
        // The same path on the bare domain or with the slug reaches nothing.
        let (status, _) = get(addr, "city.test", path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{module} on the bare domain");
    }
    let (_, body) = get(
        addr,
        "id.city.test",
        "/realms/jc/.well-known/openid-configuration",
    )
    .await;
    assert!(body.contains("http://id.city.test/realms/jc"), "{body}");

    for host in [
        "nope.city.test",
        "city.test",
        "id.other.test",
        "id.city.test.evil",
    ] {
        let (status, _) = get(addr, host, "/realms/jc/.well-known/openid-configuration").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{host}");
    }
    running.stop().await.expect("clean stop");
}

#[tokio::test]
async fn explicit_mounts_mix_hosts_and_slugs() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let mounts = BTreeMap::from([
        (
            Module::Kaisatsu,
            MountOverride {
                host: Some("Login.City.Test".into()),
                path_prefix: None,
            },
        ),
        (
            Module::Dataroom,
            MountOverride {
                host: None,
                path_prefix: Some("/data/".into()),
            },
        ),
    ]);
    let cfg = config(&fx, MountMode::Slug, mounts);
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let addr = running.addr;
    let (status, body) = get(
        addr,
        "login.city.test",
        "/realms/jc/.well-known/openid-configuration",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = get(addr, "city.test", "/data/api/3/action/status_show").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = get(addr, "city.test", "/opendata/api/3/action/status_show").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = get(addr, "anything.test", "/dashboards/api/health").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    running.stop().await.expect("clean stop");
}

#[tokio::test]
async fn a_forwarded_host_from_an_untrusted_peer_is_ignored() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let mut cfg = config(&fx, MountMode::Host, BTreeMap::new());
    cfg.roles = vec![Module::Daiya];
    cfg.trusted_proxies = vec!["10.0.0.0/8".parse().unwrap()];
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let resp = reqwest::Client::new()
        .get(format!("http://{}/api/health", running.addr))
        .header("host", "nope.city.test")
        .header("x-forwarded-host", "dashboards.city.test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    running.stop().await.expect("clean stop");
}

#[tokio::test]
async fn overlapping_mounts_refuse_to_start() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let same = MountOverride {
        host: Some("city.test".into()),
        path_prefix: Some("/shared".into()),
    };
    let cfg = config(
        &fx,
        MountMode::Slug,
        BTreeMap::from([(Module::Dataroom, same.clone()), (Module::Daiya, same)]),
    );
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("overlap refused");
    assert!(matches!(err, StartError::Mount(_)), "{err}");
    assert!(err.to_string().contains("overlapping"), "{err}");

    // A slug override that lands on another module's default slug is refused too.
    let cfg = config(
        &fx,
        MountMode::Slug,
        BTreeMap::from([(
            Module::Daiya,
            MountOverride {
                host: None,
                path_prefix: Some("/id".into()),
            },
        )]),
    );
    assert!(matches!(jc::plan_mounts(&cfg), Err(StartError::Mount(_))));
}

#[tokio::test]
async fn an_invalid_prefix_or_a_bad_module_setting_refuses_to_start() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let cfg = config(
        &fx,
        MountMode::Slug,
        BTreeMap::from([(
            Module::Dataroom,
            MountOverride {
                host: None,
                path_prefix: Some("/a/../b".into()),
            },
        )]),
    );
    assert!(matches!(
        jc::start(&cfg, CancellationToken::new()).await,
        Err(StartError::Mount(_))
    ));

    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.dataroom.org_id = "not-a-uuid".into();
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Dataroom,
                ..
            }
        ),
        "{err}"
    );

    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.kaisatsu.kek_file = Some(fx.realms.join("missing-kek"));
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Kaisatsu,
                ..
            }
        ),
        "{err}"
    );

    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.kaisatsu.kek_file = None;
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Kaisatsu,
                ..
            }
        ),
        "{err}"
    );

    let bad_realms = fx.realms.join("bad");
    std::fs::create_dir(&bad_realms).unwrap();
    std::fs::write(bad_realms.join("x.json"), "{ not json").unwrap();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.kaisatsu.import_dir = Some(bad_realms);
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Kaisatsu,
                ..
            }
        ),
        "{err}"
    );

    let bad_realm = fx.realms.join("bad-realm");
    std::fs::create_dir(&bad_realm).unwrap();
    std::fs::write(bad_realm.join("x.json"), r#"{"realm": 42}"#).unwrap();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.kaisatsu.import_dir = Some(bad_realm);
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Kaisatsu,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn a_bad_antares_environment_refuses_to_start() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    // SAFETY of the test: env access is serialized by the lock above; removed before unlocking.
    unsafe { std::env::set_var("ANTARES_BUS", "carrier-pigeon") };
    let result = jc::start(&cfg, CancellationToken::new()).await;
    unsafe { std::env::remove_var("ANTARES_BUS") };
    let err = result.err().expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Antares,
                ..
            }
        ),
        "{err}"
    );

    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.antares.admin_token_file = None;
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StartError::Module {
                module: Module::Antares,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn a_taken_port_is_reported() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.listen = taken.local_addr().unwrap();
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, StartError::Bind { .. }), "{err}");
    assert!(err.to_string().contains("cannot listen"));

    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.admin_listen = Some(taken.local_addr().unwrap());
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, StartError::Bind { .. }), "{err}");
}

#[tokio::test]
async fn an_admin_listener_without_a_readable_token_is_refused() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.roles = vec![Module::Daiya];
    cfg.admin_token_file = None;
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, StartError::Config(_)), "{err}");

    let empty = fx.admin_token.with_file_name("empty-admin-token");
    std::fs::write(&empty, "\n").unwrap();
    cfg.admin_token_file = Some(empty);
    let err = jc::start(&cfg, CancellationToken::new())
        .await
        .err()
        .expect("refused");
    assert!(matches!(err, StartError::Admin(_)), "{err}");
    assert!(err.to_string().contains("is empty"), "{err}");
}

#[tokio::test]
async fn run_until_serves_until_the_stop_future_resolves() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.admin_listen = None;
    cfg.roles = vec![Module::Daiya];
    // jc's start future is not Send (Antares' build_state), so it runs on this task.
    let stop = tokio::time::sleep(std::time::Duration::from_millis(50));
    jc::run_until(cfg, stop).await.expect("clean stop");
}

#[tokio::test]
async fn running_exposes_its_shutdown_token() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    let fx = fixture();
    let mut cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    cfg.roles = vec![Module::Daiya];
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let addr = running.addr;
    running.shutdown_token().cancel();
    running.wait().await.expect("clean stop");
    assert!(
        reqwest::get(format!("http://{addr}/dashboards/api/health"))
            .await
            .is_err()
    );
}

/// A secret another module reads from the environment must not come back through an ekiben
/// stream posted to the shared process: not by `${VAR}`, not by Bloblang `env()` (the `file`
/// input of `/proc/self/environ` is in `tests/binary.rs`, where the secret is in the process's
/// initial environment). The ekiben API also refuses callers without its token.
#[tokio::test]
async fn another_modules_secret_is_not_readable_through_an_ekiben_stream() {
    let _serial = ONE_JC_AT_A_TIME.lock().await;
    const SECRET: &str = "kaisatsu-db-password-7f3a91";
    // SAFETY of the test: a unique variable name, set once before jc starts.
    unsafe { std::env::set_var("JC_E2E_KAISATSU_DB_PASSWORD", SECRET) };
    let fx = fixture();
    let cfg = config(&fx, MountMode::Slug, BTreeMap::new());
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    let base = format!("http://{}/streams", running.addr);
    let client = reqwest::Client::new();

    // Without the token: refused, also for the read-only endpoints.
    for path in ["/streams", "/metrics", "/ready"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    // ${VAR}: the foreign variable is undefined in a posted config.
    let resp = client
        .post(format!("{base}/streams/leak"))
        .bearer_auth(EKIBEN_TOKEN)
        .body("input:\n  generate:\n    interval: 1h\n    mapping: 'root = \"${JC_E2E_KAISATSU_DB_PASSWORD}\"'\noutput:\n  drop: {}\n")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.text().await.unwrap();
    assert!(!body.contains(SECRET), "{body}");

    // env(), written to a file in the sandbox the test reads back.
    let once = "input:\n  generate:\n    count: 1\n    interval: \"\"\n    mapping: 'root = {\"env\": env(\"JC_E2E_KAISATSU_DB_PASSWORD\")}'\noutput:\n  file:\n    path: env.jsonl\n    codec: lines\n";
    let resp = client
        .post(format!("{base}/streams/env-probe?run=once"))
        .bearer_auth(EKIBEN_TOKEN)
        .body(once)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "{}",
        resp.text().await.unwrap()
    );
    let written = std::fs::read_to_string(fx.ekiben_root.join("env.jsonl")).expect("env.jsonl");
    assert!(written.contains("\"env\":null"), "{written}");
    assert!(!written.contains(SECRET), "{written}");

    running.stop().await.expect("clean stop");
}
