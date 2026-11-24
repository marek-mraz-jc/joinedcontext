#![forbid(unsafe_code)]
//! Git Smart HTTP `receive-pack` server with quarantine (`objects/incoming-<uuid>/`),
//! thin-pack completion, object size and delta-depth limits, connectivity check,
//! secret scanning, ref policy evaluation, and `report-status` / `report-status-v2` (Contract S4, S5, T5, T7, T9).

use crate::pack::{Admit, BYTES_PER_OBJECT};
use crate::pktline::{
    MAX_SIDEBAND_1K_DATA, MAX_SIDEBAND_64K_DATA, PktLine, read_pkt_line_from, write_flush,
    write_sideband_chunks, write_text_line,
};
use crate::repo::{collect_changed_blobs, collect_tree_blobs, is_ancestor, open_isolated};
use gix_hash::ObjectId;
use gix_object::tree::EntryKind;
use gix_pack::data::input::{BytesToEntriesIter, EntryDataMode, Mode as InputMode};
use gix_packetline::Channel;
use gix_ref::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};
use regex::Regex;
use shako_core::{
    Authorizer, MAX_DELTA_CHAIN_DEPTH, MAX_REFS_PER_PUSH, MAX_SINGLE_OBJECT_BYTES, RefPolicy,
    RefUpdate, RequestCtx, ShakoError, ShakoResult, validate_full_ref_name, validate_tree_path,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, atomic::AtomicBool};

/// The commands in front of a pushed pack: a thousand refs with long names fit.
pub const MAX_COMMAND_BYTES: usize = 1024 * 1024;

pub const RECEIVE_PACK_CAPABILITIES: &str = "report-status report-status-v2 delete-refs side-band-64k quiet atomic ofs-delta object-format=sha1 agent=shako/0.1.0";

/// Generates the `GET {repo}.git/info/refs?service=git-receive-pack` advertisement (S4).
pub fn advertise_receive_pack(repo_path: &Path) -> ShakoResult<Vec<u8>> {
    let repo = open_isolated(repo_path)?;
    let (_, refs) = crate::upload_pack::collect_advertised_refs(&repo, false)?;
    // receive-pack advertises `refs/heads/*` and `refs/tags/*` (not `HEAD`)
    let non_head: Vec<_> = refs.into_iter().filter(|r| r.name != "HEAD").collect();

    let mut out = Vec::new();
    write_text_line(&mut out, "# service=git-receive-pack")?;
    write_flush(&mut out)?;

    if non_head.is_empty() {
        let zero = ObjectId::null(gix_hash::Kind::Sha1);
        write_text_line(
            &mut out,
            &format!("{zero} capabilities^{{}}\0{RECEIVE_PACK_CAPABILITIES}"),
        )?;
    } else {
        for (idx, r) in non_head.iter().enumerate() {
            if idx == 0 {
                write_text_line(
                    &mut out,
                    &format!("{} {}\0{RECEIVE_PACK_CAPABILITIES}", r.oid, r.name),
                )?;
            } else {
                write_text_line(&mut out, &format!("{} {}", r.oid, r.name))?;
            }
        }
    }
    write_flush(&mut out)?;
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdateCommand {
    pub old_oid: ObjectId,
    pub new_oid: ObjectId,
    pub refname: String,
}

impl RefUpdateCommand {
    #[must_use]
    pub fn is_create(&self) -> bool {
        self.old_oid.is_null()
    }

    #[must_use]
    pub fn is_delete(&self) -> bool {
        self.new_oid.is_null()
    }
}

#[derive(Debug, Clone)]
pub struct ReceivePackOutcome {
    pub response_bytes: Vec<u8>,
    pub updated_refs: Vec<RefUpdateCommand>,
    pub pack_size_bytes: u64,
}

/// RAII guard for `{repo}.git/objects/incoming-<uuid>/` quarantine directory:
/// automatically removes the quarantine directory on drop unless disarmed after the pack is moved (S4).
struct QuarantineDir {
    path: PathBuf,
    disarmed: bool,
}

impl QuarantineDir {
    fn create(repo_path: &Path) -> ShakoResult<Self> {
        let id = uuid::Uuid::now_v7();
        let path = repo_path.join("objects").join(format!("incoming-{id}"));
        std::fs::create_dir_all(&path)
            .map_err(|e| ShakoError::Storage(format!("failed to create quarantine dir: {e}")))?;
        Ok(Self {
            path,
            disarmed: false,
        })
    }

    fn disarm(&mut self) {
        self.disarmed = true;
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Drop for QuarantineDir {
    fn drop(&mut self) {
        if !self.disarmed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

static SECRET_SCAN_PATTERNS: LazyLock<Vec<(&'static str, regex::bytes::Regex)>> =
    LazyLock::new(|| {
        use regex::bytes::Regex;
        vec![
            (
                "private-key",
                Regex::new(r"-----BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----")
                    .expect("static regex"),
            ),
            (
                "aws-access-key",
                Regex::new(r"AKIA[0-9A-Z]{16}").expect("static regex"),
            ),
            (
                "github-token",
                Regex::new(r"ghp_[A-Za-z0-9]{36}").expect("static regex"),
            ),
        ]
    });

/// The field names a manifest writes a credential under: the Portal's `SECRET_KEYS`
/// (`crates/portal/src/api/mutate.rs`, MF-24), which refuses the same names at every Portal door.
const SECRET_FIELDS: &[&str] = &[
    "password",
    "token",
    "secret",
    "clientSecret",
    "apiKey",
    "client_secret",
    "api_key",
    "apiToken",
    "api_token",
    "access_token",
    "accessToken",
    "access_token_secret",
    "accessTokenSecret",
    "consumer_secret",
    "consumerSecret",
    "auth_secret",
    "authSecret",
    "account_key",
    "accountKey",
    "bot_token",
    "botToken",
    "private_key",
    "privateKey",
    "private_key_pass",
    "privateKeyPass",
    "passphrase",
    "user_jwt",
    "userJwt",
    "user_nkey_seed",
    "userNkeySeed",
];

static YAML_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:-\s+)?["']?([A-Za-z_]+)["']?\s*:\s*(.*)$"#).expect("static regex")
});

/// Whether a secret field's string value holds no credential: empty, a whole `${VAR}`
/// reference, a whole `${{ secrets.X }}` workflow expression, or sops ciphertext
/// (`ENC[AES256_GCM,...]`).
fn is_open_secret_value(value: &str) -> bool {
    let value = value.trim();
    let reference = value
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    let expression = value.starts_with("${{") && value.ends_with("}}");
    value.is_empty() || reference || expression || value.starts_with("ENC[")
}

/// The first secret field a parsed YAML value writes out as a literal string, with that string:
/// a mapping key from [`SECRET_FIELDS`] whose value is a scalar string, the shape the Portal's
/// `find_literal_secret` judges. Text inside a block scalar is a value, never a key.
fn find_secret_field(value: &serde_norway::Value) -> Option<(&str, &str)> {
    use serde_norway::Value;
    match value {
        Value::Mapping(map) => map.iter().find_map(|(key, val)| match (key, val) {
            (Value::String(k), Value::String(v))
                if SECRET_FIELDS.contains(&k.as_str()) && !is_open_secret_value(v) =>
            {
                Some((k.as_str(), v.as_str()))
            }
            _ => find_secret_field(val),
        }),
        Value::Sequence(items) => items.iter().find_map(find_secret_field),
        Value::Tagged(tagged) => find_secret_field(&tagged.value),
        _ => None,
    }
}

/// The literal secret field of a YAML manifest (`password: hunter2`) as (field, 1-based line),
/// or `None`. Each document is parsed and judged when it is a manifest (top-level `apiVersion`
/// and `kind`); Helm values (`secret: <Secret name>`), workflows and a file that is no YAML are
/// left to the shape patterns. The line is the first `field: value` line holding that value,
/// `None` for a multi-line one.
fn literal_secret_field(text: &str) -> Option<(String, Option<usize>)> {
    use serde::Deserialize;
    for doc in serde_norway::Deserializer::from_str(text) {
        let Ok(value) = serde_norway::Value::deserialize(doc) else {
            return None;
        };
        let manifest = ["apiVersion", "kind"]
            .iter()
            .all(|key| value.get(key).is_some_and(serde_norway::Value::is_string));
        if let Some((field, literal)) = find_secret_field(&value).filter(|_| manifest) {
            let line = text.lines().position(|line| {
                YAML_FIELD.captures(line).is_some_and(|caps| {
                    caps.get(1).is_some_and(|f| f.as_str() == field)
                        && caps.get(2).is_some_and(|v| v.as_str().contains(literal))
                })
            });
            return Some((field.to_owned(), line.map(|n| n + 1)));
        }
    }
    None
}

/// Scans blob bytes for leaked secrets before a push lands (Threat T9, SEC-06): the credential
/// shapes of [`SECRET_SCAN_PATTERNS`] in any file, and a literal under a secret field in a YAML
/// manifest. The refusal names the path, the line and the rule, never the value.
pub fn scan_blob_for_secrets(path: &str, bytes: &[u8]) -> Option<String> {
    for (rule_name, regex) in SECRET_SCAN_PATTERNS.iter() {
        if let Some(found) = regex.find(bytes) {
            let line = bytes[..found.start()]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
                + 1;
            return Some(format!(
                "{SECRET_REFUSAL} {rule_name} in '{path}' line {line}"
            ));
        }
    }
    if (path.ends_with(".yaml") || path.ends_with(".yml"))
        && let Some((field, line)) = literal_secret_field(&String::from_utf8_lossy(bytes))
    {
        let at = line.map(|n| format!(" line {n}")).unwrap_or_default();
        return Some(format!(
            "{SECRET_REFUSAL} literal-secret-field in '{path}'{at} \
             (field '{field}'; name the credential with secretRef)"
        ));
    }
    None
}

/// Who pushes into which repository, and the rules every ref command of the push must pass.
pub struct PushGuard<'a> {
    pub ctx: &'a RequestCtx,
    pub authorizer: &'a dyn Authorizer,
    pub owner: &'a str,
    pub repo_name: &'a str,
    pub policy: &'a RefPolicy,
}

/// Processes a `POST {repo}.git/git-receive-pack` request held in `body` (S4, S5, T5, T7, T9);
/// `admit` is asked after the pack's scan, before anything inflates it.
pub fn handle_receive_pack(
    repo_path: &Path,
    guard: &PushGuard<'_>,
    max_push_bytes: u64,
    mut body: &File,
    admit: Admit<'_>,
) -> ShakoResult<ReceivePackOutcome> {
    // 1. Parse command list up to Flush (0000), leaving `body` at the start of the raw PACK stream
    let unreadable = |e: std::io::Error| ShakoError::Storage(format!("read push: {e}"));
    body.seek(SeekFrom::Start(0)).map_err(unreadable)?;
    let mut read = 0usize;
    let mut commands = Vec::new();
    let mut caps = BTreeSet::new();

    loop {
        let Some((line, used)) = read_pkt_line_from(body)? else {
            return Err(ShakoError::BadRequest(
                "receive-pack missing flush packet after commands".into(),
            ));
        };
        read += used;
        if read > MAX_COMMAND_BYTES {
            return Err(ShakoError::BadRequest(format!(
                "receive-pack commands exceed {MAX_COMMAND_BYTES} bytes"
            )));
        }
        match line {
            PktLine::Flush => break,
            PktLine::Data(raw) => {
                let text = std::str::from_utf8(&raw)
                    .map_err(|_| ShakoError::BadRequest("non-utf8 command line".into()))?;
                let text = text.trim_end_matches(['\r', '\n']);
                if text.starts_with("shallow ") {
                    continue;
                }
                let (cmd_part, cap_part) = match text.split_once('\0') {
                    Some((c, cp)) => (c, Some(cp)),
                    None => (text, None),
                };
                if let Some(cp) = cap_part {
                    for c in cp.split_whitespace() {
                        caps.insert(c.to_owned());
                    }
                }
                let mut parts = cmd_part.split_whitespace();
                let (Some(old_s), Some(new_s), Some(ref_s)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Err(ShakoError::BadRequest(format!(
                        "malformed receive-pack command '{cmd_part}'"
                    )));
                };
                let old_oid = ObjectId::from_hex(old_s.as_bytes())
                    .map_err(|_| ShakoError::BadRequest("invalid old_id".into()))?;
                let new_oid = ObjectId::from_hex(new_s.as_bytes())
                    .map_err(|_| ShakoError::BadRequest("invalid new_id".into()))?;
                commands.push(RefUpdateCommand {
                    old_oid,
                    new_oid,
                    refname: ref_s.to_owned(),
                });
                if commands.len() > MAX_REFS_PER_PUSH {
                    return format_unpack_error(
                        &format!("too many refs in push (limit {MAX_REFS_PER_PUSH})"),
                        &commands,
                        &caps,
                    );
                }
            }
            _ => {}
        }
    }

    let body_len = body.metadata().map_err(unreadable)?.len();
    if commands.is_empty() {
        // git asks with a lone flush packet before it sends a push over http.postBuffer.
        if read == 4 && body_len == 4 {
            return Ok(ReceivePackOutcome {
                response_bytes: Vec::new(),
                updated_refs: Vec::new(),
                pack_size_bytes: 0,
            });
        }
        return Err(ShakoError::BadRequest(
            "receive-pack requires at least one command".into(),
        ));
    }

    let atomic = caps.contains("atomic");
    let pack_start = read as u64;
    let pack_len = body_len.saturating_sub(pack_start);

    // 2. Enforce streaming push byte limit / quota BEFORE storing anything (S4, S6, Threat T5)
    if pack_len > max_push_bytes {
        return format_unpack_error("quota", &commands, &caps);
    }

    let has_non_delete = commands.iter().any(|c| !c.is_delete());
    let mut quarantine = QuarantineDir::create(repo_path)?;
    let mut written_pack_path: Option<PathBuf> = None;
    let mut written_idx_path: Option<PathBuf> = None;

    // 3. Unpack and validate incoming packfile in quarantine (`objects/incoming-<uuid>/`)
    if has_non_delete || pack_len > 0 {
        let mut magic = Vec::new();
        body.take(4).read_to_end(&mut magic).map_err(unreadable)?;
        if magic != b"PACK" {
            return format_unpack_error("missing PACK header", &commands, &caps);
        }
        body.seek(SeekFrom::Start(pack_start)).map_err(unreadable)?;
        let (held, count) = match scan_pack(body, pack_start) {
            Ok(scanned) => scanned,
            Err(reason) => return format_unpack_error(&reason, &commands, &caps),
        };
        admit(held.saturating_add(count.saturating_mul(BYTES_PER_OBJECT)))?;
        body.seek(SeekFrom::Start(pack_start)).map_err(unreadable)?;
        if let Err(reason) = index_pack_in_quarantine(
            repo_path,
            &quarantine.path,
            body,
            &mut written_pack_path,
            &mut written_idx_path,
        ) {
            return format_unpack_error(&reason, &commands, &caps);
        }
    }

    // 4. Move indexed pack into `objects/pack/` temporarily with a rollback list so `gix::Repository`
    //    can inspect trees, fast-forward ancestry, and connectivity of new tips; if all refs are
    //    rejected (or atomic fails), we delete the moved pack before returning! A pack is named by
    //    its content, so a file already stored under that name belongs to an earlier push: it is
    //    neither written over nor put on the rollback list.
    let mut installed_files = Vec::new();
    if let (Some(p_src), Some(i_src)) = (&written_pack_path, &written_idx_path) {
        let pack_dir = repo_path.join("objects/pack");
        std::fs::create_dir_all(&pack_dir)
            .map_err(|e| ShakoError::Storage(format!("mkdir pack: {e}")))?;
        for (what, src) in [("pack", p_src), ("idx", i_src)] {
            let dst = pack_dir.join(src.file_name().unwrap_or_default());
            let stored = dst
                .try_exists()
                .map_err(|e| ShakoError::Storage(format!("look up {what}: {e}")))?;
            if stored {
                continue;
            }
            std::fs::rename(src, &dst)
                .map_err(|e| ShakoError::Storage(format!("move {what}: {e}")))?;
            installed_files.push(dst);
        }
    }
    quarantine.disarm();

    let repo = open_isolated(repo_path)?;

    // 5. Evaluate each command: ref validation, old_oid CAS check, connectivity check,
    //    fast-forward check, tree path validation (T1), secret scan (T9), and `RefPolicy` (S5)
    let mut cmd_results: Vec<(RefUpdateCommand, Result<(), String>)> =
        Vec::with_capacity(commands.len());

    for cmd in &commands {
        let res = evaluate_single_command(&repo, guard, cmd);
        cmd_results.push((cmd.clone(), res));
    }

    // A credential in any command refuses the whole push like a failed `atomic` one, so its pack
    // is removed below instead of staying beside the other refs (Contract S4).
    let leaked = cmd_results
        .iter()
        .any(|(_, r)| r.as_ref().is_err_and(|e| e.starts_with(SECRET_REFUSAL)));
    // If `atomic` capability was requested and any command failed, reject all commands
    if (atomic || leaked) && cmd_results.iter().any(|(_, r)| r.is_err()) {
        for (_, r) in &mut cmd_results {
            if r.is_ok() {
                *r = Err(if leaked {
                    "push refused whole: another ref carries a credential".into()
                } else {
                    "atomic push failed".into()
                });
            }
        }
    }

    // 6. Apply accepted ref updates in `gix-ref` transaction
    let mut final_results = Vec::with_capacity(cmd_results.len());
    let mut updated_refs = Vec::new();

    if atomic {
        if cmd_results.iter().all(|(_, r)| r.is_ok()) {
            let edits: Vec<RefEdit> = cmd_results
                .iter()
                .filter_map(|(c, _)| build_ref_edit(c).ok())
                .collect();
            match repo
                .refs
                .transaction()
                .prepare(
                    edits,
                    gix::lock::acquire::Fail::Immediately,
                    gix::lock::acquire::Fail::Immediately,
                )
                .and_then(|tx| tx.commit(None))
            {
                Ok(_) => {
                    for (c, _) in cmd_results {
                        updated_refs.push(c.clone());
                        final_results.push((c, Ok(())));
                    }
                }
                Err(_) => {
                    for (c, _) in cmd_results {
                        final_results
                            .push((c, Err("failed to lock/update ref (fetch first)".into())));
                    }
                }
            }
        } else {
            final_results = cmd_results;
        }
    } else {
        for (c, pre_res) in cmd_results {
            match pre_res {
                Err(reason) => final_results.push((c, Err(reason))),
                Ok(()) => {
                    let edit = match build_ref_edit(&c) {
                        Ok(e) => e,
                        Err(reason) => {
                            final_results.push((c, Err(reason)));
                            continue;
                        }
                    };
                    match repo
                        .refs
                        .transaction()
                        .prepare(
                            [edit],
                            gix::lock::acquire::Fail::Immediately,
                            gix::lock::acquire::Fail::Immediately,
                        )
                        .and_then(|tx| tx.commit(None))
                    {
                        Ok(_) => {
                            updated_refs.push(c.clone());
                            final_results.push((c, Ok(())));
                        }
                        Err(_) => {
                            final_results.push((c, Err("fetch first".into())));
                        }
                    }
                }
            }
        }
    }

    // If no ref was updated at all, remove the newly installed packfiles so refused pushes
    // leave zero objects behind (Contract S4: "refused pushes leave no object and no ref behind")!
    let pack_size_bytes = if updated_refs.is_empty() {
        for f in installed_files {
            let _ = std::fs::remove_file(f);
        }
        0
    } else {
        pack_len
    };

    let response_bytes = format_report_status("ok", &final_results, &caps)?;
    Ok(ReceivePackOutcome {
        response_bytes,
        updated_refs,
        pack_size_bytes,
    })
}

/// Streaming pre-scan of pack entries with `BytesToEntriesIter`, enforcing an object size
/// (a delta's result included) `<= MAX_SINGLE_OBJECT_BYTES` (100 MiB) and
/// `delta_chain_depth <= MAX_DELTA_CHAIN_DEPTH` (50) (Threat T5). Answers what indexing holds at
/// its largest and the number of entries. `pack_start` is the pack's offset in `pack`.
fn scan_pack(pack: &File, pack_start: u64) -> Result<(u64, u64), String> {
    let entries_iter = BytesToEntriesIter::new_from_header(
        BufReader::new(pack),
        InputMode::Verify,
        EntryDataMode::Ignore,
        gix_hash::Kind::Sha1,
    )
    .map_err(|e| format!("invalid pack header: {e}"))?;

    let too_large =
        |size: u64| format!("object size {size} exceeds limit {MAX_SINGLE_OBJECT_BYTES}");
    let mut depth_by_offset: BTreeMap<u64, usize> = BTreeMap::new();
    let mut held = 0;
    for entry_res in entries_iter {
        let entry = entry_res.map_err(|e| format!("corrupt pack entry: {e}"))?;
        if entry.decompressed_size > MAX_SINGLE_OBJECT_BYTES {
            return Err(too_large(entry.decompressed_size));
        }
        let depth = match entry.header {
            gix_pack::data::entry::Header::OfsDelta { base_distance } => {
                let base_off = entry.pack_offset.saturating_sub(base_distance);
                depth_by_offset.get(&base_off).copied().unwrap_or(1) + 1
            }
            gix_pack::data::entry::Header::RefDelta { .. } => 1,
            _ => 0,
        };
        if depth > MAX_DELTA_CHAIN_DEPTH {
            return Err(format!(
                "delta chain depth {depth} exceeds limit {MAX_DELTA_CHAIN_DEPTH}"
            ));
        }
        depth_by_offset.insert(entry.pack_offset, depth);
        // A whole object is held inflated and compressed. A delta's size is the delta's: its
        // base, its result and the result's compressed form (a thin pack's base is appended)
        // are held while it is resolved, measured at 3 x 80 MiB for one appended line.
        let cost = if depth == 0 {
            entry.decompressed_size.saturating_mul(2)
        } else {
            let at = pack_start + entry.pack_offset + u64::from(entry.header_size);
            let (base, result) = delta_sizes(pack, at, entry.compressed_size)?;
            if base.max(result) > MAX_SINGLE_OBJECT_BYTES {
                return Err(too_large(base.max(result)));
            }
            base.saturating_add(result.saturating_mul(2))
                .saturating_add(entry.decompressed_size)
        };
        held = held.max(cost);
    }
    Ok((held, depth_by_offset.len() as u64))
}

/// The base and result sizes a delta's data starts with (two little-endian base-128 numbers),
/// read from its zlib stream of `len` bytes at `at` in `pack` without moving the file's position.
/// A dynamic Huffman block's tables come before its first byte, so up to 4 KiB are read.
fn delta_sizes(pack: &File, at: u64, len: u64) -> Result<(u64, u64), String> {
    use std::os::unix::fs::FileExt as _;
    let unreadable = |e: &dyn std::fmt::Display| format!("unreadable delta at {at}: {e}");
    let mut compressed = vec![0u8; usize::try_from(len.min(4096)).unwrap_or(4096)];
    pack.read_exact_at(&mut compressed, at)
        .map_err(|e| unreadable(&e))?;
    let mut header = [0u8; 20];
    let mut inflate = flate2::Decompress::new(true);
    inflate
        .decompress(&compressed, &mut header, flate2::FlushDecompress::Sync)
        .map_err(|e| unreadable(&e))?;
    let header = &header[..usize::try_from(inflate.total_out()).unwrap_or(0)];
    let mut sizes = header.iter().copied();
    let mut size = || {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = sizes.next()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    };
    match (size(), size()) {
        (Some(base), Some(result)) => Ok((base, result)),
        _ => Err(unreadable(&"truncated delta header")),
    }
}

/// Indexes and thickens a thin pack in `quarantine_dir` using `Bundle::write_to_directory`.
fn index_pack_in_quarantine(
    repo_path: &Path,
    quarantine_dir: &Path,
    pack: &File,
    out_pack: &mut Option<PathBuf>,
    out_idx: &mut Option<PathBuf>,
) -> Result<(), String> {
    let repo = open_isolated(repo_path).map_err(|e| e.to_string())?;
    let odb = repo.objects.clone();
    let mut pack_reader = BufReader::new(pack);
    let mut progress = gix::progress::Discard;
    let should_interrupt = AtomicBool::new(false);

    let outcome = gix_pack::Bundle::write_to_directory(
        &mut pack_reader,
        Some(quarantine_dir),
        &mut progress,
        &should_interrupt,
        Some(odb),
        gix_hash::Kind::Sha1,
        gix_pack::bundle::write::Options {
            thread_limit: Some(1),
            iteration_mode: InputMode::Verify,
            index_version: gix_pack::index::Version::V2,
            alloc_limit_bytes: Some(MAX_SINGLE_OBJECT_BYTES as usize),
            compression: gix::zlib::Compression::default(),
        },
    )
    .map_err(|e| format!("index-pack failed: {e}"))?;

    if let Some(keep) = outcome.keep_path {
        let _ = std::fs::remove_file(keep);
    }
    *out_pack = outcome.data_path;
    *out_idx = outcome.index_path;
    Ok(())
}

fn evaluate_single_command(
    repo: &gix::Repository,
    guard: &PushGuard<'_>,
    cmd: &RefUpdateCommand,
) -> Result<(), String> {
    let PushGuard {
        ctx,
        authorizer,
        owner,
        repo_name,
        policy,
    } = *guard;
    if cmd.refname == "HEAD"
        || !(cmd.refname.starts_with("refs/heads/") || cmd.refname.starts_with("refs/tags/"))
    {
        return Err("refusing to update non-branch/tag ref".into());
    }
    validate_full_ref_name(&cmd.refname).map_err(|e| e.to_string())?;

    // Check current ref value for CAS (`fetch first` on race)
    let current_oid = repo
        .refs
        .try_find(cmd.refname.as_str())
        .map_err(|e| e.to_string())?
        .and_then(|r| r.target.try_id().map(ToOwned::to_owned));

    match (cmd.is_create(), current_oid) {
        (true, Some(_)) => return Err("fetch first".into()),
        (false, None) => return Err("fetch first".into()),
        (false, Some(cur)) if cur != cmd.old_oid => return Err("fetch first".into()),
        _ => {}
    }

    let mut is_ff = true;
    let mut changed_paths = Vec::new();

    if !cmd.is_delete() {
        // Connectivity and tree/blob verification of `cmd.new_oid`
        verify_tip_connectivity_and_tree(repo, current_oid, cmd.new_oid, &mut changed_paths)?;
        scan_new_commits(repo, cmd.new_oid)?;
        if let Some(old) = current_oid {
            is_ff = is_ancestor(repo, old, cmd.new_oid);
        }
    }

    policy
        .check_ref_update(
            ctx,
            authorizer,
            owner,
            repo_name,
            RefUpdate {
                refname: &cmd.refname,
                is_delete: cmd.is_delete(),
                is_create: cmd.is_create(),
                is_fast_forward: is_ff,
                changed_paths: &changed_paths,
            },
        )
        .map_err(|e| e.public_detail())?;

    Ok(())
}

/// Verifies that `new_oid` exists, peels to a valid commit and tree whose every referenced
/// subtree and blob exists in the object database and validates every path (T1).
fn verify_tip_connectivity_and_tree(
    repo: &gix::Repository,
    old_oid: Option<ObjectId>,
    new_oid: ObjectId,
    changed_paths: &mut Vec<String>,
) -> Result<(), String> {
    let obj = repo
        .find_object(new_oid)
        .map_err(|_| "missing necessary objects".to_owned())?;
    let commit_obj = obj
        .peel_to_kind(gix_object::Kind::Commit)
        .map_err(|_| "missing necessary objects".to_owned())?;
    let commit = repo
        .find_commit(commit_obj.id)
        .map_err(|_| "missing necessary objects".to_owned())?;
    let tree = commit
        .tree()
        .map_err(|_| "missing necessary objects".to_owned())?;

    let mut new_blobs = BTreeMap::new();
    collect_tree_blobs(repo, &tree, "", &mut new_blobs).map_err(|e| match e {
        ShakoError::Unprocessable(why) => why,
        _ => "missing necessary objects".to_owned(),
    })?;

    let mut old_blobs = BTreeMap::new();
    if let Some(old) = old_oid
        && let Ok(old_commit) = repo.find_commit(old)
        && let Ok(old_tree) = old_commit.tree()
    {
        let _ = collect_tree_blobs(repo, &old_tree, "", &mut old_blobs);
    }

    for (path, (kind, blob_oid)) in &new_blobs {
        validate_tree_path(path).map_err(|e| e.to_string())?;
        // A gitlink's commit lives in the submodule's repository, not in this pack.
        if *kind != EntryKind::Commit {
            // The header answers: the index pass verified the bytes.
            repo.find_header(*blob_oid)
                .ok()
                .filter(|h| h.kind() == gix_object::Kind::Blob)
                .ok_or_else(|| "missing necessary objects".to_owned())?;
        }
        let old_entry = old_blobs.get(path);
        if old_entry.map(|(_, o)| o) != Some(blob_oid) {
            changed_paths.push(path.clone());
        }
    }
    for old_path in old_blobs.keys() {
        if !new_blobs.contains_key(old_path) {
            changed_paths.push(old_path.clone());
        }
    }
    Ok(())
}

/// Runs the pre-receive secret scan (T9, SEC-06) on every blob a commit of this push brings in,
/// not only the tip's: a credential added in one commit and removed in the next would otherwise
/// stay in history. Commits any branch already holds are hidden from the walk. The scan always
/// runs; no instance or repository setting turns it off (T-6713).
/// ponytail: only local branches are hidden, so a first push into an empty repository walks every
/// commit's full tree (fine at seed size); hide tags too or diff trees if large imports lag.
/// How every secret-scan refusal starts.
const SECRET_REFUSAL: &str = "pre-receive secret scan blocked";

fn scan_new_commits(repo: &gix::Repository, new_oid: ObjectId) -> Result<(), String> {
    let missing = |_| "missing necessary objects".to_owned();
    let tip = repo
        .find_object(new_oid)
        .map_err(missing)?
        .peel_to_kind(gix_object::Kind::Commit)
        .map_err(missing)?
        .id;
    let held: Vec<ObjectId> = repo
        .references()
        .map_err(|e| e.to_string())?
        .local_branches()
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|r| r.try_id().map(|id| id.detach()))
        .collect();
    let walk = repo
        .rev_walk([tip])
        .with_hidden(held)
        .all()
        .map_err(|e| e.to_string())?;
    let mut scanned = BTreeSet::new();
    for info in walk {
        let commit = info.map_err(missing)?.object().map_err(missing)?;
        let tree = commit.tree().map_err(missing)?;
        let parents: Vec<gix::Tree<'_>> = commit
            .parent_ids()
            .filter_map(|p| p.object().ok()?.into_commit().tree().ok())
            .collect();
        // A blob a parent holds at the same path was scanned when that parent came in.
        let mut blobs = BTreeMap::new();
        collect_changed_blobs(repo, &tree, &parents, &mut blobs).map_err(|e| e.to_string())?;
        for (path, blob_oid) in &blobs {
            if !scanned.insert(*blob_oid) {
                continue;
            }
            let blob = repo.find_blob(*blob_oid).map_err(missing)?;
            if let Some(msg) = scan_blob_for_secrets(path, &blob.data) {
                return Err(msg);
            }
        }
    }
    Ok(())
}

fn build_ref_edit(cmd: &RefUpdateCommand) -> Result<RefEdit, String> {
    let full_name: FullName = cmd
        .refname
        .clone()
        .try_into()
        .map_err(|e: gix_validate::reference::name::Error| e.to_string())?;
    if cmd.is_delete() {
        Ok(RefEdit {
            change: Change::Delete {
                expected: PreviousValue::MustExistAndMatch(Target::Object(cmd.old_oid)),
                log: RefLog::AndReference,
            },
            name: full_name,
            deref: false,
        })
    } else {
        let expected = if cmd.is_create() {
            PreviousValue::MustNotExist
        } else {
            PreviousValue::MustExistAndMatch(Target::Object(cmd.old_oid))
        };
        Ok(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: "push".into(),
                },
                expected,
                new: Target::Object(cmd.new_oid),
            },
            name: full_name,
            deref: false,
        })
    }
}

fn format_unpack_error(
    reason: &str,
    commands: &[RefUpdateCommand],
    caps: &BTreeSet<String>,
) -> ShakoResult<ReceivePackOutcome> {
    let results: Vec<(RefUpdateCommand, Result<(), String>)> = commands
        .iter()
        .map(|c| (c.clone(), Err("unpacker error".into())))
        .collect();
    let response_bytes = format_report_status(&format!("error {reason}"), &results, caps)?;
    Ok(ReceivePackOutcome {
        response_bytes,
        updated_refs: Vec::new(),
        pack_size_bytes: 0,
    })
}

fn format_report_status(
    unpack_status: &str,
    results: &[(RefUpdateCommand, Result<(), String>)],
    caps: &BTreeSet<String>,
) -> ShakoResult<Vec<u8>> {
    let report = caps.contains("report-status") || caps.contains("report-status-v2");
    if !report {
        return Ok(Vec::new());
    }
    let v2 = caps.contains("report-status-v2");
    let mut inner = Vec::new();
    write_text_line(&mut inner, &format!("unpack {unpack_status}"))?;
    for (cmd, res) in results {
        match res {
            Ok(()) => {
                write_text_line(&mut inner, &format!("ok {}", cmd.refname))?;
                if v2 {
                    write_text_line(&mut inner, &format!("option refname {}", cmd.refname))?;
                    write_text_line(&mut inner, &format!("option old-oid {}", cmd.old_oid))?;
                    write_text_line(&mut inner, &format!("option new-oid {}", cmd.new_oid))?;
                }
            }
            Err(reason) => {
                write_text_line(&mut inner, &format!("ng {} {reason}", cmd.refname))?;
            }
        }
    }
    write_flush(&mut inner)?;

    if caps.contains("side-band-64k") {
        let mut framed = Vec::new();
        write_sideband_chunks(&mut framed, Channel::Data, &inner, MAX_SIDEBAND_64K_DATA)?;
        write_flush(&mut framed)?;
        Ok(framed)
    } else if caps.contains("side-band") {
        let mut framed = Vec::new();
        write_sideband_chunks(&mut framed, Channel::Data, &inner, MAX_SIDEBAND_1K_DATA)?;
        write_flush(&mut framed)?;
        Ok(framed)
    } else {
        Ok(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FLUSH, pack_of, pkt, put, receive, repo, text};
    use shako_core::BranchProtectionRule;

    const ZERO: &str = "0000000000000000000000000000000000000000";

    /// Commands (the first carrying `caps`), a flush, then `pack`.
    fn body(commands: &[String], caps: &str, pack: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, command) in commands.iter().enumerate() {
            let line = if i == 0 {
                format!("{command}\0{caps}")
            } else {
                command.clone()
            };
            write_text_line(&mut out, &line).expect("pkt");
        }
        write_flush(&mut out).expect("flush");
        out.extend_from_slice(pack);
        out
    }

    /// A source with `main` at `c1` then `c2`, and an empty destination.
    struct Push {
        _tmp: tempfile::TempDir,
        source: PathBuf,
        dest: PathBuf,
        c1: ObjectId,
        c2: ObjectId,
    }

    fn push() -> Push {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = repo(tmp.path(), 1, true);
        let c1 = put(&source, "main", "a.txt", b"one\n");
        let c2 = put(&source, "main", "b.txt", b"two\n");
        let dest = repo(tmp.path(), 2, false);
        Push {
            _tmp: tmp,
            source,
            dest,
            c1,
            c2,
        }
    }

    fn create(p: &Push, tip: ObjectId, refname: &str, caps: &str) -> ReceivePackOutcome {
        let pack = pack_of(&p.source, &tip.to_string());
        receive(
            &p.dest,
            &RefPolicy::default(),
            &body(&[format!("{ZERO} {tip} {refname}")], caps, &pack),
        )
        .expect("receive")
    }

    fn report(p: &Push, commands: &[String], caps: &str, pack: &[u8]) -> String {
        text(
            &receive(&p.dest, &RefPolicy::default(), &body(commands, caps, pack))
                .expect("receive")
                .response_bytes,
        )
    }

    #[test]
    fn the_advertisement_lists_branches_and_tags_but_never_head() {
        let p = push();
        let empty = text(&advertise_receive_pack(&p.dest).expect("adv"));
        assert!(
            empty.contains(&format!(
                "{ZERO} capabilities^{{}}\0{RECEIVE_PACK_CAPABILITIES}"
            )),
            "{empty}"
        );
        create(&p, p.c2, "refs/heads/main", "report-status");
        create(&p, p.c1, "refs/tags/v1", "report-status");
        let adv = text(&advertise_receive_pack(&p.dest).expect("adv"));
        assert!(
            adv.contains(&format!("{} refs/heads/main\0report-status", p.c2)),
            "{adv}"
        );
        assert!(adv.contains(&format!("{} refs/tags/v1\n", p.c1)), "{adv}");
        assert!(!adv.contains(" HEAD"), "{adv}");
        assert!(advertise_receive_pack(&p.dest.join("none")).is_err());
    }

    #[test]
    fn a_command_knows_a_create_and_a_delete_by_its_zero_ids() {
        let zero = ObjectId::null(gix_hash::Kind::Sha1);
        let one = ObjectId::from_hex(b"1111111111111111111111111111111111111111").expect("id");
        let cmd = |old_oid, new_oid| RefUpdateCommand {
            old_oid,
            new_oid,
            refname: "refs/heads/main".into(),
        };
        assert!(cmd(zero, one).is_create() && !cmd(zero, one).is_delete());
        assert!(cmd(one, zero).is_delete() && !cmd(one, zero).is_create());
        assert!(!cmd(one, one).is_create() && !cmd(one, one).is_delete());
    }

    #[test]
    fn a_malformed_command_list_is_refused_before_anything_is_stored() {
        let p = push();
        let new = p.c1;
        let refused = |raw: &[u8], says: &str| {
            let err = receive(&p.dest, &RefPolicy::default(), raw).expect_err(says);
            assert_eq!(err.status_code(), 400, "{says}: {err}");
            assert!(err.to_string().contains(says), "{says}: {err}");
        };
        refused(
            &pkt(&[&format!("{ZERO} {new} refs/heads/main")]),
            "missing flush packet",
        );
        refused(b"0008\xff\xfe\xfd\xfc0000", "non-utf8 command line");
        refused(
            &pkt(&["only two", FLUSH]),
            "malformed receive-pack command 'only two'",
        );
        refused(
            &pkt(&[&format!("zz {new} refs/heads/main"), FLUSH]),
            "invalid old_id",
        );
        refused(
            &pkt(&[&format!("{ZERO} zz refs/heads/main"), FLUSH]),
            "invalid new_id",
        );
        refused(b"0000PACK", "requires at least one command");
        let mut long = Vec::new();
        let filler = format!("shallow {}", "a".repeat(65_000));
        for _ in 0..17 {
            write_text_line(&mut long, &filler).expect("pkt");
        }
        refused(&long, "commands exceed");
        // A lone flush is git asking before a large push: an empty answer.
        let probe = receive(&p.dest, &RefPolicy::default(), b"0000").expect("probe");
        assert!(probe.response_bytes.is_empty() && probe.updated_refs.is_empty());
        assert!(!crate::compact::has_active_quarantine(&p.dest));
    }

    #[test]
    fn a_push_past_the_ref_limit_or_with_a_bad_pack_is_an_unpack_error() {
        let p = push();
        let many: Vec<String> = (0..=MAX_REFS_PER_PUSH)
            .map(|i| format!("{ZERO} {} refs/heads/b{i}", p.c1))
            .collect();
        let out = report(&p, &many, "report-status", b"");
        assert!(
            out.contains("unpack error too many refs in push (limit 1000)"),
            "{}",
            &out[..80]
        );
        let one = [format!("{ZERO} {} refs/heads/main", p.c1)];
        let out = report(&p, &one, "report-status", b"JUNK");
        assert!(out.contains("unpack error missing PACK header"), "{out}");
        assert!(out.contains("ng refs/heads/main unpacker error"), "{out}");
        let out = report(&p, &one, "report-status", b"PACK\0\0\0\x09garbage");
        assert!(out.contains("unpack error invalid pack header"), "{out}");
        let mut pack = pack_of(&p.source, &p.c1.to_string());
        pack.truncate(pack.len() - 30);
        let out = report(&p, &one, "report-status", &pack);
        assert!(
            out.contains("unpack error ") && !out.contains("unpack ok"),
            "{out}"
        );
        // A pack that reads but leaves out an object the tip needs.
        let full = pack_of(&p.source, &p.c1.to_string());
        let out = report(
            &p,
            &[format!("{ZERO} {} refs/heads/main", p.c2)],
            "report-status",
            &full,
        );
        assert!(
            out.contains("ng refs/heads/main missing necessary objects"),
            "{out}"
        );
        assert!(
            crate::repo::list_branches(&p.dest)
                .expect("branches")
                .is_empty()
        );
        assert!(!crate::compact::has_active_quarantine(&p.dest));
    }

    #[test]
    fn the_report_follows_the_asked_format() {
        let p = push();
        let out = create(
            &p,
            p.c1,
            "refs/heads/main",
            "report-status-v2 side-band-64k",
        );
        let lines = crate::pktline::parse_all_pkt_lines(&out.response_bytes).expect("framed");
        let inner: Vec<u8> = lines
            .iter()
            .filter_map(|l| match l {
                crate::pktline::PktLine::Data(d) => d.strip_prefix(b"\x01").map(<[u8]>::to_vec),
                _ => None,
            })
            .flatten()
            .collect();
        let inner = text(&inner);
        assert!(inner.contains("ok refs/heads/main"), "{inner}");
        assert!(inner.contains("option refname refs/heads/main"), "{inner}");
        assert!(inner.contains(&format!("option old-oid {ZERO}")), "{inner}");
        assert!(
            inner.contains(&format!("option new-oid {}", p.c1)),
            "{inner}"
        );
        assert!(out.pack_size_bytes > 0);

        let out = create(&p, p.c1, "refs/heads/one-k", "report-status side-band");
        assert!(
            text(&out.response_bytes).contains("\u{1}000eunpack ok"),
            "{:?}",
            text(&out.response_bytes)
        );
        let out = create(&p, p.c1, "refs/heads/quiet", "ofs-delta");
        assert!(out.response_bytes.is_empty());
        assert_eq!(
            out.updated_refs.len(),
            1,
            "no report asked, the ref still moves"
        );
    }

    #[test]
    fn each_ref_is_checked_against_what_the_repository_holds() {
        let p = push();
        create(&p, p.c1, "refs/heads/main", "report-status");
        let (c1, c2) = (p.c1, p.c2);
        let pack = pack_of(&p.source, &c2.to_string());
        let cases = [
            (
                format!("{ZERO} {c2} refs/pull/1/head"),
                "refusing to update non-branch/tag ref",
            ),
            (
                format!("{ZERO} {c2} HEAD"),
                "refusing to update non-branch/tag ref",
            ),
            (format!("{ZERO} {c2} refs/heads/a..b"), "ng refs/heads/a..b"),
            (
                format!("{ZERO} {c2} refs/heads/main"),
                "ng refs/heads/main fetch first",
            ),
            (
                format!("{c2} {c2} refs/heads/main"),
                "ng refs/heads/main fetch first",
            ),
            (
                format!("{c1} {c2} refs/heads/other"),
                "ng refs/heads/other fetch first",
            ),
        ];
        for (command, says) in cases {
            let out = report(&p, std::slice::from_ref(&command), "report-status", &pack);
            assert!(
                out.contains("unpack ok") && out.contains(says),
                "{command}: {out}"
            );
        }
        assert_eq!(
            crate::repo::list_branches(&p.dest).expect("branches").len(),
            1
        );
        // A refused push leaves no pack beside the one the accepted push stored.
        let packs = std::fs::read_dir(p.dest.join("objects/pack"))
            .expect("dir")
            .count();
        assert_eq!(packs, 2, "one .pack and its .idx");

        // A fast-forward, then a delete with no pack at all.
        let out = report(
            &p,
            &[format!("{c1} {c2} refs/heads/main")],
            "report-status",
            &pack,
        );
        assert!(out.contains("ok refs/heads/main"), "{out}");
        let gone = receive(
            &p.dest,
            &RefPolicy::default(),
            &body(
                &[format!("{c2} {ZERO} refs/heads/main")],
                "report-status",
                b"",
            ),
        )
        .expect("delete");
        assert!(text(&gone.response_bytes).contains("ok refs/heads/main"));
        assert_eq!(gone.pack_size_bytes, 0);
        assert!(gone.updated_refs[0].is_delete());
        assert!(
            crate::repo::list_branches(&p.dest)
                .expect("branches")
                .is_empty()
        );
    }

    #[test]
    fn a_protected_branch_refuses_a_push_that_rewrites_it() {
        let p = push();
        create(&p, p.c2, "refs/heads/main", "report-status");
        let protected = RefPolicy {
            branch_rules: vec![BranchProtectionRule {
                rule_name: "main".into(),
                enable_push: true,
                ..BranchProtectionRule::default()
            }],
            tag_rules: vec![],
        };
        let pack = pack_of(&p.source, &p.c1.to_string());
        let rewind = body(
            &[format!("{} {} refs/heads/main", p.c2, p.c1)],
            "report-status",
            &pack,
        );
        let out = text(
            &receive(&p.dest, &protected, &rewind)
                .expect("receive")
                .response_bytes,
        );
        assert!(out.contains("ng refs/heads/main"), "{out}");
        assert_eq!(
            crate::repo::list_branches(&p.dest).expect("branches")[0].1,
            p.c2.to_string()
        );
    }

    #[test]
    fn an_atomic_push_moves_every_ref_or_none() {
        let p = push();
        let pack = pack_of(&p.source, &p.c2.to_string());
        let both = [
            format!("{ZERO} {} refs/heads/main", p.c2),
            format!("{ZERO} {} refs/tags/v1", p.c1),
        ];
        let half = [both[0].clone(), format!("{ZERO} {} refs/pull/1/head", p.c2)];
        let out = report(&p, &half, "report-status atomic", &pack);
        assert!(
            out.contains("ng refs/heads/main atomic push failed"),
            "{out}"
        );
        assert!(
            crate::repo::list_branches(&p.dest)
                .expect("branches")
                .is_empty()
        );
        assert_eq!(
            std::fs::read_dir(p.dest.join("objects/pack"))
                .expect("dir")
                .count(),
            0,
            "the refused pack is gone"
        );
        let out = report(&p, &both, "report-status atomic", &pack);
        assert!(
            out.contains("ok refs/heads/main") && out.contains("ok refs/tags/v1"),
            "{out}"
        );
        assert_eq!(crate::repo::list_tags(&p.dest).expect("tags").len(), 1);
    }

    #[test]
    fn the_secret_scan_names_each_rule_and_lets_references_through() {
        let key = format!("-----BEGIN {}PRIVATE KEY-----", "RSA ");
        let aws = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
        assert_eq!(
            scan_blob_for_secrets("k.pem", format!("a\nb\n{key}\n").as_bytes()).as_deref(),
            Some("pre-receive secret scan blocked private-key in 'k.pem' line 3")
        );
        assert!(
            scan_blob_for_secrets("x.txt", aws.as_bytes())
                .is_some_and(|m| m.contains("aws-access-key"))
        );
        let manifest =
            |body: &str| format!("apiVersion: joinedcontext.com/v1\nkind: DataSource\n{body}");
        let literal = format!("hunter{}", "2x9q");
        for open in [
            "${PW}",
            "${{ secrets.PW }}",
            "ENC[AES256,abc]",
            "''",
            "\"\"",
        ] {
            let yaml = manifest(&format!("spec:\n  password: {open}\n"));
            assert_eq!(
                scan_blob_for_secrets("s.yaml", yaml.as_bytes()),
                None,
                "{open}"
            );
        }
        let found = |path: &str, yaml: &str| scan_blob_for_secrets(path, yaml.as_bytes());
        let listed = manifest(&format!("spec:\n  items:\n  - token: {literal}\n"));
        assert!(found("s.yml", &listed).is_some_and(|m| m.contains("line 5 (field 'token'")));
        let tagged = manifest(&format!("spec: !Custom\n  apiKey: {literal}\n"));
        assert!(found("s.yaml", &tagged).is_some_and(|m| m.contains("field 'apiKey'")));
        // A flow mapping names the field on another line: the refusal has no line number.
        let flow = manifest(&format!("spec: {{password: {literal}}}\n"));
        assert_eq!(
            found("s.yaml", &flow).as_deref(),
            Some(
                "pre-receive secret scan blocked literal-secret-field in 's.yaml' (field 'password'; name the credential with secretRef)"
            )
        );
        let second = format!(
            "{}---\n{}",
            manifest("spec: {}\n"),
            manifest(&format!("spec:\n  secret: {literal}\n"))
        );
        assert!(found("s.yaml", &second).is_some());
        // Not a manifest, not YAML, not parseable, a ${...} that is no plain name: as written.
        assert_eq!(found("s.yaml", &format!("password: {literal}\n")), None);
        assert_eq!(
            found(
                "s.json",
                &manifest(&format!("spec:\n  password: {literal}\n"))
            ),
            None
        );
        assert_eq!(found("s.yaml", "a: [\n"), None);
        let odd = manifest("spec:\n  password: ${a-b}\n");
        assert!(found("s.yaml", &odd).is_some());
        let empty_ref = manifest("spec:\n  password: ${}\n");
        assert!(found("s.yaml", &empty_ref).is_some());
    }
}
