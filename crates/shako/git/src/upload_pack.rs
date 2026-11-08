#![forbid(unsafe_code)]
//! Git Smart HTTP `upload-pack` server for Protocol v0 and Protocol v2 (Contract S3).
//!
//! Capabilities supported:
//! - Protocol v0: `multi_ack_detailed no-done side-band-64k thin-pack ofs-delta shallow
//!   deepen-since deepen-not no-progress include-tag symref=HEAD:<default>
//!   allow-tip-sha1-in-want allow-reachable-sha1-in-want filter agent=shako/0.1.0`
//! - Protocol v2: `ls-refs=unborn`, `fetch=shallow filter ref-in-want sideband-all wait-for-done`,
//!   `object-info`, `server-option`, `object-format=sha1`, `agent=shako/0.1.0`.
//!   `packfile-uris` is explicitly refused (S3).
//! - Pack generation calls `odb.prevent_pack_unload()` before `gix-pack` counting/encoding
//!   and streams 64 KiB sideband frames.

use crate::pktline::{
    MAX_SIDEBAND_1K_DATA, MAX_SIDEBAND_64K_DATA, PktLine, parse_all_pkt_lines, write_delim,
    write_err_line, write_flush, write_sideband_chunks, write_text_line,
};
use crate::repo::open_isolated;
use bstr::ByteSlice;
use gix_hash::ObjectId;
use gix_pack::data::output;
use gix_packetline::Channel;
use shako_core::{ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub const V0_UPLOAD_CAPABILITIES: &str = "multi_ack multi_ack_detailed no-done side-band side-band-64k thin-pack ofs-delta shallow deepen-since deepen-not deepen-relative no-progress include-tag allow-tip-sha1-in-want allow-reachable-sha1-in-want filter object-format=sha1 agent=shako/0.1.0";

#[derive(Debug, Clone)]
pub struct AdvertisedRef {
    pub name: String,
    pub oid: ObjectId,
    pub peeled: Option<ObjectId>,
    pub symref_target: Option<String>,
}

/// Collects advertised references in C-locale order (`HEAD` first, then `refs/heads/*` and `refs/tags/*`).
/// `refs/pull/*` is hidden from advertisement unless `include_hidden` is set (S3).
pub fn collect_advertised_refs(
    repo: &gix::Repository,
    include_hidden: bool,
) -> ShakoResult<(Option<String>, Vec<AdvertisedRef>)> {
    let mut out = Vec::new();
    let head_symref = repo
        .head_name()
        .ok()
        .flatten()
        .map(|n| n.as_bstr().to_str_lossy().into_owned());

    if let Ok(head_id) = repo.head_id() {
        out.push(AdvertisedRef {
            name: "HEAD".into(),
            oid: head_id.detach(),
            peeled: None,
            symref_target: head_symref.clone(),
        });
    }

    let platform = repo
        .references()
        .map_err(|e| ShakoError::Git(format!("references: {e}")))?;
    let all_refs = platform
        .all()
        .map_err(|e| ShakoError::Git(format!("all refs: {e}")))?;

    let mut named: BTreeMap<String, AdvertisedRef> = BTreeMap::new();
    for mut r in all_refs.flatten() {
        let name = r.name().as_bstr().to_str_lossy().into_owned();
        if name.starts_with("refs/pull/") && !include_hidden {
            continue;
        }
        if !(name.starts_with("refs/heads/")
            || name.starts_with("refs/tags/")
            || (include_hidden && name.starts_with("refs/pull/")))
        {
            continue;
        }
        let Some(direct_id) = r.target().try_id().map(ToOwned::to_owned) else {
            continue;
        };
        let is_annotated_tag = name.starts_with("refs/tags/")
            && repo
                .find_header(direct_id)
                .is_ok_and(|h| h.kind() == gix_object::Kind::Tag);
        let peeled = if is_annotated_tag {
            r.peel_to_id().ok().map(|id| id.detach())
        } else {
            None
        };
        named.insert(
            name.clone(),
            AdvertisedRef {
                name,
                oid: direct_id,
                peeled,
                symref_target: None,
            },
        );
    }
    out.extend(named.into_values());
    Ok((head_symref, out))
}

/// Generates the `GET {repo}.git/info/refs?service=git-upload-pack` response for Protocol v0 or v2 (S3).
pub fn advertise_upload_pack(repo_path: &Path, git_protocol: Option<&str>) -> ShakoResult<Vec<u8>> {
    let is_v2 = git_protocol
        .is_some_and(|hdr| hdr.split(':').any(|p| p.trim() == "version=2"));

    let mut out = Vec::new();
    if is_v2 {
        write_text_line(&mut out, "version 2")?;
        write_text_line(&mut out, "agent=shako/0.1.0")?;
        write_text_line(&mut out, "ls-refs=unborn")?;
        write_text_line(
            &mut out,
            "fetch=shallow filter ref-in-want sideband-all wait-for-done",
        )?;
        write_text_line(&mut out, "server-option")?;
        write_text_line(&mut out, "object-format=sha1")?;
        write_text_line(&mut out, "object-info")?;
        write_flush(&mut out)?;
        return Ok(out);
    }

    let repo = open_isolated(repo_path)?;
    let (head_symref, refs) = collect_advertised_refs(&repo, false)?;

    write_text_line(&mut out, "# service=git-upload-pack")?;
    write_flush(&mut out)?;

    let symref_cap = match &head_symref {
        Some(target) => format!("symref=HEAD:{target} {V0_UPLOAD_CAPABILITIES}"),
        None => V0_UPLOAD_CAPABILITIES.to_owned(),
    };

    if refs.is_empty() {
        let zero = ObjectId::null(gix_hash::Kind::Sha1);
        write_text_line(
            &mut out,
            &format!("{zero} capabilities^{{}}\0{symref_cap}"),
        )?;
    } else {
        for (idx, r) in refs.iter().enumerate() {
            if idx == 0 {
                write_text_line(&mut out, &format!("{} {}\0{symref_cap}", r.oid, r.name))?;
            } else {
                write_text_line(&mut out, &format!("{} {}", r.oid, r.name))?;
            }
            if let Some(peeled) = r.peeled {
                write_text_line(&mut out, &format!("{peeled} {}^{{}}", r.name))?;
            }
        }
    }
    write_flush(&mut out)?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFilter {
    None,
    BlobNone,
    BlobLimit(u64),
    TreeDepth(u64),
}

impl ObjectFilter {
    pub fn parse(spec: &str) -> ShakoResult<Self> {
        let s = spec.trim();
        if s == "blob:none" {
            return Ok(Self::BlobNone);
        }
        if let Some(lim) = s.strip_prefix("blob:limit=") {
            let n = parse_scaled_u64(lim)
                .ok_or_else(|| ShakoError::BadRequest(format!("invalid blob:limit '{lim}'")))?;
            return Ok(Self::BlobLimit(n));
        }
        if let Some(d) = s.strip_prefix("tree:") {
            let n = d
                .parse::<u64>()
                .map_err(|_| ShakoError::BadRequest(format!("invalid tree filter '{d}'")))?;
            return Ok(Self::TreeDepth(n));
        }
        Err(ShakoError::BadRequest(format!(
            "unsupported filter-spec '{s}'"
        )))
    }
}

fn parse_scaled_u64(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if let Some(k) = raw.strip_suffix(['k', 'K']) {
        return k.parse::<u64>().ok()?.checked_mul(1024);
    }
    if let Some(m) = raw.strip_suffix(['m', 'M']) {
        return m.parse::<u64>().ok()?.checked_mul(1024 * 1024);
    }
    if let Some(g) = raw.strip_suffix(['g', 'G']) {
        return g.parse::<u64>().ok()?.checked_mul(1024 * 1024 * 1024);
    }
    raw.parse::<u64>().ok()
}

/// Handles `POST {repo}.git/git-upload-pack` for both Protocol v0 and Protocol v2 (S3).
pub fn handle_upload_pack(
    repo_path: &Path,
    git_protocol: Option<&str>,
    body: &[u8],
) -> ShakoResult<Vec<u8>> {
    let lines = parse_all_pkt_lines(body)?;
    if lines.is_empty() || lines.iter().all(|l| *l == PktLine::Flush) {
        return Ok(Vec::new());
    }

    let is_v2 = git_protocol
        .is_some_and(|hdr| hdr.split(':').any(|p| p.trim() == "version=2"))
        || lines
            .first()
            .and_then(PktLine::as_text)
            .is_some_and(|t| t.starts_with("command="));

    if is_v2 {
        handle_upload_pack_v2(repo_path, &lines)
    } else {
        handle_upload_pack_v0(repo_path, &lines)
    }
}

fn handle_upload_pack_v2(repo_path: &Path, lines: &[PktLine]) -> ShakoResult<Vec<u8>> {
    let mut idx = 0usize;
    while idx < lines.len() && lines[idx] == PktLine::Flush {
        idx += 1;
    }
    if idx >= lines.len() {
        return Ok(Vec::new());
    }
    let first = lines[idx]
        .as_text()
        .ok_or_else(|| ShakoError::BadRequest("expected v2 command pkt-line".into()))?;
    let cmd = first
        .strip_prefix("command=")
        .ok_or_else(|| ShakoError::BadRequest(format!("expected command=, got '{first}'")))?;
    idx += 1;

    // Read capabilities up to Delimiter (0001) or Flush (0000)
    while idx < lines.len() {
        match &lines[idx] {
            PktLine::Delimiter => {
                idx += 1;
                break;
            }
            PktLine::Flush => break,
            _ => idx += 1,
        }
    }

    // Collect command args up to Flush
    let mut args = Vec::new();
    while idx < lines.len() {
        match &lines[idx] {
            PktLine::Flush | PktLine::ResponseEnd => break,
            PktLine::Data(_) => {
                if let Some(txt) = lines[idx].as_text() {
                    args.push(txt.to_owned());
                }
                idx += 1;
            }
            PktLine::Delimiter => idx += 1,
        }
    }

    match cmd {
        "ls-refs" => handle_v2_ls_refs(repo_path, &args),
        "fetch" => handle_v2_fetch(repo_path, &args),
        "object-info" => handle_v2_object_info(repo_path, &args),
        other => Err(ShakoError::BadRequest(format!(
            "unsupported protocol v2 command '{other}'"
        ))),
    }
}

fn handle_v2_ls_refs(repo_path: &Path, args: &[String]) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let mut peel = false;
    let mut symrefs = false;
    let mut unborn = false;
    let mut prefixes = Vec::new();

    for arg in args {
        if arg == "peel" {
            peel = true;
        } else if arg == "symrefs" {
            symrefs = true;
        } else if arg == "unborn" {
            unborn = true;
        } else if let Some(p) = arg.strip_prefix("ref-prefix ") {
            prefixes.push(p.to_owned());
        }
    }

    let (head_symref, refs) = collect_advertised_refs(&repo, true)?;
    let mut out = Vec::new();

    // If repo is unborn (no HEAD commit) and client requested `unborn`, emit `unborn HEAD symref-target:...`
    if unborn
        && !refs.iter().any(|r| r.name == "HEAD")
        && let Some(target) = &head_symref
        && (prefixes.is_empty() || prefixes.iter().any(|p| "HEAD".starts_with(p)))
    {
        if symrefs {
            write_text_line(&mut out, &format!("unborn HEAD symref-target:{target}"))?;
        } else {
            write_text_line(&mut out, "unborn HEAD")?;
        }
    }

    for r in &refs {
        // Hide `refs/pull/*` unless explicitly requested via `ref-prefix`
        if r.name.starts_with("refs/pull/")
            && !prefixes.iter().any(|p| p.starts_with("refs/pull/"))
        {
            continue;
        }
        if !prefixes.is_empty() && !prefixes.iter().any(|p| r.name.starts_with(p)) {
            continue;
        }
        let mut line = format!("{} {}", r.oid, r.name);
        if symrefs && let Some(sym) = &r.symref_target {
            line.push_str(&format!(" symref-target:{sym}"));
        }
        if peel && let Some(peeled) = r.peeled {
            line.push_str(&format!(" peeled:{peeled}"));
        }
        write_text_line(&mut out, &line)?;
    }
    write_flush(&mut out)?;
    Ok(out)
}

fn handle_v2_object_info(repo_path: &Path, args: &[String]) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let mut want_size = false;
    let mut oids = Vec::new();
    for arg in args {
        if arg == "size" {
            want_size = true;
        } else if let Some(hex) = arg.strip_prefix("oid ") {
            let oid = ObjectId::from_hex(hex.trim().as_bytes())
                .map_err(|_| ShakoError::BadRequest(format!("invalid oid '{hex}'")))?;
            oids.push(oid);
        }
    }
    let mut out = Vec::new();
    if want_size {
        write_text_line(&mut out, "size")?;
    }
    for oid in oids {
        let header = repo
            .find_header(oid)
            .map_err(|_| ShakoError::NotFound(format!("object {oid} not found")))?;
        if want_size {
            write_text_line(&mut out, &format!("{oid} {}", header.size()))?;
        } else {
            write_text_line(&mut out, &oid.to_string())?;
        }
    }
    write_flush(&mut out)?;
    Ok(out)
}

#[derive(Default)]
struct FetchParams {
    wants: Vec<ObjectId>,
    want_refs: Vec<String>,
    haves: Vec<ObjectId>,
    client_shallows: BTreeSet<ObjectId>,
    deepen: Option<u32>,
    deepen_relative: bool,
    deepen_since: Option<i64>,
    deepen_not: Vec<String>,
    filter: ObjectFilter,
    thin_pack: bool,
    include_tag: bool,
    done: bool,
    wait_for_done: bool,
    sideband_all: bool,
}

impl Default for ObjectFilter {
    fn default() -> Self {
        Self::None
    }
}

fn handle_v2_fetch(repo_path: &Path, args: &[String]) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let mut params = FetchParams::default();
    let mut seen_want_refs = BTreeSet::new();

    for arg in args {
        if arg.starts_with("packfile-uris") {
            return Err(ShakoError::BadRequest(
                "packfile-uris is refused by this forge".into(),
            ));
        } else if let Some(hex) = arg.strip_prefix("want ") {
            let oid = ObjectId::from_hex(hex.trim().as_bytes())
                .map_err(|_| ShakoError::BadRequest(format!("invalid want oid '{hex}'")))?;
            params.wants.push(oid);
        } else if let Some(rname) = arg.strip_prefix("want-ref ") {
            let rname = rname.trim().to_owned();
            if !seen_want_refs.insert(rname.clone()) {
                return Err(ShakoError::BadRequest(format!(
                    "duplicate want-ref '{rname}'"
                )));
            }
            params.want_refs.push(rname);
        } else if let Some(hex) = arg.strip_prefix("have ") {
            let oid = ObjectId::from_hex(hex.trim().as_bytes())
                .map_err(|_| ShakoError::BadRequest(format!("invalid have oid '{hex}'")))?;
            params.haves.push(oid);
        } else if let Some(hex) = arg.strip_prefix("shallow ") {
            let oid = ObjectId::from_hex(hex.trim().as_bytes())
                .map_err(|_| ShakoError::BadRequest(format!("invalid shallow oid '{hex}'")))?;
            params.client_shallows.insert(oid);
        } else if let Some(d) = arg.strip_prefix("deepen ") {
            let n = d
                .trim()
                .parse::<u32>()
                .map_err(|_| ShakoError::BadRequest("invalid deepen depth".into()))?;
            params.deepen = Some(n);
        } else if arg == "deepen-relative" {
            params.deepen_relative = true;
        } else if let Some(ts) = arg.strip_prefix("deepen-since ") {
            let t = ts
                .trim()
                .parse::<i64>()
                .map_err(|_| ShakoError::BadRequest("invalid deepen-since timestamp".into()))?;
            params.deepen_since = Some(t);
        } else if let Some(rev) = arg.strip_prefix("deepen-not ") {
            params.deepen_not.push(rev.trim().to_owned());
        } else if let Some(fspec) = arg.strip_prefix("filter ") {
            params.filter = ObjectFilter::parse(fspec)?;
        } else if arg == "thin-pack" {
            params.thin_pack = true;
        } else if arg == "include-tag" {
            params.include_tag = true;
        } else if arg == "done" {
            params.done = true;
        } else if arg == "wait-for-done" {
            params.wait_for_done = true;
        } else if arg == "sideband-all" {
            params.sideband_all = true;
        }
    }

    if params.deepen.is_some() && (params.deepen_since.is_some() || !params.deepen_not.is_empty())
    {
        return Err(ShakoError::BadRequest(
            "deepen cannot be combined with deepen-since or deepen-not".into(),
        ));
    }

    // Resolve `want-ref` lines
    let mut resolved_wanted_refs = Vec::new();
    for rname in &params.want_refs {
        let partial: &gix_ref::PartialNameRef = rname
            .as_str()
            .try_into()
            .map_err(|_| ShakoError::BadRequest(format!("invalid want-ref '{rname}'")))?;
        let mut r = repo
            .try_find_reference(partial)
            .map_err(|e| ShakoError::Git(e.to_string()))?
            .ok_or_else(|| ShakoError::BadRequest(format!("wanted ref '{rname}' not found")))?;
        let id = r
            .peel_to_id()
            .map_err(|_| ShakoError::BadRequest(format!("cannot peel '{rname}'")))?
            .detach();
        params.wants.push(id);
        resolved_wanted_refs.push((id, rname.clone()));
    }

    if params.wants.is_empty() {
        return Err(ShakoError::BadRequest("fetch requires at least one want".into()));
    }

    let reachable_Pool = collect_reachable_objects_for_check(&repo)?;
    for w in &params.wants {
        if !repo.has_object(*w) || !reachable_Pool.contains(w) {
            let mut err_out = Vec::new();
            write_err_line(&mut err_out, &format!("upload-pack: not our ref {w}"))?;
            return Err(ShakoError::BadRequest(format!(
                "upload-pack: want {w} is not reachable"
            )));
        }
    }

    let common_haves: Vec<ObjectId> = params
        .haves
        .iter()
        .copied()
        .filter(|h| repo.has_object(*h))
        .collect();

    let ready_to_send = params.done || (!params.wait_for_done && (params.haves.is_empty() || !common_haves.is_empty()));

    let mut out = Vec::new();
    let write_line = |buf: &mut Vec<u8>, line: &str, sideband_all: bool| -> ShakoResult<()> {
        if sideband_all {
            let with_lf = format!("{line}\n");
            write_sideband_chunks(buf, Channel::Data, with_lf.as_bytes(), MAX_SIDEBAND_64K_DATA)
        } else {
            write_text_line(buf, line)
        }
    };

    if !params.done {
        write_line(&mut out, "acknowledgments", params.sideband_all)?;
        if common_haves.is_empty() {
            write_line(&mut out, "NAK", params.sideband_all)?;
        } else {
            for c in &common_haves {
                write_line(&mut out, &format!("ACK {c}"), params.sideband_all)?;
            }
        }
        if ready_to_send {
            write_line(&mut out, "ready", params.sideband_all)?;
            write_delim(&mut out)?;
        } else {
            write_flush(&mut out)?;
            return Ok(out);
        }
    }

    let (shallow_boundaries, unshallows, object_set) =
        compute_pack_object_set(&repo, &params, &common_haves)?;

    if !shallow_boundaries.is_empty() || !unshallows.is_empty() {
        write_line(&mut out, "shallow-info", params.sideband_all)?;
        for s in &shallow_boundaries {
            write_line(&mut out, &format!("shallow {s}"), params.sideband_all)?;
        }
        for u in &unshallows {
            write_line(&mut out, &format!("unshallow {u}"), params.sideband_all)?;
        }
        write_delim(&mut out)?;
    }

    if !resolved_wanted_refs.is_empty() {
        write_line(&mut out, "wanted-refs", params.sideband_all)?;
        for (oid, rname) in &resolved_wanted_refs {
            write_line(&mut out, &format!("{oid} {rname}"), params.sideband_all)?;
        }
        write_delim(&mut out)?;
    }

    write_line(&mut out, "packfile", params.sideband_all)?;
    let pack_bytes = build_pack_bytes(&repo, &object_set, params.thin_pack)?;
    write_sideband_chunks(&mut out, Channel::Data, &pack_bytes, MAX_SIDEBAND_64K_DATA)?;
    write_flush(&mut out)?;
    Ok(out)
}

fn handle_upload_pack_v0(repo_path: &Path, lines: &[PktLine]) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let mut params = FetchParams::default();
    let mut caps = BTreeSet::new();
    let mut idx = 0usize;

    // 1. Parse `want`, `shallow`, `deepen*`, `filter` up to first Flush
    while idx < lines.len() {
        match &lines[idx] {
            PktLine::Flush => {
                idx += 1;
                break;
            }
            PktLine::Data(_) => {
                let Some(text) = lines[idx].as_text() else {
                    idx += 1;
                    continue;
                };
                if let Some(rest) = text.strip_prefix("want ") {
                    let (oid_part, cap_part) = if let Some((o, c)) = rest.split_once('\0') {
                        (o.trim(), Some(c))
                    } else if let Some((o, c)) = rest.split_once(' ') {
                        (o.trim(), Some(c))
                    } else {
                        (rest.trim(), None)
                    };
                    let oid = ObjectId::from_hex(oid_part.as_bytes()).map_err(|_| {
                        ShakoError::BadRequest(format!("invalid want oid '{oid_part}'"))
                    })?;
                    params.wants.push(oid);
                    if let Some(c_str) = cap_part {
                        for cap in c_str.split_whitespace() {
                            caps.insert(cap.to_owned());
                        }
                    }
                } else if let Some(hex) = text.strip_prefix("shallow ") {
                    let oid = ObjectId::from_hex(hex.trim().as_bytes())
                        .map_err(|_| ShakoError::BadRequest("invalid shallow oid".into()))?;
                    params.client_shallows.insert(oid);
                } else if let Some(d) = text.strip_prefix("deepen ") {
                    let n = d
                        .trim()
                        .parse::<u32>()
                        .map_err(|_| ShakoError::BadRequest("invalid deepen".into()))?;
                    params.deepen = Some(n);
                } else if let Some(ts) = text.strip_prefix("deepen-since ") {
                    let t = ts
                        .trim()
                        .parse::<i64>()
                        .map_err(|_| ShakoError::BadRequest("invalid deepen-since".into()))?;
                    params.deepen_since = Some(t);
                } else if let Some(rev) = text.strip_prefix("deepen-not ") {
                    params.deepen_not.push(rev.trim().to_owned());
                } else if let Some(fspec) = text.strip_prefix("filter ") {
                    params.filter = ObjectFilter::parse(fspec)?;
                }
                idx += 1;
            }
            _ => idx += 1,
        }
    }

    if caps.contains("side-band") && caps.contains("side-band-64k") {
        return Err(ShakoError::BadRequest(
            "client must not request both side-band and side-band-64k".into(),
        ));
    }
    params.thin_pack = caps.contains("thin-pack");
    params.include_tag = caps.contains("include-tag");
    params.deepen_relative = caps.contains("deepen-relative");

    if params.wants.is_empty() {
        return Ok(Vec::new());
    }

    let reachable_pool = collect_reachable_objects_for_check(&repo)?;
    for w in &params.wants {
        if !repo.has_object(*w) || !reachable_pool.contains(w) {
            return Err(ShakoError::BadRequest(format!(
                "upload-pack: want {w} is not reachable"
            )));
        }
    }

    // 2. Parse `have` lines and optional `done`
    while idx < lines.len() {
        if let Some(text) = lines[idx].as_text() {
            if let Some(hex) = text.strip_prefix("have ") {
                let oid = ObjectId::from_hex(hex.trim().as_bytes())
                    .map_err(|_| ShakoError::BadRequest("invalid have oid".into()))?;
                params.haves.push(oid);
            } else if text == "done" {
                params.done = true;
                break;
            }
        }
        idx += 1;
    }

    let mut out = Vec::new();
    let common_haves: Vec<ObjectId> = params
        .haves
        .iter()
        .copied()
        .filter(|h| repo.has_object(*h))
        .collect();

    let (shallow_boundaries, unshallows, object_set) =
        compute_pack_object_set(&repo, &params, &common_haves)?;

    // If client requested shallow/deepen, send shallow/unshallow + flush-pkt
    if params.deepen.is_some() || params.deepen_since.is_some() || !params.deepen_not.is_empty() {
        for s in &shallow_boundaries {
            write_text_line(&mut out, &format!("shallow {s}"))?;
        }
        for u in &unshallows {
            write_text_line(&mut out, &format!("unshallow {u}"))?;
        }
        write_flush(&mut out)?;
    }

    let multi_ack_detailed = caps.contains("multi_ack_detailed");
    let multi_ack = caps.contains("multi_ack");
    let no_done = caps.contains("no-done");

    if !params.done {
        if multi_ack_detailed {
            for c in &common_haves {
                write_text_line(&mut out, &format!("ACK {c} common"))?;
            }
            if let Some(last) = common_haves.last() {
                write_text_line(&mut out, &format!("ACK {last} ready"))?;
                if no_done {
                    write_text_line(&mut out, "NAK")?;
                    write_text_line(&mut out, &format!("ACK {last}"))?;
                } else {
                    write_text_line(&mut out, "NAK")?;
                    return Ok(out);
                }
            } else {
                write_text_line(&mut out, "NAK")?;
                return Ok(out);
            }
        } else if multi_ack {
            for c in &common_haves {
                write_text_line(&mut out, &format!("ACK {c} continue"))?;
            }
            write_text_line(&mut out, "NAK")?;
            return Ok(out);
        } else {
            if let Some(first) = common_haves.first() {
                write_text_line(&mut out, &format!("ACK {first}"))?;
            } else {
                write_text_line(&mut out, "NAK")?;
            }
            return Ok(out);
        }
    } else if let Some(last) = common_haves.last() {
        if multi_ack || multi_ack_detailed {
            for c in &common_haves {
                let status = if multi_ack_detailed { "common" } else { "continue" };
                write_text_line(&mut out, &format!("ACK {c} {status}"))?;
            }
        }
        write_text_line(&mut out, &format!("ACK {last}"))?;
    } else {
        write_text_line(&mut out, "NAK")?;
    }

    let pack_bytes = build_pack_bytes(&repo, &object_set, params.thin_pack)?;
    if caps.contains("side-band-64k") {
        write_sideband_chunks(&mut out, Channel::Data, &pack_bytes, MAX_SIDEBAND_64K_DATA)?;
        write_flush(&mut out)?;
    } else if caps.contains("side-band") {
        write_sideband_chunks(&mut out, Channel::Data, &pack_bytes, MAX_SIDEBAND_1K_DATA)?;
        write_flush(&mut out)?;
    } else {
        out.extend_from_slice(&pack_bytes);
    }

    Ok(out)
}

/// Computes the set of reachable objects from all references (`HEAD`, `refs/heads/*`, `refs/tags/*`, `refs/pull/*`)
/// to validate `allow-reachable-sha1-in-want` and reject unreachable object probes (S3).
fn collect_reachable_objects_for_check(repo: &gix::Repository) -> ShakoResult<BTreeSet<ObjectId>> {
    let (_, refs) = collect_advertised_refs(repo, true)?;
    let mut reachable = BTreeSet::new();
    let mut commit_tips = Vec::new();

    for r in refs {
        reachable.insert(r.oid);
        if let Some(peeled) = r.peeled {
            reachable.insert(peeled);
            commit_tips.push(peeled);
        } else if repo.find_commit(r.oid).is_ok() {
            commit_tips.push(r.oid);
        }
    }

    if !commit_tips.is_empty()
        && let Ok(walk) = repo.rev_walk(commit_tips).all()
    {
        for info in walk.flatten() {
            reachable.insert(info.id);
        }
    }
    Ok(reachable)
}

/// Walks the commit and tree graph from `params.wants`, respecting `common_haves`, `deepen`,
/// `deepen_since`, `deepen_not`, `include_tag`, and `filter`, returning
/// `(shallow_boundaries, unshallows, objects_to_pack)`.
fn compute_pack_object_set(
    repo: &gix::Repository,
    params: &FetchParams,
    common_haves: &[ObjectId],
) -> ShakoResult<(Vec<ObjectId>, Vec<ObjectId>, Vec<ObjectId>)> {
    let mut stop_commits: BTreeSet<ObjectId> = common_haves.iter().copied().collect();
    // Expand ancestors of `common_haves` and `deepen_not` so we don't send objects the client already has
    let mut boundary_seeds = common_haves.to_vec();
    for rev in &params.deepen_not {
        if let Ok(oid) = crate::repo::resolve_commit_oid(repo, rev) {
            boundary_seeds.push(oid);
            stop_commits.insert(oid);
        }
    }
    if !boundary_seeds.is_empty()
        && let Ok(walk) = repo.rev_walk(boundary_seeds).all()
    {
        for item in walk.flatten() {
            stop_commits.insert(item.id);
        }
    }

    let mut objects = BTreeSet::new();
    let mut included_commits = BTreeSet::new();
    let mut shallow_boundaries = Vec::new();
    let mut queue: VecDeque<(ObjectId, u32)> = VecDeque::new();

    for &want in &params.wants {
        let Ok(obj) = repo.find_object(want) else {
            continue;
        };
        match obj.kind {
            gix_object::Kind::Tag => {
                objects.insert(want);
                if let Ok(peeled) = obj.peel_to_kind(gix_object::Kind::Commit) {
                    queue.push_back((peeled.id, 1));
                }
            }
            gix_object::Kind::Commit => {
                queue.push_back((want, 1));
            }
            gix_object::Kind::Tree => {
                if let Ok(tree) = repo.find_tree(want) {
                    collect_filtered_tree_objects(repo, &tree, 0, params.filter, &mut objects)?;
                }
            }
            gix_object::Kind::Blob => {
                // Explicitly wanted blobs are always included even under blob:none
                objects.insert(want);
            }
        }
    }

    while let Some((commit_oid, depth)) = queue.pop_front() {
        if stop_commits.contains(&commit_oid) || !included_commits.insert(commit_oid) {
            continue;
        }
        let Ok(commit) = repo.find_commit(commit_oid) else {
            continue;
        };
        if let Some(min_ts) = params.deepen_since
            && let Ok(time) = commit.time()
            && time.seconds < min_ts
        {
            included_commits.remove(&commit_oid);
            continue;
        }

        objects.insert(commit_oid);
        if let Ok(tree) = commit.tree() {
            collect_filtered_tree_objects(repo, &tree, 0, params.filter, &mut objects)?;
        }

        let at_depth_limit = params.deepen.is_some_and(|max_d| depth >= max_d);
        let parents: Vec<ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
        if at_depth_limit && !parents.is_empty() {
            shallow_boundaries.push(commit_oid);
            continue;
        }
        for p in parents {
            if !stop_commits.contains(&p) {
                queue.push_back((p, depth.saturating_add(1)));
            }
        }
    }

    // Compute `unshallow` lines: only commits the client declared `shallow` whose parents are now included
    let mut unshallows = Vec::new();
    for client_shallow in &params.client_shallows {
        if included_commits.contains(client_shallow)
            && !shallow_boundaries.contains(client_shallow)
            && let Ok(c) = repo.find_commit(*client_shallow)
            && c.parent_ids().all(|p| included_commits.contains(&p.detach()))
        {
            unshallows.push(*client_shallow);
        }
    }

    // `include-tag`: include annotated tag objects whose target is in `objects`
    if params.include_tag
        && let Ok(platform) = repo.references()
        && let Ok(tags) = platform.tags()
    {
        for mut r in tags.flatten() {
            if let Some(tag_oid) = r.target().try_id().map(ToOwned::to_owned)
                && repo
                    .find_header(tag_oid)
                    .is_ok_and(|h| h.kind() == gix_object::Kind::Tag)
                && let Ok(peeled) = r.peel_to_id()
                && objects.contains(&peeled.detach())
            {
                objects.insert(tag_oid);
            }
        }
    }

    Ok((
        shallow_boundaries,
        unshallows,
        objects.into_iter().collect(),
    ))
}

fn collect_filtered_tree_objects(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    depth: u64,
    filter: ObjectFilter,
    objects: &mut BTreeSet<ObjectId>,
) -> ShakoResult<()> {
    if let ObjectFilter::TreeDepth(max_d) = filter
        && depth >= max_d
    {
        return Ok(());
    }
    if !objects.insert(tree.id) {
        return Ok(());
    }
    let decoded = tree
        .decode()
        .map_err(|e| ShakoError::Git(format!("decode tree: {e}")))?;
    for entry in decoded.entries {
        if entry.mode.is_tree() {
            let sub = repo
                .find_tree(entry.oid)
                .map_err(|e| ShakoError::Git(format!("find subtree: {e}")))?;
            collect_filtered_tree_objects(repo, &sub, depth + 1, filter, objects)?;
        } else if !entry.mode.is_commit() {
            match filter {
                ObjectFilter::BlobNone => {}
                ObjectFilter::BlobLimit(limit) => {
                    if let Ok(hdr) = repo.find_header(entry.oid)
                        && hdr.size() <= limit
                    {
                        objects.insert(entry.oid.to_owned());
                    }
                }
                ObjectFilter::None | ObjectFilter::TreeDepth(_) => {
                    objects.insert(entry.oid.to_owned());
                }
            }
        }
    }
    Ok(())
}

/// Generates a packfile from `object_ids` using `gix-pack` `data::output::count::objects`,
/// `entry::iter_from_counts`, and `bytes::FromEntriesIter` with `prevent_pack_unload()` called first (S3).
pub fn build_pack_bytes(
    repo: &gix::Repository,
    object_ids: &[ObjectId],
    allow_thin_pack: bool,
) -> ShakoResult<Vec<u8>> {
    let mut odb = repo.objects.clone();
    // Critical: prevent pack unload while pack locations are held in `Count` (Contract S3, T-6639)
    odb.prevent_pack_unload();

    let should_interrupt = AtomicBool::new(false);
    let progress = gix_features::progress::Discard;
    let oid_iter: Box<dyn Iterator<Item = Result<ObjectId, std::convert::Infallible>> + Send> =
        Box::new(object_ids.iter().copied().map(Ok));

    let (counts, _outcome) = output::count::objects(
        odb.clone(),
        oid_iter,
        &progress,
        &should_interrupt,
        output::count::objects::Options {
            thread_limit: Some(1),
            chunk_size: 1000,
            input_object_expansion: output::count::objects::ObjectExpansion::AsIs,
        },
    )
    .map_err(|e| ShakoError::Git(format!("pack object count failed: {e}")))?;

    let num_entries = u32::try_from(counts.len())
        .map_err(|_| ShakoError::BadRequest("pack object count exceeds u32".into()))?;

    let entries_iter = output::entry::iter_from_counts(
        counts,
        odb,
        Box::new(gix_features::progress::Discard),
        output::entry::iter_from_counts::Options {
            thread_limit: Some(1),
            mode: output::entry::iter_from_counts::Mode::PackCopyAndBaseObjects,
            allow_thin_pack,
            chunk_size: 1000,
            version: gix_pack::data::Version::V2,
            compression: gix_zlib::Compression::default(),
        },
    );

    let mut pack_bytes = Vec::new();
    let mut bytes_iter = output::bytes::FromEntriesIter::new(
        entries_iter,
        &mut pack_bytes,
        num_entries,
        gix_pack::data::Version::V2,
        gix_hash::Kind::Sha1,
    );
    while let Some(res) = bytes_iter.next() {
        res.map_err(|e| ShakoError::Git(format!("pack byte encoding failed: {e}")))?;
    }

    Ok(pack_bytes)
}
