#![forbid(unsafe_code)]
//! One behaviour contract for every `Store` implementation (W2): the same calls and the same
//! expected answers against `MemoryStore` and, with `KAISATSU_TEST_DATABASE_URL`, `PgStore`.

use jc_time::TimestampTz;
use jiff::SignedDuration;
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_memory::MemoryStore;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

mod common;

const R: &str = "contract";

fn org(id: &str, domain: &str, verified: bool, enabled: bool) -> Organization {
    Organization {
        id: id.into(),
        realm_id: R.into(),
        name: id.into(),
        alias: id.into(),
        enabled,
        description: String::new(),
        domains: vec![OrganizationDomain {
            id: format!("dom-{id}"),
            realm_id: R.into(),
            org_id: id.into(),
            name: domain.into(),
            verified,
            verification_token: None,
            challenge_hash: None,
            challenge_expires_at: None,
        }],
        attributes: HashMap::new(),
        managed_by: None,
    }
}

fn user(id: &str, username: &str, email: Option<&str>, org_id: Option<&str>) -> UserRepresentation {
    UserRepresentation {
        id: id.into(),
        realm_id: R.into(),
        org_id: org_id.map(str::to_owned),
        username: username.into(),
        email: email.map(str::to_owned),
        first_name: Some("Ada".into()),
        last_name: Some(format!("Last-{id}")),
        ..Default::default()
    }
}

fn family(id: &str, session: &str, user: &str, client: &str) -> RefreshTokenFamily {
    RefreshTokenFamily {
        id: id.into(),
        realm_id: R.into(),
        org_id: None,
        sso_session_id: session.into(),
        user_id: user.into(),
        client_id: client.into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        acr: "1".into(),
        amr: vec!["pwd".into()],
        auth_time: 1,
        rotation_counter: 0,
        current_jti_hash: format!("hash-{id}"),
        previous_hashes: Vec::new(),
        dpop_jkt: None,
        offline: false,
        revoked: false,
        expires_at: TimestampTz::now() + SignedDuration::from_hours(1),
        act: None,
        only_audience: None,
    }
}

fn sso(id: &str, user: &str, handle: &str, expires_in: SignedDuration) -> SsoSession {
    SsoSession {
        id: id.into(),
        realm_id: R.into(),
        org_id: None,
        handle_hash: handle.into(),
        user_id: user.into(),
        username: user.into(),
        ip_address: "192.0.2.1".into(),
        user_agent: String::new(),
        start: 1,
        last_access: 1,
        auth_time: 1,
        acr: "1".into(),
        amr: vec!["pwd".into()],
        clients: HashMap::from([("portal".to_owned(), "portal".to_owned())]),
        idle_expires_at: TimestampTz::now() + expires_in,
        max_expires_at: TimestampTz::now() + SignedDuration::from_hours(10),
    }
}

fn admin_ctx() -> RequestCtx {
    RequestCtx::new(
        R,
        Principal {
            subject: Some("root".into()),
            client_id: None,
            realm_roles: vec!["realm-admin".into()],
            client_roles: HashMap::new(),
            organizations: Vec::new(),
            org_roles: HashMap::new(),
            second_factor: true,
        },
    )
}

async fn realms_orgs_and_memberships(store: &dyn Store) {
    store
        .upsert_realm(RealmConfig {
            id: String::new(),
            realm: R.into(),
            smtp_server: Some(SmtpServerConfig {
                host: "smtp.example.org".into(),
                password: "smtp-secret-value".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("realm");
    let realm = store
        .get_realm(R)
        .await
        .expect("get")
        .expect("realm exists");
    assert_eq!(realm.id, R, "an empty realm id defaults to the name");
    assert_eq!(
        realm.smtp_server.as_ref().map(|s| s.password.as_str()),
        Some("smtp-secret-value")
    );
    assert!(
        store
            .get_realm("no-such-realm")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        store
            .list_realms()
            .await
            .expect("list")
            .iter()
            .any(|r| r.realm == R)
    );

    let keys = store.list_signing_keys(R).await.expect("keys");
    assert!(
        keys.iter()
            .any(|k| k.algorithm == "ES256" && k.status == "ACTIVE")
    );
    assert!(
        keys.iter()
            .any(|k| k.algorithm == "RS256" && k.status == "ACTIVE")
    );
    assert_eq!(keys[0].algorithm, "ES256", "highest priority first");
    assert!(keys.iter().all(|k| !k.encrypted_private_key.is_empty()));
    let again = store.list_signing_keys(R).await.expect("keys");
    assert_eq!(
        keys.len(),
        again.len(),
        "listing keys twice creates no new key"
    );

    store
        .upsert_organization(org("hel", "Hel.FI", true, true))
        .await
        .expect("org");
    store
        .upsert_organization(org("zil", "zilina.sk", false, true))
        .await
        .expect("org");
    store
        .upsert_organization(org("off", "off.example", true, false))
        .await
        .expect("org");
    let by_alias = store
        .get_organization(R, "hel")
        .await
        .expect("get")
        .expect("org");
    assert_eq!(by_alias.domains.len(), 1);
    assert!(
        store
            .get_organization(R, "nope")
            .await
            .expect("get")
            .is_none()
    );
    let only_hel = store
        .list_organizations(R, Some(&["hel".to_owned()]))
        .await
        .expect("list");
    assert_eq!(
        only_hel.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
        ["hel"]
    );
    assert_eq!(
        store.list_organizations(R, None).await.expect("list").len(),
        3
    );
    let found = store.find_org_by_domain(R, "hel.fi").await.expect("find");
    assert_eq!(found.map(|o| o.id), Some("hel".to_owned()));
    assert!(
        store
            .find_org_by_domain(R, "zilina.sk")
            .await
            .expect("find")
            .is_none(),
        "an unverified domain routes nowhere"
    );
    assert!(
        store
            .find_org_by_domain(R, "off.example")
            .await
            .expect("find")
            .is_none(),
        "a disabled organization routes nowhere"
    );
    // Replacing by alias keeps one organization.
    let mut renamed = org("hel", "hel.fi", true, true);
    renamed.name = "Helsinki".into();
    store.upsert_organization(renamed).await.expect("org");
    assert_eq!(
        store
            .get_organization(R, "hel")
            .await
            .expect("get")
            .map(|o| o.name),
        Some("Helsinki".to_owned())
    );
    assert_eq!(
        store.list_organizations(R, None).await.expect("list").len(),
        3
    );

    for _ in 0..2 {
        store
            .upsert_membership(Membership {
                id: "m1".into(),
                realm_id: R.into(),
                org_id: "zil".into(),
                person_id: "u-member".into(),
                org_roles: vec!["org-admin".into()],
                created_at: TimestampTz::now(),
            })
            .await
            .expect("membership");
    }
    let ms = store
        .list_memberships_for_person(R, "u-member")
        .await
        .expect("list");
    assert_eq!(ms.len(), 1, "a membership upsert replaces");
}

async fn people_and_credentials(store: &dyn Store) {
    let created = store
        .create_user(user("u1", "Alice", Some("Alice@Hel.fi"), Some("hel")))
        .await
        .expect("create");
    assert_eq!(created.username, "alice");
    assert_eq!(created.email.as_deref(), Some("alice@hel.fi"));
    assert!(created.created_timestamp.is_some());
    let generated = store
        .create_user(user("", "bob", Some("bob@zilina.sk"), Some("zil")))
        .await
        .expect("create with generated id");
    assert!(!generated.id.is_empty());
    store
        .create_user(user("u-member", "carol", None, None))
        .await
        .expect("create");
    let dup_name = store.create_user(user("u9", "ALICE", None, None)).await;
    assert!(matches!(dup_name, Err(KaisatsuError::Conflict(_))));
    let dup_mail = store
        .create_user(user("u9", "someone", Some("alice@HEL.fi"), None))
        .await;
    assert!(matches!(dup_mail, Err(KaisatsuError::Conflict(_))));
    let mut transient = user("u-transient", "visitor", None, None);
    transient.transient = true;
    store.create_user(transient).await.expect("transient");
    assert!(
        store
            .get_user_by_id(R, None, "u-transient")
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        store
            .get_user_by_username_or_email(R, None, "visitor")
            .await
            .expect("get")
            .is_none()
    );

    let mut changed = created.clone();
    changed.first_name = Some("Alicia".into());
    changed.email = Some("ALICIA@hel.fi".into());
    let updated = store.update_user(changed).await.expect("update");
    assert_eq!(updated.email.as_deref(), Some("alicia@hel.fi"));
    let mut clash = updated.clone();
    clash.username = "bob".into();
    assert!(matches!(
        store.update_user(clash).await,
        Err(KaisatsuError::Conflict(_))
    ));
    let ghost = user("ghost", "ghost", None, None);
    assert!(matches!(
        store.update_user(ghost).await,
        Err(KaisatsuError::NotFound(_))
    ));

    let by_mail = store
        .get_user_by_username_or_email(R, None, " Alicia@hel.fi ")
        .await
        .expect("lookup")
        .expect("found by e-mail");
    assert_eq!(by_mail.id, "u1");
    let hel = ["hel".to_owned()];
    assert!(
        store
            .get_user_by_username_or_email(R, Some(&hel), "bob")
            .await
            .expect("lookup")
            .is_none(),
        "another organization's person is not found by name"
    );
    // Membership makes carol visible to zil.
    let zil = ["zil".to_owned()];
    assert!(
        store
            .get_user_by_id(R, Some(&zil), "u-member")
            .await
            .expect("get")
            .is_some()
    );
    assert!(
        store
            .get_user_by_id(R, Some(&hel), "u-member")
            .await
            .expect("get")
            .is_none()
    );

    let all = store
        .list_users(R, None, None, None, None, false, 0, 100)
        .await
        .expect("list");
    assert_eq!(
        all.iter().map(|u| u.username.as_str()).collect::<Vec<_>>(),
        ["alice", "bob", "carol"],
        "sorted by username, transient people never listed"
    );
    let search = store
        .list_users(R, None, Some("LAST-U1"), None, None, false, 0, 100)
        .await
        .expect("search");
    assert_eq!(search.len(), 1);
    let exact_mail = store
        .list_users(R, None, None, Some("bob@zilina.sk"), None, true, 0, 100)
        .await
        .expect("exact e-mail");
    assert_eq!(exact_mail.len(), 1);
    let partial_mail = store
        .list_users(R, None, None, Some("zilina"), None, true, 0, 100)
        .await
        .expect("exact partial");
    assert!(partial_mail.is_empty(), "exact means the whole address");
    let contains_name = store
        .list_users(R, None, None, None, Some("AR"), false, 0, 100)
        .await
        .expect("username contains");
    assert_eq!(contains_name.len(), 1);
    let exact_name = store
        .list_users(R, None, None, None, Some("ar"), true, 0, 100)
        .await
        .expect("username exact");
    assert!(exact_name.is_empty());
    let page = store
        .list_users(R, None, None, None, None, false, 1, 1)
        .await
        .expect("page");
    assert_eq!(
        page.iter().map(|u| u.username.as_str()).collect::<Vec<_>>(),
        ["bob"]
    );
    let zero_max = store
        .list_users(R, None, None, None, None, false, 0, 0)
        .await
        .expect("max 0");
    assert_eq!(
        zero_max.len(),
        1,
        "max 0 still returns one row, as Keycloak"
    );
    let scoped = store
        .list_users(R, Some(&hel), None, None, None, false, 0, 100)
        .await
        .expect("scoped");
    assert_eq!(
        scoped.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        ["u1"]
    );

    assert!(
        !store
            .delete_user(R, Some(&hel), &generated.id)
            .await
            .expect("delete")
    );
    assert!(
        store
            .get_user_by_id(R, None, &generated.id)
            .await
            .expect("get")
            .is_some()
    );

    // Credentials: a new password keeps the old one as history.
    for secret in ["$argon2id$one", "$argon2id$two"] {
        store
            .upsert_credential(CredentialRecord {
                realm_id: R.into(),
                user_id: "u1".into(),
                secret_data: secret.into(),
                ..Default::default()
            })
            .await
            .expect("credential");
    }
    store
        .upsert_credential(CredentialRecord {
            id: "otp-1".into(),
            realm_id: R.into(),
            user_id: "u1".into(),
            credential_type: "otp".into(),
            secret_data: "totp-secret".into(),
            ..Default::default()
        })
        .await
        .expect("otp");
    let creds = store.list_credentials(R, "u1").await.expect("list");
    assert_eq!(
        creds
            .iter()
            .filter(|c| c.credential_type == "password")
            .count(),
        1
    );
    let hist: Vec<_> = creds
        .iter()
        .filter(|c| c.credential_type == "password-history")
        .collect();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].secret_data, "$argon2id$one");
    assert!(creds.iter().all(|c| c.created_date > 0));
    assert!(
        store
            .delete_credential(R, "u1", "otp-1")
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_credential(R, "u1", "otp-1")
            .await
            .expect("delete again")
    );
}

async fn groups(store: &dyn Store) {
    let root = store
        .upsert_group(GroupRepresentation {
            realm_id: R.into(),
            name: "hel".into(),
            sub_groups: vec![
                GroupRepresentation {
                    name: "members".into(),
                    sub_groups: vec![GroupRepresentation {
                        name: "board".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                GroupRepresentation {
                    name: "staff".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .await
        .expect("group tree");
    assert_eq!(root.path, "/hel");
    assert_eq!(
        root.org_id.as_deref(),
        Some("hel"),
        "path root names an organization"
    );
    let board = store
        .get_group_by_path(R, None, "hel/members/board")
        .await
        .expect("by path")
        .expect("nested group stored");
    assert_eq!(board.org_id.as_deref(), Some("hel"));
    let members = store
        .get_group_by_path(R, None, "/hel/members")
        .await
        .expect("by path")
        .expect("members");
    // Upsert by path keeps the id.
    let again = store
        .upsert_group(GroupRepresentation {
            realm_id: R.into(),
            name: "members".into(),
            path: "/hel/members".into(),
            parent_id: Some(root.id.clone()),
            attributes: HashMap::from([("k".to_owned(), vec!["v".to_owned()])]),
            ..Default::default()
        })
        .await
        .expect("upsert by path");
    assert_eq!(again.id, members.id);
    store
        .upsert_group(GroupRepresentation {
            id: "g-zil".into(),
            realm_id: R.into(),
            name: "zil".into(),
            ..Default::default()
        })
        .await
        .expect("zil root");
    store
        .upsert_group(GroupRepresentation {
            id: "g-loose".into(),
            realm_id: R.into(),
            name: "loose".into(),
            ..Default::default()
        })
        .await
        .expect("realm-level root");

    let tree = store.list_groups(R, None, None, false).await.expect("tree");
    assert_eq!(
        tree.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        ["hel", "loose", "zil"]
    );
    let hel_tree = &tree[0];
    assert_eq!(
        hel_tree
            .sub_groups
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        ["members", "staff"]
    );
    assert_eq!(hel_tree.sub_groups[0].sub_groups[0].name, "board");
    let found = store
        .list_groups(R, None, Some("BOA"), false)
        .await
        .expect("search");
    assert_eq!(found.len(), 1, "a root stays when a descendant matches");
    assert_eq!(found[0].sub_groups.len(), 1);
    let exact = store
        .list_groups(R, None, Some("boa"), true)
        .await
        .expect("exact");
    assert!(exact.is_empty());
    let scoped = store
        .list_groups(R, Some(&["zil".to_owned()]), None, false)
        .await
        .expect("scoped");
    assert_eq!(
        scoped.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        ["g-zil"]
    );
    assert!(
        store
            .get_group(R, Some(&["zil".to_owned()]), &root.id)
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        store
            .get_group(R, None, &root.id)
            .await
            .expect("get")
            .is_some()
    );
    assert!(
        store
            .get_group_by_path(R, Some(&["zil".to_owned()]), "/hel")
            .await
            .expect("by path")
            .is_none()
    );

    store
        .add_user_to_group(R, "u1", &members.id)
        .await
        .expect("add");
    store
        .add_user_to_group(R, "u1", &members.id)
        .await
        .expect("add twice");
    store
        .add_user_to_group(R, "u-member", "g-loose")
        .await
        .expect("add");
    assert!(matches!(
        store.add_user_to_group(R, "u1", "no-group").await,
        Err(KaisatsuError::NotFound(_))
    ));
    let u1 = store
        .get_user_by_id(R, None, "u1")
        .await
        .expect("get")
        .expect("u1");
    assert!(
        u1.groups.is_empty(),
        "membership is a row, never a path in the document"
    );
    let u1_groups = store.list_user_groups(R, "u1").await.expect("groups");
    assert_eq!(
        u1_groups
            .iter()
            .map(|g| g.path.as_str())
            .collect::<Vec<_>>(),
        ["/hel/members"]
    );
    assert!(
        store
            .list_user_groups(R, "nobody")
            .await
            .expect("groups")
            .is_empty()
    );
    let m = store
        .list_group_members(R, None, &members.id, 0, 10)
        .await
        .expect("members");
    assert_eq!(m.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["u1"]);
    assert!(matches!(
        store
            .list_group_members(R, Some(&["zil".to_owned()]), &members.id, 0, 10)
            .await,
        Err(KaisatsuError::NotFound(_))
    ));
    assert!(matches!(
        store.list_group_members(R, None, "no-group", 0, 10).await,
        Err(KaisatsuError::NotFound(_))
    ));
    store
        .remove_user_from_group(R, "u1", &members.id)
        .await
        .expect("remove");
    let left = store.list_user_groups(R, "u1").await.expect("groups");
    assert!(left.is_empty(), "a removed member has no group");
    store
        .add_user_to_group(R, "u1", &board.id)
        .await
        .expect("add");

    assert!(
        !store
            .delete_group(R, Some(&["zil".to_owned()]), &board.id)
            .await
            .expect("delete scoped"),
        "another organization's group is not deleted"
    );
    assert!(
        store
            .delete_group(R, None, &board.id)
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_group(R, None, &board.id)
            .await
            .expect("delete again")
    );
    let u1 = store
        .get_user_by_id(R, None, "u1")
        .await
        .expect("get")
        .expect("u1");
    assert!(
        u1.groups.is_empty(),
        "a deleted group leaves no path on its members"
    );
    let left = store.list_user_groups(R, "u1").await.expect("groups");
    assert!(left.is_empty(), "a deleted group leaves no membership");
}

async fn roles_and_clients(store: &dyn Store) {
    let r1 = store
        .upsert_role(RoleRepresentation {
            realm_id: R.into(),
            name: "viewer".into(),
            ..Default::default()
        })
        .await
        .expect("role");
    let r1b = store
        .upsert_role(RoleRepresentation {
            realm_id: R.into(),
            name: "viewer".into(),
            description: Some("again".into()),
            ..Default::default()
        })
        .await
        .expect("role again");
    assert_eq!(r1.id, r1b.id, "a role upsert by name keeps the id");

    let client = store
        .upsert_client(ClientRepresentation {
            realm_id: R.into(),
            org_id: Some("hel".into()),
            client_id: "hel-app".into(),
            service_accounts_enabled: true,
            protocol_mappers: vec![ProtocolMapperRepresentation {
                name: "aud".into(),
                ..Default::default()
            }],
            attributes: BTreeMap::from([("managed-by".to_owned(), "apply".to_owned())]),
            ..Default::default()
        })
        .await
        .expect("client");
    let secret = client
        .secret
        .clone()
        .expect("a confidential client gets a secret");
    assert_eq!(client.managed_by.as_deref(), Some("apply"));
    assert!(!client.protocol_mappers[0].id.is_empty());
    let sa = store
        .get_user_by_username_or_email(R, None, "service-account-hel-app")
        .await
        .expect("lookup")
        .expect("service account person");
    assert_eq!(sa.service_account_client_id.as_deref(), Some("hel-app"));
    // Upsert by client id without a secret keeps the secret; the mappers are the ones given
    // (removing the last mapper must be possible; callers that merge do so themselves).
    let again = store
        .upsert_client(ClientRepresentation {
            realm_id: R.into(),
            org_id: Some("hel".into()),
            client_id: "hel-app".into(),
            service_accounts_enabled: true,
            ..Default::default()
        })
        .await
        .expect("client again");
    assert_eq!(again.id, client.id);
    assert_eq!(again.secret.as_deref(), Some(secret.as_str()));
    assert!(again.protocol_mappers.is_empty());
    let public = store
        .upsert_client(ClientRepresentation {
            id: "pub-uuid".into(),
            realm_id: R.into(),
            client_id: "portal".into(),
            public_client: true,
            default_client_scopes: vec!["profile".into()],
            optional_client_scopes: vec!["profile".into(), "email".into()],
            ..Default::default()
        })
        .await
        .expect("public client");
    assert!(public.secret.is_none());

    let got = store
        .get_client_by_uuid(R, None, &client.id)
        .await
        .expect("get")
        .expect("client");
    assert_eq!(got.secret.as_deref(), Some(secret.as_str()));
    assert!(
        store
            .get_client_by_uuid(R, Some(&["zil".to_owned()]), &client.id)
            .await
            .expect("get")
            .is_none()
    );
    assert_eq!(
        store
            .get_client_by_client_id(R, "portal")
            .await
            .expect("get")
            .map(|c| c.id),
        Some("pub-uuid".to_owned())
    );
    assert!(
        store
            .get_client_by_client_id(R, "nope")
            .await
            .expect("get")
            .is_none()
    );
    let exact = store
        .list_clients(R, None, Some("hel"), false)
        .await
        .expect("list");
    assert!(exact.is_empty());
    let search = store
        .list_clients(R, None, Some("hel"), true)
        .await
        .expect("search");
    assert_eq!(search.len(), 1);
    assert_eq!(
        store
            .list_clients(R, None, None, false)
            .await
            .expect("all")
            .len(),
        2
    );
    assert_eq!(
        store
            .list_clients(R, Some(&["hel".to_owned()]), None, false)
            .await
            .expect("scoped")
            .len(),
        1
    );

    let client_role = store
        .upsert_role(RoleRepresentation {
            realm_id: R.into(),
            name: "editor".into(),
            client_role: true,
            container_id: Some(client.id.clone()),
            ..Default::default()
        })
        .await
        .expect("client role");
    let realm_roles = store.list_roles(R, None).await.expect("realm roles");
    assert_eq!(
        realm_roles
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["viewer"]
    );
    let client_roles = store
        .list_roles(R, Some(&client.id))
        .await
        .expect("client roles");
    assert_eq!(
        client_roles
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        ["editor"]
    );

    for (uuid, role) in [(None, &r1), (Some(client.id.clone()), &client_role)] {
        store
            .add_role_mapping(RoleMappingRecord {
                id: String::new(),
                realm_id: R.into(),
                org_id: None,
                principal_kind: "user".into(),
                principal_id: "u1".into(),
                role_id: role.id.clone(),
                role_name: role.name.clone(),
                client_uuid: uuid,
            })
            .await
            .expect("mapping");
    }
    store
        .add_role_mapping(RoleMappingRecord {
            id: "gm".into(),
            realm_id: R.into(),
            org_id: None,
            principal_kind: "group".into(),
            principal_id: "g-zil".into(),
            role_id: client_role.id.clone(),
            role_name: "editor".into(),
            client_uuid: Some(client.id.clone()),
        })
        .await
        .expect("group mapping");
    let u1 = store
        .get_user_by_id(R, None, "u1")
        .await
        .expect("get")
        .expect("u1");
    assert_eq!(u1.realm_roles, ["viewer"]);
    assert_eq!(
        u1.client_roles.get("hel-app").cloned(),
        Some(vec!["editor".to_owned()])
    );
    let realm_maps = store
        .list_role_mappings_for_principal(R, "user", "u1", None)
        .await
        .expect("maps");
    assert_eq!(realm_maps.len(), 1);
    assert!(!realm_maps[0].id.is_empty());
    let holders = store
        .list_principals_with_client_role(R, &client.id, "editor", "group")
        .await
        .expect("holders");
    assert_eq!(holders, ["g-zil"]);
    store
        .remove_role_mapping(R, "user", "u1", Some(&client.id), "editor")
        .await
        .expect("remove client mapping");
    store
        .remove_role_mapping(R, "user", "u1", None, "viewer")
        .await
        .expect("remove realm mapping");
    let u1 = store
        .get_user_by_id(R, None, "u1")
        .await
        .expect("get")
        .expect("u1");
    assert!(u1.realm_roles.is_empty());
    assert_eq!(u1.client_roles.get("hel-app").map(Vec::len), Some(0));
    assert!(
        store
            .delete_role(R, Some(&client.id), "editor")
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_role(R, Some(&client.id), "editor")
            .await
            .expect("again")
    );
    assert!(
        store
            .list_principals_with_client_role(R, &client.id, "editor", "group")
            .await
            .expect("holders")
            .is_empty(),
        "deleting a role deletes its mappings"
    );

    // Client scopes and their links on clients.
    let scope = store
        .upsert_client_scope(ClientScopeRepresentation {
            realm_id: R.into(),
            name: "profile".into(),
            protocol_mappers: vec![ProtocolMapperRepresentation {
                name: "given".into(),
                ..Default::default()
            }],
            attributes: BTreeMap::from([("managed-by".to_owned(), "apply".to_owned())]),
            ..Default::default()
        })
        .await
        .expect("scope");
    let same = store
        .upsert_client_scope(ClientScopeRepresentation {
            realm_id: R.into(),
            name: "profile".into(),
            ..Default::default()
        })
        .await
        .expect("scope again");
    assert_eq!(scope.id, same.id);
    assert_eq!(scope.managed_by.as_deref(), Some("apply"));
    assert!(
        store
            .get_client_scope(R, "profile")
            .await
            .expect("get")
            .is_some()
    );
    assert!(
        store
            .get_client_scope(R, &scope.id)
            .await
            .expect("get")
            .is_some()
    );
    assert_eq!(store.list_client_scopes(R).await.expect("list").len(), 1);
    assert!(
        store
            .delete_client_scope(R, &scope.id)
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_client_scope(R, &scope.id)
            .await
            .expect("again")
    );
    let portal = store
        .get_client_by_client_id(R, "portal")
        .await
        .expect("get")
        .expect("portal");
    assert!(portal.default_client_scopes.is_empty());
    assert_eq!(portal.optional_client_scopes, ["email"]);

    // Identity providers.
    for alias in ["entra", "entra"] {
        store
            .upsert_idp(IdentityProviderRepresentation {
                realm_id: R.into(),
                alias: alias.into(),
                ..Default::default()
            })
            .await
            .expect("idp");
    }
    let idp = store.get_idp(R, "entra").await.expect("get").expect("idp");
    assert!(!idp.internal_id.is_empty());
    assert!(store.get_idp(R, "none").await.expect("get").is_none());
    assert_eq!(store.list_idps(R).await.expect("list").len(), 1);

    // Deleting a client removes its service account, never a person of that name.
    assert!(
        !store
            .delete_client(R, Some(&["zil".to_owned()]), &client.id)
            .await
            .expect("delete scoped")
    );
    assert!(
        store
            .delete_client(R, None, &client.id)
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_client(R, None, &client.id)
            .await
            .expect("again")
    );
    assert!(
        store
            .get_user_by_username_or_email(R, None, "service-account-hel-app")
            .await
            .expect("lookup")
            .is_none()
    );
}

async fn sessions_and_tokens(store: &dyn Store) {
    let now = TimestampTz::now();
    let auth = AuthSession {
        id: "as1".into(),
        realm_id: R.into(),
        org_id: None,
        handle_hash: "h".into(),
        client_id: "portal".into(),
        redirect_uri: "https://portal.example/cb".into(),
        response_type: "code".into(),
        response_mode: "query".into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        state: Some("s".into()),
        nonce: None,
        code_challenge: "c".into(),
        code_challenge_method: "S256".into(),
        acr_requested: None,
        prompt: None,
        login_hint: None,
        ui_locales: None,
        dpop_jkt: None,
        csrf_token: "csrf".into(),
        step_state_json: "{}".into(),
        pending_totp_secret: None,
        webauthn_challenge: None,
        pending_email_code_hash: None,
        broker_state_json: None,
        kc_action: None,
        kc_action_done: false,
        user_id: None,
        amr: Vec::new(),
        acr_achieved: "0".into(),
        created_at: now,
        expires_at: now + SignedDuration::from_mins(30),
    };
    store.put_auth_session(auth.clone()).await.expect("put");
    let mut replaced = auth.clone();
    replaced.user_id = Some("u1".into());
    store.put_auth_session(replaced).await.expect("replace");
    let got = store
        .get_auth_session_by_id(R, "as1")
        .await
        .expect("get")
        .expect("session");
    assert_eq!(got.user_id.as_deref(), Some("u1"));
    let mut expired = auth.clone();
    expired.id = "as-old".into();
    expired.expires_at = now - SignedDuration::from_mins(1);
    store.put_auth_session(expired).await.expect("put expired");
    assert!(
        store
            .get_auth_session_by_id(R, "as-old")
            .await
            .expect("get")
            .is_none()
    );
    store.delete_auth_session(R, "as1").await.expect("delete");
    assert!(
        store
            .get_auth_session_by_id(R, "as1")
            .await
            .expect("get")
            .is_none()
    );

    let code = AuthorizationCodeRecord {
        code_hash: "code-hash".into(),
        realm_id: R.into(),
        org_id: None,
        client_id: "portal".into(),
        user_id: "u1".into(),
        sso_session_id: "sso1".into(),
        redirect_uri: "https://portal.example/cb".into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        code_challenge: "c".into(),
        code_challenge_method: "S256".into(),
        nonce: None,
        acr: "1".into(),
        amr: vec!["pwd".into()],
        dpop_jkt: None,
        auth_time: 1,
        consumed: false,
        created_at: now,
        expires_at: now + SignedDuration::from_secs(60),
    };
    store.put_auth_code(code.clone()).await.expect("put code");
    let (first, replay) = store
        .consume_auth_code(R, "code-hash")
        .await
        .expect("consume");
    assert!(!replay);
    assert!(first.expect("record").consumed);
    let (second, replay) = store
        .consume_auth_code(R, "code-hash")
        .await
        .expect("consume");
    assert!(replay, "a second redemption is reported as replay");
    assert!(second.is_some());
    assert_eq!(
        store.consume_auth_code(R, "nope").await.expect("consume"),
        (None, false)
    );

    let par = ParRequestRecord {
        request_uri_hash: "par-hash".into(),
        realm_id: R.into(),
        client_id: "portal".into(),
        params: BTreeMap::from([("scope".to_owned(), "openid".to_owned())]),
        consumed: false,
        expires_at: now + SignedDuration::from_secs(60),
    };
    store.put_par_request(par.clone()).await.expect("par");
    let mut old_par = par.clone();
    old_par.request_uri_hash = "par-old".into();
    old_par.expires_at = now - SignedDuration::from_secs(1);
    store.put_par_request(old_par).await.expect("old par");
    assert!(
        store
            .consume_par_request(R, "par-hash")
            .await
            .expect("consume")
            .is_some()
    );
    assert!(
        store
            .consume_par_request(R, "par-hash")
            .await
            .expect("consume")
            .is_none()
    );
    assert!(
        store
            .consume_par_request(R, "par-old")
            .await
            .expect("consume")
            .is_none()
    );

    store
        .upsert_sso_session(sso("sso1", "u1", "handle-1", SignedDuration::from_hours(1)))
        .await
        .expect("sso");
    store
        .upsert_sso_session(sso("sso1", "u1", "handle-1", SignedDuration::from_hours(1)))
        .await
        .expect("sso twice");
    store
        .upsert_sso_session(sso("sso2", "u1", "handle-2", SignedDuration::from_hours(1)))
        .await
        .expect("sso");
    store
        .upsert_sso_session(sso(
            "sso-old",
            "u1",
            "handle-old",
            -SignedDuration::from_mins(1),
        ))
        .await
        .expect("sso");
    let got = store
        .get_sso_session(R, "sso1")
        .await
        .expect("get")
        .expect("sso");
    assert_eq!(got.handle_hash, "handle-1", "the handle hash is kept");
    assert!(
        store
            .get_sso_session(R, "sso-old")
            .await
            .expect("get")
            .is_none()
    );
    assert_eq!(
        store
            .get_sso_session_by_handle(R, "handle-2")
            .await
            .expect("by handle")
            .map(|s| s.id),
        Some("sso2".to_owned())
    );
    assert!(
        store
            .get_sso_session_by_handle(R, "handle-old")
            .await
            .expect("by handle")
            .is_none()
    );
    assert_eq!(
        store
            .list_user_sso_sessions(R, "u1")
            .await
            .expect("list")
            .len(),
        2
    );
    let ctx = admin_ctx();
    let cs = store
        .query_entity_set(
            &ctx,
            &LocalAuthorizer,
            "ClientSessions",
            &odata_table::QueryOptions {
                count: Some(true),
                ..Default::default()
            },
            100,
        )
        .await
        .expect("client sessions");
    assert_eq!(
        cs.count,
        Some(3),
        "one client session per SSO session and client"
    );

    store
        .upsert_refresh_family(family("f1", "sso1", "u1", "portal"))
        .await
        .expect("family");
    store
        .upsert_refresh_family(family("f2", "sso2", "u1", "portal"))
        .await
        .expect("family");
    store
        .upsert_refresh_family(family("f3", "sso2", "u1", "grafana"))
        .await
        .expect("family");
    store
        .upsert_refresh_family(family("f4", "sso-x", "u2", "portal"))
        .await
        .expect("family");
    store
        .upsert_refresh_family(family("f5", "sso-y", "u3", "portal"))
        .await
        .expect("family");
    // Rotation is a compare-and-swap on the current handle hash: one winner per handle.
    store
        .upsert_refresh_family(family("f6", "sso-z", "u4", "portal"))
        .await
        .expect("family");
    let mut next = family("f6", "sso-z", "u4", "portal");
    next.current_jti_hash = "hash-f6-2".into();
    next.previous_hashes = vec!["hash-f6".into()];
    next.rotation_counter = 1;
    assert!(
        !store
            .rotate_refresh_family(next.clone(), "stale-hash")
            .await
            .expect("rotate"),
        "a stale expected hash loses"
    );
    assert!(
        store
            .rotate_refresh_family(next.clone(), "hash-f6")
            .await
            .expect("rotate")
    );
    let stored = store
        .get_refresh_family(R, "f6")
        .await
        .expect("get")
        .expect("f6");
    assert_eq!(stored.current_jti_hash, "hash-f6-2");
    assert_eq!(stored.previous_hashes, ["hash-f6"]);
    assert_eq!(stored.rotation_counter, 1);
    let mut again = next.clone();
    again.current_jti_hash = "hash-f6-3".into();
    assert!(
        !store
            .rotate_refresh_family(again.clone(), "hash-f6")
            .await
            .expect("rotate"),
        "the same handle never rotates twice"
    );
    let mut revoked = next.clone();
    revoked.revoked = true;
    assert!(
        store
            .rotate_refresh_family(revoked, "hash-f6-2")
            .await
            .expect("revoke by rotation")
    );
    assert!(
        !store
            .rotate_refresh_family(again, "hash-f6-2")
            .await
            .expect("rotate"),
        "a revoked family never rotates"
    );
    let mut missing = family("f-missing", "sso-z", "u4", "portal");
    missing.current_jti_hash = "h2".into();
    assert!(
        !store
            .rotate_refresh_family(missing, "hash-f-missing")
            .await
            .expect("rotate")
    );
    assert!(
        store
            .get_refresh_family(R, "f-missing")
            .await
            .expect("get")
            .is_none(),
        "rotation never creates a family"
    );
    let removed = store
        .delete_sso_session(R, "sso1")
        .await
        .expect("delete")
        .expect("removed");
    assert_eq!(removed.handle_hash, "handle-1");
    assert!(
        store
            .delete_sso_session(R, "sso1")
            .await
            .expect("delete")
            .is_none()
    );
    assert!(
        store
            .get_refresh_family(R, "f1")
            .await
            .expect("get")
            .expect("f1")
            .revoked
    );
    store
        .revoke_refresh_families_for_session(R, "sso2", Some("grafana"))
        .await
        .expect("revoke one client");
    assert!(
        store
            .get_refresh_family(R, "f3")
            .await
            .expect("get")
            .expect("f3")
            .revoked
    );
    assert!(
        !store
            .get_refresh_family(R, "f2")
            .await
            .expect("get")
            .expect("f2")
            .revoked
    );
    store
        .revoke_refresh_families_for_session(R, "sso2", None)
        .await
        .expect("revoke session");
    assert!(
        store
            .get_refresh_family(R, "f2")
            .await
            .expect("get")
            .expect("f2")
            .revoked
    );
    assert!(
        store
            .get_refresh_family(R, "nope")
            .await
            .expect("get")
            .is_none()
    );
    let gone = store
        .delete_user_sso_sessions(R, "u1")
        .await
        .expect("delete all");
    assert_eq!(gone.len(), 2, "sso2 and the expired one");
    assert!(
        store
            .list_user_sso_sessions(R, "u1")
            .await
            .expect("list")
            .is_empty()
    );

    store
        .upsert_offline_session(OfflineSession {
            id: "off1".into(),
            realm_id: R.into(),
            org_id: None,
            user_id: "u2".into(),
            client_id: "portal".into(),
            scopes: vec!["offline_access".into()],
            created_at: now,
            last_access: now,
            idle_expires_at: now + SignedDuration::from_hours(24 * 30),
            max_expires_at: now + SignedDuration::from_hours(24 * 60),
        })
        .await
        .expect("offline");
    store
        .delete_user_client_offline_sessions(R, "u2", "portal")
        .await
        .expect("delete offline");
    assert!(
        store
            .get_refresh_family(R, "f4")
            .await
            .expect("get")
            .expect("f4")
            .revoked
    );

    let rec = RevokedTokenRecord {
        jti_hash: "jti-1".into(),
        realm_id: R.into(),
        expires_at: now + SignedDuration::from_mins(5),
    };
    store.mark_token_revoked(rec.clone()).await.expect("revoke");
    assert!(matches!(
        store.mark_token_revoked(rec).await,
        Err(KaisatsuError::Conflict(_))
    ));
    assert!(store.is_token_revoked(R, "jti-1").await.expect("is"));
    assert!(!store.is_token_revoked(R, "jti-2").await.expect("is"));
    store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: "jti-old".into(),
            realm_id: R.into(),
            expires_at: now - SignedDuration::from_mins(5),
        })
        .await
        .expect("revoke expired");
    assert!(!store.is_token_revoked(R, "jti-old").await.expect("is"));

    let failure = LoginFailureRecord {
        id: "lf".into(),
        realm_id: R.into(),
        target_key: "u1".into(),
        num_failures: 1,
        last_failure_ms: 1,
        last_ip_failure: "192.0.2.1".into(),
        locked_until: None,
        window_started_at: now,
    };
    store
        .upsert_login_failure(failure.clone())
        .await
        .expect("failure");
    let mut more = failure.clone();
    more.num_failures = 2;
    store
        .upsert_login_failure(more)
        .await
        .expect("failure again");
    assert_eq!(
        store
            .get_login_failure(R, "u1")
            .await
            .expect("get")
            .map(|f| f.num_failures),
        Some(2)
    );
    store.clear_login_failure(R, "u1").await.expect("clear");
    assert!(
        store
            .get_login_failure(R, "u1")
            .await
            .expect("get")
            .is_none()
    );

    for scopes in [
        vec!["openid".to_owned()],
        vec!["openid".to_owned(), "email".to_owned()],
    ] {
        store
            .upsert_consent(ConsentRecord {
                id: String::new(),
                realm_id: R.into(),
                org_id: None,
                user_id: "u3".into(),
                client_id: "portal".into(),
                granted_client_scopes: scopes,
                created_date: 1,
                last_updated_date: 2,
            })
            .await
            .expect("consent");
    }
    let consents = store.list_consents(R, "u3").await.expect("list");
    assert_eq!(consents.len(), 1);
    assert_eq!(consents[0].granted_client_scopes.len(), 2);
    assert!(!consents[0].id.is_empty());
    assert!(
        store
            .delete_consent(R, "u3", "portal")
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete_consent(R, "u3", "portal")
            .await
            .expect("again")
    );
    assert!(
        store
            .get_refresh_family(R, "f5")
            .await
            .expect("get")
            .expect("f5")
            .revoked,
        "revoking consent ends the client's refresh tokens"
    );
}

async fn config_events_and_sweeper(store: &dyn Store) {
    for name in ["Configure OTP", "Configure OTP (again)"] {
        store
            .upsert_required_action(RequiredActionConfig {
                realm_id: R.into(),
                name: name.into(),
                ..Default::default()
            })
            .await
            .expect("required action");
    }
    let ras = store.list_required_actions(R).await.expect("list");
    assert_eq!(ras.len(), 1);
    assert_eq!(ras[0].id, format!("{R}:CONFIGURE_TOTP"));

    store
        .upsert_branding(BrandingBlock {
            id: "brand-realm".into(),
            realm_id: R.into(),
            instance_name: "Realm look".into(),
            ..Default::default()
        })
        .await
        .expect("realm branding");
    store
        .upsert_branding(BrandingBlock {
            id: "brand-hel".into(),
            realm_id: R.into(),
            org_id: Some("hel".into()),
            instance_name: "Helsinki look".into(),
            colours: Colours {
                primary: "javascript:alert(1)".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .await
        .expect("org branding");
    let hel = store
        .get_branding(R, Some("hel"))
        .await
        .expect("get")
        .expect("hel");
    assert_eq!(hel.instance_name, "Helsinki look");
    assert!(
        hel.colours.primary.starts_with('#'),
        "invalid colours fall back"
    );
    let other = store
        .get_branding(R, Some("zil"))
        .await
        .expect("get")
        .expect("realm");
    assert_eq!(other.instance_name, "Realm look");
    let realm = store
        .get_branding(R, None)
        .await
        .expect("get")
        .expect("realm");
    assert_eq!(realm.instance_name, "Realm look");
    assert!(
        store
            .get_branding("other-realm", None)
            .await
            .expect("get")
            .is_none()
    );

    for (org_id, kind) in [
        (Some("hel"), "LOGIN"),
        (Some("zil"), "LOGIN_ERROR"),
        (None, "CODE_TO_TOKEN"),
    ] {
        store
            .append_event(AuditEvent {
                id: String::new(),
                realm_id: R.into(),
                org_id: org_id.map(str::to_owned),
                time: 1,
                event_type: kind.into(),
                client_id: None,
                user_id: None,
                session_id: None,
                ip_address: "192.0.2.1".into(),
                error: None,
                details: BTreeMap::new(),
            })
            .await
            .expect("event");
        store
            .append_admin_event(AdminEvent {
                id: String::new(),
                realm_id: R.into(),
                org_id: org_id.map(str::to_owned),
                time: 1,
                operation_type: "CREATE".into(),
                resource_type: "USER".into(),
                resource_path: format!("users/{kind}"),
                actor_id: "root".into(),
                actor_client_id: None,
                representation: None,
            })
            .await
            .expect("admin event");
    }
    let all = store.list_events(R, None).await.expect("events");
    assert_eq!(
        all.iter()
            .map(|e| e.event_type.as_str())
            .collect::<Vec<_>>(),
        ["LOGIN", "LOGIN_ERROR", "CODE_TO_TOKEN"],
        "events keep their order"
    );
    assert!(all.iter().all(|e| !e.id.is_empty()));
    let hel_only = store
        .list_events(R, Some(&["hel".to_owned()]))
        .await
        .expect("events");
    assert_eq!(hel_only.len(), 1);
    assert_eq!(
        store.list_admin_events(R, None).await.expect("admin").len(),
        3
    );
    assert_eq!(
        store
            .list_admin_events(R, Some(&["zil".to_owned()]))
            .await
            .expect("admin")
            .len(),
        1
    );

    // Every entity set answers from the store.
    let ctx = admin_ctx();
    for set in [
        "Realms",
        "Organizations",
        "OrganizationDomains",
        "People",
        "Memberships",
        "Groups",
        "GroupMembers",
        "Roles",
        "RoleMappings",
        "Applications",
        "ClientScopes",
        "ProtocolMappers",
        "IdentityProviders",
        "Credentials",
        "Sessions",
        "ClientSessions",
        "OfflineSessions",
        "Consents",
        "Events",
        "AdminEvents",
        "SigningKeys",
        "RequiredActions",
        "BrandingBlocks",
    ] {
        let ans = store
            .query_entity_set(
                &ctx,
                &LocalAuthorizer,
                set,
                &odata_table::QueryOptions {
                    count: Some(true),
                    ..Default::default()
                },
                1000,
            )
            .await
            .unwrap_or_else(|e| panic!("entity set {set}: {e}"));
        assert_eq!(ans.count, Some(ans.rows.len() as u64), "set {set}");
    }
    let people = store
        .query_entity_set(&ctx, &LocalAuthorizer, "People", &Default::default(), 1000)
        .await
        .expect("people");
    assert_eq!(people.rows.len(), 3, "transient people are not an entity");
    assert!(matches!(
        store
            .query_entity_set(&ctx, &LocalAuthorizer, "Nope", &Default::default(), 10)
            .await,
        Err(KaisatsuError::NotFound(_))
    ));

    // The sweeper removes what expired above and keeps the rest.
    let swept = store.sweep_expired().await.expect("sweep");
    assert!(
        swept >= 3,
        "expired auth session, PAR request and revoked token, got {swept}"
    );
    assert!(store.is_token_revoked(R, "jti-1").await.expect("is"));
}

fn device(device: &str, user_code: &str, client: &str, ttl: i64) -> DeviceCodeRecord {
    let now = TimestampTz::now();
    DeviceCodeRecord {
        device_code_hash: device.into(),
        user_code_hash: user_code.into(),
        realm_id: R.into(),
        client_id: client.into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        status: DeviceStatus::Pending,
        user_id: None,
        sso_session_id: None,
        acr: String::new(),
        amr: Vec::new(),
        auth_time: 0,
        interval_secs: 5,
        last_poll_at: None,
        consumed: false,
        created_at: now,
        expires_at: now + SignedDuration::from_secs(ttl),
    }
}

/// RFC 8628 device codes (D1): unique user codes, polls only by the client the code was issued
/// to, `slow_down` on a poll inside the interval, one answer per code, single-use tokens.
async fn device_codes(store: &dyn Store) {
    let now = TimestampTz::now();
    store
        .put_device_code(device("dev-a", "user-a", "jcctl", 600))
        .await
        .expect("put");
    assert!(
        matches!(
            store
                .put_device_code(device("dev-b", "user-a", "jcctl", 600))
                .await,
            Err(KaisatsuError::Conflict(_))
        ),
        "a user code two pending codes share"
    );
    let got = store
        .get_device_code_by_user_code(R, "user-a")
        .await
        .expect("get")
        .expect("found");
    assert_eq!(got.device_code_hash, "dev-a");
    assert!(
        store
            .get_device_code_by_user_code("other-realm", "user-a")
            .await
            .unwrap_or(None)
            .is_none(),
        "another realm's user code"
    );

    // Another client's poll is no poll: nothing answered, nothing changed.
    assert!(
        store
            .poll_device_code(R, "dev-a", "intruder", now)
            .await
            .expect("poll")
            .is_none()
    );
    let (_, first) = store
        .poll_device_code(R, "dev-a", "jcctl", now)
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(first, DevicePoll::Pending);
    let (_, fast) = store
        .poll_device_code(R, "dev-a", "jcctl", now + SignedDuration::from_secs(1))
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(fast, DevicePoll::SlowDown);
    let after = store
        .get_device_code_by_user_code(R, "user-a")
        .await
        .expect("get")
        .expect("found");
    assert_eq!(after.interval_secs, 10, "slow_down adds 5 s");

    let mut approved = after.clone();
    approved.status = DeviceStatus::Approved;
    approved.user_id = Some("u1".into());
    assert!(
        store
            .decide_device_code(approved.clone())
            .await
            .expect("decide")
    );
    let mut denied = after;
    denied.status = DeviceStatus::Denied;
    assert!(
        !store.decide_device_code(denied).await.expect("decide"),
        "a code answers once"
    );
    let (before, ok) = store
        .poll_device_code(R, "dev-a", "jcctl", now + SignedDuration::from_secs(30))
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(ok, DevicePoll::Approved);
    assert_eq!(before.user_id.as_deref(), Some("u1"));
    let (_, again) = store
        .poll_device_code(R, "dev-a", "jcctl", now + SignedDuration::from_secs(60))
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(again, DevicePoll::Used, "the tokens go once");

    store
        .put_device_code(device("dev-c", "user-c", "jcctl", -1))
        .await
        .expect("put");
    let (_, expired) = store
        .poll_device_code(R, "dev-c", "jcctl", now)
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(expired, DevicePoll::Expired);

    store
        .put_device_code(device("dev-d", "user-d", "jcctl", 600))
        .await
        .expect("put");
    let mut no = store
        .get_device_code_by_user_code(R, "user-d")
        .await
        .expect("get")
        .expect("found");
    no.status = DeviceStatus::Denied;
    assert!(store.decide_device_code(no).await.expect("decide"));
    let (_, denied) = store
        .poll_device_code(R, "dev-d", "jcctl", now)
        .await
        .expect("poll")
        .expect("known");
    assert_eq!(denied, DevicePoll::Denied);
}

/// T-7525: the paged list calls answer the same in SQL as in memory.
async fn paged_lists(store: &dyn Store) {
    const P: &str = "paging";
    store
        .upsert_realm(RealmConfig {
            id: P.into(),
            realm: P.into(),
            ..Default::default()
        })
        .await
        .expect("realm");
    let person = |id: &str, name: &str, email: Option<&str>| UserRepresentation {
        realm_id: P.into(),
        ..user(id, name, email, None)
    };
    for u in [
        person("a", "Anna", Some("Anna@City.Example")),
        person("b", "bea%_", None),
        UserRepresentation {
            first_name: Some("\u{c4}rne".into()),
            ..person("c", "carl", Some("carl@city.example"))
        },
        UserRepresentation {
            service_account_client_id: Some("sync".into()),
            ..person("s", "service-account-sync", None)
        },
    ] {
        store.create_user(u).await.expect("person");
    }

    type Query = (
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        bool,
    );
    let queries: [(Query, &[&str]); 14] = [
        ((None, None, None, false), &["a", "b", "c", "s"]),
        // A search given, also an empty one, lists people only.
        ((Some(""), None, None, false), &["a", "b", "c"]),
        ((Some("service"), None, None, false), &[]),
        ((Some("ANNA@city"), None, None, false), &["a"]),
        ((Some("%_"), None, None, false), &["b"]),
        ((Some("\u{c4}RNE"), None, None, false), &["c"]),
        // ASCII case only, as the stores have always compared.
        ((Some("\u{e4}rne"), None, None, false), &[]),
        ((Some("a\0"), None, None, false), &[]),
        ((None, Some("ANNA@CITY.EXAMPLE"), None, true), &["a"]),
        ((None, Some(""), None, true), &["b", "s"]),
        ((None, Some(""), None, false), &["a", "b", "c", "s"]),
        ((None, Some("city"), None, true), &[]),
        ((None, None, Some("ANNA"), true), &["a"]),
        ((None, None, Some("' OR true --"), false), &[]),
    ];
    for ((search, email, name, exact), expected) in queries {
        let found = store
            .list_users(P, None, search, email, name, exact, 0, 100)
            .await
            .expect("list");
        let ids: Vec<&str> = found.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, expected, "{search:?} {email:?} {name:?} exact={exact}");
    }
    let second = store
        .list_users(P, None, Some(""), None, None, false, 1, 1)
        .await
        .expect("second");
    assert_eq!(second[0].id, "b", "first counts people only");

    use MatchOp::{Contains, Equals, StartsWith};
    use PersonField::{Email, FirstName, Id, LastName, Username};
    type Conditions = &'static [(PersonField, MatchOp, &'static str)];
    let filters: [(Conditions, &[&str]); 8] = [
        (&[], &["a", "b", "c", "s"]),
        (&[(Username, Equals, "ANNA")], &["a"]),
        (&[(Email, Equals, "")], &["b", "s"]),
        (&[(Email, StartsWith, "CARL@")], &["c"]),
        (&[(FirstName, Contains, "RNE")], &["c"]),
        (&[(LastName, Equals, "last-b"), (Id, Equals, "B")], &["b"]),
        (
            &[(Username, Contains, "a"), (Email, Contains, "city")],
            &["a", "c"],
        ),
        (&[(Username, Contains, "a\0")], &[]),
    ];
    for (conditions, expected) in filters {
        let filter: Vec<FieldMatch<PersonField>> = conditions
            .iter()
            .map(|(field, op, value)| FieldMatch {
                field: *field,
                op: *op,
                value: (*value).into(),
            })
            .collect();
        let all = store
            .find_users(P, None, &filter, 0, 100)
            .await
            .expect("find");
        let ids: Vec<&str> = all.items.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(
            (ids, all.total),
            (expected.to_vec(), expected.len()),
            "{filter:?}"
        );
        let none = store
            .find_users(P, None, &filter, 0, 0)
            .await
            .expect("count");
        assert_eq!((none.items.len(), none.total), (0, expected.len()));
    }

    let group = |id: &str, org: &str, parent: Option<&str>, path: &str| GroupRepresentation {
        id: id.into(),
        realm_id: P.into(),
        org_id: Some(org.into()),
        parent_id: parent.map(str::to_owned),
        name: path.rsplit('/').next().unwrap_or_default().into(),
        path: path.into(),
        ..Default::default()
    };
    for g in [
        group("h", "hel", None, "/hel"),
        group("hm", "hel", Some("h"), "/hel/members"),
        // No parentId: the path alone places it.
        group("hb", "hel", None, "/hel/members/board"),
        group("hx", "hel", None, "/helper"),
        group("z", "zil", None, "/zil"),
        group("zm", "zil", Some("z"), "/zil/members"),
    ] {
        store.upsert_group(g).await.expect("group");
    }
    let (hel, zil) = (["hel".to_owned()], ["zil".to_owned()]);
    let exact = store
        .list_groups(P, None, Some("MEMBERS"), true)
        .await
        .expect("exact");
    assert_eq!(exact.len(), 2);
    assert_eq!(exact[0].sub_groups[0].id, "hm");
    assert!(exact[0].sub_groups[0].sub_groups.is_empty());
    let deep = store
        .list_groups(P, None, Some("oar"), false)
        .await
        .expect("deep");
    assert_eq!(deep.len(), 1);
    assert_eq!(deep[0].sub_groups[0].sub_groups[0].id, "hb");

    let root = store
        .get_group(P, None, "h")
        .await
        .expect("get")
        .expect("root");
    let below = store
        .list_child_groups(None, &root)
        .await
        .expect("children");
    assert_eq!(below.len(), 1, "/helper is no child of /hel");
    assert_eq!(below[0].sub_groups[0].id, "hb");
    assert!(
        store
            .list_child_groups(Some(&zil), &root)
            .await
            .expect("scoped children")
            .is_empty()
    );

    store.add_user_to_group(P, "a", "hm").await.expect("member");
    store.add_user_to_group(P, "c", "hm").await.expect("member");
    // A path in the person's own document adds no second membership and is not kept.
    let mut named = store
        .get_user_by_id(P, None, "c")
        .await
        .expect("get")
        .expect("c");
    named.groups.push("/hel/members".into());
    store.update_user(named).await.expect("named member");
    let members_group = [FieldMatch {
        field: GroupField::Name,
        op: Equals,
        value: "members".into(),
    }];
    let mine = store
        .find_groups(P, Some(&hel), &members_group, 0, 100)
        .await
        .expect("find groups");
    assert_eq!(
        mine.total, 1,
        "another Organization's nested group stays out"
    );
    assert_eq!(mine.items[0].group.id, "hm");
    let ids: Vec<&str> = mine.items[0]
        .members
        .iter()
        .map(|u| u.id.as_str())
        .collect();
    assert_eq!(ids, ["a", "c"]);
    let page = store.find_groups(P, None, &[], 1, 2).await.expect("page");
    assert_eq!(page.total, 6);
    let ids: Vec<&str> = page.items.iter().map(|g| g.group.id.as_str()).collect();
    assert_eq!(ids, ["hm", "hb"], "flat, ordered by path");

    for (realm, id, name) in [
        (P, "1", "endpoint:abcdefgh"),
        (P, "2", "endpoint:qrstuvwx"),
        (P, "3", "endpoints"),
        (R, "4", "endpoint:otherrlm"),
    ] {
        store
            .upsert_client_scope(ClientScopeRepresentation {
                id: id.into(),
                realm_id: realm.into(),
                name: name.into(),
                ..Default::default()
            })
            .await
            .expect("scope");
    }
    let keys = ["endpoint:abcdefgh", "3", "endpoint:otherrlm", "4", "a\0"].map(str::to_owned);
    let mut ids: Vec<String> = store
        .get_client_scopes(P, &keys)
        .await
        .expect("by key")
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["1", "3"], "by name or id, never another realm's");
    let mut ids: Vec<String> = store
        .list_client_scopes_with_prefix(P, "endpoint:")
        .await
        .expect("by prefix")
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["1", "2"]);
    store.delete_client_scope(R, "4").await.expect("clean up");

    // A group past one page is read whole with `first`; one call stops at MAX_PAGE.
    let crowd = store
        .upsert_group(group("crowd", "hel", None, "/crowd"))
        .await
        .expect("crowd");
    for n in 0..1201 {
        let id = format!("m{n}");
        store
            .create_user(person(&id, &format!("member-{n:04}"), None))
            .await
            .expect("crowd member");
        store
            .add_user_to_group(P, &id, &crowd.id)
            .await
            .expect("crowd membership");
    }
    let page = |first, max| store.list_group_members(P, None, "crowd", first, max);
    let mut seen = std::collections::BTreeSet::new();
    for first in [0, 500, 1000] {
        seen.extend(
            page(first, 500)
                .await
                .expect("members")
                .into_iter()
                .map(|u| u.id),
        );
    }
    assert_eq!(seen.len(), 1201);
    assert_eq!(page(0, usize::MAX).await.expect("clamp").len(), MAX_PAGE);
    assert!(page(usize::MAX, 10).await.expect("past the end").is_empty());
    let one_call = store
        .list_users(P, None, None, None, None, false, 0, usize::MAX)
        .await
        .expect("clamp");
    assert_eq!(one_call.len(), MAX_PAGE);
    let whole = listing::all_users(store, P).await.expect("every person");
    assert_eq!(whole.len(), 1205);
    let whole = listing::all_group_members(store, P, "crowd")
        .await
        .expect("every member");
    assert_eq!(whole.len(), 1201);
}

async fn run_contract(store: Arc<dyn Store>) {
    realms_orgs_and_memberships(store.as_ref()).await;
    people_and_credentials(store.as_ref()).await;
    groups(store.as_ref()).await;
    roles_and_clients(store.as_ref()).await;
    sessions_and_tokens(store.as_ref()).await;
    device_codes(store.as_ref()).await;
    config_events_and_sweeper(store.as_ref()).await;
    paged_lists(store.as_ref()).await;
}

#[tokio::test]
async fn memory_store_meets_the_store_contract() {
    run_contract(Arc::new(MemoryStore::default())).await;
}

#[tokio::test]
async fn pg_store_meets_the_store_contract() {
    let Some(db) = common::test_db("pg_store_meets_the_store_contract").await else {
        return;
    };
    run_contract(Arc::new(db.store(Kek::generate_for_tests()).await)).await;
    db.drop_db().await;
}
