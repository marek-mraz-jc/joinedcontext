// SPDX-License-Identifier: EUPL-1.2
//! Cache scopes (MCP D-6, MC9).

use crate::common::{fixture, rpc};
use serde_json::json;

/// MC9: only `server/discover` may be shared across callers.
#[tokio::test(flavor = "multi_thread")]
async fn every_cacheable_result_but_discover_is_private() -> Result<(), String> {
    let f = fixture().await?;
    for (method, params) in [
        ("tools/list", json!({})),
        ("resources/list", json!({})),
        ("resources/templates/list", json!({})),
        (
            "resources/read",
            json!({"uri": "ngsi-ld://cars/entities/urn:ngsi-ld:Car:1"}),
        ),
    ] {
        let a = rpc(&f.app, "cars", method, params).await;
        assert_eq!(
            a.result().get("cacheScope"),
            Some(&json!("private")),
            "{method}: {}",
            a.text
        );
    }
    let d = rpc(&f.app, "cars", "server/discover", json!({})).await;
    assert_eq!(
        d.result().get("cacheScope"),
        Some(&json!("public")),
        "{}",
        d.text
    );
    let text = d.result().to_string();
    for name in ["Car", "speed", "cars", "city"] {
        assert!(!text.contains(&format!("\"{name}\"")), "{name} in {text}");
    }
    Ok(())
}
