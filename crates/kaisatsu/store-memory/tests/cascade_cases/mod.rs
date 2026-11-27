//! Shared by kaisatsu-store-memory's tests/grant_cascade.rs and kaisatsu-store-pg's.

use kaisatsu_core::*;

pub const R: &str = "cascade";

pub async fn realm(store: &dyn Store) {
    store
        .upsert_realm(RealmConfig {
            id: R.into(),
            realm: R.into(),
            ..Default::default()
        })
        .await
        .expect("realm");
}

pub async fn person(store: &dyn Store, id: &str) {
    store
        .create_user(UserRepresentation {
            id: id.into(),
            realm_id: R.into(),
            username: id.into(),
            ..Default::default()
        })
        .await
        .expect("person");
}

pub async fn client(store: &dyn Store, id: &str, client_id: &str) {
    store
        .upsert_client(ClientRepresentation {
            id: id.into(),
            realm_id: R.into(),
            client_id: client_id.into(),
            enabled: true,
            ..Default::default()
        })
        .await
        .expect("client");
}

pub async fn role(store: &dyn Store, client: Option<&str>, name: &str) {
    store
        .upsert_role(RoleRepresentation {
            id: format!("{}-{name}", client.unwrap_or("realm")),
            realm_id: R.into(),
            name: name.into(),
            client_role: client.is_some(),
            container_id: client.map(str::to_owned),
            ..Default::default()
        })
        .await
        .expect("role");
}

pub async fn group(store: &dyn Store, id: &str, parent: Option<&str>, path: &str) {
    store
        .upsert_group(GroupRepresentation {
            id: id.into(),
            realm_id: R.into(),
            parent_id: parent.map(str::to_owned),
            name: path.rsplit('/').next().unwrap_or(path).into(),
            path: path.into(),
            ..Default::default()
        })
        .await
        .expect("group");
}

pub async fn grant(store: &dyn Store, kind: &str, id: &str, client: Option<&str>, name: &str) {
    store
        .add_role_mapping(RoleMappingRecord {
            id: String::new(),
            realm_id: R.into(),
            org_id: None,
            principal_kind: kind.into(),
            principal_id: id.into(),
            role_id: format!("{}-{name}", client.unwrap_or("realm")),
            client_uuid: client.map(str::to_owned),
            role_name: name.into(),
        })
        .await
        .expect("grant");
}

pub async fn get(store: &dyn Store, id: &str) -> UserRepresentation {
    store
        .get_user_by_id(R, None, id)
        .await
        .expect("get")
        .expect("person")
}

pub async fn mappings(store: &dyn Store, kind: &str, id: &str) -> usize {
    store
        .list_role_mappings_for_principal(R, kind, id, None)
        .await
        .expect("mappings")
        .len()
}

pub async fn deleted_realm_role(store: &dyn Store) {
    person(store, "rr").await;
    group(store, "rrg", None, "/rrg").await;
    role(store, None, "auditor").await;
    grant(store, "user", "rr", None, "auditor").await;
    grant(store, "group", "rrg", None, "auditor").await;
    assert!(store.delete_role(R, None, "auditor").await.expect("delete"));
    assert!(
        get(store, "rr").await.realm_roles.is_empty(),
        "a deleted role still grants"
    );
    assert_eq!(mappings(store, "user", "rr").await, 0);
    let g = store
        .get_group(R, None, "rrg")
        .await
        .expect("get")
        .expect("group");
    assert!(
        g.realm_roles.is_empty(),
        "a deleted role still grants by group"
    );
}

pub async fn deleted_client(store: &dyn Store) {
    person(store, "cc").await;
    client(store, "cc-uuid", "cc-app").await;
    role(store, Some("cc-uuid"), "editor").await;
    grant(store, "user", "cc", Some("cc-uuid"), "editor").await;
    assert!(
        store
            .delete_client(R, None, "cc-uuid")
            .await
            .expect("delete")
    );
    client(store, "cc-uuid-2", "cc-app").await;
    assert!(
        !get(store, "cc").await.client_roles.contains_key("cc-app"),
        "a client created again under the id inherits the old grant"
    );
    assert_eq!(mappings(store, "user", "cc").await, 0);
}

pub async fn deleted_group(store: &dyn Store) {
    person(store, "gg").await;
    group(store, "gp", None, "/gp").await;
    group(store, "gc", Some("gp"), "/gp/child").await;
    store
        .add_user_to_group(R, "gg", "gc")
        .await
        .expect("member");
    assert!(store.delete_group(R, None, "gp").await.expect("delete"));
    assert!(
        store
            .list_user_groups(R, "gg")
            .await
            .expect("groups")
            .is_empty(),
        "a child of a deleted group still grants"
    );
}

pub async fn deleted_person(store: &dyn Store) {
    person(store, "pp").await;
    role(store, None, "viewer").await;
    grant(store, "user", "pp", None, "viewer").await;
    assert!(store.delete_user(R, None, "pp").await.expect("delete"));
    assert_eq!(
        mappings(store, "user", "pp").await,
        0,
        "a deleted person keeps a mapping"
    );
}

pub async fn stale_profile_write(store: &dyn Store) {
    person(store, "st").await;
    role(store, None, "steward").await;
    grant(store, "user", "st", None, "steward").await;
    let mut read_before_the_delete = get(store, "st").await;
    assert!(store.delete_role(R, None, "steward").await.expect("delete"));
    read_before_the_delete.first_name = Some("Ada".into());
    store
        .update_user(read_before_the_delete)
        .await
        .expect("update");
    assert!(
        get(store, "st").await.realm_roles.is_empty(),
        "a profile write restored a grant"
    );
}

pub async fn named_path_without_membership(store: &dyn Store) {
    group(store, "named", None, "/named").await;
    store
        .create_user(UserRepresentation {
            id: "nm".into(),
            realm_id: R.into(),
            username: "nm".into(),
            groups: vec!["/named".into()],
            ..Default::default()
        })
        .await
        .expect("person");
    assert!(
        store
            .list_user_groups(R, "nm")
            .await
            .expect("groups")
            .is_empty(),
        "a path in the document gave a group"
    );
    assert!(
        store
            .list_group_members(R, None, "named", 0, 10)
            .await
            .expect("members")
            .is_empty()
    );
}

pub async fn deleted_client_role(store: &dyn Store) {
    person(store, "cr").await;
    group(store, "crg", None, "/crg").await;
    client(store, "cr-uuid", "cr-app").await;
    role(store, Some("cr-uuid"), "editor").await;
    role(store, Some("cr-uuid"), "reader").await;
    grant(store, "user", "cr", Some("cr-uuid"), "editor").await;
    grant(store, "user", "cr", Some("cr-uuid"), "reader").await;
    grant(store, "group", "crg", Some("cr-uuid"), "editor").await;
    assert!(
        store
            .delete_role(R, Some("cr-uuid"), "editor")
            .await
            .expect("delete")
    );
    assert_eq!(get(store, "cr").await.client_roles["cr-app"], ["reader"]);
    let left = store
        .list_role_mappings_for_principal(R, "group", "crg", Some("cr-uuid"))
        .await
        .expect("mappings");
    assert!(
        left.is_empty(),
        "a deleted client role still grants by group"
    );
}

/// Removing what was never there changes nothing: a client role of a client that is gone, a
/// group's mapping (no person document to edit), a person's mapping of a client they hold no
/// role of.
pub async fn absent_grants_remove_cleanly(store: &dyn Store) {
    person(store, "ab").await;
    client(store, "ab-uuid", "ab-app").await;
    role(store, None, "ab-role").await;
    grant(store, "user", "ab", None, "ab-role").await;
    store
        .delete_role(R, Some("gone-uuid"), "editor")
        .await
        .expect("a gone client's role deletes as nothing");
    store
        .remove_role_mapping(R, "group", "no-group", None, "ab-role")
        .await
        .expect("a group mapping that is not there");
    store
        .remove_role_mapping(R, "user", "ab", Some("ab-uuid"), "editor")
        .await
        .expect("a client role the person never held");
    let ab = get(store, "ab").await;
    assert_eq!(ab.realm_roles, ["ab-role"]);
    assert!(
        !ab.client_roles.contains_key("ab-app"),
        "{:?}",
        ab.client_roles
    );
}

pub async fn documents_lose_what_is_deleted(store: &dyn Store) {
    client(store, "doc-uuid", "doc-app").await;
    role(store, Some("doc-uuid"), "editor").await;
    role(store, None, "doc-role").await;
    store
        .upsert_group(GroupRepresentation {
            id: "doc-group".into(),
            realm_id: R.into(),
            name: "doc-group".into(),
            path: "/doc-group".into(),
            realm_roles: vec!["doc-role".into()],
            client_roles: [("doc-app".to_owned(), vec!["editor".to_owned()])].into(),
            ..Default::default()
        })
        .await
        .expect("group");
    store
        .upsert_role(RoleRepresentation {
            realm_id: R.into(),
            name: "doc-parent".into(),
            composites: RoleComposites {
                realm: vec!["doc-role".into()],
                client: [("doc-app".to_owned(), vec!["editor".to_owned()])].into(),
            },
            ..Default::default()
        })
        .await
        .expect("composite");
    let mut config = store.get_realm(R).await.expect("get").expect("realm");
    config.default_roles.push("doc-role".into());
    store.upsert_realm(config).await.expect("defaults");

    assert!(store.delete_role(R, None, "doc-role").await.expect("role"));
    assert!(
        store
            .delete_client(R, None, "doc-uuid")
            .await
            .expect("client")
    );
    let g = store
        .get_group(R, None, "doc-group")
        .await
        .expect("get")
        .expect("group");
    assert!(
        g.realm_roles.is_empty() && g.client_roles.is_empty(),
        "{g:?}"
    );
    let parent = store
        .list_roles(R, None)
        .await
        .expect("roles")
        .into_iter()
        .find(|r| r.name == "doc-parent")
        .expect("parent");
    assert!(parent.composites.realm.is_empty() && parent.composites.client.is_empty());
    let config = store.get_realm(R).await.expect("get").expect("realm");
    assert!(!config.default_roles.iter().any(|r| r == "doc-role"));
    assert!(
        store
            .list_roles(R, Some("doc-uuid"))
            .await
            .expect("roles")
            .is_empty()
    );
}

pub async fn renamed_client(store: &dyn Store) {
    person(store, "rn").await;
    let mut renamed = store
        .upsert_client(ClientRepresentation {
            id: "rn-uuid".into(),
            realm_id: R.into(),
            client_id: "rn-old".into(),
            enabled: true,
            service_accounts_enabled: true,
            ..Default::default()
        })
        .await
        .expect("client");
    grant(store, "user", "rn", Some("rn-uuid"), "editor").await;
    renamed.client_id = "rn-new".into();
    store.upsert_client(renamed).await.expect("rename");
    let grants = get(store, "rn").await.client_roles;
    assert!(!grants.contains_key("rn-old"), "the old id still grants");
    assert_eq!(grants["rn-new"], ["editor"]);
    let account = async |name: &str| {
        store
            .get_user_by_username_or_email(R, None, name)
            .await
            .expect("lookup")
    };
    assert!(account("service-account-rn-old").await.is_none());
    let moved = account("service-account-rn-new")
        .await
        .expect("service account");
    assert_eq!(moved.service_account_client_id.as_deref(), Some("rn-new"));

    client(store, "rn-other", "rn-taken").await;
    let mut clash = store
        .get_client_by_uuid(R, None, "rn-uuid")
        .await
        .expect("get")
        .expect("client");
    clash.client_id = "rn-taken".into();
    assert!(matches!(
        store.upsert_client(clash).await,
        Err(KaisatsuError::Conflict(_))
    ));
}

pub async fn deleted_service_account(store: &dyn Store) {
    group(store, "sag", None, "/sag").await;
    store
        .upsert_client(ClientRepresentation {
            id: "sa-uuid".into(),
            realm_id: R.into(),
            client_id: "sa-app".into(),
            service_accounts_enabled: true,
            ..Default::default()
        })
        .await
        .expect("client");
    let account = store
        .get_user_by_username_or_email(R, None, "service-account-sa-app")
        .await
        .expect("lookup")
        .expect("account");
    role(store, None, "sa-role").await;
    grant(store, "user", &account.id, None, "sa-role").await;
    store
        .add_user_to_group(R, &account.id, "sag")
        .await
        .expect("member");
    assert!(
        store
            .delete_client(R, None, "sa-uuid")
            .await
            .expect("delete")
    );
    assert_eq!(mappings(store, "user", &account.id).await, 0);
    assert!(
        store
            .list_group_members(R, None, "sag", 0, 10)
            .await
            .expect("members")
            .is_empty()
    );
}

pub async fn edited_group(store: &dyn Store) {
    role(store, None, "eg-role").await;
    let stored = store
        .upsert_group(GroupRepresentation {
            id: "eg".into(),
            realm_id: R.into(),
            name: "eg".into(),
            path: "/eg".into(),
            realm_roles: vec!["eg-role".into()],
            ..Default::default()
        })
        .await
        .expect("group");
    assert!(store.delete_role(R, None, "eg-role").await.expect("role"));
    let edited = store.update_group(stored.clone()).await.expect("edit");
    assert!(
        edited.realm_roles.is_empty(),
        "a group edit restored a grant"
    );
    assert!(store.delete_group(R, None, "eg").await.expect("delete"));
    assert!(matches!(
        store.update_group(stored).await,
        Err(KaisatsuError::NotFound(_))
    ));
}

pub async fn subgroup_of_another_organization(store: &dyn Store) {
    for (id, org, parent, path) in [
        ("og", "hel", None, "/hel-root"),
        ("ogc", "zil", Some("og"), "/hel-root/zil"),
    ] {
        store
            .upsert_group(GroupRepresentation {
                id: id.into(),
                realm_id: R.into(),
                org_id: Some(org.into()),
                parent_id: parent.map(str::to_owned),
                name: id.into(),
                path: path.into(),
                ..Default::default()
            })
            .await
            .expect("group");
    }
    let hel = ["hel".to_owned()];
    assert!(matches!(
        store.delete_group(R, Some(&hel), "og").await,
        Err(KaisatsuError::Conflict(_))
    ));
    assert!(
        store
            .get_group(R, None, "ogc")
            .await
            .expect("get")
            .is_some()
    );
    assert!(store.delete_group(R, None, "og").await.expect("delete"));
    assert!(
        store
            .get_group(R, None, "ogc")
            .await
            .expect("get")
            .is_none()
    );
}

pub async fn grants_to_the_absent(store: &dyn Store) {
    group(store, "ab", None, "/ab").await;
    assert!(matches!(
        store.add_user_to_group(R, "nobody", "ab").await,
        Err(KaisatsuError::NotFound(_))
    ));
    for kind in ["user", "group"] {
        let mapping = RoleMappingRecord {
            id: String::new(),
            realm_id: R.into(),
            org_id: None,
            principal_kind: kind.into(),
            principal_id: "nobody".into(),
            role_id: "any".into(),
            role_name: "any".into(),
            client_uuid: None,
        };
        assert!(matches!(
            store.add_role_mapping(mapping).await,
            Err(KaisatsuError::NotFound(_))
        ));
    }
}
