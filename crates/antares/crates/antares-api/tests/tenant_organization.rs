// SPDX-License-Identifier: EUPL-1.2
//! The Organization check (EP-188, PF-130, T-7036): under
//! `ANTARES_TENANT_ORGANIZATION=enforce` a gateway request serves a Tenant
//! `{organization id}__{segment}` only for the Organization the gateway
//! vouched, and a refusal is the exact answer an unknown Tenant gets.

use antares_api::admin_auth::AdminAuth;
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// A member of Organization `org`, as the gateway sends one.
fn member(org: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("x-jc-subject", "someone"),
        ("x-jc-member", "true"),
        ("x-jc-organization-id", org),
    ]
}

/// A caller the Endpoint admits without membership (public, grant,
/// agreement) on a space of Organization `space_org`.
fn visitor(space_org: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("x-jc-subject", "someone"),
        ("x-jc-space-organization", space_org),
    ]
}

async fn state(enforce: bool) -> AppState {
    antares_api::policy::default_subject_headers("x-jc-subject,x-jc-member");
    let mut st = AppState::new("tenant-organization".into())
        .with_admin_auth(AdminAuth::new(SECRET, false).expect("secret"));
    antares_api::wire(&mut st).await;
    st.subject_required = true;
    st.tenant_organization = enforce;
    st
}

/// Status, the echoed `NGSILD-Tenant` and the body bytes.
async fn send(
    st: &AppState,
    method: &str,
    path: &str,
    tenant: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(t) = tenant {
        b = b.header("NGSILD-Tenant", t);
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = body.map(|v| v.to_string()).unwrap_or_default();
    let req = b
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let echo = res
        .headers()
        .get("NGSILD-Tenant")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, echo, bytes.to_vec())
}

fn room(id: &str) -> Value {
    json!({
        "id": id, "type": "Room",
        "temperature": {"type": "Property", "value": 21.5,
                        "observedAt": "2026-01-01T00:00:00Z"}
    })
}

/// Every operation of the task: (method, path, body). The creating ones
/// would make the Tenant implicitly.
fn operations() -> Vec<(&'static str, &'static str, Option<Value>)> {
    vec![
        ("GET", "/ngsi-ld/v1/entities?type=Room", None),
        (
            "POST",
            "/ngsi-ld/v1/entities",
            Some(room("urn:ngsi-ld:Room:2")),
        ),
        ("GET", "/ngsi-ld/v1/temporal/entities?type=Room", None),
        (
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            Some(room("urn:ngsi-ld:Room:3")),
        ),
        ("GET", "/ngsi-ld/v1/subscriptions", None),
        (
            "POST",
            "/ngsi-ld/v1/subscriptions",
            Some(json!({
                "type": "Subscription", "entities": [{"type": "Room"}],
                "notification": {"endpoint": {"uri": "http://127.0.0.1:1/n"}}
            })),
        ),
        ("GET", "/ngsi-ld/v1/csourceRegistrations?type=Room", None),
        (
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(json!({
                "type": "ContextSourceRegistration",
                "information": [{"entities": [{"type": "Room"}]}],
                "endpoint": "http://127.0.0.1:1"
            })),
        ),
    ]
}

/// Red before T-7036: a member of `sk_a` read and wrote `sk_b__x`. Now every
/// operation answers byte for byte what the unknown `sk_b__x` answered its
/// own member's read, and no creating operation makes the Tenant.
#[tokio::test]
async fn another_organizations_tenant_reads_as_unknown_for_every_operation() {
    let st = state(true).await;
    let owner = member("sk_b");
    let intruder = member("sk_a");
    let unknown = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        Some("sk_b__x"),
        &owner,
        None,
    )
    .await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND, "nothing there yet");
    assert_eq!(unknown.1.as_deref(), Some("sk_b__x"));

    // While the tenant does not exist: the creates create nothing.
    for (method, path, body) in operations() {
        let refused = send(&st, method, path, Some("sk_b__x"), &intruder, body).await;
        assert_eq!(refused, unknown, "{method} {path}");
    }
    let still = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        Some("sk_b__x"),
        &owner,
        None,
    )
    .await;
    assert_eq!(still, unknown, "a refused create made the Tenant");

    // Once it exists, the refusal still matches the unknown answer.
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some("sk_b__x"),
        &owner,
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    for (method, path, body) in operations() {
        let refused = send(&st, method, path, Some("sk_b__x"), &intruder, body).await;
        assert_eq!(refused, unknown, "{method} {path}");
    }
    let (s, _, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        Some("sk_b__x"),
        &owner,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rooms: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(rooms.as_array().map(Vec::len), Some(1), "{rooms}");
}

/// The compare is after the split: `sk_a` is no prefix of `sk_ab__x`, and
/// the member rule reads `x-jc-organization-id` while every other caller is
/// held to `x-jc-space-organization`.
#[tokio::test]
async fn the_organization_is_compared_whole_and_by_the_callers_rule() {
    let st = state(true).await;
    let path = "/ngsi-ld/v1/entities?type=Room";
    let get = |tenant: &'static str, headers: Vec<(&'static str, &'static str)>| {
        let st = st.clone();
        async move { send(&st, "GET", path, Some(tenant), &headers, None).await.0 }
    };
    assert_eq!(get("sk_ab__x", member("sk_a")).await, StatusCode::NOT_FOUND);
    assert_eq!(get("sk_a__x", member("sk_ab")).await, StatusCode::NOT_FOUND);
    // a visitor of a space of sk_b reaches it: the Policy decides from here
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some("sk_b__x"),
        &member("sk_b"),
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(get("sk_b__x", visitor("sk_b")).await, StatusCode::OK);
    assert_eq!(get("sk_b__x", visitor("sk_a")).await, StatusCode::NOT_FOUND);
    // a project-key Tenant compares its OrgId half the same way (ARCH/TEN-10)
    const KEYED: &str = "apniwb5dkdjn5j4ik4hv7zugza__d3d2ouepc7jkdlbvzor5vezejv__x";
    const ORG: &str = "apniwb5dkdjn5j4ik4hv7zugza";
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(KEYED),
        &member(ORG),
        Some(room("urn:ngsi-ld:Room:2")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(get(KEYED, visitor(ORG)).await, StatusCode::OK);
    assert_eq!(get(KEYED, member("sk_b")).await, StatusCode::NOT_FOUND);
    assert_eq!(
        get(KEYED, visitor("bpniwb5dkdjn5j4ik4hv7zugza")).await,
        StatusCode::NOT_FOUND
    );
    // a member is held to its own Organization, not to the space's header
    let mut both = member("sk_a");
    both.push(("x-jc-space-organization", "sk_b"));
    assert_eq!(get("sk_b__x", both).await, StatusCode::NOT_FOUND);
    // neither header: refused
    assert_eq!(
        get("sk_b__x", vec![("x-jc-subject", "someone")]).await,
        StatusCode::NOT_FOUND
    );
    // two values of the vouched id are no value
    let mut twice = member("sk_b");
    twice.push(("x-jc-organization-id", "sk_b"));
    assert_eq!(get("sk_b__x", twice).await, StatusCode::NOT_FOUND);
}

/// A gateway request names a Tenant of the platform's shape, and names one:
/// anything else is the 400 a malformed Tenant gets.
#[tokio::test]
async fn a_gateway_request_without_an_organization_tenant_is_malformed() {
    let st = state(true).await;
    let path = "/ngsi-ld/v1/entities?type=Room";
    for tenant in [
        None,
        Some("plain"),
        Some("sk_b__x_y"),
        Some("__x"),
        Some("sk__x"),
    ] {
        let (s, _, body) = send(&st, "GET", path, tenant, &member("sk_b"), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{tenant:?}");
        let body: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            body["type"],
            "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
        );
    }
}

/// The admin bearer is the operator's own: unchecked beyond the shape of a
/// named Tenant. Off, a member of another Organization is served as before.
#[tokio::test]
async fn the_admin_bearer_and_the_off_switch_are_unaffected() {
    let st = state(true).await;
    let bearer = format!("Bearer {SECRET}");
    let admin = [("authorization", bearer.as_str())];
    let path = "/ngsi-ld/v1/entities";
    let (s, ..) = send(
        &st,
        "POST",
        path,
        Some("sk_b__x"),
        &admin,
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, ..) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        None,
        &admin,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "the default tenant, named by no header");
    let (s, ..) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        Some("plain"),
        &admin,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let off = state(false).await;
    let (s, ..) = send(
        &off,
        "POST",
        path,
        Some("sk_b__x"),
        &member("sk_a"),
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "off: as before");
    let (s, ..) = send(
        &off,
        "GET",
        "/ngsi-ld/v1/entities?type=Room",
        Some("plain"),
        &member("sk_a"),
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "off: a plain unknown tenant as before"
    );
}

/// The view root asks the same rule on the view's Tenant (EP-188): a caller
/// of another Organization finds no view, a member and a lookup outside a
/// request do.
#[tokio::test]
async fn a_view_of_another_organizations_tenant_is_not_admitted() {
    let st = state(true).await;
    assert!(
        antares_api::admin_auth::view_admits(&st, "sk_b__x"),
        "no request in scope: a lookup this process makes"
    );
    let probe = |headers: Vec<(&'static str, &'static str)>| {
        let st = st.clone();
        async move {
            let app = axum::Router::new()
                .route(
                    "/probe",
                    axum::routing::get(
                        |axum::extract::State(st): axum::extract::State<AppState>| async move {
                            if antares_api::admin_auth::view_admits(&st, "sk_b__x") {
                                "admitted"
                            } else {
                                "refused"
                            }
                        },
                    ),
                )
                .layer(axum::middleware::from_fn_with_state(
                    st.clone(),
                    antares_api::admin_auth::view_organization_layer,
                ))
                .with_state(st);
            let mut b = Request::builder().uri("/probe");
            for (k, v) in headers {
                b = b.header(k, v);
            }
            let res = app
                .oneshot(b.body(Body::empty()).expect("request"))
                .await
                .expect("response");
            let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .expect("body");
            String::from_utf8(bytes.to_vec()).expect("utf-8")
        }
    };
    assert_eq!(probe(member("sk_b")).await, "admitted");
    assert_eq!(probe(visitor("sk_b")).await, "admitted");
    assert_eq!(probe(member("sk_a")).await, "refused");
    assert_eq!(probe(visitor("sk_a")).await, "refused");
}

/// A 128-character Tenant, the longest `{organization id}__{segment}`, is
/// written and read back.
#[tokio::test]
async fn the_longest_tenant_round_trips() {
    let st = state(true).await;
    let org: &'static str = Box::leak(format!("{}_b", "a".repeat(61)).into_boxed_str());
    let tenant = format!("{org}__{}", "c".repeat(63));
    assert_eq!(tenant.len(), 128);
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(&tenant),
        &member(org),
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, echo, _) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Room:1",
        Some(&tenant),
        &member(org),
        None,
    )
    .await;
    assert_eq!(
        (s, echo.as_deref()),
        (StatusCode::OK, Some(tenant.as_str()))
    );
}

mod common;

/// `st` asking a [`common::Double`] over its own store, which a test tells
/// what `tenant_standing` answers (T-7042).
fn doubled(st: &mut AppState) -> std::sync::Arc<common::Double> {
    let double = std::sync::Arc::new(common::Double::passthrough(st.store.clone()));
    st.store = double.clone();
    double
}

/// Design 10.5: a row whose stored `organization_id` is not its name's is an
/// unknown Tenant to its own name's members too, so no path that rewrote the
/// row reassigns the Tenant.
#[tokio::test]
async fn a_stored_organization_that_disagrees_with_the_name_refuses() {
    let mut st = state(true).await;
    let double = doubled(&mut st);
    let owner = member("sk_b");
    let read = "/ngsi-ld/v1/entities?type=Room";
    let unknown = send(&st, "GET", read, Some("sk_b__x"), &owner, None).await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some("sk_b__x"),
        &owner,
        Some(room("urn:ngsi-ld:Room:1")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let agrees = antares_store::TenantStanding {
        tombstoned: false,
        organization: Some("sk_b".into()),
    };
    double.set_standing("sk_b__x", agrees);
    let (s, ..) = send(&st, "GET", read, Some("sk_b__x"), &owner, None).await;
    assert_eq!(s, StatusCode::OK, "the stored id agrees");
    double.set_standing(
        "sk_b__x",
        antares_store::TenantStanding {
            tombstoned: false,
            organization: Some("sk_c".into()),
        },
    );
    for (method, path, body) in operations() {
        let refused = send(&st, method, path, Some("sk_b__x"), &owner, body).await;
        assert_eq!(refused, unknown, "{method} {path}");
    }
}

/// Design 10.1: a name the rename moved away is neither served nor created,
/// by a member or by the admin bearer, and answers as an unknown Tenant.
#[tokio::test]
async fn a_tombstoned_name_is_neither_served_nor_created() {
    let mut st = state(true).await;
    let double = doubled(&mut st);
    let owner = member("sk_b");
    let read = "/ngsi-ld/v1/entities?type=Room";
    let unknown = send(&st, "GET", read, Some("sk_b__old"), &owner, None).await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    double.set_standing(
        "sk_b__old",
        antares_store::TenantStanding {
            tombstoned: true,
            organization: None,
        },
    );
    let bearer = format!("Bearer {SECRET}");
    let admin = [("authorization", bearer.as_str())];
    for caller in [&owner[..], &admin[..]] {
        for (method, path, body) in operations() {
            let refused = send(&st, method, path, Some("sk_b__old"), caller, body).await;
            assert_eq!(refused, unknown, "{method} {path}");
        }
    }
    let old = antares_model::TenantId::new("sk_b__old").expect("tenant");
    assert_eq!(
        st.store.tenant_exists(&old).await.ok(),
        Some(false),
        "created nothing"
    );
}

/// Remote-notify (CIM 009 clause 5.8.1, step 4) acts for the Tenant its inbound index stored: a
/// gateway request vouching another Organization gets the answer of a
/// mapping this broker does not hold.
#[tokio::test]
async fn remote_notify_for_another_organizations_tenant_reads_as_an_unknown_mapping() {
    let st = state(true).await;
    let owner = member("sk_b");
    let (s, ..) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some("sk_b__x"),
        &owner,
        Some(json!({
            "id": "urn:ngsi-ld:Subscription:own", "type": "Subscription",
            "entities": [{"type": "Room"}],
            "notification": {"endpoint": {"uri": "http://127.0.0.1:1/n"}}
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let index = antares_model::TenantId::new_internal("distsub-index").expect("index");
    let remote = "urn:ngsi-ld:Subscription:remote1";
    let map = || {
        st.store.create(
            &index,
            antares_store::Kind::DistSub,
            remote,
            json!({"tenant": "sk_b__x", "own": "urn:ngsi-ld:Subscription:own"}),
        )
    };
    map().await.expect("mapping");
    let note = json!({"id": "urn:ngsi-ld:Notification:1", "type": "Notification",
                      "subscriptionId": remote, "notifiedAt": "2026-01-01T00:00:00Z",
                      "data": []});
    let notify = |caller: Vec<(&'static str, &'static str)>| {
        let (st, note) = (st.clone(), note.clone());
        async move {
            send(
                &st,
                "POST",
                "/ex/v1/remote-notify",
                None,
                &caller,
                Some(note),
            )
            .await
        }
    };
    let served = notify(owner.clone()).await;
    assert_ne!(
        served.0,
        StatusCode::NOT_FOUND,
        "the owner's notification is taken"
    );
    let refused = notify(member("sk_a")).await;
    st.store
        .delete(&index, antares_store::Kind::DistSub, remote)
        .await
        .expect("unmap");
    let unknown = notify(member("sk_a")).await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    assert_eq!(refused, unknown);
}

/// ADR-0031 channels: binding, reading and listing on another
/// Organization's Tenant answer as an unknown Tenant, and under `enforce` an
/// absent Tenant answers the same there, so the refusal tells nothing.
#[cfg(feature = "channels")]
#[tokio::test(flavor = "multi_thread")]
async fn a_channel_on_another_organizations_tenant_reads_as_an_unknown_tenant() {
    let st = channel_state(true, None).await;
    let owner = member("sk_b");
    let bind = json!({"entity": "urn:ngsi-ld:Bus:1", "attribute": "speed",
                      "broker": "lo", "topic": "sk_b__x/bus/1/speed", "retention": "mirror"});
    let unknown = send(&st, "GET", "/x/channels", Some("sk_b__x"), &owner, None).await;
    assert_eq!(unknown.0, StatusCode::NOT_FOUND);
    make_bus(&st).await;
    let (s, _, body) = send(
        &st,
        "PUT",
        "/x/channels/c1",
        Some("sk_b__x"),
        &owner,
        Some(bind.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    let intruder = member("sk_a");
    for (method, path, body) in [
        ("PUT", "/x/channels/c2", Some(bind)),
        ("GET", "/x/channels", None),
        ("GET", "/x/channels/c1", None),
        ("DELETE", "/x/channels/c1", None),
    ] {
        let refused = send(&st, method, path, Some("sk_b__x"), &intruder, body).await;
        // the unknown answer was taken before `sk_b__x` existed
        assert_eq!(refused, unknown, "{method} {path}");
    }
    let (s, ..) = send(&st, "GET", "/x/channels/c1", Some("sk_b__x"), &owner, None).await;
    assert_eq!(s, StatusCode::OK, "the owner's binding is still there");
}

/// The replay of a stored Channel acts for the stored Tenant, which no
/// caller names: a Tenant renamed away since the bind is not restored, one
/// in good standing is.
#[cfg(feature = "channels")]
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_channel_of_a_renamed_tenant_is_not_replayed() {
    let first = channel_state(true, None).await;
    make_bus(&first).await;
    let bind = json!({"entity": "urn:ngsi-ld:Bus:1", "attribute": "speed",
                      "broker": "lo", "topic": "sk_b__x/bus/1/speed", "retention": "mirror"});
    let (s, ..) = send(
        &first,
        "PUT",
        "/x/channels/c1",
        Some("sk_b__x"),
        &member("sk_b"),
        Some(bind),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let speed = || async {
        let (_, _, body) = send(
            &first,
            "GET",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Bus:1?format=keyValues",
            Some("sk_b__x"),
            &member("sk_b"),
            None,
        )
        .await;
        serde_json::from_slice::<Value>(&body)
            .ok()
            .map(|e| e["speed"].clone())
    };
    for (tombstoned, value) in [(true, "7"), (false, "8")] {
        let (_st, lo) = channel_state_over(&first, tombstoned).await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut landed = false;
        while tokio::time::Instant::now() < deadline {
            lo.publish("sk_b__x/bus/1/speed", value);
            if speed().await.is_some_and(|v| v.to_string().contains(value)) {
                landed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(landed, !tombstoned, "tombstoned {tombstoned}");
    }
}

#[cfg(feature = "channels")]
async fn make_bus(st: &AppState) {
    let (s, ..) = send(
        st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some("sk_b__x"),
        &member("sk_b"),
        Some(json!({"id": "urn:ngsi-ld:Bus:1", "type": "Bus",
                    "speed": {"type": "Property", "value": 0}})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
}

/// [`state`] with the channels surface on a loopback bridge `lo`, over
/// `store` when given.
#[cfg(feature = "channels")]
async fn channel_state(
    enforce: bool,
    store: Option<std::sync::Arc<dyn antares_store::CurrentStateDriver>>,
) -> AppState {
    channel_state_with(enforce, store).await.0
}

#[cfg(feature = "channels")]
async fn channel_state_with(
    enforce: bool,
    store: Option<std::sync::Arc<dyn antares_store::CurrentStateDriver>>,
) -> (AppState, std::sync::Arc<antares_api::channels::Loopback>) {
    use antares_api::channels::{Bridge, Channels, Loopback};
    antares_jsonld::allow_private_egress(true);
    antares_api::policy::default_subject_headers("x-jc-subject,x-jc-member");
    let lo = std::sync::Arc::new(Loopback::new());
    let mut bridges: std::collections::HashMap<String, std::sync::Arc<dyn Bridge>> =
        std::collections::HashMap::new();
    bridges.insert("lo".into(), lo.clone());
    let mut st = AppState::new("tenant-organization".into())
        .with_admin_auth(AdminAuth::new(SECRET, false).expect("secret"))
        .with_surface(Box::new(Channels::new(bridges)))
        .expect("mount");
    if let Some(store) = store {
        st.store = store;
    }
    antares_api::wire(&mut st).await;
    st.subject_required = true;
    st.tenant_organization = enforce;
    // the first router built restores the stored Channels
    let _ = antares_api::router(st.clone());
    (st, lo)
}

/// A second broker over `first`'s store, as after a restart, whose store
/// says `sk_b__x` was renamed away when `tombstoned`.
#[cfg(feature = "channels")]
async fn channel_state_over(
    first: &AppState,
    tombstoned: bool,
) -> (AppState, std::sync::Arc<antares_api::channels::Loopback>) {
    let double = std::sync::Arc::new(common::Double::passthrough(first.store.clone()));
    double.set_standing(
        "sk_b__x",
        antares_store::TenantStanding {
            tombstoned,
            organization: None,
        },
    );
    channel_state_with(true, Some(double)).await
}
