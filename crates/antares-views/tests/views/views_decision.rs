// SPDX-License-Identifier: EUPL-1.2
//! The view root takes no decision of its own (T-5012): `serve` hands each
//! request to `AppState::call`, whose NGSI-LD handlers ask the policy engine,
//! and the view, never the client, names the Tenant the engine judges.
//! The code graph cannot follow that in-process call, so these tests are
//! the proof `ci/cg-allowlist.txt` cites for `ANY /x/views/...`.

use crate::common::{fixture_with, send, stored, FLEET};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// (Tenant, view) of every decision taken.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Grants nothing asked through a view; the root, where the fixture seeds,
/// is allowed.
struct NoViewGrant(Asked);

impl PolicyEngine for NoViewGrant {
    fn name(&self) -> &'static str {
        "no-view-grant"
    }

    fn decide<'a>(&'a self, subject: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        let view = subject.view().map(str::to_owned);
        let deny = view.is_some();
        if let Ok(mut asked) = self.0.lock() {
            asked.push((subject.tenant.as_str().to_owned(), view));
        }
        Box::pin(std::future::ready(if deny {
            Decision::Deny("no rule grants it".into())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn taken(asked: &Asked) -> Vec<(String, Option<String>)> {
    asked
        .lock()
        .map(|mut a| std::mem::take(&mut *a))
        .unwrap_or_default()
}

/// Every kind of operation the view `fleet` serves (read, query, type
/// discovery, temporal, create, update, delete, Subscription) is 403 when
/// the engine grants nothing through it, and each was decided once in the
/// view's Tenant, through the view.
#[tokio::test(flavor = "multi_thread")]
async fn every_operation_through_a_view_waits_for_a_decision() -> Result<(), String> {
    let asked = Asked::default();
    let f = fixture_with(
        TestBroker::new()
            .await?
            .with_policy(Arc::new(NoViewGrant(asked.clone()))),
    )
    .await?;
    let car = "urn:ngsi-ld:Car:1";
    let before = stored(&f.app, car).await.json;
    taken(&asked);
    let speed = json!({"speed": {"type": "Property", "value": 1}});
    let cases: [(Method, String, Option<Value>); 8] = [
        (Method::GET, format!("{FLEET}/entities/{car}"), None),
        (Method::GET, format!("{FLEET}/entities?type=Car"), None),
        (Method::GET, format!("{FLEET}/types"), None),
        (
            Method::GET,
            format!("{FLEET}/temporal/entities/{car}"),
            None,
        ),
        (
            Method::POST,
            format!("{FLEET}/entities"),
            Some(json!({"id": "urn:ngsi-ld:Car:7", "type": "Car"})),
        ),
        (
            Method::PATCH,
            format!("{FLEET}/entities/{car}/attrs"),
            Some(speed),
        ),
        (Method::DELETE, format!("{FLEET}/entities/{car}"), None),
        (
            Method::POST,
            format!("{FLEET}/subscriptions"),
            Some(json!({
                "type": "Subscription", "entities": [{"type": "Car"}],
                "notification": {"endpoint": {"uri": "http://127.0.0.1:9/n"}}
            })),
        ),
    ];
    for (method, uri, body) in cases {
        let a = send(&f.app, method.clone(), &uri, &[], body.as_ref()).await;
        assert_eq!(
            a.status,
            StatusCode::FORBIDDEN,
            "{method} {uri}: {}",
            a.text
        );
        let decided = taken(&asked);
        assert!(!decided.is_empty(), "{method} {uri}: no decision taken");
        assert!(
            decided
                .iter()
                .all(|(t, v)| t == "city" && v.as_deref() == Some("fleet")),
            "{method} {uri}: {decided:?}"
        );
    }
    assert_eq!(
        stored(&f.app, car).await.json,
        before,
        "a refused write changed {car}"
    );
    let made = send(
        &f.app,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:7",
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await;
    assert_eq!(made.status, StatusCode::NOT_FOUND, "{}", made.text);
    Ok(())
}

/// The Tenant a decision through a view is taken in is the view's: a client
/// naming none is judged in it, and a client naming another one, on a read
/// or a write, is refused before any engine is asked and lands nothing there.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_cannot_move_a_view_decision_to_another_tenant() -> Result<(), String> {
    let asked = Asked::default();
    let f = fixture_with(
        TestBroker::new()
            .await?
            .with_policy(Arc::new(NoViewGrant(asked.clone()))),
    )
    .await?;
    taken(&asked);
    let read = format!("{FLEET}/entities?type=Car");
    let none = send(&f.app, Method::GET, &read, &[], None).await;
    assert_eq!(none.status, StatusCode::FORBIDDEN, "{}", none.text);
    assert!(
        taken(&asked).iter().all(|(t, _)| t == "city"),
        "no Tenant named: judged in the view's"
    );
    let write = format!("{FLEET}/entities");
    let car = json!({"id": "urn:ngsi-ld:Car:8", "type": "Car"});
    for (method, uri, body) in [
        (Method::GET, &read, None),
        (Method::POST, &write, Some(&car)),
    ] {
        for spoof in [
            vec![("NGSILD-Tenant", "other")],
            vec![("NGSILD-Tenant", "city"), ("NGSILD-Tenant", "other")],
        ] {
            let a = send(&f.app, method.clone(), uri, &spoof, body).await;
            assert_eq!(
                a.status,
                StatusCode::BAD_REQUEST,
                "{method} {spoof:?}: {}",
                a.text
            );
            assert_eq!(
                taken(&asked),
                Vec::new(),
                "{method} {spoof:?}: an engine was asked"
            );
        }
    }
    let landed = send(
        &f.app,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:8",
        &[("NGSILD-Tenant", "other")],
        None,
    )
    .await;
    assert_eq!(landed.status, StatusCode::NOT_FOUND, "{}", landed.text);
    Ok(())
}
