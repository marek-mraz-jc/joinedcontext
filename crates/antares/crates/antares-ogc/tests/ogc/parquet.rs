// SPDX-License-Identifier: EUPL-1.2
//! T-4465, EP-119: `items` as Apache Parquet and `GeoParquet` 1.1, read back with the parquet
//! reader and held against the `GeoJSON` answer of the same request.
#![cfg(feature = "parquet")]

use crate::common::{fixture, fixture_with, get, href, ids, schemas_on, send, Fixture};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use serde_json::{json, Value};
use tower::ServiceExt;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";

/// The status, the headers and the raw bytes of a `GET`.
async fn raw(
    f: &Fixture,
    uri: &str,
    accept: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::builder().uri(uri);
    if let Some(accept) = accept {
        req = req.header("Accept", accept);
    }
    let Ok(resp) = f
        .app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    (status, headers, bytes)
}

type Row = Vec<(String, Field)>;

/// A Parquet file's rows as (column, field) lists, and its file metadata reader.
fn rows(bytes: Vec<u8>) -> Result<(SerializedFileReader<bytes::Bytes>, Vec<Row>), String> {
    let file = SerializedFileReader::new(bytes::Bytes::from(bytes)).map_err(|e| e.to_string())?;
    let rows = file
        .get_row_iter(None)
        .map_err(|e| e.to_string())?
        .map(|r| {
            r.map(|r| {
                r.get_column_iter()
                    .map(|(n, f)| (n.clone(), f.clone()))
                    .collect()
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok((file, rows))
}

fn column(row: &[(String, Field)], name: &str) -> Option<Field> {
    row.iter().find(|(n, _)| n == name).map(|(_, f)| f.clone())
}

fn ids_of(rows: &[Vec<(String, Field)>]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match column(r, "id") {
            Some(Field::Str(id)) => Some(id),
            _ => None,
        })
        .collect()
}

/// The coordinates of a WKB point, little-endian.
fn point(wkb: &[u8]) -> Option<[f64; 2]> {
    let f = |at: usize| {
        Some(f64::from_le_bytes(
            wkb.get(at..at.checked_add(8)?)?.try_into().ok()?,
        ))
    };
    (wkb.first() == Some(&1) && wkb.get(1..5) == Some(&1u32.to_le_bytes()[..])).then_some(())?;
    Some([f(5)?, f(13)?])
}

fn geo(file: &SerializedFileReader<bytes::Bytes>) -> Option<Value> {
    file.metadata()
        .file_metadata()
        .key_value_metadata()?
        .iter()
        .find(|k| k.key == "geo")
        .and_then(|k| k.value.as_deref())
        .and_then(|v| serde_json::from_str(v).ok())
}

#[tokio::test]
async fn a_file_holds_the_rows_the_geojson_answer_holds_for_the_same_request() -> Result<(), String>
{
    let f = fixture().await?;
    for (view, query) in [
        ("city", ""),
        ("city", "bbox=170,-10,180,10"),
        ("city", "filter=speed%20%3E%2020"),
        ("city", "ngsild.q=speed%3C50"),
        ("slow", ""),
    ] {
        let at = CARS.replace("/city/", &format!("/{view}/"));
        let geojson = get(&f.app, &format!("{at}?limit=1000&{query}")).await;
        assert_eq!(geojson.status, StatusCode::OK, "{query}: {}", geojson.body);
        let (status, headers, bytes) = raw(&f, &format!("{at}?f=parquet&{query}"), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{view} {query}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(
            headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some("application/vnd.apache.parquet")
        );
        assert_eq!(
            headers
                .get("content-disposition")
                .and_then(|v| v.to_str().ok()),
            Some("attachment; filename=\"Car.parquet\"")
        );
        let (_, rows) = rows(bytes)?;
        assert_eq!(ids_of(&rows), ids(&geojson.body), "{view} {query}");
    }
    Ok(())
}

#[tokio::test]
async fn the_geometry_is_wkb_with_geoparquet_metadata_and_hidden_attributes_are_absent(
) -> Result<(), String> {
    let f = fixture().await?;
    let (status, _, bytes) = raw(&f, CARS, Some("application/vnd.apache.parquet")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an Accept naming Parquet asks for it too"
    );
    let (file, rows) = rows(bytes)?;
    let names: Vec<String> = file
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|c| c.name().to_owned())
        .collect();
    assert_eq!(names.first().map(String::as_str), Some("id"));
    assert!(
        names.iter().any(|n| n == "geometry") && names.iter().any(|n| n == "speed"),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "secret"),
        "the view hides it: {names:?}"
    );
    let car = |id: &str| {
        rows.iter()
            .find(|r| column(r, "id") == Some(Field::Str(id.to_owned())))
    };
    let wkb = |id: &str| match car(id).and_then(|r| column(r, "geometry")) {
        Some(Field::Bytes(b)) => point(b.data()),
        _ => None,
    };
    assert_eq!(wkb("urn:ngsi-ld:Car:1"), Some([17.1, 48.1]));
    assert_eq!(wkb("urn:ngsi-ld:Car:4"), Some([-179.5, 0.0]));
    assert_eq!(
        car("urn:ngsi-ld:Car:3").and_then(|r| column(r, "geometry")),
        Some(Field::Null),
        "a Car without a location has no geometry"
    );
    assert_eq!(
        car("urn:ngsi-ld:Car:1").and_then(|r| column(r, "speed")),
        Some(Field::Double(10.0)),
        "speed is a float slot"
    );
    let geo = geo(&file).ok_or("no geo metadata")?;
    assert_eq!(geo.pointer("/version"), Some(&json!("1.1.0")));
    assert_eq!(geo.pointer("/primary_column"), Some(&json!("geometry")));
    assert_eq!(
        geo.pointer("/columns/geometry/encoding"),
        Some(&json!("WKB"))
    );
    assert_eq!(
        geo.pointer("/columns/geometry/geometry_types"),
        Some(&json!(["Point"]))
    );
    assert_eq!(
        geo.pointer("/columns/geometry/bbox"),
        Some(&json!([-179.5, 0.0, 179.5, 48.1]))
    );
    Ok(())
}

#[tokio::test]
async fn properties_keeps_its_columns_only() -> Result<(), String> {
    let f = fixture().await?;
    let (status, _, bytes) = raw(&f, &format!("{CARS}?f=parquet&properties=speed"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (file, _) = rows(bytes)?;
    let names: Vec<String> = file
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|c| c.name().to_owned())
        .collect();
    assert_eq!(names, ["id", "geometry", "speed"]);
    Ok(())
}

#[tokio::test]
async fn a_collection_without_a_geometry_is_plain_parquet() -> Result<(), String> {
    let f = fixture().await?;
    let (status, _, bytes) = raw(
        &f,
        "/x/views/empty/ogc/features/collections/Stop/items?f=parquet",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (file, rows) = rows(bytes)?;
    assert!(geo(&file).is_none(), "no geo metadata without a geometry");
    assert_eq!(ids_of(&rows), ["urn:ngsi-ld:Stop:1"]);
    assert_eq!(
        rows.first().and_then(|r| column(r, "name")),
        Some(Field::Str("Hlavná".into()))
    );
    Ok(())
}

#[tokio::test]
async fn the_geojson_links_its_parquet_and_a_file_is_only_in_crs84() -> Result<(), String> {
    let f = fixture().await?;
    let page = get(&f.app, &format!("{CARS}?limit=2")).await;
    let alternate = href(&page.body, "alternate").ok_or("an alternate link")?;
    assert!(
        alternate.ends_with("/items?limit=2&f=parquet"),
        "{alternate}"
    );
    let bare = get(&f.app, CARS).await;
    let alternate = href(&bare.body, "alternate").ok_or("an alternate link")?;
    assert!(alternate.ends_with("/items?f=parquet"), "{alternate}");
    let unknown = get(&f.app, &format!("{CARS}?f=parquet&limit=abc")).await;
    assert_eq!(
        unknown.status,
        StatusCode::BAD_REQUEST,
        "the items' own refusals: {}",
        unknown.body
    );
    let crs = "crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F0%2FCRS84h";
    let refused = get(&f.app, &format!("{CARS}?f=parquet&{crs}")).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    let one = get(&f.app, &format!("{CARS}/urn%3Angsi-ld%3ACar%3A1?f=parquet")).await;
    assert_eq!(
        one.status,
        StatusCode::BAD_REQUEST,
        "a single Feature writes no file: {}",
        one.body
    );
    let landing = send(
        &f.app,
        "GET",
        "/x/views/city/ogc/features",
        "",
        &[("Accept", "application/vnd.apache.parquet")],
    )
    .await;
    assert_eq!(
        landing.status,
        StatusCode::NOT_ACCEPTABLE,
        "only items write a file"
    );
    Ok(())
}

#[tokio::test]
async fn a_file_past_the_answer_ceiling_is_refused_whole() -> Result<(), String> {
    schemas_on();
    let f = fixture_with(TestBroker::with_max_answer_bytes(600).await?).await?;
    let (status, headers, bytes) = raw(&f, &format!("{CARS}?f=parquet"), None).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/problem+json"),
        "an error, never a cut file"
    );
    Ok(())
}

/// Ten thousand Cars are ten pages of a thousand: each is one row group, written as it is
/// read, so the surface holds one page of Features at a time.
#[tokio::test]
async fn ten_thousand_rows_are_written_one_page_at_a_time() -> Result<(), String> {
    let f = fixture().await?;
    for batch in 0..10 {
        let cars: Vec<Value> = (0..1000)
            .map(|i| {
                let n = batch * 1000 + i;
                json!({"id": format!("urn:ngsi-ld:Car:bulk-{n}"), "type": "Car",
                       "speed": {"type": "Property", "value": n % 120}})
            })
            .collect();
        let a = send(
            &f.app,
            "POST",
            "/ngsi-ld/v1/entityOperations/create",
            &Value::Array(cars).to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        assert!(
            a.status.is_success(),
            "batch {batch}: {} {}",
            a.status,
            a.body
        );
    }
    let (status, _, bytes) = raw(&f, &format!("{CARS}?f=parquet"), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (file, rows) = rows(bytes)?;
    let meta = file.metadata();
    assert_eq!(rows.len(), 10_004, "the bulk Cars and the fixture's four");
    assert_eq!(
        meta.num_row_groups(),
        11,
        "a row group per page of a thousand"
    );
    let largest = (0..meta.num_row_groups())
        .map(|i| meta.row_group(i).num_rows())
        .max()
        .unwrap_or_default();
    assert_eq!(largest, 1000, "no row group holds more than one page");
    Ok(())
}
