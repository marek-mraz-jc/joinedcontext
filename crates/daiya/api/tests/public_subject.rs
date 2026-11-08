#![forbid(unsafe_code)]
//! Public-dashboard queries run as a dedicated least-privilege subject (R7): they work under a
//! host authorizer that knows nothing of them, and that subject can neither write nor read
//! anything beyond its one dashboard and the datasources its saved panel names.

use axum::Router;
use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{
    Action, Authorizer, GrafanaRole, InstallationId, InstanceId, ObjectTarget, OrgId,
    PolicyAuthorizer, PublicDashboardAuthorizer, RequestCtx, RoleBinding, Subject,
};
use daiya_query::OutboundPolicy;
use daiya_store_memory::MemoryStore;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(77);
const OTHER_ORG: OrgId = OrgId::from_u128(78);

async fn prometheus_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let body = json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                { "metric": { "job": "web" }, "values": [[1700000000, "5"]] }
            ]}})
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn dashboard(uid: &str, ds_uid: &str) -> Value {
    json!({
        "dashboard": {
            "uid": uid,
            "title": format!("Dash {uid}"),
            "schemaVersion": 42,
            "time": { "from": "now-1h", "to": "now" },
            "panels": [{
                "id": 1, "type": "timeseries", "title": "Web",
                "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                "datasource": { "type": "prometheus", "uid": ds_uid },
                "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": ds_uid } }]
            }]
        },
        "overwrite": true
    })
}

#[tokio::test]
async fn public_queries_work_under_a_host_authorizer_with_no_internal_admin() {
    let url = prometheus_mock().await;
    // The host knows one owner. There is no binding for any internal "admin" principal.
    let authz = Arc::new(PolicyAuthorizer::new(vec![RoleBinding::from_grafana_role(
        ORG,
        "owner",
        GrafanaRole::Admin,
        None,
    )]));
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Arc::new(MemoryStore::new()),
        authz.clone(),
    )
    .with_tokens(vec![TokenBinding::new(
        "owner-token",
        Subject::authenticated("owner", vec![ORG], vec![]),
    )]);
    state.outbound_policy = OutboundPolicy {
        allow_http: true,
        allowed_networks: vec!["127.0.0.0/8".parse().unwrap()],
        ..OutboundPolicy::default()
    };
    let app = api_router(
        state,
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );

    let (st, body) = call(
        &app,
        "POST",
        "/api/datasources",
        Some("owner-token"),
        json!({ "uid": "prom", "name": "Prom", "type": "prometheus", "url": url }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    for uid in ["pub", "private"] {
        let (st, body) = call(
            &app,
            "POST",
            "/api/dashboards/db",
            Some("owner-token"),
            dashboard(uid, "prom"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
    let (st, created) = call(
        &app,
        "POST",
        "/api/dashboards/uid/pub/public-dashboards",
        Some("owner-token"),
        json!({ "isEnabled": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let token = created["accessToken"].as_str().unwrap().to_owned();

    let (st, answer) = call(
        &app,
        "POST",
        &format!("/api/public/dashboards/{token}/panels/1/query"),
        None,
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{answer}");
    assert!(answer.to_string().contains('5'), "{answer}");
    assert!(
        !answer.to_string().contains("prom\""),
        "no datasource uid in the answer: {answer}"
    );

    // The host authorizer grants the public subject nothing at all.
    let subject = Subject::public_dashboard(ORG, "pub");
    let ctx = RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        subject,
    );
    for (action, object) in [
        (
            Action::Read,
            ObjectTarget::Dashboard {
                org_id: ORG,
                dashboard_uid: "pub".into(),
                folder_uid: None,
            },
        ),
        (
            Action::Read,
            ObjectTarget::Datasource {
                org_id: ORG,
                datasource_uid: "prom".into(),
            },
        ),
        (Action::Propose, ObjectTarget::Organization { org_id: ORG }),
    ] {
        assert!(
            !authz.check(&ctx, action, &object).allowed,
            "{action:?} {object:?}"
        );
    }
}

#[test]
fn the_public_subject_reads_only_its_dashboard_and_its_datasources() {
    let subject = Subject::public_dashboard(ORG, "pub");
    assert!(subject.read_only);
    assert!(subject.bearer_token.is_none());
    assert!(subject.groups.is_empty());
    assert_eq!(subject.organizations, vec![ORG]);
    assert_eq!(subject.actor_id.as_deref(), Some("public-dashboard:pub"));

    let authz = PublicDashboardAuthorizer::new(ORG, "pub", ["prom".to_owned()]);
    let ctx = RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        subject,
    );
    let dash = |uid: &str| ObjectTarget::Dashboard {
        org_id: ORG,
        dashboard_uid: uid.into(),
        folder_uid: None,
    };
    let ds = |org: OrgId, uid: &str| ObjectTarget::Datasource {
        org_id: org,
        datasource_uid: uid.into(),
    };

    // Allowed: read its dashboard, its public record and its saved datasource.
    assert!(authz.check(&ctx, Action::Read, &dash("pub")).allowed);
    assert!(
        authz
            .check(
                &ctx,
                Action::Read,
                &ObjectTarget::PublicDashboard {
                    org_id: ORG,
                    dashboard_uid: "pub".into()
                }
            )
            .allowed
    );
    assert!(authz.check(&ctx, Action::Read, &ds(ORG, "prom")).allowed);

    // Denied, always as a hidden 404: any write, another dashboard, another datasource,
    // another organization, folders, library panels, the organization itself.
    let denied = [
        (Action::Propose, dash("pub")),
        (Action::Approve, dash("pub")),
        (Action::Delete, dash("pub")),
        (Action::Admin, ds(ORG, "prom")),
        (Action::Read, dash("private")),
        (Action::Read, ds(ORG, "secret-ds")),
        (Action::Read, ds(OTHER_ORG, "prom")),
        (
            Action::Read,
            ObjectTarget::Folder {
                org_id: ORG,
                folder_uid: "f".into(),
            },
        ),
        (
            Action::Read,
            ObjectTarget::LibraryPanel {
                org_id: ORG,
                panel_uid: "l".into(),
                folder_uid: None,
            },
        ),
        (Action::Read, ObjectTarget::Organization { org_id: ORG }),
    ];
    for (action, object) in denied {
        let d = authz.check(&ctx, action, &object);
        assert!(!d.allowed && !d.visible, "{action:?} {object:?}");
    }

    // The scoped authorizer serves only the subject of its own dashboard, in its organization.
    let other_subject = RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Subject::public_dashboard(ORG, "private"),
    );
    assert!(
        !authz
            .check(&other_subject, Action::Read, &dash("pub"))
            .allowed
    );
    let human = RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Subject::authenticated("admin", vec![ORG], vec![]),
    );
    assert!(!authz.check(&human, Action::Read, &ds(ORG, "prom")).allowed);
    let wrong_org = RequestCtx::new(
        InstallationId::from_u128(1),
        OTHER_ORG,
        InstanceId::from_u128(1),
        Subject::public_dashboard(OTHER_ORG, "pub"),
    );
    assert!(
        !authz
            .check(&wrong_org, Action::Read, &ds(OTHER_ORG, "prom"))
            .allowed
    );
}
