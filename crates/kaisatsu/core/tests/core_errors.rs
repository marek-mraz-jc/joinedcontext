#![forbid(unsafe_code)]
//! Error paths of the pairwise subject mapper (K4, OIDC Core §8.1) and the OData row builders of
//! the K-item entity sets: nothing secret leaves in a row.

use chrono::{TimeZone, Utc};
use kaisatsu_core::claims::{compute_pairwise_sub, compute_subject_for_client};
use kaisatsu_core::odata_defs::{
    email_outbox_to_row, idp_link_to_row, signing_key_to_row, webauthn_credential_to_row,
};
use kaisatsu_core::*;
use std::collections::BTreeMap;

fn pairwise(cfg: &[(&str, &str)]) -> ProtocolMapperRepresentation {
    ProtocolMapperRepresentation {
        name: "pairwise".into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-sha256-pairwise-sub-mapper".into(),
        config: cfg
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        ..Default::default()
    }
}

fn client(redirects: &[&str]) -> ClientRepresentation {
    ClientRepresentation {
        client_id: "edge".into(),
        redirect_uris: redirects.iter().map(|r| (*r).to_owned()).collect(),
        ..Default::default()
    }
}

#[test]
fn pairwise_refuses_a_bad_or_hostless_sector_uri() {
    let c = client(&["https://portal.example.test/cb"]);
    for bad in ["not a url", "mailto:someone@example.test"] {
        let err = compute_pairwise_sub(&pairwise(&[("sectorIdentifierUri", bad)]), &c, "u-1")
            .expect_err(bad);
        assert!(
            matches!(err, KaisatsuError::BadRequest(_)),
            "{bad}: {err:?}"
        );
        assert!(!err.to_string().contains("u-1"));
    }
    // The same refusal reaches every caller through compute_subject_for_client.
    let mut with_mapper = c.clone();
    with_mapper
        .protocol_mappers
        .push(pairwise(&[("sectorIdentifierUri", "not a url")]));
    assert!(compute_subject_for_client(&with_mapper, &[], "u-1").is_err());
}

#[test]
fn pairwise_reads_the_salt_key_and_redirect_hosts() {
    let c = client(&[
        "https://portal.example.test/cb",
        "https://portal.example.test/other/*",
    ]);
    // `salt` is read when `pairwiseSubAlgorithmSalt` is absent, and it changes the subject.
    let a = compute_pairwise_sub(&pairwise(&[("salt", "s-one")]), &c, "u-1").unwrap();
    let b = compute_pairwise_sub(&pairwise(&[("salt", "s-two")]), &c, "u-1").unwrap();
    let same = compute_pairwise_sub(
        &pairwise(&[("pairwiseSubAlgorithmSalt", "s-one")]),
        &c,
        "u-1",
    )
    .unwrap();
    assert_ne!(a, b);
    assert_eq!(a, same, "both salt keys name one salt");
    assert_ne!(a, "u-1");
    // Zero or several redirect hosts without a sector URI are refused.
    for redirects in [
        &[][..],
        &["https://a.example.test/cb", "https://b.example.test/cb"][..],
    ] {
        assert!(compute_pairwise_sub(&pairwise(&[]), &client(redirects), "u-1").is_err());
    }
    // Without a pairwise mapper the local id is the subject.
    assert_eq!(compute_subject_for_client(&c, &[], "u-1").unwrap(), "u-1");
}

#[test]
fn k_item_rows_carry_no_secret_fields() {
    let at = Utc.with_ymd_and_hms(2026, 11, 8, 12, 0, 0).unwrap();
    let key = SigningKeyRecord {
        id: "k1".into(),
        realm_id: "dev".into(),
        kid: "kid-1".into(),
        algorithm: "ES256".into(),
        status: "ACTIVE".into(),
        priority: 100,
        public_jwk_json: "{}".into(),
        encrypted_private_key: vec![1, 2, 3],
        created_at: at,
        activate_at: Some(at),
        expires_at: Some(at),
    };
    let row = signing_key_to_row(&key);
    assert_eq!(row["activate_at"], at.to_rfc3339());
    assert_eq!(row["expires_at"], at.to_rfc3339());
    assert!(row.get("encrypted_private_key").is_none());

    let cred = WebAuthnCredentialRecord {
        id: "w1".into(),
        realm_id: "dev".into(),
        org_id: Some("hel".into()),
        user_id: "u-1".into(),
        user_handle: "handle-opaque".into(),
        credential_id: "cred-1".into(),
        public_key_cose_json: "{\"kty\":\"EC2\"}".into(),
        cose_alg: -7,
        sign_count: 3,
        uv_initialized: true,
        backup_eligible: true,
        backup_state: false,
        transports: vec!["internal".into()],
        attestation_type: "None".into(),
        rp_id: "idm.example.test".into(),
        user_label: Some("Laptop".into()),
        created_at: at,
        last_used_at: None,
    };
    let row = webauthn_credential_to_row(&cred);
    assert_eq!(row["credentialId"], "cred-1");
    assert_eq!(row["signCount"], 3);
    let text = row.to_string();
    assert!(
        !text.contains("handle-opaque") && !text.contains("EC2"),
        "{text}"
    );

    let mail = EmailOutboxRecord {
        id: "o1".into(),
        realm_id: "dev".into(),
        org_id: None,
        user_id: "u-1".into(),
        to_address: "a@example.test".into(),
        locale: "sk".into(),
        template_kind: "execute-actions".into(),
        action_jti: "jti-not-shown".into(),
        actions: vec!["VERIFY_EMAIL".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: "https://idm.example.test/realms/dev".into(),
        status: "failed".into(),
        attempts: 5,
        last_error: Some("smtp refused".into()),
        next_attempt_at: at,
        created_at: at,
        sent_at: None,
    };
    let row = email_outbox_to_row(&mail);
    assert_eq!(row["status"], "failed");
    assert_eq!(row["lastError"], "smtp refused");
    assert!(!row.to_string().contains("jti-not-shown"));

    let link = IdentityProviderLinkRecord {
        id: "l1".into(),
        realm_id: "dev".into(),
        org_id: None,
        user_id: "u-1".into(),
        idp_alias: "upstream".into(),
        external_user_id: "ext-1".into(),
        external_username: Some("ext".into()),
        created_at: at,
    };
    let row = idp_link_to_row(&link);
    assert_eq!(row["idpAlias"], "upstream");
    assert_eq!(row["externalUserId"], "ext-1");
}
