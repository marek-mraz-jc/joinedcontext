// SPDX-License-Identifier: EUPL-1.2
//! Erasure of personal data from the current state and the dead letters
//! (T-4665): what goes, what a re-applied erasure's `before` keeps, and the
//! counts the `/q/erasures` log records.

use crate::erasure::{erase, record_id};
use crate::state::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use serde_json::{json, Value};

const SPEED: &str = "https://uri.etsi.org/ngsi-ld/default-context/speed";
const OWNER: &str = "https://uri.etsi.org/ngsi-ld/default-context/owner";

fn tenant() -> TenantId {
    TenantId::new("erasure").expect("tenant")
}

async fn seed(st: &AppState, id: &str, created: &str, attrs: Value) {
    let mut doc = json!({"id": id, "type": "https://x/Bike", "createdAt": created});
    if let (Some(o), Value::Object(a)) = (doc.as_object_mut(), attrs) {
        o.extend(a);
    }
    assert!(st
        .store
        .create(&tenant(), Kind::Entity, id, doc)
        .await
        .expect("seed"));
}

async fn stored(st: &AppState, id: &str) -> Option<Value> {
    st.store
        .get(&tenant(), Kind::Entity, id)
        .await
        .expect("get")
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn a_record_id_is_fresh_each_time() {
    let (a, b) = (record_id(), record_id());
    assert_eq!(a.len(), 32, "a simple uuid: {a}");
    assert_ne!(a, b);
}

#[tokio::test]
async fn the_whole_entity_goes_with_the_dead_letters_that_name_it() {
    let st = AppState::new("antares-test".into());
    seed(&st, "urn:ngsi-ld:Bike:1", "2026-01-01T00:00:00Z", json!({})).await;
    seed(&st, "urn:ngsi-ld:Bike:2", "2026-01-01T00:00:00Z", json!({})).await;
    for (id, names) in [
        ("urn:ngsi-ld:DeadLetter:1", "urn:ngsi-ld:Bike:1"),
        ("urn:ngsi-ld:DeadLetter:2", "urn:ngsi-ld:Bike:2"),
        ("urn:ngsi-ld:DeadLetter:3", "urn:ngsi-ld:Bike:10"),
    ] {
        let letter = json!({"id": id, "payload": {"data": [{"id": names}]}});
        st.store
            .create(&tenant(), Kind::DeadLetter, id, letter)
            .await
            .expect("letter");
    }
    let gone = erase(&st, &tenant(), &ids(&["urn:ngsi-ld:Bike:1"]), None, None)
        .await
        .expect("erase");
    assert_eq!(gone, json!({"entities": 1, "history": 0, "deadLetters": 1}));
    assert!(stored(&st, "urn:ngsi-ld:Bike:1").await.is_none());
    assert!(
        stored(&st, "urn:ngsi-ld:Bike:2").await.is_some(),
        "another Entity stays"
    );
    let left: Vec<String> = st
        .store
        .list(&tenant(), Kind::DeadLetter)
        .await
        .expect("letters")
        .iter()
        .filter_map(|l| l["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        left.len(),
        2,
        "a letter naming Bike:10 is no letter of Bike:1: {left:?}"
    );
    assert!(!left.contains(&"urn:ngsi-ld:DeadLetter:1".to_owned()));

    // an Entity that is not there erases nothing, and says so
    let again = erase(&st, &tenant(), &ids(&["urn:ngsi-ld:Bike:1"]), None, None)
        .await
        .expect("erase again");
    assert_eq!(
        again,
        json!({"entities": 0, "history": 0, "deadLetters": 0})
    );
}

#[tokio::test]
async fn a_re_applied_erasure_keeps_what_was_written_after_it() {
    let st = AppState::new("antares-test".into());
    let cut = "2026-03-01T00:00:00Z";
    seed(
        &st,
        "urn:ngsi-ld:Bike:old",
        "2026-01-01T00:00:00Z",
        json!({}),
    )
    .await;
    seed(
        &st,
        "urn:ngsi-ld:Bike:new",
        "2026-04-01T00:00:00Z",
        json!({}),
    )
    .await;
    let gone = erase(
        &st,
        &tenant(),
        &ids(&["urn:ngsi-ld:Bike:old", "urn:ngsi-ld:Bike:new"]),
        None,
        Some(cut),
    )
    .await
    .expect("erase");
    assert_eq!(
        gone["entities"], 1,
        "only the Entity created before the cut"
    );
    assert!(stored(&st, "urn:ngsi-ld:Bike:old").await.is_none());
    assert!(stored(&st, "urn:ngsi-ld:Bike:new").await.is_some());
}

#[tokio::test]
async fn named_attributes_go_by_their_own_stamp() {
    let st = AppState::new("antares-test".into());
    let id = "urn:ngsi-ld:Bike:3";
    seed(
        &st,
        id,
        "2026-01-01T00:00:00Z",
        json!({
            // two instances, the newer one after the cut: the Attribute stays
            SPEED: [{"type": "Property", "value": 1, "observedAt": "2026-02-01T00:00:00Z"},
                    {"type": "Property", "value": 2, "observedAt": "2026-05-01T00:00:00Z"}],
            // stamped before the cut: goes
            OWNER: {"type": "Property", "value": "Ann", "modifiedAt": "2026-02-01T00:00:00Z"},
            // no stamp at all: goes
            "https://x/phone": {"type": "Property", "value": "+421"},
        }),
    )
    .await;
    let attrs = ids(&[SPEED, OWNER, "https://x/phone", "https://x/absent"]);
    let gone = erase(
        &st,
        &tenant(),
        &ids(&[id]),
        Some(&attrs),
        Some("2026-03-01T00:00:00Z"),
    )
    .await
    .expect("erase");
    assert_eq!(gone["entities"], 1);
    let doc = stored(&st, id).await.expect("the Entity itself stays");
    assert!(doc.get(SPEED).is_some(), "observed after the cut: {doc}");
    assert!(
        doc.get(OWNER).is_none() && doc.get("https://x/phone").is_none(),
        "{doc}"
    );

    // nothing left to take: the Entity counts as untouched
    let again = erase(&st, &tenant(), &ids(&[id]), Some(&ids(&[OWNER])), None)
        .await
        .expect("erase again");
    assert_eq!(again["entities"], 0);

    // a `before` that is no instant erases nothing: read as no cut it
    // would take what was written after the erasure
    let refused = erase(
        &st,
        &tenant(),
        &ids(&[id]),
        Some(&ids(&[SPEED])),
        Some("yesterday"),
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(stored(&st, id).await.expect("stays").get(SPEED).is_some());

    // no `before`: every named Attribute goes, whatever its stamp
    let all = erase(&st, &tenant(), &ids(&[id]), Some(&ids(&[SPEED])), None)
        .await
        .expect("erase all");
    assert_eq!(all["entities"], 1);
    assert!(stored(&st, id).await.expect("stays").get(SPEED).is_none());

    // an Attribute erasure on an Entity that is not there
    let none = erase(
        &st,
        &tenant(),
        &ids(&["urn:ngsi-ld:Bike:0"]),
        Some(&attrs),
        None,
    )
    .await
    .expect("erase absent");
    assert_eq!(none["entities"], 0);
}
