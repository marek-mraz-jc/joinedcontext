#![forbid(unsafe_code)]
//! Request identity (R9, R11) and panel execution (R3, R4, R6): who a request is comes only
//! from the host resolver or a hashed credential; a panel answers its saved targets through a
//! host-registered or stored driver, with expressions, transformations, sort, variables and the
//! named empty state of an unreadable source.

mod common;

use common::{ORG, OTHER_ORG, World, dashboard, prometheus, world};
use daiya_api::render::{
    TimeEdge, find_panel_in_slice, parse_time_str_ms, render_frames_html_table,
};
use daiya_api::{DaiyaState, PanelTimeRange, panel_view};
use daiya_model::{DaiyaStore, FieldColumn, Frame, FrameSet, OrgId, OrgKey, PanelSpec, Subject};
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
                    // Query C reads ten times A's values, so a combined result tells them apart.
                    let k = if q.ref_id == "C" { 10.0 } else { 1.0 };
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
                                    vec![Some(2.0 * k), Some(k), None],
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

/// [`daiya_api::resolve_request_ctx`], which asks the store for the organization (T-7509).
fn resolve_request_ctx(state: &DaiyaState, headers: &HeaderMap) -> daiya_model::RequestCtx {
    try_request_ctx(state, headers).expect("a request that names its Organization")
}

fn try_request_ctx(
    state: &DaiyaState,
    headers: &HeaderMap,
) -> daiya_model::CrResult<daiya_model::RequestCtx> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(daiya_api::resolve_request_ctx(state, headers))
}

#[test]
fn identity_comes_only_from_the_resolver_or_a_hashed_credential() {
    let w = world();
    // Named, so that the subject alone decides what these answer.
    let actor = |h: &HeaderMap| {
        let mut named = h.clone();
        named.insert("x-grafana-org-id", HeaderValue::from_static("1"));
        resolve_request_ctx(&w.state, &named).subject.actor_id
    };

    assert_eq!(
        actor(&headers(&[("authorization", "Bearer admin")])).as_deref(),
        Some("admin")
    );
    let ctx = resolve_request_ctx(&w.state, &headers(&[("authorization", "Bearer admin")]));
    // A daiya API credential, Bearer or Basic, never becomes a token to pass on.
    assert!(ctx.subject.bearer_token.is_none());
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
    // through the store; an unknown one is nobody's organization, never the subject's first, and
    // the router answers such a request 404 before any handler reads it.
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
    let refused = |pairs: &[(&'static str, &str)]| {
        try_request_ctx(&w.state, &headers(pairs))
            .expect_err("no Organization")
            .status_code()
    };
    assert_eq!(
        refused(&[
            ("x-grafana-org-id", "99"),
            ("authorization", "Bearer outsider")
        ]),
        404,
        "an unknown number is no fallback to the caller's own organization"
    );
    assert_eq!(
        refused(&[("x-grafana-org-id", "main")]),
        404,
        "nor to the main one"
    );
    assert_eq!(refused(&[]), 401, "anonymous, no header: no default org");

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

#[tokio::test]
async fn the_store_holds_a_name_once_and_the_main_organization_is_number_one() {
    let store = Arc::new(MemoryStore::new());
    let state = DaiyaState::for_standalone(store.clone(), ORG);
    let installation = state.installation_id;
    let create = |id| store.create_org(installation, "Edge", None, OrgId::from_u128(id));
    let edge = create(2).await.expect("created");
    assert_eq!((edge.number, edge.org_id), (2, OrgId::from_u128(2)));
    assert!(create(3).await.is_err(), "a name is held once");
    let held = state.organization(OrgKey::Name("Edge".into())).await;
    assert_eq!(held.expect("store"), Some(edge));
    let main = state.organization(OrgKey::Name("Main Org.".into())).await;
    assert_eq!(main.expect("store").map(|org| org.number), Some(1));
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

/// Built drivers are bounded per process (S9-33): ten times the Organizations hold no more
/// drivers or bytes, and a deleted datasource gives its driver back at once.
#[tokio::test]
async fn built_drivers_stay_bounded_across_ten_times_the_organizations() {
    use daiya_api::state::BUILT_DRIVERS_KEPT;
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    let record = w
        .store
        .inner
        .get_datasource(ORG, "prom")
        .await
        .unwrap()
        .unwrap();
    w.state.driver_for_record(&record).await.unwrap();
    assert_eq!(w.state.built_drivers.len(), 1);
    w.ok("DELETE", "/api/datasources/uid/prom", None).await;
    assert!(
        w.state.built_drivers.is_empty(),
        "the deleted source's driver goes"
    );

    let mut held = Vec::new();
    for orgs in [BUILT_DRIVERS_KEPT, 10 * BUILT_DRIVERS_KEPT] {
        for n in 0..orgs {
            let mut other = record.clone();
            other.org_id = OrgId::from_u128(10_000 + n as u128);
            w.state.driver_for_record(&other).await.unwrap();
        }
        held.push((w.state.built_drivers.len(), w.state.built_drivers.bytes()));
    }
    assert!(held[0].0 <= BUILT_DRIVERS_KEPT, "{held:?}");
    assert_eq!(held[0], held[1], "flat across 10x Organizations");
}

/// A dashboard page loads and authorises its dashboard once, not once per panel (S9-34).
#[tokio::test]
async fn a_twenty_panel_page_authorises_its_dashboard_once() {
    struct Counting {
        inner: daiya_model::PolicyAuthorizer,
        dashboard_reads: AtomicUsize,
    }
    impl daiya_model::Authorizer for Counting {
        fn check(
            &self,
            ctx: &daiya_model::RequestCtx,
            action: daiya_model::Action,
            object: &daiya_model::ObjectTarget,
        ) -> daiya_model::Decision {
            if matches!(object, daiya_model::ObjectTarget::Dashboard { .. }) {
                self.dashboard_reads.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.check(ctx, action, object)
        }
        fn read_scope(
            &self,
            ctx: &daiya_model::RequestCtx,
            kind: daiya_model::ReadKind,
        ) -> daiya_model::ReadScope {
            self.inner.read_scope(ctx, kind)
        }
    }
    let authz = Arc::new(Counting {
        inner: daiya_model::PolicyAuthorizer::new(common::bindings()),
        dashboard_reads: AtomicUsize::new(0),
    });
    let w = common::world_with(authz.clone(), common::loopback());
    let panels: Vec<Value> = (1..=20)
        .map(|id| {
            json!({
                "id": id, "type": "text", "title": format!("Note {id}"),
                "gridPos": { "x": 0, "y": id, "w": 12, "h": 2 },
                "options": { "mode": "markdown", "content": format!("panel {id}") }
            })
        })
        .collect();
    let mut board = dashboard("many", "prom");
    board["panels"] = Value::Array(panels);
    w.add_dashboard(board, None).await;
    authz.dashboard_reads.store(0, Ordering::SeqCst);

    let (st, page) = w.call("GET", "/d/many", Some("viewer"), None).await;
    assert_eq!(st, StatusCode::OK, "{page}");
    let page = page.as_str().unwrap();
    assert_eq!(page.matches("<section>").count(), 20);
    let order: Vec<usize> = (1..=20)
        .map(|id| page.find(&format!("<h2>Note {id}</h2>")).unwrap())
        .collect();
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "panels in dashboard order"
    );
    assert_eq!(authz.dashboard_reads.load(Ordering::SeqCst), 1);
}

#[test]
fn time_strings_parse_as_grafana_writes_them() {
    use TimeEdge::{From, To};
    let now = jc_time::TimestampTz::now().as_millis();
    let near = |v: i64, want: i64| (v - want).abs() < 5_000;
    let p = |raw: &str, edge| parse_time_str_ms(Some(raw), edge).unwrap();
    assert!(near(
        parse_time_str_ms(None, From).unwrap(),
        now - 6 * 3_600_000
    ));
    assert!(near(parse_time_str_ms(Some("  "), To).unwrap(), now));
    assert!(near(p("now", From), now));
    assert!(near(p("now-1h", From), now - 3_600_000));
    assert!(near(p("now+2m", To), now + 120_000));
    assert!(near(p("now-1w", From), now - 7 * 86_400_000));
    assert_eq!(p("1700000000000", From), 1_700_000_000_000);
    assert_eq!(p("2023-11-14T22:13:20Z", From), 1_700_000_000_000);
    // A time outside the years 0..=9999 is clamped to them, so no range arithmetic
    // downstream can overflow.
    for (wild, want) in [
        ("9223372036854775807", 253_402_300_799_999),
        ("-9223372036854775808", -62_167_219_200_000),
        ("now-99999999999999w", -62_167_219_200_000),
        ("now-99999999999999y", -62_167_219_200_000),
        ("now+99999999999999M", 253_402_300_799_999),
        ("now+99999999999999y/y", 253_402_300_799_999),
    ] {
        assert_eq!(p(wild, To), want, "{wild}");
    }
}

/// Months, years and `/unit` rounding follow Grafana's date math in UTC: `from` rounds down to
/// the unit's start, `to` up to its last millisecond, weeks start on Monday (S9-21).
#[test]
fn date_math_reads_months_years_and_rounding() {
    use TimeEdge::{From, To};
    let utc = |s: &str| jc_time::parse_rfc3339(s).unwrap().as_millisecond();
    let p = |raw: &str, edge| parse_time_str_ms(Some(raw), edge).unwrap();
    let now = jc_time::TimestampTz::now();
    let today = now.utc().date();
    let day_start = utc(&format!("{today}T00:00:00Z"));
    assert_eq!(p("now/d", From), day_start);
    assert_eq!(p("now/d", To), day_start + 86_400_000 - 1);
    assert_eq!(p("now-1d/d", From), day_start - 86_400_000);
    assert_eq!(p("now-7d/d", To), day_start - 6 * 86_400_000 - 1);
    let month_start = utc(&format!(
        "{:04}-{:02}-01T00:00:00Z",
        today.year(),
        today.month()
    ));
    assert_eq!(p("now/M", From), month_start);
    assert_eq!(p("now-1M/M", To), month_start - 1);
    assert_eq!(
        p("now/y", From),
        utc(&format!("{:04}-01-01T00:00:00Z", today.year()))
    );
    let monday = p("now/w", From);
    assert_eq!(monday % 86_400_000, 0);
    assert!(monday <= now.as_millis() && now.as_millis() - monday < 7 * 86_400_000);
    assert_eq!(
        jc_time::TimestampTz::from_millis(monday)
            .unwrap()
            .utc()
            .weekday(),
        jiff::civil::Weekday::Monday
    );
    // A month back from a month's last day lands on the shorter month's last day.
    // The parser reads its own now after this test's, so both bounds allow 5 s.
    let one_month = p("now-1M", From);
    let span = now.as_millis() - one_month;
    assert!(
        (28 * 86_400_000 - 5_000..=31 * 86_400_000 + 5_000).contains(&span),
        "{span}"
    );
    let one_year = p("now-1y", From);
    let span = now.as_millis() - one_year;
    assert!(
        (365 * 86_400_000 - 5_000..=366 * 86_400_000 + 5_000).contains(&span),
        "{span}"
    );
    assert_eq!(p("now-h/h", From) % 3_600_000, 0);
    assert_eq!(p("now/m", To) % 60_000, 59_999);
    assert_eq!(p("now/s", From) % 1_000, 0);
}

/// A time string nothing reads is a 400 that names the field; it never becomes another range
/// (S9-21).
#[test]
fn an_unreadable_time_is_a_400_naming_the_field() {
    for (raw, edge, field) in [
        ("now-banana", TimeEdge::From, "from"),
        ("yesterday", TimeEdge::To, "to"),
        ("now-1q", TimeEdge::From, "from"),
        ("now/q", TimeEdge::To, "to"),
        ("now-", TimeEdge::From, "from"),
        ("now*2d", TimeEdge::From, "from"),
        ("now-500ms", TimeEdge::From, "from"),
        ("now\u{20ac}1d", TimeEdge::From, "from"),
        ("now-1\u{20ac}", TimeEdge::To, "to"),
        ("now/\u{20ac}", TimeEdge::To, "to"),
    ] {
        let err = parse_time_str_ms(Some(raw), edge).unwrap_err();
        assert_eq!(err.status_code(), 400, "{raw}");
        let text = err.to_string();
        assert!(text.starts_with(&format!("{field}: ")), "{text}");
        assert!(text.contains(raw), "{text}");
    }
}

/// Every time range of the real exported dashboards in the import fixtures parses (S9-21).
#[test]
fn every_saved_range_of_the_fixture_dashboards_parses() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../import/tests/fixtures");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let Some(time) = doc.get("time") else {
            continue;
        };
        for (key, edge) in [("from", TimeEdge::From), ("to", TimeEdge::To)] {
            let raw = time[key].as_str().unwrap();
            assert!(
                parse_time_str_ms(Some(raw), edge).is_ok(),
                "{}: {key} {raw}",
                path.display()
            );
            seen += 1;
        }
    }
    assert!(seen >= 10, "{seen}");
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
            FieldColumn::time("t", vec![1_700_000_000_000, i64::MAX, 1_763_482_800_120]),
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
    // chrono's to_rfc3339(): 3 fraction digits for whole milliseconds, `+00:00` (T-7234).
    assert!(html.contains("2025-11-18T16:20:00.120+00:00"), "{html}");
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
            },
            { "id": 6, "type": "table", "title": "Merged",
              "transformations": [{ "id": "merge", "options": {} }],
              "targets": [{ "refId": "A", "datasource": { "uid": "fixed" } }, { "refId": "C", "datasource": { "uid": "fixed" } }] },
            { "id": 7, "type": "table", "title": "Joined",
              "transformations": [{ "id": "joinByField", "options": { "byField": "time", "mode": "outer" } }],
              "targets": [{ "refId": "A", "datasource": { "uid": "fixed" } }, { "refId": "C", "datasource": { "uid": "fixed" } }] },
            { "id": 8, "type": "table", "title": "Rows",
              "transformations": [{ "id": "seriesToRows", "options": {} }],
              "targets": [{ "refId": "A", "datasource": { "uid": "fixed" } }, { "refId": "C", "datasource": { "uid": "fixed" } }] }
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

/// `merge`, `joinByField` and `seriesToRows` combine the frames of every query of the panel, as
/// Grafana runs a panel's transformations once over all its series (S9-20).
#[tokio::test]
async fn transformations_combine_the_frames_of_all_queries() {
    let (w, _) = fixed_world().await;
    let admin = Subject::authenticated("admin", vec![ORG], vec![]);
    let view = |id| panel_view(&w.state, &admin, ORG, "fx", id, PanelTimeRange::default());

    let merged = view(6).await.unwrap();
    assert_eq!(merged.frame_sets.len(), 1, "{:?}", merged.frame_sets);
    let frames = &merged.frame_sets[0].frames;
    assert_eq!(frames.len(), 1, "{frames:?}");
    // Three rows of each query; the all-null row both share is one row, as Grafana merges it.
    assert_eq!(frames[0].row_count(), 5, "{frames:?}");

    let joined = view(7).await.unwrap();
    assert_eq!(joined.frame_sets.len(), 1, "{:?}", joined.frame_sets);
    let frame = &joined.frame_sets[0].frames[0];
    assert_eq!(frame.row_count(), 3, "{frame:?}");
    assert_eq!(
        frame.fields.len(),
        5,
        "time once, name and value per query: {frame:?}"
    );
    let values: Vec<Value> = frame
        .fields
        .iter()
        .filter_map(|f| f.values.first().cloned())
        .collect();
    assert!(
        values.contains(&json!(2.0)) && values.contains(&json!(20.0)),
        "{values:?}"
    );

    let rows = view(8).await.unwrap();
    assert_eq!(rows.frame_sets.len(), 1, "{:?}", rows.frame_sets);
    let frame = &rows.frame_sets[0].frames[0];
    // One row per time and value field of each query: 2 queries x 3 times x 2 fields.
    assert_eq!(frame.row_count(), 12, "{frame:?}");
    let values = &frame.fields[2].values;
    assert!(
        values.contains(&json!(2.0)) && values.contains(&json!(20.0)),
        "{values:?}"
    );
}

/// A saved range nothing reads falls back to the default and says so on the answer; a caller's
/// own unreadable range is a 400 naming the field (S9-21).
#[tokio::test]
async fn a_saved_unreadable_range_is_a_notice_and_a_callers_is_a_400() {
    let (w, _) = fixed_world().await;
    let mut board = fixed_board();
    board["uid"] = json!("bad-time");
    board["time"] = json!({ "from": "now-1q", "to": "now" });
    w.add_dashboard(board, None).await;
    let admin = Subject::authenticated("admin", vec![ORG], vec![]);
    let saved = PanelTimeRange {
        from: None,
        to: None,
        ..PanelTimeRange::default()
    };
    let view = panel_view(&w.state, &admin, ORG, "bad-time", 1, saved.clone())
        .await
        .unwrap();
    let notices = &view.frame_sets[0].frames[0].meta.notices;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].text.contains("now-1q"), "{notices:?}");
    // No frame at all still carries the notice.
    let empty = panel_view(&w.state, &admin, ORG, "bad-time", 3, saved)
        .await
        .unwrap();
    assert!(
        empty.frame_sets[0].frames[0].meta.notices[0]
            .text
            .contains("from: ")
    );

    let err = panel_view(
        &w.state,
        &admin,
        ORG,
        "fx",
        1,
        PanelTimeRange {
            to: Some("tomorrow-ish".into()),
            ..PanelTimeRange::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().starts_with("to: "), "{err}");
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

    fn read_scope(
        &self,
        _ctx: &daiya_model::RequestCtx,
        _kind: daiya_model::ReadKind,
    ) -> daiya_model::ReadScope {
        // "All but one" is no scope: it lists nothing.
        daiya_model::ReadScope::Nothing
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
        Some("You cannot read the source of this panel: named")
    );
    assert!(view.frame_sets.is_empty());
    assert!(view.html_table.contains("No data"));

    // The message names only the uid the dashboard already shows, never the stored name, and
    // reads the same whether or not the store holds the source.
    w.store.inner.delete_datasource(ORG, "named").await.unwrap();
    w.store.fail("get_datasource");
    let view = panel_view(&w.state, &who, ORG, "fx", 4, PanelTimeRange::default())
        .await
        .unwrap();
    w.store.heal();
    assert_eq!(
        view.access_denied_message.as_deref(),
        Some("You cannot read the source of this panel: named")
    );
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
    // A caller with no identity is refused before any target is parsed or evaluated,
    // also when the request names no datasource at all.
    let (st, body) = w
        .call(
            "POST",
            "/api/ds/query",
            None,
            Some(q(
                json!([{ "refId": "A", "datasource": { "uid": "__expr__" }, "type": "math", "expression": "1 + 1" }]),
            )),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
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
