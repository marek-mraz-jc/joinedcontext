#![forbid(unsafe_code)]
//! Error answers (W12), branding sanitising (W7), the realm LoA map, the operation registry and
//! the OData scope each caller gets per entity set (W5, W6).

use axum::response::IntoResponse;
use kaisatsu_core::branding::{compute_primary_foreground, is_safe_svg};
use kaisatsu_core::*;
use serde_json::Value;
use std::collections::BTreeMap;

async fn answer(e: KaisatsuError) -> (u16, http::HeaderMap, Value) {
    let resp = e.into_response();
    let st = resp.status().as_u16();
    let h = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (st, h, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn every_error_answers_its_status_slug_and_a_safe_detail() {
    let cases: Vec<(KaisatsuError, u16, &str)> = vec![
        (KaisatsuError::NotFound("x".into()), 404, "not-found"),
        (KaisatsuError::Forbidden("x".into()), 403, "forbidden"),
        (KaisatsuError::Unauthorized("x".into()), 401, "unauthorized"),
        (
            KaisatsuError::SecondFactorRequired,
            403,
            "second-factor-required",
        ),
        (KaisatsuError::Conflict("x".into()), 409, "conflict"),
        (
            KaisatsuError::BadRequest("x".into()),
            400,
            "invalid-request",
        ),
        (
            KaisatsuError::field_error("password", "too short"),
            422,
            "validation-error",
        ),
        (
            KaisatsuError::OutboundRefused("10.0.0.7".into()),
            400,
            "outbound-refused",
        ),
        (
            KaisatsuError::NotSupported("passkeys".into()),
            501,
            "not-supported",
        ),
        (KaisatsuError::StalePlan("x".into()), 412, "stale-plan"),
        (
            KaisatsuError::Storage("SELECT secret".into()),
            500,
            "storage-error",
        ),
        (
            KaisatsuError::Crypto("kid k1 at /run/kek".into()),
            500,
            "crypto-error",
        ),
    ];
    for (e, status, slug) in cases {
        assert_eq!(e.status_code(), status);
        assert_eq!(e.slug(), slug);
        let (st, h, body) = answer(e).await;
        assert_eq!(st, status, "{slug}");
        assert_eq!(h["content-type"], "application/problem+json");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(body["slug"], slug);
        assert_eq!(body["type"], format!("urn:joinedcontext:problem:{slug}"));
        let detail = body["detail"].as_str().unwrap();
        for internal in ["SELECT", "/run/kek", "10.0.0.7"] {
            assert!(!detail.contains(internal), "{slug}: {detail}");
        }
        assert_eq!(body["errorMessage"], body["detail"]);
    }
    let v = KaisatsuError::field_error("password", "too short");
    if let KaisatsuError::Validation { fields, message } = &v {
        assert_eq!(message, "password: too short");
        assert_eq!(fields["password"], vec!["too short".to_owned()]);
    }

    let (st, h, body) = answer(KaisatsuError::RateLimited {
        message: "slow down".into(),
        retry_after_secs: 17,
    })
    .await;
    assert_eq!(st, 429);
    assert_eq!(h["retry-after"], "17");
    assert_eq!(body["slug"], "rate-limited");
}

#[tokio::test]
async fn oauth_errors_answer_rfc_6749_bodies() {
    let cases = [
        (KaisatsuError::invalid_request("r"), 400, "invalid_request"),
        (KaisatsuError::invalid_client("c"), 401, "invalid_client"),
        (KaisatsuError::invalid_grant("g"), 400, "invalid_grant"),
        (
            KaisatsuError::unauthorized_client("u"),
            400,
            "unauthorized_client",
        ),
        (
            KaisatsuError::unsupported_grant_type("t"),
            400,
            "unsupported_grant_type",
        ),
        (
            KaisatsuError::unsupported_response_type("t"),
            400,
            "unsupported_response_type",
        ),
        (KaisatsuError::invalid_scope("s"), 400, "invalid_scope"),
        (KaisatsuError::invalid_token("t"), 400, "invalid_token"),
        (KaisatsuError::use_dpop_nonce("n"), 400, "use_dpop_nonce"),
        (
            KaisatsuError::invalid_dpop_proof("p"),
            400,
            "invalid_dpop_proof",
        ),
    ];
    for (e, status, code) in cases {
        assert_eq!(e.slug(), "oauth-error");
        let (st, h, body) = answer(e).await;
        assert_eq!(st, status);
        assert_eq!(h["content-type"], "application/json");
        assert_eq!(h["cache-control"], "no-store");
        assert_eq!(body["error"], code);
        assert!(body["error_description"].is_string());
    }
    let (st, _, body) = answer(KaisatsuError::oauth(400, "access_denied", "")).await;
    assert_eq!(st, 400);
    assert!(body.get("error_description").is_none());
    // A status that is not HTTP falls back to 400.
    let (st, _, _) = answer(KaisatsuError::oauth(1000, "server_error", "x")).await;
    assert_eq!(st, 400);
}

#[test]
fn odata_table_errors_map_to_kaisatsu_errors() {
    use odata_table::TableError;
    assert!(matches!(
        KaisatsuError::from(TableError::NotImplemented("$apply".into())),
        KaisatsuError::NotSupported(_)
    ));
    let e = KaisatsuError::from(TableError::UnknownProperty("secretData".into()));
    assert!(
        matches!(&e, KaisatsuError::BadRequest(m) if m.contains("unknown property: secretData"))
    );
    assert!(matches!(
        KaisatsuError::from(TableError::BadRequest("bad".into())),
        KaisatsuError::BadRequest(_)
    ));
    assert!(matches!(
        KaisatsuError::from(TableError::Source("db".into())),
        KaisatsuError::Storage(_)
    ));
}

#[test]
fn branding_falls_back_field_by_field() {
    let b = BrandingBlock {
        colours: Colours {
            primary: "#abc".into(),
            secondary: "#12345".into(),
            accent: "#GGGGGG".into(),
            background: "#ffffff".into(),
            text: "red".into(),
        },
        fonts: Fonts {
            heading: "  ".into(),
            body: "'Open Sans', sans-serif".into(),
        },
        logo: "logo-v2.png".into(),
        favicon: "../etc/passwd".into(),
        logo_svg: "<svg viewBox=\"0 0 1 1\"><use href=\"#a\"/><rect fill=\"url(#g)\"/></svg>"
            .into(),
        languages: Languages {
            default: "1x".into(),
            offered: vec!["sk".into(), "toolonglocale1".into(), "e n".into()],
        },
        instance_name: " ".into(),
        short_name: String::new(),
        organisation: String::new(),
        ..BrandingBlock::default()
    }
    .sanitised();
    let fb = BrandingBlock::default();
    assert_eq!(b.colours.primary, "#abc");
    assert_eq!(b.colours.secondary, fb.colours.secondary);
    assert_eq!(b.colours.accent, fb.colours.accent);
    assert_eq!(b.colours.text, fb.colours.text);
    assert_eq!(b.fonts.heading, fb.fonts.heading);
    assert_eq!(b.fonts.body, "'Open Sans', sans-serif");
    assert_eq!(b.logo, "logo-v2.png");
    assert!(b.favicon.is_empty());
    assert!(!b.logo_svg.is_empty(), "local references are fine");
    assert_eq!(b.languages.default, fb.languages.default);
    assert_eq!(b.languages.offered[0], fb.languages.default);
    assert!(b.languages.offered.contains(&"sk".to_owned()));
    assert_eq!(b.languages.offered.len(), 2);
    assert_eq!(b.instance_name, fb.instance_name);
    assert_eq!(b.short_name, b.instance_name);
    assert_eq!(b.organisation, b.instance_name);
    for asset in ["/abs.png", "a b.png", "https://x/y.png", "", "a/../b"] {
        let b = BrandingBlock {
            logo: asset.into(),
            ..BrandingBlock::default()
        }
        .sanitised();
        assert!(b.logo.is_empty(), "{asset}");
    }
}

#[test]
fn svg_logos_refuse_scripts_handlers_and_outside_references() {
    for bad in [
        "<svg><script>alert(1)</script></svg>",
        "<svg onload=alert(1)>",
        "<svg><animate onbegin=\"alert(1)\" attributeName=\"x\"/></svg>",
        "<svg><g onfocusin = 'x'/></svg>",
        "<svg/onmouseenter=x>",
        "<svg><image href=\"https://evil.example/t.png\"/></svg>",
        "<svg><use xlink:href='data:image/svg+xml,<svg onload=alert(1)>'/></svg>",
        "<svg><a href = javascript:alert(1)>x</a></svg>",
        "<svg><style>@import url(https://evil)</style></svg>",
        "<svg><rect style=\"fill:url(https://evil/p)\"/></svg>",
        "<svg><foreignObject/></svg>",
        "<svg><iframe/></svg>",
        "<svg>&#106;avascript</svg>",
        "<!DOCTYPE svg><svg/>",
    ] {
        assert!(!is_safe_svg(bad), "{bad}");
    }
    for good in [
        "",
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><polygon points=\"0,0 1,1\"/><text>Lisbon</text></svg>",
        "<svg><use href=\"#icon\"/><rect fill=\"url( '#grad')\" transform=\"rotate(45)\"/></svg>",
    ] {
        assert!(is_safe_svg(good), "{good}");
    }
}

#[test]
fn primary_foreground_keeps_contrast() {
    assert_eq!(compute_primary_foreground("#ffffff"), "#0f172a");
    assert_eq!(compute_primary_foreground("#fff"), "#0f172a");
    assert_eq!(compute_primary_foreground("#000000"), "#ffffff");
    assert_eq!(compute_primary_foreground("#00f"), "#ffffff");
    for broken in ["", "fff", "#ff", "#fgf", "#zzzzzz", "#1234567"] {
        assert_eq!(compute_primary_foreground(broken), "#ffffff", "{broken}");
    }
}

#[test]
fn acr_loa_map_reads_the_realm_attribute_or_defaults() {
    let mut realm = RealmConfig::default();
    assert_eq!(realm.acr_loa_map()["mfa"], 2);
    realm
        .attributes
        .insert("acr.loa.map".into(), r#"{"pwd":1,"mfa":3,"hw":4}"#.into());
    assert_eq!(realm.acr_loa_map()["hw"], 4);
    realm.attributes.insert("acr.loa.map".into(), "{".into());
    assert_eq!(
        realm.acr_loa_map(),
        BTreeMap::from([("pwd".into(), 1), ("mfa".into(), 2)])
    );
}

#[test]
fn registry_default_and_lookups() {
    let reg = OperationRegistry::default();
    assert_eq!(
        reg.operations().len(),
        OperationRegistry::new().operations().len()
    );
    assert!(reg.find("realm_apply").is_some());
    assert!(reg.find("no_such_operation").is_none());
    assert!(reg.find_odata("NoSuchOperation", None).is_none());
}

#[test]
fn odata_scope_per_entity_set_and_caller() {
    let sets = [
        "Realms",
        "IdentityProviders",
        "SigningKeys",
        "RequiredActions",
        "Organizations",
        "OrganizationDomains",
        "BrandingBlocks",
        "People",
        "Memberships",
        "Credentials",
        "Consents",
        "Groups",
        "GroupMembers",
        "Roles",
        "RoleMappings",
        "Applications",
        "ProtocolMappers",
        "ClientScopes",
        "Sessions",
        "ClientSessions",
        "OfflineSessions",
        "Events",
        "AdminEvents",
    ];
    let admin = RequestCtx::new("dev", Principal::realm_admin("root"));
    let mut member = Principal::anonymous();
    member.subject = Some("m".into());
    member.organizations = vec!["hel".into()];
    member
        .org_roles
        .insert("hel".into(), vec!["auditor".into()]);
    let member = RequestCtx::new("dev", member).with_org("hel");
    let anon = RequestCtx::new("dev", Principal::anonymous());
    for set in sets {
        let a = format!("{:?}", scope_for_context(&admin, &LocalAuthorizer, set));
        assert!(a.contains("read"), "{set}: {a}");
        assert!(a.contains("write:"), "{set}: {a}");
        let m = format!("{:?}", scope_for_context(&member, &LocalAuthorizer, set));
        assert!(
            !m.contains("write:"),
            "{set}: an auditor writes nothing: {m}"
        );
        let n = format!("{:?}", scope_for_context(&anon, &LocalAuthorizer, set));
        assert!(
            !n.contains("\"read\"") && !n.contains("write:"),
            "{set}: {n}"
        );
    }
    let m = format!(
        "{:?}",
        scope_for_context(&member, &LocalAuthorizer, "People")
    );
    assert!(
        m.contains("hel"),
        "an organization caller is limited to its organization: {m}"
    );
}
