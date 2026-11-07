// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 equivalence layer 2, "same answer": every federated ETSI test
//! purpose, rebuilt tenant-to-tenant, runs on two brokers set up alike. On
//! one the source Tenants are reached over HTTP, through the broker's own
//! loopback listener; on the other the listener's URL is a self URL, so the
//! forwards are served in process. Every answer (status, `NGSILD-Warning`
//! values, body after `@context` normalisation) and the state left in every
//! Tenant must be equal.
//!
//! The suite's own Context Source is a mock HTTP server, which an in-process
//! transport cannot stand in for, hence the rebuild: each scenario keeps its
//! TP's registration mode, operations and the request it sends.
//! [`icsr_every_federated_tp_has_a_case`] diffs the TP files against the
//! table, so a TP added upstream fails until it has a case.

use antares_icsr::SelfUrls;
use antares_ops::test_kit::{allow_private_egress, TestBroker};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tower::ServiceExt;

/// The Tenants a scenario reads back after its steps.
const TENANTS: [&str; 4] = ["a", "b", "c", "zz"];

/// The ETSI `DistributedOperations` directory, from this crate.
const TP_DIR: &str = "../../ngsi-ld-test-suite/TP/NGSI-LD/DistributedOperations";

/// One broker with the internal transport, served on its own loopback
/// listener.
struct Side {
    app: axum::Router,
    url: String,
    _broker: TestBroker,
}

async fn side(in_process: bool) -> Side {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-Subject");
    allow_private_egress();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let addr = socket.local_addr().unwrap_or_else(|e| unreachable!("{e}"));
    let url = format!("http://{addr}");
    let selfs = SelfUrls::parse(if in_process {
        &url
    } else {
        "http://self.invalid"
    })
    .unwrap_or_else(|e| unreachable!("{e}"));
    let broker = TestBroker::new()
        .await
        .and_then(|b| b.with_forward_transport(antares_icsr::transport(selfs)))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let app = broker
        .serve_mounted(Vec::new())
        .unwrap_or_else(|e| unreachable!("{e}"));
    let served = app.clone();
    tokio::spawn(async move { axum::serve(socket, served).await });
    Side {
        app,
        url,
        _broker: broker,
    }
}

/// An answer, normalised: the side's URL replaced, `@context` dropped,
/// Entity lists sorted by id.
#[derive(Debug, PartialEq)]
struct Answer {
    status: u16,
    warnings: Vec<String>,
    body: Value,
}

impl Side {
    async fn call(
        &self,
        tenant: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Answer {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("NGSILD-Tenant", tenant);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = match body {
            Some(b) => {
                let text = b.to_string();
                req.header("Content-Type", "application/json")
                    .header("Content-Length", text.len())
                    .body(Body::from(text))
            }
            None => req.body(Body::empty()),
        }
        .unwrap_or_else(|e| unreachable!("{e}"));
        let answer = self
            .app
            .clone()
            .oneshot(req)
            .await
            .unwrap_or_else(|e| unreachable!("{e}"));
        let status = answer.status().as_u16();
        let warnings = answer
            .headers()
            .get_all("ngsild-warning")
            .iter()
            .map(|v| self.plain(&String::from_utf8_lossy(v.as_bytes())))
            .collect();
        let bytes = answer
            .into_body()
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
            .unwrap_or_default();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Answer {
            status,
            warnings,
            body: self.normal(body),
        }
    }

    fn plain(&self, text: &str) -> String {
        text.replace(&self.url, "<source>")
    }

    fn normal(&self, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.plain(&s)),
            Value::Array(items) => {
                let mut items: Vec<Value> = items.into_iter().map(|i| self.normal(i)).collect();
                items.sort_by_key(|i| {
                    i.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                });
                Value::Array(items)
            }
            Value::Object(o) => Value::Object(
                o.into_iter()
                    .filter(|(k, _)| k != "@context")
                    .map(|(k, v)| (k, self.normal(v)))
                    .collect(),
            ),
            other => other,
        }
    }

    /// [`Self::state`] once it has held for 300 ms (at most 3 s): what a
    /// spawned forward (a distributed subscription's copy, 5.8.1.4) leaves.
    async fn settled(&self) -> Vec<(String, Answer, Value)> {
        let mut last = self.state().await;
        let mut held = 0_u8;
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let now = self.state().await;
            held = if now == last {
                held.saturating_add(1)
            } else {
                0
            };
            last = now;
            if held >= 6 {
                break;
            }
        }
        last
    }

    /// What every Tenant holds: its Vehicles, and the subscriptions by
    /// what they ask and where they notify (their ids are minted).
    async fn state(&self) -> Vec<(String, Answer, Value)> {
        let mut out = Vec::new();
        for tenant in TENANTS {
            let vehicles = self
                .call(
                    tenant,
                    "GET",
                    "/ngsi-ld/v1/entities?type=Vehicle&local=true",
                    &[],
                    None,
                )
                .await;
            let subs = self
                .call(tenant, "GET", "/ngsi-ld/v1/subscriptions", &[], None)
                .await;
            let mut asked: Vec<Value> = subs
                .body
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    let at = |p: &str| s.pointer(p).cloned().unwrap_or(Value::Null);
                    json!([at("/entities"), at("/q"), at("/notification/endpoint/uri")])
                })
                .collect();
            asked.sort_by_key(ToString::to_string);
            out.push((tenant.to_owned(), vehicles, Value::Array(asked)));
        }
        out
    }
}

/// A registration in Tenant `from` naming the listener as the source of
/// Tenant `target`.
struct Reg {
    from: &'static str,
    target: &'static str,
    mode: &'static str,
    ops: &'static [&'static str],
    information: Value,
    extra: Value,
}

/// One request in Tenant `a`; `look`: the Tenants' state is compared after
/// it too, not only after the last step.
struct Step {
    method: &'static str,
    path: String,
    headers: Vec<(&'static str, &'static str)>,
    body: Option<Value>,
    look: bool,
}

/// The rebuild of one or more TPs.
struct Scenario {
    tps: &'static [&'static str],
    seeds: Vec<(&'static str, Value)>,
    regs: Vec<Reg>,
    steps: Vec<Step>,
}

const V1: &str = "urn:ngsi-ld:Vehicle:V1";
const V2: &str = "urn:ngsi-ld:Vehicle:V2";

fn prop(v: Value) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("type".into(), json!("Property"));
    p.insert("value".into(), v);
    Value::Object(p)
}

/// Vehicle `id` with the attributes `attrs` names (`brandName`, `speed`).
fn vehicle(id: &str, attrs: &[&str]) -> Value {
    let mut v = serde_json::Map::new();
    v.insert("id".into(), json!(id));
    v.insert("type".into(), json!("Vehicle"));
    for a in attrs {
        let value = match *a {
            "brandName" => prop(json!("Mercedes")),
            _ => prop(json!(60)),
        };
        v.insert((*a).to_owned(), value);
    }
    Value::Object(v)
}

const BOTH: &[&str] = &["brandName", "speed"];
const REDIRECTION: &[&str] = &["redirectionOps"];

fn types() -> Value {
    json!([{"entities": [{"type": "Vehicle"}]}])
}

fn of(id: &str, props: &[&str]) -> Value {
    if props.is_empty() {
        json!([{"entities": [{"id": id, "type": "Vehicle"}]}])
    } else {
        json!([{"entities": [{"id": id, "type": "Vehicle"}], "propertyNames": props}])
    }
}

fn reg(
    mode: &'static str,
    target: &'static str,
    ops: &'static [&'static str],
    information: Value,
) -> Reg {
    Reg {
        from: "a",
        target,
        mode,
        ops,
        information,
        extra: json!({}),
    }
}

fn inc(target: &'static str, ops: &'static [&'static str]) -> Reg {
    reg("inclusive", target, ops, types())
}

fn exc(target: &'static str, ops: &'static [&'static str]) -> Reg {
    reg("exclusive", target, ops, of(V1, &["speed"]))
}

fn red(target: &'static str, ops: &'static [&'static str]) -> Reg {
    reg("redirect", target, ops, of(V1, &[]))
}

fn aux(target: &'static str) -> Reg {
    reg("auxiliary", target, &["retrieveOps"], types())
}

fn with(mut r: Reg, extra: Value) -> Reg {
    r.extra = extra;
    r
}

fn step(method: &'static str, path: &str, body: Option<Value>) -> Step {
    Step {
        method,
        path: path.to_owned(),
        headers: Vec::new(),
        body,
        look: false,
    }
}

fn entities(query: &str) -> String {
    format!("/ngsi-ld/v1/entities{query}")
}

fn entity(id: &str, rest: &str) -> String {
    format!("/ngsi-ld/v1/entities/{id}{rest}")
}

fn batch(op: &str, query: &str) -> String {
    format!("/ngsi-ld/v1/entityOperations/{op}{query}")
}

fn s(
    tps: &'static [&'static str],
    seeds: Vec<(&'static str, Value)>,
    regs: Vec<Reg>,
    steps: Vec<Step>,
) -> Scenario {
    Scenario {
        tps,
        seeds,
        regs,
        steps,
    }
}

/// The source side of a mode: what the remote Tenant holds before the
/// step, and the local remainder.
fn split(mode: &str) -> Vec<(&'static str, Value)> {
    match mode {
        "inclusive" => vec![("a", vehicle(V1, BOTH)), ("b", vehicle(V1, BOTH))],
        "exclusive" => vec![
            ("a", vehicle(V1, &["brandName"])),
            ("b", vehicle(V1, &["speed"])),
        ],
        _ => vec![("b", vehicle(V1, BOTH)), ("c", vehicle(V1, BOTH))],
    }
}

/// The registrations of a mode, as the D0xx TPs make them.
fn regs_of(mode: &str) -> Vec<Reg> {
    match mode {
        "inclusive" => vec![inc("b", REDIRECTION)],
        "exclusive" => vec![exc("b", REDIRECTION)],
        _ => vec![red("b", REDIRECTION), red("c", REDIRECTION)],
    }
}

/// A write TP family: one scenario per mode, the same step for each.
fn per_mode(tps: [&'static [&'static str]; 3], make: impl Fn() -> Vec<Step>) -> Vec<Scenario> {
    ["exclusive", "inclusive", "redirect"]
        .into_iter()
        .zip(tps)
        .map(|(mode, tps)| s(tps, split(mode), regs_of(mode), make()))
        .collect()
}

fn speed(v: i64) -> Value {
    json!({"speed": prop(json!(v))})
}

/// One step, a read of `path`.
fn get(path: &str) -> Vec<Step> {
    vec![step("GET", path, None)]
}

/// Create Vehicle V1, then read it back.
fn create_v1() -> Vec<Step> {
    vec![
        step("POST", &entities(""), Some(vehicle(V1, BOTH))),
        step("GET", &entity(V1, ""), None),
    ]
}

/// Create Entity (D001) and its 4.3.6 TPs.
fn creates() -> Vec<Scenario> {
    vec![
        // Create Entity (5.6.1, D001)
        s(
            &["D001_01_exc"],
            vec![],
            vec![exc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_01_inc", "D018_02"],
            vec![],
            vec![inc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_01_red"],
            vec![],
            vec![red("b", REDIRECTION), red("c", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_02_exc"],
            vec![("b", vehicle(V1, &["speed"]))],
            vec![exc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_02_inc"],
            vec![],
            vec![inc("b", REDIRECTION)],
            vec![step(
                "POST",
                &entities(""),
                Some(vehicle("not a uri", BOTH)),
            )],
        ),
        s(
            &["D001_03_01_inc"],
            vec![("a", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_03_02_inc"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_03_03_inc"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            create_v1(),
        ),
        s(
            &["D001_04_inc"],
            vec![],
            vec![inc("b", REDIRECTION)],
            vec![step(
                "POST",
                &entities("?local=true"),
                Some(vehicle(V1, BOTH)),
            )],
        ),
        s(
            &["5614_01"],
            vec![],
            vec![red("b", REDIRECTION)],
            create_v1(),
        ),
        s(&["436_02"], vec![], vec![aux("b")], create_v1()),
        s(
            &["436_06"],
            vec![],
            vec![with(
                inc("b", REDIRECTION),
                json!({"contextSourceInfo": [{"key": "contentType", "value": "application/ld+json"}]}),
            )],
            create_v1(),
        ),
    ]
}

/// Delete Entity (D002).
fn deletes() -> Vec<Scenario> {
    vec![
        // Delete Entity (5.6.6, D002)
        s(
            &["D002_01_exc"],
            split("exclusive"),
            vec![exc("b", REDIRECTION)],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_01_inc"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_01_red"],
            split("redirect"),
            regs_of("redirect"),
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_02_01_inc"],
            vec![("a", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_02_02_inc"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_02_exc"],
            split("exclusive"),
            vec![exc("b", &["retrieveOps"])],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D002_03_inc"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            vec![step("DELETE", &entity(V1, "?local=true"), None)],
        ),
    ]
}

/// Retrieve Entity (D010), the 4.3.6 TPs that read, a failing source.
fn retrieves() -> Vec<Scenario> {
    vec![
        // Retrieve Entity (5.7.1, D010)
        s(
            &["D010_01_aux"],
            split("exclusive"),
            vec![aux("b")],
            get(&entity(V1, "")),
        ),
        s(
            &["D010_01_exc"],
            split("exclusive"),
            vec![exc("b", &["retrieveEntity"])],
            get(&entity(V1, "")),
        ),
        s(
            &["D010_01_inc", "6317_02"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entity(V1, "")),
        ),
        s(
            &["D010_01_red"],
            vec![("b", vehicle(V1, BOTH))],
            vec![red("b", REDIRECTION)],
            get(&entity(V1, "")),
        ),
        s(
            &["D010_02_exc"],
            split("exclusive"),
            vec![exc("b", REDIRECTION)],
            get(&entity(V1, "?local=true")),
        ),
        s(
            &["D010_02_inc"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entity(V1, "?local=true")),
        ),
        s(
            &["D010_02_red"],
            vec![("b", vehicle(V1, BOTH))],
            vec![red("b", REDIRECTION)],
            get(&entity(V1, "?local=true")),
        ),
        s(
            &["D010_03_inc"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", &["retrieveEntity"])],
            get(&entity(V1, "")),
        ),
        s(
            &["436_01"],
            vec![("b", vehicle(V1, BOTH))],
            vec![inc("b", &["retrieveEntity"])],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["436_04"],
            vec![("c", vehicle(V1, BOTH))],
            vec![
                with(
                    inc("b", REDIRECTION),
                    json!({"management": {"localOnly": true}}),
                ),
                Reg {
                    from: "b",
                    ..inc("c", REDIRECTION)
                },
            ],
            get(&entity(V1, "")),
        ),
        s(
            &["436_05"],
            vec![("b", vehicle(V1, BOTH))],
            vec![with(
                inc("b", REDIRECTION),
                json!({"contextSourceInfo": [{"key": "X-Auth-Token", "value": "t-1"}]}),
            )],
            get(&entity(V1, "")),
        ),
        // A source that answers with an error (6.3.17): Tenant zz holds nothing.
        s(
            &["6317_01"],
            vec![],
            vec![inc("zz", REDIRECTION)],
            get(&entity(V1, "")),
        ),
    ]
}

/// Query Entities (D011, 5724).
fn queries() -> Vec<Scenario> {
    vec![
        // Query Entities (5.7.2, D011)
        s(
            &["D011_01_01_inc"],
            vec![("a", vehicle(V1, BOTH)), ("b", vehicle(V2, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_01_02_inc"],
            vec![("a", vehicle(V1, BOTH)), ("b", vehicle(V2, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&attrs=isParked")),
        ),
        s(
            &["D011_01_03_inc"],
            vec![("a", vehicle(V1, &["brandName"])), ("b", vehicle(V2, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&attrs=speed")),
        ),
        s(
            &["D011_01_04_inc"],
            vec![
                ("a", vehicle(V1, &["brandName"])),
                ("b", vehicle(V2, &["brandName"])),
            ],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&attrs=speed")),
        ),
        s(
            &["D011_01_05_inc"],
            vec![
                ("a", vehicle(V1, &["brandName"])),
                ("b", vehicle(V1, &["speed"])),
            ],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_01_aux"],
            split("exclusive"),
            vec![aux("b")],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_01_exc"],
            split("exclusive"),
            vec![exc("b", REDIRECTION)],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_01_red"],
            vec![("b", vehicle(V1, BOTH))],
            vec![red("b", REDIRECTION)],
            get(&entities("?type=Vehicle")),
        ),
    ]
}

/// Query Entities that a registration's operations or the local flag keep
/// from the source (`D011_02` to `D011_05`), and 5724.
fn queries_kept() -> Vec<Scenario> {
    vec![
        s(
            &["D011_02_exc"],
            split("exclusive"),
            vec![exc("b", &["retrieveEntity"])],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_02_inc"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&local=true")),
        ),
        s(
            &["D011_02_red"],
            vec![("b", vehicle(V1, BOTH))],
            vec![red("b", &["queryEntity"])],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_03_inc"],
            split("inclusive"),
            vec![inc("b", &["retrieveEntity"])],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_04_inc"],
            split("inclusive"),
            vec![inc("b", &["queryBatch"])],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["D011_05_inc"],
            vec![("a", vehicle(V1, BOTH)), ("b", vehicle(V2, BOTH))],
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&q=speed%3E50")),
        ),
        s(
            &["5724_01"],
            vec![("b", vehicle(V2, BOTH))],
            vec![with(
                inc("b", REDIRECTION),
                json!({"contextSourceInfo": [{"key": "X-Source", "value": "s-1"}]}),
            )],
            get(&entities("?type=Vehicle")),
        ),
        s(
            &["5724_02"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            get(&entities("?type=Vehicle&q=speed%3E50&attrs=brandName")),
        ),
    ]
}

/// Purge (D017), loops (D018), distributed subscriptions (5814).
fn others() -> Vec<Scenario> {
    vec![
        // Purge Entities (5.6.21, D017)
        s(
            &["D017_01_exc"],
            split("exclusive"),
            vec![exc("b", REDIRECTION)],
            vec![step("DELETE", &entities("?type=Vehicle"), None)],
        ),
        s(
            &["D017_01_inc"],
            split("inclusive"),
            vec![inc("b", REDIRECTION)],
            vec![step("DELETE", &entities("?type=Vehicle"), None)],
        ),
        s(
            &["D017_01_red"],
            split("redirect"),
            regs_of("redirect"),
            vec![step("DELETE", &entities("?type=Vehicle"), None)],
        ),
    ]
}

/// Loop detection (D018): a to b and b back to a, and an inbound `Via`.
fn loops() -> Vec<Scenario> {
    vec![
        s(
            &["D018_01"],
            vec![],
            vec![
                red("b", REDIRECTION),
                Reg {
                    from: "b",
                    ..red("a", REDIRECTION)
                },
            ],
            vec![step("DELETE", &entity(V1, ""), None)],
        ),
        s(
            &["D018_03"],
            vec![],
            vec![inc("b", REDIRECTION)],
            vec![Step {
                headers: vec![("Via", "1.1 upstream-broker")],
                ..step("POST", &entities(""), Some(vehicle(V1, BOTH)))
            }],
        ),
    ]
}

/// Distributed subscriptions (5.8.1.4, 5.8.5.4, rule 7): the reduced copy
/// reaches Tenant b, which must exist, in a spawned forward; the delete
/// follows it.
fn subscriptions() -> Vec<Scenario> {
    let sub = json!({"id": "urn:ngsi-ld:Subscription:distsub", "type": "Subscription",
                     "entities": [{"type": "Vehicle"}], "q": "speed>50",
                     "notification": {"endpoint": {"uri": "http://original.subscriber.example:9998/notify"}}});
    vec![s(
        &["5814_01", "5814_02"],
        vec![("b", vehicle(V2, BOTH))],
        vec![inc(
            "b",
            &["createSubscription", "deleteSubscription", "retrieveOps"],
        )],
        vec![
            Step {
                look: true,
                ..step("POST", "/ngsi-ld/v1/subscriptions", Some(sub))
            },
            Step {
                look: true,
                ..step(
                    "DELETE",
                    "/ngsi-ld/v1/subscriptions/urn:ngsi-ld:Subscription:distsub",
                    None,
                )
            },
        ],
    )]
}

/// The attribute and entity writes per mode (D003..D009).
fn writes() -> Vec<Scenario> {
    let mut out = Vec::new();
    // Append (D003), with noOverwrite (D003_02)
    out.extend(per_mode(
        [&["D003_01_exc"], &["D003_01_inc"], &["D003_01_red"]],
        || {
            vec![
                step("POST", &entity(V1, "/attrs"), Some(speed(80))),
                step("GET", &entity(V1, ""), None),
            ]
        },
    ));
    out.extend(per_mode(
        [&["D003_02_exc"], &["D003_02_inc"], &["D003_02_red"]],
        || {
            vec![step(
                "POST",
                &entity(V1, "/attrs?options=noOverwrite"),
                Some(speed(81)),
            )]
        },
    ));
    // Update Attributes (D004)
    out.extend(per_mode(
        [&["D004_01_exc"], &["D004_01_inc"], &["D004_01_red"]],
        || vec![step("PATCH", &entity(V1, "/attrs"), Some(speed(90)))],
    ));
    // Partial Attribute Update (D005)
    out.extend(per_mode(
        [&["D005_01_exc"], &["D005_01_inc"], &["D005_01_red"]],
        || {
            vec![step(
                "PATCH",
                &entity(V1, "/attrs/speed"),
                Some(json!({"value": 70})),
            )]
        },
    ));
    // Delete Attribute (D006), every instance (D006_02)
    out.extend(per_mode(
        [&["D006_01_exc"], &["D006_01_inc"], &["D006_01_red"]],
        || vec![step("DELETE", &entity(V1, "/attrs/speed"), None)],
    ));
    out.extend(per_mode(
        [&["D006_02_exc"], &["D006_02_inc"], &["D006_02_red"]],
        || {
            vec![step(
                "DELETE",
                &entity(V1, "/attrs/speed?deleteAll=true"),
                None,
            )]
        },
    ));
    // Replace Entity (D007), Merge Entity (D008), Replace Attribute (D009)
    out.extend(per_mode(
        [&["D007_01_exc"], &["D007_01_inc"], &["D007_01_red"]],
        || {
            vec![step(
                "PUT",
                &entity(V1, ""),
                Some(json!({"id": V1, "type": "Vehicle", "speed": prop(json!(10))})),
            )]
        },
    ));
    out.extend(per_mode(
        [&["D008_01_exc"], &["D008_01_inc"], &["D008_01_red"]],
        || vec![step("PATCH", &entity(V1, ""), Some(speed(33)))],
    ));
    out.extend(per_mode(
        [&["D009_01_exc"], &["D009_01_inc"], &["D009_01_red"]],
        || {
            vec![step(
                "PUT",
                &entity(V1, "/attrs/speed"),
                Some(prop(json!(5))),
            )]
        },
    ));
    out
}

/// The batch operations per mode (D012..D016).
fn batches() -> Vec<Scenario> {
    let mut out = Vec::new();
    // Batch operations (D012..D016)
    let pair = || json!([vehicle(V1, BOTH), vehicle(V2, BOTH)]);
    for (mode, tps) in [
        ("exclusive", &["D012_01_exc"]),
        ("inclusive", &["D012_01_inc"]),
        ("redirect", &["D012_01_red"]),
    ] {
        let create = step(
            "POST",
            &batch("create", ""),
            Some(json!([vehicle(V1, BOTH)])),
        );
        out.push(s(tps, vec![], regs_of(mode), vec![create]));
    }
    out.extend(per_mode(
        [&["D013_01_exc"], &["D013_01_inc"], &["D013_01_red"]],
        || vec![step("POST", &batch("upsert", ""), Some(pair()))],
    ));
    out.extend(per_mode(
        [&["D013_02_exc"], &["D013_02_inc"], &["D013_02_red"]],
        || {
            vec![step(
                "POST",
                &batch("upsert", "?options=update"),
                Some(pair()),
            )]
        },
    ));
    out.extend(per_mode(
        [&["D014_01_exc"], &["D014_01_inc"], &["D014_01_red"]],
        || {
            vec![step(
                "POST",
                &batch("update", ""),
                Some(json!([vehicle(V1, &["speed"])])),
            )]
        },
    ));
    out.extend(per_mode(
        [&["D014_02_exc"], &["D014_02_inc"], &["D014_02_red"]],
        || {
            vec![step(
                "POST",
                &batch("update", "?options=noOverwrite"),
                Some(json!([vehicle(V1, &["speed"])])),
            )]
        },
    ));
    out.extend(per_mode(
        [&["D015_01_exc"], &["D015_01_inc"], &["D015_01_red"]],
        || vec![step("POST", &batch("delete", ""), Some(json!([V1])))],
    ));
    out.extend(per_mode(
        [&["D016_01_exc"], &["D016_01_inc"], &["D016_01_red"]],
        || {
            vec![step(
                "POST",
                &batch("merge", ""),
                Some(json!([{"id": V1, "type": "Vehicle", "speed": prop(json!(44))}])),
            )]
        },
    ));
    out
}

/// Every scenario.
fn scenarios() -> Vec<Scenario> {
    [
        creates(),
        deletes(),
        retrieves(),
        queries(),
        queries_kept(),
        others(),
        loops(),
        subscriptions(),
        writes(),
        batches(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Seeds, grants and registrations, then the steps; every answer and the
/// state after.
async fn run(side: &Side, sc: &Scenario) -> (Vec<Answer>, Vec<Vec<(String, Answer, Value)>>) {
    for target in TENANTS {
        for source in TENANTS {
            if target != source {
                let path = format!("/q/grants/{target}/{source}");
                let doc = json!({"target": target, "source": source, "access": "read-write"});
                let made = side.call("default", "PUT", &path, &[], Some(&doc)).await;
                assert!(matches!(made.status, 201 | 204), "{path}: {made:?}");
            }
        }
    }
    for (tenant, e) in &sc.seeds {
        let made = side
            .call(tenant, "POST", "/ngsi-ld/v1/entities", &[], Some(e))
            .await;
        assert_eq!(made.status, 201, "{:?} seed {tenant}: {made:?}", sc.tps);
    }
    for (n, r) in sc.regs.iter().enumerate() {
        let mut doc = json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:r{n}"),
            "type": "ContextSourceRegistration",
            "endpoint": side.url,
            "mode": r.mode,
            "operations": r.ops,
            "information": r.information,
            "tenant": r.target,
        });
        if let (Some(d), Some(e)) = (doc.as_object_mut(), r.extra.as_object()) {
            d.extend(e.clone());
        }
        let made = side
            .call(
                r.from,
                "POST",
                "/ngsi-ld/v1/csourceRegistrations",
                &[],
                Some(&doc),
            )
            .await;
        assert_eq!(made.status, 201, "{:?} registration {n}: {made:?}", sc.tps);
    }
    let mut answers = Vec::new();
    let mut states = Vec::new();
    for st in &sc.steps {
        answers.push(
            side.call("a", st.method, &st.path, &st.headers, st.body.as_ref())
                .await,
        );
        if st.look {
            states.push(side.settled().await);
        }
    }
    states.push(side.state().await);
    (answers, states)
}

/// Layer 2: every scenario answers and leaves the Tenants the same in
/// process as over HTTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn icsr_answers_every_federated_tp_as_http_does() {
    for sc in scenarios() {
        let http = side(false).await;
        let internal = side(true).await;
        let over_http = run(&http, &sc).await;
        let in_process = run(&internal, &sc).await;
        assert_eq!(in_process.0, over_http.0, "{:?}: answers", sc.tps);
        assert_eq!(in_process.1, over_http.1, "{:?}: state", sc.tps);
    }
}

/// The TP-list diff: the `DistributedOperations` TPs of the vendored suite
/// and the scenario table name the same TPs (the suite's duplicate
/// `D015_01_inc` once).
#[test]
fn icsr_every_federated_tp_has_a_case() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(TP_DIR);
    let mut found = BTreeSet::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let entries =
            std::fs::read_dir(&dir).unwrap_or_else(|e| unreachable!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "robot") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    found.insert(stem.to_owned());
                }
            }
        }
    }
    let table: BTreeSet<String> = scenarios()
        .iter()
        .flat_map(|sc| sc.tps.iter().map(|t| (*t).to_owned()))
        .collect();
    let missing: Vec<&String> = found.difference(&table).collect();
    let stale: Vec<&String> = table.difference(&found).collect();
    assert!(missing.is_empty(), "TPs without a case: {missing:?}");
    assert!(stale.is_empty(), "cases for no TP: {stale:?}");
}

/// The table names each TP once.
#[test]
fn icsr_no_tp_has_two_cases() {
    let all: Vec<&str> = scenarios()
        .iter()
        .flat_map(|sc| sc.tps.iter().copied())
        .collect();
    let once: BTreeSet<&str> = all.iter().copied().collect();
    assert_eq!(all.len(), once.len());
}

/// What a scenario left behind is compared per Tenant, so a test whose
/// brokers share nothing cannot pass by both being empty: the HTTP side of
/// `D001_01_inc` holds the Vehicle in both Tenants.
#[tokio::test]
async fn icsr_the_equivalence_compares_real_state() {
    let sc = scenarios()
        .into_iter()
        .find(|sc| sc.tps.contains(&"D001_01_inc"))
        .unwrap_or_else(|| unreachable!("D001_01_inc has a case"));
    let http = side(false).await;
    let (answers, states) = run(&http, &sc).await;
    assert_eq!(answers.first().map(|a| a.status), Some(201));
    for (tenant, vehicles, _) in states.iter().flatten() {
        let held = vehicles.body.as_array().map_or(0, Vec::len);
        let want = usize::from(tenant == "a" || tenant == "b");
        assert_eq!(held, want, "{tenant}: {:?}", vehicles.body);
    }
}
