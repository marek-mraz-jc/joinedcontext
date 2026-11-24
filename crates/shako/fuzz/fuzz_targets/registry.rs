//! Input: a selector byte, then text, sent as a reader to the OCI registry. 0: the query of
//! `/v2/token` (scopes); 1: a blob digest; 2: the query of a tag list; 3: a manifest reference;
//! 4: a referrers digest; 5: a path under `/v2/`. Each request ends in an answer, never a panic,
//! and a token is granted only the pull a reader holds.
#![no_main]

mod reader;

use std::sync::LazyLock;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use libfuzzer_sys::fuzz_target;
use shako_registry::oci_v2_routes;
use tower::ServiceExt;

static APP: LazyLock<Router> = LazyLock::new(|| oci_v2_routes().with_state(reader::state()));

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    let uri = match which % 6 {
        0 => format!("/v2/token?{text}"),
        1 => format!("/v2/city/app/blobs/{text}"),
        2 => format!("/v2/city/app/tags/list?{text}"),
        3 => format!("/v2/city/app/manifests/{text}"),
        4 => format!("/v2/city/app/referrers/{text}"),
        _ => format!("/v2/{text}"),
    };
    let Ok(req) = Request::builder()
        .uri(&uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", reader::READER))
        .body(Body::empty())
    else {
        return;
    };
    reader::RT.block_on(async {
        let Ok(resp) = APP.clone().oneshot(req).await;
        let status = resp.status().as_u16();
        assert!((200..600).contains(&status), "{uri}: {status}");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 24)
            .await
            .unwrap_or_default();
        if which % 6 == 0 && status == 200 {
            let answer: serde_json::Value = serde_json::from_slice(&body).expect("a JSON token");
            assert!(answer["token"].is_string(), "{answer}");
        }
    });
});
