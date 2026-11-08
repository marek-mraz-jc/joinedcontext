#![forbid(unsafe_code)]
//! Request identity (R9, R11) and panel execution (R3, R4, R6): who a request is comes only
//! from the host resolver or a hashed credential; a panel answers its saved targets through a
//! host-registered or stored driver, with expressions, transformations, sort, variables and the
//! named empty state of an unreadable source.

mod common;

use common::{ORG, OTHER_ORG, World, dashboard, prometheus, world};
use daiya_api::render::{find_panel_in_slice, parse_time_str_ms, render_frames_html_table};
use daiya_api::state::OrgDirectory;
use daiya_api::{DaiyaState, PanelTimeRange, panel_view, resolve_request_ctx};
use daiya_model::{FieldColumn, Frame, FrameSet, OrgId, PanelSpec, Subject};
use daiya_query::{
    Datasource, DsError, DsFuture, EditorSchema, Health, ImportOutcome, Query, QueryCtx,
    ResourceReq, ResourceResp,
};
use daiya_store_memory::MemoryStore;
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A driver answering every query with a time, a name and a value column (rows `b`, `a`, null).
#[derive(Default)]
struct Fixed {
    calls: AtomicUsize,
}

impl Datasource for Fixed {
    fn kind(&self) -> &'static str {
        "fixed"
    }
    fn query<'a>(
        &'a self,
        _ctx: &'a QueryCtx,
        q: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(q.iter()
                .map(|q| {
                    FrameSet::new(
                        q.ref_id.clone(),
                        vec![Frame {
                            name: "fixed".into(),
                            fields: vec![
                                FieldColumn::time(
                                    "time",
                                    vec![1_700_000_000_000, 1_700_000_060_000, 1_700_000_120_000],
                                ),
                                FieldColumn::strings(
                                    "name",
                                    vec![Some("b<".into()), Some("a".into()), None],
                                ),
                                FieldColumn::numbers(
                                    "value",
                                    BTreeMap::new(),
                                    vec![Some(2.0), Some(1.0), None],
                                ),
                            ],
                            ..Frame::default()
                        }],
                    )
                })
                .collect())
        })
    }
    fn health<'a>(&'a self, _ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async { Health::ok("fixed") })
    }
    fn resource<'a>(
        &'a self,
        _ctx: &'a QueryCtx,
        r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            Ok(ResourceResp {
                data: json!({ "path": r.path }),
            })
        })
    }
    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "fixed".into(),
            json_schema: json!({ "type": "object" }),
            field_hints: vec![],
        }
    }
    fn import(&self, t: &Value) -> Result<ImportOutcome, DsError> {
        Ok(ImportOutcome {
            query: Query {
                ref_id: "A".into(),
                hide: false,
                model: t.clone(),
            },
            warnings: vec![],
        })
    }
    fn export(&self, q: &Query) -> Value {
        q.model.clone()
    }
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(*k, HeaderValue::from_str(v).unwrap());
    }
    h
}

#[test]
fn identity_comes_only_from_the_resolver_or_a_hashed_credential() {
    let w = world();
    let actor = |h: &HeaderMap| resolve_request_ctx(&w.state, h).subject.actor_id;

    assert_eq!(
        actor(&headers(&[("authorization", "Bearer admin")])).as_deref(),
        Some("admin")
    );
    let ctx = resolve_request_ctx(&w.state, &headers(&[("authorization", "Bearer admin")]));
    assert_eq!(ctx.subject.bearer_token.as_deref(), Some("admin"));
    // Basic carries the credential but never becomes a token to pass on.
    let basic = resolve_request_ctx(&w.state, &headers(&[("authorization", "basic viewer")]));
    assert_eq!(basic.subject.actor_id.as_deref(), Some("viewer"));
    assert!(basic.subject.bearer_token.is_none());
    for refused in [
        "Bearer",
        "Bearer    ",
        "Token admin",
        "Bearer nope",
        "admin",
    ] {
        assert_eq!(
            actor(&headers(&[("authorization", refused)])),
            None,
            "{refused}"
        );
    }
    let mut opaque = HeaderMap::new();
    opaque.insert(
        "authorization",
        HeaderValue::from_bytes(b"Bearer \xff").unwrap(),
    );
    assert_eq!(actor(&opaque), None);
    assert_eq!(actor(&HeaderMap::new()), None);

    // Org headers choose among the subject's organizations; a numeric Grafana org id maps
    // through the directory; an unknown one falls back to the subject's first organization.
    let pick =
        |pairs: &[(&'static str, &str)]| resolve_request_ctx(&w.state, &headers(pairs)).org_id;
    assert_eq!(pick(&[("authorization", "Bearer admin")]), ORG);
    assert_eq!(
        pick(&[("x-daiya-org-id", &OTHER_ORG.to_string())]),
        OTHER_ORG
    );
    assert_eq!(
        pick(&[("x-grafana-org-id", &OTHER_ORG.to_string())]),
        OTHER_ORG
    );
    assert_eq!(pick(&[("x-grafana-org-id", "1")]), ORG);
    assert_eq!(
        pick(&[
            ("x-grafana-org-id", "99"),
            ("authorization", "Bearer outsider")
        ]),
        OTHER_ORG
    );
    assert_eq!(
        pick(&[("x-grafana-org-id", "main")]),
        ORG,
        "anonymous: default org"
    );

    // The read-only header only narrows.
    for v in ["true", "1"] {
        let ro = resolve_request_ctx(
            &w.state,
            &headers(&[("authorization", "Bearer admin"), ("x-daiya-readonly", v)]),
        );
        assert!(ro.subject.read_only, "{v}");
    }
    let rw = resolve_request_ctx(
        &w.state,
        &headers(&[
            ("authorization", "Bearer admin"),
            ("x-daiya-readonly", "false"),
        ]),
    );
    assert!(!rw.subject.read_only);

    // The host resolver wins over credentials; when it declines, credentials count.
    let state = w.state.clone().with_subject_resolver(|h: &HeaderMap| {
        h.get("x-host-user")
            .map(|_| Subject::authenticated("host-person", vec![ORG], vec![]))
    });
    let via_host = resolve_request_ctx(
        &state,
        &headers(&[("x-host-user", "1"), ("authorization", "Bearer admin")]),
    );
    assert_eq!(via_host.subject.actor_id.as_deref(), Some("host-person"));
    let via_token = resolve_request_ctx(&state, &headers(&[("authorization", "Bearer admin")]));
    assert_eq!(via_token.subject.actor_id.as_deref(), Some("admin"));
}

#[tokio::test]
async fn a_read_only_header_refuses_writes() {
    let w = world();
    let (st, _) = w
        .send(
            http::Request::builder()
                .method("POST")
                .uri("/api/folders")
                .header("authorization", "Bearer admin")
                .header("x-daiya-readonly", "true")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(json!({ "title": "x" }).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[test]
fn the_org_directory_registers_a_name_once() {
    let main = OrgId::from_u128(1);
    let mut dir = OrgDirectory::with_main_org(main);
    let (num, id, created) = dir.register_org("Edge", OrgId::from_u128(2));
    assert_eq!((num, created), (2, true));
    let (again, same, created) = dir.register_org("Edge", OrgId::from_u128(3));
    assert_eq!((again, same, created), (2, id, false));
    assert_eq!(dir.register_org("Main Org.", OrgId::from_u128(4)).0, 1);
}

#[tokio::test]
async fn a_standalone_state_admits_its_principal() {
    let state = DaiyaState::for_standalone(Arc::new(MemoryStore::new()), ORG);
    let ctx = daiya_model::RequestCtx::new(
        state.installation_id,
        ORG,
        state.default_instance_id,
        Subject::authenticated("admin", vec![ORG], vec![]),
    );
    let dec = state.authorizer.check(
        &ctx,
        daiya_model::Action::Approve,
        &daiya_model::ObjectTarget::Organization { org_id: ORG },
    );
    assert!(dec.allowed);
}

/// The host's driver serves a uid with or without a stored record, and stored drivers are
/// rebuilt only when the record changes.
#[tokio::test]
async fn registered_drivers_come_first_and_built_ones_are_cached_per_revision() {
    let w = world();
    let fixed = Arc::new(Fixed::default());
    w.state
        .register_datasource_driver(ORG, "fixed", fixed.clone());
    assert!(w.state.get_datasource_driver(ORG, "fixed").is_some());
    assert!(w.state.driver_for(ORG, "fixed").await.unwrap().is_some());
    assert!(w.state.driver_for(ORG, "absent").await.unwrap().is_none());
    let health = w.ok("GET", "/api/datasources/uid/fixed/health", None).await;
    assert_eq!(health["status"], "ok", "{health}");
    let editor = w
        .ok("GET", "/odata/v4/Datasources('fixed')/EditorSchema", None)
        .await;
    assert_eq!(editor["kind"], "fixed");

    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    use daiya_model::DaiyaStore;
    let record = w
        .store
        .inner
        .get_datasource(ORG, "prom")
        .await
        .unwrap()
        .unwrap();
    let a = w.state.driver_for_record(&record).await.unwrap();
    let b = w.state.driver_for_record(&record).await.unwrap();
    assert!(Arc::ptr_eq(&a, &b), "same revision, same driver");
    // A host driver registered under a stored uid takes over its record too.
    w.state
        .register_datasource_driver(ORG, "prom", fixed.clone());
    let c = w.state.driver_for_record(&record).await.unwrap();
    assert_eq!(c.kind(), "fixed");
}

#[test]
fn time_strings_parse_as_grafana_writes_them() {
    let now = chrono::Utc::now().timestamp_millis();
    let near = |v: i64, want: i64| (v - want).abs() < 5_000;
    assert!(near(parse_time_str_ms(None, -1000), now - 1000));
    assert!(near(parse_time_str_ms(Some("  "), 0), now));
    assert!(near(parse_time_str_ms(Some("now"), -1), now));
    assert!(near(parse_time_str_ms(Some("now-1h"), 0), now - 3_600_000));
    assert!(near(
        parse_time_str_ms(Some("now-1d/d"), 0),
        now - 86_400_000
    ));
    assert_eq!(
        parse_time_str_ms(Some("1700000000000"), 0),
        1_700_000_000_000
    );
    assert_eq!(
        parse_time_str_ms(Some("2023-11-14T22:13:20Z"), 0),
        1_700_000_000_000
    );
    // Unparseable: the default offset, also after a bad relative duration.
    assert!(near(
        parse_time_str_ms(Some("now-banana"), -7_000),
        now - 7_000
    ));
    assert!(near(
        parse_time_str_ms(Some("yesterday"), -7_000),
        now - 7_000
    ));
}

#[test]
fn the_html_table_escapes_every_cell_and_names_an_empty_answer() {
    let empty = render_frames_html_table("<T>", &[], 10);
    assert!(
        empty.contains("No data") && empty.contains("&lt;T&gt;"),
        "{empty}"
    );
    let no_fields = render_frames_html_table("t", &[Frame::default()], 10);
    assert!(no_fields.contains("No data"));
    let frame = Frame {
        fields: vec![
            FieldColumn::time("t", vec![1_700_000_000_000, i64::MAX]),
            FieldColumn::strings("s", vec![Some("<b>".into()), None]),
            FieldColumn::numbers("n", BTreeMap::new(), vec![Some(1.5), None]),
            FieldColumn {
                values: vec![json!(true)],
                ..FieldColumn::strings("o", vec![])
            },
        ],
        ..Frame::default()
    };
    let html = render_frames_html_table("t", std::slice::from_ref(&frame), 10);
    assert!(html.contains("2023-11-14T22:13:20+00:00"), "{html}");
    assert!(html.contains(&i64::MAX.to_string()), "{html}");
    assert!(
        html.contains("&lt;b&gt;") && !html.contains("<b>"),
        "{html}"
    );
    assert!(
        html.contains("1.5") && html.contains("–") && html.contains("true"),
        "{html}"
    );
    let one_row = render_frames_html_table("t", &[frame], 1);
    assert_eq!(one_row.matches("<tr>").count(), 2, "header + 1 row");
}

#[test]
fn panels_are_found_inside_rows() {
    let panels: Vec<PanelSpec> = serde_json::from_value(json!([
        { "id": 1, "type": "row", "title": "Row", "panels": [
            { "id": 4, "type": "stat", "title": "Inside" }
        ]},
        { "id": 2, "type": "stat", "title": "Top" }
    ]))
    .unwrap();
    assert_eq!(
        find_panel_in_slice(&panels, 4).unwrap().title.as_deref(),
        Some("Inside")
    );
    assert_eq!(
        find_panel_in_slice(&panels, 2).unwrap().title.as_deref(),
        Some("Top")
    );
    assert!(
        find_panel_in_slice(&panels, 1).is_none(),
        "a row is no panel"
    );
    assert!(find_panel_in_slice(&panels, 9).is_none());
}

/// A dashboard whose panels read the host's `fixed` driver.
fn fixed_board() -> Value {
    json!({
        "uid": "fx", "title": "Fixed", "schemaVersion": 42,
        "time": { "from": "1700000000000", "to": "1700000600000" },
        "templating": { "list": [
            { "name": "all", "type": "custom", "current": { "value": "$__all" }, "allValue": ".*" },
            { "name": "one", "type": "custom", "current": { "value": "x" } },
            { "name": "unset", "type": "custom", "allValue": "*" }
        ]},
        "panels": [
            {
                "id": 1, "type": "table", "title": "Sorted",
                "datasource": { "uid": "fixed" },
                "fieldConfig": { "defaults": { "unit": "short" }, "overrides": [] },
                "options": { "sortBy": [{ "displayName": "value", "desc": false }] },
                "targets": [{ "refId": "A", "datasource": { "uid": "fixed" } }]
            },
            {
                "id": 2, "type": "timeseries",
                "datasource": { "uid": "fixed" },
                "transformations": [{ "id": "organize", "options": { "excludeByName": { "name": true } } }],
                "targets": [
                    { "refId": "A", "datasource": { "uid": "fixed" }, "hide": true },
                    { "refId": "B", "datasource": { "type": "__expr__", "uid": "__expr__" }, "type": "math", "expression": "$A * 10" }
                ]
            },
            { "id": 3, "type": "stat", "title": "No targets" },
            {
                "id": 4, "type": "stat", "title": "Secret source",
                "targets": [{ "refId": "A", "datasource": { "uid": "secret-ds" } }]
            },
            {
                "id": 5, "type": "table", "title": "No sort column",
                "options": { "sortBy": [{ "displayName": "", "desc": true }] },
                "targets": [{ "refId": "A", "datasource": { "uid": "fixed" } }]
            }
        ]
    })
}

async fn fixed_world() -> (World, Arc<Fixed>) {
    let w = world();
    let fixed = Arc::new(Fixed::default());
    w.state
        .register_datasource_driver(ORG, "fixed", fixed.clone());
    w.add_dashboard(fixed_board(), None).await;
    (w, fixed)
}

#[tokio::test]
async fn panel_view_runs_targets_expressions_transformations_and_sort() {
    let (w, fixed) = fixed_world().await;
    let admin = Subject::authenticated("admin", vec![ORG], vec![]);
    let range = PanelTimeRange {
        variables: BTreeMap::from([("one".to_owned(), "y".to_owned())]),
        ..PanelTimeRange::default()
    };
    let sorted = panel_view(&w.state, &admin, ORG, "fx", 1, range.clone())
        .await
        .unwrap();
    assert_eq!(sorted.panel_title, "Sorted");
    assert_eq!(sorted.field_config["defaults"]["unit"], "short");
    let values = &sorted.frame_sets[0].frames[0].fields[2].values;
    assert_eq!(values[0], Value::Null, "nulls sort first: {values:?}");
    assert_eq!(values[1], json!(1.0));
    assert!(sorted.html_table.contains("b&lt;"));
    // The same call again is answered by the query cache.
    let calls = fixed.calls.load(Ordering::SeqCst);
    panel_view(&w.state, &admin, ORG, "fx", 1, range)
        .await
        .unwrap();
    assert_eq!(fixed.calls.load(Ordering::SeqCst), calls);

    let expr = panel_view(&w.state, &admin, ORG, "fx", 2, PanelTimeRange::default())
        .await
        .unwrap();
    assert_eq!(expr.panel_title, "Panel 2");
    let refs: Vec<&str> = expr.frame_sets.iter().map(|f| f.ref_id.as_str()).collect();
    assert_eq!(refs, ["B"], "the hidden source feeds the expression only");

    let empty = panel_view(&w.state, &admin, ORG, "fx", 3, PanelTimeRange::default())
        .await
        .unwrap();
    assert!(empty.frame_sets.is_empty());
    let unsorted = panel_view(&w.state, &admin, ORG, "fx", 5, PanelTimeRange::default())
        .await
        .unwrap();
    assert_eq!(
        unsorted.frame_sets[0].frames[0].fields[2].values[0],
        json!(2.0)
    );

    // A source that exists nowhere is a 404.
    let err = panel_view(&w.state, &admin, ORG, "fx", 4, PanelTimeRange::default())
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

/// Allows every member everything except reading the datasource `named`.
struct HideNamed;

impl daiya_model::Authorizer for HideNamed {
    fn check(
        &self,
        ctx: &daiya_model::RequestCtx,
        _action: daiya_model::Action,
        object: &daiya_model::ObjectTarget,
    ) -> daiya_model::Decision {
        match object {
            daiya_model::ObjectTarget::Datasource { datasource_uid, .. }
                if datasource_uid == "named" =>
            {
                daiya_model::Decision::deny_hidden("not this source")
            }
            _ if ctx.subject.is_member_of(object.org_id()) => {
                daiya_model::Decision::allow("member")
            }
            _ => daiya_model::Decision::deny_hidden("outside"),
        }
    }
}

#[tokio::test]
async fn an_unreadable_panel_source_is_a_named_empty_state() {
    let w = common::world_with(Arc::new(HideNamed), common::loopback());
    w.add_prometheus("named", "https://prom.example").await;
    let mut board = fixed_board();
    board["panels"][3]["targets"][0]["datasource"]["uid"] = json!("named");
    board["panels"][4]["targets"][0]["datasource"]["uid"] = json!("named-gone");
    w.add_dashboard(board, None).await;
    use daiya_model::DaiyaStore;
    let who = Subject::authenticated("anyone", vec![ORG], vec![]);
    let view = panel_view(&w.state, &who, ORG, "fx", 4, PanelTimeRange::default())
        .await
        .unwrap();
    assert_eq!(
        view.access_denied_message.as_deref(),
        Some("You cannot read the source of this panel: Prom named")
    );
    assert!(view.frame_sets.is_empty());
    assert!(view.html_table.contains("No data"));

    // A source the caller cannot read and the store does not hold is named by its uid.
    w.store.inner.delete_datasource(ORG, "named").await.unwrap();
    let view = panel_view(&w.state, &who, ORG, "fx", 4, PanelTimeRange::default())
        .await
        .unwrap();
    assert_eq!(
        view.access_denied_message.as_deref(),
        Some("You cannot read the source of this panel: named")
    );
    w.store.fail("get_datasource");
    let failed = panel_view(&w.state, &who, ORG, "fx", 4, PanelTimeRange::default()).await;
    w.store.heal();
    assert_eq!(failed.unwrap_err().status_code(), 500);
}

#[tokio::test]
async fn query_requests_are_bounded_and_named() {
    let (w, _) = fixed_world().await;
    let q = |queries: Value| json!({ "from": "1700000000000", "to": "1700000600000", "queries": queries });
    let too_many: Vec<Value> = (0..=w.state.limits.max_targets_per_panel)
        .map(|i| json!({ "refId": format!("R{i}"), "datasource": { "uid": "fixed" } }))
        .collect();
    for (queries, want) in [
        (json!(too_many), StatusCode::BAD_REQUEST),
        (
            json!([{ "refId": "A", "datasource": "fixed" }, { "refId": "A", "datasource": "fixed" }]),
            StatusCode::BAD_REQUEST,
        ),
        (json!([{ "refId": "A" }]), StatusCode::BAD_REQUEST),
        (
            json!([{ "refId": "A", "datasource": { "uid": "-- Mixed --" } }]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{ "refId": "A", "datasource": { "uid": "__expr__" }, "type": "nonsense" }]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{ "refId": "A", "datasource": { "uid": "fixed" }, "hide": true }]),
            StatusCode::OK,
        ),
    ] {
        let (st, body) = w
            .call(
                "POST",
                "/api/ds/query",
                Some("admin"),
                Some(q(queries.clone())),
            )
            .await;
        assert_eq!(st, want, "{queries}: {body}");
    }
    // A hidden source with no expression is not run and not answered.
    let (_, body) = w
        .call(
            "POST",
            "/api/ds/query",
            Some("admin"),
            Some(q(
                json!([{ "refId": "A", "datasource": { "uid": "fixed" }, "hide": true }]),
            )),
        )
        .await;
    assert_eq!(body["results"], json!({}), "{body}");
    // A source in another organization is the same 404 as an unknown one.
    let (st, _) = w
        .call(
            "POST",
            "/api/ds/query",
            Some("outsider"),
            Some(q(
                json!([{ "refId": "A", "datasource": { "uid": "fixed" } }]),
            )),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_stored_dashboard_that_does_not_parse_is_an_internal_error() {
    let w = world();
    w.add_dashboard(dashboard("ok", "prom"), None).await;
    use daiya_model::DaiyaStore;
    let mut rec = w
        .store
        .inner
        .get_dashboard(ORG, "ok")
        .await
        .unwrap()
        .unwrap();
    rec.data = json!({ "panels": "not a list" });
    w.store
        .inner
        .upsert_dashboard(rec, "corrupt", true)
        .await
        .unwrap();
    let (st, body) = w
        .call(
            "GET",
            "/api/dashboards/ok/render?panel=1",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}
