#![forbid(unsafe_code)]
//! A present credential that names nobody is refused 401, as Grafana refuses an invalid API key
//! or password, and never read as anonymous: a bearer the host's verifier refuses (a forged
//! signature, another issuer, another audience, an expired token), an unknown API key, a wrong
//! Basic pair. No header at all stays anonymous.

mod common;

use axum::body::Body;
use base64::Engine as _;
use common::{ORG, world};
use daiya_model::Subject;
use http::{HeaderMap, Request, StatusCode};
use serde_json::{Value, json};

const ISSUER: &str = "https://kaisatsu.example/realms/jc";
const AUDIENCE: &str = "daiya";
const SIGNATURE: &str = "signed-by-the-realm";
const NOW: i64 = 1_794_117_600;

fn jwt(claims: &Value, signature: &str) -> String {
    let part = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.{signature}",
        part(&json!({"alg": "RS256"})),
        part(claims)
    )
}

fn claims(iss: &str, aud: &str, exp: i64) -> Value {
    json!({"sub": "person-1", "iss": iss, "aud": aud, "exp": exp})
}

/// A stand-in for the host's verifier: the realm's signature, issuer, audience and expiry.
fn verify(headers: &HeaderMap) -> Option<Subject> {
    let token = headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let mut parts = token.split('.');
    let (_, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?,
    )
    .ok()?;
    (signature == SIGNATURE
        && claims["iss"] == ISSUER
        && claims["aud"] == AUDIENCE
        && claims["exp"].as_i64()? > NOW)
        .then(|| Subject::authenticated("person-1", vec![ORG], vec![]))
}

async fn folders(authorization: Option<String>) -> (StatusCode, Value) {
    let w = world();
    let w = common::World {
        app: daiya_api::api_router(
            w.state.clone().with_subject_resolver(verify),
            jc_router::Mount::new("daiya", "https", "x.example", "").unwrap(),
        ),
        ..w
    };
    let mut req = Request::builder().method("GET").uri("/api/folders");
    if let Some(value) = authorization {
        req = req.header("authorization", value);
    }
    w.send(req.body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn a_verified_bearer_reads_and_no_credential_stays_anonymous() {
    let good = jwt(&claims(ISSUER, AUDIENCE, NOW + 300), SIGNATURE);
    assert_eq!(
        folders(Some(format!("Bearer {good}"))).await.0,
        StatusCode::OK
    );
    assert_eq!(folders(None).await, (StatusCode::OK, json!([])));
    // An API key daiya holds still reads.
    assert_eq!(
        folders(Some("Bearer viewer".into())).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_present_bearer_the_verifier_refuses_is_401_never_anonymous() {
    let cases = [
        (
            "forged signature",
            jwt(&claims(ISSUER, AUDIENCE, NOW + 300), "forged"),
        ),
        (
            "another issuer",
            jwt(
                &claims("https://evil.example", AUDIENCE, NOW + 300),
                SIGNATURE,
            ),
        ),
        (
            "another audience",
            jwt(&claims(ISSUER, "portal", NOW + 300), SIGNATURE),
        ),
        (
            "expired",
            jwt(&claims(ISSUER, AUDIENCE, NOW - 1), SIGNATURE),
        ),
        ("not a token", "garbage".to_owned()),
    ];
    for (case, token) in cases {
        let (status, body) = folders(Some(format!("Bearer {token}"))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}: {body}");
        // The refusal names no reason and never repeats the token.
        assert!(!body.to_string().contains(&token), "{case}: {body}");
    }
}

#[tokio::test]
async fn a_wrong_basic_pair_or_an_unknown_scheme_is_401() {
    let wrong = base64::engine::general_purpose::STANDARD.encode("admin:not-the-password");
    for value in [
        format!("Basic {wrong}"),
        "Digest x".to_owned(),
        "Bearer".to_owned(),
    ] {
        let (status, body) = folders(Some(value.clone())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{value}: {body}");
    }
}

#[tokio::test]
async fn every_surface_refuses_before_reading_the_caller() {
    let w = world();
    for uri in ["/api/search", "/d/any", "/api/dashboards/uid/any"] {
        let (status, _) = w.call("GET", uri, Some("forged"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}
