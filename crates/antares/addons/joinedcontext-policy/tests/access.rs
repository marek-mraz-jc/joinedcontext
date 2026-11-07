// SPDX-License-Identifier: EUPL-1.2
//! The access surface through the broker's router (README section 4): the
//! cases of the platform's `access_authzen_tests`, `access_odrl_tests` and
//! `access_ucast_tests`, carried over (T-3638), and the rule that an answer
//! never names a rule of another subject.

use antares_ops::policy::PolicyEngine;
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use joinedcontext_policy::surface::Access;
use joinedcontext_policy::JoinedcontextPolicy;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

type Res = Result<(), String>;

const CITY: &str = "did:web:banskabystrica.sk";
const ANONYMOUS: [(&str, &str); 1] = [("x-jc-roles", "")];
const STEWARD: [(&str, &str); 2] = [
    ("x-jc-subject", "bo@example.org"),
    ("x-jc-roles", "steward"),
];

/// The public grant of DEMO step 4, a grant for somebody else, and a
/// prohibition taking one operation back from the public.
fn doc() -> Value {
    json!({"policies": [
        {"id": "ovzdusie/public-read", "assigner": CITY,
         "assignee": {"kind": "role", "id": "public"},
         "operations": ["5.7.1", "5.7.2", "5.7.4"],
         "information": [{"entities": [{"type": "AirQualityObserved",
                                        "idPattern": "^urn:ngsi-ld:AirQualityObserved:banskabystrica\\.sk:ovzdusie:.*$"}],
                          "propertyNames": ["pm10", "pm25", "location"]}],
         "q": "pm10>=0", "scopeQ": "/geo/SK/BB"},
        {"id": "ovzdusie/steward-incidents", "assigner": CITY,
         "assignee": {"kind": "role", "id": "steward"},
         "operations": ["5.6.1", "5.6.4", "5.7.2"],
         "information": [{"entities": [{"type": "InternalIncident"}],
                          "propertyNames": ["severity", "reportedBy"]}]},
        {"id": "ovzdusie/no-public-history", "effect": "prohibit", "assigner": CITY,
         "assignee": {"kind": "role", "id": "public"}, "operations": ["5.7.4"],
         "information": [{"entities": [{"type": "AirQualityObserved"}]}]}
    ]})
}

/// The broker with the access surface over `engine`, `doc()` loaded for
/// the Tenant `city`, and the view `air` of `city`. The broker itself asks
/// no engine, so the view is written as an operator writes it.
async fn setup() -> Result<(TestBroker, Router, Arc<JoinedcontextPolicy>), String> {
    static HEADERS: std::sync::Once = std::sync::Once::new();
    HEADERS.call_once(|| {
        std::env::set_var(
            "ANTARES_POLICY_SUBJECT_HEADERS",
            joinedcontext_policy::subject::HEADERS.join(","),
        );
    });
    let engine = Arc::new(JoinedcontextPolicy::new());
    engine.load("city", &doc())?;
    let broker = TestBroker::new().await?;
    let app = broker.serve(Access::new(engine.clone()))?;
    let view = json!({"id": "air", "tenant": "city",
        "selector": ["AirQualityObserved", "InternalIncident"],
        "rules": [{"types": ["AirQualityObserved", "InternalIncident"], "attributes": ["pm10"]}],
        "operations": ["query_entities", "query_entities_post", "retrieve_entity",
                       "temporal_query", "temporal_query_post", "create_entity",
                       "partial_attribute_update"]});
    let (status, _, body) = call(&app, "PUT", "/q/views/air", &[], Some(&view.to_string())).await?;
    if status != StatusCode::CREATED {
        return Err(format!("the view: {status} {body}"));
    }
    Ok((broker, app, engine))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<(StatusCode, String, String), String> {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    if let Some(body) = body {
        b = b.header("Content-Type", "application/json");
        b = b.header("Content-Length", body.len());
    }
    let req = b
        .body(Body::from(body.unwrap_or_default().to_owned()))
        .map_err(|e| e.to_string())?;
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let media = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((status, media, String::from_utf8_lossy(&bytes).into_owned()))
}

async fn document(
    app: &Router,
    who: &[(&str, &str)],
    accept: Option<&str>,
) -> Result<String, String> {
    let mut headers = who.to_vec();
    if let Some(accept) = accept {
        headers.push(("Accept", accept));
    }
    let (status, _, body) = call(app, "GET", "/x/views/air/access", &headers, None).await?;
    if status != StatusCode::OK {
        return Err(format!("{status} {body}"));
    }
    Ok(body)
}

/// The member at `path` (a JSON pointer), `null` when absent.
fn at(v: &Value, path: &str) -> Value {
    v.pointer(path).cloned().unwrap_or(Value::Null)
}

fn parse(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("{e}: {text}"))
}

async fn check(app: &Router, who: &[(&str, &str)], query: &Value) -> Result<Value, String> {
    let (status, _, body) = call(
        app,
        "POST",
        "/x/views/air/access/check",
        who,
        Some(&query.to_string()),
    )
    .await?;
    if status != StatusCode::OK {
        return Err(format!("{status} {body}"));
    }
    parse(&body)
}

/// `access_authzen_tests::the_access_document_lists_the_callers_grants`
#[tokio::test]
async fn the_access_document_lists_the_callers_grants() -> Res {
    let (_b, app, _) = setup().await?;
    let d = parse(&document(&app, &ANONYMOUS, None).await?)?;
    assert_eq!(at(&d, "/subject"), json!({"type": "role", "id": "public"}));
    assert_eq!(at(&d, "/resource"), json!({"type": "view", "id": "air"}));
    let permissions = at(&d, "/permissions");
    assert_eq!(
        permissions.as_array().map_or(0, Vec::len),
        1,
        "one granted type: {d}"
    );
    let g = &at(&permissions, "/0");
    assert_eq!(at(g, "/resource/type"), "AirQualityObserved");
    assert_eq!(
        at(g, "/actions"),
        json!([
            "queryBatch",
            "queryEntity",
            "queryTemporal",
            "retrieveEntity"
        ])
    );
    assert_eq!(at(g, "/attributes"), json!(["location", "pm10", "pm25"]));
    assert_eq!(
        at(g, "/constraints"),
        json!({"q": "pm10>=0", "scopeQ": "/geo/SK/BB"})
    );
    assert!(at(g, "/resource/idPatterns/0").is_string());
    Ok(())
}

/// `access_authzen_tests::a_type_the_caller_may_not_touch_is_absent_from_the_document`,
/// and the same for every representation: no rule of another subject.
#[tokio::test]
async fn a_type_the_caller_may_not_touch_is_absent_from_every_representation() -> Res {
    let (_b, app, _) = setup().await?;
    for accept in [
        None,
        Some("application/odrl+json"),
        Some("text/turtle"),
        Some("application/vnd.joinedcontext.grant-ast+json"),
    ] {
        let text = document(&app, &ANONYMOUS, accept).await?;
        for hidden in [
            "InternalIncident",
            "severity",
            "reportedBy",
            "steward",
            "ovzdusie/",
        ] {
            assert!(!text.contains(hidden), "{accept:?} names {hidden}: {text}");
        }
    }
    let steward = document(&app, &STEWARD, None).await?;
    for hidden in ["AirQualityObserved", "pm10", "public"] {
        assert!(
            !steward.contains(hidden),
            "the steward sees {hidden}: {steward}"
        );
    }
    Ok(())
}

/// `access_authzen_tests::a_prohibition_that_applies_to_the_caller_is_reported`
#[tokio::test]
async fn a_prohibition_that_applies_to_the_caller_is_reported() -> Res {
    let (_b, app, _) = setup().await?;
    let d = parse(&document(&app, &ANONYMOUS, None).await?)?;
    let p = at(&d, "/prohibitions");
    assert_eq!(p.as_array().map_or(0, Vec::len), 1);
    assert_eq!(at(&p, "/0/resource/type"), "AirQualityObserved");
    assert_eq!(at(&p, "/0/actions"), json!(["queryTemporal"]));
    assert_eq!(at(&p, "/0/attributes"), json!("*"));
    Ok(())
}

/// `access_authzen_tests::the_check_endpoint_answers_one_prospective_request`
#[tokio::test]
async fn the_check_endpoint_answers_one_prospective_request() -> Res {
    let (_b, app, _) = setup().await?;
    let yes = check(
        &app,
        &ANONYMOUS,
        &json!({"subject": {"id": "anonymous"}, "action": {"name": "queryEntity"},
                "resource": {"type": "AirQualityObserved"}}),
    )
    .await?;
    assert_eq!(
        yes,
        json!({"decision": true, "context": {"reason": "policy_grant_matched", "assigner": CITY}})
    );
    for refused in [
        json!({"action": {"name": "deleteEntity"}, "resource": {"type": "AirQualityObserved"}}),
        json!({"action": {"name": "queryEntity"}, "resource": {"type": "InternalIncident"}}),
        // prohibited, although a permission names the operation (GW8)
        json!({"action": {"name": "queryTemporal"}, "resource": {"type": "AirQualityObserved"}}),
        // an operation the view serves no row of
        json!({"action": {"name": "createSubscription"}}),
    ] {
        assert_eq!(
            check(&app, &ANONYMOUS, &refused).await?,
            json!({"decision": false}),
            "{refused}"
        );
    }
    let steward = check(
        &app,
        &STEWARD,
        &json!({"action": {"name": "createEntity"}, "resource": {"type": "InternalIncident"}}),
    )
    .await?;
    assert_eq!(at(&steward, "/decision"), true);
    Ok(())
}

/// `access_simulate_tests`: explain answers what check answers and names
/// the policy that decided, a refusal by prohibition included (T-3881).
#[tokio::test]
async fn explain_names_the_policy_that_decided() -> Res {
    let (_b, app, _) = setup().await?;
    let explain = |who: &'static [(&'static str, &'static str)], query: Value| {
        let app = app.clone();
        async move {
            let (status, media, body) = call(
                &app,
                "POST",
                "/x/views/air/access/explain",
                who,
                Some(&query.to_string()),
            )
            .await?;
            if status != StatusCode::OK || media != "application/json" {
                return Err(format!("{status} {media} {body}"));
            }
            parse(&body)
        }
    };
    let read =
        json!({"action": {"name": "queryEntity"}, "resource": {"type": "AirQualityObserved"}});
    assert_eq!(
        explain(&ANONYMOUS, read.clone()).await?,
        json!({"decision": true, "context": {"reason": "policy_grant_matched",
               "policy": "ovzdusie/public-read", "assigner": CITY}})
    );
    let history =
        json!({"action": {"name": "queryTemporal"}, "resource": {"type": "AirQualityObserved"}});
    assert_eq!(
        explain(&ANONYMOUS, history.clone()).await?,
        json!({"decision": false, "context": {"reason": "prohibited",
               "policy": "ovzdusie/no-public-history", "assigner": CITY}})
    );
    let incident =
        json!({"action": {"name": "queryEntity"}, "resource": {"type": "InternalIncident"}});
    assert_eq!(
        explain(&ANONYMOUS, incident.clone()).await?,
        json!({"decision": false, "context": {"reason": "no_grant"}})
    );
    // the steward's grant is named to the steward alone
    assert_eq!(
        at(
            &explain(&STEWARD, incident.clone()).await?,
            "/context/policy"
        ),
        "ovzdusie/steward-incidents"
    );
    for (who, query) in [
        (&ANONYMOUS[..], &read),
        (&ANONYMOUS[..], &history),
        (&ANONYMOUS[..], &incident),
        (&STEWARD[..], &incident),
    ] {
        assert_eq!(
            at(&explain(who, query.clone()).await?, "/decision"),
            at(&check(&app, who, query).await?, "/decision"),
            "{query}"
        );
    }
    Ok(())
}

/// `access_authzen_tests::the_access_surface_refuses_the_same_way_the_data_surface_does`
#[tokio::test]
async fn the_access_surface_refuses_the_same_way_the_data_surface_does() -> Res {
    let (_b, app, _) = setup().await?;
    for (method, uri) in [
        ("GET", "/x/views/nosuch/access"),
        ("POST", "/x/views/nosuch/access/check"),
        ("POST", "/x/views/nosuch/access/explain"),
    ] {
        let (status, _, body) = call(&app, method, uri, &ANONYMOUS, Some("{}")).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
    }
    for (uri, body) in ["check", "explain"].iter().flat_map(|route| {
        ["{}", "not json", r#"{"action": {"name": 7}}"#]
            .map(|body| (format!("/x/views/air/access/{route}"), body))
    }) {
        let (status, _, text) = call(&app, "POST", &uri, &ANONYMOUS, Some(body)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}: {text}");
        assert!(
            !text.contains("not json"),
            "a refusal never echoes the body: {text}"
        );
    }
    let huge = format!(r#"{{"action": {{"name": "{}"}}}}"#, "a".repeat(70 * 1024));
    for uri in ["/x/views/air/access/check", "/x/views/air/access/explain"] {
        let (status, _, _) = call(&app, "POST", uri, &ANONYMOUS, Some(&huge)).await?;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}");
    }
    Ok(())
}

/// `access_authzen_tests::a_named_representation_is_the_one_that_comes_back`
#[tokio::test]
async fn a_named_representation_is_the_one_that_comes_back() -> Res {
    let (_b, app, _) = setup().await?;
    for (accept, media) in [
        ("application/odrl+json", "application/odrl+json"),
        ("text/turtle", "text/turtle"),
        (
            "application/vnd.joinedcontext.grant-ast+json",
            "application/vnd.joinedcontext.grant-ast+json",
        ),
        ("application/json", "application/json"),
        ("*/*", "application/json"),
        ("text/turtle, application/json", "text/turtle"),
        ("text/csv", "application/json"),
    ] {
        let (status, got, _) = call(
            &app,
            "GET",
            "/x/views/air/access",
            &[("Accept", accept)],
            None,
        )
        .await?;
        assert_eq!(status, StatusCode::OK, "{accept}");
        assert_eq!(got, media, "{accept}");
    }
    Ok(())
}

/// `access_odrl_tests`: the grants as an ODRL Set naming the caller, its
/// uid under the broker's public URL, the same rules as Turtle.
#[tokio::test]
async fn the_odrl_policy_and_its_turtle_say_the_same_grants() -> Res {
    let (_b, app, _) = setup().await?;
    let who = [
        ("x-jc-subject", "ana@example.org"),
        ("Host", "broker.example"),
    ];
    let d = parse(&document(&app, &who, Some("application/odrl+json")).await?)?;
    assert_eq!(at(&d, "/@type"), "Set");
    assert_eq!(at(&d, "/assignee"), "ana@example.org");
    assert_eq!(at(&d, "/permission"), json!([]), "ana holds nothing here");
    let uid = at(&d, "/uid");
    let uid = uid.as_str().ok_or("uid")?;
    assert!(
        uid.starts_with("http") && uid.contains("/x/views/air/access#sha256:"),
        "{uid}"
    );
    let public = parse(&document(&app, &ANONYMOUS, Some("application/odrl+json")).await?)?;
    assert_eq!(at(&public, "/assigner"), CITY);
    assert_eq!(
        at(&public, "/permission/0/target/uid"),
        "AirQualityObserved"
    );
    assert_eq!(
        at(&public, "/prohibition/0/action"),
        json!(["ngsi-ld:queryTemporal"])
    );
    assert_ne!(
        at(&public, "/uid"),
        at(&d, "/uid"),
        "different grants, different uid"
    );
    let ttl = document(&app, &ANONYMOUS, Some("text/turtle")).await?;
    assert!(ttl.contains(&format!(
        "<{}> a odrl:Set",
        at(&public, "/uid").as_str().ok_or("uid")?
    )));
    assert!(ttl.contains("odrl:prohibition ["));
    assert!(ttl.contains(r#"odrl:rightOperand "pm10>=0""#), "{ttl}");
    Ok(())
}

/// `access_ucast_tests`: one entry per type, the prohibited operation off.
#[tokio::test]
async fn the_grant_ast_takes_the_prohibited_operation_off() -> Res {
    let (_b, app, _) = setup().await?;
    let d = parse(
        &document(
            &app,
            &ANONYMOUS,
            Some("application/vnd.joinedcontext.grant-ast+json"),
        )
        .await?,
    )?;
    assert_eq!(
        d,
        json!({"AirQualityObserved": {
            "operations": ["queryBatch", "queryEntity", "retrieveEntity"],
            "project": ["location", "pm10", "pm25"],
            "q": "pm10>=0",
            "where": {"type": "field", "operator": "scope_under", "field": "scope", "value": "/geo/SK/BB"}
        }})
    );
    Ok(())
}

/// EP-60: the surface reads the table the engine decides with. A document
/// loaded after the surface was mounted is what it shows, and one taken
/// away leaves nothing granted.
#[tokio::test]
async fn the_surface_answers_from_the_engines_current_table() -> Res {
    let (_b, app, engine) = setup().await?;
    engine.load("city", &json!({"policies": []}))?;
    let d = parse(&document(&app, &ANONYMOUS, None).await?)?;
    assert_eq!(at(&d, "/permissions"), json!([]));
    engine.load("city", &Value::Null)?;
    let none = check(
        &app,
        &ANONYMOUS,
        &json!({"action": {"name": "queryEntity"}}),
    )
    .await?;
    assert_eq!(none, json!({"decision": false}));
    // another Tenant's table never reaches this view
    engine.load("other", &doc())?;
    let d = parse(&document(&app, &ANONYMOUS, None).await?)?;
    assert_eq!(at(&d, "/permissions"), json!([]));
    Ok(())
}

/// A call from inside the cluster (no `x-jc-*` header) holds no policy.
#[tokio::test]
async fn the_clusters_own_caller_holds_nothing() -> Res {
    let (_b, app, _) = setup().await?;
    let d = parse(&document(&app, &[], None).await?)?;
    assert_eq!(at(&d, "/subject"), json!({"type": "internal"}));
    assert_eq!(at(&d, "/permissions"), json!([]));
    let no = check(&app, &[], &json!({"action": {"name": "queryEntity"}})).await?;
    assert_eq!(no, json!({"decision": false}));
    Ok(())
}

/// T-3702: through a view the document lists, the access document is the
/// view's policies alone, as the engine decides there.
#[tokio::test]
async fn a_view_bound_to_policies_describes_those_alone() -> Res {
    let (_broker, app, engine) = setup().await?;
    let before = document(&app, &STEWARD, None).await?;
    assert!(before.contains("InternalIncident"), "{before}");

    let mut bound = doc();
    if let Some(o) = bound.as_object_mut() {
        o.insert("views".into(), json!({"air": ["ovzdusie/public-read"]}));
    }
    engine.load("city", &bound)?;
    let after = document(&app, &STEWARD, None).await?;
    assert!(!after.contains("InternalIncident"), "{after}");
    let decision = check(
        &app,
        &STEWARD,
        &json!({"action": {"name": "createEntity"}, "resource": {"type": "InternalIncident"}}),
    )
    .await?;
    assert_eq!(at(&decision, "/decision"), json!(false), "{decision}");
    Ok(())
}

/// T-5527: `resource.properties.attribute` narrows the check to one Attribute, and
/// `access/attributes` answers the caller's read, write and delete per Attribute of a type.
#[tokio::test]
async fn the_check_and_the_attribute_table_answer_per_attribute() -> Res {
    let (_b, app, _) = setup().await?;
    let on = |attribute: &str| {
        json!({"action": {"name": "retrieveEntity"},
               "resource": {"type": "AirQualityObserved", "properties": {"attribute": attribute}}})
    };
    assert_eq!(
        at(&check(&app, &ANONYMOUS, &on("pm10")).await?, "/decision"),
        true
    );
    assert_eq!(
        check(&app, &ANONYMOUS, &on("temperature")).await?,
        json!({"decision": false})
    );
    let ask = json!({"type": "InternalIncident", "attributes": ["severity", "notes"]});
    let (status, _, body) = call(
        &app,
        "POST",
        "/x/views/air/access/attributes",
        &STEWARD,
        Some(&ask.to_string()),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    // the view serves no delete_attribute, so no delete is granted through it
    assert_eq!(
        parse(&body)?,
        json!({"type": "InternalIncident", "attributes": [
            {"name": "severity", "read": false, "write": true, "delete": false, "reason": "grant"},
            {"name": "notes", "read": false, "write": false, "delete": false, "reason": "not_in_grant"}
        ]})
    );
    assert!(!body.contains("ovzdusie"), "a policy id in {body}");
    let (_, _, anonymous) = call(
        &app,
        "POST",
        "/x/views/air/access/attributes",
        &ANONYMOUS,
        Some(&ask.to_string()),
    )
    .await?;
    assert_eq!(at(&parse(&anonymous)?, "/attributes/0/reason"), "no_grant");
    let names: Vec<String> = (0..=200).map(|i| format!("a{i}")).collect();
    for malformed in [
        json!({"attributes": ["severity"]}),
        json!({"type": "InternalIncident"}),
        json!({"type": "InternalIncident", "attributes": []}),
        json!({"type": "InternalIncident", "attributes": [1]}),
        json!({"type": "InternalIncident", "attributes": names}),
    ] {
        let (status, _, body) = call(
            &app,
            "POST",
            "/x/views/air/access/attributes",
            &STEWARD,
            Some(&malformed.to_string()),
        )
        .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{malformed} {body}");
    }
    let (status, _, _) = call(
        &app,
        "POST",
        "/x/views/nosuch/access/attributes",
        &STEWARD,
        Some(&ask.to_string()),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}
