// SPDX-License-Identifier: EUPL-1.2
//! T-5159: a fan-out to in-process sinks books in shared statements.
//!
//! One change matching N WebSocket-like subscriptions used to write N
//! bookkeeping transactions; the deliveries now join their tenant's next
//! `UPDATE ... FROM unnest`, so the statement count follows the rounds the
//! store needed, not N. Each subscription's `timesSent` still counts its
//! notification. The statements are counted off sqlx's own `sqlx::query`
//! events, so the count is what reached Postgres.
//!
//! Skips without ANTARES_TEST_DATABASE_URL.
#![cfg(feature = "postgres")]

use antares_api::AppState;
use antares_model::TenantId;
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_store::Kind;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;

/// Subscriptions of the fan-out.
const N: usize = 50;

/// A binding that opens no socket, like the WebSocket one, and counts what
/// it is handed.
struct InProcess(Arc<AtomicUsize>);

impl antares_notifier::NotificationSink for InProcess {
    fn schemes(&self) -> &'static [&'static str] {
        &["x-inproc"]
    }
    fn parse_endpoint(
        &self,
        _uri: &str,
        _notifier_info: &[(&str, &str)],
    ) -> Result<(), antares_model::NgsiError> {
        Ok(())
    }
    fn network(&self) -> bool {
        false
    }
    fn deliver<'a>(
        &'a self,
        _uri: &'a str,
        _out: &'a antares_notifier::Outbound,
        _timeout: std::time::Duration,
    ) -> antares_notifier::DeliveryFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

/// Counts the statements sqlx sent that book a delivery.
#[derive(Clone, Default)]
struct Bookings(Arc<AtomicUsize>);

struct Sql(String);

impl tracing::field::Visit for Sql {
    fn record_debug(&mut self, _field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{value:?}"));
    }
    fn record_str(&mut self, _field: &tracing::field::Field, value: &str) {
        self.0.push_str(value);
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Bookings {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut sql = Sql(String::new());
        event.record(&mut sql);
        if sql.0.contains("UPDATE subscriptions") && sql.0.contains("timesSent") {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// One change, N in-process subscriptions: every one is notified and booked
/// once, in a handful of statements instead of N.
///
/// A current-thread runtime keeps every spawned delivery on the thread the
/// counting subscriber is installed on.
#[tokio::test]
async fn an_in_process_fan_out_books_in_shared_statements() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let bookings = Bookings::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(bookings.clone()));
    let pool = antares_sql::store::pg::connect(&url, 10)
        .await
        .expect("connect");
    let name = format!("fanout{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let tenant = TenantId::new(&name).expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tenant)
        .await
        .expect("tenant row");
    let delivered = Arc::new(AtomicUsize::new(0));
    let st = AppState::with_store(
        "antares1".into(),
        Arc::new(AnyStore::Pg(PgBackend::new(pool))),
        "postgres",
    )
    .with_sink(Box::new(InProcess(delivered.clone())));
    let vehicle = "https://uri.etsi.org/ngsi-ld/default-context/Vehicle";
    for i in 0..N {
        let id = format!("urn:ngsi-ld:Subscription:fanout-{i}");
        st.store
            .create(
                &tenant,
                Kind::Subscription,
                &id,
                json!({
                    "id": id,
                    "type": "Subscription",
                    "status": "active",
                    "createdAt": "2020-01-01T00:00:00Z",
                    "entities": [{"type": vehicle}],
                    "notification": {"endpoint": {"uri": format!("x-inproc:{i}")}},
                }),
            )
            .await
            .expect("seed subscription");
    }
    let entity = json!({
        "id": "urn:ngsi-ld:Vehicle:fanout",
        "type": [vehicle],
        "https://uri.etsi.org/ngsi-ld/default-context/speed": [{"type": "Property", "value": 1}],
    });
    bookings.0.store(0, Ordering::SeqCst);
    antares_api::notify::process_change(&st, tenant.as_str(), None, Some(entity)).await;

    assert_eq!(
        delivered.load(Ordering::SeqCst),
        N,
        "every subscription is notified"
    );
    let statements = bookings.0.load(Ordering::SeqCst);
    assert!(
        (1..=3).contains(&statements),
        "{N} in-process deliveries took {statements} bookkeeping statements"
    );
    for i in 0..N {
        let id = format!("urn:ngsi-ld:Subscription:fanout-{i}");
        let sub: Value = st
            .store
            .get(&tenant, Kind::Subscription, &id)
            .await
            .expect("read")
            .expect("subscription");
        assert_eq!(sub["notification"]["timesSent"], json!(1), "{id}");
        assert_eq!(sub["notification"]["status"], json!("ok"), "{id}");
    }
}
