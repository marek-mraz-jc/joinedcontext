#![forbid(unsafe_code)]
//! The `LocalAuthorizer` decision table (W6): which role may do what on which object, where a
//! refusal is a hidden 404 and where a visible 403, self-service, realms and active organizations.

use kaisatsu_core::authz::*;
use std::collections::HashMap;

fn person(org: Option<&str>, id: &str) -> ObjectTarget {
    ObjectTarget::Person {
        realm_id: "dev".into(),
        org_id: org.map(str::to_owned),
        person_id: id.into(),
    }
}

fn targets(org: Option<&str>) -> Vec<ObjectTarget> {
    let o = org.map(str::to_owned);
    vec![
        person(org, "p1"),
        ObjectTarget::Group {
            realm_id: "dev".into(),
            org_id: o.clone(),
            group_id: "g1".into(),
            managed_by: None,
        },
        ObjectTarget::Role {
            realm_id: "dev".into(),
            org_id: o.clone(),
            role_id: "r1".into(),
        },
        ObjectTarget::ClientScope {
            realm_id: "dev".into(),
            org_id: o.clone(),
            scope_id: "s1".into(),
        },
        ObjectTarget::Application {
            realm_id: "dev".into(),
            org_id: o.clone(),
            client_id: "c1".into(),
            managed_by: None,
        },
        ObjectTarget::Session {
            realm_id: "dev".into(),
            org_id: o.clone(),
            person_id: "p1".into(),
        },
        ObjectTarget::Audit {
            realm_id: "dev".into(),
            org_id: o,
        },
    ]
}

fn org_member(org: &str, roles: &[&str]) -> Principal {
    Principal {
        subject: Some("member".into()),
        client_id: Some("portal".into()),
        realm_roles: vec![],
        client_roles: HashMap::new(),
        organizations: vec![org.into()],
        org_roles: HashMap::from([(org.into(), roles.iter().map(|r| (*r).to_owned()).collect())]),
        second_factor: false,
    }
}

fn mgmt(roles: &[&str]) -> Principal {
    let mut p = Principal::anonymous();
    p.subject = Some("svc".into());
    p.client_roles.insert(
        "realm-management".into(),
        roles.iter().map(|r| (*r).to_owned()).collect(),
    );
    p
}

fn check(p: &Principal, org: Option<&str>, action: Action, obj: &ObjectTarget) -> Decision {
    let mut ctx = RequestCtx::new("dev", p.clone());
    if let Some(o) = org {
        ctx = ctx.with_org(o);
    }
    LocalAuthorizer.check(&ctx, action, obj)
}

/// For each target kind in `targets()` order: (read allowed, write allowed).
fn expect(p: &Principal, active: Option<&str>, org: Option<&str>, want: [(bool, bool); 7]) {
    for (obj, (read, write)) in targets(org).iter().zip(want) {
        let r = check(p, active, Action::Read, obj);
        assert_eq!(r.allowed, read, "read {obj:?}: {}", r.reason);
        for a in [Action::Create, Action::Update, Action::Delete] {
            let w = check(p, active, a, obj);
            assert_eq!(w.allowed, write, "{} {obj:?}: {}", a.as_str(), w.reason);
            if !w.allowed {
                assert!(!w.visible, "an object refusal is a 404: {obj:?}");
            }
        }
    }
}

#[test]
fn organization_roles() {
    let all = [(true, true); 7];
    expect(&org_member("hel", &["org-admin"]), None, Some("hel"), all);
    expect(
        &org_member("hel", &["org-admin"]),
        Some("hel"),
        Some("hel"),
        all,
    );
    expect(
        &org_member("hel", &["people-manager"]),
        None,
        Some("hel"),
        [
            (true, true),
            (true, true),
            (false, false),
            (false, false),
            (false, false),
            (true, true),
            (true, true),
        ],
    );
    expect(
        &org_member("hel", &["application-manager"]),
        None,
        Some("hel"),
        [
            (false, false),
            (false, false),
            (true, true),
            (true, true),
            (true, true),
            (false, false),
            (false, false),
        ],
    );
    expect(
        &org_member("hel", &["auditor"]),
        None,
        Some("hel"),
        [
            (true, false),
            (false, false),
            (false, false),
            (false, false),
            (false, false),
            (true, false),
            (true, false),
        ],
    );
    expect(
        &org_member("hel", &[]),
        None,
        Some("hel"),
        [(false, false); 7],
    );
    // Another organization's objects, a realm-level object, or another active org: hidden.
    let admin = org_member("hel", &["org-admin"]);
    expect(&admin, None, Some("tre"), [(false, false); 7]);
    expect(&admin, None, None, [(false, false); 7]);
    expect(&admin, Some("tre"), Some("hel"), [(false, false); 7]);
    // An org role held for an organization the caller is not a member of counts for nothing.
    let mut stale = org_member("hel", &[]);
    stale
        .org_roles
        .insert("tre".into(), vec!["org-admin".into()]);
    stale.organizations.push("tre".into());
    expect(&stale, None, Some("tre"), [(true, true); 7]);
    stale.organizations.retain(|o| o != "tre");
    expect(&stale, None, Some("tre"), [(false, false); 7]);
}

#[test]
fn realm_management_roles() {
    expect(
        &mgmt(&["manage-users"]),
        None,
        None,
        [
            (true, true),
            (true, true),
            (false, false),
            (false, false),
            (false, false),
            (true, true),
            (true, true),
        ],
    );
    expect(
        &mgmt(&["query-users"]),
        None,
        None,
        [
            (true, false),
            (false, false),
            (false, false),
            (false, false),
            (false, false),
            (false, false),
            (false, false),
        ],
    );
    expect(
        &mgmt(&["query-groups"]),
        None,
        None,
        [
            (false, false),
            (true, false),
            (true, false),
            (true, false),
            (false, false),
            (false, false),
            (false, false),
        ],
    );
    // manage-clients is not one of the realm-wide service roles: it acts inside the caller's
    // organizations only.
    expect(&mgmt(&["manage-clients"]), None, None, [(false, false); 7]);
    let mut clients_mgr = mgmt(&["manage-clients"]);
    clients_mgr.organizations = vec!["hel".into()];
    expect(
        &clients_mgr,
        Some("hel"),
        Some("hel"),
        [
            (false, false),
            (false, false),
            (true, true),
            (true, true),
            (true, true),
            (false, false),
            (false, false),
        ],
    );
    let mut viewer = mgmt(&["view-users", "view-events", "scim"]);
    viewer.organizations = vec!["hel".into()];
    expect(
        &viewer,
        None,
        Some("hel"),
        [
            (true, true),
            (true, true),
            (false, false),
            (false, false),
            (false, false),
            (true, false),
            (true, false),
        ],
    );
    // view-clients (portal-api, T-2725) writes only applications managed by joinedcontext.
    let portal = mgmt(&["view-clients"]);
    let app = |managed: Option<&str>| ObjectTarget::Application {
        realm_id: "dev".into(),
        org_id: None,
        client_id: "c".into(),
        managed_by: managed.map(str::to_owned),
    };
    for (managed, write) in [
        (None, true),
        (Some("joinedcontext"), true),
        (Some("gitops"), false),
    ] {
        assert!(check(&portal, None, Action::Read, &app(managed)).allowed);
        assert_eq!(
            check(&portal, None, Action::Update, &app(managed)).allowed,
            write,
            "{managed:?}"
        );
    }
    // Pinned to an organization, a realm-wide service account is scoped like a member.
    assert!(
        !check(
            &mgmt(&["manage-users"]),
            Some("hel"),
            Action::Read,
            &person(Some("hel"), "x")
        )
        .allowed
    );
}

#[test]
fn realm_admins_realm_appliers_and_the_realm_object() {
    let admin = Principal::realm_admin("root");
    assert!(admin.is_realm_admin() && admin.second_factor);
    expect(&admin, None, None, [(true, true); 7]);
    expect(&admin, None, Some("any"), [(true, true); 7]);
    expect(&admin, Some("hel"), Some("hel"), [(true, true); 7]);
    expect(&admin, Some("hel"), Some("tre"), [(false, false); 7]);
    expect(&admin, Some("hel"), None, [(true, true); 7]);
    let mut kc_admin = Principal::anonymous();
    kc_admin.subject = Some("kc".into());
    kc_admin
        .client_roles
        .insert("realm-management".into(), vec!["realm-admin".into()]);
    assert!(kc_admin.is_realm_admin() && kc_admin.is_realm_applier());
    let mut department_admin = org_member("hel", &[]);
    department_admin.realm_roles = vec!["admin".into(), "platform-admin".into()];
    assert!(!department_admin.is_realm_admin());

    let realm = ObjectTarget::Realm {
        realm_id: "dev".into(),
    };
    assert!(check(&admin, None, Action::Apply, &realm).allowed);
    let mut applier = Principal::anonymous();
    applier.subject = Some("ci".into());
    applier.realm_roles = vec!["realm-apply".into()];
    assert!(check(&applier, None, Action::Apply, &realm).allowed);
    assert!(check(&applier, None, Action::Read, &realm).allowed);
    let d = check(&applier, None, Action::Update, &realm);
    assert!(!d.allowed && d.visible, "a realm refusal is a visible 403");
    let manage_realm = mgmt(&["manage-realm"]);
    assert!(manage_realm.is_realm_applier());
    assert!(check(&manage_realm, None, Action::Apply, &realm).allowed);
    // Reaching realm-level objects gives an applier no rights on them, nor on organizations.
    expect(&applier, None, None, [(false, false); 7]);
    expect(&applier, None, Some("hel"), [(false, false); 7]);
    let d = check(&applier, Some("hel"), Action::Apply, &realm);
    assert!(
        !d.allowed && !d.visible,
        "pinned to an organization the realm is out of reach"
    );
    // A pinned non-member manager is scoped like a member: nothing in that organization.
    expect(
        &mgmt(&["manage-clients"]),
        Some("hel"),
        Some("hel"),
        [(false, false); 7],
    );
    // An org caller never sees the realm object.
    let d = check(
        &org_member("hel", &["org-admin"]),
        None,
        Action::Read,
        &realm,
    );
    assert!(!d.allowed && !d.visible);
    // A realm-wide service account reaches the realm object but may not change it.
    let d = check(&mgmt(&["manage-users"]), None, Action::Apply, &realm);
    assert!(!d.allowed && d.visible);
}

#[test]
fn organization_objects_and_self_service() {
    let org = ObjectTarget::Organization {
        realm_id: "dev".into(),
        org_id: "hel".into(),
    };
    assert!(
        check(
            &org_member("hel", &["org-admin"]),
            None,
            Action::Update,
            &org
        )
        .allowed
    );
    assert!(check(&org_member("hel", &[]), None, Action::Read, &org).allowed);
    let d = check(&org_member("hel", &[]), None, Action::Update, &org);
    assert!(!d.allowed && !d.visible);
    assert!(!check(&org_member("tre", &["org-admin"]), None, Action::Read, &org).allowed);

    let mut me = org_member("hel", &[]);
    me.subject = Some("p1".into());
    for a in [Action::Read, Action::SelfService] {
        assert!(check(&me, None, a, &person(None, "p1")).allowed);
    }
    assert!(!check(&me, None, Action::Update, &person(Some("hel"), "p1")).allowed);
    let own_session = ObjectTarget::Session {
        realm_id: "dev".into(),
        org_id: None,
        person_id: "p1".into(),
    };
    for a in [Action::Read, Action::SelfService, Action::Delete] {
        assert!(check(&me, None, a, &own_session).allowed, "{a:?}");
    }
    assert!(!check(&me, None, Action::Create, &own_session).allowed);
    assert!(!check(&me, None, Action::Read, &person(Some("hel"), "p2")).allowed);
}

#[test]
fn anonymous_callers_and_other_realms_are_hidden() {
    let anon = Principal::anonymous();
    for obj in targets(Some("hel")) {
        let d = check(&anon, None, Action::Read, &obj);
        assert!(!d.allowed && !d.visible && d.reason == "unauthenticated");
    }
    let admin = Principal::realm_admin("root");
    let other = ObjectTarget::Audit {
        realm_id: "other".into(),
        org_id: None,
    };
    let d = check(&admin, None, Action::Read, &other);
    assert!(!d.allowed && !d.visible);
    assert_eq!(other.realm_id(), "other");
    assert_eq!(other.org_id(), None);
    let org = ObjectTarget::Organization {
        realm_id: "dev".into(),
        org_id: "hel".into(),
    };
    assert_eq!(org.org_id(), Some("hel"));
    assert_eq!(
        ObjectTarget::Realm {
            realm_id: "dev".into()
        }
        .org_id(),
        None
    );
}

#[test]
fn allowed_orgs_and_action_names() {
    let ctx = |p: Principal, org: Option<&str>| {
        let c = RequestCtx::new("dev", p);
        match org {
            Some(o) => c.with_org(o),
            None => c,
        }
    };
    assert_eq!(ctx(Principal::realm_admin("a"), None).allowed_orgs(), None);
    assert_eq!(
        ctx(Principal::realm_admin("a"), Some("hel")).allowed_orgs(),
        Some(vec!["hel".to_owned()])
    );
    for role in [
        "manage-users",
        "view-clients",
        "query-groups",
        "query-users",
    ] {
        assert_eq!(ctx(mgmt(&[role]), None).allowed_orgs(), None, "{role}");
    }
    assert_eq!(
        ctx(org_member("hel", &[]), Some("hel")).allowed_orgs(),
        Some(vec!["hel".to_owned()])
    );
    assert_eq!(
        ctx(org_member("hel", &[]), Some("tre")).allowed_orgs(),
        Some(vec![])
    );
    assert_eq!(
        ctx(org_member("hel", &[]), None).allowed_orgs(),
        Some(vec!["hel".to_owned()])
    );
    let names: Vec<_> = [
        Action::Read,
        Action::Create,
        Action::Update,
        Action::Delete,
        Action::Apply,
        Action::SelfService,
    ]
    .iter()
    .map(|a| (a.as_str(), a.is_write()))
    .collect();
    assert_eq!(
        names,
        vec![
            ("read", false),
            ("create", true),
            ("update", true),
            ("delete", true),
            ("apply", true),
            ("self", true)
        ]
    );
    assert!(Decision::deny_visible("x").visible);
    assert!(Decision::allow("x").allowed);
}
