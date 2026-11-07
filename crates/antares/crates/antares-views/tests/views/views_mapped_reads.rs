// SPDX-License-Identifier: EUPL-1.2
//! The temporal and discovery reads through a view of another model
//! (`docs/analysis/view-mapping.md` sections 4.1 and 7, T-3670): `vehicles`
//! against `plain`, the same view without its mapping.

use crate::common::{get, put_view, send, Answer, Fixture};
use crate::views_mapped::{speaks_the_target_model, vehicles, vehicles_view, ROOT};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

const PLAIN: &str = "/x/views/plain/ngsi-ld/v1";
/// The fixture's instances carry no `observedAt`: `timeproperty` passes
/// through the mapping as written. Car:3 is left out: what its history
/// shows through the rule's `q` varies under load (chyby.md, 2026-10-11).
const SINCE: &str = "timerel=after&timeAt=1970-01-01T00:00:00Z&timeproperty=modifiedAt&id=urn:ngsi-ld:Car:1,urn:ngsi-ld:Car:2";

/// The fixture with `vehicles` and `plain`.
async fn twins() -> Result<Fixture, String> {
    let f = vehicles().await?;
    let mut plain = vehicles_view();
    if let Some(m) = plain.as_object_mut() {
        m.remove("mapping");
        m.insert("id".into(), json!("plain"));
    }
    put_view(&f.app, &plain).await?;
    // the history of the fixture's last PATCH lands after its 204; every
    // read below waits for it, so both views read the same history
    let root = "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Car:3?timerel=after&timeAt=1970-01-01T00:00:00Z&timeproperty=modifiedAt";
    for _ in 0..200 {
        let a = send(
            &f.app,
            Method::GET,
            root,
            &[("NGSILD-Tenant", "city")],
            None,
        )
        .await;
        if at(&a.json, "/speed")
            .as_array()
            .is_some_and(|i| i.len() == 2)
        {
            return Ok(f);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err("the history of Car:3 never held both speeds".into())
}

/// A temporal Entity of `plain` as `vehicles` rebuilds it: `velocity` is
/// `speed` times 2, `paint` is `colour` through the table, `ownedBy` is
/// `owner`; the constant and the expression have no history.
fn rebuilt(plain: &Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("id".into(), at(plain, "/id"));
    out.insert("type".into(), json!("Vehicle"));
    let each = |attr: &Value, f: &dyn Fn(&Value) -> Value| -> Value {
        Value::Array(
            attr.as_array()
                .into_iter()
                .flatten()
                .map(|i| {
                    let mut i = i.clone();
                    if let Some(v) = i.get_mut("value") {
                        *v = f(v);
                    }
                    i
                })
                .collect(),
        )
    };
    if let Some(speed) = plain.get("speed") {
        let doubled = each(speed, &|v| json!(v.as_f64().unwrap_or_default() * 2.0));
        out.insert("velocity".into(), doubled);
    }
    if let Some(colour) = plain.get("colour") {
        let painted = each(colour, &|v| match v.as_str() {
            Some("red") => json!("rot"),
            Some("blue") => json!("blau"),
            _ => v.clone(),
        });
        out.insert("paint".into(), painted);
    }
    if let Some(owner) = plain.get("owner") {
        out.insert("ownedBy".into(), owner.clone());
    }
    Value::Object(out)
}

/// The member at JSON pointer `path`, `null` when there is none.
fn at(v: &Value, path: &str) -> Value {
    v.pointer(path).cloned().unwrap_or_default()
}

fn page(a: &Answer) -> Value {
    Value::Array(
        a.json
            .as_array()
            .into_iter()
            .flatten()
            .map(rebuilt)
            .collect(),
    )
}

fn ok(a: &Answer, what: &str) {
    assert_eq!(a.status, StatusCode::OK, "{what}: {}", a.text);
    speaks_the_target_model(a);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_through_the_mapping_is_the_rebuilt_history_of_the_view() -> Result<(), String> {
    let f = twins().await?;
    for id in ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"] {
        let mapped = get(&f.app, &format!("{ROOT}/temporal/entities/{id}")).await;
        let plain = get(&f.app, &format!("{PLAIN}/temporal/entities/{id}")).await;
        ok(&mapped, id);
        assert_eq!(plain.status, StatusCode::OK, "{}", plain.text);
        assert_eq!(mapped.json, rebuilt(&plain.json), "{id}");
        assert!(mapped.json.get("velocity").is_some(), "{}", mapped.text);
    }
    let mapped = get(
        &f.app,
        &format!("{ROOT}/temporal/entities?type=Vehicle&{SINCE}"),
    )
    .await;
    let plain = get(
        &f.app,
        &format!("{PLAIN}/temporal/entities?type=Car&{SINCE}"),
    )
    .await;
    ok(&mapped, "temporal query");
    assert!(!mapped.ids().is_empty(), "{}", mapped.text);
    assert_eq!(mapped.json, page(&plain));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_filters_and_projects_in_the_target_model() -> Result<(), String> {
    let f = twins().await?;
    for (target, source) in [
        (
            "q=paint==%22rot%22&attrs=velocity",
            "q=colour==%22red%22&attrs=speed",
        ),
        ("q=velocity%3E30", "q=speed%3E15"),
        ("q=ownedBy&attrs=paint", "q=owner&attrs=colour"),
    ] {
        let a = get(
            &f.app,
            &format!("{ROOT}/temporal/entities?type=Vehicle&{SINCE}&{target}"),
        )
        .await;
        let plain = get(
            &f.app,
            &format!("{PLAIN}/temporal/entities?type=Car&{SINCE}&{source}"),
        )
        .await;
        ok(&a, target);
        assert!(!a.ids().is_empty(), "{target}: {}", a.text);
        assert_eq!(a.ids(), plain.ids(), "{target}");
        assert_eq!(a.json, page(&plain), "{target}");
    }
    let a = get(
        &f.app,
        &format!("{ROOT}/temporal/entities/urn:ngsi-ld:Car:1?options=temporalValues&attrs=paint,velocity"),
    )
    .await;
    ok(&a, "temporalValues");
    assert_eq!(at(&a.json, "/paint/values/0/0"), json!("rot"), "{}", a.text);
    assert_eq!(
        at(&a.json, "/velocity/values/0/0"),
        json!(20.0),
        "{}",
        a.text
    );
    for (uri, status) in [
        (format!("{ROOT}/temporal/entities?type=Car&{SINCE}"), StatusCode::OK),
        (
            format!("{ROOT}/temporal/entities/urn:ngsi-ld:Car:1?type=Car"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{ROOT}/temporal/entities/urn:ngsi-ld:Car:404"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{ROOT}/temporal/entities/urn:ngsi-ld:Car:1?options=aggregatedValues&aggrMethods=avg"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{ROOT}/temporal/entities?type=Vehicle&{SINCE}&q=speed%3C5"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, status, "{uri}: {}", a.text);
        speaks_the_target_model(&a);
        if status == StatusCode::OK {
            assert_eq!(a.json, json!([]), "another type: nothing");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_names_only_the_target_model_and_what_the_view_shows() -> Result<(), String> {
    let f = twins().await?;
    let a = get(&f.app, &format!("{ROOT}/types")).await;
    ok(&a, "/types");
    assert_eq!(at(&a.json, "/typeList"), json!(["Vehicle"]));
    let shown = json!(["velocity", "paint", "ownedBy", "maker", "label"]);
    let a = get(&f.app, &format!("{ROOT}/types?details=true")).await;
    ok(&a, "/types?details=true");
    assert_eq!(
        a.json,
        json!([{"id": "https://uri.etsi.org/ngsi-ld/default-context/Vehicle", "type": "EntityType",
                "typeName": "Vehicle", "attributeNames": shown}])
    );
    let a = get(&f.app, &format!("{ROOT}/types/Vehicle")).await;
    ok(&a, "/types/Vehicle");
    let plain = get(&f.app, &format!("{PLAIN}/types/Car")).await;
    assert_eq!(
        at(&a.json, "/entityCount"),
        at(&plain.json, "/entityCount"),
        "{}",
        a.text
    );
    let names: Vec<Value> = at(&a.json, "/attributeDetails")
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| at(d, "/attributeName"))
        .collect();
    assert_eq!(Value::Array(names), shown);
    let a = get(&f.app, &format!("{ROOT}/attributes")).await;
    ok(&a, "/attributes");
    assert_eq!(at(&a.json, "/attributeList"), shown);
    let a = get(&f.app, &format!("{ROOT}/attributes?details=true")).await;
    ok(&a, "/attributes?details=true");
    for d in a.json.as_array().into_iter().flatten() {
        assert_eq!(at(d, "/typeNames"), json!(["Vehicle"]), "{d}");
    }
    let a = get(&f.app, &format!("{ROOT}/attributes/velocity")).await;
    ok(&a, "/attributes/velocity");
    let plain = get(&f.app, &format!("{PLAIN}/attributes/speed")).await;
    assert_eq!(
        (
            at(&a.json, "/attributeName"),
            at(&a.json, "/attributeCount"),
            at(&a.json, "/typeNames")
        ),
        (
            json!("velocity"),
            at(&plain.json, "/attributeCount"),
            json!(["Vehicle"])
        )
    );
    for (uri, status) in [
        ("/types/Car", StatusCode::NOT_FOUND),
        ("/types/Person", StatusCode::NOT_FOUND),
        ("/attributes/hidden", StatusCode::NOT_FOUND),
        ("/attributes/speed", StatusCode::BAD_REQUEST),
        ("/attributes/maker", StatusCode::BAD_REQUEST),
        ("/types?type=Vehicle", StatusCode::BAD_REQUEST),
    ] {
        let a = get(&f.app, &format!("{ROOT}{uri}")).await;
        assert_eq!(a.status, status, "{uri}: {}", a.text);
        speaks_the_target_model(&a);
    }
    let a = send(
        &f.app,
        Method::GET,
        &format!("{ROOT}/types/Vehicle"),
        &[("Accept", "application/ld+json")],
        None,
    )
    .await;
    ok(&a, "/types/Vehicle as JSON-LD");
    assert_eq!(
        at(&a.json, "/@context"),
        json!("https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld"),
        "the view's @context, never the inner one"
    );
    // `hidden` reads `secret`, which neither view shows
    for uri in [
        "/types?details=true",
        "/attributes",
        "/attributes?details=true",
        "/types/Vehicle",
    ] {
        let a = get(&f.app, &format!("{ROOT}{uri}")).await;
        assert!(!a.text.contains("hidden"), "{uri}: {}", a.text);
    }
    for uri in ["/types?details=true", "/attributes", "/types/Car"] {
        let plain = get(&f.app, &format!("{PLAIN}{uri}")).await;
        assert_eq!(plain.status, StatusCode::OK, "{uri}");
        assert!(!plain.text.contains("secret"), "{uri}: {}", plain.text);
    }
    Ok(())
}
