//! Minting an Organization id at the operator's door (EP-186, PF-41, PF-78, T-7012): the id of a
//! domain is minted only for an operator who proved a second factor, only once the domain's
//! `_joinedcontext.{domain}` TXT record carries the challenge, and never again after that, a
//! delete of its row included.
//!
//! DNS is a table of canned records; the store is the database `JC_PORTAL_TEST_DATABASE_URL`
//! names (CI's portal-test jobs set it). A missing variable fails, never skips. Each test mints
//! a domain of its own run, so the tests share the database.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::domain_verification::{TxtRecords, Unanswered};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{person, send};

const CHALLENGE: &str = "/api/v1/organization-ids/challenge";
const MINT: &str = "/api/v1/organization-ids";

/// The DNS of the test: TXT strings per name; a name in `silent` never answers.
#[derive(Default)]
struct Dns {
    records: Mutex<HashMap<String, Vec<String>>>,
    silent: Mutex<Vec<String>>,
}

impl Dns {
    fn publish(&self, name: &str, text: &str) {
        self.records
            .lock()
            .expect("dns")
            .entry(name.to_owned())
            .or_default()
            .push(text.to_owned());
    }
}

impl TxtRecords for Dns {
    fn txt<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, Unanswered>> {
        let silent = self.silent.lock().expect("dns").iter().any(|n| n == name);
        let found = self.records.lock().expect("dns").get(name).cloned();
        Box::pin(async move {
            if silent {
                return Err(Unanswered);
            }
            Ok(found.unwrap_or_default())
        })
    }
}

async fn world() -> (AppState, Arc<Dns>) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let mut config = Config::for_tests();
    config.decisions_need_second_factor = true;
    let dns = Arc::new(Dns::default());
    let state = AppState::new(config, None)
        .with_db(pool)
        .with_txt_records(dns.clone());
    (state, dns)
}

/// A domain of this run alone, e.g. `t1a2b3c.mint.example`, and its id.
fn fresh_domain(test: &str) -> (String, String) {
    let run = format!("{test}{:x}", std::process::id() ^ rand_suffix());
    (format!("{run}.example"), format!("example_{run}"))
}

fn rand_suffix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos())
}

fn operator(config: &Config) -> Identity {
    let mut who = person("ops");
    who.groups = vec![config.bootstrap_admins.clone()];
    who.second_factor = true;
    who
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_default()
}

/// The challenge of `domain`, asked by the operator; the TXT string it answers.
async fn challenge(state: &AppState, domain: &str) -> String {
    let asked = send(
        state,
        operator(&state.config),
        "POST",
        CHALLENGE,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text);
    let answer = body(&asked.text);
    assert_eq!(answer["name"], format!("_joinedcontext.{domain}"));
    answer["value"].as_str().expect("a value").to_owned()
}

#[tokio::test]
async fn the_published_challenge_mints_the_id_once() {
    let (state, dns) = world().await;
    let (domain, id) = fresh_domain("once");
    let value = challenge(&state, &domain).await;
    assert_eq!(
        challenge(&state, &domain).await,
        value,
        "one challenge per domain"
    );
    dns.publish(&format!("_joinedcontext.{domain}"), "v=spf1 -all");
    dns.publish(&format!("_joinedcontext.{domain}"), &value);

    let minted = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.text);
    assert_eq!(body(&minted.text)["organizationId"], id);

    let again = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);
    assert!(again.text.contains("organizationId"), "{}", again.text);
}

#[tokio::test]
async fn a_txt_record_that_is_not_the_challenge_refuses_the_mint() {
    let (state, dns) = world().await;
    let (domain, _) = fresh_domain("mismatch");
    let value = challenge(&state, &domain).await;
    dns.publish(&format!("_joinedcontext.{domain}"), &format!("{value}x"));
    dns.publish(
        &format!("_joinedcontext.{domain}"),
        "jc-verify=somebody-elses",
    );

    let refused = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.names_field("domain", "is this challenge"),
        "{}",
        refused.text
    );
}

#[tokio::test]
async fn a_missing_or_silent_record_refuses_the_mint() {
    let (state, dns) = world().await;
    let (domain, _) = fresh_domain("missing");
    challenge(&state, &domain).await;
    let refused = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.names_field("domain", "has no TXT record"),
        "{}",
        refused.text
    );

    dns.silent
        .lock()
        .expect("dns")
        .push(format!("_joinedcontext.{domain}"));
    let silent = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(
        silent.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        silent.text
    );
    assert!(
        silent.names_field("domain", "did not answer"),
        "{}",
        silent.text
    );

    let (unasked, _) = fresh_domain("unasked");
    let first = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": unasked })),
    )
    .await;
    assert_eq!(
        first.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        first.text
    );
    assert!(
        first.names_field("domain", "no challenge was asked"),
        "{}",
        first.text
    );
}

#[tokio::test]
async fn a_minted_id_stays_reserved_after_a_delete() {
    let (state, dns) = world().await;
    let (domain, id) = fresh_domain("reserved");
    let value = challenge(&state, &domain).await;
    dns.publish(&format!("_joinedcontext.{domain}"), &value);
    let minted = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.text);

    let pool = state.db.as_ref().expect("a database");
    for statement in [
        "DELETE FROM organization_ids WHERE id = $1",
        "UPDATE organization_ids SET domain = 'renamed.example' WHERE id = $1",
        "UPDATE organization_ids SET minted_at = NULL, minted_by = NULL WHERE id = $1",
    ] {
        let refused = sqlx::query(statement).bind(&id).execute(pool).await;
        assert!(refused.is_err(), "{statement} must be refused");
    }

    let asked = send(
        &state,
        operator(&state.config),
        "POST",
        CHALLENGE,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(asked.status, StatusCode::CONFLICT, "{}", asked.text);
    assert!(asked.text.contains("organizationId"), "{}", asked.text);
    let again = send(
        &state,
        operator(&state.config),
        "POST",
        MINT,
        Some(json!({ "domain": domain })),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);
}

#[tokio::test]
async fn only_an_operator_with_a_second_factor_mints() {
    let (state, dns) = world().await;
    let (domain, _) = fresh_domain("door");
    let value = challenge(&state, &domain).await;
    dns.publish(&format!("_joinedcontext.{domain}"), &value);
    let ask = Some(json!({ "domain": domain }));

    // An Organization administrator who is no operator is a stranger here (T-7138 bounce row 5).
    let administrator = common::organization_admin(&state.mirror, "org-admin");
    for uri in [CHALLENGE, MINT] {
        for who in [person("jana"), administrator.clone()] {
            let stranger = send(&state, who, "POST", uri, ask.clone()).await;
            assert_eq!(
                stranger.status,
                StatusCode::FORBIDDEN,
                "{uri}: {}",
                stranger.text
            );
        }
    }
    // Both doors are the operators' (T-7138 bounce row 4): a password-only operator is refused
    // at the challenge too, not only at the mint. Both call operator_changes::operator, whose
    // unit test a_bearer_in_the_operators_group_is_no_operator refuses a client's token (a
    // cookie session carries no client, so this suite cannot send one).
    let mut no_factor = operator(&state.config);
    no_factor.second_factor = false;
    for uri in [CHALLENGE, MINT] {
        let refused = send(&state, no_factor.clone(), "POST", uri, ask.clone()).await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{uri}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("second-factor"),
            "{uri}: {}",
            refused.text
        );
    }

    let minted = send(&state, operator(&state.config), "POST", MINT, ask).await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.text);
}

#[tokio::test]
async fn a_name_that_gives_no_public_id_is_refused_naming_the_domain() {
    let (state, _) = world().await;
    for bad in [
        "portal.jc.svc.cluster.local",
        "localhost",
        "Bbsk.sk",
        "bbsk",
        "example.1x",
        "a_b.sk",
    ] {
        let refused = send(
            &state,
            operator(&state.config),
            "POST",
            CHALLENGE,
            Some(json!({ "domain": bad })),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            refused.text
        );
        assert!(refused.names_field("domain", ""), "{bad}: {}", refused.text);
    }
}

// The Organization page's Addresses (T-7013): what jc's host registry routes for each
// Organization, to the people of that Organization alone, with where they sign in.

const ADDRESSES: &str = "/api/v1/organization/addresses";

fn addresses_world() -> AppState {
    use joinedcontext_portal::organization_host::parse_addresses;
    let mut config = Config::for_tests();
    config.organization_addresses = parse_addresses(
        &json!([
            { "organization": "bbsk", "organizationId": "sk_bbsk", "addresses": [
                { "kind": "portal", "url": "https://portal.bbsk.sk/" },
                { "kind": "mcp", "url": "https://mcp.bbsk.sk/api/mcp/" },
                { "kind": "shared", "url": "https://smartcity.sk/sk_bbsk/" },
            ]},
            { "organization": "nitra", "addresses": [
                { "kind": "portal", "url": "https://portal.nitra.sk/" },
            ]},
        ])
        .to_string(),
    )
    .expect("the addresses parse");
    config.oidc = Some(joinedcontext_portal::config::OidcConfig {
        issuer: "https://login.smartcity.sk/realms/smartcity"
            .parse()
            .expect("a url"),
        client_id: "portal".into(),
        auth: joinedcontext_portal::config::ClientAuth::Secret("secret".into()),
        extra_ca_pem: None,
        admin_url: None,
    });
    let state = AppState::new(config, None);
    let mut organization = common::envelope(
        "Organization",
        "bbsk",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "bbsk.sk", "locales": ["sk"], "defaultLocale": "sk" }),
    );
    organization.metadata.title =
        Some(serde_json::from_value(json!("Banskobystrický kraj")).expect("a title"));
    state.mirror.upsert(organization);
    state
}

#[tokio::test]
async fn an_operator_reads_every_organizations_addresses_with_its_sign_in() {
    let state = addresses_world();
    let read = send(&state, operator(&state.config), "GET", ADDRESSES, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let items = body(&read.text)["items"].clone();
    assert_eq!(items[0]["organization"], "bbsk");
    assert_eq!(items[0]["title"], "Banskobystrický kraj");
    assert_eq!(items[0]["organizationId"], "sk_bbsk");
    let kinds: Vec<&str> = items[0]["addresses"]
        .as_array()
        .expect("addresses")
        .iter()
        .filter_map(|a| a["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["portal", "mcp", "shared", "sign-in"]);
    assert_eq!(
        items[0]["addresses"][3]["url"],
        "https://login.smartcity.sk/realms/smartcity"
    );
    assert_eq!(items[1]["title"], "nitra", "no title: the name");
    assert!(items[1].get("organizationId").is_none(), "{}", read.text);
}

#[tokio::test]
async fn a_person_reads_only_the_addresses_of_their_own_organization() {
    let state = addresses_world();
    let read = send(&state, person("jana"), "GET", ADDRESSES, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert_eq!(
        body(&read.text)["items"],
        json!([]),
        "a stranger belongs to none"
    );

    use tower::ServiceExt as _;
    let anonymous = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::get(ADDRESSES)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn an_address_list_jc_could_not_have_written_is_refused() {
    use joinedcontext_portal::organization_host::parse_addresses;
    for bad in [
        r#"[{"organization":"a","addresses":[{"kind":"portal","url":"ftp://x.sk/"}]}]"#,
        r#"[{"organization":"a","organizationId":"bbsk","addresses":[]}]"#,
        r#"[{"organization":"a","addresses":[]},{"organization":"a","addresses":[]}]"#,
        r#"[{"organization":"a","addresses":[{"kind":"door","url":"https://x.sk/"}]}]"#,
    ] {
        assert!(parse_addresses(bad).is_err(), "{bad}");
    }
    assert!(parse_addresses("").expect("empty").is_empty());
}
