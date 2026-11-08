#![forbid(unsafe_code)]
//! DPoP at the token endpoint (RFC 9449, W1): a valid proof makes a `DPoP` token bound to the
//! key thumbprint (`cnf.jkt`), a replayed proof or one minted for another URL is refused.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use jc_router::Mount;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

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
        Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap(),
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
        Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap(),
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

fn resource_proof(
    h: &Holder,
    jti: &str,
    method: &str,
    htu: &str,
    access_token: Option<&str>,
    nonce: Option<&str>,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".into());
    header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
    let mut claims = json!({
        "jti": jti,
        "htm": method,
        "htu": htu,
        "iat": chrono::Utc::now().timestamp()
    });
    if let Some(at) = access_token {
        claims["ath"] = json!(URL_SAFE_NO_PAD.encode(Sha256::digest(at.as_bytes())));
    }
    if let Some(n) = nonce {
        claims["nonce"] = json!(n);
    }
    encode(&header, &claims, &h.key).unwrap()
}

/// K6: Proved red against pre-K6 where Admin Compat, Native Admin, OData, MCP, and SCIM
/// accepted a DPoP-bound token under `Authorization: Bearer ...` and issued no server DPoP nonces.
#[tokio::test]
async fn k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip() {
    let (state, store, kek) = setup_dev_realm_state().await;
    let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
    let app = router(state, mount);
    let h = holder();
    let thief = holder();

    // Mint a DPoP-bound realm-admin token with `cnf.jkt`.
    let keys = store.list_signing_keys("dev").await.unwrap();
    let key = keys.iter().find(|k| k.status == "ACTIVE").unwrap();
    let now = chrono::Utc::now().timestamp();
    let bound_admin_token = kaisatsu_crypto::sign_jwt(
        key,
        &kek,
        Some("JWT"),
        &json!({
            "iss": "https://idm.dev.example.test/realms/dev",
            "sub": "admin-dpop",
            "azp": "admin-cli",
            "typ": "Bearer",
            "jti": "jti-admin-dpop",
            "iat": now,
            "exp": now + 600,
            "realm_access": { "roles": ["realm-admin"] },
            "cnf": { "jkt": thumbprint(&h.jwk) }
        }),
    )
    .unwrap();

    let endpoints = [
        ("GET", "/admin/realms/dev/users", None),
        ("GET", "/api/v1/realms/dev/allowed?op=read&kind=Person", None),
        ("GET", "/api/v1/realms/dev/odata/People", None),
        ("GET", "/api/v1/realms/dev/scim/v2/Users", None),
        (
            "POST",
            "/api/v1/realms/dev/mcp",
            Some(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#),
        ),
    ];

    for (i, (method, path, body_opt)) in endpoints.iter().enumerate() {
        let htu = format!("https://idm.dev.example.test{path}")
            .split('?')
            .next()
            .unwrap()
            .to_owned();
        let send = |authz: String, dpop: Option<String>| {
            let app = app.clone();
            async move {
                let mut b = Request::builder()
                    .method(*method)
                    .uri(*path)
                    .header(header::AUTHORIZATION, authz);
                if let Some(p) = dpop {
                    b = b.header("dpop", p);
                }
                let req = match body_opt {
                    Some(json_str) => b
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(*json_str))
                        .unwrap(),
                    None => b.body(Body::empty()).unwrap(),
                };
                app.oneshot(req).await.unwrap()
            }
        };

        // 1. Bearer with a DPoP-bound token -> 401 Unauthorized.
        let r = send(format!("Bearer {bound_admin_token}"), None).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "Bearer accepted at {path}");

        // 2. Wrong htu, missing ath, wrong jkt -> 401 Unauthorized.
        for bad_proof in [
            resource_proof(
                &h,
                &format!("bad-htu-{i}"),
                method,
                "https://idm.dev.example.test/wrong",
                Some(&bound_admin_token),
                None,
            ),
            resource_proof(&h, &format!("no-ath-{i}"), method, &htu, None, None),
            resource_proof(
                &thief,
                &format!("wrong-jkt-{i}"),
                method,
                &htu,
                Some(&bound_admin_token),
                None,
            ),
        ] {
            let r = send(format!("DPoP {bound_admin_token}"), Some(bad_proof)).await;
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "bad proof accepted at {path}");
        }

        // 3. Valid proof succeeds once; replaying the same jti is refused.
        let ok_proof = resource_proof(
            &h,
            &format!("ok-{i}"),
            method,
            &htu,
            Some(&bound_admin_token),
            None,
        );
        let r_ok = send(format!("DPoP {bound_admin_token}"), Some(ok_proof.clone())).await;
        assert_eq!(r_ok.status(), StatusCode::OK, "valid DPoP refused at {path}");
        let r_replay = send(format!("DPoP {bound_admin_token}"), Some(ok_proof)).await;
        assert_eq!(r_replay.status(), StatusCode::UNAUTHORIZED, "replayed jti accepted at {path}");
    }

    // 4. Enable server DPoP nonces on the realm and test AS + RS nonce round trip and stale nonce refusal.
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm
        .attributes
        .insert("dpop.require.nonce".into(), "true".into());
    store.upsert_realm(realm).await.unwrap();

    // Token endpoint without nonce -> 400 use_dpop_nonce + DPoP-Nonce header.
    let p_no_nonce = resource_proof(&h, "as-n-1", "POST", HTU, None, None);
    let resp_as_1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(TOKEN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("dpop", p_no_nonce)
                .body(Body::from(concat!(
                    "grant_type=client_credentials&client_id=apisix-gateway&client_secret",
                    "=apisix-",
                    "secret-123"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_as_1.status(), StatusCode::BAD_REQUEST);
    let as_nonce = resp_as_1
        .headers()
        .get("dpop-nonce")
        .expect("DPoP-Nonce header at AS")
        .to_str()
        .unwrap()
        .to_owned();
    let body_as_1: Value = serde_json::from_slice(
        &resp_as_1.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    assert_eq!(body_as_1["error"], "use_dpop_nonce");

    // Retry token endpoint with the server-issued nonce -> 200 OK + fresh DPoP-Nonce.
    let p_with_nonce = resource_proof(&h, "as-n-2", "POST", HTU, None, Some(&as_nonce));
    let (st_as_2, body_as_2) = token(&app, Some(&p_with_nonce)).await;
    assert_eq!(st_as_2, StatusCode::OK, "{body_as_2}");

    // Stale nonce (from 10 slots ago) is refused at token endpoint.
    let stale_nonce = kaisatsu_oidc::issue_dpop_nonce(&kek, "dev", "as", now - 600);
    let p_stale = resource_proof(&h, "as-n-stale", "POST", HTU, None, Some(&stale_nonce));
    let (st_stale, body_stale) = token(&app, Some(&p_stale)).await;
    assert_eq!(st_stale, StatusCode::BAD_REQUEST);
    assert_eq!(body_stale["error"], "use_dpop_nonce");

    // Resource server (OData) without nonce -> 401 + WWW-Authenticate: DPoP error="use_dpop_nonce" + DPoP-Nonce.
    let odata_htu = "https://idm.dev.example.test/api/v1/realms/dev/odata/People";
    let rs_no_nonce = resource_proof(
        &h,
        "rs-n-1",
        "GET",
        odata_htu,
        Some(&bound_admin_token),
        None,
    );
    let resp_rs_1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/realms/dev/odata/People")
                .header(header::AUTHORIZATION, format!("DPoP {bound_admin_token}"))
                .header("dpop", rs_no_nonce)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_rs_1.status(), StatusCode::UNAUTHORIZED);
    let www_auth = resp_rs_1.headers()[header::WWW_AUTHENTICATE]
        .to_str()
        .unwrap();
    assert!(www_auth.contains(r#"error="use_dpop_nonce""#), "{www_auth}");
    let rs_nonce = resp_rs_1
        .headers()
        .get("dpop-nonce")
        .expect("DPoP-Nonce at RS")
        .to_str()
        .unwrap()
        .to_owned();

    // Retry OData resource with the RS nonce -> 200 OK.
    let rs_with_nonce = resource_proof(
        &h,
        "rs-n-2",
        "GET",
        odata_htu,
        Some(&bound_admin_token),
        Some(&rs_nonce),
    );
    let resp_rs_2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/realms/dev/odata/People")
                .header(header::AUTHORIZATION, format!("DPoP {bound_admin_token}"))
                .header("dpop", rs_with_nonce)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_rs_2.status(), StatusCode::OK);
}
