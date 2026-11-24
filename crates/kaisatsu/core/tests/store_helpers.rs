#![forbid(unsafe_code)]
//! The store-generic helpers of kaisatsu-core (whole-realm reads page by page, the attributes
//! protocol mappers copy into tokens) over a `MemoryStore`.

use kaisatsu_core::claims::claim_bearing_attributes;
use kaisatsu_core::listing::{all_group_members, all_users};
use kaisatsu_core::*;
use kaisatsu_store_memory::MemoryStore;
use std::collections::BTreeMap;

async fn realm(store: &MemoryStore, id: &str) {
    store
        .upsert_realm(RealmConfig {
            id: id.into(),
            realm: id.into(),
            ..RealmConfig::default()
        })
        .await
        .expect("realm");
}

async fn people(store: &MemoryStore, realm_id: &str, n: usize) {
    for i in 0..n {
        store
            .create_user(UserRepresentation {
                id: format!("{realm_id}-u{i:05}"),
                realm_id: realm_id.into(),
                username: format!("u{i:05}"),
                ..UserRepresentation::default()
            })
            .await
            .expect("person");
    }
}

#[tokio::test]
async fn every_person_of_the_realm_is_read_across_full_pages() {
    for n in [0, 1, MAX_PAGE - 1, MAX_PAGE, MAX_PAGE + 1, 2 * MAX_PAGE + 3] {
        let store = MemoryStore::default();
        realm(&store, "r").await;
        realm(&store, "other").await;
        people(&store, "r", n).await;
        people(&store, "other", 2).await;
        let all = all_users(&store, "r").await.expect("all");
        assert_eq!(all.len(), n, "{n} people");
        assert!(all.iter().all(|u| u.realm_id == "r"));
        let mut ids: Vec<&str> = all.iter().map(|u| u.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "no one twice");
    }
}

#[tokio::test]
async fn every_member_of_the_group_is_read_across_full_pages() {
    for n in [0, MAX_PAGE, MAX_PAGE + 2] {
        let store = MemoryStore::default();
        realm(&store, "r").await;
        people(&store, "r", n + 3).await;
        let group = store
            .upsert_group(GroupRepresentation {
                realm_id: "r".into(),
                name: "team".into(),
                path: "/team".into(),
                ..GroupRepresentation::default()
            })
            .await
            .expect("group");
        for i in 0..n {
            store
                .add_user_to_group("r", &format!("r-u{i:05}"), &group.id)
                .await
                .expect("member");
        }
        let members = all_group_members(&store, "r", &group.id)
            .await
            .expect("members");
        assert_eq!(members.len(), n, "{n} members of {} people", n + 3);
        let mut ids: Vec<&str> = members.iter().map(|u| u.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "no one twice");
        let unknown = all_group_members(&store, "r", "no-such-group").await;
        assert!(
            !matches!(unknown, Ok(m) if !m.is_empty()),
            "an unknown group has no members"
        );
    }
}

fn attribute_mapper(attr: Option<&str>) -> ProtocolMapperRepresentation {
    let mut config = BTreeMap::new();
    if let Some(attr) = attr {
        config.insert("user.attribute".to_owned(), attr.to_owned());
    }
    config.insert("claim.name".to_owned(), "c".to_owned());
    ProtocolMapperRepresentation {
        name: format!("m-{}", attr.unwrap_or("none")),
        protocol_mapper: "oidc-usermodel-attribute-mapper".into(),
        config,
        ..ProtocolMapperRepresentation::default()
    }
}

#[tokio::test]
async fn claim_bearing_attributes_come_from_client_and_scope_mappers_of_the_realm() {
    let store = MemoryStore::default();
    realm(&store, "r").await;
    realm(&store, "other").await;
    assert!(
        claim_bearing_attributes(&store, "r")
            .await
            .expect("none")
            .is_empty()
    );
    store
        .upsert_client(ClientRepresentation {
            id: "c1".into(),
            realm_id: "r".into(),
            client_id: "app".into(),
            protocol_mappers: vec![attribute_mapper(Some("department")), attribute_mapper(None)],
            ..ClientRepresentation::default()
        })
        .await
        .expect("client");
    store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "s1".into(),
            realm_id: "r".into(),
            name: "extra".into(),
            protocol_mappers: vec![attribute_mapper(Some("badge"))],
            ..ClientScopeRepresentation::default()
        })
        .await
        .expect("scope");
    store
        .upsert_client(ClientRepresentation {
            id: "c2".into(),
            realm_id: "other".into(),
            client_id: "app".into(),
            protocol_mappers: vec![attribute_mapper(Some("elsewhere"))],
            ..ClientRepresentation::default()
        })
        .await
        .expect("client elsewhere");
    let mut names = claim_bearing_attributes(&store, "r").await.expect("names");
    names.sort_unstable();
    assert!(names.contains(&"department".to_owned()), "{names:?}");
    assert!(names.contains(&"badge".to_owned()), "{names:?}");
    assert!(!names.contains(&"elsewhere".to_owned()), "{names:?}");
}
