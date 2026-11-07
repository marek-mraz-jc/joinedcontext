// SPDX-License-Identifier: EUPL-1.2
//! Navigation in a query and the write preferences (T-5906): `any` over a collection-valued
//! navigation as the linked-entity terms of TS 104 175 4.9, `$orderby` by a member of a
//! single-valued navigation's target, the key under `$select`, a bind setting the Property its
//! navigation's `ReferentialConstraint` names, and `Prefer: return=` on PATCH.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const SHOP: &str = "https://example.org/shop/";
const ROOT: &str = "/x/views/shop/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/shop", "name": "shop",
  "prefixes": {"shop": {"prefix_prefix": "shop", "prefix_reference": "https://example.org/shop/"}},
  "default_prefix": "shop",
  "classes": {"Product": {"slots": ["ID", "Name", "Price", "CategoryID", "Group", "Notes", "Related"]},
              "Category": {"slots": ["code", "Name"]},
              "Note": {"slots": ["key", "Lang", "Secret"]}},
  "slots": {"ID": {"identifier": true},
            "Name": {}, "Price": {"range": "float"}, "CategoryID": {},
            "Group": {"range": "Category", "annotations": {"odata_referential_constraint":
                {"tag": "odata_referential_constraint", "value": "CategoryID"}}},
            "Notes": {"range": "Note", "multivalued": true},
            "Related": {"range": "Product", "multivalued": true},
            "code": {"identifier": true}, "key": {"identifier": true},
            "Lang": {}, "Secret": {}}
}"#;

struct Answer {
    status: StatusCode,
    applied: String,
    body: Value,
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    prefer: Option<&str>,
) -> Answer {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "shop");
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    if let Some(p) = prefer {
        req = req.header("Prefer", p);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let applied = resp
        .headers()
        .get("Preference-Applied")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Answer {
        status,
        applied,
        body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
    }
}

async fn get(app: &Router, path: &str) -> Answer {
    send(app, Method::GET, &format!("{ROOT}/{path}"), None, None).await
}

/// The `Name` of each Product a read answers, in its order.
fn names(a: &Answer) -> Vec<String> {
    a.body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p["Name"].as_str().map(str::to_owned))
        .collect()
}

fn property(value: &str) -> Value {
    json!({"type": "Property", "value": value})
}

fn rel(objects: &[&str]) -> Value {
    json!({"type": "Relationship", "object": objects})
}

/// Tenant `shop`: Drill (notes EN, DE; category tools), Saw (note FR; category garden; related
/// Drill), Nail (no notes, related Saw). The view hides `Note.Secret`.
async fn shop() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new().await?;
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "shop", "tenant": "shop", "@context": {"shop": SHOP},
        "selector": ["shop:Product", "shop:Category", "shop:Note"],
        "rules": [{"types": ["shop:Product"], "attributes": ["shop:ID", "shop:Name", "shop:Price",
                    "shop:CategoryID", "shop:Group", "shop:Notes", "shop:Related"]},
                  {"types": ["shop:Category"], "attributes": ["shop:code", "shop:Name"]},
                  {"types": ["shop:Note"], "attributes": ["shop:key", "shop:Lang"]}],
        "operations": ["query_entities", "retrieve_entity", "create_entity", "merge_entity"]
    });
    for (uri, body) in [
        ("/q/views/shop", view.to_string()),
        ("/q/schemas/shop", SCHEMA.to_owned()),
    ] {
        let a = send(&app, Method::PUT, uri, Some(&body), None).await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{uri}: {} {}", a.status, a.body));
        }
    }
    let t = |class: &str| format!("{SHOP}{class}");
    let a = |name: &str| format!("{SHOP}{name}");
    let mut entities = Vec::new();
    for (code, name) in [("tools", "Tools"), ("garden", "Garden")] {
        entities.push(
            json!({"id": format!("urn:ngsi-ld:Category:{code}"), "type": t("Category"),
            a("code"): property(code), a("Name"): property(name)}),
        );
    }
    for (key, lang) in [("n1", "EN"), ("n2", "DE"), ("n3", "FR")] {
        entities.push(
            json!({"id": format!("urn:ngsi-ld:Note:{key}"), "type": t("Note"),
            a("key"): property(key), a("Lang"): property(lang), a("Secret"): property("hush")}),
        );
    }
    for (id, name, category, notes, related) in [
        ("drill", "Drill", "tools", &["n1", "n2"][..], &[][..]),
        ("saw", "Saw", "garden", &["n3"], &["drill"]),
        ("nail", "Nail", "tools", &[], &["saw"]),
    ] {
        let mut e = json!({"id": format!("urn:ngsi-ld:Product:{id}"), "type": t("Product"),
            a("ID"): property(id), a("Name"): property(name), a("CategoryID"): property(category),
            a("Group"): {"type": "Relationship", "object": format!("urn:ngsi-ld:Category:{category}")}});
        let notes: Vec<String> = notes
            .iter()
            .map(|n| format!("urn:ngsi-ld:Note:{n}"))
            .collect();
        let related: Vec<String> = related
            .iter()
            .map(|p| format!("urn:ngsi-ld:Product:{p}"))
            .collect();
        if let Some(o) = e.as_object_mut() {
            if !notes.is_empty() {
                o.insert(
                    a("Notes"),
                    rel(&notes.iter().map(String::as_str).collect::<Vec<_>>()),
                );
            }
            if !related.is_empty() {
                o.insert(
                    a("Related"),
                    rel(&related.iter().map(String::as_str).collect::<Vec<_>>()),
                );
            }
        }
        entities.push(e);
    }
    for entity in entities {
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some(&entity.to_string()),
            None,
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{:?}: {} {}", entity.get("id"), a.status, a.body));
        }
    }
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn any_over_a_navigation_matches_one_target() -> Result<(), String> {
    let (_b, app) = shop().await?;
    for (filter, want) in [
        ("Notes/any(d:d/Lang%20eq%20'EN')", &["Drill"][..]),
        (
            "Notes/any(d:d/Lang%20eq%20'FR'%20or%20d/Lang%20eq%20'DE')",
            &["Drill", "Saw"],
        ),
        ("Notes/any(d:contains(d/Lang,'R'))", &["Saw"]),
        ("Notes/any()", &["Drill", "Saw"]),
        ("Related/any(r:r/Notes/any(d:d/Lang%20eq%20'EN'))", &["Saw"]),
        ("Related/any(r:r/Notes/any())", &["Nail", "Saw"]),
        (
            "Notes/any(d:d/Lang%20eq%20'EN')%20and%20Name%20ne%20'Saw'",
            &["Drill"],
        ),
    ] {
        let a = get(&app, &format!("Product?$filter={filter}&$orderby=Name")).await;
        assert_eq!(
            (a.status, names(&a)),
            (
                StatusCode::OK,
                want.iter().map(|s| (*s).to_owned()).collect()
            ),
            "{filter}: {}",
            a.body
        );
    }
    // `and` inside the lambda and `all` have no q form; a member the view hides is no member
    for (filter, status) in [
        (
            "Notes/any(d:d/Lang%20eq%20'EN'%20and%20d/Lang%20eq%20'DE')",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Notes/all(d:d/Lang%20ne%20'XX')",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "Notes/any(d:d/Secret%20eq%20'hush')",
            StatusCode::BAD_REQUEST,
        ),
        ("Notes/any(d:d/Lang%20eq%201)", StatusCode::BAD_REQUEST),
    ] {
        let a = get(&app, &format!("Product?$filter={filter}")).await;
        assert_eq!(a.status, status, "{filter}: {}", a.body);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn orderby_a_member_of_a_navigation_target_and_the_key_under_select() -> Result<(), String> {
    let (_b, app) = shop().await?;
    let a = get(&app, "Product?$orderby=Group/Name%20desc,Name&$select=Name").await;
    assert_eq!(names(&a), ["Drill", "Nail", "Saw"], "{}", a.body);
    let a = get(
        &app,
        "Product?$orderby=Group/Name&$filter=Name%20ne%20'Nail'&$select=Name",
    )
    .await;
    assert_eq!(names(&a), ["Saw", "Drill"], "{}", a.body);
    // Protocol 11.2.5.1: the key, not the NGSI-LD id, beside what $select names
    let a = get(&app, "Product?$select=Name&$filter=Name%20eq%20'Saw'").await;
    assert_eq!(
        a.body["value"],
        json!([{"ID": "saw", "Name": "Saw"}]),
        "{}",
        a.body
    );
    for refused in ["Product?$orderby=Notes/Lang", "Product?$orderby=Group/Nope"] {
        assert_eq!(
            get(&app, refused).await.status,
            StatusCode::BAD_REQUEST,
            "{refused}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bind_sets_the_constrained_property_and_patch_honours_return() -> Result<(), String> {
    let (_b, app) = shop().await?;
    let body = json!({"ID": "rake", "Name": "Rake", "Group@odata.bind": "Category('garden')"});
    let a = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Product"),
        Some(&body.to_string()),
        None,
    )
    .await;
    assert_eq!(
        (a.status, &a.body["CategoryID"]),
        (StatusCode::CREATED, &json!("garden")),
        "{}",
        a.body
    );
    let a = send(
        &app,
        Method::POST,
        &format!("{ROOT}/Product"),
        Some(&json!({"ID": "hoe", "Name": "Hoe"}).to_string()),
        Some("return=minimal"),
    )
    .await;
    assert_eq!(
        (a.status, a.applied.as_str()),
        (StatusCode::NO_CONTENT, "return=minimal")
    );
    let rake = format!("{ROOT}/Product('rake')");
    let a = send(
        &app,
        Method::PATCH,
        &rake,
        Some(r#"{"Price": 7.5}"#),
        Some("odata.continue-on-error, return=representation"),
    )
    .await;
    assert_eq!(
        (a.status, a.applied.as_str(), &a.body["Price"]),
        (StatusCode::OK, "return=representation", &json!(7.5)),
        "{}",
        a.body
    );
    let a = send(
        &app,
        Method::PATCH,
        &rake,
        Some(r#"{"Price": 8}"#),
        Some("return=minimal"),
    )
    .await;
    assert_eq!(
        (a.status, a.applied.as_str()),
        (StatusCode::NO_CONTENT, "return=minimal")
    );
    let a = send(&app, Method::PATCH, &rake, Some(r#"{"Price": 9}"#), None).await;
    assert_eq!((a.status, a.applied.as_str()), (StatusCode::NO_CONTENT, ""));
    // a bind to a Category that does not exist writes nothing
    let a = send(
        &app,
        Method::PATCH,
        &rake,
        Some(r#"{"Group@odata.bind": "Category('none')"}"#),
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
    assert_eq!(
        get(&app, "Product('rake')").await.body["CategoryID"],
        json!("garden")
    );
    Ok(())
}
