// SPDX-License-Identifier: EUPL-1.2
//! The read path's statement budget (T-5502): how many statements one warm
//! read sends to Postgres, pinned per read shape so a read that gains a
//! round trip fails here instead of showing up weeks later in
//! pg_stat_statements (on dev the Tenant probe, the registration lookup,
//! BEGIN and set_config ran 2-24 M times each since 2026-10-21, beside
//! ~1 M of each page read). The router runs on the Postgres store as a role
//! without BYPASSRLS, so the RLS `set_config` statements are counted as in
//! production. Statements are counted from sqlx's own `sqlx::query`
//! events, as in antares-sql/tests/pg_batch_statements.rs (T-5050); this
//! file is its own test binary because the subscriber is process-wide.
//! Only statements inside the counted request's span count: the interval
//! notifier `wire` spawns polls csource_subscriptions on its own clock, and
//! under CI load its ticks landed in the counted window (runs 40222431822,
//! 40239706032).
//! A task that removes a statement lowers its number here in the same commit.
//! Skips loudly without ANTARES_TEST_DATABASE_URL.
#![cfg(feature = "postgres")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use antares_model::TenantId;
use antares_sql::sqlx;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tracing::Instrument;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// The target and name of the span around a counted request.
const COUNTED: &str = "read_statement_budget";

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// Every statement sqlx sends, as its full text (or its summary when the
/// summary is the whole statement).
#[derive(Clone, Default)]
struct Statements(Arc<Mutex<Vec<String>>>);

impl Statements {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

struct Sql(String, String);

impl tracing::field::Visit for Sql {
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        match f.name() {
            "summary" => self.0 = v.to_owned(),
            "db.statement" => self.1 = v.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => {}
        }
    }
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.record_str(f, &format!("{v:?}"));
    }
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Statements {
    fn on_event(&self, e: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let in_request = ctx
            .event_scope(e)
            .is_some_and(|mut scope| scope.any(|span| span.name() == COUNTED));
        if e.metadata().target() == "sqlx::query" && in_request {
            let mut sql = Sql(String::new(), String::new());
            e.record(&mut sql);
            let text = if sql.1.is_empty() { sql.0 } else { sql.1 };
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(text);
        }
    }
}

/// A view as the views surface hands it to the API: one rule, type Car.
struct View;

impl PolicyEngine for View {
    fn name(&self) -> &str {
        "read-statement-budget"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
            Decision::Filter(Filter {
                rules: vec![Rule {
                    types: vec![format!("{DEFAULT}Car")],
                    ..Rule::default()
                }],
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Serves one @context document on any GET, so a Link header names a
/// Cached (non-pinned) @context.
fn ctx_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let body = r#"{"@context":{"budgetTemp":"http://example.org/budgetTemp"}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ld+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            use std::io::{Read, Write};
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}/budget-ctx.jsonld")
}

/// The same non-superuser, non-BYPASSRLS role pg_rls_pentest.rs runs as,
/// set up under its advisory lock 4334 (role DDL is cluster-global).
async fn app_url(admin: &sqlx::PgPool, url: &str) -> String {
    let mut tx = admin.begin().await.expect("begin");
    for stmt in [
        "SELECT pg_advisory_xact_lock(4334)",
        "DO $$ BEGIN CREATE ROLE antares_app LOGIN PASSWORD 'app';
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$",
        "GRANT USAGE ON SCHEMA public TO antares_app",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO antares_app",
    ] {
        sqlx::query(stmt).execute(&mut *tx).await.expect(stmt);
    }
    tx.commit().await.expect("commit");
    let (scheme, rest) = url.split_once("://").expect("a postgres:// URL");
    let (_, host) = rest.rsplit_once('@').expect("test URL carries credentials");
    format!("{scheme}://antares_app:app@{host}")
}

async fn get(st: &AppState, uri: &str, tenant: &str, link: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().uri(uri).header("NGSILD-Tenant", tenant);
    if let Some(link) = link {
        req = req.header("Link", link);
    }
    let res = antares_api::router(st.clone())
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The statements of one warm read: the first call fills the caches, the
/// second is counted.
async fn counted(
    st: &AppState,
    log: &Statements,
    uri: &str,
    tenant: &str,
    link: Option<&str>,
) -> (Value, Vec<String>) {
    let (code, body) = get(st, uri, tenant, link).await;
    assert_eq!(code, StatusCode::OK, "{uri} warm-up: {body}");
    log.take();
    let (code, body) = get(st, uri, tenant, link)
        .instrument(tracing::info_span!(target: COUNTED, COUNTED))
        .await;
    assert_eq!(code, StatusCode::OK, "{uri}: {body}");
    (body, log.take())
}

#[tokio::test(flavor = "current_thread")]
async fn each_read_shape_sends_its_pinned_statements() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let log = Statements::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(
            log.clone().with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target("sqlx::query", tracing::Level::TRACE)
                    .with_target(COUNTED, tracing::Level::INFO),
            ),
        ),
    )
    .expect("the only subscriber of this binary");
    antares_jsonld::allow_private_egress(true);

    let admin = antares_sql::store::pg::connect(&url, 2)
        .await
        .expect("connect+migrate");
    let tenant = format!("rb{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let tid = TenantId::new(&tenant).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&admin, &tid)
        .await
        .expect("tenant row");
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(&tenant)
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         SELECT $1, 'urn:ngsi-ld:Car:' || lpad(n::text, 3, '0'),
                jsonb_build_object('id', 'urn:ngsi-ld:Car:' || lpad(n::text, 3, '0'),
                                   'type', jsonb_build_array($2 || 'Car'),
                                   $2 || 'speed', jsonb_build_array(
                                       jsonb_build_object('type', 'Property', 'value', n))),
                ARRAY[$2 || 'Car'], now(), now()
           FROM generate_series(1, 50) AS n",
    )
    .bind(&tenant)
    .bind(DEFAULT)
    .execute(&mut *tx)
    .await
    .expect("entities");
    tx.commit().await.expect("commit");

    // migrations ran as the admin above; the app role may not run DDL
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url(&admin, &url).await)
        .await
        .expect("app-role connect");
    assert!(
        !antares_sql::store::pg::role_bypasses_rls(&app).await,
        "the app role must not bypass RLS, or set_config is not what production runs"
    );
    let store = Arc::new(antares_sql::store::any::AnyStore::Pg(
        antares_sql::store::any::PgBackend::new(app.clone()),
    ));
    let mut st = AppState::with_store("me".into(), store.clone(), "postgres");
    antares_api::wire(&mut st).await;
    let mut viewed =
        AppState::with_store("me".into(), store, "postgres").with_policy(Arc::new(View));
    antares_api::wire(&mut viewed).await;

    let page = "/ngsi-ld/v1/entities?type=Car&limit=20";
    let one = "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:007";
    let link = format!(
        "<{}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\"",
        ctx_server()
    );

    // Counted per read (sqlx logs no event for BEGIN, so each transaction
    // shows as set_config ... COMMIT; on dev BEGIN ran as often as
    // set_config):
    // (a) a page by type: the Tenant's set_config, the page SELECT, COMMIT.
    // tenant_exists_layer's probe (`SELECT 1 FROM tenants`) ran on the
    // warm-up only: the router remembers a Tenant the store said exists
    // (T-5500), so no warm read in (a) to (d) probes it; no registration
    // lookup reaches Postgres on a warm read.
    let (body, a) = counted(&st, &log, page, &tenant, None).await;
    assert_eq!(body.as_array().map(Vec::len), Some(20), "{body}");
    assert_eq!(a.len(), 3, "statements: {a:#?}");
    assert!(
        !a.iter().any(|q| q.contains("FROM tenants")),
        "a warm read probed the Tenant: {a:#?}"
    );

    // (b) the same page through a view: the view adds no statement.
    let (body, b) = counted(&viewed, &log, page, &tenant, None).await;
    assert_eq!(body.as_array().map(Vec::len), Some(20), "{body}");
    assert_eq!(b.len(), 3, "statements: {b:#?}");
    assert_eq!(a, b, "a view must not change what a page reads");

    // (c) one Entity by id: set_config, the SELECT by id, COMMIT.
    let (body, c) = counted(&st, &log, one, &tenant, None).await;
    assert_eq!(body["id"], "urn:ngsi-ld:Car:007", "{body}");
    assert_eq!(c.len(), 3, "statements: {c:#?}");

    // (d) the page with a Link to the Cached @context the warm-up fetched:
    // one transaction of the @context write-through the hit still runs
    // (state.rs `set_cache_writer`: `context_mutate`'s set_config, SELECT
    // ... FOR UPDATE, UPDATE, COMMIT), then the page's own three. A warm hit
    // should write nothing; pinned here as today's cost, the change that
    // stops the rewrite lowers both numbers.
    let (body, d) = counted(&st, &log, page, &tenant, Some(&link)).await;
    assert_eq!(body.as_array().map(Vec::len), Some(20), "{body}");
    assert_eq!(d.len(), 7, "statements: {d:#?}");
    let context_writes: Vec<&String> = d
        .iter()
        .filter(|q| q.contains("jsonld_contexts") && !q.starts_with("SELECT"))
        .collect();
    assert_eq!(
        context_writes.len(),
        1,
        "@context writes: {context_writes:#?}"
    );

    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&admin)
        .await
        .expect("clean");
}
