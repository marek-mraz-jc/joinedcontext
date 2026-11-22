#![forbid(unsafe_code)]
//! A subject id, a display username and a group name never stand in for one another (AUZ-12,
//! IDN-03).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::{Signature, SigningKey, signature::Signer as _};
use shako_core::{
    Action, Authorizer, BranchProtectionRule, InstanceId, LocalAuthorizer, ObjectTarget, OrgId,
    RefPolicy, RefUpdate, RequestCtx, RoleBinding, ShakoSigningKey, Subject, TokenVerifier,
};

const ORG: OrgId = OrgId::from_u128(10);
const INSTANCE: InstanceId = InstanceId::from_u128(1);
const SECRET: [u8; 32] = [7; 32];

fn bound(principal: &str) -> RoleBinding {
    RoleBinding {
        org_id: ORG,
        principal: principal.into(),
        actions: vec![Action::Write],
        project_id: None,
        repo: None,
    }
}

fn may_read(authz: &LocalAuthorizer, subject: Subject) -> bool {
    let target = ObjectTarget::Organization {
        instance_id: INSTANCE,
        org_id: ORG,
        owner: "city".into(),
    };
    let ctx = RequestCtx::new(INSTANCE, ORG, subject);
    authz.check(&ctx, Action::Read, &target).allowed
}

/// A token shako's verifier accepts, with the claims given.
fn token(claims: &serde_json::Value) -> (TokenVerifier, String) {
    let key = ShakoSigningKey::from_secret_bytes(&SECRET).unwrap();
    let header = serde_json::json!({"alg": "ES256", "kid": key.kid});
    let part = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
    let signed = format!("{}.{}", part(&header), part(claims));
    let signature: Signature = SigningKey::from_slice(&SECRET)
        .unwrap()
        .sign(signed.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    (TokenVerifier::new(key), format!("{signed}.{signature}"))
}

fn claims(sub: &str) -> serde_json::Value {
    serde_json::json!({
        "sub": sub,
        "aud": "shako",
        "exp": jc_time::TimestampTz::now().as_second() + 300,
        "jc_org": ORG.to_string(),
    })
}

#[test]
fn a_username_equal_to_a_group_name_gets_no_group_rights() {
    let authz = LocalAuthorizer::new(vec![bound("writers")]);
    let named = Subject::authenticated("u-1", "writers", vec![ORG], vec![]);
    assert!(!may_read(&authz, named));
}

#[test]
fn a_username_equal_to_a_subject_id_gets_none_of_its_rights() {
    let authz = LocalAuthorizer::new(vec![bound("u-portal")]);
    let named = Subject::authenticated("u-2", "u-portal", vec![ORG], vec![]);
    assert!(!may_read(&authz, named));
}

#[test]
fn a_groups_claim_gives_no_rights() {
    let authz = LocalAuthorizer::new(vec![bound("writers")]);
    let mut claims = claims("u-3");
    claims["groups"] = serde_json::json!(["writers"]);
    let (verifier, token) = token(&claims);
    let subject = verifier.verify(&token, "shako").expect("a valid token");
    assert!(!may_read(&authz, subject));
}

#[test]
fn a_token_without_a_subject_id_is_nobody() {
    let (verifier, token) = token(&claims(""));
    assert!(verifier.verify(&token, "shako").is_err());
}

#[test]
fn a_username_on_an_allow_list_does_not_pass_it() {
    let authz = LocalAuthorizer::new(vec![bound("u-portal"), bound("u-2")]);
    let named = Subject::authenticated("u-2", "u-portal", vec![ORG], vec![]);
    let policy = RefPolicy {
        branch_rules: vec![BranchProtectionRule {
            rule_name: "main".into(),
            enable_push: true,
            enable_push_whitelist: true,
            push_whitelist_usernames: vec!["u-portal".into()],
            ..BranchProtectionRule::default()
        }],
        tag_rules: vec![],
    };
    let ctx = RequestCtx::new(INSTANCE, ORG, named);
    assert!(
        policy
            .check_ref_update(
                &ctx,
                &authz,
                "city",
                "config",
                RefUpdate {
                    refname: "refs/heads/main",
                    is_delete: false,
                    is_create: false,
                    is_fast_forward: true,
                    changed_paths: &[],
                }
            )
            .is_err()
    );
}
