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
//! - The pack is self-contained and written one object at a time (`crate::pack`) into the
//!   caller's writer in 64 KiB sideband frames.

use crate::pack::{Admit, Sideband, write_pack};
use crate::pktline::{
    MAX_SIDEBAND_1K_DATA, MAX_SIDEBAND_64K_DATA, PktLine, parse_all_pkt_lines, write_delim,
    write_flush, write_sideband_chunks, write_text_line,
};
use crate::repo::open_isolated;
use crate::{git_err, storage_err};
use bstr::ByteSlice;
use gix_hash::ObjectId;
use gix_packetline::Channel;
use shako_core::{ShakoError, ShakoResult};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::Path;

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

    let platform = repo.references().map_err(git_err("references"))?;
    let all_refs = platform.all().map_err(git_err("all refs"))?;

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
    let is_v2 = git_protocol.is_some_and(|hdr| hdr.split(':').any(|p| p.trim() == "version=2"));

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
        write_text_line(&mut out, &format!("{zero} capabilities^{{}}\0{symref_cap}"))?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectFilter {
    #[default]
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

/// What a parsed negotiation holds per byte of request: the request, a line's own buffer and
/// its place in the list (`parse_all_pkt_lines` refuses many short lines), and the wants and haves.
pub const NEGOTIATION_BYTES_PER_BYTE: u64 = 4;

/// Reads a spooled upload-pack request of at most `limit` bytes, asking `admit` before each
/// MiB of it is held. git gzips one over 1 KiB, so the inflated bytes count too.
pub fn read_negotiation(
    mut file: &std::fs::File,
    gzip: bool,
    limit: u64,
    admit: Admit<'_>,
) -> ShakoResult<Vec<u8>> {
    use std::io::{Read as _, Seek as _};
    const STEP: usize = 1024 * 1024;
    file.rewind().map_err(storage_err("negotiation spool"))?;
    let mut reader: Box<dyn std::io::Read + '_> = if gzip {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut request = Vec::new();
    loop {
        let at = request.len();
        if at as u64 > limit {
            return Err(ShakoError::QuotaExceeded {
                resource: "request_body_bytes".into(),
                limit,
            });
        }
        admit(((at + STEP) as u64).saturating_mul(NEGOTIATION_BYTES_PER_BYTE))?;
        request.reserve_exact(STEP);
        let read = reader
            .by_ref()
            .take(STEP as u64)
            .read_to_end(&mut request)
            .map_err(|e| ShakoError::BadRequest(format!("unreadable upload-pack request: {e}")))?;
        if read == 0 {
            return Ok(request);
        }
    }
}

/// Handles `POST {repo}.git/git-upload-pack` for both Protocol v0 and Protocol v2 (S3), writing
/// the answer to `out`; `admit` is asked before the pack is built.
pub fn handle_upload_pack(
    repo_path: &Path,
    git_protocol: Option<&str>,
    body: &[u8],
    mut out: impl Write,
    admit: Admit<'_>,
) -> ShakoResult<()> {
    let lines = parse_all_pkt_lines(body)?;
    if lines.is_empty() || lines.iter().all(|l| *l == PktLine::Flush) {
        return Ok(());
    }

    let is_v2 = git_protocol.is_some_and(|hdr| hdr.split(':').any(|p| p.trim() == "version=2"))
        || lines
            .first()
            .and_then(PktLine::as_text)
            .is_some_and(|t| t.starts_with("command="));

    let answer = if is_v2 {
        handle_upload_pack_v2(repo_path, &lines)?
    } else {
        handle_upload_pack_v0(repo_path, &lines)?
    };
    out.write_all(&answer.head)
        .map_err(storage_err("upload-pack answer"))?;
    if let Some(Pack {
        repo,
        objects,
        band,
    }) = answer.pack
    {
        match band {
            Some(max) => {
                let mut framed = Sideband::new(&mut out, max);
                write_pack(&repo, &objects, &mut framed, admit)?;
                framed.flush().map_err(storage_err("upload-pack answer"))?;
                drop(framed);
                write_flush(&mut out)?;
            }
            None => write_pack(&repo, &objects, &mut out, admit)?,
        }
    }
    out.flush().map_err(storage_err("upload-pack answer"))
}

/// An upload-pack answer: the negotiation's lines and the pack that follows them.
struct Answer {
    head: Vec<u8>,
    pack: Option<Pack>,
}

struct Pack {
    repo: gix::Repository,
    objects: Vec<ObjectId>,
    /// The side-band payload size, `None` for a raw pack.
    band: Option<usize>,
}

impl From<Vec<u8>> for Answer {
    fn from(head: Vec<u8>) -> Self {
        Self { head, pack: None }
    }
}

fn handle_upload_pack_v2(repo_path: &Path, lines: &[PktLine]) -> ShakoResult<Answer> {
    let mut idx = 0usize;
    while idx < lines.len() && lines[idx] == PktLine::Flush {
        idx += 1;
    }
    // `handle_upload_pack` answers a body of flush packets alone before it gets here.
    let first = lines
        .get(idx)
        .and_then(PktLine::as_text)
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
        "ls-refs" => handle_v2_ls_refs(repo_path, &args).map(Answer::from),
        "fetch" => handle_v2_fetch(repo_path, &args),
        "object-info" => handle_v2_object_info(repo_path, &args).map(Answer::from),
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
        if r.name.starts_with("refs/pull/") && !prefixes.iter().any(|p| p.starts_with("refs/pull/"))
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
    include_tag: bool,
    done: bool,
    wait_for_done: bool,
    sideband_all: bool,
}

fn handle_v2_fetch(repo_path: &Path, args: &[String]) -> ShakoResult<Answer> {
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

    if params.deepen.is_some() && (params.deepen_since.is_some() || !params.deepen_not.is_empty()) {
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
            .map_err(git_err("find ref"))?
            .ok_or_else(|| ShakoError::BadRequest(format!("wanted ref '{rname}' not found")))?;
        let id = r
            .peel_to_id()
            .map_err(|_| ShakoError::BadRequest(format!("cannot peel '{rname}'")))?
            .detach();
        params.wants.push(id);
        resolved_wanted_refs.push((id, rname.clone()));
    }

    if params.wants.is_empty() {
        return Err(ShakoError::BadRequest(
            "fetch requires at least one want".into(),
        ));
    }

    let reachable_pool = collect_reachable_objects_for_check(&repo, &params.wants)?;
    for w in &params.wants {
        if !repo.has_object(*w) || !reachable_pool.contains(w) {
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

    let ready_to_send = params.done
        || (!params.wait_for_done && (params.haves.is_empty() || !common_haves.is_empty()));

    let mut out = Vec::new();
    let write_line = |buf: &mut Vec<u8>, line: &str, sideband_all: bool| -> ShakoResult<()> {
        if sideband_all {
            let with_lf = format!("{line}\n");
            write_sideband_chunks(
                buf,
                Channel::Data,
                with_lf.as_bytes(),
                MAX_SIDEBAND_64K_DATA,
            )
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
            return Ok(out.into());
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
    let pack = Pack {
        repo,
        objects: object_set,
        band: Some(MAX_SIDEBAND_64K_DATA),
    };
    Ok(Answer {
        head: out,
        pack: Some(pack),
    })
}

fn handle_upload_pack_v0(repo_path: &Path, lines: &[PktLine]) -> ShakoResult<Answer> {
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
    params.include_tag = caps.contains("include-tag");
    params.deepen_relative = caps.contains("deepen-relative");

    if params.wants.is_empty() {
        return Ok(Vec::new().into());
    }

    let reachable_pool = collect_reachable_objects_for_check(&repo, &params.wants)?;
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
                    return Ok(out.into());
                }
            } else {
                write_text_line(&mut out, "NAK")?;
                return Ok(out.into());
            }
        } else if multi_ack {
            for c in &common_haves {
                write_text_line(&mut out, &format!("ACK {c} continue"))?;
            }
            write_text_line(&mut out, "NAK")?;
            return Ok(out.into());
        } else {
            if let Some(first) = common_haves.first() {
                write_text_line(&mut out, &format!("ACK {first}"))?;
            } else {
                write_text_line(&mut out, "NAK")?;
            }
            return Ok(out.into());
        }
    } else if let Some(last) = common_haves.last() {
        if multi_ack || multi_ack_detailed {
            for c in &common_haves {
                let status = if multi_ack_detailed {
                    "common"
                } else {
                    "continue"
                };
                write_text_line(&mut out, &format!("ACK {c} {status}"))?;
            }
        }
        write_text_line(&mut out, &format!("ACK {last}"))?;
    } else {
        write_text_line(&mut out, "NAK")?;
    }

    let band = if caps.contains("side-band-64k") {
        Some(MAX_SIDEBAND_64K_DATA)
    } else if caps.contains("side-band") {
        Some(MAX_SIDEBAND_1K_DATA)
    } else {
        None
    };
    let pack = Pack {
        repo,
        objects: object_set,
        band,
    };
    Ok(Answer {
        head: out,
        pack: Some(pack),
    })
}

/// Computes the set of reachable objects from all references (`HEAD`, `refs/heads/*`, `refs/tags/*`, `refs/pull/*`)
/// to validate `allow-reachable-sha1-in-want` and reject unreachable object probes (S3).
fn collect_reachable_objects_for_check(
    repo: &gix::Repository,
    wants: &[ObjectId],
) -> ShakoResult<BTreeSet<ObjectId>> {
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
    // The callers only ask whether each want is in the set: a fetch of advertised tips (every
    // clone and plain fetch) needs no history walk, which only adds objects.
    if wants.iter().all(|w| reachable.contains(w)) {
        return Ok(reachable);
    }

    if !commit_tips.is_empty()
        && let Ok(walk) = repo.rev_walk(commit_tips).all()
    {
        for info in walk.flatten() {
            reachable.insert(info.id);
        }
    }

    // A partial clone fetches missing trees and blobs by id later; those count as ours when a
    // reachable commit's tree holds them (git's `allowReachableSHA1InWant`), never otherwise.
    if wants
        .iter()
        .any(|w| !reachable.contains(w) && repo.has_object(*w))
    {
        let commits: Vec<ObjectId> = reachable.iter().copied().collect();
        for c in commits {
            if let Ok(commit) = repo.find_commit(c)
                && let Ok(tree) = commit.tree()
            {
                collect_filtered_tree_objects(repo, &tree, 0, ObjectFilter::None, &mut reachable)?;
            }
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
            && c.parent_ids()
                .all(|p| included_commits.contains(&p.detach()))
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
    let decoded = tree.decode().map_err(git_err("decode tree"))?;
    for entry in decoded.entries {
        if entry.mode.is_tree() {
            let sub = repo.find_tree(entry.oid).map_err(git_err("find subtree"))?;
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

#[cfg(test)]
mod tests {
    use crate::repo::{
        CommitAuthor, FileChangeOp, FileOpKind, commit_file_changes, init_bare_repo,
    };
    use shako_core::{InstanceId, RepoId};

    /// P-6-02: the tip shortcut keeps the gate's answer: a tip and an older commit of a ref are
    /// served, a commit no ref reaches is not, though it sits in the object store.
    #[test]
    fn the_want_gate_serves_reachable_commits_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(300),
            "main",
            true,
            "Gate",
        )
        .expect("init");
        let author = CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        };
        let commit = |name: &str| {
            let sha = commit_file_changes(
                &path,
                "main",
                name,
                &author,
                &author,
                &[FileChangeOp {
                    operation: FileOpKind::Create,
                    path: name.into(),
                    content: Some(b"x\n".to_vec()),
                    sha: None,
                }],
            )
            .expect("commit");
            ObjectId::from_hex(sha.as_bytes()).expect("sha")
        };
        let older = commit("a.txt");
        let tip = commit("b.txt");
        let dangling = commit("c.txt");
        let repo = gix::open(&path).expect("open");
        repo.reference(
            "refs/heads/main",
            tip,
            gix::refs::transaction::PreviousValue::Any,
            "drop c.txt",
        )
        .expect("reset main");
        assert!(repo.has_object(dangling));

        let has = |want: ObjectId| {
            collect_reachable_objects_for_check(&repo, &[want])
                .expect("gate")
                .contains(&want)
        };
        assert!(has(tip), "the tip");
        assert!(has(older), "an older commit of main");
        assert!(!has(dangling), "a commit no ref reaches");
        let both = collect_reachable_objects_for_check(&repo, &[tip, dangling]).expect("gate");
        assert!(both.contains(&tip) && !both.contains(&dangling));
    }

    use super::*;
    use crate::repo::set_ref_direct;
    use crate::testutil::{DELIM, FLUSH, annotated_tag, pack_count, pkt, put, repo, text};
    use std::path::PathBuf;

    /// `main`: the initial commit, then `a.txt`, a 2000-byte `big.bin` (tagged `v1`) and
    /// `dir/c.txt`, one commit each; `refs/pull/1/head` on the first of them. A clone of `main`
    /// is 13 objects: 4 commits, 5 trees, 4 blobs.
    struct Fixture {
        _tmp: tempfile::TempDir,
        path: PathBuf,
        c1: ObjectId,
        c2: ObjectId,
        c3: ObjectId,
        tag: ObjectId,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = repo(tmp.path(), 1, true);
        let c1 = put(&path, "main", "a.txt", b"one\n");
        let c2 = put(&path, "main", "big.bin", &[b'x'; 2000]);
        let c3 = put(&path, "main", "dir/c.txt", b"three\n");
        let tag = annotated_tag(&path, "v1", c2, gix_object::Kind::Commit);
        set_ref_direct(&path, "refs/pull/1/head", &c1.to_string()).expect("pull ref");
        Fixture {
            _tmp: tmp,
            path,
            c1,
            c2,
            c3,
            tag,
        }
    }

    fn serve(path: &Path, protocol: Option<&str>, lines: &[&str]) -> ShakoResult<Vec<u8>> {
        let mut out = Vec::new();
        handle_upload_pack(
            path,
            protocol,
            &pkt(lines),
            &mut out,
            &mut crate::testutil::admit_all,
        )?;
        Ok(out)
    }

    fn v2(path: &Path, command: &str, args: &[&str]) -> ShakoResult<Vec<u8>> {
        let command = format!("command={command}");
        let mut lines = vec![command.as_str(), "agent=git/2.47", DELIM];
        lines.extend_from_slice(args);
        lines.push(FLUSH);
        serve(path, Some("version=2"), &lines)
    }

    fn fetch(path: &Path, args: &[&str]) -> ShakoResult<Vec<u8>> {
        v2(path, "fetch", args)
    }

    fn status(result: ShakoResult<Vec<u8>>) -> u16 {
        result.expect_err("refused").status_code()
    }

    #[test]
    fn the_advertised_refs_hide_pull_refs_and_peel_annotated_tags() {
        let f = fixture();
        let handle = open_isolated(&f.path).expect("open");
        handle
            .reference(
                "refs/notes/commits",
                f.c1,
                gix::refs::transaction::PreviousValue::Any,
                "a ref outside heads and tags",
            )
            .expect("notes ref");
        let (head, refs) = collect_advertised_refs(&handle, false).expect("refs");
        assert_eq!(head.as_deref(), Some("refs/heads/main"));
        let names: Vec<_> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["HEAD", "refs/heads/main", "refs/tags/v1"]);
        assert_eq!(refs[0].symref_target.as_deref(), Some("refs/heads/main"));
        assert_eq!((refs[2].oid, refs[2].peeled), (f.tag, Some(f.c2)));
        assert_eq!(refs[1].peeled, None);
        let (_, hidden) = collect_advertised_refs(&handle, true).expect("refs");
        assert!(hidden.iter().any(|r| r.name == "refs/pull/1/head"));
        assert!(hidden.iter().all(|r| r.name != "refs/notes/commits"));

        let tmp = tempfile::tempdir().expect("tempdir");
        let unborn = open_isolated(&repo(tmp.path(), 2, false)).expect("open");
        let (head, refs) = collect_advertised_refs(&unborn, true).expect("refs");
        assert_eq!(head.as_deref(), Some("refs/heads/main"));
        assert!(refs.is_empty());
    }

    #[test]
    fn the_v0_advertisement_names_capabilities_even_for_an_empty_repository() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let empty = text(&advertise_upload_pack(&repo(tmp.path(), 2, false), None).expect("adv"));
        assert!(
            empty.contains("0000000000000000000000000000000000000000 capabilities^{}\0symref=HEAD:refs/heads/main multi_ack"),
            "{empty}"
        );
        let f = fixture();
        let adv = text(&advertise_upload_pack(&f.path, Some("version=1")).expect("adv"));
        assert!(
            adv.contains(&format!("{} refs/tags/v1^{{}}", f.c2)),
            "{adv}"
        );
        assert!(!adv.contains("refs/pull/"), "{adv}");
        let v2 = text(&advertise_upload_pack(&f.path, Some("version=0: version=2")).expect("v2"));
        assert!(
            v2.contains("ls-refs=unborn") && v2.contains("object-info"),
            "{v2}"
        );
        assert!(advertise_upload_pack(&tmp.path().join("none"), None).is_err());
    }

    #[test]
    fn a_filter_spec_parses_each_supported_form_and_refuses_the_rest() {
        let cases = [
            ("blob:none", ObjectFilter::BlobNone),
            (" blob:limit=10 ", ObjectFilter::BlobLimit(10)),
            ("blob:limit=2k", ObjectFilter::BlobLimit(2048)),
            ("blob:limit=3M", ObjectFilter::BlobLimit(3 << 20)),
            ("blob:limit=1g", ObjectFilter::BlobLimit(1 << 30)),
            ("blob:limit=4K", ObjectFilter::BlobLimit(4096)),
            ("tree:0", ObjectFilter::TreeDepth(0)),
            ("tree:3", ObjectFilter::TreeDepth(3)),
        ];
        for (spec, want) in cases {
            assert_eq!(ObjectFilter::parse(spec).expect(spec), want);
        }
        for bad in [
            "blob:limit=",
            "blob:limit=-1",
            "blob:limit=xk",
            "blob:limit=99999999999999999999",
            "blob:limit=18446744073709551615k",
            "blob:limit=18446744073709551615m",
            "blob:limit=18446744073709551615g",
            "tree:x",
            "sparse:oid=abc",
            "",
        ] {
            let err = ObjectFilter::parse(bad).expect_err(bad);
            assert_eq!(err.status_code(), 400, "{bad}");
        }
        assert_eq!(ObjectFilter::default(), ObjectFilter::None);
    }

    #[test]
    fn a_negotiation_reads_plain_or_gzip_within_its_limit() {
        use std::io::Write as _;
        let body = b"0009done\n".repeat(3);
        let plain = crate::testutil::spooled(&body);
        let mut asked = Vec::new();
        let read = read_negotiation(&plain, false, 1 << 20, &mut |n| {
            asked.push(n);
            Ok(())
        })
        .expect("plain");
        assert_eq!(read, body);
        assert_eq!(asked[0], (1 << 20) * NEGOTIATION_BYTES_PER_BYTE);

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&body).expect("gzip");
        let zipped = crate::testutil::spooled(&gz.finish().expect("gzip"));
        assert_eq!(
            read_negotiation(&zipped, true, 1 << 20, &mut crate::testutil::admit_all)
                .expect("gzip"),
            body
        );

        let err = read_negotiation(&plain, false, 4, &mut crate::testutil::admit_all)
            .expect_err("over limit");
        assert!(
            matches!(&err, ShakoError::QuotaExceeded { resource, limit: 4 } if resource == "request_body_bytes"),
            "{err}"
        );
        let err = read_negotiation(&plain, false, 1 << 20, &mut |_| {
            Err(ShakoError::Overloaded("busy".into()))
        })
        .expect_err("not admitted");
        assert!(matches!(err, ShakoError::Overloaded(_)), "{err}");
        let err = read_negotiation(&plain, true, 1 << 20, &mut crate::testutil::admit_all)
            .expect_err("no gzip");
        assert_eq!(err.status_code(), 400, "{err}");
        let empty = crate::testutil::spooled(b"");
        assert!(
            read_negotiation(&empty, false, 0, &mut crate::testutil::admit_all)
                .expect("empty")
                .is_empty()
        );
    }

    #[test]
    fn an_empty_body_or_flushes_alone_answer_nothing() {
        let f = fixture();
        assert!(serve(&f.path, None, &[]).expect("empty").is_empty());
        assert!(
            serve(&f.path, Some("version=2"), &[FLUSH, FLUSH])
                .expect("flushes")
                .is_empty()
        );
    }

    #[test]
    fn a_v2_request_names_a_known_command_first() {
        let f = fixture();
        assert_eq!(
            status(serve(&f.path, Some("version=2"), &[FLUSH, DELIM])),
            400
        );
        let err = serve(&f.path, Some("version=2"), &["want x", FLUSH]).expect_err("no command");
        assert!(err.to_string().contains("expected command="), "{err}");
        let err = v2(&f.path, "push", &[]).expect_err("unknown");
        assert!(
            err.to_string()
                .contains("unsupported protocol v2 command 'push'"),
            "{err}"
        );
        // A leading flush, a command with no arguments, and a v2 body without the header.
        let listed = text(
            &serve(
                &f.path,
                Some("version=2"),
                &[FLUSH, "command=ls-refs", FLUSH],
            )
            .expect("ls"),
        );
        assert!(listed.contains("refs/heads/main"), "{listed}");
        let bare =
            text(&serve(&f.path, None, &["command=ls-refs", DELIM, "peel", FLUSH]).expect("ls"));
        assert!(bare.contains(&format!("peeled:{}", f.c2)), "{bare}");
        // Arguments stop at a response-end packet; a delimiter among them is skipped.
        let mut body = pkt(&["command=ls-refs", DELIM, "ref-prefix refs/tags/", DELIM]);
        body.extend_from_slice(b"0002");
        body.extend_from_slice(&pkt(&["ref-prefix refs/heads/", FLUSH]));
        let mut out = Vec::new();
        handle_upload_pack(
            &f.path,
            Some("version=2"),
            &body,
            &mut out,
            &mut crate::testutil::admit_all,
        )
        .expect("ls");
        let out = text(&out);
        assert!(
            out.contains("refs/tags/v1") && !out.contains("refs/heads/main"),
            "{out}"
        );
    }

    #[test]
    fn ls_refs_filters_by_prefix_and_adds_symrefs_peels_and_the_unborn_head() {
        let f = fixture();
        let ls = |args: &[&str]| text(&v2(&f.path, "ls-refs", args).expect("ls-refs"));
        let heads = ls(&["symrefs", "ref-prefix refs/heads/"]);
        assert_eq!(heads, format!("003d{} refs/heads/main\n0000", f.c3));
        let head = ls(&["symrefs", "ref-prefix HEAD"]);
        assert!(
            head.contains("HEAD symref-target:refs/heads/main"),
            "{head}"
        );
        let tags = ls(&["peel", "ref-prefix refs/tags/"]);
        assert!(
            tags.contains(&format!("{} refs/tags/v1 peeled:{}", f.tag, f.c2)),
            "{tags}"
        );
        assert!(!ls(&[]).contains("refs/pull/"));
        assert!(ls(&["ref-prefix refs/pull/"]).contains(&format!("{} refs/pull/1/head", f.c1)));

        let tmp = tempfile::tempdir().expect("tempdir");
        let unborn = repo(tmp.path(), 2, false);
        let ls = |args: &[&str]| text(&v2(&unborn, "ls-refs", args).expect("ls-refs"));
        assert!(ls(&["unborn", "symrefs"]).contains("unborn HEAD symref-target:refs/heads/main"));
        assert_eq!(ls(&["unborn"]), "0010unborn HEAD\n0000");
        assert_eq!(ls(&["unborn", "ref-prefix refs/heads/"]), "0000");
        assert_eq!(ls(&[]), "0000");
    }

    #[test]
    fn object_info_answers_sizes_and_refuses_bad_or_missing_ids() {
        let f = fixture();
        let oid_line = format!("oid {}", f.c1);
        let sized = text(&v2(&f.path, "object-info", &["size", &oid_line]).expect("info"));
        let size = open_isolated(&f.path)
            .expect("open")
            .find_header(f.c1)
            .expect("hdr")
            .size();
        assert!(sized.starts_with("0009size\n"), "{sized}");
        assert!(sized.contains(&format!("{} {size}\n", f.c1)), "{sized}");
        let bare = text(&v2(&f.path, "object-info", &[&oid_line]).expect("info"));
        assert_eq!(bare, format!("002d{}\n0000", f.c1));
        assert_eq!(status(v2(&f.path, "object-info", &["oid xyz"])), 400);
        let missing = "oid 1111111111111111111111111111111111111111";
        assert_eq!(status(v2(&f.path, "object-info", &[missing])), 404);
    }

    #[test]
    fn a_v2_fetch_refuses_every_malformed_or_unreachable_argument() {
        let f = fixture();
        let want = format!("want {}", f.c3);
        let tmp_orphan = put(&f.path, "orphan", "o.txt", b"o\n");
        crate::repo::delete_branch(&f.path, "orphan").expect("drop the branch");
        let orphan = format!("want {tmp_orphan}");
        let cases: [(&[&str], &str); 13] = [
            (&[&want, "packfile-uris https"], "packfile-uris is refused"),
            (&["want zz"], "invalid want oid"),
            (&[&want, "have zz"], "invalid have oid"),
            (&[&want, "shallow zz"], "invalid shallow oid"),
            (&[&want, "deepen x"], "invalid deepen depth"),
            (&[&want, "deepen-since x"], "invalid deepen-since"),
            (
                &["want-ref refs/heads/main", "want-ref refs/heads/main"],
                "duplicate want-ref",
            ),
            (&["want-ref refs/heads/none"], "not found"),
            (&["want-ref refs/heads/a..b"], "want-ref"),
            (&[&want, "deepen 1", "deepen-since 5"], "cannot be combined"),
            (
                &[&want, "deepen 1", "deepen-not main"],
                "cannot be combined",
            ),
            (&["done"], "at least one want"),
            (&[&orphan, "done"], "is not reachable"),
        ];
        for (args, says) in cases {
            let err = fetch(&f.path, args).expect_err(says);
            assert_eq!(err.status_code(), 400, "{says}: {err}");
            assert!(err.to_string().contains(says), "{says}: {err}");
        }
        assert_eq!(status(fetch(&f.path, &[&want, "filter nope"])), 400);
    }

    #[test]
    fn a_v2_fetch_negotiates_haves_then_sends_the_pack() {
        let f = fixture();
        let want = format!("want {}", f.c3);
        let have = format!("have {}", f.c1);
        let unknown = "have 1111111111111111111111111111111111111111";

        let clone = fetch(&f.path, &[&want, "done"]).expect("clone");
        let head = text(&clone);
        assert!(head.starts_with("000dpackfile\n"), "{head}");
        assert!(!head.contains("acknowledgments"));
        assert_eq!(pack_count(&clone), Some(13));

        let incremental = fetch(&f.path, &[&want, &have]).expect("with a common have");
        let head = text(&incremental);
        assert!(
            head.starts_with(&format!(
                "0014acknowledgments\n0031ACK {}\n000aready\n0001000dpackfile\n",
                f.c1
            )),
            "{head}"
        );
        assert_eq!(
            pack_count(&incremental),
            Some(9),
            "c2 and c3 with their trees and blobs"
        );

        let lost = fetch(&f.path, &[&want, unknown]).expect("no common have");
        assert_eq!(text(&lost), "0014acknowledgments\n0008NAK\n0000");
        let waiting = fetch(&f.path, &[&want, &have, "wait-for-done"]).expect("waits");
        assert!(text(&waiting).ends_with(&format!("ACK {}\n0000", f.c1)));
        assert_eq!(pack_count(&waiting), None);
        // No haves at all: ready at once.
        let fresh = fetch(&f.path, &[&want]).expect("no haves");
        assert!(text(&fresh).contains("NAK\n000aready\n"));
        assert_eq!(pack_count(&fresh), Some(13));

        let banded = fetch(&f.path, &[&want, &have, "sideband-all"]).expect("sideband-all");
        let head = text(&banded);
        assert!(head.starts_with("0015\u{1}acknowledgments\n"), "{head}");
        assert!(head.contains("\u{1}packfile\n"), "{head}");
    }

    #[test]
    fn a_v2_fetch_cuts_history_by_depth_time_or_ref_and_reports_it() {
        let f = fixture();
        let want = format!("want {}", f.c3);
        let shallow = fetch(&f.path, &[&want, "deepen 1", "done"]).expect("deepen");
        let head = text(&shallow);
        assert!(
            head.contains(&format!("shallow-info\n0035shallow {}\n0001", f.c3)),
            "{head}"
        );
        assert_eq!(pack_count(&shallow), Some(7));

        let client = format!("shallow {}", f.c3);
        let deeper = fetch(
            &f.path,
            &[&want, &client, "deepen 3", "deepen-relative", "done"],
        )
        .expect("deepen further");
        let head = text(&deeper);
        assert!(head.contains(&format!("unshallow {}", f.c3)), "{head}");
        assert!(head.contains(&format!("shallow {}", f.c1)), "{head}");

        let not = fetch(
            &f.path,
            &[&want, "deepen-not main~2", "deepen-not nope", "done"],
        )
        .expect("deepen-not");
        assert_eq!(pack_count(&not), Some(9));
        let since = fetch(&f.path, &[&want, "deepen-since 4102444800", "done"]).expect("since");
        assert_eq!(pack_count(&since), Some(0), "every commit is older");
        let all = fetch(&f.path, &[&want, "deepen-since 0", "done"]).expect("since");
        assert_eq!(pack_count(&all), Some(13));

        let by_ref = fetch(&f.path, &["want-ref refs/heads/main", "done"]).expect("want-ref");
        let head = text(&by_ref);
        assert!(
            head.contains(&format!("wanted-refs\n003d{} refs/heads/main\n0001", f.c3)),
            "{head}"
        );
        assert_eq!(pack_count(&by_ref), Some(13));
    }

    #[test]
    fn a_v2_fetch_filters_objects_and_serves_tags_trees_and_blobs_asked_by_id() {
        let f = fixture();
        let want = format!("want {}", f.c3);
        let count = |args: &[&str]| pack_count(&fetch(&f.path, args).expect("fetch"));
        assert_eq!(count(&[&want, "filter blob:none", "done"]), Some(9));
        assert_eq!(count(&[&want, "filter blob:limit=100", "done"]), Some(12));
        assert_eq!(count(&[&want, "filter tree:0", "done"]), Some(4));
        assert_eq!(count(&[&want, "filter tree:1", "done"]), Some(11));
        assert_eq!(count(&[&want, "include-tag", "done"]), Some(14));
        let c1 = format!("want {}", f.c1);
        assert_eq!(
            count(&[&c1, "include-tag", "done"]),
            Some(6),
            "v1 is not on c1's history"
        );
        let tag = format!("want {}", f.tag);
        assert_eq!(count(&[&tag, "done"]), Some(10), "the tag and c2's history");

        let handle = open_isolated(&f.path).expect("open");
        let tree = handle
            .find_commit(f.c1)
            .expect("c1")
            .tree_id()
            .expect("tree")
            .detach();
        let tree_want = format!("want {tree}");
        assert_eq!(
            count(&[&tree_want, "done"]),
            Some(3),
            "the tree, README.md and a.txt"
        );
        let blob = gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, b"one\n")
            .expect("hash");
        let blob_want = format!("want {blob}");
        assert_eq!(count(&[&blob_want, "filter blob:none", "done"]), Some(1));
    }

    #[test]
    fn a_v0_fetch_refuses_bad_lines_and_both_sideband_sizes() {
        let f = fixture();
        let v0 = |lines: &[&str]| serve(&f.path, None, lines);
        let want = format!("want {}", f.c3);
        let both = format!("want {} side-band side-band-64k", f.c3);
        let dropped = format!("want {}", put(&f.path, "x", "x", b"x"));
        crate::repo::delete_branch(&f.path, "x").expect("unreachable once dropped");
        let cases: [&[&str]; 9] = [
            &[&both, FLUSH, "done"],
            &["want zz", FLUSH],
            &[&want, "shallow zz", FLUSH],
            &[&want, "deepen x", FLUSH],
            &[&want, "deepen-since x", FLUSH],
            &[&want, "filter nope", FLUSH],
            &[&want, FLUSH, "have zz", "done"],
            &[
                "want 1111111111111111111111111111111111111111",
                FLUSH,
                "done",
            ],
            &[&dropped, FLUSH, "done"],
        ];
        for lines in cases {
            assert_eq!(status(v0(lines)), 400, "{lines:?}");
        }
        assert!(v0(&[FLUSH, "done"]).expect("no wants").is_empty());
    }

    #[test]
    fn a_v0_fetch_acknowledges_as_each_multi_ack_mode_says() {
        let f = fixture();
        let v0 = |caps: &str, rest: &[&str]| {
            let want = format!("want {} {caps}", f.c3);
            let mut lines = vec![want.as_str(), FLUSH];
            lines.extend_from_slice(rest);
            serve(&f.path, None, &lines).expect("v0")
        };
        let have = format!("have {}", f.c1);
        let unknown = "have 1111111111111111111111111111111111111111";
        let c1 = f.c1;

        let out = v0("multi_ack_detailed no-done side-band-64k", &[&have, FLUSH]);
        let head = text(&out);
        assert!(
            head.starts_with(&format!(
                "0038ACK {c1} common\n0037ACK {c1} ready\n0008NAK\n0031ACK {c1}\n"
            )),
            "{head}"
        );
        assert_eq!(pack_count(&out), Some(9));
        let out = v0("multi_ack_detailed", &[&have, FLUSH]);
        assert_eq!(
            text(&out),
            format!("0038ACK {c1} common\n0037ACK {c1} ready\n0008NAK\n")
        );
        assert_eq!(
            text(&v0("multi_ack_detailed", &[unknown, FLUSH])),
            "0008NAK\n"
        );
        assert_eq!(
            text(&v0("multi_ack", &[&have, FLUSH])),
            format!("003aACK {c1} continue\n0008NAK\n")
        );
        assert_eq!(
            text(&v0("ofs-delta", &[&have, FLUSH])),
            format!("0031ACK {c1}\n")
        );
        assert_eq!(text(&v0("ofs-delta", &[unknown, FLUSH])), "0008NAK\n");

        let out = v0("multi_ack", &[&have, "done"]);
        assert!(text(&out).starts_with(&format!("003aACK {c1} continue\n0031ACK {c1}\nPACK")));
        let out = v0("multi_ack_detailed", &[&have, "done"]);
        assert!(text(&out).starts_with(&format!("0038ACK {c1} common\n0031ACK {c1}\nPACK")));
        let out = v0("side-band", &["done"]);
        assert!(text(&out).starts_with("0008NAK\n"));
        assert_eq!(pack_count(&out), Some(13));
        let nul = format!("want {}\0side-band-64k", f.c3);
        let out = serve(&f.path, None, &[&nul, FLUSH, "done"]).expect("caps after NUL");
        assert_eq!(pack_count(&out), Some(13));
        assert!(text(&out).contains("\u{1}PACK"), "banded");
    }

    #[test]
    fn a_v0_fetch_reports_shallow_boundaries_before_acknowledging() {
        let f = fixture();
        let want = format!("want {}", f.c3);
        let out = serve(&f.path, None, &[&want, "deepen 1", FLUSH, "done"]).expect("deepen");
        assert!(text(&out).starts_with(&format!("0035shallow {}\n00000008NAK\n", f.c3)));
        assert_eq!(pack_count(&out), Some(7));
        let client = format!("shallow {}", f.c3);
        let out = serve(
            &f.path,
            None,
            &[&want, &client, "deepen-since 0", FLUSH, "done"],
        )
        .expect("since");
        assert!(text(&out).starts_with(&format!("0037unshallow {}\n0000", f.c3)));
        let out = serve(&f.path, None, &[&want, "deepen-not main~1", FLUSH, "done"]).expect("not");
        assert!(text(&out).starts_with("00000008NAK\n"));
        assert_eq!(
            pack_count(&out),
            Some(7),
            "c3 alone with its two trees and four blobs"
        );
    }
}
