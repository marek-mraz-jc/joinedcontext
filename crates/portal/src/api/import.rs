//! Importing a manifest, a bundle or an archive into one project (T-0193, MF-20…MF-26).
//!
//! The mirror image of [`super::export`], and deliberately the same shape: an export is the
//! repository at a revision, so an import is a set of manifests written back to the
//! repository as one merge request. Nothing lands directly. What the caller uploads is
//! parsed, validated, remapped and planned, and the answer is a `202` with the same
//! `kind: Change` a single write produces — the reviewer sees one lane and one diff for the
//! whole bundle rather than one merge request per file (MF-21, CC-63).
//!
//! Four gates stand between an upload and a branch, and each one refuses rather than
//! repairs (MF-24): a manifest of a kind this platform does not serve, a `status` block the
//! Portal computes itself, a credential pasted as a literal string, and a reference to a
//! resource that is in neither the bundle nor the project. A bundle that half-imports is
//! worse than one that does not import, because the half that landed is now a project
//! nobody described.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::api::mutate::{author_credentials, create_or_reuse_branch, find_literal_secret};
use crate::apps::reconciler::generate_slug;
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangePhase, ChangeStatus, Lane, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::Author;
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// Largest upload the endpoint reads: the edge's largest body (ADR-N-035). The organization's
/// `spec.limits.data.uploadMegabytes` is the limit in force below it (`within_upload_limit`).
pub(crate) const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Entries an archive may hold. A bundle carrying a thousand files is a mistake or an attack,
/// and either way it must not become a thousand forge calls.
const MAX_ARCHIVE_ENTRIES: usize = 2_000;

/// Bytes an archive may unpack to, every entry counted together.
///
/// [`MAX_UPLOAD_BYTES`] bounds what arrives on the wire, and deflate is free to multiply it:
/// two hundred kilobytes of one repeated character expand to two hundred megabytes, and the
/// reader that met them held all of it in memory at once. A bundle is manifests, so what comes
/// out of the archive is held to the same ceiling as what went in (CC-08, MF-05).
const MAX_ARCHIVE_BYTES: u64 = MAX_UPLOAD_BYTES as u64;

/// The mode bits a zip entry carries for a symbolic link (`S_IFLNK`).
const UNIX_SYMLINK: u32 = 0o120_000;

/// The file-type half of a unix mode.
const UNIX_FILE_TYPE: u32 = 0o170_000;

/// The path of an archive entry inside the archive, or `None` for one that climbs out of it or
/// is absolute. zip 9 strips a leading `/`, `\` or drive and calls the rest enclosed; a name that
/// says `/etc/cron.d/x` was written to go there, so it is refused rather than read as `etc/cron.d/x`.
pub(crate) fn enclosed_path<R: Read + ?Sized>(
    entry: &zip::read::ZipFile<'_, R>,
) -> Option<std::path::PathBuf> {
    let raw = entry.name_raw();
    let absolute = matches!(raw.first(), Some(b'/' | b'\\')) || raw.get(1) == Some(&b':');
    if absolute {
        return None;
    }
    entry.enclosed_name()
}

/// The index an exported archive carries. It describes the bundle rather than belonging to the
/// project, so it is read for provenance and never written (MF-17).
const BUNDLE_KIND: &str = "Bundle";

/// What to do with a resource the project already has (MF-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Refuse the whole import. The default: an import that silently overwrote a live
    /// endpoint would be a deployment nobody proposed.
    #[default]
    Fail,
    /// Leave the existing resource alone and import the rest.
    Skip,
    /// Overwrite the existing resource with the imported one.
    Replace,
    /// Import under a new name, rewriting every reference in the bundle that named it (MF-26).
    Rename,
}

impl ConflictPolicy {
    fn parse(raw: &str) -> Result<Self, ApiError> {
        match raw {
            "fail" => Ok(Self::Fail),
            "skip" => Ok(Self::Skip),
            "replace" => Ok(Self::Replace),
            "rename" => Ok(Self::Rename),
            other => Err(ApiError::BadRequest(format!(
                "conflictPolicy '{other}' is not fail, skip, replace or rename (MF-23)"
            ))),
        }
    }
}

/// The options an import wizard sends beside the file (Architecture/06 §6).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    /// The namespace every imported manifest is rewritten into; the project when absent (MF-22).
    #[serde(default)]
    pub target_namespace: Option<String>,
    /// The organisation domain every imported id is rewritten to; this instance's own
    /// organisation when absent (PF-10, PF-43).
    #[serde(default)]
    pub org_domain: Option<String>,
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    #[serde(default)]
    pub dry_run: bool,
    /// A bundle to fetch rather than upload. Refused for now, see [`import`].
    #[serde(default)]
    pub url: Option<String>,
    /// The manifests themselves, for a caller that posts JSON rather than a file.
    #[serde(default)]
    pub manifests: Option<Value>,
    /// Spaces the bundle names that are not copied: each reference to `from` lands on the
    /// target project's space `to`, ids included (T-1441, MF-26).
    #[serde(default)]
    pub space_mapping: Vec<SpaceMapping>,
}

/// One space of the origin mapped onto a space of the target project (T-1441).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpaceMapping {
    pub from: String,
    pub to: String,
}

/// `?dryRun=All`, as the OpenAPI document, the multipart field and every caller spell it.
///
/// Without the rename serde bound the parameter `dry_run`, so `?dryRun=All` — what the endpoint
/// form's **Check** sends, what `API/01 §10` documents and what the generated client uses — was
/// silently ignored and every check committed a change and opened a merge request (T-1226). A
/// person pressing a button that says "check" wrote to the repository, and the next check
/// answered `409: a change for … is already open`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportQuery {
    #[serde(default)]
    pub dry_run: Option<String>,
    /// `validate_only=true` (AIP-163): the same as `dryRun=All` (T-5949).
    #[serde(default, rename = "validate_only")]
    pub validate_only: Option<bool>,
    /// `git`: the archive of a `format=git` export, landing as a new project (MF-45).
    #[serde(default)]
    pub format: Option<String>,
}

/// What one import would do, answered on a dry run and echoed in the merge request body.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Resources that would be created.
    pub created: Vec<String>,
    /// Resources that would replace one the project already has.
    pub replaced: Vec<String>,
    /// Resources left alone because the project already has them (`skip`).
    pub skipped: Vec<String>,
    /// Resources imported under a new name, `old -> new` (`rename`).
    pub renamed: BTreeMap<String, String>,
    /// Policies whose `assigner` the import rewrote to `did:web:{orgDomain}`, as `Policy/{name}`
    /// to the DID the bundle carried: the grant is this organisation's now (CC-82, R6).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reassigned: BTreeMap<String, String>,
    /// Files carried through untouched: `bento.yaml`, LinkML sources, schema artifacts.
    pub native_files: usize,
    /// The lane the whole bundle lands in: the riskiest of everything it carries (CC-63).
    pub lane: Lane,
    /// Where the bundle came from, when it carried a `kind: Bundle` index (MF-20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Per file, whether what the import wrote equals the checksum the bundle index carries,
    /// with the namespace mapping undone (MF-42). Empty when the bundle carries no checksums:
    /// an unverifiable transfer says so rather than claiming every file is equal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified: Vec<Verified>,
    /// What the bundle cannot carry and has to be provided where it lands (CC-84): secret
    /// values, people, hosts, feed credentials. Empty means nothing is to be provided. Never a
    /// secret value, only where one is set.
    pub needs: Vec<Need>,
}

/// One thing a copy cannot carry (CC-84, API/01 §10).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Need {
    /// `secret`, `person` or `host`.
    pub kind: String,
    /// The manifest and the path inside it, `Kind/name spec.path`.
    #[serde(rename = "where")]
    pub location: String,
    /// Why it has to be provided, in words.
    pub why: String,
    /// The Portal page that sets it.
    pub link: String,
}

/// Every need of the manifests an import writes, in manifest order (CC-84).
///
/// A `secretRef` names a value that stays behind in the origin's secret store; a user named
/// by a RoleBinding, a Policy or a Group is a person of the origin's realm; an Environment's
/// hosts and certificates are the origin's; a DataSource's `authorization` is a feed
/// credential. Each is reported where it is, and nothing here blocks the import.
pub fn needs_of(manifests: &[ResourceEnvelope], project: &str) -> Vec<Need> {
    /// Every field a jc-core kind types as a `SecretRef`, or a list of them (MF-35, T-2564):
    /// `secretRef`/`previousSecretRef` (Subscription, SyncSource), `secretRefs` (Pipeline),
    /// `secrets` (DataSource), `passwordRef`, `headerRef` and `caCertRef` (DataSource),
    /// `apiTokenRef` (CkanInstance), `tokenSecretRef` (DataspaceConnector) and
    /// `cachedTokenSecretRef` (Endpoint). A new one in jc-core is added here.
    const SECRET_FIELDS: [&str; 9] = [
        "secretRef",
        "previousSecretRef",
        "secretRefs",
        "passwordRef",
        "headerRef",
        "caCertRef",
        "apiTokenRef",
        "tokenSecretRef",
        "cachedTokenSecretRef",
    ];
    fn walk(value: &Value, path: &str, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let here = format!("{path}.{key}");
                    // One need per field, a list included: the report names where to set the
                    // value, never the secret's name, key or value (CC-06).
                    if SECRET_FIELDS.contains(&key.as_str())
                        && (child.is_object() || child.is_array())
                    {
                        found.push(here);
                        continue;
                    }
                    // A `secrets` list is a need of its own, and each entry is still walked: an
                    // entry names its `secretRef`, which is one more (MF-35).
                    if key == "secrets" && child.is_array() {
                        found.push(here.clone());
                    }
                    walk(child, &here, found);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{index}]"), found);
                }
            }
            _ => {}
        }
    }
    let mut needs = Vec::new();
    for envelope in manifests {
        let name = format!("{}/{}", envelope.kind, envelope.metadata.name);
        let plural = resource::by_kind(&envelope.kind).map_or("resources", |info| info.plural);
        let link = format!("/projects/{project}/{plural}");
        let need = |kind: &str, path: &str, why: String| Need {
            kind: kind.to_owned(),
            location: format!("{name} {path}"),
            why,
            link: link.clone(),
        };
        let mut secrets = Vec::new();
        walk(&envelope.spec, "spec", &mut secrets);
        for path in secrets {
            needs.push(need(
                "secret",
                &path,
                "the value behind this reference stays in the origin's secret store; set it here"
                    .into(),
            ));
        }
        let mut users = Vec::new();
        match envelope.kind.as_str() {
            "RoleBinding" => {
                for (index, subject) in envelope.spec["subjects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(user) = subject.get("user").and_then(Value::as_str) {
                        users.push((format!("spec.subjects[{index}].user"), user.to_owned()));
                    }
                }
            }
            "Group" => {
                for (index, member) in envelope.spec["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(user) = member.get("user").and_then(Value::as_str) {
                        users.push((format!("spec.members[{index}].user"), user.to_owned()));
                    }
                }
            }
            "Policy"
                if envelope
                    .spec
                    .pointer("/assignee/kind")
                    .and_then(Value::as_str)
                    == Some("user") =>
            {
                if let Some(user) = envelope
                    .spec
                    .pointer("/assignee/id")
                    .and_then(Value::as_str)
                {
                    users.push(("spec.assignee.id".to_owned(), user.to_owned()));
                }
            }
            "Environment" => {
                needs.push(need(
                    "host",
                    "spec",
                    "the hosts and certificates of an environment are the origin's; set this instance's".into(),
                ));
            }
            _ => {}
        }
        for (path, user) in users {
            needs.push(need(
                "person",
                &path,
                format!(
                    "{user} is a person of the origin's sign-in; bind someone of this organization"
                ),
            ));
        }
    }
    needs
}

/// One file of a bundle as the import verified it (MF-42).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Verified {
    /// The path the bundle index gave the file.
    pub path: String,
    /// Whether the checksum matched.
    pub equal: bool,
}

impl ImportReport {
    /// `n of m files equal`, the line the `Change` body leads with (MF-42), or `None` when the
    /// bundle carried no checksums.
    pub fn verification_summary(&self) -> Option<String> {
        if self.verified.is_empty() {
            return None;
        }
        let equal = self.verified.iter().filter(|file| file.equal).count();
        Some(format!("{equal} of {} files equal", self.verified.len()))
    }

    /// One line per Policy whose assigner the import rewrote, for the `Change` body: the person
    /// who approves sees that the grant is signed by this organisation now (R6, T-2256).
    fn reassignment_lines(&self) -> Vec<String> {
        self.reassigned
            .iter()
            .map(|(policy, carried)| format!("{policy}: assigner {carried} is now {OWN_ASSIGNER}"))
            .collect()
    }
}

/// One manifest or native file on its way into the repository.
struct Incoming {
    path: Option<String>,
    envelope: Option<ResourceEnvelope>,
    content: String,
}

// --- parsing what was uploaded ---------------------------------------------------------

/// Everything an upload holds, whatever shape it arrived in (Architecture/06 §6).
///
/// A zip is the archive an export produced, a YAML stream is one or more manifests, and JSON
/// is a single manifest or a `kind: List` of them. The three are told apart by content rather
/// than by a declared media type: a browser's `Content-Type` on a file part is whatever the
/// operating system guessed, and being wrong about it would refuse a valid bundle.
fn parse(bytes: &[u8]) -> Result<Vec<Incoming>, ApiError> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the upload is larger than the {MAX_UPLOAD_BYTES} byte import limit"
        )));
    }
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("the upload is empty".into()));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return parse_archive(bytes);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ApiError::BadRequest("the upload is neither a zip archive nor text".into()))?;
    parse_text(text, None)
}

fn parse_archive(bytes: &[u8]) -> Result<Vec<Incoming>, ApiError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| ApiError::BadRequest(format!("the archive did not open: {err}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ApiError::BadRequest(format!(
            "the archive holds {} entries, past the {MAX_ARCHIVE_ENTRIES} entry limit",
            archive.len()
        )));
    }
    let mut incoming = Vec::new();
    // What is left of the unpacked ceiling. Counted across entries rather than per entry,
    // because a thousand files of a megabyte each is the same bomb spread thin (CC-08).
    let mut budget = MAX_ARCHIVE_BYTES;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| ApiError::BadRequest(format!("archive entry {index}: {err}")))?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` is None for a path that escapes the archive root, which is the
        // zip-slip a bundle from another instance could carry.
        let name = enclosed_path(&entry)
            .ok_or_else(|| ApiError::BadRequest("the archive holds an escaping path".into()))?
            .to_string_lossy()
            .into_owned();
        // A symbolic link is not a file of the project. Read as one it becomes a manifest whose
        // body is the path it pointed at, which is how an archive smuggles `/etc/passwd` in as
        // content nobody uploaded; refused by name rather than quietly flattened (MF-05).
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & UNIX_FILE_TYPE == UNIX_SYMLINK)
        {
            return Err(ApiError::BadRequest(format!(
                "the archive entry '{name}' is a symbolic link, and a bundle carries files"
            )));
        }
        // A complete export's README and schemas explain the bundle; they are not the project's
        // files, so they are never written into one (MF-41).
        if name == crate::api::export::README_PATH
            || name.starts_with(crate::api::export::SCHEMAS_DIR)
        {
            continue;
        }
        // One byte past what is left, so the ceiling is measured on what actually came out of
        // the decompressor and never on the size the archive declares for itself.
        let mut raw = Vec::new();
        if entry
            .by_ref()
            .take(budget + 1)
            .read_to_end(&mut raw)
            .is_err()
        {
            // A blob whose stream does not read back. An export counts these as omitted; an
            // import has nothing to write for them either.
            continue;
        }
        if raw.len() as u64 > budget {
            return Err(ApiError::BadRequest(format!(
                "the archive unpacks to more than the {MAX_ARCHIVE_BYTES} byte import limit"
            )));
        }
        budget -= raw.len() as u64;
        let Ok(content) = String::from_utf8(raw) else {
            // A binary blob committed beside the manifests, as above.
            continue;
        };
        incoming.extend(parse_text(&content, Some(&name))?);
    }
    Ok(incoming)
}

/// A document parsed with the key of a mapping read once and once only (MF-05).
///
/// Both parsers below build a [`Value`], and both let the last of two equal keys win: a
/// manifest whose `metadata` names `name: audited` and then `name: shadow` is reviewed as one
/// resource and written as another, which is a check made on one value and an action taken on
/// another. YAML's own `Mapping` refuses the repeat, but the moment the target type is a
/// `serde_json::Value` that refusal is gone, so the rule is carried here instead — one place,
/// both formats, every level of nesting.
struct Unique(Value);

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a manifest document")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Unique(Value::Null))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Unique(Value::Null))
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, inner: D) -> Result<Self::Value, D::Error> {
        Self::Value::deserialize(inner)
    }

    fn visit_newtype_struct<D: serde::Deserializer<'de>>(
        self,
        inner: D,
    ) -> Result<Self::Value, D::Error> {
        Self::Value::deserialize(inner)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Unique(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Unique(Value::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Unique(Value::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(Unique(Value::from(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Unique(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Unique(Value::String(value)))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or_default().min(64));
        while let Some(Unique(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Unique(Value::Array(items)))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut object = serde_json::Map::new();
        while let Some((key, Unique(value))) = map.next_entry::<String, Unique>()? {
            if object.insert(key.clone(), value).is_some() {
                return Err(serde::de::Error::custom(format!(
                    "the key '{key}' is set twice in the same mapping, and which of the two a \
                     reviewer would see is not the same question as which one would be written"
                )));
            }
        }
        Ok(Unique(Value::Object(object)))
    }
}

/// One text file as manifests, or as a native file that travels unread.
fn parse_text(text: &str, path: Option<&str>) -> Result<Vec<Incoming>, ApiError> {
    let manifest_path = path.is_none_or(|p| p.ends_with(".yaml") || p.ends_with(".yml"));
    if !manifest_path {
        return Ok(vec![Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: text.to_owned(),
        }]);
    }

    let mut documents: Vec<Value> = Vec::new();
    if text.trim_start().starts_with('{') {
        let value = Unique::deserialize(&mut serde_json::Deserializer::from_str(text))
            .map_err(|err| ApiError::BadRequest(format!("the JSON did not parse: {err}")))?;
        documents.push(value.0);
    } else {
        for document in serde_norway::Deserializer::from_str(text) {
            let value = Unique::deserialize(document)
                .map_err(|err| ApiError::BadRequest(format!("the YAML did not parse: {err}")))?;
            documents.push(value.0);
        }
    }

    // A top-level array is a list of manifests, which is what `jcctl import` sends and what a
    // hand-written multi-manifest JSON looks like.
    let documents: Vec<Value> = documents
        .into_iter()
        .flat_map(|document| match document {
            Value::Array(items) => items,
            other => vec![other],
        })
        .collect();

    // One document that is not a manifest is the file's own format — Bento's config, a LinkML
    // source — and it travels byte for byte (MF-17, T-0923). Re-serialising the parse would
    // drop the comments and the layout that are half of what those formats carry, and would
    // make the bundle's checksum disagree with what was written (MF-42).
    let single = documents.len() == 1;
    let mut incoming = Vec::new();
    for document in documents {
        if document.is_null() {
            continue;
        }
        // A `kind: List` is the JSON export format, and its items are the manifests.
        if document.get("kind").and_then(Value::as_str) == Some("List") {
            for item in document
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                incoming.push(as_incoming(item, path)?);
            }
            continue;
        }
        let mut item = as_incoming(&document, path)?;
        if single && item.envelope.is_none() {
            item.content = text.to_owned();
        }
        incoming.push(item);
    }
    if incoming.is_empty() && !text.trim().is_empty() {
        // A `.yaml` beside the manifests that is not a manifest: Bento's own config, a
        // LinkML source. It travels as it is.
        incoming.push(Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: text.to_owned(),
        });
    }
    Ok(incoming)
}

fn as_incoming(document: &Value, path: Option<&str>) -> Result<Incoming, ApiError> {
    let looks_like_manifest = document.get("kind").is_some() && document.get("metadata").is_some();
    if !looks_like_manifest {
        return Ok(Incoming {
            path: path.map(str::to_owned),
            envelope: None,
            content: serde_norway::to_string(document)
                .map_err(|e| ApiError::Internal(format!("document did not serialise: {e}")))?,
        });
    }
    let envelope: ResourceEnvelope = serde_json::from_value(document.clone()).map_err(|err| {
        ApiError::BadRequest(format!(
            "{}: not a manifest this platform serves: {err}",
            path.unwrap_or("the upload")
        ))
    })?;
    Ok(Incoming {
        path: path.map(str::to_owned),
        envelope: Some(envelope),
        content: String::new(),
    })
}

// --- the gates (MF-24) -------------------------------------------------------------------

/// Everything wrong with one manifest, in the order a person would read it.
fn refusals(envelope: &ResourceEnvelope, raw: &Value) -> Vec<String> {
    let name = &envelope.metadata.name;
    let mut refusals = Vec::new();
    if !jc_core::serves(&envelope.kind, &envelope.api_version) {
        refusals.push(format!(
            "{name}: apiVersion '{}' is not supported (expected '{}')",
            envelope.api_version,
            resource::API_VERSION
        ));
    }
    if resource::by_kind(&envelope.kind).is_none() {
        refusals.push(format!(
            "{name}: kind '{}' is not a resource this platform serves",
            envelope.kind
        ));
    }
    if let Err(problem) = jc_core::names::validate_dns1123_label(name) {
        refusals.push(format!("{name}: {problem}"));
    }
    if envelope.status.is_some() || raw.get("status").is_some() {
        refusals.push(format!(
            "{name}: status is computed by the platform and cannot be imported (MF-04)"
        ));
    }
    if let Some(key) = find_literal_secret(&envelope.spec) {
        refusals.push(format!(
            "{name}: literal secret in field '{key}'; use secretRef instead (MF-24)"
        ));
    }
    for key in crate::apps::converge::BUILT_ANNOTATIONS {
        if envelope.metadata.annotations.contains_key(key) {
            refusals.push(format!(
                "{name}: annotation '{key}' is written by this instance's build lane and cannot \
                 be imported; the image is built here (AP-11, AP-13a)"
            ));
        }
    }
    refusals
}

/// Field names whose *string* value names another manifest whose kind is fixed by the field.
///
/// A typed reference (`{ kind, name }`) says what it points at and is resolved generically.
/// A bare string does not, so only the fields whose kind is settled by the schema are checked
/// here; guessing at the rest would refuse valid bundles.
const STRING_REFS: &[(&str, &str)] = &[("contextSpaceRef", "ContextSpace")];

/// Every reference in one manifest, as `(kind, name)`.
fn references(spec: &Value, found: &mut BTreeSet<(String, String)>) {
    match spec {
        Value::Object(members) => {
            for (key, value) in members {
                // A secretRef names a Secret in the cluster, not a manifest in the repository.
                if key == "secretRef" || key == "apiTokenRef" {
                    continue;
                }
                if let Some((_, kind)) = STRING_REFS.iter().find(|(field, _)| field == key) {
                    if let Some(name) = value.as_str() {
                        found.insert(((*kind).to_owned(), name.to_owned()));
                        continue;
                    }
                }
                if key.ends_with("Ref") {
                    if let (Some(kind), Some(name)) = (
                        value.get("kind").and_then(Value::as_str),
                        value.get("name").and_then(Value::as_str),
                    ) {
                        found.insert((kind.to_owned(), name.to_owned()));
                        continue;
                    }
                }
                references(value, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| references(item, found)),
        _ => {}
    }
}

/// References the bundle names and neither it nor the project holds (MF-24).
fn unresolved(manifests: &[ResourceEnvelope], state: &AppState, project: &str) -> Vec<String> {
    let inside: BTreeSet<(String, String)> = manifests
        .iter()
        .map(|envelope| (envelope.kind.clone(), envelope.metadata.name.clone()))
        .collect();
    let mut missing = BTreeSet::new();
    for envelope in manifests {
        let mut named = BTreeSet::new();
        references(&envelope.spec, &mut named);
        for (kind, name) in named {
            if inside.contains(&(kind.clone(), name.clone())) {
                continue;
            }
            // A kind that lives in either place resolves in the project first, then the
            // organization: a project's own DataModel is no missing reference.
            let homes = resource::by_kind(&kind).map_or_else(
                || vec![namespace_for(&kind, project).to_owned()],
                |info| resource::homes(info, project),
            );
            if homes
                .iter()
                .any(|home| state.mirror.get(home, &kind, &name).is_some())
            {
                continue;
            }
            let options = if kind == "ContextSpace" {
                // The three honest outcomes of a copy whose space stays behind (T-1441).
                "; copy the space with it, map it onto a space of this project (spaceMapping), \
                 or use the source's data through a SharedSpaceReference"
            } else {
                ""
            };
            missing.insert(format!(
                "{}: {kind} '{name}' is in neither the bundle nor project '{project}'{options}",
                envelope.metadata.name
            ));
        }
    }
    missing.into_iter().collect()
}

// --- namespace remapping (MF-22) ---------------------------------------------------------

/// The spaces a bundle carries: every `ContextSpace` it holds and the space each other
/// manifest declares.
///
/// An id in one of those spaces is the bundle's own and travels with it; an id in any other
/// space names another organisation — a federated registration, a peer's endpoint — and is
/// left alone, or an import would quietly re-point federation at this city (MF-22, PF-43).
fn own_spaces(manifests: &[ResourceEnvelope]) -> BTreeSet<String> {
    let mut spaces = BTreeSet::new();
    for envelope in manifests {
        if envelope.kind == "ContextSpace" {
            spaces.insert(envelope.metadata.name.clone());
        }
        if let Some(space) = space_of(envelope) {
            spaces.insert(space.to_owned());
        }
    }
    spaces
}

/// Rewrites one manifest into the target namespace: the metadata, the namespace of every
/// typed reference, and the organisation domain of every id it carries.
///
/// The URN is the part that is easy to forget and expensive to get wrong. An id follows
/// `urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}` (PF-10), and the gateway refuses a write
/// whose `{orgDomain}` is not the one owning the space (PF-43), so a bundle imported into
/// another organisation keeps pointing at the organisation it came from unless the segment
/// moves with it. The `{space}` segment is a ContextSpace name, not a project name: it changes
/// only when the space itself is renamed by the conflict policy, which happens later, in
/// [`plan_import`].
fn remap(
    envelope: &mut ResourceEnvelope,
    from: &str,
    to: &str,
    domain: &str,
    spaces: &BTreeSet<String>,
) {
    // An organization-scoped kind lives in namespace `org` whatever project imported it;
    // jc-core refuses the manifest otherwise, so the target project would write a file its
    // own CI rejects (PF-22, MF-22).
    let declared = envelope.metadata.namespace.clone();
    envelope.metadata.namespace =
        Some(namespace_of(&envelope.kind, declared.as_deref(), to).to_owned());
    remap_value(&mut envelope.spec, from, to, domain, spaces);
    // A reference by name to an Endpoint of the project the bundle left follows it here; one
    // to another project of the organization keeps naming that project (EP-77, MF-43).
    if envelope.kind == "SharedSpaceReference" {
        if let Some(Value::String(project)) = envelope.spec.pointer_mut("/endpointRef/project") {
            if project.as_str() == from {
                *project = to.to_owned();
            }
        }
    }
}

/// The assigner every Policy of this repository writes: the loader renders it for the
/// organisation that owns the file (CC-82).
const OWN_ASSIGNER: &str = "did:web:{orgDomain}";

/// Rewrites every Policy's `assigner` that is not [`OWN_ASSIGNER`] to it, and answers what each
/// carried, keyed `Policy/{name}` by the name it lands under.
///
/// A Policy grants over a space of the project it lands in, so the organisation that may give
/// that data away is this one (R6); a DID from the bundle would read as another organisation's
/// decision. Rewritten rather than refused, so a migration keeps its grants, and reported, so the
/// person who approves the Change sees it (T-2256).
fn reassign_policies(manifests: &mut [ResourceEnvelope]) -> BTreeMap<String, String> {
    let mut reassigned = BTreeMap::new();
    for envelope in manifests
        .iter_mut()
        .filter(|envelope| envelope.kind == "Policy")
    {
        let Some(assigner) = envelope.spec.get_mut("assigner") else {
            continue;
        };
        if assigner.as_str() == Some(OWN_ASSIGNER) {
            continue;
        }
        let carried = match &*assigner {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        *assigner = Value::String(OWN_ASSIGNER.to_owned());
        reassigned.insert(format!("Policy/{}", envelope.metadata.name), carried);
    }
    reassigned
}

/// The namespace a kind is stored in: `org` for an organization-scoped kind, the project
/// otherwise. A kind that lives in either — `Role` — keeps `org` when the bundle wrote it
/// there and lands in the project when the bundle wrote it in one (PF-68, T-0820).
fn namespace_of<'a>(kind: &str, declared: Option<&str>, project: &'a str) -> &'a str {
    if resource::belongs_to_the_organization(kind, declared) {
        crate::permissions::ORG_NAMESPACE
    } else {
        project
    }
}

/// Where a reference to `kind` points, with nothing declared: what a typed reference means from
/// inside `project`.
fn namespace_for<'a>(kind: &str, project: &'a str) -> &'a str {
    namespace_of(kind, None, project)
}

fn remap_value(value: &mut Value, from: &str, to: &str, domain: &str, spaces: &BTreeSet<String>) {
    match value {
        Value::Object(members) => {
            // A typed reference that named the source namespace names the one it landed in;
            // an organization-scoped kind is addressed as `org` from anywhere.
            let referenced = members
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| namespace_for(kind, to).to_owned());
            if let (Some(target), Some(Value::String(namespace))) =
                (referenced, members.get_mut("namespace"))
            {
                if namespace.as_str() == from || target == crate::permissions::ORG_NAMESPACE {
                    *namespace = target;
                }
            }
            for child in members.values_mut() {
                remap_value(child, from, to, domain, spaces);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| remap_value(item, from, to, domain, spaces)),
        Value::String(text) => {
            if let Some(rewritten) = remap_urn(text, domain, spaces) {
                *text = rewritten;
            }
        }
        _ => {}
    }
}

/// One id, or one anchored `idPattern`, with its organisation domain replaced (PF-10, R33).
///
/// A pattern is a URN prefix with `^` in front and the dots of the domain escaped
/// (`^urn:ngsi-ld:AirQualityObserved:hel\.fi:air-quality:.*$`), which is why the comparison
/// unescapes and the replacement escapes: a Policy or a registration anchored on the source
/// organisation would otherwise keep routing there after the move (MF-22, T-0826).
fn remap_urn(urn: &str, domain: &str, spaces: &BTreeSet<String>) -> Option<String> {
    let pattern = urn.starts_with('^');
    let body = urn.strip_prefix('^').unwrap_or(urn);
    if !body.starts_with("urn:ngsi-ld:") {
        return None;
    }
    let mut segments: Vec<String> = body.split(':').map(str::to_owned).collect();
    // urn : ngsi-ld : Type : orgDomain : space : localId (or `.*$` in a pattern)
    if segments.len() < 6 {
        return None;
    }
    if !spaces.contains(&segments[4]) {
        return None;
    }
    let replacement = if pattern {
        regex::escape(domain)
    } else {
        domain.to_owned()
    };
    if segments[3] == replacement {
        return None;
    }
    segments[3] = replacement;
    let joined = segments.join(":");
    Some(if pattern {
        format!("^{joined}")
    } else {
        joined
    })
}

/// Whether a string is a domain name the URN scheme can carry: labels of letters, digits and
/// hyphens, separated by dots. A `:` would split an id into a segment nobody meant.
fn is_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Moves every id of a renamed space onto its new name (MF-23, MF-26).
fn rewrite_space(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::Object(members) => members
            .values_mut()
            .for_each(|child| rewrite_space(child, old, new)),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_space(item, old, new)),
        Value::String(text) => {
            let pattern = text.starts_with('^');
            let body = text.strip_prefix('^').unwrap_or(text);
            if !body.starts_with("urn:ngsi-ld:") {
                return;
            }
            let mut segments: Vec<String> = body.split(':').map(str::to_owned).collect();
            if segments.len() < 6 || segments[4] != old {
                return;
            }
            segments[4] = new.to_owned();
            let joined = segments.join(":");
            *text = if pattern {
                format!("^{joined}")
            } else {
                joined
            };
        }
        _ => {}
    }
}

/// Rewrites every reference to `old` so it names `new` (MF-23 `rename`, MF-26).
fn rewrite_reference(value: &mut Value, kind: &str, old: &str, new: &str) {
    match value {
        Value::Object(members) => {
            for (key, child) in members.iter_mut() {
                if let Some((_, fixed)) = STRING_REFS.iter().find(|(field, _)| field == key) {
                    if *fixed == kind && child.as_str() == Some(old) {
                        *child = Value::String(new.to_owned());
                        continue;
                    }
                }
                if key.ends_with("Ref")
                    && child.get("kind").and_then(Value::as_str) == Some(kind)
                    && child.get("name").and_then(Value::as_str) == Some(old)
                {
                    child["name"] = Value::String(new.to_owned());
                    continue;
                }
                rewrite_reference(child, kind, old, new);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_reference(item, kind, old, new)),
        _ => {}
    }
}

/// The name a renamed resource takes: the original suffixed with where it came from, so
/// importing the same bundle twice lands on the same name instead of `-1`, `-2`, `-3`.
///
/// The suffix is the origin and not the project being imported into: everything in the
/// project already belongs to the project, and `ovzdusie-helsinki` is the only form that
/// says which of the two spaces named `ovzdusie` this one is.
fn renamed(name: &str, origin: &str) -> String {
    let suffix: String = origin
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if suffix.is_empty() {
        format!("{name}-imported")
    } else {
        format!("{name}-{suffix}")
    }
}

// --- the endpoint -------------------------------------------------------------------------

/// The upload and the options, however the caller sent them.
///
/// One extractor consumes the body, so the dispatch is here rather than in the signature: a
/// wizard posts `multipart/form-data` with the file and the option fields beside it, and
/// `jcctl` posts JSON.
/// The multipart body of a git or App import: the archive as `file`, `parameters` as a JSON
/// object, `displayName`, `name` and `dryRun` (MF-45, CC-88, UI-87).
async fn read_git_request(
    state: &AppState,
    request: Request,
) -> Result<(Vec<u8>, crate::api::import_git::GitImport), ApiError> {
    let mut multipart = Multipart::from_request(request, state)
        .await
        .map_err(|err| {
            ApiError::BadRequest(format!(
                "a git import is multipart/form-data with the archive as 'file': {err}"
            ))
        })?;
    let mut bytes = Vec::new();
    let mut input = crate::api::import_git::GitImport::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::BadRequest(format!("the upload did not parse: {err}")))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        let data = field
            .bytes()
            .await
            .map_err(|err| ApiError::BadRequest(format!("field '{name}': {err}")))?;
        match name.as_str() {
            "file" | "bundle" => bytes = data.to_vec(),
            "parameters" => {
                input.parameters = serde_json::from_slice(&data).map_err(|err| {
                    ApiError::BadRequest(format!(
                        "parameters is a JSON object of values (CC-88): {err}"
                    ))
                })?
            }
            "displayName" => {
                input.display_name = Some(String::from_utf8_lossy(&data).trim().to_owned())
                    .filter(|name| !name.is_empty())
            }
            "name" => {
                input.name = Some(String::from_utf8_lossy(&data).trim().to_owned())
                    .filter(|name| !name.is_empty())
            }
            "dryRun" => {
                input.dry_run = matches!(String::from_utf8_lossy(&data).trim(), "true" | "All")
            }
            other => {
                return Err(ApiError::BadRequest(format!(
                    "a git import takes file, parameters, displayName, name and dryRun, not \
                     '{other}'"
                )))
            }
        }
    }
    if bytes.is_empty() {
        return Err(ApiError::BadRequest(
            "a git import carries the archive as 'file'".into(),
        ));
    }
    Ok((bytes, input))
}

async fn read_request(
    state: &AppState,
    request: Request,
) -> Result<(Vec<u8>, ImportOptions), ApiError> {
    let multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("multipart/"));

    if multipart {
        let mut multipart = Multipart::from_request(request, state)
            .await
            .map_err(|err| ApiError::BadRequest(format!("the upload did not parse: {err}")))?;
        let mut bytes: Vec<u8> = Vec::new();
        let mut options = ImportOptions::default();
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|err| ApiError::BadRequest(format!("the upload did not parse: {err}")))?
        {
            let name = field.name().unwrap_or_default().to_owned();
            let data = field
                .bytes()
                .await
                .map_err(|err| ApiError::BadRequest(format!("field '{name}': {err}")))?;
            match name.as_str() {
                "file" | "bundle" => bytes = data.to_vec(),
                "targetNamespace" => {
                    options.target_namespace = Some(String::from_utf8_lossy(&data).into_owned())
                }
                "orgDomain" => {
                    options.org_domain = Some(String::from_utf8_lossy(&data).trim().to_owned())
                }
                "conflictPolicy" => {
                    options.conflict_policy =
                        ConflictPolicy::parse(String::from_utf8_lossy(&data).trim())?
                }
                "dryRun" => {
                    options.dry_run =
                        matches!(String::from_utf8_lossy(&data).trim(), "true" | "All")
                }
                "url" => options.url = Some(String::from_utf8_lossy(&data).into_owned()),
                "spaceMapping" => {
                    options.space_mapping = serde_json::from_slice(&data).map_err(|err| {
                        ApiError::BadRequest(format!(
                            "spaceMapping is a list of {{from, to}}: {err}"
                        ))
                    })?
                }
                _ => {}
            }
        }
        return Ok((bytes, options));
    }

    let Json(body) = Json::<Value>::from_request(request, state)
        .await
        .map_err(|_| {
            ApiError::BadRequest("send a multipart file or a JSON body with 'manifests'".into())
        })?;
    let options: ImportOptions = serde_json::from_value(body.clone())
        .map_err(|err| ApiError::BadRequest(format!("the import options did not parse: {err}")))?;
    let manifests = match options.manifests.clone() {
        Some(manifests) => manifests,
        // A bare manifest posted as the whole body, which is what `jcctl import` sends.
        None if body.get("kind").is_some() => body.clone(),
        None => return Ok((Vec::new(), options)),
    };
    let text = serde_json::to_vec(&manifests)
        .map_err(|err| ApiError::Internal(format!("the manifests did not serialise: {err}")))?;
    Ok((text, options))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/import",
    summary = "Import A Bundle",
    description = "Imports a bundle into the project as one change a person approves, or answers the plan alone.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project the bundle is imported into"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' to validate and plan without proposing"),
        ("format" = Option<String>, Query, description = "'git': the archive of a format=git export, landing as the new project of the path (layout 2); 'app': the archive of an App export, landing as a new App of the project (UI-87)"),
    ),
    request_body(
        content(
            (ImportOptions = "application/json", example = json!({
                "manifests": [{
                    "apiVersion": "joinedcontext.com/v1alpha1",
                    "kind": "Group",
                    "metadata": { "name": "stewards", "namespace": "org" },
                    "spec": { "members": [] }
                }],
                "conflictPolicy": "fail"
            })),
            // The parts are `file` (the bundle or the git archive) and these options as text.
            (ImportOptions = "multipart/form-data", example = json!({ "conflictPolicy": "fail" })),
        ),
        description = "The manifests as JSON, or a bundle file beside these options as a form",
    ),
    responses(
        (status = 202, description = "One merge request for the whole bundle", body = Change),
        (status = 200, description = "Dry run: what the import would do", body = ImportReport),
        (status = 400, description = "The bundle was refused", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "format=git by anybody who may not open a project (PF-65), format=git or format=app by anybody who does not administer the organization (UI-87)", body = ProblemDetails),
        (status = 409, description = "A resource already exists and the policy is 'fail', or the bundle has no fresh check of its own (`verdict_required`, PF-57)", body = ProblemDetails),
        (status = 501, description = "Importing from a URL is not implemented", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn import(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ImportQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    if !resource::is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // Who may not import learns that before the body is read: a refused caller cannot make the
    // Portal buffer an upload of the organization's size (F-POR2-005).
    match query.format.as_deref() {
        None => {}
        Some("git") => {
            // Who may not open a project learns that and nothing else (PF-65).
            crate::api::projects::may_open(&state, &user.0.identity)?;
            crate::permissions::require_organization_admin(
                &state,
                &user.0.identity,
                "importing a project",
            )?;
        }
        Some("app") => {
            // As for a project (UI-87): who may not read the project learns nothing more, who
            // does not administer the organization is refused.
            if !crate::permissions::for_request(&state, &user.0.identity, &project)
                .may_read_project()
            {
                return Err(ApiError::NotFound(format!("project '{project}' not found")));
            }
            crate::permissions::require_organization_admin(
                &state,
                &user.0.identity,
                "importing an application",
            )?;
        }
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "format '{other}' is neither git nor app; a bundle or manifests import without one"
            )))
        }
    }
    // The organization's upload size judges the body as it arrived: a JSON import is re-encoded
    // before the bundle is read, and a git archive is unpacked (ADR-N-035).
    let (parts, body) = request.into_parts();
    let raw = axum::body::to_bytes(body, MAX_UPLOAD_BYTES + 64 * 1024)
        .await
        .map_err(|err| ApiError::BadRequest(format!("the upload could not be read: {err}")))?;
    crate::api::organization_limits::within_upload_limit(&state, raw.len())?;
    let request = Request::from_parts(parts, axum::body::Body::from(raw));
    if query.format.is_some() {
        let (bytes, mut input) = read_git_request(&state, request).await?;
        input.dry_run |=
            query.dry_run.as_deref() == Some("All") || query.validate_only == Some(true);
        let (status, body) = if query.format.as_deref() == Some("app") {
            crate::api::app_transfer::import(&state, &user.0.identity, &project, &bytes, input)
                .await?
        } else {
            crate::api::import_git::import(&state, &user.0.identity, &project, &bytes, input)
                .await?
        };
        return Ok((status, Json(body)).into_response());
    }
    let (bytes, mut options) = read_request(&state, request).await?;
    if query.dry_run.as_deref() == Some("All") || query.validate_only == Some(true) {
        options.dry_run = true;
    }
    let (status, body) = import_bundle(&state, &user.0.identity, &project, &bytes, options).await?;
    Ok((status, Json(body)).into_response())
}

/// One bundle imported into a project: the plan when it is a dry run, the `Change` otherwise
/// (MF-18…MF-21). The route and the `jc_project_import` operation both call this, so an MCP
/// client and a browser import the same way (AG-59, T-0840).
pub async fn import_bundle(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    bytes: &[u8],
    options: ImportOptions,
) -> Result<(StatusCode, Value), ApiError> {
    let project = project.to_owned();
    if options.url.is_some() {
        // MF-20 allows a URL, and fetching one means the Portal opening a connection to a
        // host a caller named. That is an egress decision with no policy behind it yet, and
        // inventing one here would be inventing an SSRF surface.
        return Err(ApiError::NotImplemented(
            "importing from a URL needs an egress policy that does not exist yet; upload the \
             bundle instead (MF-20)"
                .into(),
        ));
    }

    crate::api::organization_limits::within_upload_limit(state, bytes.len())?;
    let incoming = parse(bytes)?;
    authorize(state, identity, &project, &incoming)?;
    well_formed(&incoming, state.config.apps_dir.as_deref())?;
    let target = options
        .target_namespace
        .clone()
        .unwrap_or_else(|| project.clone());
    if !resource::is_dns1123(&target) {
        return Err(ApiError::BadRequest(format!(
            "targetNamespace '{target}' is not a DNS-1123 label"
        )));
    }
    if target != project {
        // The repository path of every kind starts `projects/{project}/`, so a namespace
        // that is not the project would write manifests the reconciler never reads.
        return Err(ApiError::BadRequest(format!(
            "targetNamespace '{target}' must be the project '{project}'"
        )));
    }

    // The organisation whose ids the import writes: this instance's own, unless the caller
    // names another (a staging instance replaying a city's bundle under its own domain).
    let domain = match options.org_domain.clone().filter(|d| !d.trim().is_empty()) {
        Some(domain) => {
            if !is_domain(&domain) {
                return Err(ApiError::BadRequest(format!(
                    "orgDomain '{domain}' is not a domain name (PF-10)"
                )));
            }
            domain
        }
        None => crate::api::assistant::org_domain(state, &project),
    };

    // A space mapped onto is one the caller may read in this project; the refusal is the same
    // whether it exists or not, so nobody learns of a space they may not read (T-1441, R20).
    let reads_spaces =
        crate::permissions::for_request(state, identity, &project).may_read("ContextSpace");
    for mapping in &options.space_mapping {
        let there = resource::is_dns1123(&mapping.from)
            && resource::is_dns1123(&mapping.to)
            && reads_spaces
            && state
                .mirror
                .get(&project, "ContextSpace", &mapping.to)
                .is_some();
        if !there {
            return Err(ApiError::BadRequest(format!(
                "spaceMapping: '{}' is not a space of project '{project}' you may read",
                mapping.to
            )));
        }
    }

    let (report, files) = plan_import(
        &incoming, state, identity, &project, &target, &domain, &options,
    )?;
    // PF-57 on the import door (T-1460): the dry run is the bundle's check, over the bundle as
    // sent and the options it is imported with, so another manifest, policy or domain is stale;
    // the import itself needs that check before anything reaches the forge. Not over the planned
    // files: a plan mints a fresh endpoint slug each time, so no two plans are equal.
    let subject = import_subject(
        bytes,
        options.conflict_policy,
        &domain,
        &target,
        &options.space_mapping,
    );
    if options.dry_run {
        crate::ops::record_import_check(state, identity, &project, &subject).await;
        return Ok((
            StatusCode::OK,
            serde_json::to_value(report).map_err(|e| ApiError::Internal(e.to_string()))?,
        ));
    }
    if files.is_empty() {
        return Err(ApiError::BadRequest(
            "the bundle holds nothing to import".into(),
        ));
    }
    crate::ops::verdict_for_import(state, identity, &project, &subject).await?;
    let headline = incoming.iter().find_map(|item| {
        item.envelope
            .as_ref()
            .map(|envelope| (envelope.kind.clone(), envelope.metadata.name.clone()))
    });
    let change = propose_bundle(
        state,
        identity,
        &project,
        report,
        files,
        headline.as_ref().map(|(k, n)| (k.as_str(), n.as_str())),
    )
    .await?;
    crate::ops::forget_import_check(state, identity, &project).await;
    Ok((
        StatusCode::ACCEPTED,
        serde_json::to_value(change).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

/// What an import's check is fresh for: the SHA-256 of the bundle as sent, and the options that
/// decide what it writes.
fn import_subject(
    bytes: &[u8],
    policy: ConflictPolicy,
    domain: &str,
    target: &str,
    space_mapping: &[SpaceMapping],
) -> Value {
    use sha2::{Digest, Sha256};
    let bundle: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    serde_json::json!({
        "bundle": bundle,
        "conflictPolicy": policy,
        "orgDomain": domain,
        "targetNamespace": target,
        "spaceMapping": space_mapping,
    })
}

/// Commits a bundle of files and manifests as one merge request (MF-21, CC-63). `headline`
/// is the manifest (kind, name) Approvals shows for the bundle: the branch is named
/// `portal/create-{kind}-{name}-{hash}`, the form the change list reads, so the whole merge
/// request can be listed, approved and merged from the Portal.
/// ponytail: the change's plan counts that one manifest; a bundle-aware Change (every
/// manifest in the plan) is the upgrade.
pub async fn propose_bundle(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    report: ImportReport,
    files: Vec<(String, String)>,
    headline: Option<(&str, &str)>,
) -> Result<Change, ApiError> {
    let hash = digest(&files);
    let branch = match headline {
        Some((kind, name)) => format!(
            "portal/create-{}-{name}-{hash:08x}",
            kind.to_ascii_lowercase()
        ),
        None => format!("portal/import-{project}-{hash:08x}"),
    };
    let title = format!("import {} resources into {project}", files.len());
    let detail = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "imported bundle".into());
    // The one line an approver reads before the report: whether the transfer arrived whole
    // (MF-42). A bundle without checksums has no such line rather than a reassuring one.
    let lead: Vec<String> = report
        .verification_summary()
        .into_iter()
        .chain(report.reassignment_lines())
        .collect();
    let body = if lead.is_empty() {
        detail
    } else {
        format!("{}\n\n{detail}", lead.join("\n"))
    };
    let summary = crate::change::PlanSummary::new(
        report.created.len(),
        report.replaced.len() + report.renamed.len(),
        0,
    );
    propose_files(
        state,
        identity,
        project,
        Proposal {
            files,
            branch,
            title,
            body,
            lane: report.lane,
            summary,
        },
    )
    .await
}

/// One merge request of several files, which a person approves as one Change.
pub struct Proposal {
    /// `(repository path, content)` of every file the merge request writes.
    pub files: Vec<(String, String)>,
    /// The branch, `portal/{verb}-…`, which is how the change list recognises it.
    pub branch: String,
    /// The merge request's title and its one commit's message.
    pub title: String,
    /// Its description: what the reviewer reads before the diff.
    pub body: String,
    /// The riskiest lane of what it writes (CC-63).
    pub lane: Lane,
    /// What the plan counts.
    pub summary: crate::change::PlanSummary,
}

/// The forge half of [`propose_bundle`], for every door that writes several files as one Change
/// in a person's name: the files go onto `proposal.branch` as one commit with `identity` as its
/// author, so all of them land or none does, and one merge request carries them.
pub async fn propose_files(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    proposal: Proposal,
) -> Result<Change, ApiError> {
    let Proposal {
        files,
        branch,
        title,
        body,
        lane,
        summary,
    } = proposal;
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let default_branch = gitea.default_branch().await?;
    let held = crate::api::mutate::turn(gitea, &branch).await;
    let branch = create_or_reuse_branch(gitea, &held, &default_branch).await?;

    let (author_name, author_email) = author_credentials(identity, project);
    gitea
        .change_files(
            &branch,
            &title,
            Author {
                name: &author_name,
                email: &author_email,
            },
            &files,
            &[],
        )
        .await?;

    let pull = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;
    Ok(waiting_change(state, gitea, project, lane, summary, pull))
}

/// The open merge request `pull` as the Change a person follows.
pub(crate) fn waiting_change(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    project: &str,
    lane: Lane,
    summary: crate::change::PlanSummary,
    pull: crate::git::PullRequest,
) -> Change {
    Change::new(
        crate::api::changes::change_meta(state, gitea, pull.number, project),
        ChangeStatus::new(lane, ChangePhase::PendingApproval, summary)
            .in_repository(&pull.repository)
            .proposed_through(&pull.body)
            .with_merge_request(pull.url),
    )
}

/// What the bundle index says each of its files hashes to, against what arrived (MF-42).
///
/// The comparison is made on the manifest as it was uploaded — before the namespace mapping,
/// the domain rewrite and the `imported-from` annotation this import applies — because that is
/// the written manifest with the mapping undone, and it is the form the exporter hashed. A
/// bundle with no `spec.files` answers an empty list: an import that cannot verify a transfer
/// says nothing about it rather than reporting every file equal.
fn verify(incoming: &[Incoming]) -> Result<Vec<Verified>, ApiError> {
    use sha2::{Digest, Sha256};

    let expected: BTreeMap<String, String> = incoming
        .iter()
        .filter_map(|item| item.envelope.as_ref())
        .find(|envelope| envelope.kind == BUNDLE_KIND)
        .and_then(|index| index.spec.get("files").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| {
            let path = file.get("path")?.as_str()?.to_owned();
            let sha256 = file.get("sha256")?.as_str()?.to_owned();
            Some((path, sha256))
        })
        .collect();
    if expected.is_empty() {
        return Ok(Vec::new());
    }

    let mut verified = Vec::new();
    for item in incoming {
        let Some(path) = item.path.as_deref() else {
            continue;
        };
        let Some(wanted) = expected.get(path) else {
            continue;
        };
        let bytes = match &item.envelope {
            Some(envelope) if envelope.kind == BUNDLE_KIND => continue,
            Some(envelope) => serde_norway::to_string(envelope)
                .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?,
            // A native file travels byte for byte, so the bytes that arrived are the bytes
            // that will be written.
            None => item.content.clone(),
        };
        let found = format!("{:x}", Sha256::digest(bytes.as_bytes()));
        verified.push(Verified {
            path: path.to_owned(),
            equal: &found == wanted,
        });
    }
    // A file the index lists and the upload does not hold was lost on the way: it is unequal,
    // not absent from the report (MF-42).
    for path in expected.keys() {
        if !verified.iter().any(|file| &file.path == path) {
            verified.push(Verified {
                path: path.clone(),
                equal: false,
            });
        }
    }
    verified.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(verified)
}

/// What the import would do, and the files it would write.
#[allow(clippy::type_complexity)]
fn plan_import(
    incoming: &[Incoming],
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    target: &str,
    domain: &str,
    options: &ImportOptions,
) -> Result<(ImportReport, Vec<(String, String)>), ApiError> {
    let (policy, space_mapping) = (options.conflict_policy, options.space_mapping.as_slice());
    let mut manifests: Vec<ResourceEnvelope> = Vec::new();
    let mut natives: Vec<(String, String)> = Vec::new();
    let mut source: Option<String> = None;
    let mut refused: Vec<String> = Vec::new();
    let verified = verify(incoming)?;

    for item in incoming {
        let Some(envelope) = item.envelope.clone() else {
            if let Some(path) = &item.path {
                natives.push((path.clone(), item.content.clone()));
            }
            continue;
        };
        if envelope.kind == BUNDLE_KIND {
            // The index describes the bundle, so it is provenance and not a resource (MF-20).
            // The index is the platform's `Bundle` (T-0823): the project is its name and the
            // revision is `sourceRevision`. An archive downloaded from an older Portal carries
            // `spec.project` and `spec.revision` instead, and still says where it came from.
            let from = envelope
                .spec
                .get("project")
                .and_then(Value::as_str)
                .unwrap_or(&envelope.metadata.name)
                .to_owned();
            let revision = envelope
                .spec
                .get("sourceRevision")
                .or_else(|| envelope.spec.get("revision"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            source = Some(format!("{from}@{revision}"));
            continue;
        }
        let raw = serde_json::to_value(&envelope)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
        refused.extend(refusals(&envelope, &raw));
        manifests.push(envelope);
    }
    if !refused.is_empty() {
        return Err(ApiError::BadRequest(refused.join("; ")));
    }
    if manifests.is_empty() && natives.is_empty() {
        return Err(ApiError::BadRequest(
            "the upload holds no manifest this platform serves".into(),
        ));
    }

    // The destination project is the one in the URL, and its own manifest already describes
    // it: a `Project` carried by the bundle would be written at `projects/{its own name}/`,
    // a directory of this repository that belongs to another project (PF-22, MF-22).
    manifests.retain(|envelope| envelope.kind != "Project");

    // The source namespace is whatever the bundle was exported from; a bundle whose manifests
    // disagree about it is remapped from each one's own, which is what a hand-assembled
    // multi-project bundle needs.
    // The origin travels with each manifest because a rename is named after it.
    let spaces = own_spaces(&manifests);
    let manifests: Vec<(String, ResourceEnvelope)> = manifests
        .into_iter()
        .map(|mut envelope| {
            let from = envelope
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| target.to_owned());
            remap(&mut envelope, &from, target, domain, &spaces);
            // A mapped space is not copied: its references and the `{space}` segment of its
            // ids land on the target's space (T-1441, PF-84).
            for mapping in space_mapping {
                let old = crate::spaces::segment(&state.mirror, &from, &mapping.from);
                let new = crate::spaces::segment(&state.mirror, project, &mapping.to);
                rewrite_reference(
                    &mut envelope.spec,
                    "ContextSpace",
                    &mapping.from,
                    &mapping.to,
                );
                rewrite_space(&mut envelope.spec, &old, &new);
                // A bundle from elsewhere, or a space that predates rendered segments, writes
                // the space's own name as the segment.
                rewrite_space(&mut envelope.spec, &mapping.from, &new);
            }
            (from, envelope)
        })
        .collect();

    let mut report = ImportReport {
        created: Vec::new(),
        replaced: Vec::new(),
        skipped: Vec::new(),
        renamed: BTreeMap::new(),
        reassigned: BTreeMap::new(),
        native_files: natives.len(),
        lane: Lane::Green,
        source,
        verified,
        needs: Vec::new(),
    };

    // Conflicts first: a rename rewrites references, so it has to happen before anything is
    // resolved or planned.
    let mut keep: Vec<ResourceEnvelope> = Vec::new();
    for (origin, envelope) in manifests {
        let stored = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let existing = state
            .mirror
            .get(stored, &envelope.kind, &envelope.metadata.name)
            .is_some();
        if !existing {
            report.created.push(envelope.metadata.name.clone());
            keep.push(envelope);
            continue;
        }
        match policy {
            ConflictPolicy::Fail => {
                return Err(ApiError::Conflict(format!(
                    "{} '{}' already exists in project '{project}'; choose skip, replace or \
                     rename (MF-23)",
                    envelope.kind, envelope.metadata.name
                )))
            }
            ConflictPolicy::Skip => report.skipped.push(envelope.metadata.name.clone()),
            ConflictPolicy::Replace => {
                report.replaced.push(envelope.metadata.name.clone());
                keep.push(envelope);
            }
            ConflictPolicy::Rename => {
                let old = envelope.metadata.name.clone();
                let stem = renamed(&old, &origin);
                let mut new = stem.clone();
                let mut attempt = 2;
                while state.mirror.get(stored, &envelope.kind, &new).is_some() {
                    new = format!("{stem}-{attempt}");
                    attempt += 1;
                }
                if !resource::is_dns1123(&new) {
                    return Err(ApiError::BadRequest(format!(
                        "renaming '{old}' would produce '{new}', which is not a name this \
                         platform can store; rename it in the source project first (MF-23)"
                    )));
                }
                report.renamed.insert(old.clone(), new.clone());
                let mut envelope = envelope;
                envelope.metadata.name = new;
                keep.push(envelope);
            }
        }
    }
    for (old, new) in &report.renamed {
        let kind = keep
            .iter()
            .find(|envelope| &envelope.metadata.name == new)
            .map(|envelope| envelope.kind.clone());
        if let Some(kind) = kind {
            for envelope in &mut keep {
                rewrite_reference(&mut envelope.spec, &kind, old, new);
                // A renamed ContextSpace is the one rename that also moves ids: the `{space}`
                // segment of every URN is that name (PF-10, MF-26).
                if kind == "ContextSpace" {
                    rewrite_space(&mut envelope.spec, old, new);
                }
            }
        }
    }

    // The slug is the endpoint's public address and the platform mints it, per environment
    // (EP-02, EP-75, CC-74). A bundle carries the source's, so an import that kept it would
    // serve the destination's dataset at the source's capability URL — and two imports of one
    // bundle would answer at one address, where the gateway's table keeps only the last
    // (T-0821). The one slug that survives is the one this instance already minted for this
    // endpoint, so updating a project by re-importing its bundle does not move its endpoints
    // under the people using them.
    for envelope in &mut keep {
        if envelope.kind != "Endpoint" {
            continue;
        }
        let stored = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let held = state
            .mirror
            .get(stored, "Endpoint", &envelope.metadata.name)
            .and_then(|existing| {
                existing
                    .spec
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let slug = held.unwrap_or_else(|| generate_slug().to_string());
        if let Some(spec) = envelope.spec.as_object_mut() {
            spec.insert("slug".to_owned(), Value::String(slug));
        }
    }

    report.reassigned = reassign_policies(&mut keep);
    report.needs = needs_of(&keep, project);

    let missing = unresolved(&keep, state, project);
    if !missing.is_empty() {
        return Err(ApiError::BadRequest(missing.join("; ")));
    }

    // Every App role comes with its default group (AP-118), as at the propose door, and like
    // that door's grants the groups ride without raising the lane: an empty group grants only
    // its App role. An import only adds files, so the group of a role the bundle takes away
    // stays until the App is next proposed, which removes it. A project in its own repository
    // (layout 2) cannot carry an organization file in its Change.
    let mut default_groups: Vec<ResourceEnvelope> = Vec::new();
    if state.mirror.repository_of(project).is_none() {
        let bundled: std::collections::BTreeSet<String> = keep
            .iter()
            .filter(|envelope| envelope.kind == "Group")
            .map(|envelope| envelope.metadata.name.clone())
            .collect();
        for envelope in keep.iter_mut().filter(|envelope| envelope.kind == "App") {
            let planned = crate::apps::default_groups::plan(state, identity, project, envelope)?;
            default_groups.extend(
                planned
                    .written
                    .into_iter()
                    .filter(|group| !bundled.contains(&group.metadata.name)),
            );
        }
    }

    let mut files: Vec<(String, String)> = Vec::new();
    for envelope in &keep {
        let info = resource::by_kind(&envelope.kind)
            .ok_or_else(|| ApiError::Internal(format!("kind '{}' vanished", envelope.kind)))?;
        let operation = if state
            .mirror
            .get(
                namespace_of(
                    &envelope.kind,
                    envelope.metadata.namespace.as_deref(),
                    project,
                ),
                &envelope.kind,
                &envelope.metadata.name,
            )
            .is_some()
        {
            Operation::Update
        } else {
            Operation::Create
        };
        report.lane = riskiest(
            report.lane,
            change::classify(info.kind, operation, &envelope.spec),
        );

        // The manifest's own namespace decides where it lands, which is what keeps an
        // organization role in `users/` and a project's own role in its project (PF-68).
        let home = namespace_of(
            &envelope.kind,
            envelope.metadata.namespace.as_deref(),
            project,
        );
        let path =
            resource::repository_path(info, home, space_of(envelope), &envelope.metadata.name)
                .map_err(ApiError::BadRequest)?;
        let mut committed = envelope.clone();
        committed.strip_status();
        annotate(&mut committed, report.source.as_deref());
        let content = serde_norway::to_string(&committed)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
        files.push((path, content));
    }
    let group = resource::by_kind("Group")
        .ok_or_else(|| ApiError::Internal("no catalogue entry for Group".into()))?;
    for envelope in default_groups {
        let path = resource::repository_path(
            group,
            crate::permissions::ORG_NAMESPACE,
            None,
            &envelope.metadata.name,
        )
        .map_err(ApiError::BadRequest)?;
        let content = serde_norway::to_string(&envelope)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
        report.created.push(envelope.metadata.name);
        files.push((path, content));
    }
    // A native file keeps the path it had in the archive, rewritten into this project.
    for (path, content) in natives {
        files.push((reproject(&path, project), content));
    }
    Ok((report, files))
}

/// Who may propose what the bundle holds (T-0798, PF-50): the caller's bindings in this
/// project, checked per manifest before anything is planned, so a dry run discloses nothing
/// either. `mutate::propose_with_identity` asks the same two questions of a single manifest.
fn authorize(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    incoming: &[Incoming],
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    for item in incoming {
        match &item.envelope {
            Some(envelope) if envelope.kind == BUNDLE_KIND => {}
            Some(envelope) => authorize_manifest(state, identity, project, &effective, envelope)?,
            None => {
                let Some(path) = &item.path else { continue };
                // A native file is proposed under the kind its directory names; a path that
                // names no kind has no role that could grant it.
                let kind = native_kind(&reproject(path, project)).ok_or_else(|| {
                    ApiError::Denied(format!(
                        "'{path}' belongs to no kind this platform serves, so no role grants \
                         proposing it (PF-50)"
                    ))
                })?;
                effective.check(kind, jc_core::kinds::Verb::Propose, None)?;
            }
        }
    }
    Ok(())
}

/// Whether `identity` may propose this one manifest in `project`, with every refusal the
/// single-manifest door makes: the grant for its kind, space and constraints, a Policy or a
/// binding within the proposer's own rights, a Pipeline's rights, and the space, App name and
/// group checks (PF-50, PF-84). An import asks it of each manifest; a restore and an undo of the
/// manifests they put back (T-5698).
pub(crate) fn authorize_manifest(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    effective: &crate::permissions::Effective,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    let raw = serde_json::to_value(envelope)
        .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?;
    effective.check(&envelope.kind, jc_core::kinds::Verb::Propose, Some(&raw))?;
    crate::permissions::within_own_rights(state, identity, &raw, None, "proposer")?;
    crate::pipeline_rights::check(
        state,
        identity,
        project,
        &raw,
        jc_core::kinds::Verb::Propose,
    )?;
    // PF-84, AP-14a, AP-114, AP-115: the same refusals as the single-manifest door.
    crate::spaces::check(
        state,
        identity,
        project,
        &envelope.kind,
        &envelope.metadata.name,
        &envelope.spec,
    )?;
    crate::apps::names::check(
        state,
        identity,
        project,
        &envelope.kind,
        &envelope.metadata.name,
    )?;
    crate::groups::check(state, identity, &envelope.kind, &envelope.metadata)
}

/// The kind's own parse and invariants, for every manifest the bundle carries (T-2253, CC-08).
///
/// `mutate::propose_engine` asks this of a single manifest, so a form and `jc_resource_propose`
/// refuse what `jcctl apply` would refuse on `main`. The import door asked only who may propose,
/// and a `ModelProjection` naming `dataModelRef.version: 1.1.0` went through it into a Change on
/// 2026-09-18: the approver read an ordinary plan, and the reconciler has refused the file on every
/// tick since (DM-22). Checked before anything is planned, so a dry run says the same thing as the
/// proposal, and the file it came from is named — a bundle holds many manifests.
fn well_formed(incoming: &[Incoming], apps_dir: Option<&str>) -> Result<(), ApiError> {
    for item in incoming {
        let Some(envelope) = &item.envelope else {
            continue;
        };
        if envelope.kind == BUNDLE_KIND {
            continue;
        }
        // The shipped bundle a published static App may stand on instead of a repository has
        // to be on this Portal, not on the one the bundle was exported from (AP-87).
        if let Some(refusal) = crate::apps::static_host::unshipped_claim(apps_dir, envelope) {
            return Err(ApiError::BadRequest(refusal));
        }
        let Some(checked) = jc_core::registry::validate_yaml(
            &envelope.kind,
            &serde_json::to_string(envelope)
                .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))?,
        ) else {
            // A kind the Portal serves and jc-core has no type for has nothing to check against.
            continue;
        };
        checked.map_err(|e| {
            let named = item
                .path
                .clone()
                .unwrap_or_else(|| format!("{}/{}", envelope.kind, envelope.metadata.name));
            ApiError::BadRequest(format!(
                "{named}: spec is not a valid {}: {e}",
                envelope.kind
            ))
        })?;
    }
    Ok(())
}

/// The kind a native file belongs to: the plural directory after `projects/{project}/`, or
/// after `spaces/{space}/` for the kinds that live under a space.
/// The context space a repository path lies in: `projects/{project}/spaces/{space}/…`.
pub(crate) fn space_in_path(path: &str) -> Option<&str> {
    let mut segments = path.split('/');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("projects"), Some(_), Some("spaces"), Some(space)) if !space.is_empty() => {
            segments.next().map(|_| space)
        }
        _ => None,
    }
}

/// Whether the manifest at `manifest_path`, named `name`, owns the native file at `native`: in the
/// same directory, and either the resource that directory is (`pipelines/aq/pipeline.yaml` for
/// `pipelines/aq/bento.yaml`) or the one the file is named after (`datamodels/air.yaml` for
/// `datamodels/air.linkml.yaml`).
pub(crate) fn owns_native(native: &str, manifest_path: &str, name: &str) -> bool {
    let (Some((dir, file)), Some((manifest_dir, _))) =
        (native.rsplit_once('/'), manifest_path.rsplit_once('/'))
    else {
        return false;
    };
    let dir_name = dir.rsplit('/').next().unwrap_or_default();
    manifest_dir == dir && (dir_name == name || file.starts_with(&format!("{name}.")))
}

pub(crate) fn native_kind(path: &str) -> Option<&'static str> {
    let mut segments = path.split('/').skip(2);
    let first = segments.next()?;
    let plural = if first == "spaces" {
        segments.nth(1)?
    } else {
        first
    };
    resource::by_plural(plural).map(|info| info.kind)
}

/// The space a manifest lives under, for the kinds whose path has a `{space}` segment.
///
/// `spec.contextSpaceRef` is the answer for every kind that has one, and it is the right
/// answer: an Endpoint of the air quality space belongs under `spaces/ovzdusie/`, not under a
/// directory named after the project. The label is the override for a kind that has no
/// `contextSpaceRef` but still lives under a space, and the namespace is the last resort.
fn space_of(envelope: &ResourceEnvelope) -> Option<&str> {
    envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .map(String::as_str)
        .or_else(|| match envelope.spec.get("contextSpaceRef") {
            Some(Value::String(name)) => Some(name.as_str()),
            Some(reference) => reference.get("name").and_then(Value::as_str),
            None => None,
        })
        .or(envelope.metadata.namespace.as_deref())
}

/// The riskier of two lanes. One merge request carries the whole bundle, so it has to be
/// reviewed at the level of the riskiest thing in it (CC-63).
pub(crate) fn riskiest(left: Lane, right: Lane) -> Lane {
    let rank = |lane: Lane| match lane {
        Lane::Green => 0,
        Lane::Yellow => 1,
        Lane::Red => 2,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

/// Tags an imported manifest with where it came from (MF-20, MF-08).
fn annotate(envelope: &mut ResourceEnvelope, source: Option<&str>) {
    let value = source.unwrap_or("upload").to_owned();
    envelope
        .metadata
        .annotations
        .insert(jc_core::annotations::IMPORTED_FROM.to_owned(), value);
}

/// `projects/<anything>/rest` becomes `projects/<project>/rest`, so an archive from another
/// instance lands in this project rather than recreating the one it was exported from.
fn reproject(path: &str, project: &str) -> String {
    let mut segments = path.split('/');
    match (segments.next(), segments.next()) {
        (Some("projects"), Some(_)) => {
            format!(
                "projects/{project}/{}",
                segments.collect::<Vec<_>>().join("/")
            )
        }
        _ => format!("projects/{project}/{path}"),
    }
}

/// A branch name that is the same for the same bundle, so a retry after a failed forge call
/// reuses the branch instead of leaving one behind per attempt.
pub(crate) fn digest(files: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, content) in files {
        path.hash(&mut hasher);
        content.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn router() -> Router<AppState> {
    // axum reads 2 MiB by default; the import's own limit, plus the multipart framing, is the
    // one that holds (MF-45).
    Router::new().route(
        "/projects/{project}/import",
        axum::routing::post(import).layer(axum::extract::DefaultBodyLimit::max(
            MAX_UPLOAD_BYTES + 64 * 1024,
        )),
    )
}

#[cfg(test)]
mod verification_tests {
    use super::{ImportReport, Verified};
    use crate::change::Lane;

    fn report(verified: Vec<Verified>) -> ImportReport {
        ImportReport {
            created: Vec::new(),
            replaced: Vec::new(),
            skipped: Vec::new(),
            renamed: std::collections::BTreeMap::new(),
            reassigned: std::collections::BTreeMap::new(),
            native_files: 0,
            lane: Lane::Green,
            source: None,
            verified,
            needs: Vec::new(),
        }
    }

    fn file(path: &str, equal: bool) -> Verified {
        Verified {
            path: path.to_owned(),
            equal,
        }
    }

    /// CC-84: two secret references, a person and a feed credential are four needs, each
    /// where it is, and none carries a value.
    #[test]
    fn what_a_copy_cannot_carry_is_listed_where_it_is_and_never_as_a_value() {
        let manifest =
            |kind: &str, name: &str, spec: serde_json::Value| crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, "espoo"),
                spec,
                status: None,
            };
        let bundle = vec![
            manifest(
                "DataSource",
                "feed",
                serde_json::json!({
                    "type": "http",
                    "http": {
                        "url": "https://example.invalid/feed",
                        "authorization": { "headerRef": { "name": "feed", "key": "token" } }
                    },
                    "secrets": [{ "name": "feed", "key": "password", "envVar": "PASSWORD" }],
                }),
            ),
            manifest(
                "RoleBinding",
                "stewards",
                serde_json::json!({ "subjects": [{ "user": "demo.steward@hel.fi" }, { "group": "g" }], "role": "steward" }),
            ),
            manifest(
                "ContextSpace",
                "air",
                serde_json::json!({ "isSandbox": false }),
            ),
        ];
        let needs = super::needs_of(&bundle, "espoo");
        let kinds: Vec<&str> = needs.iter().map(|need| need.kind.as_str()).collect();
        assert_eq!(kinds, ["secret", "secret", "person"], "{needs:?}");
        assert_eq!(
            needs[0].location,
            "DataSource/feed spec.http.authorization.headerRef"
        );
        assert_eq!(needs[1].location, "DataSource/feed spec.secrets");
        assert_eq!(
            needs[2].location,
            "RoleBinding/stewards spec.subjects[0].user"
        );
        assert!(needs[2].why.contains("demo.steward@hel.fi"));
        assert!(needs
            .iter()
            .all(|need| need.link.starts_with("/projects/espoo/")));
        let said = serde_json::to_string(&needs).expect("serializes");
        assert!(
            !said.contains("\"token\"") && !said.contains("password"),
            "{said}"
        );
        assert!(
            super::needs_of(&bundle[2..], "espoo").is_empty(),
            "a space alone needs nothing"
        );
    }

    /// EP-77: a reference by name to the project the bundle left follows it; one to another
    /// project of the organization keeps naming that project.
    #[test]
    fn an_endpoint_ref_follows_the_project_it_named_and_no_other() {
        let reference = |project: &str| crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "SharedSpaceReference".to_owned(),
            metadata: crate::resource::ObjectMeta::new("city-bikes", "helsinki-mobility"),
            spec: serde_json::json!({ "endpointRef": { "project": project, "name": "bikes" }, "alias": "a" }),
            status: None,
        };
        let spaces = std::collections::BTreeSet::new();
        let mut own = reference("helsinki-mobility");
        super::remap(
            &mut own,
            "helsinki-mobility",
            "espoo-mobility",
            "espoo.fi",
            &spaces,
        );
        assert_eq!(own.spec["endpointRef"]["project"], "espoo-mobility");
        assert_eq!(own.metadata.namespace.as_deref(), Some("espoo-mobility"));
        let mut other = reference("helsinki");
        super::remap(
            &mut other,
            "helsinki-mobility",
            "espoo-mobility",
            "espoo.fi",
            &spaces,
        );
        assert_eq!(other.spec["endpointRef"]["project"], "helsinki");
    }

    /// MF-42: the line the change body leads with counts what matched out of what was checked.
    #[test]
    fn the_summary_counts_what_matched_out_of_what_was_checked() {
        let files: Vec<Verified> = (0..10)
            .map(|index| file(&format!("f{index}.yaml"), index != 3))
            .collect();
        assert_eq!(
            report(files).verification_summary().as_deref(),
            Some("9 of 10 files equal")
        );
    }

    #[test]
    fn a_transfer_that_all_arrived_says_so_in_the_same_words() {
        assert_eq!(
            report(vec![file("a.yaml", true), file("b.yaml", true)])
                .verification_summary()
                .as_deref(),
            Some("2 of 2 files equal")
        );
    }

    /// A bundle with no checksums is unverifiable, and says nothing rather than something
    /// reassuring: `0 of 0 files equal` would read as a verified transfer.
    #[test]
    fn a_bundle_that_carried_no_checksums_says_nothing_about_verification() {
        assert_eq!(report(Vec::new()).verification_summary(), None);
    }
}

/// The upload gates of CC-08 and MF-05: what an archive, a YAML stream or a single document may
/// be before any of it is planned, remapped or written.
///
/// These run on the parser alone, which is the point: every one of them is a refusal that has to
/// happen before a forge call, a branch or a merge request exists, so the place to prove them is
/// the funnel every upload passes through rather than a route that has already authorised a
/// caller. The attack they replay is T-1704 — path traversal, an absolute path, a symbolic link,
/// a zip bomb, ten thousand files, an alias that expands, a key set twice and a document larger
/// than the limit.
#[cfg(test)]
mod upload_gate_tests {
    use super::*;
    use std::io::Write;

    /// A manifest small enough to sit beside whatever the case under test is really about.
    const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: ovzdusie\n  namespace: helsinki\nspec:\n  title: { en: Air }\n";

    fn zipped(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            writer.start_file(*name, options).expect("an entry");
            writer.write_all(body).expect("the body");
        }
        writer.finish().expect("the archive").into_inner()
    }

    fn refusal(bytes: &[u8]) -> String {
        match parse(bytes) {
            Err(ApiError::BadRequest(why)) => why,
            Err(other) => panic!("refused as {other:?} rather than a bad request"),
            Ok(incoming) => panic!("{} entries were accepted", incoming.len()),
        }
    }

    /// CC-08, MF-05 (T-1704): a bundle that is small on the wire and enormous once unpacked is
    /// refused on what came out of the decompressor, not on what the archive declared.
    #[test]
    fn an_archive_that_expands_past_the_import_limit_is_refused() {
        let payload = vec![b'a'; MAX_ARCHIVE_BYTES as usize + 1];
        let archive = zipped(&[("projects/helsinki/spaces/a/space.yaml", &payload)]);
        assert!(
            archive.len() < MAX_UPLOAD_BYTES,
            "the archive has to pass the upload limit for the unpacked one to be what refuses it"
        );
        let why = refusal(&archive);
        assert!(why.contains("unpacks to more than"), "{why}");
    }

    /// CC-08, MF-05 (T-1704): the same bomb spread over many entries, each one within any
    /// per-file ceiling, is refused on the sum.
    #[test]
    fn entries_that_add_up_past_the_import_limit_are_refused_together() {
        let each = vec![b'b'; 4 * 1024 * 1024];
        // Each entry under the limit, together past it, whatever the limit is.
        let count = MAX_ARCHIVE_BYTES as usize / each.len() + 1;
        let names: Vec<String> = (0..count)
            .map(|index| format!("projects/helsinki/pipelines/p{index}/bento.yaml"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), each.as_slice()))
            .collect();
        let why = refusal(&zipped(&entries));
        assert!(why.contains("unpacks to more than"), "{why}");
    }

    /// CC-08, MF-05 (T-1704): a symbolic link is refused by name. Read as a file it becomes a
    /// manifest whose body is the path it pointed at.
    #[test]
    fn an_archive_entry_that_is_a_symbolic_link_is_refused_by_name() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .add_symlink(
                "projects/helsinki/spaces/a/space.yaml",
                "../../../../etc/passwd",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("a link");
        let archive = writer.finish().expect("the archive").into_inner();
        let why = refusal(&archive);
        assert!(why.contains("symbolic link"), "{why}");
        assert!(
            why.contains("space.yaml"),
            "the refusal names the entry: {why}"
        );
    }

    /// CC-08, MF-05 (T-1704): a path that climbs out of the archive root, and one that starts at
    /// the filesystem root, are both refused before an entry is read.
    #[test]
    fn an_archive_path_that_leaves_its_root_is_refused() {
        for name in [
            "projects/helsinki/../../../etc/cron.d/x.yaml",
            "/etc/cron.d/x.yaml",
            "../x.yaml",
        ] {
            let why = refusal(&zipped(&[(name, SPACE.as_bytes())]));
            assert!(why.contains("escaping path"), "{name}: {why}");
        }
    }

    /// T-4382: zip 9 reads `/etc/x` as the enclosed `etc/x`; every caller of `enclosed_path`
    /// (bundle import, git import, sync checkout) still refuses an absolute or escaping name.
    #[test]
    fn an_absolute_or_escaping_entry_name_has_no_enclosed_path() {
        let archive = zipped(&[
            ("/etc/cron.d/x", b"x"),
            ("\\etc\\x", b"x"),
            ("C:/x", b"x"),
            ("a/../../x", b"x"),
            ("projects/helsinki/space.yaml", b"x"),
        ]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(archive)).expect("a zip");
        let paths: Vec<Option<String>> = (0..archive.len())
            .map(|i| {
                let entry = archive.by_index(i).expect("an entry");
                enclosed_path(&entry).map(|p| p.to_string_lossy().into_owned())
            })
            .collect();
        assert_eq!(
            paths,
            [
                None,
                None,
                None,
                None,
                Some("projects/helsinki/space.yaml".to_owned())
            ]
        );
    }

    /// CC-08 (T-1704): ten thousand files are refused on the count, before any of them is opened.
    #[test]
    fn an_archive_of_more_entries_than_the_limit_is_refused() {
        let names: Vec<String> = (0..=MAX_ARCHIVE_ENTRIES)
            .map(|index| format!("projects/helsinki/spaces/s{index}/space.yaml"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), b"x" as &[u8]))
            .collect();
        let why = refusal(&zipped(&entries));
        assert!(why.contains("entry limit"), "{why}");
    }

    /// CC-08 (T-1704): a document larger than the upload limit is refused before it is parsed.
    #[test]
    fn an_upload_larger_than_the_import_limit_is_refused() {
        let why = refusal(&vec![b'c'; MAX_UPLOAD_BYTES + 1]);
        assert!(why.contains("import limit"), "{why}");
    }

    /// MF-05 (T-1704): a YAML alias that expands into a billion nodes is refused by the parser's
    /// repetition budget rather than expanded into memory.
    #[test]
    fn a_yaml_alias_that_expands_without_end_is_refused() {
        let mut yaml = String::from("a: &a [x, x, x, x, x, x, x, x, x]\n");
        for (index, name) in "bcdefghi".chars().enumerate() {
            let previous = "abcdefgh".chars().nth(index).expect("a previous anchor");
            let aliases = std::iter::repeat_n(format!("*{previous}"), 9)
                .collect::<Vec<_>>()
                .join(", ");
            yaml.push_str(&format!("{name}: &{name} [{aliases}]\n"));
        }
        yaml.push_str("kind: ContextSpace\nmetadata: { name: x }\n");
        let started = std::time::Instant::now();
        let why = refusal(yaml.as_bytes());
        assert!(why.contains("repetition limit"), "{why}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the refusal has to come from the budget, not from finishing the expansion"
        );
    }

    /// MF-05 (T-1704): a document nested deeper than the parser's recursion budget is refused
    /// rather than overflowing the stack that walks it.
    #[test]
    fn a_yaml_document_nested_without_end_is_refused() {
        let depth = 50_000;
        let yaml = format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata: {{ name: x }}\nspec: {}{}\n",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        let why = refusal(yaml.as_bytes());
        assert!(why.contains("recursion limit"), "{why}");
    }

    /// MF-05 (T-1704): a key set twice is refused in YAML and in JSON alike. The reviewer reads
    /// one of the two values and the writer takes the other, which is a check made on one value
    /// and an action taken on another.
    #[test]
    fn a_key_set_twice_in_one_mapping_is_refused_in_both_formats() {
        let yaml = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: audited\n  name: shadow\n  namespace: helsinki\nspec: {}\n";
        let why = refusal(yaml.as_bytes());
        assert!(why.contains("set twice"), "{why}");
        assert!(why.contains("name"), "the refusal names the key: {why}");

        let json = r#"{"apiVersion":"joinedcontext.com/v1alpha1","kind":"ContextSpace","metadata":{"name":"audited","name":"shadow"}}"#;
        let why = refusal(json.as_bytes());
        assert!(why.contains("set twice"), "{why}");
    }

    /// CC-08, MF-05 (T-1704): the counterpart, so every refusal above is read as a limit and not
    /// as a parser that stopped working — a bundle inside every ceiling still parses whole.
    #[test]
    fn a_bundle_within_every_ceiling_still_parses() {
        let archive = zipped(&[
            (
                "projects/helsinki/spaces/ovzdusie/space.yaml",
                SPACE.as_bytes(),
            ),
            (
                "projects/helsinki/pipelines/aq/bento.yaml",
                b"input:\n  mqtt:\n    urls: [ mqtts://mqtt.example:8883 ]\n",
            ),
        ]);
        let incoming = parse(&archive).expect("a bundle inside the limits parses");
        assert_eq!(incoming.len(), 2);
        assert!(
            incoming.iter().any(|item| item
                .envelope
                .as_ref()
                .is_some_and(|envelope| envelope.metadata.name == "ovzdusie")),
            "the manifest is read as one"
        );
        assert!(
            incoming
                .iter()
                .any(|item| item.envelope.is_none() && item.content.contains("mqtt")),
            "a native file travels byte for byte"
        );
    }
}
