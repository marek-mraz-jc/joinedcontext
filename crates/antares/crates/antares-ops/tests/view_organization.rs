// SPDX-License-Identifier: EUPL-1.2
//! EP-188 on the view root: `Ops::mapped_view` answers a view of another
//! Organization's Tenant as an unknown view, and the owner's member sees it.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Ops, Surface};
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

/// `/x/views/{view}/probe`: 200 when the caller may see the view, else 404.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/views"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/probe",
            get(move |Path(view): Path<String>| {
                let ops = ops.clone();
                async move {
                    if ops.mapped_view(&view).is_some() {
                        StatusCode::OK
                    } else {
                        StatusCode::NOT_FOUND
                    }
                }
            }),
        )
    }
}

const MEMBER_OF_A: [(&str, &str); 3] = [
    ("x-jc-subject", "someone"),
    ("x-jc-member", "true"),
    ("x-jc-organization-id", "sk_a"),
];
const MEMBER_OF_B: [(&str, &str); 3] = [
    ("x-jc-subject", "someone"),
    ("x-jc-member", "true"),
    ("x-jc-organization-id", "sk_b"),
];

async fn probe(app: &Router, view: &str, caller: &[(&str, &str)]) -> Result<StatusCode, String> {
    let mut req = Request::get(format!("/x/views/{view}/probe"));
    for (k, v) in caller {
        req = req.header(*k, *v);
    }
    let req = req.body(Body::empty()).map_err(|e| e.to_string())?;
    let Ok(answer) = app.clone().oneshot(req).await;
    Ok(answer.status())
}

#[tokio::test]
async fn a_view_of_another_organizations_tenant_is_an_unknown_view() -> Result<(), String> {
    TestBroker::subject_headers("x-jc-subject,x-jc-member");
    let broker = TestBroker::enforcing_tenant_organization().await?;
    let doc = json!({"id": "vb", "tenant": "sk_b__x", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["retrieve_entity"]});
    broker.store_view_doc("vb", doc).await?;
    let app = broker.serve(Probe)?;
    assert_eq!(probe(&app, "vb", &MEMBER_OF_B).await?, StatusCode::OK);
    assert_eq!(
        probe(&app, "vb", &MEMBER_OF_A).await?,
        StatusCode::NOT_FOUND,
        "another Organization's member sees no view of sk_b"
    );
    assert_eq!(
        probe(&app, "nope", &MEMBER_OF_B).await?,
        StatusCode::NOT_FOUND,
        "an unknown view stays unknown to the owner"
    );
    Ok(())
}
