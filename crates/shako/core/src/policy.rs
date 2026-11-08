#![forbid(unsafe_code)]
//! Reference policy: branch and tag protection evaluation on every ref-moving path (S5, R§1.4, R§3.3, T7, T8).

use crate::auth::{Action, Authorizer, ObjectTarget, RequestCtx};
use crate::error::{ShakoError, ShakoResult};
use crate::ids::{InstanceId, OrgId};
use crate::model::{
    BranchProtectionRule, CommitStatusRecord, ReviewRecord, ReviewVerdict, TagProtectionRule,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::BTreeMap;

/// Context for evaluating a Pull Request merge against a branch protection rule (S5, S10).
#[derive(Debug, Clone)]
pub struct MergePolicyInput<'a> {
    pub pr_author_login: &'a str,
    pub head_sha: &'a str,
    pub is_behind_base: bool,
    pub reviews: &'a [ReviewRecord],
    pub statuses: &'a [CommitStatusRecord],
}

/// Evaluates branch and tag protection rules for one repository (S5).
#[derive(Debug, Clone, Default)]
pub struct RefPolicy {
    pub branch_rules: Vec<BranchProtectionRule>,
    pub tag_rules: Vec<TagProtectionRule>,
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == candidate {
        return true;
    }
    Glob::new(pattern)
        .ok()
        .map(|g| g.compile_matcher().is_match(candidate))
        .unwrap_or(false)
}

fn compile_glob_list(patterns_semicolon_or_comma: &str) -> Option<GlobSet> {
    let trimmed = patterns_semicolon_or_comma.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    let mut count = 0usize;
    for raw in trimmed.split([';', ',', '\n']) {
        let pat = raw.trim();
        if pat.is_empty() {
            continue;
        }
        if let Ok(g) = Glob::new(pat) {
            builder.add(g);
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    builder.build().ok()
}

/// One ref move checked by [`RefPolicy::check_ref_update`]: the full ref name, what kind of move it
/// is, and the paths the new tip changes.
#[derive(Debug, Clone, Copy)]
pub struct RefUpdate<'a> {
    pub refname: &'a str,
    pub is_delete: bool,
    pub is_create: bool,
    pub is_fast_forward: bool,
    pub changed_paths: &'a [String],
}

impl RefPolicy {
    /// Validates a `BranchProtectionRule` before saving it:
    /// - Deploy keys and bypass allowlists must be `false` (R§3.3).
    /// - Every subject named in `push_whitelist_usernames`, `merge_whitelist_usernames`, or
    ///   `force_push_allowlist_usernames` MUST hold `Action::Write` on the repository; if not,
    ///   refuses with `422 Unprocessable` naming that subject (S5, Threat T8, `KNOWLEDGE.md:190`).
    pub fn validate_rule_for_repo(
        rule: &BranchProtectionRule,
        authorizer: &dyn Authorizer,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo: &str,
    ) -> ShakoResult<()> {
        if rule.rule_name.trim().is_empty() {
            return Err(ShakoError::Unprocessable(
                "protection rule_name must not be empty".into(),
            ));
        }
        if rule.enable_bypass_allowlist
            || rule.push_whitelist_deploy_keys
            || rule.force_push_allowlist_deploy_keys
        {
            return Err(ShakoError::Unprocessable(
                "deploy keys and bypass allowlists are not supported (must be false)".into(),
            ));
        }
        for subject in rule
            .push_whitelist_usernames
            .iter()
            .chain(rule.merge_whitelist_usernames.iter())
            .chain(rule.force_push_allowlist_usernames.iter())
        {
            if !authorizer.principal_has_write(instance_id, org_id, owner, repo, subject) {
                return Err(ShakoError::Unprocessable(format!(
                    "whitelisted subject '{subject}' does not have write access on {owner}/{repo}"
                )));
            }
        }
        Ok(())
    }

    /// Validates a `TagProtectionRule` before saving it.
    pub fn validate_tag_rule_for_repo(
        rule: &TagProtectionRule,
        authorizer: &dyn Authorizer,
        instance_id: InstanceId,
        org_id: OrgId,
        owner: &str,
        repo: &str,
    ) -> ShakoResult<()> {
        if rule.name_pattern.trim().is_empty() {
            return Err(ShakoError::Unprocessable(
                "tag protection name_pattern must not be empty".into(),
            ));
        }
        for subject in &rule.whitelist_usernames {
            if !authorizer.principal_has_write(instance_id, org_id, owner, repo, subject) {
                return Err(ShakoError::Unprocessable(format!(
                    "whitelisted subject '{subject}' does not have write access on {owner}/{repo}"
                )));
            }
        }
        Ok(())
    }

    /// Finds the first matching branch protection rule for `branch` (without `refs/heads/`).
    #[must_use]
    pub fn matching_branch_rule(&self, branch: &str) -> Option<&BranchProtectionRule> {
        let short = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        self.branch_rules
            .iter()
            .find(|r| glob_matches(&r.rule_name, short))
    }

    /// Finds the first matching tag protection rule for `tag` (without `refs/tags/`).
    #[must_use]
    pub fn matching_tag_rule(&self, tag: &str) -> Option<&TagProtectionRule> {
        let short = tag.strip_prefix("refs/tags/").unwrap_or(tag);
        self.tag_rules
            .iter()
            .find(|r| glob_matches(&r.name_pattern, short))
    }

    /// Evaluates whether a direct push or API commit (not a PR merge) may update/create/delete `refname`.
    pub fn check_ref_update(
        &self,
        ctx: &RequestCtx,
        authorizer: &dyn Authorizer,
        owner: &str,
        repo: &str,
        update: RefUpdate<'_>,
    ) -> ShakoResult<()> {
        let RefUpdate {
            refname,
            is_delete,
            is_create,
            is_fast_forward,
            changed_paths,
        } = update;
        let target = ObjectTarget::Repository {
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
            project_id: None,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            git_ref: Some(refname.to_owned()),
        };

        // 1. Tag protection (`refs/tags/*`)
        if let Some(tag_name) = refname.strip_prefix("refs/tags/") {
            if let Some(tag_rule) = self.matching_tag_rule(tag_name) {
                if is_delete || !is_create {
                    return Err(ShakoError::Forbidden(format!(
                        "tag '{tag_name}' is protected and cannot be moved or deleted"
                    )));
                }
                authorizer
                    .check(ctx, Action::Write, &target)
                    .into_result("repository not found")?;
                let principals = ctx.subject.principals();
                if !tag_rule.whitelist_usernames.is_empty()
                    && !tag_rule
                        .whitelist_usernames
                        .iter()
                        .any(|u| principals.contains(&u.as_str()))
                {
                    return Err(ShakoError::Forbidden(format!(
                        "tag '{tag_name}' is protected: subject '{}' is not in tag whitelist",
                        ctx.subject.username
                    )));
                }
                return Ok(());
            }
            // Unprotected tag still requires at least Propose/Write
            authorizer
                .check(ctx, Action::Propose, &target)
                .into_result("repository not found")?;
            return Ok(());
        }

        // 2. Branch protection (`refs/heads/*`)
        let branch_name = refname.strip_prefix("refs/heads/").unwrap_or(refname);
        let Some(rule) = self.matching_branch_rule(branch_name) else {
            // Unprotected branch: requires `Action::Propose` (or `Action::Write`)
            authorizer
                .check(ctx, Action::Propose, &target)
                .into_result("repository not found")?;
            return Ok(());
        };

        // Protected branch requires `Action::Write`
        authorizer
            .check(ctx, Action::Write, &target)
            .into_result("repository not found")?;

        // Protected branches can never be deleted
        if is_delete {
            return Err(ShakoError::Forbidden(format!(
                "branch '{branch_name}' is protected and cannot be deleted"
            )));
        }

        // Direct push disabled (`enable_push: false`)
        if !rule.enable_push {
            return Err(ShakoError::Forbidden(format!(
                "branch '{branch_name}' is protected: direct pushes are disabled"
            )));
        }

        let principals = ctx.subject.principals();
        let whitelist_active =
            rule.enable_push_whitelist || !rule.push_whitelist_usernames.is_empty();
        if whitelist_active
            && !rule
                .push_whitelist_usernames
                .iter()
                .any(|u| principals.contains(&u.as_str()))
        {
            return Err(ShakoError::Forbidden(format!(
                "branch '{branch_name}' is protected: subject '{}' is not in push_whitelist_usernames",
                ctx.subject.username
            )));
        }

        if !is_create && !is_fast_forward {
            if !rule.enable_force_push {
                return Err(ShakoError::Forbidden(format!(
                    "branch '{branch_name}' is protected: non-fast-forward force push is disabled"
                )));
            }
            let fp_whitelist_active =
                rule.enable_force_push_allowlist || !rule.force_push_allowlist_usernames.is_empty();
            if fp_whitelist_active
                && !rule
                    .force_push_allowlist_usernames
                    .iter()
                    .any(|u| principals.contains(&u.as_str()))
            {
                return Err(ShakoError::Forbidden(format!(
                    "branch '{branch_name}' is protected: subject '{}' is not in force_push_allowlist_usernames",
                    ctx.subject.username
                )));
            }
        }

        // Check `protected_file_patterns` and `unprotected_file_patterns`
        if let Some(prot_set) = compile_glob_list(&rule.protected_file_patterns) {
            let unprot_set = compile_glob_list(&rule.unprotected_file_patterns);
            for path in changed_paths {
                let is_prot = prot_set.is_match(path);
                let is_unprot = unprot_set.as_ref().is_some_and(|u| u.is_match(path));
                if is_prot && !is_unprot {
                    return Err(ShakoError::Forbidden(format!(
                        "path '{path}' is protected by branch rule '{}'",
                        rule.rule_name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Evaluates whether a Pull Request into `base_branch` may be merged by `ctx` (S5, S7, S10).
    pub fn check_pr_merge(
        &self,
        ctx: &RequestCtx,
        authorizer: &dyn Authorizer,
        owner: &str,
        repo: &str,
        base_branch: &str,
        input: &MergePolicyInput<'_>,
    ) -> ShakoResult<()> {
        let target = ObjectTarget::Repository {
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
            project_id: None,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            git_ref: Some(format!("refs/heads/{base_branch}")),
        };
        authorizer
            .check(ctx, Action::Write, &target)
            .into_result("repository not found")?;

        let Some(rule) = self.matching_branch_rule(base_branch) else {
            return Ok(());
        };

        let principals = ctx.subject.principals();
        let is_admin = authorizer.check(ctx, Action::Admin, &target).allowed;
        let can_admin_override = is_admin && !rule.block_admin_merge_override;

        // 1. Merge whitelist check (`block_admin_merge_override` blocks admins too!)
        let merge_wl_active =
            rule.enable_merge_whitelist || !rule.merge_whitelist_usernames.is_empty();
        if merge_wl_active
            && !can_admin_override
            && !rule
                .merge_whitelist_usernames
                .iter()
                .any(|u| principals.contains(&u.as_str()))
        {
            return Err(ShakoError::Forbidden(format!(
                "branch '{base_branch}' is protected: subject '{}' is not in merge_whitelist_usernames",
                ctx.subject.username
            )));
        }

        // 2. Outdated branch check: MUST return 405 with "behind the base branch" (S5, S7)
        if rule.block_on_outdated_branch && input.is_behind_base && !can_admin_override {
            return Err(ShakoError::BehindBaseBranch(
                "Pull request is behind the base branch".into(),
            ));
        }

        // 3. Reviews evaluation (latest non-dismissed verdict per reviewer)
        let mut latest_by_author: BTreeMap<&str, (&ReviewRecord, bool)> = BTreeMap::new();
        for rev in input.reviews {
            if rev.dismissed || rev.verdict == ReviewVerdict::Comment {
                continue;
            }
            let stale = rule.dismiss_stale_approvals
                && !rev.commit_id.is_empty()
                && rev.commit_id != input.head_sha;
            latest_by_author.insert(rev.author_login.as_str(), (rev, stale));
        }

        if rule.block_on_rejected_reviews && !can_admin_override {
            let rejected = latest_by_author
                .values()
                .any(|(rev, _)| rev.verdict == ReviewVerdict::RequestChanges);
            if rejected {
                return Err(ShakoError::MethodNotAllowed(
                    "Pull request is blocked by rejected reviews".into(),
                ));
            }
        }

        if rule.required_approvals > 0 && !can_admin_override {
            let approvals = latest_by_author
                .iter()
                .filter(|(reviewer, (rev, stale))| {
                    rev.verdict == ReviewVerdict::Approved
                        && !*stale
                        && **reviewer != input.pr_author_login
                        && (!rule.enable_approvals_whitelist
                            || rule
                                .approvals_whitelist_username
                                .iter()
                                .any(|u| u == *reviewer))
                })
                .count() as u64;
            if approvals < rule.required_approvals {
                return Err(ShakoError::MethodNotAllowed(format!(
                    "Pull request requires {} approval(s), has {approvals}",
                    rule.required_approvals
                )));
            }
        }

        // 4. Required status checks
        if (rule.enable_status_check || !rule.status_check_contexts.is_empty())
            && !can_admin_override
        {
            for req_ctx in &rule.status_check_contexts {
                let passed = input.statuses.iter().any(|s| {
                    s.sha == input.head_sha && &s.context == req_ctx && s.state == "success"
                });
                if !passed {
                    return Err(ShakoError::MethodNotAllowed(format!(
                        "Required status check '{req_ctx}' has not succeeded"
                    )));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{LocalAuthorizer, RoleBinding, Subject};
    use chrono::Utc;

    #[test]
    fn ref_policy_enforces_whitelist_outdated_base_reviews_and_admin_override() {
        let inst = InstanceId::from_u128(1);
        let org = OrgId::from_u128(1);
        let authz = LocalAuthorizer::new(vec![
            RoleBinding {
                org_id: org,
                principal: "jc-portal".into(),
                actions: vec![Action::Write],
                project_id: None,
                repo: None,
            },
            RoleBinding {
                org_id: org,
                principal: "writer-bob".into(),
                actions: vec![Action::Write, Action::Approve],
                project_id: None,
                repo: None,
            },
            RoleBinding {
                org_id: org,
                principal: "admin-root".into(),
                actions: vec![Action::Admin],
                project_id: None,
                repo: None,
            },
            RoleBinding {
                org_id: org,
                principal: "reader-eve".into(),
                actions: vec![Action::Read],
                project_id: None,
                repo: None,
            },
        ]);

        // T8: whitelisting reader-eve (without write) is refused with 422 naming reader-eve
        let bad_rule = BranchProtectionRule {
            rule_name: "main".into(),
            enable_push: true,
            enable_push_whitelist: true,
            push_whitelist_usernames: vec!["reader-eve".into()],
            ..BranchProtectionRule::default()
        };
        let err = RefPolicy::validate_rule_for_repo(&bad_rule, &authz, inst, org, "jc", "cfg")
            .expect_err("reader-eve must be refused");
        assert_eq!(err.status_code(), 422);
        assert!(err.to_string().contains("reader-eve"));

        // Platform bootstrap rule
        let platform_rule = BranchProtectionRule {
            rule_name: "main".into(),
            enable_push: true,
            enable_push_whitelist: true,
            push_whitelist_usernames: vec!["jc-portal".into()],
            enable_force_push: false,
            enable_merge_whitelist: true,
            merge_whitelist_usernames: vec!["jc-portal".into()],
            required_approvals: 1,
            dismiss_stale_approvals: true,
            block_on_outdated_branch: true,
            block_on_rejected_reviews: true,
            block_admin_merge_override: true,
            protected_file_patterns: ".gitea/**".into(),
            ..BranchProtectionRule::default()
        };
        RefPolicy::validate_rule_for_repo(&platform_rule, &authz, inst, org, "jc", "cfg")
            .expect("jc-portal has write");

        let policy = RefPolicy {
            branch_rules: vec![platform_rule],
            tag_rules: vec![TagProtectionRule {
                name_pattern: "v*".into(),
                whitelist_usernames: vec!["jc-portal".into()],
            }],
        };

        let portal_ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
        );
        let bob_ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("writer-bob", "writer-bob", vec![org], vec![]),
        );
        let admin_ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("admin-root", "admin-root", vec![org], vec![]),
        );

        // Non-whitelisted writer-bob and even admin-root cannot push directly to main
        assert!(
            policy
                .check_ref_update(
                    &bob_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: false,
                        is_create: false,
                        is_fast_forward: true,
                        changed_paths: &[]
                    }
                )
                .is_err()
        );
        assert!(
            policy
                .check_ref_update(
                    &admin_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: false,
                        is_create: false,
                        is_fast_forward: true,
                        changed_paths: &[]
                    }
                )
                .is_err()
        );
        // jc-portal can push fast-forward, but NOT force-push, NOT delete, and NOT touch .gitea/
        assert!(
            policy
                .check_ref_update(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: false,
                        is_create: false,
                        is_fast_forward: true,
                        changed_paths: &["a.yaml".into()]
                    }
                )
                .is_ok()
        );
        assert!(
            policy
                .check_ref_update(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: false,
                        is_create: false,
                        is_fast_forward: false,
                        changed_paths: &[]
                    }
                )
                .is_err()
        );
        assert!(
            policy
                .check_ref_update(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: true,
                        is_create: false,
                        is_fast_forward: true,
                        changed_paths: &[]
                    }
                )
                .is_err()
        );
        assert!(
            policy
                .check_ref_update(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    RefUpdate {
                        refname: "refs/heads/main",
                        is_delete: false,
                        is_create: false,
                        is_fast_forward: true,
                        changed_paths: &[".gitea/workflows/build.yml".into()]
                    }
                )
                .is_err()
        );

        // Merge behind base -> 405 with "behind the base branch"
        let behind_err = policy
            .check_pr_merge(
                &portal_ctx,
                &authz,
                "jc",
                "cfg",
                "main",
                &MergePolicyInput {
                    pr_author_login: "writer-bob",
                    head_sha: "sha2",
                    is_behind_base: true,
                    reviews: &[],
                    statuses: &[],
                },
            )
            .expect_err("behind base must fail");
        assert_eq!(behind_err.status_code(), 405);
        assert!(behind_err.to_string().contains("behind the base branch"));

        // Stale approval (on sha1 instead of sha2) does not satisfy required_approvals
        let stale_rev = ReviewRecord {
            id: uuid::Uuid::now_v7(),
            instance_id: inst,
            org_id: org,
            repo_id: crate::ids::RepoId::from_u128(1),
            pr_number: 1,
            verdict: ReviewVerdict::Approved,
            body: "lgtm".into(),
            author_login: "jc-portal".into(),
            commit_id: "sha1".into(),
            dismissed: false,
            created_at: Utc::now(),
        };
        assert!(
            policy
                .check_pr_merge(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    "main",
                    &MergePolicyInput {
                        pr_author_login: "writer-bob",
                        head_sha: "sha2",
                        is_behind_base: false,
                        reviews: std::slice::from_ref(&stale_rev),
                        statuses: &[],
                    },
                )
                .is_err()
        );

        // Fresh approval on sha2 by someone other than pr_author_login succeeds
        let fresh_rev = ReviewRecord {
            commit_id: "sha2".into(),
            ..stale_rev
        };
        assert!(
            policy
                .check_pr_merge(
                    &portal_ctx,
                    &authz,
                    "jc",
                    "cfg",
                    "main",
                    &MergePolicyInput {
                        pr_author_login: "writer-bob",
                        head_sha: "sha2",
                        is_behind_base: false,
                        reviews: &[fresh_rev],
                        statuses: &[],
                    },
                )
                .is_ok()
        );
    }
}
