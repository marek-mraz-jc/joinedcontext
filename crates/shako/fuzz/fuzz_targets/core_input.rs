//! The names and headers shako-core reads from a request before any store sees them. The first
//! byte picks the reader: 0 owner, repository, branch, tag, tree path and package names, 1 an
//! `Authorization` header and the bearer it carries, 2 a branch protection rule body checked and
//! then applied to a push. A name a validator accepts must hold no `..` segment, no NUL and no
//! backslash, and nothing may panic.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use shako_core::{
    Action, BranchProtectionRule, InstanceId, LocalAuthorizer, OrgId, Principal, RefPolicy,
    RefUpdate, RequestCtx, RoleBinding, ShakoSigningKey, Subject, TokenVerifier,
    validate_branch_name, validate_full_ref_name, validate_package_segment,
    validate_repo_or_owner_name, validate_tag_name, validate_tree_path,
};

static VERIFIER: LazyLock<TokenVerifier> =
    LazyLock::new(|| TokenVerifier::new(ShakoSigningKey::generate().expect("a signing key")));

fn assert_tame(name: &str) {
    assert!(!name.contains('\0') && !name.contains('\\'), "{name:?}");
    assert!(!name.split('/').any(|s| s == ".."), "{name:?}");
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    match mode % 3 {
        0 => {
            for (what, ok) in [
                ("owner", validate_repo_or_owner_name(&text, "owner").is_ok()),
                (
                    "repo",
                    validate_repo_or_owner_name(&text, "repository").is_ok(),
                ),
                ("branch", validate_branch_name(&text).is_ok()),
                ("tag", validate_tag_name(&text).is_ok()),
                ("ref", validate_full_ref_name(&text).is_ok()),
                (
                    "package",
                    validate_package_segment(&text, "version").is_ok(),
                ),
            ] {
                if ok {
                    assert_tame(&text);
                    assert!(!text.is_empty(), "{what}");
                }
            }
            if let Ok(clean) = validate_tree_path(&text) {
                assert_tame(&clean);
                assert!(!clean.starts_with('/'));
            }
        }
        1 => {
            if let Some(token) = TokenVerifier::extract_raw_token(&text) {
                assert!(!token.is_empty());
                // No static binding and no JWKS: a fuzzed bearer never verifies.
                assert!(VERIFIER.verify(&token, "shako").is_err());
            }
        }
        _ => {
            let Ok(rule) = serde_json::from_slice::<BranchProtectionRule>(rest) else {
                return;
            };
            let (inst, org) = (InstanceId::from_u128(1), OrgId::from_u128(1));
            let authz = LocalAuthorizer::new(vec![RoleBinding {
                org_id: org,
                principal: Principal::User("writer".into()),
                actions: vec![Action::Write],
                project_id: None,
                repo: None,
            }]);
            if RefPolicy::validate_rule_for_repo(&rule, &authz, inst, org, "jc", "cfg").is_err() {
                return;
            }
            let policy = RefPolicy {
                branch_rules: vec![rule],
                tag_rules: vec![],
            };
            let ctx = RequestCtx::new(
                inst,
                org,
                Subject::authenticated("writer", "writer", vec![org]),
            );
            let paths = ["docs/a.md".to_owned(), "src/main.rs".to_owned()];
            for (is_delete, is_create, ff) in [
                (false, false, true),
                (true, false, false),
                (false, false, false),
            ] {
                let _ = policy.check_ref_update(
                    &ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete,
                        is_create,
                        is_fast_forward: ff,
                        changed_paths: &paths,
                    },
                );
            }
        }
    }
});
