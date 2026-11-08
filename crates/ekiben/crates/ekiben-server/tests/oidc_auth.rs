//! ekiben in `jc` admits the Portal's runner client by its kaisatsu token: a JWT signed by a key
//! of the provider's JWKS, for issuer and audience `ekiben`, of an admitted client. Everything
//! else stays refused: another audience, issuer or client, an expired token, a token without
//! `aud` or `kid`, a foreign key under a known `kid`, an HMAC token keyed with the public key,
//! and any token when no client is configured. Unknown keys refetch the JWKS at most once per
//! period.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ekiben_core::{EnvAccess, ResourceManager};
use ekiben_server::{
    build_full_registry, build_router, OidcAuth, ServerAuthConfig, ServerState, StreamManager,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::net::TcpListener;

const ISSUER: &str = "http://127.0.0.1/id/realms/jc";
const RUNNER: &str = "portal-pipeline-runner";

struct Key {
    pkcs8: Vec<u8>,
    jwk: Value,
}

fn key(kid: &str) -> Key {
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();
    // Uncompressed point: 0x04 || x || y.
    let point = pair.public_key().as_ref().to_vec();
    Key {
        pkcs8: pkcs8.as_ref().to_vec(),
        jwk: json!({
            "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": kid,
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        }),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn sign(key: &Key, kid: Option<&str>, claims: Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = kid.map(str::to_owned);
    encode(&header, &claims, &EncodingKey::from_ec_der(&key.pkcs8)).unwrap()
}

fn claims() -> Value {
    json!({"iss": ISSUER, "aud": ["ekiben", "account"], "azp": RUNNER, "exp": now() + 300, "iat": now(), "sub": "service-account"})
}

/// Serves `{"keys": [jwk]}` and counts the fetches.
async fn jwks(jwk: Value) -> (String, Arc<AtomicUsize>) {
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = fetches.clone();
    let app = axum::Router::new().route(
        "/certs",
        axum::routing::get(move || {
            let (jwk, counter) = (jwk.clone(), counter.clone());
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({"keys": [jwk]}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/certs"), fetches)
}

async fn ekiben(jwks_url: &str, clients: &[&str]) -> String {
    let manager = StreamManager::new(
        build_full_registry(),
        ResourceManager::new(),
        None,
        EnvAccess::AllowListed,
    );
    let oidc = OidcAuth::new(
        jwks_url,
        ISSUER,
        "ekiben",
        clients.iter().map(|c| c.to_string()).collect(),
    )
    .unwrap();
    let state = ServerState::new(
        manager,
        ServerAuthConfig {
            admin_bearer_token: Some("static-admin-token".into()),
            require_auth: true,
            guard_probes: true,
            oidc: Some(Arc::new(oidc)),
            ..Default::default()
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn status(base: &str, token: Option<&str>) -> StatusCode {
    let mut req = reqwest::Client::new().get(format!("{base}/streams"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    req.send().await.unwrap().status()
}

#[tokio::test]
async fn the_runner_clients_kaisatsu_token_is_admitted_and_every_other_token_refused() {
    let signer = key("k1");
    let (url, _) = jwks(signer.jwk.clone()).await;
    let base = ekiben(&url, &[RUNNER]).await;

    let good = sign(&signer, Some("k1"), claims());
    assert_eq!(status(&base, Some(&good)).await, StatusCode::OK);
    // The admin routes (quotas) take it too: the Portal manages every project's streams.
    let quotas = reqwest::Client::new()
        .get(format!("{base}/orgs/praha/quotas"))
        .bearer_auth(&good)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(quotas, StatusCode::OK);
    assert_eq!(
        status(&base, Some("static-admin-token")).await,
        StatusCode::OK
    );

    let mut refused = Vec::new();
    let mut with = |name: &str, claims: Value| refused.push((name.to_string(), claims));
    let mut c = claims();
    c["aud"] = json!("portal-internal");
    with("another audience", c);
    let mut c = claims();
    c.as_object_mut().unwrap().remove("aud");
    with("no audience", c);
    let mut c = claims();
    c["iss"] = json!("http://127.0.0.1/id/realms/other");
    with("another issuer", c);
    let mut c = claims();
    c["azp"] = json!("praha-pipelines");
    with("another client", c);
    let mut c = claims();
    c.as_object_mut().unwrap().remove("azp");
    with("no client", c);
    let mut c = claims();
    c["exp"] = json!(now() - 3600);
    with("expired", c);
    for (name, c) in refused {
        let token = sign(&signer, Some("k1"), c);
        assert_eq!(
            status(&base, Some(&token)).await,
            StatusCode::UNAUTHORIZED,
            "{name}"
        );
    }

    let no_kid = sign(&signer, None, claims());
    assert_eq!(status(&base, Some(&no_kid)).await, StatusCode::UNAUTHORIZED);
    let forged = sign(&key("k1"), Some("k1"), claims());
    assert_eq!(status(&base, Some(&forged)).await, StatusCode::UNAUTHORIZED);
    // Algorithm confusion: HS256 keyed with the published key's bytes.
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("k1".into());
    let hmac = encode(
        &header,
        &claims(),
        &EncodingKey::from_secret(signer.jwk.to_string().as_bytes()),
    )
    .unwrap();
    assert_eq!(status(&base, Some(&hmac)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&base, None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        status(&base, Some("not-a-token")).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn no_configured_client_admits_no_token() {
    let signer = key("k1");
    let (url, _) = jwks(signer.jwk.clone()).await;
    let base = ekiben(&url, &[]).await;
    let good = sign(&signer, Some("k1"), claims());
    assert_eq!(status(&base, Some(&good)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_keys_refetch_the_jwks_at_most_once_per_period() {
    let signer = key("k1");
    let (url, fetches) = jwks(signer.jwk.clone()).await;
    let base = ekiben(&url, &[RUNNER]).await;
    assert_eq!(
        status(&base, Some(&sign(&signer, Some("k1"), claims()))).await,
        StatusCode::OK
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    for i in 0..5 {
        let token = sign(&key("x"), Some(&format!("unknown-{i}")), claims());
        assert_eq!(status(&base, Some(&token)).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    // A known key needs no fetch.
    assert_eq!(
        status(&base, Some(&sign(&signer, Some("k1"), claims()))).await,
        StatusCode::OK
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}
