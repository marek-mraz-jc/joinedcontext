#![forbid(unsafe_code)]
//! OData 4.01 service at `{mount}/odata/` (Contract S9, R§9) built on `odata-table`:
//! - `$metadata` (CSDL XML and JSON, namespace `Shako`)
//! - Entity sets: `Instances`, `Repositories`, `Branches`, `Tags`, `Commits`, `PullRequests`,
//!   `Reviews`, `ProtectionRules`, `Webhooks`, `Deliveries`, `PushMirrors`, `Teams`,
//!   `Packages`, `PackageVersions`, `PackageFiles`, `Events`.
//! - Git-backed sets (`Branches`, `Tags`, `Commits`) require `RepoId` in the key or `$filter`
//!   (an unbounded scan on `Commits` answers `501 Not Implemented`, Contract S9).
//! - Secret fields are never declared, so `$filter` or `$orderby` on them returns `400 Bad Request`.
//! - `$expand=Files,Reviews` on `PullRequests` is scoped strictly to the caller's organization.
//! - Bound and unbound actions/functions (`CreateRepository`, `Merge`, `CommitFiles`, `Protect`,
//!   `Tree`, `File`, `History`, `Compare`, `Mergeable`, `Allowed`, etc.).

use crate::service;
use crate::state::{
    ShakoState, authenticate_request, odata_error_response, resolve_and_authorize_repo,
};
use axum::extract::{DefaultBodyLimit, Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use jc_router::Mount;
use odata_table::{
    MemoryTable, Operation as TableOp, Table, TableError, allowed as table_allowed,
    export as table_export, key_options, query as table_query,
};
use serde_json::{Value, json};
use shako_core::{
    Action, BranchProtectionRule, ObjectTarget, PullRequestRecord, PushMirrorRecord, RepoId,
    RepoRecord, RequestCtx, ReviewRecord, ReviewVerdict, ShakoError, ShakoResult, WebhookFormat,
    WebhookId, repo_disk_path, scope_for_context, validate_tree_path,
};
use shako_git::{
    CommitAuthor, MergeStyle, check_mergeability, diff_refs_files, list_branches, list_commits,
    list_tags, list_tree_entries, read_commit_info, read_file_at_ref,
};
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_ODATA_PAGE: u64 = 500;

fn map_table_error(err: TableError) -> ShakoError {
    match err {
        TableError::NotImplemented(w) => ShakoError::NotImplemented(w),
        TableError::UnknownProperty(p) => {
            ShakoError::BadRequest(format!("unknown OData property '{p}'"))
        }
        TableError::BadRequest(b) => ShakoError::BadRequest(b),
        TableError::Source(s) => ShakoError::Internal(s),
    }
}

fn split_paren_segment(seg: &str) -> (&str, Option<&str>) {
    if let Some((head, tail)) = seg.split_once('(')
        && let Some(inner) = tail.strip_suffix(')')
    {
        return (head, Some(inner));
    }
    (seg, None)
}

/// An OData literal as its value: `'a''b'` is `a'b`; anything unquoted stays as written.
fn odata_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        t[1..t.len() - 1].replace("''", "'")
    } else {
        t.trim_matches('"').to_owned()
    }
}

/// Splits `a='x,y',b=2` at the commas outside quoted literals.
fn split_args(inner: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&inner[start..]);
    out
}

fn unquote_odata_key(raw: &str) -> String {
    let t = raw.trim();
    match t.split_once('=') {
        Some((k, v)) if !k.contains('\'') => odata_literal(v),
        _ => odata_literal(t),
    }
}

/// The arguments of a key or function call: `(Path=@p,Ref='main')` with the query string's
/// parameter aliases (`@p='a/b.yaml'`) resolved, plus plain query parameters.
fn parse_func_args(parens: Option<&str>, raw_query: Option<&str>) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    if let Some(q) = raw_query
        && let Ok(pairs) = serde_urlencoded::from_str::<Vec<(String, String)>>(q)
    {
        for (k, v) in pairs {
            query.insert(k, v);
        }
    }
    let mut map: BTreeMap<String, String> = query
        .iter()
        .filter(|(k, _)| !k.starts_with('@') && !k.starts_with('$'))
        .map(|(k, v)| (k.clone(), odata_literal(v)))
        .collect();
    if let Some(inner) = parens {
        for part in split_args(inner) {
            if let Some((k, v)) = part.split_once('=') {
                let v = v.trim();
                let value = if v.starts_with('@') {
                    query.get(v).map(|a| odata_literal(a)).unwrap_or_default()
                } else {
                    odata_literal(v)
                };
                map.insert(k.trim().to_owned(), value);
            }
        }
    }
    map
}

/// Extracts `RepoId` from either a composite key `"{RepoId}:{rest}"` or a `$filter` expression
/// containing `RepoId eq '<uuid>'`.
fn extract_repo_id_from_key_or_query(
    key_parens: Option<&str>,
    raw_query: Option<&str>,
) -> Option<(RepoId, Option<String>)> {
    if let Some(k) = key_parens {
        let unq = unquote_odata_key(k);
        if let Some((r_str, rest)) = unq.split_once(':')
            && let Ok(rid) = r_str.parse::<RepoId>()
        {
            return Some((rid, Some(rest.to_owned())));
        }
    }
    let q = raw_query?;
    let decoded = odata_url::percent_decoded(q).unwrap_or_else(|| q.to_owned());
    let mut rest_str = decoded.as_str();
    while let Some(pos) = rest_str.find("RepoId") {
        let after = &rest_str[pos + "RepoId".len()..];
        let trimmed = after.trim_start();
        if let Some(after_eq) = trimmed.strip_prefix("eq") {
            let val_part = after_eq.trim_start();
            if let Some(quoted) = val_part.strip_prefix('\'')
                && let Some((uuid_s, _)) = quoted.split_once('\'')
                && let Ok(rid) = uuid_s.parse::<RepoId>()
            {
                let ref_hint = extract_string_eq_from_filter(&decoded, "Ref");
                return Some((rid, ref_hint));
            }
        }
        rest_str = after;
    }
    None
}

fn extract_string_eq_from_filter(decoded_query: &str, field: &str) -> Option<String> {
    let pos = decoded_query.find(field)?;
    let after = &decoded_query[pos + field.len()..];
    let after_eq = after.trim_start().strip_prefix("eq")?.trim_start();
    let quoted = after_eq.strip_prefix('\'')?;
    let (val, _) = quoted.split_once('\'')?;
    Some(val.to_owned())
}

/// Builds an `odata-table::MemoryTable` for `set_name` scoped to `ctx` (Contract S9).
async fn build_entity_table(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    set_name: &str,
    key_parens: Option<&str>,
    raw_query: Option<&str>,
) -> ShakoResult<MemoryTable> {
    let model = Arc::new(state.registry.service.model());
    let info = state
        .registry
        .service
        .info(model, set_name)
        .ok_or_else(|| ShakoError::NotFound(format!("unknown OData entity set '{set_name}'")))?;

    let visible_repos: Vec<RepoRecord> = state
        .store()
        .list_repos(ctx.instance_id, ctx.org_id)
        .await?
        .into_iter()
        .filter(|r| {
            state
                .authorizer
                .check(
                    ctx,
                    Action::Read,
                    &ObjectTarget::Repository {
                        instance_id: r.instance_id,
                        org_id: r.org_id,
                        project_id: (!r.project_id.is_empty()).then(|| r.project_id.clone()),
                        owner: r.owner.clone(),
                        repo: r.name.clone(),
                        git_ref: None,
                    },
                )
                .allowed
        })
        .collect();

    let rows: Vec<Value> = match set_name {
        "Instances" => {
            let inst = state.instance_for(ctx.instance_id, ctx.org_id).await;
            let usage = state
                .store()
                .get_quota_usage(ctx.instance_id, ctx.org_id)
                .await?;
            if state
                .authorizer
                .check(
                    ctx,
                    Action::Read,
                    &ObjectTarget::Instance {
                        instance_id: ctx.instance_id,
                        org_id: ctx.org_id,
                    },
                )
                .allowed
            {
                vec![json!({
                    "Id": inst.instance_id.to_string(),
                    "Host": inst.host,
                    "MaxRepos": inst.max_repos,
                    "MaxStorageBytes": inst.max_storage_bytes,
                    "RepoCount": usage.repo_count,
                    "GitBytes": usage.git_bytes,
                    "PackageBytes": usage.package_bytes,
                })]
            } else {
                Vec::new()
            }
        }
        "Repositories" => visible_repos.iter().map(|r| repo_row(mount, r)).collect(),
        "Branches" => {
            let Some((repo_id, _)) = extract_repo_id_from_key_or_query(key_parens, raw_query)
            else {
                return Err(ShakoError::BadRequest(
                    "Branches query requires RepoId in key or $filter (e.g. $filter=RepoId eq '...')"
                        .into(),
                ));
            };
            let Some(repo) = visible_repos.iter().find(|r| r.id == repo_id) else {
                return MemoryTable::new(info, Vec::new()).map_err(map_table_error);
            };
            let policy = state
                .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
                .await?;
            let disk_path = repo_disk_path(state.repo_root(), repo.instance_id, repo.id)?;
            let branches = tokio::task::spawn_blocking(move || list_branches(&disk_path))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            branches
                .into_iter()
                .map(|(name, sha)| {
                    let prot = policy.matching_branch_rule(&name).is_some();
                    json!({
                        "Id": format!("{repo_id}:{name}"),
                        "RepoId": repo_id.to_string(),
                        "Name": name,
                        "HeadSha": sha,
                        "Protected": prot,
                    })
                })
                .collect()
        }
        "Tags" => {
            let Some((repo_id, _)) = extract_repo_id_from_key_or_query(key_parens, raw_query)
            else {
                return Err(ShakoError::BadRequest(
                    "Tags query requires RepoId in key or $filter".into(),
                ));
            };
            let Some(repo) = visible_repos.iter().find(|r| r.id == repo_id) else {
                return MemoryTable::new(info, Vec::new()).map_err(map_table_error);
            };
            let disk_path = repo_disk_path(state.repo_root(), repo.instance_id, repo.id)?;
            let tags = tokio::task::spawn_blocking(move || list_tags(&disk_path))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            tags.into_iter()
                .map(|(name, _id, target_sha, annotated)| {
                    json!({
                        "Id": format!("{repo_id}:{name}"),
                        "RepoId": repo_id.to_string(),
                        "Name": name,
                        "TargetSha": target_sha,
                        "Annotated": annotated,
                    })
                })
                .collect()
        }
        "Commits" => {
            // Contract S9: Commits requires RepoId in key or $filter; unbounded scans answer 501!
            let Some((repo_id, ref_or_sha)) =
                extract_repo_id_from_key_or_query(key_parens, raw_query)
            else {
                return Err(ShakoError::NotImplemented(
                    "unbounded Commits scan is not supported; specify RepoId in key or $filter"
                        .into(),
                ));
            };
            let Some(repo) = visible_repos.iter().find(|r| r.id == repo_id) else {
                return MemoryTable::new(info, Vec::new()).map_err(map_table_error);
            };
            let git_ref = ref_or_sha.unwrap_or_else(|| repo.default_branch.clone());
            let disk_path = repo_disk_path(state.repo_root(), repo.instance_id, repo.id)?;
            let ref_label = git_ref.clone();
            let commits = tokio::task::spawn_blocking(move || {
                list_commits(&disk_path, &git_ref, None, 1000, 10_000)
            })
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
            // `Position` keeps the walk's order through a page: a page is otherwise ordered by
            // its key, which sorts commits by their sha.
            commits
                .into_iter()
                .enumerate()
                .map(|(position, c)| {
                    let summary = c.message.lines().next().unwrap_or_default().to_owned();
                    json!({
                        "Id": format!("{repo_id}:{}", c.sha),
                        "Position": position,
                        "RepoId": repo_id.to_string(),
                        "Sha": c.sha,
                        "Ref": ref_label,
                        "Message": summary,
                        "Author": c.author_name,
                        "Email": c.author_email,
                        "Date": c.author_date,
                    })
                })
                .collect()
        }
        "PullRequests" => {
            let by_id: BTreeMap<RepoId, &RepoRecord> =
                visible_repos.iter().map(|r| (r.id, r)).collect();
            state
                .store()
                .list_pull_requests(ctx.instance_id, ctx.org_id, None, None)
                .await?
                .into_iter()
                .filter_map(|pr| by_id.get(&pr.repo_id).map(|r| pr_row(mount, r, &pr)))
                .collect()
        }
        "Reviews" => {
            let visible_ids: std::collections::BTreeSet<RepoId> =
                visible_repos.iter().map(|r| r.id).collect();
            state
                .store()
                .list_reviews(ctx.instance_id, ctx.org_id, None, None)
                .await?
                .iter()
                .filter(|r| visible_ids.contains(&r.repo_id))
                .map(review_row)
                .collect()
        }
        "ProtectionRules" => {
            let mut out = Vec::new();
            for r in &visible_repos {
                for prule in state
                    .store()
                    .list_protection_rules(ctx.instance_id, ctx.org_id, r.id)
                    .await?
                {
                    out.push(json!({
                        "Id": format!("{}:{}", r.id, prule.rule.rule_name),
                        "RepoId": r.id.to_string(),
                        "Pattern": prule.rule.rule_name,
                        "EnablePush": prule.rule.enable_push,
                        "EnableForcePush": prule.rule.enable_force_push,
                        "BlockOnOutdatedBranch": prule.rule.block_on_outdated_branch,
                        "BlockOnRejectedReviews": prule.rule.block_on_rejected_reviews,
                        "BlockAdminMergeOverride": prule.rule.block_admin_merge_override,
                        "RequiredApprovals": prule.rule.required_approvals,
                    }));
                }
            }
            out
        }
        "Webhooks" => {
            let visible_ids: std::collections::BTreeSet<RepoId> =
                visible_repos.iter().map(|r| r.id).collect();
            state
                .store()
                .list_webhooks(ctx.instance_id, ctx.org_id, None)
                .await?
                .into_iter()
                .filter(|h| visible_ids.contains(&h.repo_id))
                .map(|h| {
                    json!({
                        "Id": h.id.to_string(),
                        "RepoId": h.repo_id.to_string(),
                        "Url": h.url,
                        "Active": h.active,
                    })
                })
                .collect()
        }
        "Deliveries" => state
            .store()
            .list_deliveries(ctx.instance_id, ctx.org_id, None)
            .await?
            .into_iter()
            .map(|d| {
                json!({
                    "Id": d.id.to_string(),
                    "WebhookId": d.webhook_id.to_string(),
                    "Event": d.event,
                    "Status": d.status,
                    "ResponseCode": d.response_code,
                    "At": d.delivered_at.to_rfc3339(),
                })
            })
            .collect(),
        "PushMirrors" => {
            let visible_ids: std::collections::BTreeSet<RepoId> =
                visible_repos.iter().map(|r| r.id).collect();
            state
                .store()
                .list_push_mirrors(ctx.instance_id, ctx.org_id, None)
                .await?
                .iter()
                .filter(|m| visible_ids.contains(&m.repo_id))
                .map(mirror_row)
                .collect()
        }
        "Teams" => {
            let mut out = Vec::new();
            for r in &visible_repos {
                for (group, verb) in state.authorizer.bindings_for_repo(ctx.org_id, &r.name) {
                    out.push(json!({
                        "Id": format!("{}:{group}", r.id),
                        "RepoId": r.id.to_string(),
                        "Group": group,
                        "Verb": verb.as_str(),
                    }));
                }
            }
            out
        }
        "Packages" => state
            .store()
            .list_packages(ctx.instance_id, ctx.org_id)
            .await?
            .into_iter()
            .map(|p| {
                json!({
                    "Id": p.id.to_string(),
                    "Owner": p.owner,
                    "Type": p.package_type,
                    "Name": p.name,
                    "CreatedAt": p.created_at.to_rfc3339(),
                })
            })
            .collect(),
        "PackageVersions" => state
            .store()
            .list_package_versions(ctx.instance_id, ctx.org_id, None, None, None)
            .await?
            .into_iter()
            .map(|v| {
                json!({
                    "Id": v.id.to_string(),
                    "PackageId": v.package_id.to_string(),
                    "Owner": v.owner,
                    "Type": v.package_type,
                    "Name": v.package_name,
                    "Version": v.version,
                    "Digest": v.digest,
                    "Size": v.size_bytes,
                    "Signed": v.signed,
                    "CreatedAt": v.created_at.to_rfc3339(),
                })
            })
            .collect(),
        "PackageFiles" => state
            .store()
            .list_package_files(ctx.instance_id, ctx.org_id)
            .await?
            .into_iter()
            .map(|f| {
                json!({
                    "Id": f.id.to_string(),
                    "VersionId": f.version_id.to_string(),
                    "Owner": f.owner,
                    "Name": f.package_name,
                    "Version": f.version,
                    "File": f.filename,
                    "Digest": f.sha256,
                    "Size": f.size_bytes,
                    "CreatedAt": f.created_at.to_rfc3339(),
                })
            })
            .collect(),
        "Events" => state
            .store()
            .list_outbox_events(ctx.instance_id, ctx.org_id)
            .await?
            .into_iter()
            .map(|e| {
                json!({
                    "Id": e.id.to_string(),
                    "RepoId": e.repo_id.map(|r| r.to_string()),
                    "EventKind": e.event_kind,
                    "CreatedAt": e.created_at.to_rfc3339(),
                })
            })
            .collect(),
        other => {
            return Err(ShakoError::NotFound(format!(
                "unknown OData entity set '{other}'"
            )));
        }
    };

    MemoryTable::new(info, rows).map_err(map_table_error)
}

/// Parses query options while stripping `$expand` so `PullRequests?$expand=Files,Reviews` can be
/// expanded in-place after `odata-table` executes the scoped filter/sort/page (Contract S9).
fn parse_odata_options_with_expand(
    model: &odata_csdl::Model,
    set_name: &str,
    raw_query: Option<&str>,
) -> ShakoResult<(odata_url::QueryOptions, Vec<String>)> {
    let mut expand_props = Vec::new();
    let mut filtered_pairs = Vec::new();

    if let Some(q) = raw_query.filter(|s| !s.is_empty()) {
        for part in q.split('&') {
            if let Some(exp_val) = part.strip_prefix("$expand=") {
                let decoded =
                    odata_url::percent_decoded(exp_val).unwrap_or_else(|| exp_val.to_owned());
                for item in decoded.split(',') {
                    let trimmed = item.trim();
                    if !trimmed.is_empty() {
                        expand_props.push(trimmed.to_owned());
                    }
                }
            } else {
                filtered_pairs.push(part);
            }
        }
    }

    let cleaned_q = filtered_pairs.join("&");
    let uri = if cleaned_q.is_empty() {
        set_name.to_owned()
    } else {
        format!("{set_name}?{cleaned_q}")
    };
    let parsed = odata_url::parse(
        &uri,
        &odata_url::EdmModel(model),
        &odata_url::Limits::strict(),
    )
    .map_err(|e| ShakoError::BadRequest(format!("invalid OData URL query: {e}")))?;
    match parsed {
        odata_url::ODataUri::Resource { options, .. } => Ok((options, expand_props)),
        _ => Ok((odata_url::QueryOptions::default(), expand_props)),
    }
}

async fn apply_pr_expansions(
    state: &ShakoState,
    ctx: &RequestCtx,
    rows: &mut [Value],
    expand_props: &[String],
) -> ShakoResult<()> {
    if expand_props.is_empty() {
        return Ok(());
    }
    let want_reviews = expand_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("Reviews"));
    let want_files = expand_props.iter().any(|p| p.eq_ignore_ascii_case("Files"));
    if !want_reviews && !want_files {
        return Err(ShakoError::BadRequest(format!(
            "unsupported $expand property '{}'",
            expand_props[0]
        )));
    }

    for row in rows {
        let Some(repo_id_str) = row.get("RepoId").and_then(Value::as_str) else {
            continue;
        };
        let Ok(repo_id) = repo_id_str.parse::<RepoId>() else {
            continue;
        };
        let Some(num) = row.get("Number").and_then(Value::as_u64) else {
            continue;
        };
        if want_reviews {
            let revs = state
                .store()
                .list_reviews(ctx.instance_id, ctx.org_id, Some(repo_id), Some(num))
                .await?;
            let rev_json: Vec<Value> = revs.iter().map(review_row).collect();
            if let Some(obj) = row.as_object_mut() {
                obj.insert("Reviews".into(), Value::Array(rev_json));
            }
        }
        if want_files {
            let head_sha = row
                .get("HeadSha")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let base_sha = row
                .get("MergeBase")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| row.get("BaseSha").and_then(Value::as_str))
                .unwrap_or_default()
                .to_owned();
            let disk_path = repo_disk_path(state.repo_root(), ctx.instance_id, repo_id)?;
            let files = tokio::task::spawn_blocking(move || {
                diff_refs_files(&disk_path, &base_sha, &head_sha)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            if let Some(obj) = row.as_object_mut() {
                obj.insert("Files".into(), files_rows(files));
            }
        }
    }
    Ok(())
}

// =========================================================================
// Rows shared by entity sets, navigation and action answers
// =========================================================================

/// A repository as the `Repositories` entity set shows it.
fn repo_row(mount: &Mount, r: &RepoRecord) -> Value {
    json!({
        "Id": r.id.to_string(),
        "Owner": r.owner,
        "Name": r.name,
        "Project": r.project_id,
        "Description": r.description,
        "DefaultBranch": r.default_branch,
        "Private": r.private,
        "Archived": r.archived,
        "SizeBytes": r.size_bytes,
        "CloneUrl": format!("{}/{}/{}.git", mount.public_base(), r.owner, r.name),
        "CreatedAt": r.created_at.to_rfc3339(),
        "UpdatedAt": r.updated_at.to_rfc3339(),
    })
}

/// A pull request as the `PullRequests` entity set shows it.
fn pr_row(mount: &Mount, repo: &RepoRecord, pr: &PullRequestRecord) -> Value {
    json!({
        "Id": format!("{}:{}", pr.repo_id, pr.number),
        "RepoId": pr.repo_id.to_string(),
        "Number": pr.number,
        "HtmlUrl": format!("{}/{}/{}/pulls/{}", mount.public_base(), repo.owner, repo.name, pr.number),
        "State": pr.state,
        "Title": pr.title,
        "Body": pr.body,
        "Head": pr.head_ref,
        "Base": pr.base_ref,
        "HeadSha": pr.head_sha,
        "BaseSha": pr.base_sha,
        "MergeBase": pr.merge_base,
        "Mergeable": pr.mergeable,
        "Merged": pr.merged,
        "MergeCommitSha": pr.merge_commit_sha,
        "Author": pr.author_login,
        "AuthorName": pr.author_name,
        "AuthorEmail": pr.author_email,
        "CreatedAt": pr.created_at.to_rfc3339(),
        "UpdatedAt": pr.updated_at.to_rfc3339(),
        "ClosedAt": pr.closed_at.map(|d| d.to_rfc3339()),
    })
}

fn review_row(r: &ReviewRecord) -> Value {
    json!({
        "Id": r.id.to_string(),
        "RepoId": r.repo_id.to_string(),
        "Number": r.pr_number,
        "Verdict": r.verdict.as_str(),
        "Body": r.body,
        "Author": r.author_login,
        "CommitId": r.commit_id,
        "CreatedAt": r.created_at.to_rfc3339(),
    })
}

fn mirror_row(m: &PushMirrorRecord) -> Value {
    json!({
        "Id": format!("{}:{}", m.repo_id, m.remote_name),
        "RepoId": m.repo_id.to_string(),
        "Name": m.remote_name,
        "RemoteAddress": m.remote_address,
        "Branch": m.branch,
        "LastSync": m.last_sync.map(|d| d.to_rfc3339()),
        "LastError": m.last_error,
    })
}

fn files_rows(files: Vec<shako_git::DiffFileEntry>) -> Value {
    Value::Array(
        files
            .into_iter()
            .map(|f| json!({ "Filename": f.filename, "Status": f.status }))
            .collect(),
    )
}

// =========================================================================
// HTTP handlers for `/odata/{...}`
// =========================================================================

fn ok_or_no_content(val: Value) -> Response {
    if val.is_null() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::OK, Json(val)).into_response()
    }
}

/// The key of a `Repositories(...)` segment as [`find_visible_repo_by_key`] takes it: the
/// alternate key `Owner='o',Name='r'` becomes `o/r`, a UUID key stays itself.
fn repo_key(parens: &str) -> String {
    let args = parse_func_args(Some(parens), None);
    match (args.get("Owner"), args.get("Name")) {
        (Some(owner), Some(name)) => format!("{owner}/{name}"),
        _ => args
            .get("Id")
            .cloned()
            .unwrap_or_else(|| unquote_odata_key(parens)),
    }
}

/// A query string with `RepoId eq '{id}'` and-ed into its `$filter`: a collection navigated from
/// one repository shows only that repository's rows.
fn query_within_repo(repo_id: RepoId, raw_q: Option<&str>) -> String {
    let scope = format!("RepoId%20eq%20'{repo_id}'");
    let mut seen = false;
    let mut parts: Vec<String> = raw_q
        .filter(|q| !q.is_empty())
        .map(|q| q.split('&').map(str::to_owned).collect())
        .unwrap_or_default();
    for part in &mut parts {
        // A client may send the name encoded (`%24filter`): it is the same option.
        let (name, user) = part.split_once('=').unwrap_or((part.as_str(), ""));
        if percent_encoding::percent_decode_str(name).decode_utf8_lossy() == "$filter" {
            let scoped = format!("$filter={scope}%20and%20({user})");
            *part = scoped;
            seen = true;
        }
    }
    if !seen {
        parts.push(format!("$filter={scope}"));
    }
    parts.join("&")
}

/// Runs a collection read of `set_name` with OData query options (`$filter`, `$orderby`, `$top`,
/// `$skip`, `$count`, `$expand` on pull requests) inside the caller's scope (S9).
async fn collection_answer(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    set_name: &str,
    raw_q: Option<&str>,
) -> ShakoResult<Value> {
    let table = build_entity_table(state, ctx, mount, set_name, None, raw_q).await?;
    let scope = scope_for_context(ctx, state.authorizer.as_ref());
    let (opts, expand_props) =
        parse_odata_options_with_expand(&table.info().model, set_name, raw_q)?;
    let mut ans = table_query(&table, &opts, &scope, MAX_ODATA_PAGE)
        .await
        .map_err(map_table_error)?;
    if set_name == "PullRequests" {
        apply_pr_expansions(state, ctx, &mut ans.rows, &expand_props).await?;
    }
    let mut payload = json!({
        "@odata.context": format!("$metadata#{set_name}"),
        "value": ans.rows,
    });
    if let Some(c) = ans.count {
        payload["@odata.count"] = Value::from(c);
    }
    if let Some(next) = ans.next {
        payload["@odata.nextLink"] = Value::String(format!("{set_name}?$skiptoken={next}"));
    }
    Ok(payload)
}

/// One entity of `set_name` by its key, inside the caller's scope.
async fn entity_answer(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    set_name: &str,
    key: &str,
    raw_q: Option<&str>,
) -> ShakoResult<Value> {
    let key_parens = format!("'{key}'");
    let table = build_entity_table(state, ctx, mount, set_name, Some(&key_parens), raw_q).await?;
    let scope = scope_for_context(ctx, state.authorizer.as_ref());
    let (_, expand_props) = parse_odata_options_with_expand(&table.info().model, set_name, raw_q)?;
    let opts =
        key_options(table.info(), &Value::String(key.to_owned()), None).map_err(map_table_error)?;
    let mut ans = table_query(&table, &opts, &scope, 1)
        .await
        .map_err(map_table_error)?;
    if set_name == "PullRequests" {
        apply_pr_expansions(state, ctx, &mut ans.rows, &expand_props).await?;
    }
    ans.rows
        .pop()
        .ok_or_else(|| ShakoError::NotFound(format!("{set_name} entity not found")))
}

async fn odata_single_get(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path(segment): Path<String>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let (name, parens) = split_paren_segment(&segment);
    let short_name = name.strip_prefix("Shako.").unwrap_or(name);

    // Unbound function `Me()`: the caller's own account (T-6706).
    if short_name == "Me" {
        return (StatusCode::OK, Json(me_row(&ctx))).into_response();
    }

    // Unbound function `Allowed(Operation='...',Object='...')`
    if short_name == "Allowed" {
        let args = parse_func_args(parens, raw_q.as_deref());
        let op = args
            .get("Operation")
            .or_else(|| args.get("operation"))
            .map_or("Read", String::as_str);
        let obj = args
            .get("Object")
            .or_else(|| args.get("object"))
            .map(String::as_str);
        let dec = evaluate_allowed_query(&state, &ctx, op, obj).await;
        return (StatusCode::OK, Json(dec)).into_response();
    }

    let res = match parens {
        Some(p) if short_name == "Repositories" => {
            match find_visible_repo_by_key(&state, &ctx, &repo_key(p)).await {
                Ok((repo, _)) => {
                    entity_answer(
                        &state,
                        &ctx,
                        &mount,
                        "Repositories",
                        &repo.id.to_string(),
                        raw_q.as_deref(),
                    )
                    .await
                }
                Err(e) => Err(e),
            }
        }
        Some(p) => {
            entity_answer(
                &state,
                &ctx,
                &mount,
                short_name,
                &unquote_odata_key(p),
                raw_q.as_deref(),
            )
            .await
        }
        None => collection_answer(&state, &ctx, &mount, short_name, raw_q.as_deref()).await,
    };
    match res {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => odata_error_response(&e),
    }
}

/// `DELETE Repositories(...)` deletes the repository (Admin); nothing else is deletable here.
async fn odata_single_delete(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path(segment): Path<String>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let (name, parens) = split_paren_segment(&segment);
    let res = match parens {
        Some(p) if name == "Repositories" => {
            match find_visible_repo_by_key(&state, &ctx, &repo_key(p)).await {
                Ok((repo, _)) => {
                    service::delete_repository(&state, &ctx, &repo.owner, &repo.name).await
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(ShakoError::MethodNotAllowed(format!(
            "DELETE is not served on '{segment}'"
        ))),
    };
    match res {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => odata_error_response(&e),
    }
}

/// A read under one repository: `R/Branches('x')`, `R/Tags`, `R/Commits('sha')`,
/// `R/PullRequests`, `R/PullRequests(n)`, `R/PushMirrors`, or a bound function
/// (`R/Shako.Tree(...)`, `File`, `History`, `Compare`, `CommitStatus`, `ArchiveLink`,
/// `BundleLink`).
async fn repo_navigation_get(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    repo_key_str: &str,
    seg: &str,
    raw_q: Option<&str>,
) -> ShakoResult<Value> {
    let (nav, nav_parens) = split_paren_segment(seg);
    let op_short = nav.strip_prefix("Shako.").unwrap_or(nav);
    let args = parse_func_args(nav_parens, raw_q);
    match op_short {
        "Tree" => return exec_bound_repo_tree(state, ctx, repo_key_str, &args).await,
        "File" => return exec_bound_repo_file(state, ctx, repo_key_str, &args).await,
        "History" => return exec_bound_repo_history(state, ctx, repo_key_str, &args).await,
        "Compare" => return exec_bound_repo_compare(state, ctx, repo_key_str, &args).await,
        "CommitStatus" => return exec_bound_repo_status(state, ctx, repo_key_str, &args).await,
        "ArchiveLink" | "BundleLink" => {
            let (r, _) = find_visible_repo_by_key(state, ctx, repo_key_str).await?;
            let git_ref = arg(&args, "Ref").unwrap_or_else(|| r.default_branch.clone());
            let ext = if op_short == "BundleLink" {
                "bundle".to_owned()
            } else {
                arg(&args, "Format").unwrap_or_else(|| "zip".into())
            };
            return Ok(json!({
                "value": format!("{}/api/v1/repos/{}/{}/archive/{git_ref}.{ext}", mount.public_base(), r.owner, r.name)
            }));
        }
        _ => {}
    }

    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key_str).await?;
    match (op_short, nav_parens) {
        ("Branches", Some(k)) => {
            let branch = unquote_odata_key(k);
            let (sha, protected) =
                service::branch_head(state, ctx, &repo.owner, &repo.name, &branch).await?;
            Ok(json!({
                "Id": format!("{}:{branch}", repo.id),
                "RepoId": repo.id.to_string(),
                "Name": branch,
                "HeadSha": sha,
                "Protected": protected,
            }))
        }
        ("Commits", Some(k)) => {
            let sha = unquote_odata_key(k);
            let c = tokio::task::spawn_blocking(move || read_commit_info(&disk_path, &sha))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            Ok(json!({
                "Id": format!("{}:{}", repo.id, c.sha),
                "RepoId": repo.id.to_string(),
                "Sha": c.sha,
                "Message": c.message,
                "Author": c.author_name,
                "Email": c.author_email,
                "Date": c.author_date,
                "Committer": c.committer_name,
                "CommitterEmail": c.committer_email,
                "Parents": c.parent_shas,
            }))
        }
        ("PullRequests", Some(k)) => {
            let number = parse_pr_number(k)?;
            let (repo, pr) =
                service::get_pull_request(state, ctx, &repo.owner, &repo.name, number).await?;
            Ok(pr_row(mount, &repo, &pr))
        }
        ("PushMirrors", None) => {
            // Mirrors are Admin reads: the service checks the verb, the table filters the rows.
            service::list_push_mirrors(state, ctx, &repo.owner, &repo.name).await?;
            collection_answer(
                state,
                ctx,
                mount,
                "PushMirrors",
                Some(&query_within_repo(repo.id, raw_q)),
            )
            .await
        }
        (
            "Branches" | "Tags" | "Commits" | "PullRequests" | "Reviews" | "ProtectionRules"
            | "Webhooks" | "Teams",
            None,
        ) => {
            collection_answer(
                state,
                ctx,
                mount,
                op_short,
                Some(&query_within_repo(repo.id, raw_q)),
            )
            .await
        }
        _ => Err(ShakoError::NotFound(format!(
            "unknown navigation '{seg}' under a repository"
        ))),
    }
}

fn parse_pr_number(parens: &str) -> ShakoResult<u64> {
    unquote_odata_key(parens)
        .parse()
        .map_err(|_| ShakoError::BadRequest("a pull request key is its number".into()))
}

fn arg(args: &BTreeMap<String, String>, name: &str) -> Option<String> {
    args.get(name)
        .or_else(|| args.get(&lower_first(name)))
        .cloned()
}

fn lower_first(name: &str) -> String {
    let mut c = name.chars();
    c.next()
        .map(|f| f.to_lowercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

async fn odata_two_segment_get(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((seg1, seg2)): Path<(String, String)>,
    RawQuery(raw_q): RawQuery,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let (set_name, key_parens) = split_paren_segment(&seg1);
    let (sub_op, sub_parens) = split_paren_segment(&seg2);
    let op_short = sub_op.strip_prefix("Shako.").unwrap_or(sub_op);

    if let Some(k_raw) = key_parens {
        match (set_name, op_short) {
            ("Repositories", _) => {
                return match repo_navigation_get(
                    &state,
                    &ctx,
                    &mount,
                    &repo_key(k_raw),
                    &seg2,
                    raw_q.as_deref(),
                )
                .await
                {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("PackageFiles", "$value") => {
                let args = parse_func_args(Some(k_raw), raw_q.as_deref());
                let (Some(owner), Some(pkg), Some(ver), Some(file)) = (
                    arg(&args, "Owner"),
                    arg(&args, "Name"),
                    arg(&args, "Version"),
                    arg(&args, "File"),
                ) else {
                    return odata_error_response(&ShakoError::BadRequest(
                        "PackageFiles takes the key Owner, Name, Version and File".into(),
                    ));
                };
                return match service::read_generic_file(&state, &ctx, &owner, &pkg, &ver, &file)
                    .await
                {
                    Ok(bytes) => (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/octet-stream")],
                        bytes,
                    )
                        .into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("PullRequests", "Mergeable") => {
                let key_str = unquote_odata_key(k_raw);
                return match exec_bound_pr_mergeable(&state, &ctx, &key_str).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("PullRequests", "Reviews" | "Files") => {
                let key_str = unquote_odata_key(k_raw);
                return match pr_part(&state, &ctx, &key_str, op_short).await {
                    Ok(v) => (StatusCode::OK, Json(json!({ "value": v }))).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            _ => {}
        }
    }

    let table = match build_entity_table(
        &state,
        &ctx,
        &mount,
        set_name,
        key_parens,
        raw_q.as_deref(),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return odata_error_response(&e),
    };
    let scope = scope_for_context(&ctx, state.authorizer.as_ref());

    match sub_op {
        "$count" => {
            let (mut opts, _) = match parse_odata_options_with_expand(
                &table.info().model,
                set_name,
                raw_q.as_deref(),
            ) {
                Ok(o) => o,
                Err(e) => return odata_error_response(&e),
            };
            opts.count = Some(true);
            opts.top = Some(0);
            match table_query(&table, &opts, &scope, 1).await {
                Ok(ans) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    ans.count.unwrap_or(0).to_string(),
                )
                    .into_response(),
                Err(e) => odata_error_response(&map_table_error(e)),
            }
        }
        "$export" => {
            let (opts, _) = match parse_odata_options_with_expand(
                &table.info().model,
                set_name,
                raw_q.as_deref(),
            ) {
                Ok(o) => o,
                Err(e) => return odata_error_response(&e),
            };
            match table_export(&table, &opts, &scope, 100, 10_000).await {
                Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
                Err(e) => odata_error_response(&map_table_error(e)),
            }
        }
        "$allowed" => {
            let args = parse_func_args(sub_parens, raw_q.as_deref());
            let op_name = args.get("op").map_or("read", String::as_str);
            let Some(op) = TableOp::parse(op_name) else {
                return odata_error_response(&ShakoError::BadRequest(format!(
                    "unknown operation '{op_name}'"
                )));
            };
            let key_val = args.get("key").map(|k| Value::String(k.clone()));
            match table_allowed(&table, &scope, op, key_val.as_ref()).await {
                Ok(dec) => (StatusCode::OK, Json(dec.to_json())).into_response(),
                Err(e) => odata_error_response(&map_table_error(e)),
            }
        }
        _ => odata_error_response(&ShakoError::NotFound(format!(
            "unknown OData path '/odata/{seg1}/{seg2}'"
        ))),
    }
}

/// The files or the reviews of the pull request `{repo}:{number}`, through the same read check
/// as the pull request itself: a hidden repository's pull request is `NotFound`.
async fn pr_part(
    state: &ShakoState,
    ctx: &RequestCtx,
    pr_key: &str,
    part: &str,
) -> ShakoResult<Value> {
    let (repo_part, number) = resolve_pr_target(pr_key, &Value::Null)?;
    let (repo, _) = find_visible_repo_by_key(state, ctx, &repo_part).await?;
    if part == "Files" {
        let files =
            service::pull_request_files(state, ctx, &repo.owner, &repo.name, number).await?;
        Ok(files_rows(files))
    } else {
        let reviews = service::list_reviews(state, ctx, &repo.owner, &repo.name, number).await?;
        Ok(Value::Array(reviews.iter().map(review_row).collect()))
    }
}

/// `GET R/PullRequests(n)/Files` and `.../Reviews`.
async fn odata_deep_get(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((seg1, seg2, rest)): Path<(String, String, String)>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let (set_name, key_parens) = split_paren_segment(&seg1);
    let (nav, nav_parens) = split_paren_segment(&seg2);
    let res = match (set_name, key_parens, nav, nav_parens, rest.as_str()) {
        ("Repositories", Some(k), "PullRequests", Some(n), part @ ("Files" | "Reviews")) => {
            match parse_pr_number(n) {
                Ok(number) => {
                    pr_part(&state, &ctx, &format!("{}:{number}", repo_key(k)), part).await
                }
                Err(e) => Err(e),
            }
        }
        _ => Err(ShakoError::NotFound(format!(
            "unknown OData path '/odata/{seg1}/{seg2}/{rest}'"
        ))),
    };
    match res {
        Ok(v) => (StatusCode::OK, Json(json!({ "value": v }))).into_response(),
        Err(e) => odata_error_response(&e),
    }
}

async fn odata_single_post(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path(segment): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let (name, _) = split_paren_segment(&segment);
    let op_short = name.strip_prefix("Shako.").unwrap_or(name);
    let payload = body.map_or(Value::Object(Default::default()), |Json(v)| v);

    match exec_odata_operation(&state, &ctx, &mount, op_short, None, payload).await {
        Ok(val) => ok_or_no_content(val),
        Err(e) => odata_error_response(&e),
    }
}

/// The bound key an action under `seg1` runs on: `o/r` (or a UUID) for a repository,
/// `{repo}:{number}` for a pull request.
fn bound_key_of(seg1: &str) -> Option<String> {
    let (set_name, key_parens) = split_paren_segment(seg1);
    let k = key_parens?;
    Some(if set_name == "Repositories" {
        repo_key(k)
    } else {
        unquote_odata_key(k)
    })
}

async fn odata_two_segment_post(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((seg1, seg2)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let bound_key = bound_key_of(&seg1);
    let op_short = seg2.strip_prefix("Shako.").unwrap_or(&seg2);
    let payload = body.map_or(Value::Object(Default::default()), |Json(v)| v);

    match exec_odata_operation(
        &state,
        &ctx,
        &mount,
        op_short,
        bound_key.as_deref(),
        payload,
    )
    .await
    {
        Ok(val) => ok_or_no_content(val),
        Err(e) => odata_error_response(&e),
    }
}

/// `POST R/PullRequests(n)/Shako.{Review,Close,UpdateBranch,Merge}`.
async fn odata_deep_post(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((seg1, seg2, rest)): Path<(String, String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    let ctx = match authenticate_request(&state, &headers, None, "shako") {
        Ok(c) => c,
        Err(e) => return odata_error_response(&e),
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let (set_name, key_parens) = split_paren_segment(&seg1);
    let (nav, nav_parens) = split_paren_segment(&seg2);
    let (Some(k), Some(n), "Repositories", "PullRequests") =
        (key_parens, nav_parens, set_name, nav)
    else {
        return odata_error_response(&ShakoError::NotFound(format!(
            "unknown OData path '/odata/{seg1}/{seg2}/{rest}'"
        )));
    };
    let number = match parse_pr_number(n) {
        Ok(n) => n,
        Err(e) => return odata_error_response(&e),
    };
    let bound = format!("{}:{number}", repo_key(k));
    let op_short = rest.strip_prefix("Shako.").unwrap_or(&rest);
    let payload = body.map_or(Value::Object(Default::default()), |Json(v)| v);
    match exec_odata_operation(&state, &ctx, &mount, op_short, Some(&bound), payload).await {
        Ok(val) => ok_or_no_content(val),
        Err(e) => odata_error_response(&e),
    }
}

// =========================================================================
// HTTP Handlers for `/odata` and `/odata/*`
// =========================================================================

async fn odata_service_doc(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return odata_error_response(&e);
    }
    let sets: Vec<Value> = state
        .registry
        .service
        .sets
        .iter()
        .map(|s| {
            json!({
                "name": s.set,
                "kind": "EntitySet",
                "url": s.set,
            })
        })
        .collect();
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/json; odata.metadata=minimal",
            ),
            (header::HeaderName::from_static("odata-version"), "4.01"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Json(json!({
            "@odata.context": "$metadata",
            "value": sets,
        })),
    )
        .into_response()
}

async fn odata_metadata(State(state): State<ShakoState>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticate_request(&state, &headers, None, "shako") {
        return odata_error_response(&e);
    }
    match state.registry.csdl_xml() {
        Ok(xml) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
                (header::HeaderName::from_static("odata-version"), "4.01"),
            ],
            xml,
        )
            .into_response(),
        Err(e) => odata_error_response(&ShakoError::Internal(e)),
    }
}

// =========================================================================
// Shared Operation Executor (used by OData, MCP, and CLI, Contract S8, S9, S19)
// =========================================================================

/// What `Me()` answers: the login the caller signs in as, the name a branch rule's allow-lists
/// take, so a client names itself without guessing (T-6706).
fn me_row(ctx: &RequestCtx) -> Value {
    json!({ "Login": ctx.subject.username })
}

pub async fn evaluate_allowed_query(
    state: &ShakoState,
    ctx: &RequestCtx,
    operation: &str,
    object_spec: Option<&str>,
) -> shako_core::Decision {
    let Some(spec) = object_spec.filter(|s| !s.is_empty()) else {
        let inst_target = ObjectTarget::Instance {
            instance_id: ctx.instance_id,
            org_id: ctx.org_id,
        };
        return state.registry.check_allowed(
            state.authorizer.as_ref(),
            ctx,
            operation,
            &inst_target,
            true,
        );
    };

    // Spec can be `"Repository:owner/repo"`, `"Repository:<uuid>"`, or `"owner/repo"`
    let repo_key = spec.strip_prefix("Repository:").unwrap_or(spec);
    match find_visible_repo_by_key(state, ctx, repo_key).await {
        Ok((repo, _)) => {
            let target = ObjectTarget::Repository {
                instance_id: repo.instance_id,
                org_id: repo.org_id,
                project_id: (!repo.project_id.is_empty()).then(|| repo.project_id.clone()),
                owner: repo.owner,
                repo: repo.name,
                git_ref: None,
            };
            state
                .registry
                .check_allowed(state.authorizer.as_ref(), ctx, operation, &target, true)
        }
        Err(_) => {
            let dummy = ObjectTarget::Instance {
                instance_id: ctx.instance_id,
                org_id: ctx.org_id,
            };
            state
                .registry
                .check_allowed(state.authorizer.as_ref(), ctx, operation, &dummy, false)
        }
    }
}

pub async fn find_visible_repo_by_key(
    state: &ShakoState,
    ctx: &RequestCtx,
    key: &str,
) -> ShakoResult<(RepoRecord, std::path::PathBuf)> {
    if let Ok(repo_id) = key.parse::<RepoId>() {
        let Some(repo) = state
            .store()
            .get_repo_by_id(ctx.instance_id, ctx.org_id, repo_id)
            .await?
        else {
            return Err(ShakoError::NotFound("repository not found".into()));
        };
        return resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Read).await;
    }
    if let Some((owner, name)) = key.split_once('/') {
        return resolve_and_authorize_repo(state, ctx, owner, name, Action::Read).await;
    }
    let repos = state
        .store()
        .list_repos(ctx.instance_id, ctx.org_id)
        .await?;
    let Some(repo) = repos.into_iter().find(|r| r.name == key) else {
        return Err(ShakoError::NotFound("repository not found".into()));
    };
    resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Read).await
}

async fn exec_bound_repo_tree(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let git_ref = arg(args, "Ref").unwrap_or(repo.default_branch);
    let recursive = arg(args, "Recursive").is_some_and(|v| v == "true" || v == "1");
    let skip: usize = arg(args, "Skip").and_then(|v| v.parse().ok()).unwrap_or(0);
    let top: usize = arg(args, "Top")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
        .clamp(1, 1000);
    let (sha, entries) =
        tokio::task::spawn_blocking(move || list_tree_entries(&disk_path, &git_ref, recursive))
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
    let total = entries.len();
    let page: Vec<Value> = entries
        .into_iter()
        .skip(skip)
        .take(top)
        .map(|e| {
            json!({ "Path": e.path, "Type": e.entry_type, "Mode": e.mode, "Size": e.size, "Sha": e.sha })
        })
        .collect();
    let truncated = skip.saturating_add(page.len()) < total;
    Ok(json!({ "Sha": sha, "Tree": page, "Truncated": truncated, "TotalCount": total }))
}

async fn exec_bound_repo_file(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let git_ref = arg(args, "Ref").unwrap_or(repo.default_branch);
    let path = arg(args, "Path")
        .ok_or_else(|| ShakoError::BadRequest("File function requires Path".into()))?;
    let path = validate_tree_path(&path)?;
    let found = tokio::task::spawn_blocking(move || read_file_at_ref(&disk_path, &git_ref, &path))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))??;
    let Some((sha, bytes)) = found else {
        return Err(ShakoError::NotFound("file not found".into()));
    };
    Ok(json!({ "Sha": sha, "Size": bytes.len(), "Content": STANDARD.encode(&bytes) }))
}

async fn exec_bound_repo_history(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let git_ref = arg(args, "Ref").unwrap_or(repo.default_branch);
    let path = arg(args, "Path").filter(|p| !p.is_empty());
    let top: usize = arg(args, "Top")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .clamp(1, 1000);
    let commits = tokio::task::spawn_blocking(move || {
        list_commits(&disk_path, &git_ref, path.as_deref(), top, 10_000)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))??;
    let rows: Vec<Value> = commits
        .into_iter()
        .map(|c| {
            json!({
                "Sha": c.sha,
                "Message": c.message,
                "Author": c.author_name,
                "Email": c.author_email,
                "Date": c.author_date,
                "Parents": c.parent_shas,
            })
        })
        .collect();
    Ok(json!({ "value": rows }))
}

async fn exec_bound_repo_status(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, _) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let sha = arg(args, "Sha")
        .ok_or_else(|| ShakoError::BadRequest("CommitStatus requires Sha".into()))?;
    let (combined, statuses) =
        service::commit_status(state, ctx, &repo.owner, &repo.name, &sha).await?;
    let rows: Vec<Value> = statuses
        .iter()
        .map(|s| {
            json!({
                "Context": s.context,
                "State": s.state,
                "Description": s.description,
                "TargetUrl": s.target_url,
            })
        })
        .collect();
    Ok(json!({ "Sha": sha, "State": combined, "TotalCount": rows.len(), "Statuses": rows }))
}

async fn exec_bound_repo_compare(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (_repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let base =
        arg(args, "Base").ok_or_else(|| ShakoError::BadRequest("Compare requires Base".into()))?;
    let head =
        arg(args, "Head").ok_or_else(|| ShakoError::BadRequest("Compare requires Head".into()))?;
    let (m, files) = tokio::task::spawn_blocking(move || {
        let m = check_mergeability(&disk_path, &base, &head)?;
        let files = diff_refs_files(&disk_path, &base, &head)?;
        Ok::<_, ShakoError>((m, files))
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))??;
    Ok(json!({ "mergeability": m, "files": files }))
}

async fn exec_bound_pr_mergeable(
    state: &ShakoState,
    ctx: &RequestCtx,
    pr_key: &str,
) -> ShakoResult<Value> {
    let (repo_part, number) = resolve_pr_target(pr_key, &Value::Null)?;
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_part).await?;
    let pr = state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await?
        .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))?;
    let m = tokio::task::spawn_blocking(move || {
        check_mergeability(&disk_path, &pr.base_ref, &pr.head_ref)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))??;
    Ok(serde_json::to_value(m).unwrap_or(Value::Null))
}

/// A parameter of an action body by its declared PascalCase name, or by the camelCase name MCP
/// and CLI callers send.
fn field<'a>(body: &'a Value, name: &str) -> Option<&'a Value> {
    body.get(name)
        .or_else(|| body.get(lower_first(name).as_str()))
        .filter(|v| !v.is_null())
}

fn text(body: &Value, name: &str) -> Option<String> {
    field(body, name).and_then(Value::as_str).map(str::to_owned)
}

fn flag(body: &Value, name: &str) -> Option<bool> {
    field(body, name).and_then(Value::as_bool)
}

fn required(body: &Value, name: &str) -> ShakoResult<String> {
    text(body, name)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ShakoError::Unprocessable(format!("{name} required")))
}

/// `{Name, Email}` of a commit's author or committer.
fn person(body: &Value, name: &str) -> ShakoResult<Option<CommitAuthor>> {
    let Some(v) = field(body, name) else {
        return Ok(None);
    };
    match (text(v, "Name"), text(v, "Email")) {
        (Some(name), Some(email)) => Ok(Some(CommitAuthor { name, email })),
        _ => Err(ShakoError::Unprocessable(format!(
            "{name} takes Name and Email"
        ))),
    }
}

/// The owner and name of the repository an operation is bound to (or names in `repo`).
async fn bound_repo(state: &ShakoState, ctx: &RequestCtx, key: &str) -> ShakoResult<RepoRecord> {
    if key.is_empty() {
        return Err(ShakoError::Unprocessable(
            "the operation is bound to a repository".into(),
        ));
    }
    Ok(find_visible_repo_by_key(state, ctx, key).await?.0)
}

/// Executes any OData action or function by name (also called by `shako-mcp` and CLI, S8, S9,
/// S19). Every operation runs through [`service`], the one implementation the Gitea-compatible
/// REST shares, so each surface checks the same verbs, protection rules and preconditions.
pub async fn exec_odata_operation(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    op_name: &str,
    bound_key: Option<&str>,
    body: Value,
) -> ShakoResult<Value> {
    let repo_key = bound_key
        .map(str::to_owned)
        .or_else(|| text(&body, "RepoId"))
        .or_else(|| text(&body, "repo"))
        .unwrap_or_default();

    match op_name {
        "CreateRepository" | "jc_forge_create_repository" | "create-repo" => {
            let new = service::NewRepository {
                owner: text(&body, "Owner")
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| ctx.subject.username.clone()),
                name: required(&body, "Name")?,
                description: text(&body, "Description").unwrap_or_default(),
                private: flag(&body, "Private").unwrap_or(true),
                auto_init: flag(&body, "AutoInit").unwrap_or(true),
                default_branch: text(&body, "DefaultBranch").unwrap_or_else(|| "main".into()),
            };
            let saved = service::create_repository(state, ctx, new).await?;
            Ok(repo_row(mount, &saved))
        }
        "ImportRepository" | "jc_forge_import_repository" | "import-repo" => {
            let copy = service::RepositoryCopy {
                source_owner: required(&body, "SourceOwner")?,
                source_name: required(&body, "SourceName")?,
                owner: required(&body, "RepoOwner")?,
                name: required(&body, "RepoName")?,
                description: text(&body, "Description"),
                private: flag(&body, "Private").unwrap_or(true),
            };
            let saved = service::import_repository(state, ctx, copy).await?;
            Ok(repo_row(mount, &saved))
        }
        "Archive" | "jc_forge_archive_repository" | "archive-repo" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let patch = service::RepositoryPatch {
                archived: Some(true),
                ..Default::default()
            };
            let saved =
                service::patch_repository(state, ctx, &repo.owner, &repo.name, patch).await?;
            Ok(repo_row(mount, &saved))
        }
        "Delete" | "jc_forge_delete_repository" | "delete-repo" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            service::delete_repository(state, ctx, &repo.owner, &repo.name).await?;
            Ok(Value::Null)
        }
        "CreateBranch" | "jc_forge_create_branch" | "create-branch" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let new_b = required(&body, "NewBranchName")?;
            let from = text(&body, "OldBranchName").filter(|s| !s.is_empty());
            let sha =
                service::create_branch(state, ctx, &repo.owner, &repo.name, &new_b, from).await?;
            Ok(json!({
                "Id": format!("{}:{new_b}", repo.id),
                "RepoId": repo.id.to_string(),
                "Name": new_b,
                "HeadSha": sha,
            }))
        }
        "DeleteBranch" | "jc_forge_delete_branch" | "delete-branch" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let branch = required(&body, "Branch")?;
            service::delete_branch(state, ctx, &repo.owner, &repo.name, &branch).await?;
            Ok(Value::Null)
        }
        "CommitFiles" | "jc_forge_commit_files" | "commit-files" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let files_val = field(&body, "Files")
                .cloned()
                .ok_or_else(|| ShakoError::Unprocessable("Files required".into()))?;
            let items: Vec<crate::compat::MultiFileItemReq> = serde_json::from_value(files_val)
                .map_err(|e| ShakoError::Unprocessable(format!("invalid Files array: {e}")))?;
            let commit = service::CommitRequest {
                branch: text(&body, "Branch"),
                message: required(&body, "Message")?,
                author: person(&body, "Author")?,
                committer: person(&body, "Committer")?,
                files: items
                    .into_iter()
                    .map(|f| service::FileChange {
                        operation: f.operation,
                        path: f.path,
                        content: f.content,
                        sha: f.sha,
                    })
                    .collect(),
            };
            let sha = service::commit_files(state, ctx, &repo.owner, &repo.name, commit).await?;
            Ok(json!({ "Sha": sha }))
        }
        "OpenPullRequest" | "jc_forge_open_pull_request" | "open-pr" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let (repo, pr) = service::open_pull_request(
                state,
                ctx,
                &repo.owner,
                &repo.name,
                service::NewPullRequest {
                    head: required(&body, "Head")?,
                    base: text(&body, "Base")
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| repo.default_branch.clone()),
                    title: required(&body, "Title")?,
                    body: text(&body, "Body").unwrap_or_default(),
                },
            )
            .await?;
            Ok(pr_row(mount, &repo, &pr))
        }
        "Review" | "jc_forge_review_pull_request" | "review-pr" => {
            let (r_key, number) = resolve_pr_target(&repo_key, &body)?;
            let repo = bound_repo(state, ctx, &r_key).await?;
            let ev = required(&body, "Event")?;
            let verdict = ReviewVerdict::parse(&ev)
                .ok_or_else(|| ShakoError::Unprocessable(format!("invalid verdict '{ev}'")))?;
            let saved = service::create_review(
                state,
                ctx,
                &repo.owner,
                &repo.name,
                number,
                service::NewReview {
                    verdict,
                    body: text(&body, "Body").unwrap_or_default(),
                    commit_id: text(&body, "CommitId"),
                },
            )
            .await?;
            Ok(review_row(&saved))
        }
        "Merge" | "jc_forge_merge_pull_request" | "merge-pr" => {
            let (r_key, number) = resolve_pr_target(&repo_key, &body)?;
            let repo = bound_repo(state, ctx, &r_key).await?;
            let merge = service::MergeRequest {
                style: MergeStyle::parse(&text(&body, "Style").unwrap_or_else(|| "merge".into()))?,
                message: text(&body, "Message"),
                head_commit_id: text(&body, "HeadCommitId"),
                delete_branch_after_merge: flag(&body, "DeleteBranchAfterMerge").unwrap_or(false),
                author: person(&body, "Author")?,
            };
            let sha =
                service::merge_pull_request(state, ctx, &repo.owner, &repo.name, number, merge)
                    .await?;
            Ok(json!({ "Sha": sha, "Merged": true }))
        }
        "UpdateBranch" | "jc_forge_update_pull_request_branch" | "update-pr-branch" => {
            let (r_key, number) = resolve_pr_target(&repo_key, &body)?;
            let repo = bound_repo(state, ctx, &r_key).await?;
            if let Some(style) = text(&body, "Style").filter(|s| s != "merge") {
                return Err(ShakoError::Unprocessable(format!(
                    "update style '{style}' is not supported"
                )));
            }
            let head =
                service::update_pull_branch(state, ctx, &repo.owner, &repo.name, number).await?;
            Ok(json!({ "HeadSha": head }))
        }
        "Close" | "jc_forge_close_pull_request" | "close-pr" => {
            let (r_key, number) = resolve_pr_target(&repo_key, &body)?;
            let repo = bound_repo(state, ctx, &r_key).await?;
            let patch = service::PullRequestPatch {
                state: Some("closed".into()),
                ..Default::default()
            };
            let (repo, pr) =
                service::patch_pull_request(state, ctx, &repo.owner, &repo.name, number, patch)
                    .await?;
            Ok(pr_row(mount, &repo, &pr))
        }
        "Protect" | "jc_forge_protect_branch" | "protect" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let rule_val = field(&body, "Rule").cloned().unwrap_or(body);
            let rule: BranchProtectionRule = serde_json::from_value(rule_val)
                .map_err(|e| ShakoError::Unprocessable(format!("invalid Rule: {e}")))?;
            let saved = service::protect_branch(state, ctx, &repo.owner, &repo.name, rule).await?;
            Ok(json!({
                "Id": format!("{}:{}", repo.id, saved.rule.rule_name),
                "RepoId": repo.id.to_string(),
                "Pattern": saved.rule.rule_name,
                "EnablePush": saved.rule.enable_push,
                "RequiredApprovals": saved.rule.required_approvals,
            }))
        }
        "Unprotect" | "jc_forge_unprotect_branch" | "unprotect" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let pattern = required(&body, "Pattern")?;
            service::unprotect_branch(state, ctx, &repo.owner, &repo.name, &pattern).await?;
            Ok(Value::Null)
        }
        "AddPushMirror" | "jc_forge_add_push_mirror" | "add-mirror" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let new = service::NewPushMirror {
                remote_address: required(&body, "RemoteAddress")?,
                username: text(&body, "RemoteUsername"),
                password: text(&body, "Credential"),
                secret_ref: text(&body, "SecretRef").filter(|s| !s.is_empty()),
                interval: text(&body, "Interval"),
                sync_on_commit: flag(&body, "SyncOnCommit").unwrap_or(true),
                branch: text(&body, "Branch").filter(|s| !s.is_empty()),
            };
            let saved = service::add_push_mirror(state, ctx, &repo.owner, &repo.name, new).await?;
            Ok(mirror_row(&saved))
        }
        "RemovePushMirror" | "jc_forge_remove_push_mirror" | "remove-mirror" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let name = required(&body, "Name")?;
            service::remove_push_mirror(state, ctx, &repo.owner, &repo.name, &name).await?;
            Ok(Value::Null)
        }
        "SyncPushMirrors" | "jc_forge_sync_push_mirrors" | "sync-mirrors" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            service::sync_push_mirrors(state, ctx, &repo.owner, &repo.name).await?;
            Ok(Value::Null)
        }
        "AddWebhook" | "jc_forge_add_webhook" | "add-webhook" => {
            let repo = bound_repo(state, ctx, &repo_key).await?;
            let secret = match (text(&body, "Secret"), text(&body, "SecretRef")) {
                (Some(s), None) => service::HookSecret::Inline(s),
                (None, Some(r)) => service::HookSecret::Ref(r),
                _ => {
                    return Err(ShakoError::Unprocessable(
                        "a hook takes Secret or SecretRef".into(),
                    ));
                }
            };
            let events = match field(&body, "Events") {
                Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                    ShakoError::Unprocessable(format!("Events is a list of names: {e}"))
                })?,
                None => Vec::new(),
            };
            let new = service::NewWebhook {
                url: required(&body, "Url")?,
                secret,
                previous_secret_ref: text(&body, "PreviousSecretRef"),
                events,
                active: flag(&body, "Active").unwrap_or(true),
                format: WebhookFormat::Shako,
            };
            let saved = service::add_webhook(state, ctx, &repo.owner, &repo.name, new).await?;
            Ok(json!({
                "Id": saved.id.to_string(),
                "RepoId": saved.repo_id.to_string(),
                "Url": saved.url,
                "Active": saved.active,
            }))
        }
        "RedeliverWebhook" | "jc_forge_redeliver_webhook" | "redeliver-webhook" => {
            // Bound to `Webhooks('{id}')`; MCP and CLI name the hook and its repository.
            let hook_id: WebhookId = bound_key
                .map(str::to_owned)
                .or_else(|| text(&body, "Id"))
                .unwrap_or_default()
                .parse()
                .map_err(|_| ShakoError::Unprocessable("a webhook id is a UUID".into()))?;
            let hook = state
                .store()
                .get_webhook(ctx.instance_id, ctx.org_id, hook_id)
                .await?
                .ok_or_else(|| ShakoError::NotFound("webhook not found".into()))?;
            let repo = state
                .store()
                .get_repo_by_id(ctx.instance_id, ctx.org_id, hook.repo_id)
                .await?
                .ok_or_else(|| ShakoError::NotFound("webhook not found".into()))?;
            let del =
                service::redeliver_webhook(state, ctx, &repo.owner, &repo.name, hook_id).await?;
            Ok(json!({
                "Id": del.id.to_string(),
                "WebhookId": del.webhook_id.to_string(),
                "Event": del.event,
                "Status": del.status,
                "ResponseCode": del.response_code,
            }))
        }
        "PublishPackage" | "jc_forge_publish_package" | "publish-package" => {
            let owner = text(&body, "Owner")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ctx.subject.username.clone());
            let content = required(&body, "ContentBase64")?;
            let stripped: String = content.chars().filter(|c| !c.is_whitespace()).collect();
            let bytes = STANDARD
                .decode(stripped.as_bytes())
                .map_err(|e| ShakoError::Unprocessable(format!("invalid ContentBase64: {e}")))?;
            let saved = service::publish_generic_file(
                state,
                ctx,
                &owner,
                &required(&body, "Package")?,
                &required(&body, "Version")?,
                &required(&body, "File")?,
                bytes,
            )
            .await?;
            Ok(json!({
                "Id": saved.id.to_string(),
                "VersionId": saved.version_id.to_string(),
                "Owner": saved.owner,
                "Name": saved.package_name,
                "Version": saved.version,
                "File": saved.filename,
                "Digest": saved.sha256,
                "Size": saved.size_bytes,
            }))
        }
        "Tree"
        | "jc_forge_tree"
        | "tree"
        | "File"
        | "jc_forge_file"
        | "file"
        | "History"
        | "jc_forge_history"
        | "history"
        | "Compare"
        | "jc_forge_compare"
        | "compare"
        | "CommitStatus"
        | "jc_forge_commit_status"
        | "commit-status" => {
            let mut args = BTreeMap::new();
            if let Some(obj) = body.as_object() {
                for (k, v) in obj {
                    let val = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    args.insert(k.clone(), val);
                }
            }
            for name in [
                "Ref",
                "Path",
                "Recursive",
                "Skip",
                "Top",
                "Base",
                "Head",
                "Sha",
            ] {
                if let Some(v) = args.get(&lower_first(name)).cloned() {
                    args.entry(name.to_owned()).or_insert(v);
                }
            }
            match op_name {
                "Tree" | "jc_forge_tree" | "tree" => {
                    exec_bound_repo_tree(state, ctx, &repo_key, &args).await
                }
                "File" | "jc_forge_file" | "file" => {
                    exec_bound_repo_file(state, ctx, &repo_key, &args).await
                }
                "History" | "jc_forge_history" | "history" => {
                    exec_bound_repo_history(state, ctx, &repo_key, &args).await
                }
                "CommitStatus" | "jc_forge_commit_status" | "commit-status" => {
                    exec_bound_repo_status(state, ctx, &repo_key, &args).await
                }
                _ => exec_bound_repo_compare(state, ctx, &repo_key, &args).await,
            }
        }
        "Me" | "jc_forge_me" | "me" => Ok(me_row(ctx)),
        "Allowed" | "jc_forge_allowed" | "allowed" => {
            let op = text(&body, "Operation").unwrap_or_else(|| "Read".into());
            let obj = text(&body, "Object");
            let dec = evaluate_allowed_query(state, ctx, &op, obj.as_deref()).await;
            Ok(serde_json::to_value(dec).unwrap_or(Value::Null))
        }
        _ => {
            // An entity-set read under its MCP or CLI name (`jc_forge_repositories_list`, `quota`).
            let Some(op_def) = state.registry.find(op_name) else {
                return Err(ShakoError::NotFound(format!(
                    "unknown operation '{op_name}'"
                )));
            };
            let Some(set_name) = op_def
                .entity_set
                .filter(|_| op_def.odata.kind == shako_core::ODataKind::EntitySetRead)
            else {
                return Err(ShakoError::NotImplemented(format!(
                    "operation '{}' is not served through this surface",
                    op_def.name
                )));
            };
            let filter_q = text(&body, "Filter").map(|f| format!("$filter={f}"));
            let table =
                build_entity_table(state, ctx, mount, set_name, None, filter_q.as_deref()).await?;
            let scope = scope_for_context(ctx, state.authorizer.as_ref());
            let (opts, _) = parse_odata_options_with_expand(
                &table.info().model,
                set_name,
                filter_q.as_deref(),
            )?;
            let ans = table_query(&table, &opts, &scope, MAX_ODATA_PAGE)
                .await
                .map_err(map_table_error)?;
            Ok(json!({ "value": ans.rows }))
        }
    }
}

fn resolve_pr_target(bound_or_repo: &str, body: &Value) -> ShakoResult<(String, u64)> {
    if let Some((r, n_str)) = bound_or_repo.rsplit_once(':')
        && let Ok(n) = n_str.parse::<u64>()
    {
        return Ok((r.to_owned(), n));
    }
    let num = field(body, "Number")
        .and_then(Value::as_u64)
        .ok_or_else(|| ShakoError::Unprocessable("PullRequest Number required".into()))?;
    Ok((bound_or_repo.to_owned(), num))
}

/// The largest OData request body: a `PublishPackage` file travels base64 inside it.
/// ponytail: a package file above ~48 MiB needs the generic package PUT, which streams.
const MAX_ODATA_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Builds the OData 4.01 service router at `/odata` and `/odata/*` (Contract S9).
pub fn odata_router() -> Router<ShakoState> {
    Router::new()
        .route("/odata", get(odata_service_doc))
        .route("/odata/", get(odata_service_doc))
        .route("/odata/$metadata", get(odata_metadata))
        .route(
            "/odata/{seg1}",
            get(odata_single_get)
                .post(odata_single_post)
                .delete(odata_single_delete),
        )
        .route(
            "/odata/{seg1}/{seg2}",
            get(odata_two_segment_get).post(odata_two_segment_post),
        )
        .route(
            "/odata/{seg1}/{seg2}/{rest}",
            get(odata_deep_get).post(odata_deep_post),
        )
        .layer(DefaultBodyLimit::max(MAX_ODATA_BODY_BYTES))
}

#[cfg(test)]
mod query_within_repo_tests {
    use super::*;

    /// The scope is and-ed into the caller's filter however the option name is spelled, so a
    /// navigated collection carries one `$filter` and never a second the parser refuses.
    #[test]
    fn an_encoded_filter_name_gets_the_scope_and_ed_in() {
        let id = RepoId::new();
        for q in [
            "$filter=State%20eq%20'open'&$top=5",
            "%24filter=State%20eq%20'open'&%24top=5",
        ] {
            let scoped = query_within_repo(id, Some(q));
            assert_eq!(scoped.matches("filter=").count(), 1, "{scoped}");
            assert!(
                scoped.starts_with(&format!("$filter=RepoId%20eq%20'{id}'%20and%20(State")),
                "{scoped}"
            );
        }
        assert_eq!(
            query_within_repo(id, Some("$top=5")),
            format!("$top=5&$filter=RepoId%20eq%20'{id}'")
        );
    }
}
