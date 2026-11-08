#![forbid(unsafe_code)]
//! Domain verification refusals (K13) at the library seam: a domain without a challenge, a
//! challenge without an expiry or past it, a TXT answer that does not match, cluster-internal
//! names, and the real resolver on a name that cannot exist (RFC 6761 `.invalid`).

use chrono::{Duration, Utc};
use kaisatsu_admin::domain::{
    CHALLENGE_PREFIX, request_domain_verification, validate_verifiable_domain,
    verify_organization_domain,
};
use kaisatsu_admin::{DnsResolver, HickoryDnsResolver};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sha256_hex};
use kaisatsu_store_memory::MemoryStore;
use std::collections::HashMap;
use std::sync::Arc;

async fn store_with(domain: OrganizationDomain) -> Arc<dyn Store> {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(Kek::generate_for_tests()));
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_organization(Organization {
            id: "helsinki".into(),
            realm_id: "dev".into(),
            name: "helsinki".into(),
            alias: "helsinki".into(),
            enabled: true,
            description: String::new(),
            domains: vec![domain],
            attributes: HashMap::new(),
            managed_by: None,
        })
        .await
        .unwrap();
    store
}

fn domain(name: &str) -> OrganizationDomain {
    OrganizationDomain {
        id: "dom-hel".into(),
        realm_id: "dev".into(),
        org_id: "helsinki".into(),
        name: name.into(),
        verified: false,
        verification_token: None,
        challenge_hash: None,
        challenge_expires_at: None,
    }
}

fn message(e: KaisatsuError) -> String {
    e.to_string()
}

#[tokio::test]
async fn verify_refuses_missing_expired_and_mismatched_challenges() {
    let resolver = HickoryDnsResolver::new();
    let fqdn = format!("{CHALLENGE_PREFIX}hel.fi");

    // No challenge at all.
    let store = store_with(domain("hel.fi")).await;
    let err = verify_organization_domain(&store, &resolver, "dev", None, "dom-hel")
        .await
        .unwrap_err();
    assert!(message(err).contains("no active verification challenge"));

    // A stored hash without an expiry is refused, never treated as open-ended.
    let mut d = domain("hel.fi");
    d.challenge_hash = Some(sha256_hex("kaisatsu-verify=x"));
    let store = store_with(d.clone()).await;
    let err = verify_organization_domain(&store, &resolver, "dev", None, "hel.fi")
        .await
        .unwrap_err();
    assert!(message(err).contains("no expiry"));

    // An expired challenge.
    d.challenge_expires_at = Some(Utc::now() - Duration::minutes(1));
    let store = store_with(d.clone()).await;
    let err = verify_organization_domain(&store, &resolver, "dev", None, "dom-hel")
        .await
        .unwrap_err();
    assert!(message(err).contains("expired"));

    // A live challenge: no TXT, then a TXT that does not match; the domain stays unverified.
    let store = store_with(domain("hel.fi")).await;
    let issued = request_domain_verification(&store, "dev", None, "dom-hel")
        .await
        .unwrap();
    assert_eq!(issued["txtRecord"], fqdn.as_str());
    resolver.insert_mock_txt(&fqdn, Vec::new());
    let err = verify_organization_domain(&store, &resolver, "dev", None, "dom-hel")
        .await
        .unwrap_err();
    assert!(message(err).contains("no DNS TXT record"));
    resolver.insert_mock_txt(&fqdn, vec!["kaisatsu-verify=someone-else".into()]);
    assert!(
        verify_organization_domain(&store, &resolver, "dev", None, "dom-hel")
            .await
            .is_err()
    );
    let orgs = store.list_organizations("dev", None).await.unwrap();
    assert!(!orgs[0].domains[0].verified);

    // Outside the caller's organizations the domain does not exist.
    let other = vec!["espoo".to_owned()];
    let err = verify_organization_domain(&store, &resolver, "dev", Some(&other), "dom-hel")
        .await
        .unwrap_err();
    assert!(matches!(err, KaisatsuError::NotFound(_)));
    let err = request_domain_verification(&store, "dev", Some(&other), "dom-hel")
        .await
        .unwrap_err();
    assert!(matches!(err, KaisatsuError::NotFound(_)));

    // A stored domain under a cluster-internal suffix is refused before any lookup.
    let store = store_with(domain("portal.svc")).await;
    let err = request_domain_verification(&store, "dev", None, "dom-hel")
        .await
        .unwrap_err();
    assert!(matches!(err, KaisatsuError::OutboundRefused(_)));
}

#[test]
fn domain_names_are_validated() {
    for bad in ["", "nodot", "under_score.fi", "a b.fi"] {
        assert!(
            matches!(
                validate_verifiable_domain(bad),
                Err(KaisatsuError::BadRequest(_))
            ),
            "{bad:?}"
        );
    }
    for internal in [
        "x.cluster.local",
        "portal.svc",
        "printer.local",
        "db.internal",
        "a.localhost",
    ] {
        assert!(
            matches!(
                validate_verifiable_domain(internal),
                Err(KaisatsuError::OutboundRefused(_))
            ),
            "{internal}"
        );
    }
    assert_eq!(validate_verifiable_domain(" Hel.FI. ").unwrap(), "hel.fi");
}

#[tokio::test]
async fn real_resolver_refuses_internal_names_and_reports_missing_records() {
    let resolver = HickoryDnsResolver::new();
    let err = resolver
        .lookup_txt("_kaisatsu-challenge.portal.svc.cluster.local")
        .await
        .unwrap_err();
    assert!(matches!(err, KaisatsuError::OutboundRefused(_)));
    // `.invalid` never resolves (RFC 6761 §6.4): with or without network, the lookup errs.
    let err = resolver
        .lookup_txt("_kaisatsu-challenge.kaisatsu-tests.invalid")
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            KaisatsuError::BadRequest(_) | KaisatsuError::OutboundRefused(_)
        ),
        "{err}"
    );
}
