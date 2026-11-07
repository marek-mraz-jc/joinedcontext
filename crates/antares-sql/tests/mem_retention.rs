// SPDX-License-Identifier: EUPL-1.2
//! Retention per Tenant and type on the memory and file arms (`/q/retention`,
//! T-3900), through the driver traits the sweep calls: the same rules
//! `pg_retention.rs` holds the Postgres arm to. Current state of the type,
//! not modified since the cut and without its own `expiresAt`, goes; history
//! of the type before the cut goes; a dry run counts and keeps; the batch
//! bounds each call; another Tenant and another type stay.

use antares_model::TenantId;
use antares_sql::store::any::AnyStore;
use antares_sql::store::{Kind, Store};
use antares_store::{CurrentStateDriver, HistoryScope, TemporalDriver, HISTORY_RETENTION};
use serde_json::json;

const V: &str = "https://example.org/Vehicle";
const W: &str = "https://example.org/Stop";
const SPEED: &str = "https://example.org/speed";
const CUT: &str = "2026-01-01T00:05:00Z";

fn ty(type_iri: &str) -> HistoryScope<'_> {
    HistoryScope::Type {
        type_iri,
        except_ids: &[],
    }
}

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant")
}

async fn current_state(s: &AnyStore) {
    let (a, b) = (tenant("retaina"), tenant("retainb"));
    for (t, id, ty, modified, expires) in [
        (&a, "urn:v:old", V, "2026-01-01T00:00:00Z", false),
        (&a, "urn:v:fresh", V, "2026-01-01T00:09:00Z", false),
        (&a, "urn:v:own-expiry", V, "2026-01-01T00:00:00Z", true),
        (&a, "urn:w:old", W, "2026-01-01T00:00:00Z", false),
        (&b, "urn:v:old", V, "2026-01-01T00:00:00Z", false),
    ] {
        let mut doc = json!({"id": id, "type": [ty], "modifiedAt": modified});
        if expires {
            doc["expiresAt"] = json!("2099-01-01T00:00:00Z");
        }
        assert!(s.create(t, Kind::Entity, id, doc).await.expect("create"));
    }

    assert_eq!(
        s.retain_current(&a, V, CUT, 1000, true)
            .await
            .expect("store"),
        1
    );
    assert!(
        s.get(&a, Kind::Entity, "urn:v:old")
            .await
            .expect("store")
            .is_some(),
        "a dry run keeps"
    );
    assert_eq!(
        s.retain_current(&a, V, CUT, 1000, false)
            .await
            .expect("store"),
        1
    );
    assert!(s
        .get(&a, Kind::Entity, "urn:v:old")
        .await
        .expect("store")
        .is_none());
    for id in ["urn:v:fresh", "urn:v:own-expiry", "urn:w:old"] {
        assert!(
            s.get(&a, Kind::Entity, id).await.expect("store").is_some(),
            "{id} stays"
        );
    }
    assert!(
        s.get(&b, Kind::Entity, "urn:v:old")
            .await
            .expect("store")
            .is_some(),
        "tenant B untouched"
    );
    assert_eq!(
        s.retain_current(&a, V, CUT, 1000, false)
            .await
            .expect("store"),
        0,
        "idempotent"
    );
    assert_eq!(
        s.retain_current(&tenant("retainnone"), V, CUT, 1000, false)
            .await
            .expect("store"),
        0
    );
}

async fn history(s: &AnyStore) {
    let (h, i) = (tenant("retainh"), tenant("retaini"));
    for t in [&h, &i] {
        for (id, ty) in [("urn:v:1", V), ("urn:w:1", W)] {
            let doc = json!({"id": id, "type": [ty], SPEED: [
                {"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"},
                {"type": "Property", "value": 2, "modifiedAt": "2026-01-01T00:09:00Z"}]});
            assert!(s.create(t, Kind::Temporal, id, doc).await.expect("create"));
        }
    }
    let speeds =
        |doc: Option<serde_json::Value>| doc.expect("doc")[SPEED].as_array().map_or(0, Vec::len);

    assert_eq!(
        s.retain_history(&h, ty(V), CUT, 1000, true)
            .await
            .expect("store"),
        1
    );
    assert_eq!(
        speeds(s.get(&h, Kind::Temporal, "urn:v:1").await.expect("store")),
        2,
        "a dry run keeps"
    );
    assert_eq!(
        s.retain_history(&h, ty(V), CUT, 1000, false)
            .await
            .expect("store"),
        1
    );
    assert_eq!(
        speeds(s.get(&h, Kind::Temporal, "urn:v:1").await.expect("store")),
        1
    );
    assert_eq!(
        speeds(s.get(&h, Kind::Temporal, "urn:w:1").await.expect("store")),
        2,
        "the Stop keeps"
    );
    assert_eq!(
        speeds(s.get(&i, Kind::Temporal, "urn:v:1").await.expect("store")),
        2,
        "tenant I untouched"
    );

    // A backlog drains in calls of at most the batch, and an attribute left
    // without instances goes.
    let backlog: Vec<_> = (0..25)
        .map(|n| json!({"type": "Property", "value": n, "observedAt": format!("2025-12-31T23:{n:02}:00Z")}))
        .collect();
    let doc = json!({"id": "urn:v:backlog", "type": V, SPEED: backlog});
    assert!(s
        .create(&h, Kind::Temporal, "urn:v:backlog", doc)
        .await
        .expect("backlog"));
    let mut rounds = Vec::new();
    loop {
        let n = s
            .retain_history(&h, ty(V), CUT, 10, false)
            .await
            .expect("store");
        rounds.push(n);
        if n < 10 {
            break;
        }
    }
    // urn:v:1 has no old instance left, so the backlog's 25 are all there is.
    assert_eq!(rounds, [10, 10, 5]);
    let left = s
        .get(&h, Kind::Temporal, "urn:v:backlog")
        .await
        .expect("store")
        .expect("doc");
    assert!(
        left.get(SPEED).is_none(),
        "an emptied attribute goes: {left}"
    );
    assert_eq!(
        s.retain_history(&tenant("retainnone"), ty(V), CUT, 10, false)
            .await
            .expect("store"),
        0
    );
}

/// DM-97: one Entity, a type without the Entities that rule themselves,
/// and the rest of a Tenant without its ruled types; and the Entities that
/// carry their own `historyRetention`, across Tenants.
async fn scopes(s: &AnyStore) {
    let t = tenant("retainscope");
    for (id, ty) in [
        ("urn:v:own", V),
        ("urn:v:plain", V),
        ("urn:w:plain", W),
        ("urn:x:plain", "https://example.org/Other"),
    ] {
        let doc = json!({"id": id, "type": [ty], SPEED: [
            {"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"}]});
        assert!(s.create(&t, Kind::Temporal, id, doc).await.expect("create"));
    }
    let t = &t;
    let left = |id: &'static str| async move {
        s.get(t, Kind::Temporal, id)
            .await
            .expect("store")
            .and_then(|d| d.get(SPEED).cloned())
            .is_some()
    };
    let own = ["urn:v:own".to_owned()];
    let kept = HistoryScope::Type {
        type_iri: V,
        except_ids: &own,
    };
    assert_eq!(
        s.retain_history(t, kept, CUT, 100, false)
            .await
            .expect("store"),
        1
    );
    assert!(
        left("urn:v:own").await,
        "the Entity with its own retention stays"
    );
    assert!(!left("urn:v:plain").await);
    let rest = HistoryScope::Rest {
        except_types: &[W.to_owned()],
        except_ids: &own,
    };
    assert_eq!(
        s.retain_history(t, rest, CUT, 100, false)
            .await
            .expect("store"),
        1
    );
    assert!(
        left("urn:w:plain").await,
        "a ruled type stays out of the default"
    );
    assert!(!left("urn:x:plain").await);
    assert!(left("urn:v:own").await);
    let one = HistoryScope::Entity("urn:v:own");
    assert_eq!(
        s.retain_history(t, one, CUT, 100, true)
            .await
            .expect("store"),
        1
    );
    assert_eq!(
        s.retain_history(t, one, CUT, 100, false)
            .await
            .expect("store"),
        1
    );
    assert!(!left("urn:v:own").await);
    assert!(left("urn:w:plain").await);

    for (tn, id, own) in [
        ("retainowna", "urn:v:a", Some("P7D")),
        ("retainownb", "urn:v:b", Some("off")),
        ("retainownb", "urn:v:c", None),
    ] {
        let mut doc = json!({"id": id, "type": [V]});
        if let Some(v) = own {
            doc[HISTORY_RETENTION] = json!([{"type": "Property", "value": v}]);
        }
        assert!(s
            .create(&tenant(tn), Kind::Entity, id, doc)
            .await
            .expect("create"));
    }
    let mut found: Vec<(String, String)> = s
        .history_overrides(100)
        .await
        .expect("store")
        .into_iter()
        .map(|(t, id, _)| (t, id))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("retainowna".to_owned(), "urn:v:a".to_owned()),
            ("retainownb".to_owned(), "urn:v:b".to_owned())
        ]
    );
    assert_eq!(
        s.history_overrides(1).await.expect("store").len(),
        1,
        "the limit holds"
    );
}

#[tokio::test]
async fn the_memory_store_retains_one_entity_a_type_and_the_rest() {
    scopes(&AnyStore::Mem(Store::default())).await;
}

#[tokio::test]
async fn the_memory_store_retains_per_tenant_and_type() {
    let s = AnyStore::Mem(Store::default());
    current_state(&s).await;
    history(&s).await;
}

#[tokio::test]
async fn the_file_store_retains_per_tenant_and_type_and_keeps_it_across_a_reopen() {
    let dir = std::env::temp_dir().join(format!("antares-retention-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    {
        let s = AnyStore::Mem(Store::open_file(&dir).expect("open"));
        current_state(&s).await;
        history(&s).await;
        s.close().await;
    }
    let s = AnyStore::Mem(Store::open_file(&dir).expect("reopen"));
    let a = tenant("retaina");
    assert!(
        s.get(&a, Kind::Entity, "urn:v:old")
            .await
            .expect("store")
            .is_none(),
        "the drop was persisted"
    );
    assert!(s
        .get(&a, Kind::Entity, "urn:v:fresh")
        .await
        .expect("store")
        .is_some());
    let v1 = s
        .get(&tenant("retainh"), Kind::Temporal, "urn:v:1")
        .await
        .expect("store")
        .expect("doc");
    assert_eq!(
        v1[SPEED].as_array().map(Vec::len),
        Some(1),
        "the trimmed history was persisted"
    );
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// DM-97, owner 2026-10-18: an Entity its `expiresAt` ends takes its
/// history with it; one without keeps its own.
#[tokio::test]
async fn an_expired_entity_goes_with_its_history_on_the_memory_arm() {
    let s = AnyStore::Mem(Store::default());
    let t = tenant("retainexp");
    for (id, expires) in [
        ("urn:v:exp", Some("2000-01-01T00:00:00Z")),
        ("urn:v:stays", None),
    ] {
        let mut doc = json!({"id": id, "type": [V]});
        if let Some(at) = expires {
            doc["expiresAt"] = json!(at);
        }
        assert!(s.create(&t, Kind::Entity, id, doc).await.expect("create"));
        let hist = json!({"id": id, "type": [V], SPEED: [
            {"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"}]});
        assert!(s
            .create(&t, Kind::Temporal, id, hist)
            .await
            .expect("history"));
    }
    assert!(s.sweep_expired() >= 1);
    assert!(s
        .get(&t, Kind::Entity, "urn:v:exp")
        .await
        .expect("store")
        .is_none());
    assert!(s
        .get(&t, Kind::Temporal, "urn:v:exp")
        .await
        .expect("store")
        .is_none());
    assert!(s
        .get(&t, Kind::Temporal, "urn:v:stays")
        .await
        .expect("store")
        .is_some());
}
