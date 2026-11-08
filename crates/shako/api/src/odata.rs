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

use crate::outbox::{build_pr_webhook_payload, emit_and_dispatch_event};
use crate::state::{
    ShakoState, authenticate_request, odata_error_response, resolve_and_authorize_repo,
};
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use jc_router::Mount;
use odata_table::{
    MemoryTable, Operation as TableOp, Table, TableError, allowed as table_allowed,
    export as table_export, key_options, query as table_query,
};
use serde::Deserialize;
use serde_json::{Value, json};
use shako_core::{
    Action, BranchProtectionRule, MergePolicyInput, ObjectTarget, ProtectionRuleRecord, RefPolicy,
    RepoId, RepoRecord, RequestCtx, ReviewRecord, ReviewVerdict, ShakoError, ShakoResult,
    check_repo_count_quota, repo_disk_path, scope_for_context, validate_branch_name,
    validate_repo_or_owner_name, validate_tree_path,
};
use shako_git::{
    CommitAuthor, FileChangeOp, MergeStyle, check_mergeability, commit_file_changes,
    compute_repo_size_bytes, create_branch, delete_branch, diff_refs_files, execute_pr_merge,
    init_bare_repo, list_branches, list_commits, list_tags, list_tree_entries, read_file_at_ref,
    update_pr_branch,
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

fn unquote_odata_key(raw: &str) -> String {
    let t = raw.trim();
    if let Some((_, v)) = t.split_once('=') {
        return v.trim().trim_matches(['\'', '"']).to_owned();
    }
    t.trim_matches(['\'', '"']).to_owned()
}

fn parse_func_args(parens: Option<&str>, raw_query: Option<&str>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(inner) = parens {
        for part in inner.split(',') {
            if let Some((k, v)) = part.split_once('=') {
                map.insert(
                    k.trim().to_owned(),
                    v.trim().trim_matches(['\'', '"']).to_owned(),
                );
            }
        }
    }
    if let Some(q) = raw_query
        && let Ok(pairs) = serde_urlencoded::from_str::<Vec<(String, String)>>(q)
    {
        for (k, v) in pairs {
            map.insert(k, v);
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
        "Repositories" => {
            let base = mount.public_base();
            visible_repos
                .iter()
                .map(|r| {
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
                        "CloneUrl": format!("{base}/{}/{}.git", r.owner, r.name),
                        "CreatedAt": r.created_at.to_rfc3339(),
                        "UpdatedAt": r.updated_at.to_rfc3339(),
                    })
                })
                .collect()
        }
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
            let branches =
                tokio::task::spawn_blocking(move || list_branches(&disk_path))
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
            commits
                .into_iter()
                .map(|c| {
                    let summary = c.message.lines().next().unwrap_or_default().to_owned();
                    json!({
                        "Id": format!("{repo_id}:{}", c.sha),
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
            let visible_ids: std::collections::BTreeSet<RepoId> =
                visible_repos.iter().map(|r| r.id).collect();
            state
                .store()
                .list_pull_requests(ctx.instance_id, ctx.org_id, None, None)
                .await?
                .into_iter()
                .filter(|pr| visible_ids.contains(&pr.repo_id))
                .map(|pr| {
                    json!({
                        "Id": format!("{}:{}", pr.repo_id, pr.number),
                        "RepoId": pr.repo_id.to_string(),
                        "Number": pr.number,
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
                        "CreatedAt": pr.created_at.to_rfc3339(),
                        "ClosedAt": pr.closed_at.map(|d| d.to_rfc3339()),
                    })
                })
                .collect()
        }
        "Reviews" => {
            let visible_ids: std::collections::BTreeSet<RepoId> =
                visible_repos.iter().map(|r| r.id).collect();
            state
                .store()
                .list_reviews(ctx.instance_id, ctx.org_id, None, None)
                .await?
                .into_iter()
                .filter(|r| visible_ids.contains(&r.repo_id))
                .map(|r| {
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
                })
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
                .into_iter()
                .filter(|m| visible_ids.contains(&m.repo_id))
                .map(|m| {
                    json!({
                        "Id": format!("{}:{}", m.repo_id, m.remote_name),
                        "RepoId": m.repo_id.to_string(),
                        "Name": m.remote_name,
                        "RemoteAddress": m.remote_address,
                        "LastSync": m.last_sync.map(|d| d.to_rfc3339()),
                        "LastError": m.last_error,
                    })
                })
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
    let want_files = expand_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("Files"));
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
            let rev_json: Vec<Value> = revs
                .into_iter()
                .map(|r| {
                    json!({
                        "Id": r.id.to_string(),
                        "RepoId": r.repo_id.to_string(),
                        "Number": r.pr_number,
                        "Verdict": r.verdict.as_str(),
                        "Body": r.body,
                        "Author": r.author_login,
                        "CommitId": r.commit_id,
                    })
                })
                .collect();
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
                obj.insert(
                    "Files".into(),
                    serde_json::to_value(files).unwrap_or(Value::Array(Vec::new())),
                );
            }
        }
    }
    Ok(())
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
            (header::CONTENT_TYPE, "application/json; odata.metadata=minimal"),
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

    let table = match build_entity_table(&state, &ctx, &mount, short_name, parens, raw_q.as_deref()).await {
        Ok(t) => t,
        Err(e) => return odata_error_response(&e),
    };
    let scope = scope_for_context(&ctx, state.authorizer.as_ref());

    if let Some(key_raw) = parens {
        let key_val = Value::String(unquote_odata_key(key_raw));
        let (_, expand_props) = match parse_odata_options_with_expand(
            &table.info().model,
            short_name,
            raw_q.as_deref(),
        ) {
            Ok(o) => o,
            Err(e) => return odata_error_response(&e),
        };
        let opts = match key_options(table.info(), &key_val, None) {
            Ok(o) => o,
            Err(e) => return odata_error_response(&map_table_error(e)),
        };
        match table_query(&table, &opts, &scope, 1).await {
            Ok(mut ans) => {
                if short_name == "PullRequests"
                    && let Err(e) =
                        apply_pr_expansions(&state, &ctx, &mut ans.rows, &expand_props).await
                {
                    return odata_error_response(&e);
                }
                if let Some(row) = ans.rows.pop() {
                    (StatusCode::OK, Json(row)).into_response()
                } else {
                    odata_error_response(&ShakoError::NotFound(format!(
                        "{short_name} entity not found"
                    )))
                }
            }
            Err(e) => odata_error_response(&map_table_error(e)),
        }
    } else {
        let (opts, expand_props) = match parse_odata_options_with_expand(
            &table.info().model,
            short_name,
            raw_q.as_deref(),
        ) {
            Ok(o) => o,
            Err(e) => return odata_error_response(&e),
        };
        match table_query(&table, &opts, &scope, MAX_ODATA_PAGE).await {
            Ok(mut ans) => {
                if short_name == "PullRequests"
                    && let Err(e) =
                        apply_pr_expansions(&state, &ctx, &mut ans.rows, &expand_props).await
                {
                    return odata_error_response(&e);
                }
                let mut payload = json!({
                    "@odata.context": format!("$metadata#{short_name}"),
                    "value": ans.rows,
                });
                if let Some(c) = ans.count {
                    payload["@odata.count"] = Value::from(c);
                }
                if let Some(next) = ans.next {
                    payload["@odata.nextLink"] =
                        Value::String(format!("{short_name}?$skiptoken={next}"));
                }
                (StatusCode::OK, Json(payload)).into_response()
            }
            Err(e) => odata_error_response(&map_table_error(e)),
        }
    }
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

    // Bound functions on `Repositories('{id}')/...` or `PullRequests('{repo_id}:{num}')/...`
    if let Some(k_raw) = key_parens {
        let key_str = unquote_odata_key(k_raw);
        let args = parse_func_args(sub_parens, raw_q.as_deref());
        match (set_name, op_short) {
            ("Repositories", "Tree") => {
                return match exec_bound_repo_tree(&state, &ctx, &key_str, &args).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("Repositories", "File") => {
                return match exec_bound_repo_file(&state, &ctx, &key_str, &args).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("Repositories", "History") => {
                return match exec_bound_repo_history(&state, &ctx, &key_str, &args).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("Repositories", "Compare") => {
                return match exec_bound_repo_compare(&state, &ctx, &key_str, &args).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("Repositories", "ArchiveLink") => {
                let git_ref = args.get("Ref").or_else(|| args.get("ref")).map_or("main", String::as_str);
                let fmt = args.get("Format").or_else(|| args.get("format")).map_or("zip", String::as_str);
                return match find_visible_repo_by_key(&state, &ctx, &key_str).await {
                    Ok((r, _)) => (
                        StatusCode::OK,
                        Json(json!({
                            "value": format!("{}/api/v1/repos/{}/{}/archive/{git_ref}.{fmt}", mount.public_base(), r.owner, r.name)
                        })),
                    )
                        .into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("Repositories", "BundleLink") => {
                let git_ref = args.get("Ref").or_else(|| args.get("ref")).map_or("main", String::as_str);
                return match find_visible_repo_by_key(&state, &ctx, &key_str).await {
                    Ok((r, _)) => (
                        StatusCode::OK,
                        Json(json!({
                            "value": format!("{}/api/v1/repos/{}/{}/archive/{git_ref}.bundle", mount.public_base(), r.owner, r.name)
                        })),
                    )
                        .into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("PullRequests", "Mergeable") => {
                return match exec_bound_pr_mergeable(&state, &ctx, &key_str).await {
                    Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                    Err(e) => odata_error_response(&e),
                };
            }
            ("PullRequests", "Reviews") | ("PullRequests", "Files") => {
                let mut rows = vec![];
                if let Some((rid_s, num_s)) = key_str.split_once(':') {
                    rows.push(json!({
                        "RepoId": rid_s,
                        "Number": num_s.parse::<u64>().unwrap_or(0),
                    }));
                }
                if let Err(e) =
                    apply_pr_expansions(&state, &ctx, &mut rows, &[op_short.to_owned()]).await
                {
                    return odata_error_response(&e);
                }
                let val = rows
                    .first()
                    .and_then(|r| r.get(op_short))
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new()));
                return (StatusCode::OK, Json(json!({ "value": val }))).into_response();
            }
            _ => {}
        }
    }

    let table = match build_entity_table(&state, &ctx, &mount, set_name, key_parens, raw_q.as_deref()).await {
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
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(e) => odata_error_response(&e),
    }
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
    let (_set_name, key_parens) = split_paren_segment(&seg1);
    let bound_key = key_parens.map(unquote_odata_key);
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
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(e) => odata_error_response(&e),
    }
}

// =========================================================================
// Shared Operation Executor (used by OData, MCP, and CLI, Contract S8, S9, S19)
// =========================================================================

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
    let repos = state.store().list_repos(ctx.instance_id, ctx.org_id).await?;
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
    let git_ref = args
        .get("Ref")
        .or_else(|| args.get("ref"))
        .cloned()
        .unwrap_or(repo.default_branch);
    let recursive = args
        .get("Recursive")
        .or_else(|| args.get("recursive"))
        .is_some_and(|v| v == "true" || v == "1");
    let (sha, entries) =
        tokio::task::spawn_blocking(move || list_tree_entries(&disk_path, &git_ref, recursive))
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
    Ok(json!({ "sha": sha, "tree": entries }))
}

async fn exec_bound_repo_file(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let git_ref = args
        .get("Ref")
        .or_else(|| args.get("ref"))
        .cloned()
        .unwrap_or(repo.default_branch);
    let path = args
        .get("Path")
        .or_else(|| args.get("path"))
        .cloned()
        .ok_or_else(|| ShakoError::BadRequest("File function requires Path".into()))?;
    let found = tokio::task::spawn_blocking(move || read_file_at_ref(&disk_path, &git_ref, &path))
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))??;
    let Some((sha, bytes)) = found else {
        return Err(ShakoError::NotFound("file not found".into()));
    };
    Ok(json!({
        "sha": sha,
        "content": String::from_utf8_lossy(&bytes),
        "contentBase64": STANDARD.encode(&bytes),
    }))
}

async fn exec_bound_repo_history(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let git_ref = args
        .get("Ref")
        .or_else(|| args.get("ref"))
        .cloned()
        .unwrap_or(repo.default_branch);
    let path = args.get("Path").or_else(|| args.get("path")).cloned();
    let commits = tokio::task::spawn_blocking(move || {
        list_commits(&disk_path, &git_ref, path.as_deref(), 50, 10_000)
    })
    .await
    .map_err(|e| ShakoError::Internal(e.to_string()))??;
    Ok(json!({ "value": commits }))
}

async fn exec_bound_repo_compare(
    state: &ShakoState,
    ctx: &RequestCtx,
    repo_key: &str,
    args: &BTreeMap<String, String>,
) -> ShakoResult<Value> {
    let (_repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_key).await?;
    let base = args
        .get("Base")
        .or_else(|| args.get("base"))
        .cloned()
        .ok_or_else(|| ShakoError::BadRequest("Compare requires Base".into()))?;
    let head = args
        .get("Head")
        .or_else(|| args.get("head"))
        .cloned()
        .ok_or_else(|| ShakoError::BadRequest("Compare requires Head".into()))?;
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
    let (repo_part, num_part) = pr_key
        .rsplit_once(':')
        .ok_or_else(|| ShakoError::BadRequest("PullRequest key must be {RepoId}:{Number}".into()))?;
    let number: u64 = num_part
        .parse()
        .map_err(|_| ShakoError::BadRequest("invalid PR number".into()))?;
    let (repo, disk_path) = find_visible_repo_by_key(state, ctx, repo_part).await?;
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

/// Executes any OData action or function by name (also called by `shako-mcp` and CLI, S8, S9, S19).
pub async fn exec_odata_operation(
    state: &ShakoState,
    ctx: &RequestCtx,
    mount: &Mount,
    op_name: &str,
    bound_key: Option<&str>,
    body: Value,
) -> ShakoResult<Value> {
    let get_str = |k1: &str, k2: &str| -> Option<String> {
        body.get(k1)
            .or_else(|| body.get(k2))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let repo_key = bound_key
        .map(str::to_owned)
        .or_else(|| get_str("RepoId", "repo"))
        .unwrap_or_default();

    match op_name {
        "CreateRepository" | "jc_forge_create_repository" | "create-repo" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Req {
                #[serde(alias = "Owner", default)]
                owner: Option<String>,
                #[serde(alias = "Name")]
                name: String,
                #[serde(alias = "Description", default)]
                description: String,
                #[serde(alias = "AutoInit", default = "default_true_fn")]
                auto_init: bool,
                #[serde(alias = "DefaultBranch", default = "default_main_fn")]
                default_branch: String,
            }
            fn default_true_fn() -> bool {
                true
            }
            fn default_main_fn() -> String {
                "main".into()
            }
            let req: Req = serde_json::from_value(body)
                .map_err(|e| ShakoError::Unprocessable(format!("invalid CreateRepository body: {e}")))?;
            let owner = req
                .owner
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ctx.subject.username.clone());
            validate_repo_or_owner_name(&owner, "owner")?;
            validate_repo_or_owner_name(&req.name, "repository")?;
            validate_branch_name(&req.default_branch)?;

            state
                .authorizer
                .check(
                    ctx,
                    Action::Write,
                    &ObjectTarget::Organization {
                        instance_id: ctx.instance_id,
                        org_id: ctx.org_id,
                        owner: owner.clone(),
                    },
                )
                .into_result("organization not found")?;

            let inst = state.instance_for(ctx.instance_id, ctx.org_id).await;
            let usage = state
                .store()
                .get_quota_usage(ctx.instance_id, ctx.org_id)
                .await?;
            check_repo_count_quota(&inst, &usage)?;

            let repo_id = RepoId::new();
            let root = state.repo_root().to_path_buf();
            let inst_id = ctx.instance_id;
            let def_b = req.default_branch.clone();
            let auto_i = req.auto_init;
            let desc = req.description.clone();
            let disk_path = tokio::task::spawn_blocking(move || {
                init_bare_repo(&root, inst_id, repo_id, &def_b, auto_i, &desc)
            })
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
            let sz = compute_repo_size_bytes(&disk_path);
            let now = Utc::now();
            let rec = RepoRecord {
                id: repo_id,
                instance_id: ctx.instance_id,
                org_id: ctx.org_id,
                project_id: req.name.clone(),
                owner: owner.clone(),
                name: req.name,
                description: req.description,
                default_branch: req.default_branch,
                private: true,
                archived: false,
                size_bytes: sz,
                next_pr_number: 1,
                created_at: now,
                updated_at: now,
            };
            let saved = state.store().create_repo(rec).await?;
            state.register_owner_org(&owner, ctx.org_id);
            let _ = state
                .store()
                .adjust_quota_usage(ctx.instance_id, ctx.org_id, 1, sz as i64, 0)
                .await;
            Ok(json!({
                "Id": saved.id.to_string(),
                "Owner": saved.owner,
                "Name": saved.name,
                "DefaultBranch": saved.default_branch,
                "CloneUrl": format!("{}/{}/{}.git", mount.public_base(), saved.owner, saved.name),
            }))
        }
        "Archive" | "jc_forge_archive_repository" | "archive-repo" => {
            let (repo, _) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            let (mut repo, _) =
                resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Admin)
                    .await?;
            repo.archived = true;
            let saved = state.store().update_repo(repo).await?;
            Ok(json!({ "Id": saved.id.to_string(), "Archived": saved.archived }))
        }
        "Delete" | "jc_forge_delete_repository" | "delete-repo" => {
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Admin).await?;
            let _guard = state.repo_locks.lock(repo.id).await;
            state
                .store()
                .delete_repo(repo.instance_id, repo.org_id, repo.id)
                .await?;
            let _ = std::fs::remove_dir_all(&disk_path);
            Ok(json!({ "deleted": true }))
        }
        "CreateBranch" | "jc_forge_create_branch" | "create-branch" => {
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Propose)
                .await?;
            let new_b = get_str("NewBranchName", "newBranchName")
                .ok_or_else(|| ShakoError::Unprocessable("NewBranchName required".into()))?;
            let old_b =
                get_str("OldBranchName", "oldBranchName").unwrap_or(repo.default_branch.clone());
            let _guard = state.repo_locks.lock(repo.id).await;
            let nb = new_b.clone();
            let sha = tokio::task::spawn_blocking(move || create_branch(&disk_path, &nb, &old_b))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            Ok(json!({ "Id": format!("{}:{new_b}", repo.id), "Name": new_b, "HeadSha": sha }))
        }
        "DeleteBranch" | "jc_forge_delete_branch" | "delete-branch" => {
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            let branch = get_str("Branch", "branch")
                .ok_or_else(|| ShakoError::Unprocessable("Branch required".into()))?;
            let policy = state
                .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
                .await?;
            policy.check_ref_update(
                ctx,
                state.authorizer.as_ref(),
                &repo.owner,
                &repo.name,
                &format!("refs/heads/{branch}"),
                true,
                false,
                true,
                &[],
            )?;
            let _guard = state.repo_locks.lock(repo.id).await;
            let deleted = tokio::task::spawn_blocking(move || delete_branch(&disk_path, &branch))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            Ok(json!({ "deleted": deleted }))
        }
        "CommitFiles" | "jc_forge_commit_files" | "commit-files" => {
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            let branch = get_str("Branch", "branch").unwrap_or(repo.default_branch.clone());
            let message = get_str("Message", "message")
                .ok_or_else(|| ShakoError::Unprocessable("Message required".into()))?;
            let files_val = body
                .get("Files")
                .or_else(|| body.get("files"))
                .cloned()
                .ok_or_else(|| ShakoError::Unprocessable("Files required".into()))?;
            let items: Vec<crate::compat::MultiFileItemReq> = serde_json::from_value(files_val)
                .map_err(|e| ShakoError::Unprocessable(format!("invalid Files array: {e}")))?;

            let mut ops = Vec::with_capacity(items.len());
            let mut paths = Vec::with_capacity(items.len());
            for item in items {
                let clean = validate_tree_path(&item.path)?;
                let content = match item.content {
                    Some(s) => {
                        if let Ok(dec) = STANDARD.decode(s.as_bytes()) {
                            Some(dec)
                        } else {
                            Some(s.into_bytes())
                        }
                    }
                    None => None,
                };
                paths.push(clean.clone());
                ops.push(FileChangeOp {
                    operation: item.operation,
                    path: clean,
                    content,
                    sha: item.sha,
                });
            }
            let policy = state
                .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
                .await?;
            policy.check_ref_update(
                ctx,
                state.authorizer.as_ref(),
                &repo.owner,
                &repo.name,
                &format!("refs/heads/{branch}"),
                false,
                false,
                true,
                &paths,
            )?;

            let _guard = state.repo_locks.lock(repo.id).await;
            let author = CommitAuthor {
                name: ctx.subject.full_name.clone(),
                email: ctx
                    .subject
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
            };
            let sha = tokio::task::spawn_blocking(move || {
                commit_file_changes(&disk_path, &branch, &message, &author, &author, &ops)
            })
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
            Ok(json!({ "sha": sha }))
        }
        "OpenPullRequest" | "jc_forge_open_pull_request" | "open-pr" => {
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Propose)
                .await?;
            let head = get_str("Head", "head")
                .ok_or_else(|| ShakoError::Unprocessable("Head required".into()))?;
            let base = get_str("Base", "base").unwrap_or(repo.default_branch.clone());
            let title = get_str("Title", "title")
                .ok_or_else(|| ShakoError::Unprocessable("Title required".into()))?;
            let pr_body = get_str("Body", "body").unwrap_or_default();

            let h_c = head.clone();
            let b_c = base.clone();
            let d_c = disk_path.clone();
            let m = tokio::task::spawn_blocking(move || check_mergeability(&d_c, &b_c, &h_c))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;
            let num = state
                .store()
                .allocate_pr_number(repo.instance_id, repo.org_id, repo.id)
                .await?;
            let now = Utc::now();
            let pr = state
                .store()
                .create_pull_request( shako_core::PullRequestRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: repo.instance_id,
                    org_id: repo.org_id,
                    repo_id: repo.id,
                    number: num,
                    state: "open".into(),
                    title,
                    body: pr_body,
                    head_ref: head,
                    head_sha: m.head_sha,
                    base_ref: base,
                    base_sha: m.base_sha,
                    merge_base: m.merge_base,
                    mergeable: Some(m.mergeable),
                    merged: false,
                    merge_commit_sha: String::new(),
                    author_login: ctx.subject.username.clone(),
                    author_name: ctx.subject.full_name.clone(),
                    author_email: ctx.subject.email.clone(),
                    created_at: now,
                    updated_at: now,
                    closed_at: None,
                })
                .await?;
            Ok(json!({
                "Id": format!("{}:{}", repo.id, pr.number),
                "RepoId": repo.id.to_string(),
                "Number": pr.number,
                "State": pr.state,
                "HeadSha": pr.head_sha,
                "Mergeable": pr.mergeable,
            }))
        }
        "Review" | "jc_forge_review_pull_request" | "review-pr" => {
            let (r_key, pr_num) = resolve_pr_target(&repo_key, &body)?;
            let (repo, _) = find_visible_repo_by_key(state, ctx, &r_key).await?;
            let ev_str = get_str("Event", "event")
                .ok_or_else(|| ShakoError::Unprocessable("Event required".into()))?;
            let verdict = ReviewVerdict::parse(&ev_str)
                .ok_or_else(|| ShakoError::Unprocessable(format!("invalid verdict '{ev_str}'")))?;
            let needed = if verdict == ReviewVerdict::Comment {
                Action::Propose
            } else {
                Action::Approve
            };
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, needed).await?;
            let pr = state
                .store()
                .get_pull_request(repo.instance_id, repo.org_id, repo.id, pr_num)
                .await?
                .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))?;
            let rev = state
                .store()
                .create_review(ReviewRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: repo.instance_id,
                    org_id: repo.org_id,
                    repo_id: repo.id,
                    pr_number: pr_num,
                    verdict,
                    body: get_str("Body", "body").unwrap_or_default(),
                    author_login: ctx.subject.username.clone(),
                    commit_id: pr.head_sha,
                    dismissed: false,
                    created_at: Utc::now(),
                })
                .await?;
            Ok(json!({ "Id": rev.id.to_string(), "Verdict": rev.verdict.as_str() }))
        }
        "Merge" | "jc_forge_merge_pull_request" | "merge-pr" => {
            let (r_key, pr_num) = resolve_pr_target(&repo_key, &body)?;
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &r_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Write).await?;
            let style_s = get_str("Style", "style").unwrap_or_else(|| "merge".into());
            let style = MergeStyle::parse(&style_s)?;
            let msg = get_str("Message", "message")
                .unwrap_or_else(|| format!("Merge pull request #{pr_num}"));
            let head_cid = get_str("HeadCommitId", "headCommitId");

            let _guard = state.repo_locks.lock(repo.id).await;
            let mut pr = state
                .store()
                .get_pull_request(repo.instance_id, repo.org_id, repo.id, pr_num)
                .await?
                .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))?;
            let b_ref = pr.base_ref.clone();
            let h_ref = pr.head_ref.clone();
            let d_c = disk_path.clone();
            let m = tokio::task::spawn_blocking(move || check_mergeability(&d_c, &b_ref, &h_ref))
                .await
                .map_err(|e| ShakoError::Internal(e.to_string()))??;

            if let Some(exp) = head_cid.as_deref().filter(|s| !s.is_empty())
                && exp != m.head_sha
            {
                return Err(ShakoError::Conflict("head commit has moved since review".into()));
            }
            let policy = state
                .ref_policy_for(repo.instance_id, repo.org_id, repo.id)
                .await?;
            let reviews = state
                .store()
                .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(pr_num))
                .await?;
            let statuses = state
                .store()
                .list_commit_statuses(repo.instance_id, repo.org_id, repo.id, &m.head_sha)
                .await?;
            policy.check_pr_merge(
                ctx,
                state.authorizer.as_ref(),
                &repo.owner,
                &repo.name,
                &pr.base_ref,
                &MergePolicyInput {
                    pr_author_login: &pr.author_login,
                    head_sha: &m.head_sha,
                    is_behind_base: m.is_behind_base,
                    reviews: &reviews,
                    statuses: &statuses,
                },
            )?;

            let author = CommitAuthor {
                name: ctx.subject.full_name.clone(),
                email: ctx
                    .subject
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
            };
            let b_ref2 = pr.base_ref.clone();
            let h_ref2 = pr.head_ref.clone();
            let sha = tokio::task::spawn_blocking(move || {
                execute_pr_merge(
                    &disk_path,
                    &b_ref2,
                    &h_ref2,
                    head_cid.as_deref(),
                    style,
                    &msg,
                    &author,
                    &author,
                )
            })
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;

            pr.state = "closed".into();
            pr.merged = true;
            pr.merge_commit_sha = sha.clone();
            pr.closed_at = Some(Utc::now());
            let saved = state.store().update_pull_request(pr).await?;
            let payload =
                build_pr_webhook_payload(&repo, &saved, "closed", &ctx.subject.username);
            let _ = emit_and_dispatch_event(
                state,
                repo.org_id,
                Some(repo.id),
                "pull_request",
                format!("pr_merge:{}:{}", repo.id, saved.number),
                payload,
            )
            .await;
            Ok(json!({ "sha": sha, "merged": true }))
        }
        "UpdateBranch" | "jc_forge_update_pull_request_branch" | "update-pr-branch" => {
            let (r_key, pr_num) = resolve_pr_target(&repo_key, &body)?;
            let (repo, disk_path) = find_visible_repo_by_key(state, ctx, &r_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Propose)
                .await?;
            let _guard = state.repo_locks.lock(repo.id).await;
            let mut pr = state
                .store()
                .get_pull_request(repo.instance_id, repo.org_id, repo.id, pr_num)
                .await?
                .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))?;
            let author = CommitAuthor {
                name: ctx.subject.full_name.clone(),
                email: ctx
                    .subject
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("{}@local", ctx.subject.username)),
            };
            let b_ref = pr.base_ref.clone();
            let h_ref = pr.head_ref.clone();
            let new_head = tokio::task::spawn_blocking(move || {
                update_pr_branch(&disk_path, &b_ref, &h_ref, &author)
            })
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))??;
            pr.head_sha = new_head.clone();
            let _ = state.store().update_pull_request(pr).await;
            Ok(json!({ "headSha": new_head }))
        }
        "Close" | "jc_forge_close_pull_request" | "close-pr" => {
            let (r_key, pr_num) = resolve_pr_target(&repo_key, &body)?;
            let (repo, _) = find_visible_repo_by_key(state, ctx, &r_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Propose)
                .await?;
            let mut pr = state
                .store()
                .get_pull_request(repo.instance_id, repo.org_id, repo.id, pr_num)
                .await?
                .ok_or_else(|| ShakoError::NotFound("pull request not found".into()))?;
            pr.state = "closed".into();
            pr.closed_at = Some(Utc::now());
            let saved = state.store().update_pull_request(pr).await?;
            Ok(json!({ "Number": saved.number, "State": saved.state }))
        }
        "Protect" | "jc_forge_protect_branch" | "protect" => {
            let (repo, _) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Admin).await?;
            let rule_val = body
                .get("Rule")
                .or_else(|| body.get("rule"))
                .cloned()
                .unwrap_or(body);
            let rule: BranchProtectionRule = serde_json::from_value(rule_val)
                .map_err(|e| ShakoError::Unprocessable(format!("invalid Rule: {e}")))?;
            RefPolicy::validate_rule_for_repo(
                &rule,
                state.authorizer.as_ref(),
                repo.instance_id,
                repo.org_id,
                &repo.owner,
                &repo.name,
            )?;
            let now = Utc::now();
            let saved = state
                .store()
                .upsert_protection_rule(ProtectionRuleRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id: repo.instance_id,
                    org_id: repo.org_id,
                    repo_id: repo.id,
                    rule,
                    created_at: now,
                    updated_at: now,
                })
                .await?;
            Ok(json!({
                "Id": format!("{}:{}", repo.id, saved.rule.rule_name),
                "Pattern": saved.rule.rule_name,
            }))
        }
        "Unprotect" | "jc_forge_unprotect_branch" | "unprotect" => {
            let (repo, _) = find_visible_repo_by_key(state, ctx, &repo_key).await?;
            resolve_and_authorize_repo(state, ctx, &repo.owner, &repo.name, Action::Admin).await?;
            let pattern = get_str("Pattern", "pattern")
                .ok_or_else(|| ShakoError::Unprocessable("Pattern required".into()))?;
            let deleted = state
                .store()
                .delete_protection_rule(repo.instance_id, repo.org_id, repo.id, &pattern)
                .await?;
            Ok(json!({ "deleted": deleted }))
        }
        "Tree" | "jc_forge_tree" | "tree" => {
            let mut args = BTreeMap::new();
            if let Some(r) = get_str("Ref", "ref") {
                args.insert("Ref".into(), r);
            }
            if body
                .get("Recursive")
                .or_else(|| body.get("recursive"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                args.insert("Recursive".into(), "true".into());
            }
            exec_bound_repo_tree(state, ctx, &repo_key, &args).await
        }
        "File" | "jc_forge_file" | "file" => {
            let mut args = BTreeMap::new();
            if let Some(r) = get_str("Ref", "ref") {
                args.insert("Ref".into(), r);
            }
            if let Some(p) = get_str("Path", "path") {
                args.insert("Path".into(), p);
            }
            exec_bound_repo_file(state, ctx, &repo_key, &args).await
        }
        "History" | "jc_forge_history" | "history" => {
            let mut args = BTreeMap::new();
            if let Some(r) = get_str("Ref", "ref") {
                args.insert("Ref".into(), r);
            }
            if let Some(p) = get_str("Path", "path") {
                args.insert("Path".into(), p);
            }
            exec_bound_repo_history(state, ctx, &repo_key, &args).await
        }
        "Compare" | "jc_forge_compare" | "compare" => {
            let mut args = BTreeMap::new();
            if let Some(b) = get_str("Base", "base") {
                args.insert("Base".into(), b);
            }
            if let Some(h) = get_str("Head", "head") {
                args.insert("Head".into(), h);
            }
            exec_bound_repo_compare(state, ctx, &repo_key, &args).await
        }
        "Allowed" | "jc_forge_allowed" | "allowed" => {
            let op = get_str("Operation", "operation").unwrap_or_else(|| "Read".into());
            let obj = get_str("Object", "object");
            let dec = evaluate_allowed_query(state, ctx, &op, obj.as_deref()).await;
            Ok(serde_json::to_value(dec).unwrap_or(Value::Null))
        }
        _ => {
            // Check if it is an entity-set list operation (e.g. `jc_forge_repositories_list`, `jc_forge_quota`)
            if let Some(op_def) = state.registry.find(op_name)
                && let Some(set_name) = op_def.entity_set
            {
                let filter_q = get_str("filter", "Filter").map(|f| format!("$filter={f}"));
                let table = build_entity_table(
                    state,
                    ctx,
                    mount,
                    set_name,
                    None,
                    filter_q.as_deref(),
                )
                .await?;
                let scope = scope_for_context(ctx, state.authorizer.as_ref());
                let (opts, _) = parse_odata_options_with_expand(
                    &table.info().model,
                    set_name,
                    filter_q.as_deref(),
                )?;
                let ans = table_query(&table, &opts, &scope, MAX_ODATA_PAGE)
                    .await
                    .map_err(map_table_error)?;
                return Ok(json!({ "value": ans.rows }));
            }
            Err(ShakoError::NotFound(format!(
                "unknown operation '{op_name}'"
            )))
        }
    }
}

fn resolve_pr_target(bound_or_repo: &str, body: &Value) -> ShakoResult<(String, u64)> {
    if let Some((r, n_str)) = bound_or_repo.rsplit_once(':')
        && let Ok(n) = n_str.parse::<u64>()
    {
        return Ok((r.to_owned(), n));
    }
    let num = body
        .get("Number")
        .or_else(|| body.get("number"))
        .and_then(Value::as_u64)
        .ok_or_else(|| ShakoError::Unprocessable("PullRequest Number required".into()))?;
    Ok((bound_or_repo.to_owned(), num))
}

/// Builds the OData 4.01 service router at `/odata` and `/odata/*` (Contract S9).
pub fn odata_router() -> Router<ShakoState> {
    Router::new()
        .route("/odata", get(odata_service_doc))
        .route("/odata/", get(odata_service_doc))
        .route("/odata/$metadata", get(odata_metadata))
        .route(
            "/odata/{seg1}",
            get(odata_single_get).post(odata_single_post),
        )
        .route(
            "/odata/{seg1}/{seg2}",
            get(odata_two_segment_get).post(odata_two_segment_post),
        )
}
