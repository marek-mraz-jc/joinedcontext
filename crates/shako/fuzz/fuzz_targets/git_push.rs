//! What a writer sends to receive-pack: a command list, then a pack. Each input lands in a fresh
//! repository with one commit on `main`, pushed by a subject holding Write, so a valid push is
//! applied and an invalid one refused; never a panic, and never a ref outside `refs/heads/`,
//! `refs/tags/` or `refs/pull/`.
#![no_main]

use std::io::Write as _;

use libfuzzer_sys::fuzz_target;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, Principal, RefPolicy, RepoId, RequestCtx,
    RoleBinding, Subject, validate_full_ref_name,
};
use shako_git::{PushGuard, handle_receive_pack, init_bare_repo};

fn admit(bytes: u64) -> shako_core::ShakoResult<()> {
    if bytes > 64 << 20 {
        return Err(shako_core::ShakoError::Overloaded("fuzz budget".into()));
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let root = tempfile::tempdir().expect("a repository root");
    let (inst, org) = (InstanceId::from_u128(1), OrgId::from_u128(1));
    let repo = init_bare_repo(
        root.path(),
        inst,
        RepoId::from_u128(1),
        "main",
        true,
        "fuzz",
    )
    .expect("a seeded repository");
    let authz = LocalAuthorizer::new(vec![RoleBinding {
        org_id: org,
        principal: Principal::User("writer".into()),
        actions: vec![Action::Write],
        project_id: None,
        repo: None,
    }]);
    let ctx = RequestCtx::new(
        inst,
        org,
        Subject::authenticated("writer", "writer", vec![org]),
    );
    let policy = RefPolicy::default();
    let guard = PushGuard {
        ctx: &ctx,
        authorizer: &authz,
        owner: "jc",
        repo_name: "fuzz",
        policy: &policy,
    };
    let mut body = tempfile::tempfile().expect("spool");
    body.write_all(data).expect("spool write");
    let mut budget = admit;
    if let Ok(outcome) = handle_receive_pack(&repo, &guard, 1 << 20, &body, &mut budget) {
        for cmd in &outcome.updated_refs {
            assert!(
                validate_full_ref_name(&cmd.refname).is_ok(),
                "{}",
                cmd.refname
            );
            assert!(cmd.refname.starts_with("refs/"), "{}", cmd.refname);
        }
    }
});
