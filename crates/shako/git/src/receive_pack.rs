#![forbid(unsafe_code)]
//! Git Smart HTTP `receive-pack` server with quarantine (`objects/incoming-<uuid>/`),
//! thin-pack completion, object size and delta-depth limits, connectivity check,
//! secret scanning, ref policy evaluation, and `report-status` / `report-status-v2` (Contract S4, S5, T5, T7, T9).

use crate::pktline::{
    MAX_SIDEBAND_1K_DATA, MAX_SIDEBAND_64K_DATA, PktLine, read_pkt_line, write_flush,
    write_sideband_chunks, write_text_line,
};
use crate::repo::{collect_tree_blobs, is_ancestor, open_isolated};
use gix_hash::ObjectId;
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
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, atomic::AtomicBool};

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

static SECRET_SCAN_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
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
        (
            "shako-fake-secret",
            Regex::new(r"SHAKO_SECRET_KEY_[A-Za-z0-9]{16,}").expect("static regex"),
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
    let text = String::from_utf8_lossy(bytes);
    for (rule_name, regex) in SECRET_SCAN_PATTERNS.iter() {
        if let Some(found) = regex.find(&text) {
            let line = text[..found.start()].matches('\n').count() + 1;
            return Some(format!(
                "pre-receive secret scan blocked {rule_name} in '{path}' line {line}"
            ));
        }
    }
    if (path.ends_with(".yaml") || path.ends_with(".yml"))
        && let Some((field, line)) = literal_secret_field(&text)
    {
        let at = line.map(|n| format!(" line {n}")).unwrap_or_default();
        return Some(format!(
            "pre-receive secret scan blocked literal-secret-field in '{path}'{at} \
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

/// Processes a `POST {repo}.git/git-receive-pack` request (S4, S5, T5, T7, T9).
pub fn handle_receive_pack(
    repo_path: &Path,
    guard: &PushGuard<'_>,
    max_push_bytes: u64,
    body: &[u8],
) -> ShakoResult<ReceivePackOutcome> {
    // 1. Parse command list up to Flush (0000), leaving `rest` at the start of the raw PACK stream
    let mut rest = body;
    let mut commands = Vec::new();
    let mut caps = BTreeSet::new();

    loop {
        let Some((line, used)) = read_pkt_line(rest)? else {
            return Err(ShakoError::BadRequest(
                "receive-pack missing flush packet after commands".into(),
            ));
        };
        rest = &rest[used..];
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

    if commands.is_empty() {
        return Err(ShakoError::BadRequest(
            "receive-pack requires at least one command".into(),
        ));
    }

    let atomic = caps.contains("atomic");
    let pack_data = rest;

    // 2. Enforce streaming push byte limit / quota BEFORE storing anything (S4, S6, Threat T5)
    if pack_data.len() as u64 > max_push_bytes {
        return format_unpack_error("quota", &commands, &caps);
    }

    let has_non_delete = commands.iter().any(|c| !c.is_delete());
    let mut quarantine = QuarantineDir::create(repo_path)?;
    let mut written_pack_path: Option<PathBuf> = None;
    let mut written_idx_path: Option<PathBuf> = None;

    // 3. Unpack and validate incoming packfile in quarantine (`objects/incoming-<uuid>/`)
    if has_non_delete || !pack_data.is_empty() {
        if !pack_data.starts_with(b"PACK") {
            return format_unpack_error("missing PACK header", &commands, &caps);
        }
        if let Err(reason) = validate_and_index_pack_in_quarantine(
            repo_path,
            &quarantine.path,
            pack_data,
            &mut written_pack_path,
            &mut written_idx_path,
        ) {
            return format_unpack_error(&reason, &commands, &caps);
        }
    }

    // 4. Move indexed pack into `objects/pack/` temporarily with a rollback list so `gix::Repository`
    //    can inspect trees, fast-forward ancestry, and connectivity of new tips; if all refs are
    //    rejected (or atomic fails), we delete the moved pack before returning!
    let mut installed_files = Vec::new();
    if let (Some(p_src), Some(i_src)) = (&written_pack_path, &written_idx_path) {
        let pack_dir = repo_path.join("objects/pack");
        std::fs::create_dir_all(&pack_dir)
            .map_err(|e| ShakoError::Storage(format!("mkdir pack: {e}")))?;
        let p_dst = pack_dir.join(p_src.file_name().unwrap_or_default());
        let i_dst = pack_dir.join(i_src.file_name().unwrap_or_default());
        std::fs::rename(p_src, &p_dst)
            .map_err(|e| ShakoError::Storage(format!("move pack: {e}")))?;
        std::fs::rename(i_src, &i_dst)
            .map_err(|e| ShakoError::Storage(format!("move idx: {e}")))?;
        installed_files.push(p_dst);
        installed_files.push(i_dst);
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

    // If `atomic` capability was requested and any command failed, reject all commands
    if atomic && cmd_results.iter().any(|(_, r)| r.is_err()) {
        for (_, r) in &mut cmd_results {
            if r.is_ok() {
                *r = Err("atomic push failed".into());
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
        pack_data.len() as u64
    };

    let response_bytes = format_report_status("ok", &final_results, &caps)?;
    Ok(ReceivePackOutcome {
        response_bytes,
        updated_refs,
        pack_size_bytes,
    })
}

fn validate_and_index_pack_in_quarantine(
    repo_path: &Path,
    quarantine_dir: &Path,
    pack_data: &[u8],
    out_pack: &mut Option<PathBuf>,
    out_idx: &mut Option<PathBuf>,
) -> Result<(), String> {
    // 1. Streaming pre-scan of pack entries with `BytesToEntriesIter` to enforce:
    //    - `decompressed_size <= MAX_SINGLE_OBJECT_BYTES` (100 MiB)
    //    - `delta_chain_depth <= MAX_DELTA_CHAIN_DEPTH` (50) (Threat T5)
    let cursor = std::io::Cursor::new(pack_data);
    let entries_iter = BytesToEntriesIter::new_from_header(
        cursor,
        InputMode::Verify,
        EntryDataMode::Ignore,
        gix_hash::Kind::Sha1,
    )
    .map_err(|e| format!("invalid pack header: {e}"))?;

    let mut depth_by_offset: BTreeMap<u64, usize> = BTreeMap::new();
    for entry_res in entries_iter {
        let entry = entry_res.map_err(|e| format!("corrupt pack entry: {e}"))?;
        if entry.decompressed_size > MAX_SINGLE_OBJECT_BYTES {
            return Err(format!(
                "object size {} exceeds limit {MAX_SINGLE_OBJECT_BYTES}",
                entry.decompressed_size
            ));
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
    }

    // 2. Index and thicken thin pack in `quarantine_dir` using `Bundle::write_to_directory`
    let repo = open_isolated(repo_path).map_err(|e| e.to_string())?;
    let odb = repo.objects.clone();
    let mut pack_reader = std::io::Cursor::new(pack_data);
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

    for (path, (_, blob_oid)) in &new_blobs {
        validate_tree_path(path).map_err(|e| e.to_string())?;
        repo.find_blob(*blob_oid)
            .map_err(|_| "missing necessary objects".to_owned())?;
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
        let mut blobs = BTreeMap::new();
        let tree = commit.tree().map_err(missing)?;
        collect_tree_blobs(repo, &tree, "", &mut blobs).map_err(|e| e.to_string())?;
        let mut parents = Vec::new();
        for parent in commit.parent_ids() {
            let mut held = BTreeMap::new();
            if let Ok(parent) = parent.object().map(gix::Object::into_commit)
                && let Ok(tree) = parent.tree()
            {
                collect_tree_blobs(repo, &tree, "", &mut held).map_err(|e| e.to_string())?;
            }
            parents.push(held);
        }
        for (path, (_, blob_oid)) in &blobs {
            let inherited = parents
                .iter()
                .any(|held| held.get(path).map(|(_, o)| o) == Some(blob_oid));
            if inherited || !scanned.insert(*blob_oid) {
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
