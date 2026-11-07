// SPDX-License-Identifier: EUPL-1.2
//! T-4872: a socket opened on a view receives only what that view shows.
//! Two views of one Tenant over the same Entity type, a Tenant beside them
//! holding the same ids, and a canary Attribute and Entity type no view
//! shows: no canary, no other view's Attribute and no other Tenant's write
//! may reach a socket, and a change to what a view hides is no
//! notification at all (its timing would leak the change).
//!
//! The `*` view with a view `q` (ADR-0023 rule 12) carries no rules: its
//! Entities joined into a notification are read under its `q` as a GET
//! through it reads them, so a Linked Entity outside the `q` is not
//! delivered beside the Linking one.
#![cfg(all(feature = "full", feature = "jc-policy"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::Duration;
use tower::Service;

#[path = "common/ws_client.rs"]
mod ws_client;
use ws_client::Socket;

/// The service account the tests write as, outside every view.
const SEEDER: &str = "iso-seed";
/// The Attribute and the Entity type no view shows.
const CANARY: &str = "canary";
const CANARY_TYPE: &str = "Canary";
/// The read and subscription clauses the public role holds whole, so the
/// view alone decides.
const READS: [&str; 15] = [
    "5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.7.5", "5.7.6", "5.7.7", "5.7.8", "5.7.9", "5.7.10",
    "5.8.1", "5.8.2", "5.8.3", "5.8.4", "5.8.5",
];

async fn broker() -> (Router, antares_ops::views::Reload, u16) {
    std::env::set_var(
        "ANTARES_POLICY_SUBJECT_HEADERS",
        joinedcontext_policy::subject::HEADERS.join(","),
    );
    let mut state = AppState::new("ws-isolation".into())
        .with_sink(Box::new(antares_ws::Sink::new(antares_ws::Hub::process())))
        .with_policy(std::sync::Arc::new(
            joinedcontext_policy::JoinedcontextPolicy::new(),
        ));
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_ws::ViewWs::process()),
    ];
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await });
    (app, reload, port)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("a request"))
        .await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

/// `views` (id and document) in `tenant`, the public role reading through
/// them and the seeder writing every type on the Tenant root.
async fn install(app: &Router, tenant: &str, views: &[(&str, Value)]) {
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let policy = json!({"policies": [
        {"id": "open", "assignee": {"kind": "role", "id": "public"}, "operations": READS},
        {"id": "seed", "assignee": {"kind": "serviceAccount", "id": SEEDER},
         "operations": ["5.6.1", "5.6.2", "5.6.3", "5.6.8", "5.6.10"]}
    ]});
    let mut docs: Vec<(String, Value)> = views
        .iter()
        .map(|(id, view)| {
            let mut view = view.clone();
            view["id"] = json!(id);
            view["tenant"] = json!(tenant);
            view["operations"] = json!(ops);
            (format!("/q/views/{id}"), view)
        })
        .collect();
    docs.push((format!("/q/policy/{tenant}"), policy));
    for (uri, doc) in docs {
        let (status, body) = send(app, "PUT", &uri, &[], Some(&doc)).await;
        assert!(status.is_success(), "{uri}: {status} {body}");
    }
}

/// Upsert `entities` in `tenant` on its root, as the seeder.
async fn write(app: &Router, tenant: &str, entities: &Value) {
    let headers = [("x-jc-client", SEEDER), ("NGSILD-Tenant", tenant)];
    let uri = "/ngsi-ld/v1/entityOperations/upsert?options=update";
    let (status, body) = send(app, "POST", uri, &headers, Some(entities)).await;
    assert!(status.is_success(), "write in {tenant}: {status} {body}");
}

/// A socket on `view` as the public role with `extra` headers, subscribed
/// with `body`: the x-subscribe answer and the socket.
async fn subscribe(port: u16, view: &str, extra: &[(&str, &str)], body: Value) -> (Value, Socket) {
    let path = format!("/x/views/{view}/ws");
    let mut who = vec![("x-jc-roles".to_owned(), "public".to_owned())];
    who.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    tokio::task::spawn_blocking(move || {
        let mut socket =
            Socket::open(port, &path, &who, Duration::from_millis(800)).expect("the socket");
        let welcome = socket.next().unwrap_or_default();
        assert_eq!(welcome["metadata"]["x-op"], "x-welcome", "{welcome}");
        socket
            .send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": "s"}, "body": body}))
            .expect("sent");
        (socket.next().unwrap_or_default(), socket)
    })
    .await
    .expect("subscribed")
}

/// The messages the socket gets up to its next notification, the socket
/// handed back: a write waits for its notification before the next one,
/// since a notification reads the stored state when it leaves.
async fn until_notified(socket: Socket) -> (Socket, Vec<Value>) {
    tokio::task::spawn_blocking(move || {
        let mut socket = socket;
        let mut seen = Vec::new();
        while let Some(m) = socket.next() {
            let done = m["metadata"]["x-op"] == "x-notification";
            seen.push(m);
            if done {
                break;
            }
        }
        (socket, seen)
    })
    .await
    .expect("read")
}

/// Every message the socket gets until it falls silent.
async fn drain(socket: Socket) -> Vec<Value> {
    tokio::task::spawn_blocking(move || {
        let mut socket = socket;
        std::iter::from_fn(|| socket.next()).collect()
    })
    .await
    .expect("read")
}

/// The notified Entities of `messages`, in order.
fn notified(messages: &[Value]) -> Vec<&Value> {
    messages
        .iter()
        .filter(|m| m["metadata"]["x-op"] == "x-notification")
        .flat_map(|m| m["body"]["data"].as_array().into_iter().flatten())
        .collect()
}

/// The member names of an Entity, framing members left out.
fn names(e: &Value) -> BTreeSet<&str> {
    e.as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.as_str())
        .filter(|k| !matches!(*k, "id" | "type" | "@context"))
        .collect()
}

fn vehicle(id: &str, speed: i64, operator: &str, canary: i64) -> Value {
    json!({"id": id, "type": "Vehicle",
           "speed": {"type": "Property", "value": speed},
           "operator": {"type": "Property", "value": operator},
           CANARY: {"type": "Property", "value": canary}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_two_views_of_one_tenant_each_see_only_their_own_projection() {
    let (app, _reload, port) = broker().await;
    let (tenant, other) = ("iso-a", "iso-b");
    let rules = |attributes: &[&str]| {
        json!({"selector": ["Vehicle"],
               "rules": [{"types": ["Vehicle"], "attributes": attributes}]})
    };
    install(
        &app,
        tenant,
        &[
            ("iso-public", rules(&["speed"])),
            ("iso-full", rules(&["speed", "operator"])),
        ],
    )
    .await;
    install(&app, other, &[("iso-other", rules(&["speed", "operator"]))]).await;
    let all = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}]});
    let (made, public) = subscribe(port, "iso-public", &[], all.clone()).await;
    assert_eq!(made["metadata"]["x-status"], 201, "{made}");
    // a Tenant header naming another Tenant than the view's is refused at
    // the upgrade, before any Subscription
    let who = [
        ("x-jc-roles".to_owned(), "public".to_owned()),
        ("NGSILD-Tenant".to_owned(), other.to_owned()),
    ];
    let forged = tokio::task::spawn_blocking(move || {
        Socket::open(
            port,
            "/x/views/iso-public/ws",
            &who,
            Duration::from_millis(800),
        )
        .err()
    })
    .await
    .expect("opened");
    assert!(
        forged
            .as_deref()
            .is_some_and(|e| e.contains(" 400 ") && !e.contains(other)),
        "{forged:?}"
    );
    let (made, full) = subscribe(port, "iso-full", &[], all).await;
    assert_eq!(made["metadata"]["x-status"], 201, "{made}");

    let v = "urn:ngsi-ld:Vehicle:iso:1";
    write(&app, tenant, &json!([vehicle(v, 1, "city", 1)])).await;
    let (public, mut public_seen) = until_notified(public).await;
    let (full, mut full_seen) = until_notified(full).await;
    // the hidden canary alone: no notification on any socket
    write(&app, tenant, &json!([vehicle(v, 1, "city", 2)])).await;
    // operator alone: the full view only
    write(&app, tenant, &json!([vehicle(v, 1, "private", 2)])).await;
    let (full, seen) = until_notified(full).await;
    full_seen.extend(seen);
    // a type no view selects, carrying the canary
    write(
        &app,
        tenant,
        &json!([{"id": "urn:ngsi-ld:Canary:iso:1", "type": CANARY_TYPE,
                 "speed": {"type": "Property", "value": 7}}]),
    )
    .await;
    // the same id in the Tenant beside, with a speed no write here makes
    write(&app, other, &json!([vehicle(v, 999, "other", 3)])).await;

    public_seen.extend(drain(public).await);
    let messages = public_seen;
    let got = notified(&messages);
    assert_eq!(
        got.len(),
        1,
        "public: one notification, the create: {messages:?}"
    );
    for e in &got {
        assert_eq!(e["id"], v, "public: {messages:?}");
        assert_eq!(names(e), BTreeSet::from(["speed"]), "public: {messages:?}");
        assert_eq!(e["speed"]["value"], 1, "public: {messages:?}");
    }
    let wire = Value::Array(messages).to_string();
    for leak in [CANARY, CANARY_TYPE, "operator", other] {
        assert!(!wire.contains(leak), "public: {leak} on the wire: {wire}");
    }
    full_seen.extend(drain(full).await);
    let messages = full_seen;
    let got = notified(&messages);
    assert_eq!(
        got.len(),
        2,
        "full: the create and the operator: {messages:?}"
    );
    for e in &got {
        assert_eq!(
            names(e),
            BTreeSet::from(["operator", "speed"]),
            "{messages:?}"
        );
    }
    assert_eq!(got[1]["operator"]["value"], "private", "{messages:?}");
    let wire = Value::Array(messages).to_string();
    for leak in [CANARY, CANARY_TYPE, other] {
        assert!(!wire.contains(leak), "full: {leak} on the wire: {wire}");
    }
}

/// A Subscription naming what the view hides is made (5.8.1 admits it) and
/// notifies nothing: a type outside the selector, a watched hidden
/// Attribute, a `q` over one, an id of another Tenant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_a_subscription_on_hidden_names_receives_nothing() {
    let (app, _reload, port) = broker().await;
    let (tenant, other) = ("iso-c", "iso-d");
    let view = json!({"selector": ["Vehicle"],
                      "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}]});
    install(&app, tenant, &[("iso-hidden", view.clone())]).await;
    install(&app, other, &[("iso-hidden-other", view)]).await;
    let other_id = "urn:ngsi-ld:Vehicle:iso:other";
    let mut sockets = Vec::new();
    for body in [
        json!({"type": "Subscription", "entities": [{"type": CANARY_TYPE}]}),
        json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
               "watchedAttributes": [CANARY]}),
        json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
               "q": format!("{CANARY}>0")}),
        json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
               "q": "operator==\"city\""}),
        json!({"type": "Subscription", "entities": [{"id": other_id, "type": "Vehicle"}]}),
    ] {
        let (made, socket) = subscribe(port, "iso-hidden", &[], body.clone()).await;
        let status = made["metadata"]["x-status"].as_u64().unwrap_or_default();
        assert!(
            status == 201 || (400..500).contains(&status),
            "{body}: {made}"
        );
        let wire = made.to_string();
        assert!(!wire.contains(other), "{wire}");
        sockets.push((body, socket));
    }
    write(
        &app,
        tenant,
        &json!([vehicle("urn:ngsi-ld:Vehicle:iso:2", 1, "city", 1)]),
    )
    .await;
    write(
        &app,
        tenant,
        &json!([vehicle("urn:ngsi-ld:Vehicle:iso:2", 1, "city", 5)]),
    )
    .await;
    write(
        &app,
        tenant,
        &json!([{"id": "urn:ngsi-ld:Canary:iso:2", "type": CANARY_TYPE,
                 CANARY: {"type": "Property", "value": 1}}]),
    )
    .await;
    write(&app, other, &json!([vehicle(other_id, 999, "other", 1)])).await;
    for (body, socket) in sockets {
        let messages = drain(socket).await;
        assert!(notified(&messages).is_empty(), "{body}: {messages:?}");
    }
}

/// The `*` view with a view `q` (rule 12): a Linked Entity outside the `q`
/// is no more delivered by a joined notification than a GET through the
/// view embeds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_the_every_view_with_a_q_joins_no_entity_outside_it() {
    let (app, _reload, port) = broker().await;
    let tenant = "iso-e";
    let view = json!({"selector": ["*"],
                      "rules": [{"types": ["*"], "attributes": ["*"]}], "q": "speed<25"});
    install(&app, tenant, &[("iso-every", view)]).await;
    let hidden = "urn:ngsi-ld:Canary:iso:3";
    write(
        &app,
        tenant,
        &json!([{"id": hidden, "type": CANARY_TYPE,
                 CANARY: {"type": "Property", "value": 1}}]),
    )
    .await;
    let mut sockets = Vec::new();
    for join in ["flat", "inline"] {
        let body = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
                          "notification": {"join": join, "joinLevel": 1}});
        let (made, socket) = subscribe(port, "iso-every", &[], body).await;
        assert_eq!(made["metadata"]["x-status"], 201, "{join}: {made}");
        sockets.push((join, socket));
    }
    let v = "urn:ngsi-ld:Vehicle:iso:3";
    write(
        &app,
        tenant,
        &json!([{"id": v, "type": "Vehicle", "speed": {"type": "Property", "value": 10},
                 "linked": {"type": "Relationship", "object": hidden}}]),
    )
    .await;
    let uri = format!("/x/views/iso-every/ngsi-ld/v1/entities/{v}?join=inline&joinLevel=1");
    let (status, read) = send(&app, "GET", &uri, &[("x-jc-roles", "public")], None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert!(!read.to_string().contains(CANARY), "GET embeds it: {read}");
    for (join, socket) in sockets {
        let messages = drain(socket).await;
        let got = notified(&messages);
        assert!(got.iter().any(|e| e["id"] == v), "{join}: {messages:?}");
        let wire = Value::Array(messages.clone()).to_string();
        assert!(
            !wire.contains(&format!("\"{CANARY}\"")),
            "{join}: joined: {wire}"
        );
        assert!(
            got.iter().all(|e| e["id"] != hidden),
            "{join}: the Linked Entity outside the q: {wire}"
        );
    }
}

/// Replaces `view` of `tenant` while its sockets stay open: every `/q/views`
/// write reloads the table, and each notification is projected under the
/// view in force when it leaves (T-4874).
async fn replace_view(app: &Router, tenant: &str, id: &str, view: Value) {
    let mut view = view;
    view["id"] = json!(id);
    view["tenant"] = json!(tenant);
    view["operations"] = json!(antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect::<Vec<_>>());
    let uri = format!("/q/views/{id}");
    let (status, body) = send(app, "PUT", &uri, &[], Some(&view)).await;
    assert!(status.is_success(), "{uri}: {status} {body}");
}

/// T-4874 race: a view narrowed while about 1 000 changes a second flow to
/// an open socket. Notifications leave in order, so everything from the
/// first one carrying a marker written after the narrowing on left after
/// it: none of them carries the removed Attribute, and the socket stays up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_a_view_narrowed_mid_stream_stops_the_removed_attribute_at_once() {
    let (app, _reload, port) = broker().await;
    let tenant = "rev-a";
    let rules = |attributes: &[&str]| {
        json!({"selector": ["Vehicle"],
               "rules": [{"types": ["Vehicle"], "attributes": attributes}]})
    };
    install(&app, tenant, &[("rev-race", rules(&["speed", "operator"]))]).await;
    let all = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}]});
    let (made, socket) = subscribe(port, "rev-race", &[], all).await;
    assert_eq!(made["metadata"]["x-status"], 201, "{made}");
    let reader = tokio::spawn(drain(socket));
    let marker = "urn:ngsi-ld:Vehicle:rev:marker";
    let batch = |round: i64| {
        Value::Array(
            (0..20)
                .map(|n| vehicle(&format!("urn:ngsi-ld:Vehicle:rev:{n}"), round, "city", 0))
                .collect(),
        )
    };
    // 20 Entities every 20 ms: 1 000 changes a second.
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut sent = 0;
    for round in 0..100 {
        tick.tick().await;
        if round == 50 {
            replace_view(&app, tenant, "rev-race", rules(&["speed"])).await;
            write(&app, tenant, &json!([vehicle(marker, -1, "city", 0)])).await;
        }
        write(&app, tenant, &batch(round)).await;
        sent += 20;
    }
    let messages = reader.await.expect("read");
    let ops: Vec<&Value> = messages.iter().map(|m| &m["metadata"]["x-op"]).collect();
    assert!(
        !ops.iter().any(|op| *op == "x-closing" || *op == "x-gap"),
        "the socket kept up with {sent} changes: {ops:?}"
    );
    let got = notified(&messages);
    let at = got
        .iter()
        .position(|e| e["id"] == marker)
        .expect("the marker reached the socket");
    assert!(
        got[..at].iter().any(|e| names(e).contains("operator")),
        "the wide view delivered operator before the change"
    );
    let leaked: Vec<&&Value> = got[at..]
        .iter()
        .filter(|e| names(e).contains("operator"))
        .collect();
    assert!(
        leaked.is_empty(),
        "removed data after the change: {leaked:?}"
    );
}

/// T-4874: a Policy grant withdrawn while a socket is open stops its
/// deliveries with the next change; a view widened while it is open never
/// reaches past what the Subscription asked for (its type and
/// `notification.attributes`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_a_revoked_grant_stops_and_a_widened_view_never_widens_a_subscription() {
    let (app, _reload, port) = broker().await;
    let tenant = "rev-b";
    let narrow = json!({"selector": ["Vehicle"],
                        "rules": [{"types": ["Vehicle"], "attributes": ["speed"]}]});
    install(&app, tenant, &[("rev-grant", narrow)]).await;
    let all = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}]});
    let (made, revoked) = subscribe(port, "rev-grant", &[], all).await;
    assert_eq!(made["metadata"]["x-status"], 201, "{made}");
    let speed_only = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
                            "notification": {"attributes": ["speed"]}});
    let (made, widened) = subscribe(port, "rev-grant", &[], speed_only).await;
    assert_eq!(made["metadata"]["x-status"], 201, "{made}");
    write(
        &app,
        tenant,
        &json!([vehicle("urn:ngsi-ld:Vehicle:rev:1", 1, "city", 1)]),
    )
    .await;
    let (revoked, before) = until_notified(revoked).await;
    assert_eq!(notified(&before).len(), 1, "{before:?}");
    let (widened, _) = until_notified(widened).await;

    replace_view(
        &app,
        tenant,
        "rev-grant",
        json!({"selector": ["Vehicle", CANARY_TYPE],
               "rules": [{"types": ["Vehicle"], "attributes": ["speed", "operator", CANARY]},
                         {"types": [CANARY_TYPE], "attributes": [CANARY]}]}),
    )
    .await;
    write(
        &app,
        tenant,
        &json!([vehicle("urn:ngsi-ld:Vehicle:rev:1", 2, "bus", 2),
        {"id": "urn:ngsi-ld:Canary:rev:1", "type": CANARY_TYPE,
         CANARY: {"type": "Property", "value": 1}}]),
    )
    .await;
    let (widened, after) = until_notified(widened).await;
    let got = notified(&after);
    assert!(
        got.iter()
            .all(|e| e["type"] == "Vehicle" && names(e) == BTreeSet::from(["speed"])),
        "past the Subscription: {after:?}"
    );

    let only_seed = json!({"policies": [
        {"id": "seed", "assignee": {"kind": "serviceAccount", "id": SEEDER},
         "operations": ["5.6.1", "5.6.2", "5.6.3", "5.6.8", "5.6.10"]}
    ]});
    let uri = format!("/q/policy/{tenant}");
    let (status, body) = send(&app, "PUT", &uri, &[], Some(&only_seed)).await;
    assert!(status.is_success(), "{uri}: {status} {body}");
    write(
        &app,
        tenant,
        &json!([vehicle("urn:ngsi-ld:Vehicle:rev:1", 3, "city", 3)]),
    )
    .await;
    for (name, socket) in [("revoked", revoked), ("widened", widened)] {
        let messages = drain(socket).await;
        let got = notified(&messages);
        assert!(
            got.iter().all(|e| e["speed"]["value"] != 3),
            "{name}: delivered after the grant was withdrawn: {messages:?}"
        );
    }
}
