//! T-7767: the gateway's wiring from its configuration (`serve::build`, `serve::broker_of`):
//! every optional part a deployment names is started, and each part that cannot start refuses
//! the start with a message naming it.

use crate::common;
use context_gateway::config::Config;
use context_gateway::proxy::Broker;
use context_gateway::serve::{broker_of, build};
use serde_json::json;
use std::path::PathBuf;

/// A Portal and realm in one: the realm's JWKS, a token endpoint, and an empty list on every
/// other path the pollers ask.
async fn portal(realm: &common::Realm) -> String {
    let jwks = serde_json::to_value(realm.published_jwks()).expect("a JWKS");
    let app = axum::Router::new()
        .route(
            "/jwks",
            axum::routing::get(move || {
                let jwks = jwks.clone();
                async move { axum::Json(jwks) }
            }),
        )
        .route(
            "/token",
            axum::routing::post(|| async {
                axum::Json(json!({ "access_token": "workload-t", "expires_in": 300 }))
            }),
        )
        .fallback(|| async { axum::Json(json!({ "items": [] })) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gw-serve-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn config(vars: Vec<(&'static str, String)>) -> Config {
    Config::from_vars(move |name| {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| match name {
                "JC_GATEWAY_BROKER_URL" => Some("http://127.0.0.1:9".to_owned()),
                "JC_GATEWAY_ORG_DOMAIN" => Some("hel.fi".to_owned()),
                _ => None,
            })
    })
    .expect("a usable configuration")
}

#[tokio::test]
async fn a_bare_configuration_serves_public_endpoints_only() {
    let built = build(&config(Vec::new()), Broker::new("http://127.0.0.1:9"))
        .await
        .expect("it starts");
    assert!(built.internal.is_none());
    assert!(built.organization_realms.is_none());
    assert!(built.gateway.verifier.is_none());
}

#[tokio::test]
async fn every_named_part_starts_and_the_organization_realms_can_be_replaced() {
    let realm = common::Realm::new();
    let base = portal(&realm).await;
    let repo = scratch("repo");
    let previews = scratch("previews");
    let keys = scratch("keys");
    let org_realm = |org: &str| {
        format!(
            r#"{{"organization":"{org}","issuer":"https://idm.{org}.test/realms/{org}","jwks_url":"{base}/jwks"}}"#
        )
    };
    let config = config(vec![
        ("JC_GATEWAY_REPO_DIR", repo.display().to_string()),
        ("JC_GATEWAY_INTERNAL_BIND", "127.0.0.1:0".to_owned()),
        ("JC_OIDC_ISSUER", common::ISSUER.to_owned()),
        ("JC_OIDC_JWKS_URL", format!("{base}/jwks")),
        (
            "JC_OIDC_ORGANIZATION_REALMS",
            format!("[{}]", org_realm("bbsk")),
        ),
        ("JC_OIDC_CLIENT_ID", "context-gateway".to_owned()),
        ("JC_OIDC_CLIENT_SECRET", "test-client-secret".to_owned()),
        ("JC_OIDC_TOKEN_URL", format!("{base}/token")),
        ("JC_GATEWAY_PUBLIC_URL", "https://gw.hel.fi".to_owned()),
        ("JC_GATEWAY_OAUTH_KEY", "k".repeat(32)),
        ("JC_GATEWAY_PERSONAL_KEYS", keys.display().to_string()),
        ("JC_GATEWAY_DOMAIN_VERIFICATION", "enforce".to_owned()),
        (
            "JC_GATEWAY_DOMAIN_VERIFICATIONS_URL",
            format!("{base}/domains"),
        ),
        ("JC_GATEWAY_ASSISTANT_URL", format!("{base}/assistant")),
        ("JC_GATEWAY_USAGE_URL", format!("{base}/usage")),
        ("JC_GATEWAY_REVOCATIONS_URL", format!("{base}/revocations")),
        ("JC_GATEWAY_ACTIVITY_URL", format!("{base}/activity")),
        ("JC_GATEWAY_CONNECTORS_URL", format!("{base}/connectors")),
        ("JC_GATEWAY_PREVIEWS_URL", format!("{base}/previews")),
        ("JC_GATEWAY_PREVIEWS_DIR", previews.display().to_string()),
    ]);
    let built = build(&config, Broker::new("http://127.0.0.1:9"))
        .await
        .expect("it starts");
    assert!(built.internal.is_some(), "the MQTT brokers' router");
    assert!(
        built.gateway.verifier.is_some(),
        "the realm's keys are loaded"
    );
    let realms = built.organization_realms.expect("an installation realm");
    let two = format!("[{},{}]", org_realm("bbsk"), org_realm("zilina"));
    assert_eq!(realms.replace(&two), Ok(2));
    assert!(
        realms.replace("not json").is_err(),
        "a broken set changes nothing"
    );
    assert_eq!(realms.replace(" "), Ok(0));
    for dir in [repo, previews, keys] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test]
async fn a_part_that_cannot_start_refuses_the_start_naming_it() {
    let missing = config(vec![(
        "JC_GATEWAY_REPO_DIR",
        "/nonexistent/gw-repo".to_owned(),
    )]);
    let refused = build(&missing, Broker::new("http://127.0.0.1:9"))
        .await
        .err()
        .expect("no repository");
    assert!(
        refused.starts_with("cannot read the manifest repository"),
        "{refused}"
    );

    let no_keys = config(vec![
        ("JC_OIDC_ISSUER", common::ISSUER.to_owned()),
        ("JC_OIDC_JWKS_URL", "http://127.0.0.1:9/jwks".to_owned()),
        (
            "JC_GATEWAY_REVOCATIONS_URL",
            "http://127.0.0.1:9/revocations".to_owned(),
        ),
    ]);
    let refused = build(&no_keys, Broker::new("http://127.0.0.1:9"))
        .await
        .err()
        .expect("no keys");
    assert!(
        refused.starts_with("cannot load the realm signing keys"),
        "{refused}"
    );
}

#[test]
fn the_broker_hop_trusts_the_image_roots_or_the_named_bundle() {
    if std::path::Path::new("/etc/ssl/certs/ca-certificates.crt").exists() {
        assert!(broker_of(&config(Vec::new())).is_ok());
    }
    let refused = broker_of(&config(vec![(
        "JC_GATEWAY_EGRESS_CA_BUNDLE",
        "/nonexistent/bundle.pem".to_owned(),
    )]))
    .expect_err("no bundle");
    assert!(refused.starts_with("/nonexistent/bundle.pem"), "{refused}");
    let dir = scratch("bundle");
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, b"").expect("a file");
    let refused = broker_of(&config(vec![(
        "JC_GATEWAY_EGRESS_CA_BUNDLE",
        empty.display().to_string(),
    )]))
    .expect_err("an empty bundle");
    assert!(refused.contains("empty.pem"), "{refused}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A CA bundle the broker hop trusts instead of the image's roots, and the broker template that
/// names the other brokers.
#[test]
fn a_named_bundle_is_trusted_and_brokers_are_addressed_by_the_template() {
    let roots = std::fs::read("/etc/ssl/certs/ca-certificates.crt").expect("the image's roots");
    let dir = scratch("bundle-ok");
    let bundle = dir.join("roots.pem");
    std::fs::write(&bundle, roots).expect("a bundle");
    let config = config(vec![
        ("JC_GATEWAY_EGRESS_CA_BUNDLE", bundle.display().to_string()),
        (
            "JC_GATEWAY_BROKER_URL_TEMPLATE",
            "http://{broker}.brokers.svc:1026".to_owned(),
        ),
    ]);
    assert!(broker_of(&config).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

/// An organization of project checkouts follows them with the repository, and previews listed
/// without a workload token still start: the Portal's refusal says so in the log.
#[tokio::test]
async fn checkouts_and_previews_without_a_token_start() {
    let realm = common::Realm::new();
    let base = portal(&realm).await;
    let repo = scratch("repo-checkouts");
    let projects = scratch("projects");
    let previews = scratch("previews-no-token");
    let with_checkouts = config(vec![
        ("JC_GATEWAY_REPO_DIR", repo.display().to_string()),
        ("JC_GATEWAY_PROJECTS_DIR", projects.display().to_string()),
    ]);
    let built = build(&with_checkouts, Broker::new("http://127.0.0.1:9"))
        .await
        .expect("it starts");
    assert!(!built.gateway.stale());

    let anonymous_previews = config(vec![
        ("JC_GATEWAY_REPO_DIR", repo.display().to_string()),
        ("JC_GATEWAY_PREVIEWS_URL", format!("{base}/previews")),
        ("JC_GATEWAY_PREVIEWS_DIR", previews.display().to_string()),
    ]);
    build(&anonymous_previews, Broker::new("http://127.0.0.1:9"))
        .await
        .expect("it starts");
    for dir in [repo, projects, previews] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// An OAuth key the issuer refuses is a start refused, naming the server.
#[tokio::test]
async fn an_oauth_key_the_issuer_refuses_refuses_the_start() {
    let mut weak = config(vec![(
        "JC_GATEWAY_PUBLIC_URL",
        "https://gw.hel.fi".to_owned(),
    )]);
    weak.oauth_key = Some(jc_core::Secret::new("short"));
    let refused = build(&weak, Broker::new("http://127.0.0.1:9"))
        .await
        .err()
        .expect("a weak key");
    assert!(
        refused.starts_with("cannot start the anonymous OAuth server"),
        "{refused}"
    );
}
