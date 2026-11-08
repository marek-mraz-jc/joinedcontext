#![forbid(unsafe_code)]
//! Git Smart HTTP `send-pack` client on `gix` for pushing to push mirrors over HTTPS
//! through `OutboundGuard` (Contract S15, R§2.1, Threat T3).

use crate::pktline::{parse_all_pkt_lines, write_flush, write_text_line};
use crate::repo::open_isolated;
use crate::upload_pack::{build_pack_bytes, collect_advertised_refs};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gix_hash::ObjectId;
use shako_core::{OutboundGuard, ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Pushes all `refs/heads/*` and `refs/tags/*` from `repo_path` to `remote_address` over
/// Smart HTTP (`GET .../info/refs?service=git-receive-pack` + `POST .../git-receive-pack`)
/// through `guard`, authenticating with HTTP Basic `username:password`.
/// Never includes credentials in returned errors or logs (Contract S15).
pub async fn push_mirror_over_https(
    repo_path: &Path,
    remote_address: &str,
    username: &str,
    password: &str,
    guard: &OutboundGuard,
    allowed_hosts: Option<&[String]>,
) -> ShakoResult<()> {
    let base = remote_address.trim_end_matches('/');
    let info_url = format!("{base}/info/refs?service=git-receive-pack");

    let mut headers = Vec::new();
    if !password.is_empty() {
        let basic = STANDARD.encode(format!("{username}:{password}"));
        headers.push(("Authorization".to_owned(), format!("Basic {basic}")));
    }

    let (status, adv_bytes) = guard
        .request(
            reqwest::Method::GET,
            &info_url,
            &headers,
            Vec::new(),
            allowed_hosts,
        )
        .await
        .map_err(|e| scrub_secret(&e, password))?;
    if !(200..300).contains(&status) {
        return Err(ShakoError::Git(format!(
            "remote info/refs answered HTTP {status}"
        )));
    }

    let remote_refs = parse_remote_receive_pack_advertisement(&adv_bytes)?;
    let (commands, pack_bytes) = build_send_pack_payload(repo_path, &remote_refs)?;
    if commands.is_empty() {
        return Ok(());
    }

    let mut body = Vec::new();
    for (idx, (old_oid, new_oid, refname)) in commands.iter().enumerate() {
        if idx == 0 {
            write_text_line(
                &mut body,
                &format!("{old_oid} {new_oid} {refname}\0 report-status ofs-delta agent=shako/0.1.0"),
            )?;
        } else {
            write_text_line(&mut body, &format!("{old_oid} {new_oid} {refname}"))?;
        }
    }
    write_flush(&mut body)?;
    body.extend_from_slice(&pack_bytes);

    let mut post_headers = headers;
    post_headers.push((
        "Content-Type".to_owned(),
        "application/x-git-receive-pack-request".to_owned(),
    ));
    let post_url = format!("{base}/git-receive-pack");
    let (post_status, resp_bytes) = guard
        .request(
            reqwest::Method::POST,
            &post_url,
            &post_headers,
            body,
            allowed_hosts,
        )
        .await
        .map_err(|e| scrub_secret(&e, password))?;

    if !(200..300).contains(&post_status) {
        return Err(ShakoError::Git(format!(
            "remote git-receive-pack answered HTTP {post_status}"
        )));
    }

    verify_remote_report_status(&resp_bytes)
}

fn scrub_secret(err: &ShakoError, secret: &str) -> ShakoError {
    if secret.is_empty() {
        return err.clone();
    }
    let msg = err.to_string().replace(secret, "[REDACTED]");
    match err {
        ShakoError::SsrfBlocked(_) => ShakoError::SsrfBlocked(msg),
        _ => ShakoError::Git(msg),
    }
}

fn parse_remote_receive_pack_advertisement(bytes: &[u8]) -> ShakoResult<BTreeMap<String, ObjectId>> {
    let lines = parse_all_pkt_lines(bytes)?;
    let mut map = BTreeMap::new();
    for line in lines {
        let Some(text) = line.as_text() else {
            continue;
        };
        if text.starts_with("# service=") {
            continue;
        }
        let before_nul = text.split('\0').next().unwrap_or(text);
        let mut parts = before_nul.split_whitespace();
        let (Some(oid_s), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if name == "capabilities^{}" || name.ends_with("^{}") {
            continue;
        }
        if let Ok(oid) = ObjectId::from_hex(oid_s.as_bytes()) {
            map.insert(name.to_owned(), oid);
        }
    }
    Ok(map)
}

fn build_send_pack_payload(
    repo_path: &Path,
    remote_refs: &BTreeMap<String, ObjectId>,
) -> ShakoResult<(Vec<(ObjectId, ObjectId, String)>, Vec<u8>)> {
    let repo = open_isolated(repo_path)?;
    let (_, local_refs) = collect_advertised_refs(&repo, false)?;
    let zero = ObjectId::null(gix_hash::Kind::Sha1);

    let mut commands = Vec::new();
    let mut new_tips = Vec::new();

    for r in &local_refs {
        if r.name == "HEAD" {
            continue;
        }
        let old_oid = remote_refs.get(&r.name).copied().unwrap_or(zero);
        if old_oid != r.oid {
            commands.push((old_oid, r.oid, r.name.clone()));
            if let Some(p) = r.peeled {
                new_tips.push(p);
            } else {
                new_tips.push(r.oid);
            }
        }
    }

    if commands.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut objects: BTreeSet<ObjectId> =
        crate::bundle::collect_all_objects_from_tips(&repo, &new_tips)?
            .into_iter()
            .collect();
    for (_, new_oid, _) in &commands {
        if repo.has_object(*new_oid) {
            objects.insert(*new_oid);
        }
    }

    let oid_vec: Vec<ObjectId> = objects.into_iter().collect();
    let pack = build_pack_bytes(&repo, &oid_vec, false)?;
    Ok((commands, pack))
}

fn verify_remote_report_status(resp: &[u8]) -> ShakoResult<()> {
    let lines = parse_all_pkt_lines(resp)?;
    for line in lines {
        let Some(text) = line.as_text() else {
            continue;
        };
        if let Some(reason) = text.strip_prefix("unpack ")
            && reason.trim() != "ok"
        {
            return Err(ShakoError::Git(format!("remote unpack failed: {reason}")));
        }
        if let Some(ng) = text.strip_prefix("ng ") {
            return Err(ShakoError::Git(format!("remote rejected ref: {ng}")));
        }
    }
    Ok(())
}
