// SPDX-License-Identifier: EUPL-1.2
//! V20 (surfaces-threat-model): the view id is the raw path segment, matched
//! against the loaded ids before any decoding or normalisation, so no
//! spelling of a path reaches another view, the root or `/q`.

use crate::common::{fixture, get, CARS};
use axum::http::StatusCode;

#[tokio::test(flavor = "multi_thread")]
async fn a_view_id_is_matched_on_the_raw_segment() -> Result<(), String> {
    let f = fixture().await?;
    assert_eq!(
        get(&f.app, &format!("{CARS}/types")).await.status,
        StatusCode::OK
    );
    for uri in [
        "/x/views/ca%72s/ngsi-ld/v1/types",
        "/x/views/CARS/ngsi-ld/v1/types",
        "/x/views/cars./ngsi-ld/v1/types",
        "/x/views/cars%2Fx/ngsi-ld/v1/types",
        "/x/views/history/../cars/ngsi-ld/v1/types",
        "/x/views/x/../cars/ngsi-ld/v1/types",
        "/x/views//ngsi-ld/v1/types",
        "/x/views/cars//ngsi-ld/v1/types",
        "/x/views/cars/ngsi-ld/v1/../../../q/health",
        "/x/views/cars/ngsi-ld/v1/%2E%2E/%2E%2E/q/views",
        "/x/views/cars/ngsi-ld/v2/types",
        "/x/views/cars/ngsi-ld/v1",
        "/x/views/cars",
    ] {
        let a = get(&f.app, uri).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{uri}: {}", a.text);
        assert!(
            !a.text.contains("\"Car\""),
            "{uri} served the types: {}",
            a.text
        );
    }
    Ok(())
}
