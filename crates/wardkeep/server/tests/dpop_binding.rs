#![forbid(unsafe_code)]
//! DPoP at the token endpoint (RFC 9449, W1): a valid proof makes a `DPoP` token bound to the
//! key thumbprint (`cnf.jkt`), a replayed proof or one minted for another URL is refused.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use jc_router::Mount;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wardkeep_crypto::*;
use wardkeep_server::router;

mod common;
use common::*;

const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const HTU: &str = "https://idm.dev.example.test/realms/dev/protocol/openid-connect/token";

struct Holder {
    jwk: Value,
    key: EncodingKey,
}

fn holder() -> Holder {
    let kek = Kek::generate_for_tests();
    let rec = KeyManager::generate_es256("holder", &kek, 100, "ACTIVE").unwrap();
    let der = kek
        .decrypt(
            format!("holder:{}", rec.kid).as_bytes(),
            &rec.encrypted_private_key,
        )
        .unwrap();
    Holder {
        jwk: serde_json::from_str(&rec.public_jwk_json).unwrap(),
        key: EncodingKey::from_ec_der(&der),
    }
}

fn thumbprint(jwk: &Value) -> String {
    let canonical = format!(
        r#"{{"crv":"{}","kty":"EC","x":"{}","y":"{}"}}"#,
        jwk["crv"].as_str().unwrap(),
        jwk["x"].as_str().unwrap(),
        jwk["y"].as_str().unwrap()
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

fn proof(h: &Holder, jti: &str, htu: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".into());
    header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
    let claims = json!({ "jti": jti, "htm": "POST", "htu": htu,
                         "iat": chrono::Utc::now().timestamp() });
    encode(&header, &claims, &h.key).unwrap()
}

async fn token(app: &axum::Router, dpop: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri(TOKEN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(p) = dpop {
        b = b.header("dpop", p);
    }
    let resp = app
        .clone()
        .oneshot(
            b.body(Body::from(
                "grant_type=client_credentials&client_id=apisix-gateway&client_secret=apisix-secret-123",
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn dpop_proof_binds_the_token_and_cannot_be_replayed_or_moved() {
    let (state, _, _) = setup_dev_realm_state().await;
    let app = router(
        state,
        Mount::new("wardkeep", "https", "idm.dev.example.test", "").unwrap(),
    );
    let h = holder();

    // Without a proof: a plain Bearer token, no confirmation claim.
    let (st, body) = token(&app, None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["token_type"], "Bearer");
    let claims = decode_jwt_payload(body["access_token"].as_str().unwrap());
    assert!(claims.get("cnf").is_none());

    // With a proof: a DPoP token bound to the holder's key.
    let p = proof(&h, "proof-1", HTU);
    let (st, body) = token(&app, Some(&p)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["token_type"], "DPoP");
    let claims = decode_jwt_payload(body["access_token"].as_str().unwrap());
    assert_eq!(claims["cnf"]["jkt"], thumbprint(&h.jwk));

    // The same proof a second time.
    let (st, body) = token(&app, Some(&p)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_dpop_proof");

    // A proof minted for another endpoint.
    let (st, body) = token(
        &app,
        Some(&proof(
            &h,
            "proof-2",
            "https://idm.dev.example.test/realms/dev/protocol/openid-connect/userinfo",
        )),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_dpop_proof");

    // A garbage header is an error, never a silent Bearer token.
    let (st, body) = token(&app, Some("not-a-jwt")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_dpop_proof");
}

async fn userinfo(app: &axum::Router, authz: &str, dpop: Option<&str>) -> StatusCode {
    let mut b = Request::builder()
        .method("GET")
        .uri("/realms/dev/protocol/openid-connect/userinfo")
        .header(header::AUTHORIZATION, authz);
    if let Some(p) = dpop {
        b = b.header("dpop", p);
    }
    app.clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

fn userinfo_proof(h: &Holder, jti: &str, method: &str, access_token: Option<&str>) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".into());
    header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
    let mut claims = json!({ "jti": jti, "htm": method,
        "htu": "https://idm.dev.example.test/realms/dev/protocol/openid-connect/userinfo",
        "iat": chrono::Utc::now().timestamp() });
    if let Some(at) = access_token {
        claims["ath"] = json!(URL_SAFE_NO_PAD.encode(Sha256::digest(at.as_bytes())));
    }
    encode(&header, &claims, &h.key).unwrap()
}

#[tokio::test]
async fn a_bound_token_needs_its_key_at_userinfo() {
    let (state, _, _) = setup_dev_realm_state().await;
    let app = router(
        state,
        Mount::new("wardkeep", "https", "idm.dev.example.test", "").unwrap(),
    );
    let h = holder();
    let thief = holder();
    let (st, body) = token(&app, Some(&proof(&h, "tok-1", HTU))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let at = body["access_token"].as_str().unwrap().to_owned();

    // Stolen and replayed as a Bearer token, or under DPoP without a proof: refused.
    assert_eq!(
        userinfo(&app, &format!("Bearer {at}"), None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        userinfo(&app, &format!("DPoP {at}"), None).await,
        StatusCode::UNAUTHORIZED
    );
    // A proof by another key, without the token hash, or for another method: refused.
    for p in [
        userinfo_proof(&thief, "u-1", "GET", Some(&at)),
        userinfo_proof(&h, "u-2", "GET", None),
        userinfo_proof(&h, "u-3", "POST", Some(&at)),
    ] {
        assert_eq!(
            userinfo(&app, &format!("DPoP {at}"), Some(&p)).await,
            StatusCode::UNAUTHORIZED
        );
    }
    // The holder with a fresh proof gets through the binding check; the same proof twice does not.
    let good = userinfo_proof(&h, "u-4", "GET", Some(&at));
    let first = userinfo(&app, &format!("DPoP {at}"), Some(&good)).await;
    assert_eq!(first, StatusCode::OK, "the holder is refused");
    assert_eq!(
        userinfo(&app, &format!("DPoP {at}"), Some(&good)).await,
        StatusCode::UNAUTHORIZED
    );
}
