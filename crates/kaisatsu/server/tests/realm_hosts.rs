#![forbid(unsafe_code)]
//! Realms pinned to a sign-in host of their own (T-7001): the issuer comes from the registry,
//! never from the request's host, and a pinned realm answers nowhere else.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_server::{RealmHosts, router};
use serde_json::Value;
use tower::ServiceExt;

mod common;
use common::*;

const DISCOVERY: &str = "/realms/dev/.well-known/openid-configuration";

fn mount(host: &str) -> Mount {
    Mount::new("kaisatsu", "https", host, "").unwrap()
}

async fn app(hosts: RealmHosts) -> Router {
    let (state, _, _) = setup_dev_realm_state().await;
    router(state.with_realm_hosts(hosts), mount("idm.dev.example.test"))
}

/// GET `path` as jc-router hands it over for a request on `host`.
async fn get(app: &Router, host: &str, path: &str) -> (StatusCode, String) {
    let req = Request::get(path).body(Body::empty()).unwrap();
    let resp = app
        .clone()
        .layer(Extension(mount(host)))
        .oneshot(req)
        .await
        .unwrap();
    let st = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (st, String::from_utf8_lossy(&body).into_owned())
}

fn issuer(body: &str) -> String {
    let v: Value = serde_json::from_str(body).expect("discovery json");
    v["issuer"].as_str().expect("issuer").to_owned()
}

fn pinned() -> RealmHosts {
    RealmHosts::default()
        .pin("dev", mount("idm.bbsk.sk"))
        .internal("localhost:8080")
}

#[tokio::test]
async fn discovery_on_the_sign_in_host_names_the_registry_issuer() {
    let app = app(pinned()).await;
    for host in ["idm.bbsk.sk", "idm.bbsk.sk:443", "localhost:8080"] {
        let (st, body) = get(&app, host, DISCOVERY).await;
        assert_eq!(st, StatusCode::OK, "{host}");
        assert_eq!(issuer(&body), "https://idm.bbsk.sk/realms/dev", "{host}");
        assert!(
            !body.contains("localhost"),
            "no endpoint names the request's host: {body}"
        );
    }
}

#[tokio::test]
async fn a_pinned_realm_is_404_under_any_other_host() {
    let app = app(pinned()).await;
    for host in ["idm.hel.fi", "login.city.sk", "evil.example"] {
        for path in [
            DISCOVERY,
            "/.well-known/openid-configuration/realms/dev",
            "/realms/dev/protocol/openid-connect/auth?client_id=portal-api",
            "/realms/dev/protocol/openid-connect/certs",
        ] {
            let (st, body) = get(&app, host, path).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{host}{path}");
            assert!(!body.contains("idm."), "{host}{path}: {body}");
        }
    }
    let (st, _) = get(
        &app,
        "localhost:8080",
        "/realms/dev/protocol/openid-connect/auth?client_id=portal-api",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "no browser sign-in on the internal host"
    );
}

#[tokio::test]
async fn an_unpinned_realm_answers_byte_identical_beside_a_pinned_one() {
    let plain = app(RealmHosts::default()).await;
    let beside = app(RealmHosts::default().pin("bbsk", mount("idm.bbsk.sk"))).await;
    let (st, before) = get(&plain, "idm.dev.example.test", DISCOVERY).await;
    assert_eq!(st, StatusCode::OK);
    let (_, after) = get(&beside, "idm.dev.example.test", DISCOVERY).await;
    assert_eq!(before, after);
    let (st, _) = get(&beside, "idm.bbsk.sk", DISCOVERY).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "idm.bbsk.sk serves realm bbsk alone"
    );
}
