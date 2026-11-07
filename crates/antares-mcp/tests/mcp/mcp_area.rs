// SPDX-License-Identifier: EUPL-1.2
//! Integration tests for `area_overview` (T-5279): one area across 25 types,
//! per-type counts equal to a per-type `query_entities` count, `k` honoured,
//! the edge of a radius and of a line's buffer by the metre, polygons, the
//! limits and the view's narrowing.

use crate::common::{call, fixture, prop};
use serde_json::{json, Value};

const TENANT: &str = "area";
const CENTRE: [f64; 2] = [24.94, 60.17];
const TYPES: usize = 25;

/// Degrees of latitude per metre on the broker's mean sphere.
fn lat_m(metres: f64) -> f64 {
    (metres / 6_371_008.8).to_degrees()
}

fn kind(i: usize) -> String {
    format!("Kind{i:02}")
}

async fn place(
    app: &axum::Router,
    id: &str,
    t: &str,
    lon: f64,
    lat: f64,
    height: u64,
) -> Result<(), String> {
    crate::common::create(
        app,
        TENANT,
        &json!({
            "id": id,
            "type": t,
            "name": prop(&json!(id)),
            "height": {"type": "Property", "value": height, "unitCode": "MTR"},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [lon, lat]}}
        }),
    )
    .await
}

/// 25 types of three Entities 100, 200 and 300 m north of the centre (plus
/// the type's index in metres), each with one more 10 km away; the views
/// `all` (every type), `half` (the first twelve) and `low` (heights under 3).
async fn setup(app: &axum::Router) -> Result<(), String> {
    let types: Vec<String> = (0..TYPES).map(kind).collect();
    let view = |id: &str, types: &[String], q: Option<&str>| {
        let attributes = ["name", "height", "location"];
        let rule = match q {
            Some(q) => json!({"types": types, "attributes": attributes, "q": q}),
            None => json!({"types": types, "attributes": attributes}),
        };
        json!({"id": id, "tenant": TENANT, "selector": types, "rules": [rule], "operations": crate::common::READS})
    };
    crate::common::put_view(app, &view("all", &types, None)).await?;
    crate::common::put_view(
        app,
        &view("half", types.get(..12).unwrap_or_default(), None),
    )
    .await?;
    crate::common::put_view(app, &view("low", &types, Some("height<3"))).await?;
    for (i, t) in types.iter().enumerate() {
        let extra = f64::from(u32::try_from(i).unwrap_or_default());
        for j in 1..=3_u32 {
            let lat = CENTRE[1] + lat_m(f64::from(j) * 100.0 + extra);
            place(
                app,
                &format!("urn:ngsi-ld:{t}:{j}"),
                t,
                CENTRE[0],
                lat,
                u64::from(j),
            )
            .await?;
        }
        place(
            app,
            &format!("urn:ngsi-ld:{t}:far"),
            t,
            CENTRE[0],
            CENTRE[1] + lat_m(10_000.0),
            1,
        )
        .await?;
    }
    Ok(())
}

fn types_of(answer: &Value) -> Vec<Value> {
    answer
        .get("types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn part<'a>(types: &'a [Value], t: &str) -> Option<&'a Value> {
    types
        .iter()
        .find(|p| p.get("type").and_then(Value::as_str) == Some(t))
}

fn ids(part: Option<&Value>) -> Vec<String> {
    part.and_then(|p| p.get("entities")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.get("id")?.as_str().map(str::to_owned))
        .collect()
}

/// A point and a radius: every type, its count equal to `query_entities`' count, the `k` nearest.
#[tokio::test(flavor = "multi_thread")]
async fn a_point_counts_every_type_and_keeps_the_k_nearest() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let args = json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": 1000, "limit": 2});
    let a = call(&f.app, "all", "area_overview", args).await;
    assert!(!a.is_error(), "{}", a.text);
    let types = types_of(a.structured());
    assert_eq!(types.len(), TYPES, "{}", a.text);
    for t in (0..TYPES).map(kind) {
        let p = part(&types, &t);
        let q = call(
            &f.app,
            "all",
            "query_entities",
            json!({"type": [t], "georel": "near;maxDistance==1000", "geometry": "Point", "coordinates": CENTRE, "count": true, "limit": 1}),
        )
        .await;
        assert_eq!(
            p.and_then(|p| p.get("count")),
            q.structured().get("total"),
            "{t}"
        );
        assert_eq!(
            ids(p),
            [format!("urn:ngsi-ld:{t}:1"), format!("urn:ngsi-ld:{t}:2")]
        );
        assert_eq!(p.and_then(|p| p.get("ranked")), Some(&json!("nearest")));
    }
    let first = part(&types, "Kind00")
        .and_then(|p| p.pointer("/entities/0"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(first.get("distance_m"), Some(&json!(100.0)), "{first}");
    assert_eq!(
        first.pointer("/attributes/height/unitCode"),
        Some(&json!("MTR"))
    );
    assert!(a.text.len() < 20 * 1024, "{}", a.text.len());
    Ok(())
}

/// One metre inside the radius is in, one metre outside is out.
#[tokio::test(flavor = "multi_thread")]
async fn the_radius_holds_by_the_metre() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    // Kind00 sits at 100, 200 and 300 m: 199 m takes one, 201 m two
    for (radius, n) in [(199, 1), (201, 2)] {
        let a = call(
            &f.app,
            "all",
            "area_overview",
            json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": radius, "type": ["Kind00"]}),
        )
        .await;
        let types = types_of(a.structured());
        assert_eq!(
            part(&types, "Kind00").and_then(|p| p.get("count")),
            Some(&json!(n)),
            "{}",
            a.text
        );
    }
    Ok(())
}

/// A line keeps what lies within its buffer, by the metre.
#[tokio::test(flavor = "multi_thread")]
async fn a_line_keeps_its_buffer_by_the_metre() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let lat = CENTRE[1] + lat_m(60.0);
    place(
        &f.app,
        "urn:ngsi-ld:Kind00:in",
        "Kind00",
        CENTRE[0],
        lat + lat_m(49.0),
        1,
    )
    .await?;
    place(
        &f.app,
        "urn:ngsi-ld:Kind00:out",
        "Kind00",
        CENTRE[0],
        lat + lat_m(51.0),
        1,
    )
    .await?;
    let line = json!([[CENTRE[0] - 0.01, lat], [CENTRE[0] + 0.01, lat]]);
    let a = call(
        &f.app,
        "all",
        "area_overview",
        json!({"geometry": "LineString", "coordinates": line, "maxDistance": 50, "type": ["Kind00"], "limit": 5}),
    )
    .await;
    let types = types_of(a.structured());
    let p = part(&types, "Kind00");
    // urn:…:1 at 100 m is 40 m north of the line, in; :2 at 200 m is out
    let got = ids(p);
    assert!(
        got.contains(&"urn:ngsi-ld:Kind00:in".to_owned()),
        "{}",
        a.text
    );
    assert!(
        !got.contains(&"urn:ngsi-ld:Kind00:out".to_owned()),
        "{}",
        a.text
    );
    assert_eq!(
        p.and_then(|p| p.get("count")),
        Some(&json!(2)),
        "{}",
        a.text
    );
    assert_eq!(
        a.structured().pointer("/area/distanceFrom"),
        Some(&json!("line"))
    );
    Ok(())
}

/// A `Polygon` and a `MultiPolygon` count what lies in them.
#[tokio::test(flavor = "multi_thread")]
async fn polygons_count_what_lies_in_them() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let [x, y] = CENTRE;
    let (south, north) = (y + lat_m(150.0), y + lat_m(250.0));
    // band_answer band holding each type's 200 multi_answer Entity only
    let band = json!([[
        [x - 0.01, south],
        [x + 0.01, south],
        [x + 0.01, north],
        [x - 0.01, north],
        [x - 0.01, south]
    ]]);
    let band_answer = call(
        &f.app,
        "all",
        "area_overview",
        json!({"geometry": "Polygon", "coordinates": band}),
    )
    .await;
    let types = types_of(band_answer.structured());
    assert_eq!(types.len(), TYPES, "{}", band_answer.text);
    assert!(
        types.iter().all(|p| p.get("count") == Some(&json!(1))),
        "{}",
        band_answer.text
    );
    let (s2, n2) = (y + lat_m(50.0), y + lat_m(120.0));
    let low = json!([[
        [x - 0.01, s2],
        [x + 0.01, s2],
        [x + 0.01, n2],
        [x - 0.01, n2],
        [x - 0.01, s2]
    ]]);
    let multi = json!([band, low]);
    let multi_answer = call(
        &f.app,
        "all",
        "area_overview",
        json!({"geometry": "MultiPolygon", "coordinates": multi, "type": ["Kind00"]}),
    )
    .await;
    let types = types_of(multi_answer.structured());
    assert_eq!(
        part(&types, "Kind00").and_then(|p| p.get("count")),
        Some(&json!(2)),
        "{}",
        multi_answer.text
    );
    Ok(())
}

/// Each limit is refused with its slug, nothing read.
#[tokio::test(flavor = "multi_thread")]
async fn each_limit_is_refused_with_its_slug() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let [x, y] = CENTRE;
    let bow = json!([[
        [x, y],
        [x + 0.01, y + 0.01],
        [x + 0.01, y],
        [x, y + 0.01],
        [x, y]
    ]]);
    let mut ring: Vec<Value> = (0..1000)
        .map(|i| json!([x + f64::from(i) * 1e-6, y + f64::from(i % 2) * 1e-6]))
        .collect();
    ring.push(json!([x, y]));
    let wide = json!([[
        [x, y],
        [x + 0.5, y],
        [x + 0.5, y + 0.5],
        [x, y + 0.5],
        [x, y]
    ]]);
    for (args, slug) in [
        (
            json!({"geometry": "Polygon", "coordinates": bow}),
            "invalid-polygon",
        ),
        (
            json!({"geometry": "Polygon", "coordinates": [ring]}),
            "too-many-vertices",
        ),
        (
            json!({"geometry": "Polygon", "coordinates": wide}),
            "area-too-large",
        ),
        (
            json!({"geometry": "Point", "coordinates": [200.0, y]}),
            "coordinates-out-of-range",
        ),
    ] {
        let a = call(&f.app, "all", "area_overview", args).await;
        assert!(a.is_error(), "{slug}: {}", a.text);
        assert_eq!(
            a.structured().pointer("/error/type"),
            Some(&json!(slug)),
            "{}",
            a.text
        );
        assert_eq!(a.structured().pointer("/error/status"), Some(&json!(400)));
    }
    let far = call(
        &f.app,
        "all",
        "area_overview",
        json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": 5001}),
    )
    .await;
    assert!(
        far.is_error() && far.text.contains("maxDistance"),
        "{}",
        far.text
    );
    Ok(())
}

/// A type the view does not show adds neither its count nor its Entities,
/// and a rule's `q` narrows the counts.
#[tokio::test(flavor = "multi_thread")]
async fn the_view_narrows_types_and_counts() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let args = json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": 1000});
    let half = call(&f.app, "half", "area_overview", args.clone()).await;
    let types = types_of(half.structured());
    assert_eq!(types.len(), 12, "{}", half.text);
    assert!(!half.text.contains("Kind20"), "{}", half.text);
    let named = call(
        &f.app,
        "half",
        "area_overview",
        json!({"geometry": "Point", "coordinates": CENTRE, "type": ["Kind20"]}),
    )
    .await;
    assert!(
        named.is_error() && !named.text.contains("urn:ngsi-ld:Kind20"),
        "{}",
        named.text
    );
    let low = call(&f.app, "low", "area_overview", args).await;
    let types = types_of(low.structured());
    assert!(
        types.iter().all(|p| p.get("count") == Some(&json!(2))),
        "{}",
        low.text
    );
    assert!(!low.text.contains(":3\""), "{}", low.text);
    Ok(())
}

/// The tool is listed read-only.
#[tokio::test(flavor = "multi_thread")]
async fn area_overview_is_a_read_only_tool() -> Result<(), String> {
    let f = fixture().await?;
    setup(&f.app).await?;
    let list = crate::common::rpc(&f.app, "all", "tools/list", json!({})).await;
    let tool = list
        .result()
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|t| {
            t.iter()
                .find(|t| t.get("name") == Some(&json!("area_overview")))
                .cloned()
        })
        .unwrap_or_default();
    assert_eq!(
        tool.pointer("/annotations/readOnlyHint"),
        Some(&json!(true)),
        "{tool}"
    );
    assert_eq!(
        tool.pointer("/inputSchema/properties/maxDistance/maximum"),
        Some(&json!(5000))
    );
    Ok(())
}

/// More than 200 Entities of a type: the nearest come from a smaller ring
/// when it holds `k` and is read whole, else from a sample, and say which.
#[tokio::test(flavor = "multi_thread")]
async fn a_dense_type_ranks_its_nearest_from_a_ring_or_says_it_sampled() -> Result<(), String> {
    let f = fixture().await?;
    let types = ["Dense", "Rim", "Core"];
    let view = json!({"id": "dense", "tenant": TENANT, "selector": types,
        "rules": [{"types": types, "attributes": ["name", "location"]}], "operations": crate::common::READS});
    crate::common::put_view(&f.app, &view).await?;
    for i in 0..250_u32 {
        let step = f64::from(i);
        // Dense spread over the whole radius, Rim at its edge, Core at its centre
        place(
            &f.app,
            &format!("urn:ngsi-ld:Dense:{i}"),
            "Dense",
            CENTRE[0],
            CENTRE[1] + lat_m(10.0 + 3.9 * step),
            1,
        )
        .await?;
        place(
            &f.app,
            &format!("urn:ngsi-ld:Rim:{i}"),
            "Rim",
            CENTRE[0],
            CENTRE[1] + lat_m(990.0 + step / 100.0),
            1,
        )
        .await?;
        place(
            &f.app,
            &format!("urn:ngsi-ld:Core:{i}"),
            "Core",
            CENTRE[0],
            CENTRE[1] + lat_m(1.0 + step / 100.0),
            1,
        )
        .await?;
    }
    let args = json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": 1000, "limit": 3, "lang": "fi"});
    let a = call(&f.app, "dense", "area_overview", args).await;
    let types = types_of(a.structured());
    let ranked = |t: &str| part(&types, t).and_then(|p| p.get("ranked")).cloned();
    assert_eq!(ranked("Dense"), Some(json!("nearest")), "{}", a.text);
    assert_eq!(
        ids(part(&types, "Dense")),
        [
            "urn:ngsi-ld:Dense:0",
            "urn:ngsi-ld:Dense:1",
            "urn:ngsi-ld:Dense:2"
        ]
    );
    assert_eq!(ranked("Rim"), Some(json!("sample")), "{}", a.text);
    assert_eq!(ranked("Core"), Some(json!("sample")), "{}", a.text);
    let count = |t: &str| part(&types, t).and_then(|p| p.get("count")).cloned();
    assert_eq!(
        (count("Dense"), count("Rim"), count("Core")),
        (Some(json!(250)), Some(json!(250)), Some(json!(250)))
    );
    Ok(())
}

/// A view that shows every name reads the Tenant's types through the view;
/// one that may not list types refuses the call.
#[tokio::test(flavor = "multi_thread")]
async fn every_name_reads_the_tenant_s_types_and_a_refused_list_is_the_answer() -> Result<(), String>
{
    let f = fixture().await?;
    setup(&f.app).await?;
    let every = |id: &str, operations: Value| {
        json!({"id": id, "tenant": TENANT, "selector": ["*"],
            "rules": [{"types": ["*"], "attributes": ["*"]}], "operations": operations})
    };
    crate::common::put_view(&f.app, &every("every", json!(crate::common::READS))).await?;
    crate::common::put_view(&f.app, &every("blind", json!(["query_entities"]))).await?;
    let args = json!({"geometry": "Point", "coordinates": CENTRE, "maxDistance": 1000});
    let a = call(&f.app, "every", "area_overview", args.clone()).await;
    assert_eq!(types_of(a.structured()).len(), TYPES, "{}", a.text);
    let blind = call(&f.app, "blind", "area_overview", args).await;
    assert!(blind.is_error(), "{}", blind.text);
    Ok(())
}

/// The engine refusing every typed read leaves every type out: an empty
/// area, never a refusal that names a type.
#[tokio::test(flavor = "multi_thread")]
async fn a_type_the_engine_refuses_adds_nothing() -> Result<(), String> {
    let f = crate::common::fixture_with(Some(std::sync::Arc::new(crate::engines::NoNamedTypes)))
        .await?;
    let a = call(
        &f.app,
        "cars",
        "area_overview",
        json!({"geometry": "Point", "coordinates": [0.0, 0.0], "maxDistance": 5000}),
    )
    .await;
    assert!(!a.is_error(), "{}", a.text);
    assert_eq!(types_of(a.structured()), Vec::<Value>::new());
    assert!(!a.text.contains("Car"), "{}", a.text);
    Ok(())
}
