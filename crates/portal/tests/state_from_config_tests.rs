//! What the Portal starts with, and what stops it (T-2506, CC-03, PL-15).
//!
//! `AppState::from_config` is the whole startup: a realm that cannot be discovered, a database
//! that cannot be reached and half a forge are fatal; a realm whose keys are down, and no
//! database at all, are not. The database cases run when `JC_PORTAL_TEST_DATABASE_URL` names a
//! PostgreSQL the test may write to, and skip otherwise.
//!
//! The forge and the GitHub copy are read from the process environment inside `from_config`
//! and propagated with `?`, so their half-configured cases are played on the two `from_env`
//! functions it calls, with a lookup of the test's own rather than a process-wide variable
//! another test would see.

use joinedcontext_portal::auth::session::{now_unix, Identity, Session};
use joinedcontext_portal::config::{Config, ConfigError};
use joinedcontext_portal::git::github_mirror::GithubMirror;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM_PATH: &str = "/realms/banskabystrica";

fn with_realm(issuer: &str) -> Config {
    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer.to_owned()),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("test-secret".to_owned()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().expect("url");
    config
}

/// A realm whose discovery answers `discovery` and whose key set answers `keys`.
async fn realm(discovery: u16, keys: u16) -> (MockServer, String) {
    let server = MockServer::start().await;
    let issuer = format!("{}{REALM_PATH}", server.uri());
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(discovery).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(keys).set_body_string("down"))
        .mount(&server)
        .await;
    (server, issuer)
}

fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("an address")
        .port()
}

/// Case 2: a realm that cannot be discovered, answering an error or not answering at all,
/// stops the Portal, and the error says which.
#[tokio::test]
async fn oidc_discovery_failure_is_fatal_and_the_reason_survives() {
    let (_server, issuer) = realm(503, 200).await;
    let unreachable = format!("http://127.0.0.1:{}{REALM_PATH}", closed_port());
    for issuer in [issuer, unreachable] {
        let err = match AppState::from_config(with_realm(&issuer)).await {
            Ok(_) => panic!("{issuer}: a Portal started without its realm"),
            Err(err) => err.to_string(),
        };
        assert!(!err.trim().is_empty(), "{issuer}: an empty reason");
        assert!(
            !err.contains("test-secret"),
            "the client secret is in: {err}"
        );
    }
}

/// Case 7, as it can occur: discovery (the `openidconnect` crate's `discover_async`) reads the
/// realm's key set itself, so a key set that is down at startup stops the Portal there, before
/// the warm-up. The warm-up's warning covers only a fetch that fails between the two; a Portal
/// with a realm and no keys would refuse every login anyway.
#[tokio::test]
async fn a_realm_whose_key_set_is_down_stops_the_portal_at_discovery() {
    let (_server, issuer) = realm(200, 500).await;
    let err = match AppState::from_config(with_realm(&issuer)).await {
        Ok(_) => panic!("a Portal started with no key set to verify with"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("500"), "{err}");

    // The control: the same realm with its keys up starts, with a verifier to use them.
    let server = MockServer::start().await;
    let issuer = format!("{}{REALM_PATH}", server.uri());
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&server)
        .await;
    let state = AppState::from_config(with_realm(&issuer))
        .await
        .expect("the Portal starts");
    assert!(state.bearer.is_some());
}

/// Case 3: half a forge is a startup error, whichever half is missing; none is a Portal
/// without one.
#[test]
fn a_half_configured_gitea_is_a_startup_error_not_a_portal_without_a_forge() {
    let all = [
        ("JC_GITEA_URL", "http://gitea:3000"),
        ("JC_GITEA_OWNER", "owner"),
        ("JC_GITEA_REPO", "config"),
        ("JC_GITEA_TOKEN", "forge-token"),
    ];
    for missing in 0..all.len() {
        let lookup = |key: &str| {
            all.iter()
                .enumerate()
                .find(|(i, (k, _))| *i != missing && *k == key)
                .map(|(_, (_, v))| (*v).to_owned())
        };
        let err = match GiteaClient::from_env(lookup) {
            Ok(_) => panic!("{} missing and the forge was accepted", all[missing].0),
            Err(err) => err.to_string(),
        };
        assert!(!err.contains("forge-token"), "the token is in: {err}");
    }
    assert!(GiteaClient::from_env(|_| None).expect("off").is_none());
    let whole = |key: &str| {
        all.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    };
    assert!(GiteaClient::from_env(whole).expect("on").is_some());
}

/// A WASM shard count that is no positive number stops the Portal instead of turning into 2.
#[test]
fn an_unparsable_or_zero_wasm_shard_count_is_a_startup_error() {
    for shards in ["three", "0", "-1"] {
        let lookup = |key: &str| match key {
            "JC_PORTAL_APPS_DB_URL" => Some("postgres://apps@db/apps".to_owned()),
            "JC_PORTAL_WASM_SHARDS" => Some(shards.to_owned()),
            _ => None,
        };
        match Config::from_vars(lookup) {
            Err(ConfigError::Invalid { var, .. }) => assert_eq!(var, "JC_PORTAL_WASM_SHARDS"),
            other => panic!("{shards}: {:?}", other.map(|c| c.apps_db)),
        }
    }
    let three = Config::from_vars(|key: &str| match key {
        "JC_PORTAL_APPS_DB_URL" => Some("postgres://apps@db/apps".to_owned()),
        "JC_PORTAL_WASM_SHARDS" => Some(" 3 ".to_owned()),
        _ => None,
    })
    .expect("a shard count");
    assert_eq!(three.apps_db.map(|db| db.shards), Some(3));
}

/// Case 4: the GitHub copy takes both of its values or neither.
#[test]
fn a_half_configured_github_mirror_is_a_startup_error() {
    for (owner, token) in [(Some("jc-apps"), None), (None, Some("gh-token"))] {
        let lookup = |key: &str| match key {
            "JC_APP_MIRROR_GITHUB_OWNER" => owner.map(str::to_owned),
            "JC_APP_MIRROR_GITHUB_TOKEN" => token.map(str::to_owned),
            _ => None,
        };
        let err = match GithubMirror::from_env(lookup) {
            Ok(_) => panic!("{owner:?}/{token:?} was accepted"),
            Err(err) => err.to_string(),
        };
        assert!(!err.contains("gh-token"), "the token is in: {err}");
    }
    assert!(GithubMirror::from_env(|_| None).expect("off").is_none());
}

/// Case 5: a database that is configured and cannot be reached stops the Portal rather than
/// letting it forget preferences, runs and logout marks.
#[tokio::test]
async fn a_configured_database_that_cannot_be_reached_is_fatal() {
    let config = Config {
        database_url: Some(format!(
            "postgres://portal:db-password@127.0.0.1:{}/portal",
            closed_port()
        )),
        ..Config::for_tests()
    };
    let err = match AppState::from_config(config).await {
        Ok(_) => panic!("a Portal started without its database"),
        Err(err) => err.to_string(),
    };
    assert!(!err.contains("db-password"), "the password is in: {err}");
}

/// Case 6: no database is a Portal whose runs live in this process, and says so.
#[tokio::test]
async fn no_database_still_starts_with_a_non_durable_agent_store() {
    let state = AppState::from_config(Config::for_tests())
        .await
        .expect("the Portal starts");
    assert!(state.db.is_none());
    assert!(!state.agents.is_durable());
}

fn session(subject: &str, issued_at: i64) -> Session {
    Session {
        identity: Identity {
            client: None,
            subject: subject.into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: issued_at + 3600,
        issued_at,
        signed_in_at: None,
        id_token: "id-token".into(),
        access_expires_at: issued_at + 3600,
        refresh_token: None,
    }
}

/// Case 1: a logout the process before this one recorded is refused by this one from its first
/// request (T-0980); a mark past its lifetime is trimmed and refuses nothing.
#[tokio::test]
async fn prior_revocations_are_loaded_before_the_first_request_is_served() {
    let Ok(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL") else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let config = || Config {
        database_url: Some(url.clone()),
        ..Config::for_tests()
    };
    let now = now_unix();
    let (live, expired) = (
        format!("t2506-live-{now}-{}", std::process::id()),
        format!("t2506-expired-{now}-{}", std::process::id()),
    );
    let before = AppState::from_config(config())
        .await
        .expect("first process");
    before.revoke_subject(&live, now).await;
    let db = before.db.clone().expect("a database");
    sqlx::query(
        "INSERT INTO revocations (subject, revoked_at, expires_at) VALUES ($1, $2, now() - interval '1 hour')",
    )
    .bind(&expired)
    .bind(now)
    .execute(&db)
    .await
    .expect("an expired mark");
    drop(before);

    let after = AppState::from_config(config()).await.expect("next process");
    assert!(after.is_revoked(&session(&live, now - 60)));
    assert!(
        !after.is_revoked(&session(&live, now + 60)),
        "a later login"
    );
    assert!(!after.is_revoked(&session(&expired, now - 60)));
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM revocations WHERE subject = $1")
        .bind(&expired)
        .fetch_one(&db)
        .await
        .expect("count");
    assert_eq!(left, 0, "the expired mark was not trimmed");
}
