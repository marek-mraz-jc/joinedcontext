#![forbid(unsafe_code)]
//! Every protocol mapper kind on every token kind (W1): which claims each one writes, the
//! token-kind switches, nested claim paths, composite role cycles and the refusal of unknown kinds.

use kaisatsu_core::claims::{ActClaim, is_known_mapper_kind, resolve_effective_realm_roles};
use kaisatsu_core::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

fn mapper(kind: &str, cfg: &[(&str, &str)]) -> ProtocolMapperRepresentation {
    ProtocolMapperRepresentation {
        name: format!("{kind}-{}", cfg.len()),
        protocol: "openid-connect".into(),
        protocol_mapper: kind.into(),
        config: cfg
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        ..Default::default()
    }
}

fn user() -> UserRepresentation {
    UserRepresentation {
        id: "u-1".into(),
        username: "alice".into(),
        email: Some("alice@hel.fi".into()),
        email_verified: true,
        first_name: Some("Alice".into()),
        last_name: Some("Liddell".into()),
        realm_roles: vec!["editor".into()],
        client_roles: HashMap::from([
            ("portal-api".into(), vec!["reader".into()]),
            ("empty".into(), vec![]),
        ]),
        groups: vec!["hel/team".into(), "/top".into()],
        attributes: HashMap::from([
            ("phone".into(), vec!["+358".into(), "+359".into()]),
            ("jc_org".into(), vec!["user-org".into()]),
        ]),
        ..Default::default()
    }
}

fn roles() -> Vec<RoleRepresentation> {
    let role = |name: &str, children: &[&str]| RoleRepresentation {
        name: name.into(),
        composites: RoleComposites {
            realm: children.iter().map(|c| (*c).to_owned()).collect(),
            ..Default::default()
        },
        ..Default::default()
    };
    vec![
        role("editor", &["viewer", "editor"]),
        role("viewer", &["editor", "auditor"]),
        role("auditor", &[]),
        RoleRepresentation {
            name: "auditor-client".into(),
            client_role: true,
            ..Default::default()
        },
    ]
}

struct Fixture {
    realm: RealmConfig,
    client: ClientRepresentation,
    user: UserRepresentation,
    groups: Vec<GroupRepresentation>,
    roles: Vec<RoleRepresentation>,
    scopes: Vec<ClientScopeRepresentation>,
    amr: Vec<String>,
}

impl Fixture {
    fn new(client_mappers: Vec<ProtocolMapperRepresentation>) -> Self {
        Self {
            realm: RealmConfig::default(),
            client: ClientRepresentation {
                client_id: "edge".into(),
                full_scope_allowed: true,
                web_origins: vec!["https://portal.example".into()],
                protocol_mappers: client_mappers,
                ..Default::default()
            },
            user: user(),
            groups: vec![
                GroupRepresentation {
                    name: "team".into(),
                    path: "hel/team".into(),
                    realm_roles: vec!["auditor".into()],
                    attributes: HashMap::from([
                        ("jc_org".into(), vec!["hel".into(), "hel".into()]),
                        ("phone".into(), vec!["+1".into()]),
                    ]),
                    ..Default::default()
                },
                GroupRepresentation {
                    name: "ops".into(),
                    path: "/ops".into(),
                    ..Default::default()
                },
            ],
            roles: roles(),
            scopes: vec![ClientScopeRepresentation {
                name: "profile".into(),
                ..Default::default()
            }],
            amr: vec!["pwd".into(), "otp".into()],
        }
    }

    fn ctx(&self) -> MapperContext<'_> {
        MapperContext {
            realm: &self.realm,
            issuer: "https://idm.example/realms/dev",
            client: &self.client,
            user: &self.user,
            user_groups: &self.groups,
            all_realm_roles: &self.roles,
            active_scopes: &self.scopes,
            granted_scope_string: "openid email roles",
            acr: "mfa",
            amr: &self.amr,
            session_id: Some("sid-1"),
            nonce: Some("n-1"),
            jti: "jti-1",
            iat: 10,
            exp: 20,
            auth_time: Some(9),
            dpop_jkt: Some("thumb"),
            act: Some(ActClaim {
                sub: "svc".into(),
                act: Some(Box::new(ActClaim {
                    sub: "svc2".into(),
                    act: None,
                })),
            }),
            active_org: Some("pinned-org"),
        }
    }

    fn eval(&self, kind: TokenKind) -> serde_json::Map<String, Value> {
        evaluate_mappers(&self.ctx(), kind).unwrap()
    }
}

#[test]
fn built_in_claims_differ_per_token_kind() {
    let f = Fixture::new(vec![]);
    let at = f.eval(TokenKind::AccessToken);
    assert_eq!(at["typ"], "Bearer");
    assert_eq!(at["scope"], "openid email roles");
    assert_eq!(at["cnf"], json!({"jkt": "thumb"}));
    assert_eq!(at["act"], json!({"sub":"svc","act":{"sub":"svc2"}}));
    assert_eq!(at["sid"], "sid-1");
    assert_eq!(at["auth_time"], 9);
    assert_eq!(at["acr"], "mfa");
    assert_eq!(at["amr"], json!(["pwd", "otp"]));
    assert!(at.get("nonce").is_none());
    assert!(at.get("aud").is_none());
    assert_eq!(at["name"], "Alice Liddell");
    assert_eq!(at["given_name"], "Alice");
    assert_eq!(at["family_name"], "Liddell");
    assert_eq!(at["preferred_username"], "alice");
    assert_eq!(at["email"], "alice@hel.fi");
    assert_eq!(at["email_verified"], true);
    // Composite roles resolve through the cycle editor -> viewer -> editor, plus group roles.
    assert_eq!(
        at["realm_access"]["roles"],
        json!(["auditor", "editor", "viewer"])
    );
    assert_eq!(
        at["resource_access"],
        json!({"portal-api": {"roles": ["reader"]}})
    );

    let id = f.eval(TokenKind::IdToken);
    assert_eq!(id["typ"], "ID");
    assert_eq!(id["nonce"], "n-1");
    assert_eq!(id["aud"], "edge");
    assert!(id.get("scope").is_none() && id.get("cnf").is_none());
    assert!(id.get("realm_access").is_none());

    let intro = f.eval(TokenKind::Introspection);
    assert_eq!(intro["typ"], "Bearer");
    assert_eq!(intro["cnf"], json!({"jkt": "thumb"}));

    let ui = f.eval(TokenKind::UserInfo);
    assert_eq!(ui["sub"], "u-1");
    assert!(ui.get("iss").is_none() && ui.get("typ").is_none() && ui.get("aud").is_none());

    // No profile names, no e-mail, no roles without the scopes or full scope.
    let mut bare = Fixture::new(vec![]);
    bare.scopes = vec![ClientScopeRepresentation {
        name: "basic".into(),
        ..Default::default()
    }];
    bare.user.first_name = None;
    bare.user.last_name = None;
    bare.user.email = None;
    bare.client.full_scope_allowed = false;
    bare.user.client_roles.clear();
    let mut ctx = bare.ctx();
    ctx.granted_scope_string = "openid";
    ctx.acr = "";
    ctx.amr = &[];
    ctx.session_id = None;
    ctx.auth_time = None;
    ctx.dpop_jkt = None;
    ctx.act = None;
    let at = evaluate_mappers(&ctx, TokenKind::AccessToken).unwrap();
    assert_eq!(at["preferred_username"], "alice");
    for absent in [
        "name",
        "given_name",
        "email",
        "email_verified",
        "realm_access",
        "resource_access",
        "acr",
        "amr",
        "sid",
        "auth_time",
        "cnf",
        "act",
    ] {
        assert!(at.get(absent).is_none(), "{absent}");
    }
    ctx.granted_scope_string = "openid profile email roles";
    let mut no_names = bare.user.clone();
    no_names.username = String::new();
    no_names.realm_roles.clear();
    let groups: Vec<GroupRepresentation> = vec![];
    ctx.user = &no_names;
    ctx.user_groups = &groups;
    let mut client = bare.client.clone();
    client.full_scope_allowed = true;
    ctx.client = &client;
    let at = evaluate_mappers(&ctx, TokenKind::AccessToken).unwrap();
    assert!(at.get("name").is_none() && at.get("preferred_username").is_none());
    assert_eq!(at["email_verified"], true);
    assert!(at.get("realm_access").is_none() && at.get("resource_access").is_none());
}

#[test]
fn every_mapper_kind_writes_its_claim() {
    let f = Fixture::new(vec![
        mapper(
            "oidc-audience-mapper",
            &[("included.custom.audience", "custom-aud")],
        ),
        mapper(
            "oidc-audience-mapper",
            &[("included.client.audience", "portal-api")],
        ),
        mapper(
            "oidc-audience-mapper",
            &[("included.client.audience", "portal-api")],
        ),
        mapper("oidc-audience-mapper", &[("included.custom.audience", "")]),
        mapper("oidc-audience-resolve-mapper", &[]),
        mapper("oidc-group-membership-mapper", &[]),
        mapper(
            "oidc-group-membership-mapper",
            &[("claim.name", "jc.short_groups"), ("full.path", "false")],
        ),
        mapper(
            "oidc-usermodel-attribute-mapper",
            &[
                ("user.attribute", "jc_org"),
                ("claim.name", "jc_org"),
                ("multivalued", "true"),
            ],
        ),
        mapper(
            "oidc-usermodel-attribute-mapper",
            &[
                ("user.attribute", "phone"),
                ("claim.name", "phones"),
                ("aggregate.attrs", "true"),
                ("multivalued", "true"),
            ],
        ),
        mapper(
            "oidc-usermodel-attribute-mapper",
            &[("claim.name", "phone")],
        ),
        mapper(
            "oidc-usermodel-attribute-mapper",
            &[("user.attribute", "missing")],
        ),
        mapper("oidc-amr-mapper", &[]),
        mapper("oidc-acr-mapper", &[]),
        mapper(
            "oidc-usermodel-realm-role-mapper",
            &[("claim.name", "roles.realm")],
        ),
        mapper("oidc-usermodel-realm-role-mapper", &[]),
        mapper(
            "oidc-usermodel-client-role-mapper",
            &[("usermodel.clientRoleMapping.clientId", "portal-api")],
        ),
        mapper(
            "oidc-usermodel-client-role-mapper",
            &[("claim.name", "own_roles")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "id"), ("claim.name", "p.id")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "username"), ("claim.name", "p.username")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "email"), ("claim.name", "p.email")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "firstName"), ("claim.name", "p.first")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "lastName")],
        ),
        mapper(
            "oidc-usermodel-property-mapper",
            &[("user.attribute", "password"), ("claim.name", "p.secret")],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.bool"),
                ("claim.value", "true"),
                ("jsonType.label", "boolean"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.int"),
                ("claim.value", "42"),
                ("jsonType.label", "int"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.long"),
                ("claim.value", "x"),
                ("jsonType.label", "long"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.json"),
                ("claim.value", "{\"a\":[1]}"),
                ("jsonType.label", "JSON"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.badjson"),
                ("claim.value", "{"),
                ("jsonType.label", "JSON"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[("claim.name", "h.str"), ("claim.value", "v")],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[("claim.name", "h.novalue")],
        ),
        mapper("oidc-full-name-mapper", &[]),
        mapper("oidc-sub-mapper", &[]),
        mapper(
            "oidc-sha256-pairwise-sub-mapper",
            &[
                ("sectorIdentifierUri", "https://portal.example/callbacks"),
                ("pairwiseSubAlgorithmSalt", "salt-value-1"),
            ],
        ),
        mapper("oidc-AllowedWebOrigins-mapper", &[]),
        mapper("allowed-web-origins", &[]),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "not.in.access"),
                ("claim.value", "x"),
                ("access.token.claim", "false"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[("claim.name", ".."), ("claim.value", "ignored")],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "h.str.deeper"),
                ("claim.value", "replaces a string"),
            ],
        ),
    ]);
    let at = f.eval(TokenKind::AccessToken);
    let aud: Vec<&str> = at["aud"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(aud[0], "custom-aud");
    assert_eq!(aud[1], "portal-api");
    assert!(aud.contains(&"empty") && aud.len() == 3, "{aud:?}");
    assert_eq!(at["groups"], json!(["/hel/team", "/ops", "/top"]));
    assert_eq!(at["jc"]["short_groups"], json!(["ops", "team", "top"]));
    // jc_org comes from groups (the user's own value is ignored), plus the pinned organization.
    assert_eq!(at["jc_org"], json!(["hel", "pinned-org"]));
    assert_eq!(at["phones"], json!(["+1", "+358", "+359"]));
    assert_eq!(at["phone"], "+1");
    assert!(at.get("missing").is_none());
    assert_eq!(at["amr"], json!(["pwd", "otp"]));
    assert_eq!(at["roles"]["realm"], json!(["auditor", "editor", "viewer"]));
    assert_eq!(
        at["resource_access"]["portal-api"]["roles"],
        json!(["reader"])
    );
    assert!(at.get("own_roles").is_none(), "edge has no client roles");
    assert_eq!(at["p"]["id"], "u-1");
    assert_eq!(at["p"]["username"], "alice");
    assert_eq!(at["p"]["email"], "alice@hel.fi");
    assert_eq!(at["p"]["first"], "Alice");
    assert_eq!(at["lastName"], "Liddell");
    assert!(at["p"].get("secret").is_none());
    assert_eq!(at["h"]["bool"], true);
    assert_eq!(at["h"]["int"], 42);
    assert_eq!(at["h"]["long"], "x");
    assert_eq!(at["h"]["json"], json!({"a":[1]}));
    assert_eq!(at["h"]["badjson"], "{");
    assert_eq!(at["h"]["str"], json!({"deeper": "replaces a string"}));
    assert!(at["h"].get("novalue").is_none());
    let expected_pairwise = kaisatsu_core::claims::compute_pairwise_sub(
        &mapper(
            "oidc-sha256-pairwise-sub-mapper",
            &[
                ("sectorIdentifierUri", "https://portal.example/callbacks"),
                ("pairwiseSubAlgorithmSalt", "salt-value-1"),
            ],
        ),
        &f.client,
        "u-1",
    )
    .unwrap();
    assert_eq!(at["sub"], expected_pairwise);
    assert_ne!(at["sub"], "u-1");
    assert_eq!(at["allowed-origins"], json!(["https://portal.example"]));
    assert!(at.get("not").is_none());

    // Token-kind switches: id.token.claim, userinfo.token.claim, introspection falls back to access.
    let switched = Fixture::new(vec![
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "in_id"),
                ("claim.value", "1"),
                ("id.token.claim", "true"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "in_ui"),
                ("claim.value", "1"),
                ("userinfo.token.claim", "true"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "no_intro"),
                ("claim.value", "1"),
                ("access.token.claim", "false"),
            ],
        ),
        mapper(
            "oidc-hardcoded-claim-mapper",
            &[
                ("claim.name", "intro"),
                ("claim.value", "1"),
                ("access.token.claim", "false"),
                ("introspection.token.claim", "true"),
            ],
        ),
        mapper("oidc-acr-mapper", &[]),
    ]);
    let id = switched.eval(TokenKind::IdToken);
    assert!(id.contains_key("in_id") && !id.contains_key("in_ui"));
    let ui = switched.eval(TokenKind::UserInfo);
    assert!(ui.contains_key("in_ui") && !ui.contains_key("in_id"));
    let intro = switched.eval(TokenKind::Introspection);
    assert!(intro.contains_key("intro") && !intro.contains_key("no_intro"));
    let mut ctx = switched.ctx();
    ctx.acr = "";
    assert!(
        !evaluate_mappers(&ctx, TokenKind::AccessToken)
            .unwrap()
            .contains_key("acr")
    );
}

#[test]
fn jc_org_on_a_transient_user_and_without_groups() {
    let mut f = Fixture::new(vec![mapper(
        "oidc-usermodel-attribute-mapper",
        &[("user.attribute", "jc_org")],
    )]);
    f.groups.clear();
    let mut ctx = f.ctx();
    ctx.active_org = None;
    let at = evaluate_mappers(&ctx, TokenKind::AccessToken).unwrap();
    assert!(
        at.get("jc_org").is_none(),
        "a stored person's own jc_org never counts"
    );
    let mut transient = f.user.clone();
    transient.transient = true;
    ctx.user = &transient;
    let at = evaluate_mappers(&ctx, TokenKind::AccessToken).unwrap();
    assert_eq!(at["jc_org"], "user-org");
    ctx.active_org = Some("user-org");
    let at = evaluate_mappers(&ctx, TokenKind::AccessToken).unwrap();
    assert_eq!(at["jc_org"], "user-org");
}

#[test]
fn unknown_mapper_kinds_are_refused() {
    let f = Fixture::new(vec![mapper("script-mapper", &[])]);
    let err = evaluate_mappers(&f.ctx(), TokenKind::AccessToken).unwrap_err();
    assert!(matches!(err, KaisatsuError::BadRequest(_)));
    assert!(err.to_string().contains("script-mapper"));
    assert!(is_known_mapper_kind("oidc-amr-mapper"));
    assert!(!is_known_mapper_kind("script-mapper"));
    assert!(known_mapper_kinds().contains(&"oidc-sub-mapper"));
    // A scope mapper is checked too.
    let mut f = Fixture::new(vec![]);
    f.scopes[0].protocol_mappers = vec![mapper("nope", &[])];
    assert!(evaluate_mappers(&f.ctx(), TokenKind::IdToken).is_err());
}

/// K4: Proved red against pre-K4 where `oidc-sha256-pairwise-sub-mapper` returned the plain
/// `ctx.user.id` regardless of sector or salt.
#[test]
fn k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id() {
    let mut f1 = Fixture::new(vec![mapper(
        "oidc-sha256-pairwise-sub-mapper",
        &[
            ("sectorIdentifierUri", "https://sector-a.example.org/cb"),
            ("pairwiseSubAlgorithmSalt", "secret-salt-alpha"),
            ("id.token.claim", "true"),
            ("userinfo.token.claim", "true"),
        ],
    )]);
    let at_a1 = f1.eval(TokenKind::AccessToken);
    let id_a1 = f1.eval(TokenKind::IdToken);
    let ui_a1 = f1.eval(TokenKind::UserInfo);
    let intro_a1 = f1.eval(TokenKind::Introspection);
    let sub_a = at_a1["sub"].as_str().unwrap().to_owned();

    // Never raw user id, and stable across AccessToken, IdToken, UserInfo, Introspection, and logout helper.
    assert_ne!(sub_a, "u-1");
    assert!(!sub_a.contains("u-1"));
    assert_eq!(id_a1["sub"], sub_a);
    assert_eq!(ui_a1["sub"], sub_a);
    assert_eq!(intro_a1["sub"], sub_a);
    assert_eq!(
        kaisatsu_core::claims::compute_subject_for_client(&f1.client, &f1.scopes, "u-1").unwrap(),
        sub_a
    );

    // Second sector gets a different `sub`.
    let f2 = Fixture::new(vec![mapper(
        "oidc-sha256-pairwise-sub-mapper",
        &[
            ("sectorIdentifierUri", "https://sector-b.example.org/cb"),
            ("pairwiseSubAlgorithmSalt", "secret-salt-alpha"),
        ],
    )]);
    let sub_b = f2.eval(TokenKind::AccessToken)["sub"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(sub_a, sub_b);

    // Changing the salt changes `sub` for the same sector.
    let f3 = Fixture::new(vec![mapper(
        "oidc-sha256-pairwise-sub-mapper",
        &[
            ("sectorIdentifierUri", "https://sector-a.example.org/cb"),
            ("pairwiseSubAlgorithmSalt", "secret-salt-beta"),
        ],
    )]);
    let sub_a_salt2 = f3.eval(TokenKind::AccessToken)["sub"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(sub_a, sub_a_salt2);

    // Fallback to single redirect_uri host when sectorIdentifierUri is omitted.
    f1.client.protocol_mappers = vec![mapper(
        "oidc-sha256-pairwise-sub-mapper",
        &[("pairwiseSubAlgorithmSalt", "secret-salt-alpha")],
    )];
    f1.client.redirect_uris = vec![
        "https://sector-a.example.org/cb1".into(),
        "https://sector-a.example.org/cb2".into(),
    ];
    assert_eq!(f1.eval(TokenKind::AccessToken)["sub"], sub_a);

    // Multiple redirect_uri hosts without sectorIdentifierUri is refused.
    f1.client
        .redirect_uris
        .push("https://other-host.example.org/cb".into());
    assert!(evaluate_mappers(&f1.ctx(), TokenKind::AccessToken).is_err());
}

#[test]
fn composite_roles_resolve_once_and_ignore_client_roles() {
    let all = roles();
    assert_eq!(
        resolve_effective_realm_roles(&["editor".into()], &[], &all),
        vec!["auditor", "editor", "viewer"]
    );
    assert_eq!(
        resolve_effective_realm_roles(&["auditor-client".into()], &["unknown".into()], &all),
        vec!["auditor-client", "unknown"]
    );
    assert!(resolve_effective_realm_roles(&[], &[], &all).is_empty());
}
