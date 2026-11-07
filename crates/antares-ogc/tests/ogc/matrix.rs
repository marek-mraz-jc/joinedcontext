// SPDX-License-Identifier: EUPL-1.2
//! The OGC API - Features matrix (T-3831): every Part on every kind of Endpoint the platform
//! puts in front of this surface, one view per kind.
//!
//! | column | view | what it is |
//! |---|---|---|
//! | public | `city` | a fixed type list, reads only: what an anonymous caller of a public Endpoint gets |
//! | internal | `fleet` | the same types with the Part 4 writes: a signed-in caller of an internal Endpoint |
//! | filtered | `slow` | narrowed by the view's `q` (`speed<50`) |
//! | star | `star` | ADR-0023 rule 12: every class and attribute of the schema |
//! | projection | `proj` | Car with `speed` and `location` only |
//! | cube | `cube` | the observations of a data cube (Tenant `stats`): no geometry, dimensions and a measure |
//!
//! A cell holds, or (Part 4 on a view without writes) asserts the 405 with `Allow` that stands
//! for it; a red cell fails the test with the whole matrix in the message. Security in every
//! column: no feature, queryable or schema shows an attribute the view hides, and a filter on
//! one is the 400 of an unknown name.

use crate::common::{fixture, get, ids, send, Fixture};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fmt::Write as _;

const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
const EPSG: &str = "http://www.opengis.net/def/crs/EPSG/0/4326";
const PARTS: [&str; 5] = [
    "1 core, geojson, oas30",
    "2 crs",
    "3 queryables, filter, cql2",
    "4 create, replace, update, delete",
    "5 schemas, sortables, sortby",
];

const STATS_SCHEMA: &str = r#"{
  "id": "https://example.org/stats", "name": "stats",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "dc",
  "classes": {"Reading": {"slots": ["district", "month", "pm25", "note"], "description": "One observation of the cube"}},
  "slots": {
    "district": {"range": "string"},
    "month": {"range": "string"},
    "pm25": {"range": "float", "unit": {"ucum_code": "ug/m3"}},
    "note": {"range": "string"}
  }
}"#;

/// One column: its view, the collection read, the features it must answer, the attributes it
/// shows and hides, and a filter on a shown attribute with the features it selects.
struct Column {
    name: &'static str,
    view: &'static str,
    collection: &'static str,
    features: &'static [&'static str],
    shown: &'static [&'static str],
    hidden: Option<&'static str>,
    filter: (&'static str, &'static [&'static str]),
    sortby: &'static str,
    writes: bool,
}

const CAR: &[&str] = &["speed", "location", "seen", "at", "home", "colour", "tags"];
const ALL_CARS: &[&str] = &[
    "urn:ngsi-ld:Car:1",
    "urn:ngsi-ld:Car:2",
    "urn:ngsi-ld:Car:3",
    "urn:ngsi-ld:Car:4",
];
const FAST: &[&str] = &["urn:ngsi-ld:Car:2", "urn:ngsi-ld:Car:4"];

const COLUMNS: [Column; 6] = [
    Column {
        name: "public",
        view: "city",
        collection: "Car",
        features: ALL_CARS,
        shown: CAR,
        hidden: Some("secret"),
        filter: ("speed > 50", FAST),
        sortby: "speed",
        writes: false,
    },
    Column {
        name: "internal",
        view: "fleet",
        collection: "Car",
        features: ALL_CARS,
        shown: CAR,
        hidden: Some("secret"),
        filter: ("speed > 50", FAST),
        sortby: "speed",
        writes: true,
    },
    Column {
        name: "filtered",
        view: "slow",
        collection: "Car",
        features: &["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        shown: CAR,
        hidden: Some("secret"),
        filter: ("speed > 20", &["urn:ngsi-ld:Car:3"]),
        sortby: "speed",
        writes: false,
    },
    Column {
        name: "star",
        view: "star",
        collection: "Car",
        features: ALL_CARS,
        shown: &[
            "speed", "location", "seen", "secret", "at", "home", "colour", "tags",
        ],
        hidden: None,
        filter: ("speed > 50", FAST),
        sortby: "speed",
        writes: false,
    },
    Column {
        name: "projection",
        view: "proj",
        collection: "Car",
        features: ALL_CARS,
        shown: &["speed", "location"],
        hidden: Some("colour"),
        filter: ("speed > 50", FAST),
        sortby: "speed",
        writes: false,
    },
    Column {
        name: "cube",
        view: "cube",
        collection: "Reading",
        features: &[
            "urn:ngsi-ld:Reading:1",
            "urn:ngsi-ld:Reading:2",
            "urn:ngsi-ld:Reading:3",
        ],
        shown: &["district", "month", "pm25"],
        hidden: Some("note"),
        filter: (
            "pm25 > 20",
            &["urn:ngsi-ld:Reading:2", "urn:ngsi-ld:Reading:3"],
        ),
        sortby: "pm25",
        writes: false,
    },
];

/// The fixture with the three views and the Tenant the other columns need.
async fn matrix_fixture() -> Result<Fixture, String> {
    let f = fixture().await?;
    let reads = json!(["query_entities"]);
    let views = [
        json!({"id": "star", "tenant": "city", "selector": ["*"],
               "rules": [{"types": ["*"], "attributes": ["*"]}], "operations": reads}),
        json!({"id": "proj", "tenant": "city", "selector": ["Car"],
               "rules": [{"types": ["Car"], "attributes": ["speed", "location"]}], "operations": reads}),
        json!({"id": "cube", "tenant": "stats", "selector": ["Reading"],
               "rules": [{"types": ["Reading"], "attributes": ["district", "month", "pm25"]}], "operations": reads}),
    ];
    for view in views {
        let id = view.get("id").and_then(Value::as_str).unwrap_or_default();
        let a = send(
            &f.app,
            "PUT",
            &format!("/q/views/{id}"),
            &view.to_string(),
            &[],
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("view {id}: {} {}", a.status, a.body));
        }
    }
    let a = send(&f.app, "PUT", "/q/schemas/stats", STATS_SCHEMA, &[]).await;
    if a.status != StatusCode::CREATED {
        return Err(format!("schema stats: {} {}", a.status, a.body));
    }
    for (n, district, value) in [
        (1, "Kallio", 12.5),
        (2, "Kallio", 31.0),
        (3, "Vallila", 22.0),
    ] {
        let reading = json!({"id": format!("urn:ngsi-ld:Reading:{n}"), "type": "Reading",
            "district": {"type": "Property", "value": district},
            "month": {"type": "Property", "value": "2026-09"},
            "pm25": {"type": "Property", "value": value},
            "note": {"type": "Property", "value": "internal"}});
        let a = send(
            &f.app,
            "POST",
            "/ngsi-ld/v1/entities",
            &reading.to_string(),
            &[("NGSILD-Tenant", "stats")],
        )
        .await;
        if a.status != StatusCode::CREATED {
            return Err(format!("{reading}: {} {}", a.status, a.body));
        }
    }
    Ok(f)
}

fn enc(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn keys(v: Option<&Value>) -> BTreeSet<String> {
    v.and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn header<'a>(a: &'a crate::common::Answer, name: &str) -> &'a str {
    a.headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn ensure(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(what())
    }
}

/// The attributes a feature collection's features carry, each checked against the column.
fn check_properties(c: &Column, body: &Value) -> Result<(), String> {
    for feature in body["features"].as_array().into_iter().flatten() {
        for name in keys(feature.get("properties")) {
            ensure(c.shown.contains(&name.as_str()), || {
                format!(
                    "{}: property {name} is not shown by the view",
                    feature["id"]
                )
            })?;
        }
    }
    Ok(())
}

async fn part1(f: &Fixture, c: &Column) -> Result<(), String> {
    let root = format!("/x/views/{}/ogc/features", c.view);
    let first = c.features.first().copied().unwrap_or_default();
    let landing = get(&f.app, &root).await;
    ensure(landing.status == StatusCode::OK, || {
        format!("landing {}", landing.status)
    })?;
    let conformance = get(&f.app, &format!("{root}/conformance")).await.body;
    let classes = conformance
        .get("conformsTo")
        .map(Value::to_string)
        .unwrap_or_default();
    for class in [
        "features-1/1.0/conf/core",
        "features-1/1.0/conf/geojson",
        "features-1/1.0/conf/oas30",
    ] {
        ensure(classes.contains(class), || {
            format!("conformance lacks {class}")
        })?;
    }
    let api = get(&f.app, &format!("{root}/api")).await;
    ensure(
        api.status == StatusCode::OK && api.body.get("openapi").is_some(),
        || format!("api {}", api.status),
    )?;
    let collections = get(&f.app, &format!("{root}/collections")).await.body;
    let named: Vec<&str> = collections
        .get("collections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x["id"].as_str())
        .collect();
    ensure(named.contains(&c.collection), || {
        format!("collections {named:?} lack {}", c.collection)
    })?;
    let items = get(
        &f.app,
        &format!("{root}/collections/{}/items?limit=100", c.collection),
    )
    .await;
    ensure(items.status == StatusCode::OK, || {
        format!("items {}", items.status)
    })?;
    ensure(header(&items, "content-type").contains("geo+json"), || {
        format!("items content-type {}", header(&items, "content-type"))
    })?;
    ensure(sorted(ids(&items.body)) == c.features, || {
        format!("items {:?}, expected {:?}", ids(&items.body), c.features)
    })?;
    check_properties(c, &items.body)?;
    let one = get(
        &f.app,
        &format!("{root}/collections/{}/items/{}", c.collection, first),
    )
    .await;
    ensure(
        one.status == StatusCode::OK && one.body.get("id").and_then(Value::as_str) == Some(first),
        || format!("item {}: {}", first, one.status),
    )?;
    if c.view == "slow" {
        let outside = get(
            &f.app,
            &format!("{root}/collections/Car/items/urn:ngsi-ld:Car:2"),
        )
        .await;
        ensure(outside.status == StatusCode::NOT_FOUND, || {
            format!("a feature outside the view's q answers {}", outside.status)
        })?;
    }
    let absent = get(
        &f.app,
        &format!("{root}/collections/{}/items/urn:ngsi-ld:X:0", c.collection),
    )
    .await;
    ensure(absent.status == StatusCode::NOT_FOUND, || {
        format!("absent {}", absent.status)
    })
}

async fn part2(f: &Fixture, c: &Column) -> Result<(), String> {
    let items = format!(
        "/x/views/{}/ogc/features/collections/{}/items",
        c.view, c.collection
    );
    let crs84 = get(&f.app, &format!("{items}?crs={}", enc(CRS84))).await;
    ensure(crs84.status == StatusCode::OK, || {
        format!("crs=CRS84 {}", crs84.status)
    })?;
    ensure(
        header(&crs84, "content-crs") == format!("<{CRS84}>"),
        || format!("Content-Crs {:?}", header(&crs84, "content-crs")),
    )?;
    let epsg = get(&f.app, &format!("{items}?crs={}", enc(EPSG))).await;
    ensure(epsg.status == StatusCode::BAD_REQUEST, || {
        format!("crs=EPSG:4326 {}", epsg.status)
    })
}

async fn part3(f: &Fixture, c: &Column) -> Result<(), String> {
    let collection = format!(
        "/x/views/{}/ogc/features/collections/{}",
        c.view, c.collection
    );
    let q = get(&f.app, &format!("{collection}/queryables")).await;
    ensure(q.status == StatusCode::OK, || {
        format!("queryables {}", q.status)
    })?;
    let names = keys(q.body.get("properties"));
    for shown in c.shown {
        // A geometry is queryable as `geometry`, not by its NGSI-LD name.
        let name = if *shown == "location" {
            "geometry"
        } else {
            shown
        };
        ensure(names.contains(name), || {
            format!("queryables {names:?} lack {name}")
        })?;
    }
    let (filter, expected) = c.filter;
    let read = get(
        &f.app,
        &format!("{collection}/items?limit=100&filter={}", enc(filter)),
    )
    .await;
    ensure(read.status == StatusCode::OK, || {
        format!("filter {filter}: {} {}", read.status, read.body)
    })?;
    ensure(sorted(ids(&read.body)) == expected, || {
        format!(
            "filter {filter}: {:?}, expected {expected:?}",
            ids(&read.body)
        )
    })?;
    let Some(hidden) = c.hidden else {
        return Ok(());
    };
    ensure(!names.contains(hidden), || {
        format!("queryables show hidden {hidden}")
    })?;
    let leak = get(
        &f.app,
        &format!(
            "{collection}/items?filter={}",
            enc(&format!("{hidden} IS NOT NULL"))
        ),
    )
    .await;
    let unknown = get(
        &f.app,
        &format!(
            "{collection}/items?filter={}",
            enc("nosuchname IS NOT NULL")
        ),
    )
    .await;
    ensure(
        leak.status == StatusCode::BAD_REQUEST && leak.status == unknown.status,
        || format!("filter on hidden {hidden}: {} {}", leak.status, leak.body),
    )
}

async fn part4(f: &Fixture, c: &Column) -> Result<(), String> {
    let items = format!(
        "/x/views/{}/ogc/features/collections/{}/items",
        c.view, c.collection
    );
    let body = json!({"type": "Feature", "id": "urn:ngsi-ld:Car:matrix",
        "geometry": {"type": "Point", "coordinates": [17.0, 48.0]}, "properties": {"speed": 1}})
    .to_string();
    let geo = [("Content-Type", "application/geo+json")];
    let created = send(&f.app, "POST", &items, &body, &geo).await;
    if !c.writes {
        // A view that offers no write answers each write 405 with what it offers (W4).
        let allow = header(&created, "allow").to_owned();
        return ensure(
            created.status == StatusCode::METHOD_NOT_ALLOWED
                && allow.contains("GET")
                && !allow.contains("POST"),
            || {
                format!(
                    "POST on a read-only view: {} Allow {allow:?}",
                    created.status
                )
            },
        );
    }
    ensure(created.status == StatusCode::CREATED, || {
        format!("POST {} {}", created.status, created.body)
    })?;
    // T-5909: each write is read back as the NGSI-LD Entity the Tenant stores.
    stored(f, 1, [17.0, 48.0]).await?;
    let one = format!("{items}/urn:ngsi-ld:Car:matrix");
    let patch = send(
        &f.app,
        "PATCH",
        &one,
        r#"{"properties": {"speed": 2}}"#,
        &[("Content-Type", "application/merge-patch+json")],
    )
    .await;
    ensure(patch.status == StatusCode::NO_CONTENT, || {
        format!("PATCH {}", patch.status)
    })?;
    stored(f, 2, [17.0, 48.0]).await?;
    let replaced = json!({"type": "Feature", "id": "urn:ngsi-ld:Car:matrix",
        "geometry": {"type": "Point", "coordinates": [19.5, 49.25]}, "properties": {"speed": 3}})
    .to_string();
    let put = send(&f.app, "PUT", &one, &replaced, &geo).await;
    ensure(put.status == StatusCode::NO_CONTENT, || {
        format!("PUT {} {}", put.status, put.body)
    })?;
    stored(f, 3, [19.5, 49.25]).await?;
    let delete = send(&f.app, "DELETE", &one, "", &[]).await;
    ensure(delete.status == StatusCode::NO_CONTENT, || {
        format!("DELETE {}", delete.status)
    })?;
    let gone = get(&f.app, &one).await;
    ensure(gone.status == StatusCode::NOT_FOUND, || {
        format!("after DELETE {}", gone.status)
    })?;
    let entity = ngsi_ld(f).await;
    ensure(entity.status == StatusCode::NOT_FOUND, || {
        format!("NGSI-LD after DELETE {} {}", entity.status, entity.body)
    })
}

/// The Feature the Part 4 cell wrote, read through NGSI-LD in the Tenant.
async fn ngsi_ld(f: &Fixture) -> crate::common::Answer {
    let at = "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:matrix";
    send(&f.app, "GET", at, "", &[("NGSILD-Tenant", "city")]).await
}

/// The stored Entity holds `speed` and a `location` Point at `at`, and nothing the write
/// did not name.
async fn stored(f: &Fixture, speed: i64, at: [f64; 2]) -> Result<(), String> {
    let a = ngsi_ld(f).await;
    let b = &a.body;
    let point = json!({"type": "Point", "coordinates": at});
    ensure(
        a.status == StatusCode::OK
            && b.pointer("/type") == Some(&json!("Car"))
            && b.pointer("/speed/type") == Some(&json!("Property"))
            && b.pointer("/speed/value") == Some(&json!(speed))
            && b.pointer("/speed/unitCode").is_none()
            && b.pointer("/location/type") == Some(&json!("GeoProperty"))
            && b.pointer("/location/value") == Some(&point),
        || format!("NGSI-LD read back (speed {speed}): {} {b}", a.status),
    )
}

async fn part5(f: &Fixture, c: &Column) -> Result<(), String> {
    let collection = format!(
        "/x/views/{}/ogc/features/collections/{}",
        c.view, c.collection
    );
    let schema = get(&f.app, &format!("{collection}/schema")).await;
    ensure(schema.status == StatusCode::OK, || {
        format!("schema {}", schema.status)
    })?;
    let properties = keys(schema.body.get("properties"));
    if let Some(hidden) = c.hidden {
        ensure(!properties.contains(hidden), || {
            format!("schema shows hidden {hidden}")
        })?;
    }
    let sortables = get(&f.app, &format!("{collection}/sortables")).await;
    ensure(
        sortables.status == StatusCode::OK
            && keys(sortables.body.get("properties")).contains(c.sortby),
        || format!("sortables {} lack {}", sortables.status, c.sortby),
    )?;
    let down = get(
        &f.app,
        &format!("{collection}/items?limit=100&sortby=-{}", c.sortby),
    )
    .await;
    let values: Vec<f64> = down
        .body
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.get("properties")?.get(c.sortby)?.as_f64())
        .collect();
    ensure(
        down.status == StatusCode::OK
            && values.len() == c.features.len()
            && values.windows(2).all(|w| w.first() >= w.last()),
        || format!("sortby=-{}: {} {values:?}", c.sortby, down.status),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn every_part_on_every_endpoint_kind() -> Result<(), String> {
    let f = matrix_fixture().await?;
    let mut table = format!(
        "| part | {} |\n|---{}|\n",
        COLUMNS.map(|c| c.name).join(" | "),
        "|---".repeat(COLUMNS.len())
    );
    let mut red = Vec::new();
    for (row, part) in PARTS.iter().enumerate() {
        let _ = write!(table, "| {part} |");
        for c in &COLUMNS {
            let cell = match row {
                0 => part1(&f, c).await,
                1 => part2(&f, c).await,
                2 => part3(&f, c).await,
                3 => part4(&f, c).await,
                _ => part5(&f, c).await,
            };
            let text = match (&cell, row, c.writes) {
                (Ok(()), 3, false) => "405 + Allow".to_owned(),
                (Ok(()), 2, _) if c.hidden.is_none() => "green (hides nothing)".to_owned(),
                (Ok(()), _, _) => "green".to_owned(),
                (Err(why), _, _) => {
                    red.push(format!("{part} on {}: {why}", c.name));
                    "RED".to_owned()
                }
            };
            let _ = write!(table, " {text} |");
        }
        table.push('\n');
    }
    ensure(red.is_empty(), || format!("{table}\n{}", red.join("\n")))
}
