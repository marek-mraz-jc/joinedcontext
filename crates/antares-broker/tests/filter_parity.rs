// SPDX-License-Identifier: EUPL-1.2
//! One filter, every standard (T-4464): each case of the table is written once per standard,
//! `NGSI-LD` `q`/`georel` on the view root, `OData` `$filter`, OGC API Features `filter` (CQL2 text)
//! with `bbox` and `datetime`, and the MCP `query_entities` tool, over one view of 48 Vehicles,
//! and every surface must select the same sorted ids as the case's own predicate over the
//! fixture. A form a standard cannot write is `None` in the table and listed by `unexpressed`,
//! never skipped silently.
//!
//! The surface entry points the cases reach, the ones that turn a filter into `q` (`cg q -e
//! 'CALLERS lower;'`): `antares_odata::filter` (`$filter`), `antares_ogc::filter` (CQL2, bbox,
//! datetime), `antares_mcp::tools::build` (`query_entities` arguments) and the NGSI-LD root
//! itself. With the `parquet` feature the OGC form is asked a second time as a Parquet file
//! (`f=parquet`, T-4465) and its `id` column must hold the same ids; with `tabular` it is asked as a
//! CSV (`f=csv`, T-4477) too. A surface added without a column here is a review finding.
//!
//! It lives in the broker's tests because a surface crate may dev-depend on no other surface
//! (module rule surface-dev); it runs with the `ogc`, `odata` and `mcp` features (`full`).
//!
//! Beside the table: a filter on the attribute the view hides is the unknown name it is on every
//! surface (no row passes through it), and a malformed filter is a 400, never a 500.

#![cfg(all(feature = "ogc", feature = "odata", feature = "mcp"))]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::fmt::Write as _;
use tower::ServiceExt;

const VIEW: &str = "fleet";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"},
               "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
  "default_prefix": "dc",
  "classes": {"Vehicle": {"slots": ["vehicleType", "speed", "fuelType", "name", "dateObserved", "location", "secret"]}},
  "slots": {
    "vehicleType": {"range": "string"},
    "speed": {"range": "float"},
    "fuelType": {"range": "string"},
    "name": {"range": "string"},
    "dateObserved": {"range": "datetime", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-instant"}}},
    "location": {"slot_uri": "ngsi:location", "range": "string",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
    "secret": {"range": "string"}
  }
}"#;

/// One Vehicle of the fixture, as the cases' own predicates read it.
struct Vehicle {
    id: String,
    kind: &'static str,
    speed: f64,
    fuel: Option<&'static str>,
    name: String,
    observed: &'static str,
    inside: bool,
}

/// 48 Vehicles: four kinds, six speeds, a fuel or none, a Slovak name on the trams, a day in
/// October, two in three inside the area [17.0, 48.0, 17.2, 48.2].
fn fleet() -> Vec<Vehicle> {
    let kinds = ["bus", "tram", "car", "bicycle"].into_iter().cycle();
    let speeds = [12.0, 45.0, 30.0, 78.0, 5.0, 60.0].into_iter().cycle();
    let fuels = [
        Some("diesel"),
        Some("electric"),
        Some("hybrid"),
        None,
        Some("electric"),
    ]
    .into_iter()
    .cycle();
    let days = [
        "2026-10-01T08:00:00Z",
        "2026-10-04T08:00:00Z",
        "2026-10-07T08:00:00Z",
        "2026-10-12T08:00:00Z",
        "2026-10-15T08:00:00Z",
    ]
    .into_iter()
    .cycle();
    let inside = [true, true, false].into_iter().cycle();
    (1..=48u32)
        .zip(kinds)
        .zip(speeds)
        .zip(fuels)
        .zip(days)
        .zip(inside)
        .map(|(((((n, kind), speed), fuel), observed), inside)| Vehicle {
            id: format!("urn:ngsi-ld:Vehicle:{n:02}"),
            kind,
            speed,
            fuel,
            name: if kind == "tram" {
                format!("Električka Žilina {n}")
            } else {
                format!("Vozidlo {n}")
            },
            observed,
            inside,
        })
        .collect()
}

fn entity(v: &Vehicle) -> Value {
    let at = if v.inside { [17.1, 48.1] } else { [18.7, 49.2] };
    let mut e = json!({
        "id": v.id, "type": "Vehicle",
        "vehicleType": {"type": "Property", "value": v.kind},
        "speed": {"type": "Property", "value": v.speed},
        "name": {"type": "Property", "value": v.name},
        "dateObserved": {"type": "Property", "value": {"@type": "DateTime", "@value": v.observed}},
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": at}},
        "secret": {"type": "Property", "value": "s"},
    });
    if let (Some(fuel), Some(map)) = (v.fuel, e.as_object_mut()) {
        map.insert(
            "fuelType".into(),
            json!({"type": "Property", "value": fuel}),
        );
    }
    e
}

/// The area every other Vehicle stands in, as each standard writes it.
const AREA_GEOJSON: &str = "[[[17.0,48.0],[17.2,48.0],[17.2,48.2],[17.0,48.2],[17.0,48.0]]]";
const AREA_WKT: &str = "POLYGON((17.0 48.0,17.2 48.0,17.2 48.2,17.0 48.2,17.0 48.0))";

/// One case of the table: its forms per standard (`None`: the standard cannot write it) and
/// the predicate that decides it over the fixture.
struct Case {
    id: &'static str,
    ngsi: (&'static str, Option<&'static str>),
    odata: Option<String>,
    ogc: Option<String>,
    want: fn(&Vehicle) -> bool,
}

/// The cases on one attribute's value and their combinations.
fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "equality",
            ngsi: ("vehicleType==\"bus\"", None),
            odata: Some("vehicleType eq 'bus'".into()),
            ogc: Some("filter=vehicleType = 'bus'".into()),
            want: |v| v.kind == "bus",
        },
        Case {
            id: "range",
            ngsi: ("speed>=30;speed<60", None),
            odata: Some("speed ge 30 and speed lt 60".into()),
            ogc: Some("filter=speed >= 30 AND speed < 60".into()),
            want: |v| v.speed >= 30.0 && v.speed < 60.0,
        },
        Case {
            id: "or",
            ngsi: ("vehicleType==\"bus\"|vehicleType==\"tram\"", None),
            odata: Some("vehicleType eq 'bus' or vehicleType eq 'tram'".into()),
            ogc: Some("filter=vehicleType = 'bus' OR vehicleType = 'tram'".into()),
            want: |v| v.kind == "bus" || v.kind == "tram",
        },
        Case {
            id: "and-or",
            ngsi: ("(vehicleType==\"bus\"|vehicleType==\"car\");speed>20", None),
            odata: Some("(vehicleType eq 'bus' or vehicleType eq 'car') and speed gt 20".into()),
            ogc: Some("filter=(vehicleType = 'bus' OR vehicleType = 'car') AND speed > 20".into()),
            want: |v| (v.kind == "bus" || v.kind == "car") && v.speed > 20.0,
        },
        Case {
            id: "not",
            ngsi: ("vehicleType!=\"car\"", None),
            odata: Some("not (vehicleType eq 'car')".into()),
            ogc: Some("filter=NOT (vehicleType = 'car')".into()),
            want: |v| v.kind != "car",
        },
    ]
}

/// The cases on membership, presence, text, place and time.
fn more_cases() -> Vec<Case> {
    let within = Some("within");
    vec![
        Case {
            id: "in",
            ngsi: ("vehicleType==\"bus\",\"tram\"", None),
            odata: Some("vehicleType in ('bus','tram')".into()),
            ogc: Some("filter=vehicleType IN ('bus','tram')".into()),
            want: |v| v.kind == "bus" || v.kind == "tram",
        },
        Case {
            id: "missing",
            ngsi: ("!fuelType", None),
            odata: Some("fuelType eq null".into()),
            ogc: Some("filter=fuelType IS NULL".into()),
            want: |v| v.fuel.is_none(),
        },
        Case {
            id: "present-and-equal",
            ngsi: ("fuelType==\"electric\"", None),
            odata: Some("fuelType eq 'electric'".into()),
            ogc: Some("filter=fuelType = 'electric'".into()),
            want: |v| v.fuel == Some("electric"),
        },
        Case {
            id: "non-ascii",
            ngsi: ("name==\"Električka Žilina 2\"", None),
            odata: Some("name eq 'Električka Žilina 2'".into()),
            ogc: Some("filter=name = 'Električka Žilina 2'".into()),
            want: |v| v.name == "Električka Žilina 2",
        },
        Case {
            id: "bbox",
            ngsi: ("", within),
            odata: Some(format!(
                "geo.intersects(location,geography'SRID=4326;{}')",
                AREA_WKT.replace("POLYGON", "Polygon")
            )),
            ogc: Some("bbox=17.0,48.0,17.2,48.2".into()),
            want: |v| v.inside,
        },
        Case {
            id: "area-and-attribute",
            ngsi: ("vehicleType==\"tram\"", within),
            odata: Some(format!(
                "vehicleType eq 'tram' and geo.intersects(location,geography'SRID=4326;{}')",
                AREA_WKT.replace("POLYGON", "Polygon")
            )),
            ogc: Some(format!(
                "filter=vehicleType = 'tram' AND S_INTERSECTS(geometry,{AREA_WKT})"
            )),
            want: |v| v.kind == "tram" && v.inside,
        },
        Case {
            id: "datetime-window",
            ngsi: (
                "dateObserved>=2026-10-04T00:00:00Z;dateObserved<2026-10-13T00:00:00Z",
                None,
            ),
            odata: Some(
                "dateObserved ge 2026-10-04T00:00:00Z and dateObserved lt 2026-10-13T00:00:00Z"
                    .into(),
            ),
            ogc: Some("datetime=2026-10-04T00:00:00Z/2026-10-12T23:59:59Z".into()),
            want: |v| v.observed >= "2026-10-04" && v.observed < "2026-10-13",
        },
    ]
}

fn enc(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// `key=value&...` with every value encoded; a `k=v` pair keeps its key.
fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost");
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp.into_body();
    let bytes = axum::body::to_bytes(bytes, usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids
}

fn ids_at(body: &Value, list: &str) -> Vec<String> {
    sorted(
        body.pointer(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned)),
    )
}

/// The broker over the built-in store with the four surfaces the table asks mounted, as
/// `surfaces_equivalence` builds it; the view table's reload handle lives as long as the router.
async fn broker() -> Result<(antares_ops::views::Reload, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let mut state = AppState::new("filter-parity".into());
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        antares_ops::mount(antares_odata::OData),
        antares_ops::mount(antares_ogc::Ogc),
        antares_ops::mount(antares_mcp::Mcp),
    ];
    let state = state
        .with_surfaces(surfaces)
        .map_err(|e| format!("the surfaces mount: {e:?}"))?;
    let broker = antares_ops::views::install(&state)
        .await
        .map_err(|e| format!("the view table: {e:?}"))?;
    let app = antares_api::router(state);
    let view = json!({
        "id": VIEW, "tenant": "city", "selector": ["Vehicle"],
        "rules": [{"types": ["Vehicle"], "attributes": ["vehicleType", "speed", "fuelType", "name", "dateObserved", "location"]}],
        "operations": ["query_entities", "retrieve_entity"]
    });
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/q/views/{VIEW}"),
        &view.to_string(),
        &[],
    )
    .await;
    if status != StatusCode::CREATED {
        return Err(format!("view: {status} {body}"));
    }
    let (status, body) = send(&app, "PUT", "/q/schemas/city", SCHEMA, &[]).await;
    if status != StatusCode::CREATED {
        return Err(format!("schema: {status} {body}"));
    }
    for v in fleet() {
        let (status, body) = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            &entity(&v).to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        if status != StatusCode::CREATED {
            return Err(format!("{}: {status} {body}", v.id));
        }
    }
    Ok((broker, app))
}

async fn ngsi(app: &Router, q: &str, georel: Option<&str>) -> Result<Vec<String>, String> {
    let mut pairs = vec![("type", "Vehicle"), ("limit", "1000")];
    if !q.is_empty() {
        pairs.push(("q", q));
    }
    if let Some(rel) = georel {
        pairs.extend([
            ("georel", rel),
            ("geometry", "Polygon"),
            ("coordinates", AREA_GEOJSON),
        ]);
    }
    let uri = format!("/x/views/{VIEW}/ngsi-ld/v1/entities?{}", query(&pairs));
    let (status, body) = send(app, "GET", &uri, "", &[]).await;
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {body}"));
    }
    Ok(ids_at(&body, ""))
}

async fn odata(app: &Router, filter: &str) -> Result<Vec<String>, String> {
    // OData's ABNF takes the `=` of a geography literal's `SRID=4326` only as written (EQ = "="),
    // so a client encodes no more than a URL needs; odata-url refuses `%3D` there, rightly.
    let light: String = filter
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_owned(),
            c if c.is_ascii() => c.to_string(),
            c => enc(&c.to_string()),
        })
        .collect();
    let uri = format!("/x/views/{VIEW}/odata/v4/Vehicle?$filter={light}&$top=1000");
    let (status, body) = send(app, "GET", &uri, "", &[]).await;
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {body}"));
    }
    if body.get("@odata.nextLink").is_some() {
        return Err(format!("{uri}: a second page"));
    }
    Ok(ids_at(&body, "/value"))
}

async fn ogc(app: &Router, form: &str) -> Result<Vec<String>, String> {
    let (key, value) = form.split_once('=').ok_or("a key=value form")?;
    let uri = format!(
        "/x/views/{VIEW}/ogc/features/collections/Vehicle/items?{}",
        query(&[(key, value), ("limit", "1000")])
    );
    let (status, body) = send(app, "GET", &uri, "", &[]).await;
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {body}"));
    }
    Ok(ids_at(&body, "/features"))
}

/// The OGC form asked as a Parquet file: the ids of its `id` column.
#[cfg(feature = "parquet")]
async fn ogc_parquet(app: &Router, form: &str) -> Result<Vec<String>, String> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let (key, value) = form.split_once('=').ok_or("a key=value form")?;
    let uri = format!(
        "/x/views/{VIEW}/ogc/features/collections/Vehicle/items?{}",
        query(&[(key, value), ("f", "parquet")])
    );
    let req = Request::builder().uri(&uri).header("Host", "localhost");
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .map_err(|e| e.to_string())?;
    if status != StatusCode::OK {
        return Err(format!(
            "{uri}: {status} {}",
            String::from_utf8_lossy(&bytes)
        ));
    }
    let file = SerializedFileReader::new(bytes).map_err(|e| format!("{uri}: {e}"))?;
    let rows = file.get_row_iter(None).map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    for row in rows {
        let row = row.map_err(|e| e.to_string())?;
        match row.get_column_iter().find(|(name, _)| *name == "id") {
            Some((_, parquet::record::Field::Str(id))) => ids.push(id.clone()),
            other => return Err(format!("{uri}: a row without an id: {other:?}")),
        }
    }
    Ok(sorted(ids))
}

/// The OGC form asked as a CSV file: the first field of every row below the header.
#[cfg(feature = "tabular")]
async fn ogc_csv(app: &Router, form: &str) -> Result<Vec<String>, String> {
    let (key, value) = form.split_once('=').ok_or("a key=value form")?;
    let uri = format!(
        "/x/views/{VIEW}/ogc/features/collections/Vehicle/items?{}",
        query(&[(key, value), ("f", "csv")])
    );
    let req = Request::builder().uri(&uri).header("Host", "localhost");
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .map_err(|e| e.to_string())?;
    let csv = String::from_utf8_lossy(&bytes);
    if status != StatusCode::OK {
        return Err(format!("{uri}: {status} {csv}"));
    }
    Ok(sorted(
        csv.split("\r\n")
            .skip(1)
            .filter(|l| !l.is_empty())
            .map(|l| l.split(',').next().unwrap_or_default().to_owned()),
    ))
}

async fn mcp(app: &Router, q: &str, georel: Option<&str>) -> Result<Vec<String>, String> {
    let mut arguments = json!({"type": ["Vehicle"], "limit": 1000});
    if let Some(map) = arguments.as_object_mut() {
        if !q.is_empty() {
            map.insert("q".into(), json!(q));
        }
        if let Some(rel) = georel {
            let coordinates: Value =
                serde_json::from_str(AREA_GEOJSON).map_err(|e| e.to_string())?;
            map.insert("georel".into(), json!(rel));
            map.insert("geometry".into(), json!("Polygon"));
            map.insert("coordinates".into(), coordinates);
        }
    }
    let (status, body) = mcp_call(app, &arguments).await;
    if status != StatusCode::OK || body.pointer("/result/isError") == Some(&json!(true)) {
        return Err(format!("mcp {arguments}: {status} {body}"));
    }
    if body
        .pointer("/result/structuredContent/nextCursor")
        .is_some()
    {
        return Err(format!("mcp {arguments}: a second page"));
    }
    Ok(ids_at(&body, "/result/structuredContent/entities"))
}

async fn mcp_call(app: &Router, arguments: &Value) -> (StatusCode, Value) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "query_entities", "arguments": arguments,
        "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                  "io.modelcontextprotocol/clientInfo": {"name": "parity", "version": "1"},
                  "io.modelcontextprotocol/clientCapabilities": {}}}});
    send(
        app,
        "POST",
        &format!("/x/views/{VIEW}/mcp"),
        &body.to_string(),
        &[
            ("Accept", "application/json, text/event-stream"),
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "query_entities"),
        ],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_surface_selects_what_the_case_selects() -> Result<(), String> {
    let (_broker, app) = broker().await?;
    let fleet = fleet();
    let mut mismatches = Vec::new();
    let mut unexpressed = Vec::new();
    for case in cases().into_iter().chain(more_cases()) {
        let want = sorted(
            fleet
                .iter()
                .filter(|v| (case.want)(v))
                .map(|v| v.id.clone()),
        );
        if want.is_empty() || want.len() == fleet.len() {
            return Err(format!("{}: a case must split the fleet", case.id));
        }
        let (q, georel) = case.ngsi;
        let mut answers = vec![
            ("ngsi-ld", ngsi(&app, q, georel).await),
            ("mcp", mcp(&app, q, georel).await),
        ];
        match &case.odata {
            Some(filter) => answers.push(("odata", odata(&app, filter).await)),
            None => unexpressed.push(format!("{}: odata", case.id)),
        }
        match &case.ogc {
            Some(form) => {
                answers.push(("ogc", ogc(&app, form).await));
                #[cfg(feature = "parquet")]
                answers.push(("ogc parquet", ogc_parquet(&app, form).await));
                #[cfg(feature = "tabular")]
                answers.push(("ogc csv", ogc_csv(&app, form).await));
            }
            None => unexpressed.push(format!("{}: ogc", case.id)),
        }
        for (surface, got) in answers {
            match got {
                Ok(got) if got == want => {}
                Ok(got) => mismatches.push(format!(
                    "{} on {surface}: {} ids, want {}: {got:?}",
                    case.id,
                    got.len(),
                    want.len()
                )),
                Err(why) => mismatches.push(format!("{} on {surface}: {why}", case.id)),
            }
        }
    }
    if !mismatches.is_empty() {
        return Err(format!(
            "{} mismatches:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        ));
    }
    // Every standard writes every case today; a case one cannot write is named here.
    if !unexpressed.is_empty() {
        return Err(format!(
            "unexpressed cases, name them in the table's doc: {unexpressed:?}"
        ));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_attribute_filters_as_unknown_and_a_malformed_filter_is_400() -> Result<(), String>
{
    let (_broker, app) = broker().await?;
    // `secret` is stored on every Vehicle and hidden by the view: no surface lets it select one.
    let hidden = [
        ("ngsi-ld", ngsi(&app, "secret==\"s\"", None).await),
        ("mcp", mcp(&app, "secret==\"s\"", None).await),
        ("odata", odata(&app, "secret eq 's'").await),
        ("ogc", ogc(&app, "filter=secret = 's'").await),
    ];
    for (surface, answer) in hidden {
        if let Ok(ids) = answer {
            if !ids.is_empty() {
                return Err(format!(
                    "{surface}: a filter on the hidden attribute selected {ids:?}"
                ));
            }
        }
    }
    let malformed = [
        format!(
            "/x/views/{VIEW}/ngsi-ld/v1/entities?{}",
            query(&[("type", "Vehicle"), ("q", "speed>>")])
        ),
        format!(
            "/x/views/{VIEW}/odata/v4/Vehicle?{}",
            query(&[("$filter", "speed gt")])
        ),
        format!(
            "/x/views/{VIEW}/ogc/features/collections/Vehicle/items?{}",
            query(&[("filter", "speed >")])
        ),
    ];
    for uri in malformed {
        let (status, body) = send(&app, "GET", &uri, "", &[]).await;
        if status != StatusCode::BAD_REQUEST {
            return Err(format!("{uri}: {status} {body}, want 400"));
        }
    }
    let (status, body) = mcp_call(&app, &json!({"type": ["Vehicle"], "q": "speed>>"})).await;
    if status.is_server_error()
        || body.pointer("/result/isError") != Some(&json!(true)) && body.get("error").is_none()
    {
        return Err(format!("mcp malformed q: {status} {body}"));
    }
    Ok(())
}

/// T-5169: an id list selects the same Entities on every surface, a missing id selecting none;
/// OGC lowers `id IN (…)`, alone or with a value conjunct, to the NGSI-LD `id` list.
#[tokio::test(flavor = "multi_thread")]
async fn an_id_list_selects_the_same_on_every_surface() -> Result<(), String> {
    let (_broker, app) = broker().await?;
    let (a, b, none) = (
        "urn:ngsi-ld:Vehicle:03",
        "urn:ngsi-ld:Vehicle:10",
        "urn:ngsi-ld:Vehicle:99",
    );
    let want = sorted([a.to_owned(), b.to_owned()]);
    let uri = format!(
        "/x/views/{VIEW}/ngsi-ld/v1/entities?{}",
        query(&[("type", "Vehicle"), ("id", &format!("{a},{b},{none}"))])
    );
    let (status, body) = send(&app, "GET", &uri, "", &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(sorted(ids_at(&body, "")), want, "ngsi-ld");
    let odata_ids = odata(&app, &format!("id in ('{a}','{b}','{none}')")).await?;
    assert_eq!(sorted(odata_ids), want, "odata");
    let form = format!("filter=id IN ('{a}','{b}','{none}')");
    assert_eq!(sorted(ogc(&app, &form).await?), want, "ogc");
    #[cfg(feature = "tabular")]
    assert_eq!(ogc_csv(&app, &form).await?, want, "ogc csv");
    // Vehicle 03 is a car, 10 a tram: the value conjunct keeps one.
    let anded = format!("filter=id IN ('{a}','{b}') AND vehicleType = 'tram'");
    assert_eq!(ogc(&app, &anded).await?, [b.to_owned()], "ogc and");
    Ok(())
}
