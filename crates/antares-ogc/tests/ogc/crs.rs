// SPDX-License-Identifier: EUPL-1.2
//! Part 2 (OGC 18-058r1): `CRS84` and `CRS84h` only (mapping rows R1 to R5).

use crate::common::{fixture, get, send};
use axum::http::StatusCode;
use serde_json::json;

const ROOT: &str = "/x/views/city/ogc/features";
const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
const CRS84H: &str = "http://www.opengis.net/def/crs/OGC/0/CRS84h";
const EPSG: &str = "http://www.opengis.net/def/crs/EPSG/0/4326";

#[tokio::test(flavor = "multi_thread")]
async fn collection_lists_crs84_only() -> Result<(), String> {
    let f = fixture().await?;
    let all = get(&f.app, &format!("{ROOT}/collections")).await;
    assert_eq!(all.body.get("crs"), Some(&json!([CRS84, CRS84H])));
    let car = get(&f.app, &format!("{ROOT}/collections/Car")).await;
    assert_eq!(
        (car.body.get("crs"), car.body.get("storageCrs")),
        (Some(&json!([CRS84, CRS84H])), Some(&json!(CRS84)))
    );
    let classes = get(&f.app, &format!("{ROOT}/conformance")).await;
    assert!(classes
        .body
        .get("conformsTo")
        .and_then(|c| c.as_array())
        .is_some_and(|c| c.contains(&json!(
            "http://www.opengis.net/spec/ogcapi-features-2/1.0/conf/crs"
        ))));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_crs_is_400_with_the_list() -> Result<(), String> {
    let f = fixture().await?;
    for path in [
        format!("{ROOT}/collections/Car/items?crs={EPSG}"),
        format!("{ROOT}/collections/Car/items/urn:ngsi-ld:Car:1?crs={EPSG}"),
    ] {
        let a = get(&f.app, &path).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{path}");
        let detail = a
            .body
            .get("detail")
            .and_then(|d| d.as_str())
            .unwrap_or_default();
        assert!(
            detail.contains(CRS84) && detail.contains(CRS84H),
            "{detail}"
        );
    }
    for crs in [CRS84, CRS84H, "[OGC:CRS84]"] {
        let a = get(&f.app, &format!("{ROOT}/collections/Car/items?crs={crs}")).await;
        assert_eq!(a.status, StatusCode::OK, "{crs}");
    }
    let write = send(
        &f.app,
        "POST",
        "/x/views/fleet/ogc/features/collections/Car/items",
        &json!({"type": "Feature", "geometry": null, "properties": {}}).to_string(),
        &[("Content-Crs", &format!("<{EPSG}>"))],
    )
    .await;
    assert_eq!(
        write.status,
        StatusCode::BAD_REQUEST,
        "a write in another CRS"
    );
    let fine = send(
        &f.app,
        "POST",
        "/x/views/fleet/ogc/features/collections/Car/items",
        &json!({"type": "Feature", "geometry": null, "properties": {}}).to_string(),
        &[("Content-Crs", &format!("<{CRS84}>"))],
    )
    .await;
    assert_eq!(fine.status, StatusCode::CREATED, "{}", fine.body);
    // A Feature without a location keeps `geometry: null` under `?crs=` (RFC 7946 3.2), on the
    // item and on the items page (T-6034: the ETS transform tests read both).
    let at = fine
        .headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or("no Location")?
        .trim_start_matches("http://localhost:9090")
        .to_owned();
    let id = at
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .replace("%3A", ":");
    for crs in [CRS84, CRS84H] {
        let item = get(&f.app, &format!("{at}?crs={crs}")).await;
        assert_eq!(item.status, StatusCode::OK, "{crs}: {}", item.body);
        assert_eq!(item.body.get("geometry"), Some(&json!(null)), "{crs}");
        let page = get(
            &f.app,
            &format!("/x/views/fleet/ogc/features/collections/Car/items?crs={crs}&limit=1000"),
        )
        .await;
        assert_eq!(page.status, StatusCode::OK, "{crs}: {}", page.body);
        let feature = page
            .body
            .get("features")
            .and_then(|f| f.as_array())
            .and_then(|f| f.iter().find(|x| x.get("id") == Some(&json!(id))))
            .ok_or(format!("{id} not on the {crs} page: {}", page.body))?;
        assert_eq!(feature.get("geometry"), Some(&json!(null)), "{crs}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_bbox_crs_is_400() -> Result<(), String> {
    let f = fixture().await?;
    let items = format!("{ROOT}/collections/Car/items?bbox=0,-1,1,1");
    let a = get(&f.app, &format!("{items}&bbox-crs={EPSG}")).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);
    let b = get(&f.app, &format!("{items}&bbox-crs={CRS84}")).await;
    assert_eq!(b.status, StatusCode::OK, "{}", b.body);
    let item = get(
        &f.app,
        &format!("{ROOT}/collections/Car/items/urn:ngsi-ld:Car:1?bbox-crs={CRS84}"),
    )
    .await;
    assert_eq!(
        item.status,
        StatusCode::BAD_REQUEST,
        "no bbox on one Feature"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn content_crs_header() -> Result<(), String> {
    let f = fixture().await?;
    let header = |a: &crate::common::Answer| {
        a.headers
            .get("content-crs")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let items = get(&f.app, &format!("{ROOT}/collections/Car/items")).await;
    assert_eq!(header(&items), Some(format!("<{CRS84}>")));
    let one = get(
        &f.app,
        &format!("{ROOT}/collections/Car/items/urn:ngsi-ld:Car:1?crs={CRS84H}"),
    )
    .await;
    assert_eq!(header(&one), Some(format!("<{CRS84H}>")));
    let collection = get(&f.app, &format!("{ROOT}/collections/Car")).await;
    assert_eq!(header(&collection), None, "no coordinates, no Content-Crs");
    Ok(())
}
