//! Usage per Endpoint (T-5118): the gateway's client alone posts counts, a reader of an Endpoint
//! reads its usage and nobody else's, and what is kept is names and numbers, never who called.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

mod common;

const GATEWAY_CLIENT: &str = "context-gateway";

async fn state() -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_GATEWAY_CLIENT_ID" => Some(GATEWAY_CLIENT),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let state = AppState::new(config, None);
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    for (space, endpoint) in [("ovzdusie", "air"), ("vnutro", "inner")] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, "mesto", json!({})));
        state.mirror.upsert(envelope(
            "Endpoint",
            endpoint,
            "mesto",
            json!({ "slug": format!("{endpoint}-slug"), "contextSpaceRef": { "kind": "ContextSpace", "name": space } }),
        ));
    }
    state.mirror.upsert(envelope(
        "Role",
        "endpoint-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "endpoint-reader",
                "scope": { "contextSpace": "ovzdusie" } }),
    ));
    state
}

async fn post(state: &AppState, bearer: Option<&str>, body: Value) -> StatusCode {
    let mut request = Request::builder()
        .method("POST")
        .uri("/internal/usage")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", "203.0.113.7");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = server::internal_app(state.clone())
        .oneshot(request.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("answer");
    let status = response.status();
    let _ = response.into_body().collect().await;
    status
}

fn counts(lines: &[(&str, &str, u64)]) -> Value {
    let lines: Vec<Value> = lines
        .iter()
        .map(|(endpoint, surface, count)| json!({ "endpoint": endpoint, "surface": surface, "count": count }))
        .collect();
    json!({ "counts": lines })
}

async fn get(state: &AppState, who: &str, uri: &str) -> common::Answer {
    common::send(state, common::person(who), "GET", uri, None).await
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn only_the_gateways_client_posts_and_a_bad_post_is_refused_whole() {
    let state = state().await;
    let gateway = common::REALM.workload(GATEWAY_CLIENT);
    let good = counts(&[("air-slug", "ngsi-ld", 3)]);
    assert_eq!(
        post(&state, None, good.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &state,
            Some(&common::REALM.workload("some-app")),
            good.clone()
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    let person = common::REALM.person_token_of(common::INTERNAL_AUDIENCE, "portal-ui", "jana", &[]);
    assert_eq!(
        post(&state, Some(&person), good.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    for bad in [
        counts(&[("air-slug", "graphql", 1)]),
        counts(&[("Air Slug", "ogc", 1)]),
        counts(&[("air-slug", "ogc", 10_000_001)]),
        json!({ "counts": [{ "endpoint": "air-slug", "surface": "ogc", "count": 1, "ip": "203.0.113.7" }] }),
    ] {
        assert_eq!(
            post(&state, Some(&gateway), bad).await,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        post(&state, Some(&gateway), good).await,
        StatusCode::NO_CONTENT
    );
    let today = chrono::Utc::now().date_naive();
    let rows = state
        .endpoint_usage
        .read("mesto", today, today)
        .await
        .expect("rows");
    assert_eq!(rows.len(), 1, "the refused posts stored nothing: {rows:?}");
}

#[tokio::test]
async fn a_reader_sees_the_counts_of_the_endpoints_they_may_read_and_no_other() {
    let state = state().await;
    let gateway = common::REALM.workload(GATEWAY_CLIENT);
    let lines = counts(&[
        ("air-slug", "ngsi-ld", 3),
        ("air-slug", "download", 1),
        ("inner-slug", "ogc", 4),
        ("ghost-slug", "ogc", 9),
    ]);
    assert_eq!(
        post(&state, Some(&gateway), lines.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(&state, Some(&gateway), lines).await,
        StatusCode::NO_CONTENT
    );

    let one = get(
        &state,
        "jana",
        "/api/v1/projects/mesto/endpoints/air/usage?days=7",
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let one = json_of(&one.text);
    assert_eq!(one["days"].as_array().map(Vec::len), Some(7));
    assert_eq!(one["total"], 8);
    assert_eq!(one["totals"], json!({ "download": 2, "ngsi-ld": 6 }));
    assert_eq!(one["days"][6]["total"], 8);

    for (who, uri) in [
        ("jana", "/api/v1/projects/mesto/endpoints/inner/usage"),
        ("jana", "/api/v1/projects/mesto/endpoints/ghost/usage"),
        ("eva", "/api/v1/projects/mesto/endpoints/air/usage"),
        ("eva", "/api/v1/projects/mesto/usage/endpoints"),
        (
            "eva",
            "/api/v1/projects/mesto/usage/endpoints/export?month=2026-10",
        ),
    ] {
        assert_eq!(
            get(&state, who, uri).await.status,
            StatusCode::NOT_FOUND,
            "{who} {uri}"
        );
    }
    for uri in [
        "/api/v1/projects/mesto/endpoints/air/usage?days=0",
        "/api/v1/projects/mesto/endpoints/air/usage?days=91",
        "/api/v1/projects/mesto/usage/endpoints?since=1",
        "/api/v1/projects/mesto/usage/endpoints/export?month=2026-13",
        "/api/v1/projects/mesto/usage/endpoints/export",
    ] {
        assert_eq!(
            get(&state, "jana", uri).await.status,
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }

    let list = get(&state, "jana", "/api/v1/projects/mesto/usage/endpoints").await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text);
    let list = json_of(&list.text);
    let endpoints = list["endpoints"].as_array().cloned().unwrap_or_default();
    assert_eq!(endpoints.len(), 1, "{list}");
    assert_eq!(
        endpoints.first().map(|e| e["name"].clone()),
        Some(json!("air"))
    );
    assert_eq!(
        endpoints
            .first()
            .map(|e| e["daily"].as_array().map(Vec::len)),
        Some(Some(30))
    );
}

/// T-6186: the calls of each kind of caller add up per Endpoint, an older gateway's line without a
/// kind is `unknown`, and a kind the Portal does not know refuses the post whole.
#[tokio::test]
async fn the_usage_splits_by_kind_of_caller() {
    let state = state().await;
    let gateway = common::REALM.workload(GATEWAY_CLIENT);
    let line = |caller: &str, count: u64| json!({ "endpoint": "air-slug", "surface": "mcp", "caller": caller, "count": count });
    let lines = json!({ "counts": [
        line("person", 2),
        line("agent", 5),
        line("anonymous", 1),
        { "endpoint": "air-slug", "surface": "feed", "count": 3 },
    ] });
    assert_eq!(
        post(&state, Some(&gateway), lines).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(
            &state,
            Some(&gateway),
            json!({ "counts": [line("robot", 1)] })
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    let one = get(&state, "jana", "/api/v1/projects/mesto/endpoints/air/usage").await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let one = json_of(&one.text);
    assert_eq!(one["total"], 11);
    assert_eq!(
        one["callers"],
        json!({ "agent": 5, "anonymous": 1, "person": 2, "unknown": 3 })
    );
    let list = get(
        &state,
        "jana",
        "/api/v1/projects/mesto/usage/endpoints?days=7",
    )
    .await;
    let list = json_of(&list.text);
    assert_eq!(list["endpoints"][0]["callers"]["agent"], 5, "{list}");
}

/// What leaves the store is a day, an Endpoint, a surface and a number: the caller's address and
/// identity reached the Portal with the post and are in no row.
#[tokio::test]
async fn the_export_holds_names_and_numbers_and_never_who_called() {
    let state = state().await;
    let gateway = common::REALM.workload(GATEWAY_CLIENT);
    assert_eq!(
        post(
            &state,
            Some(&gateway),
            counts(&[("air-slug", "mcp", 2), ("inner-slug", "ogc", 1)])
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let today = chrono::Utc::now().date_naive();
    let month = today.format("%Y-%m").to_string();
    let csv = get(
        &state,
        "jana",
        &format!("/api/v1/projects/mesto/usage/endpoints/export?month={month}"),
    )
    .await;
    assert_eq!(csv.status, StatusCode::OK, "{}", csv.text);
    assert_eq!(
        csv.text,
        format!("day,endpoint,surface,calls\n{today},air,mcp,2\n")
    );
    for leaked in ["203.0.113.7", "jana", "sub-", GATEWAY_CLIENT] {
        assert!(!csv.text.contains(leaked), "{leaked} in {}", csv.text);
    }
}

/// The Postgres rows add up across posts, a day is kept 400 days, and a read is the project's
/// alone. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to.
#[tokio::test]
async fn the_postgres_rows_add_up_and_age_out() {
    use joinedcontext_portal::endpoint_usage::{Stored, UsageStore, KEEP_DAYS};
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    sqlx::query("DELETE FROM endpoint_usage_daily WHERE project LIKE 't5118%'")
        .execute(&pool)
        .await
        .expect("clean");
    let store = UsageStore::new(Some(pool));
    let today: chrono::NaiveDate = "2026-10-22".parse().expect("a day");
    let old = today - chrono::Duration::days(KEEP_DAYS + 1);
    let key = |project: &str, day| {
        (
            project.to_owned(),
            "air".to_owned(),
            "ogc".to_owned(),
            day,
            "person".to_owned(),
        )
    };
    let rows = std::collections::BTreeMap::from([
        (key("t5118", today), 2),
        (key("t5118", old), 9),
        (key("t5118b", today), 4),
    ]);
    store.add(rows, old).await.expect("first post");
    store
        .add(
            std::collections::BTreeMap::from([(key("t5118", today), 3)]),
            today,
        )
        .await
        .expect("second post");
    let read = store.read("t5118", old, today).await.expect("read");
    assert_eq!(
        read,
        vec![Stored {
            name: "air".to_owned(),
            surface: "ogc".to_owned(),
            day: today,
            caller: "person".to_owned(),
            calls: 5
        }]
    );
}
