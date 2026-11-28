//! A write in two calls (AG-149, API/14 §4.3, T-5956): `jc_change_plan` runs each change with the
//! resource API's dry run and keeps the result as a plan bound to its caller and to the etag of
//! every object it touches; `jc_change_apply` opens the change requests (or commits into the
//! workspace) only for that plan, unchanged, within 30 minutes. Neither approves anything: a
//! person approves at the change's page (AG-11, AG-151).
//!
//! Through a connector, a plan names only objects its scope reaches, and an apply needs the
//! connector's `writes` switch and `write` on every item (AG-144). A plan that fails a check is
//! answered 422 with every finding and is not kept, so a plan never holds a value the write path
//! refuses, a credential typed into a manifest included (MF-24, S-29).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use jc_time::TimestampTz;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::bounds::{manifest, text, NAME, TERM};
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::api::datamodels::{self, ModelChange, SourceDryRunResult};
use crate::api::delete::{self, DeleteOutcome};
use crate::api::mutate::{self, ProposeOutcome};
use crate::change::{Lane, Operation as ChangeOp};
use crate::connectors::{APPLY, PLAN, REVERT};
use crate::error::ApiError;
use crate::state::AppState;
use crate::tools::model_tools::MAX_REQUEST_BYTES;
use jc_core::kinds::ConnectorVerb;

/// How long a plan may wait for its apply (AG-149).
pub const TTL_MINUTES: i64 = 30;
/// What apply does, said in the plan's `next` so a model that planned what the person asked for
/// applies it instead of asking again (T-6041): the change request changes nothing live.
pub const APPLY_OPENS: &str = "apply opens a change request and changes nothing live; a person reviews and approves it in the Portal, so when the person asked for this change, apply it without asking them again";
/// Changes one plan carries (API/14 §4.3).
const MAX_CHANGES: u64 = 50;
/// Plans one Portal keeps at once; the oldest goes first when a new one would pass it.
const MAX_PLANS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    Create,
    Replace,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeInput {
    op: Op,
    kind: String,
    name: String,
    #[serde(default)]
    manifest: Option<Value>,
    #[serde(default)]
    patch: Option<Value>,
    /// With a DataModel: the whole LinkML file `spec.linkml` names (S-10, S-15).
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanInput {
    changes: Vec<ChangeInput>,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ApplyInput {
    plan_id: String,
}

/// One change of a kept plan: where it goes and what the object was when it was planned.
#[derive(Debug, Clone)]
struct Planned {
    kind: String,
    plural: &'static str,
    home: String,
    name: String,
    /// The whole manifest to propose; `None` for a delete.
    manifest: Option<Value>,
    base: String,
    /// A DataModel planned with its LinkML file, checked as the source route checks it.
    model: Option<ModelPlan>,
}

/// The LinkML half of a DataModel change, and the published file it was checked against.
#[derive(Debug, Clone)]
struct ModelPlan {
    source: String,
    checked: SourceDryRunResult,
    next_val: Value,
    imports: BTreeMap<String, String>,
    creating: bool,
    /// [`source_etag`] of the published file at plan time.
    base: String,
}

#[derive(Debug, Clone)]
struct Plan {
    owner: String,
    connector: Option<String>,
    project: String,
    workspace: Option<String>,
    items: Vec<Planned>,
    expires_at: TimestampTz,
}

/// The plans waiting for their apply, in memory.
// ponytail: one replica (Deployment/02); plans live 30 minutes and a restart loses them, which a
// client answers by planning again. A second replica moves this into Postgres beside connectors.
#[derive(Debug, Default)]
pub struct PlanStore {
    plans: Mutex<HashMap<String, Plan>>,
}

impl PlanStore {
    fn keep(&self, id: String, plan: Plan, now: TimestampTz) {
        let mut plans = self.plans.lock().unwrap_or_else(|p| p.into_inner());
        plans.retain(|_, p| p.expires_at > now);
        let oldest = plans
            .iter()
            .min_by_key(|(_, p)| p.expires_at)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest.filter(|_| plans.len() >= MAX_PLANS) {
            plans.remove(&oldest);
        }
        plans.insert(id, plan);
    }

    /// Takes the plan out, so it is applied once: only for its own caller, connector and
    /// project, before it expires. Anyone else is told it is not there, which tells them nothing
    /// about it, and a call naming another project leaves it in place (F-R2POR4-017).
    fn take(
        &self,
        id: &str,
        owner: &str,
        project: &str,
        connector: Option<&str>,
        now: TimestampTz,
    ) -> Option<Plan> {
        let mut plans = self.plans.lock().unwrap_or_else(|p| p.into_inner());
        let mine = plans.get(id).is_some_and(|p| {
            p.owner == owner
                && p.project == project
                && p.connector.as_deref() == connector
                && p.expires_at > now
        });
        mine.then(|| plans.remove(id)).flatten()
    }

    /// What a kept plan changes, read without taking it, for its own person in its own project
    /// (T-5959): the Portal assistant opens the editor on it. A plan made through any of the
    /// person's connectors is theirs to look at; only its apply stays bound to that connector.
    pub fn peek(
        &self,
        id: &str,
        owner: &str,
        project: &str,
        now: TimestampTz,
    ) -> Option<Vec<PlanItem>> {
        let plans = self.plans.lock().unwrap_or_else(|p| p.into_inner());
        let plan = plans
            .get(id)
            .filter(|p| p.owner == owner && p.project == project && p.expires_at > now)?;
        Some(
            plan.items
                .iter()
                .map(|item| PlanItem {
                    kind: item.kind.clone(),
                    home: item.home.clone(),
                    name: item.name.clone(),
                    manifest: item.manifest.clone(),
                    source: item.model.as_ref().map(|model| model.source.clone()),
                })
                .collect(),
        )
    }
}

/// One change of a kept plan as [`PlanStore::peek`] shows it: `manifest` is `None` for a delete.
#[derive(Debug, Clone)]
pub struct PlanItem {
    pub kind: String,
    pub home: String,
    pub name: String,
    pub manifest: Option<Value>,
    /// A DataModel's planned LinkML source, when the plan changes its file (JC2-11).
    pub source: Option<String>,
}

/// What an object is now, as a plan compares it: its metadata and spec (a status the reconciler
/// writes moves no etag), or `absent`.
fn etag(state: &AppState, home: &str, kind: &str, name: &str) -> String {
    match state.mirror.get(home, kind, name) {
        None => "absent".to_owned(),
        Some(envelope) => {
            let bytes = serde_json::to_vec(&json!({
                "metadata": envelope.metadata,
                "spec": envelope.spec,
            }))
            .unwrap_or_default();
            format!("{:x}", Sha256::digest(bytes))
        }
    }
}

/// What the published LinkML file of a DataModel is now: its hash, or `absent`. An apply over a
/// file that moved since the plan would overwrite someone's change.
async fn source_etag(state: &AppState, home: &str, name: &str) -> Result<String, OpError> {
    if state.mirror.get(home, "DataModel", name).is_none() {
        return Ok("absent".to_owned());
    }
    match datamodels::read_source(state, home, name).await {
        Ok(text) => Ok(format!("{:x}", Sha256::digest(text.as_bytes()))),
        Err(ApiError::NotFound(_)) => Ok("absent".to_owned()),
        Err(err) => Err(OpError::Api(err)),
    }
}

/// 26 random base32 characters, like a connector id: not guessable.
fn new_id() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0u8; 26];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 31)]))
        .collect()
}

fn invalid(path: String, message: String) -> OpError {
    OpError::InvalidInput { path, message }
}

/// The connector's half of one change (AG-144, AG-149).
fn through_connector(
    caller: &Caller,
    project: &str,
    kind: &str,
    name: &str,
    space: Option<&str>,
    write: bool,
) -> Result<(), OpError> {
    match caller.access.as_ref().and_then(|a| a.connector()) {
        Some(connector) => connector
            .may_change(project, kind, name, space, write)
            .map_err(|why| OpError::Api(ApiError::Denied(why))),
        None => Ok(()),
    }
}

/// The connector's verb on one change (T-6302): its own `kinds` list, and for a `/c/{id}`
/// connector the DelegationPolicy ceiling over the project, unless an approval stands for it.
fn verb_through_connector(
    caller: &Caller,
    state: &AppState,
    project: &str,
    kind: &str,
    name: &str,
    verb: ConnectorVerb,
) -> Result<(), OpError> {
    let Some(connector) = caller.access.as_ref().and_then(|a| a.connector()) else {
        return Ok(());
    };
    let denied = |why: String| OpError::Api(ApiError::Denied(why));
    connector.may_verb(kind, name, verb).map_err(denied)?;
    // A Portal conversation is no delegation: only the `/c/{id}` door carries the ceiling.
    if !matches!(caller.via, super::Via::Mcp) {
        return Ok(());
    }
    let ceiling =
        crate::connectors::Ceiling::of(&state.mirror, &caller.identity).with_approved(connector);
    if ceiling.allows_change(project, kind, name, verb) {
        return Ok(());
    }
    Err(denied(format!(
        "{} does not let a connector {} {kind} {name} in project {project}; an administrator widens the policy",
        ceiling.named(),
        verb.word()
    )))
}

/// Both connector checks on one change: its scope (`write` at apply) and its verb. `target` is
/// the kind, the name and where the change sits ([`crate::connectors::change_space`]).
fn connector_admits(
    caller: &Caller,
    state: &AppState,
    project: &str,
    (kind, name, space): (&str, &str, Option<&str>),
    write: bool,
    verb: ConnectorVerb,
) -> Result<(), OpError> {
    through_connector(caller, project, kind, name, space, write)?;
    verb_through_connector(caller, state, project, kind, name, verb)
}

/// The verb a planned change needs: delete, or edit when the forge holds the object, else create.
fn change_verb(op: &Op, exists: bool) -> ConnectorVerb {
    match (op, exists) {
        (Op::Delete, _) => ConnectorVerb::Delete,
        (_, true) => ConnectorVerb::Edit,
        (_, false) => ConnectorVerb::Create,
    }
}

/// The verb a plan item applies: no manifest deletes, an `absent` base creates, else it edits.
fn item_verb(has_manifest: bool, base: &str) -> ConnectorVerb {
    match (has_manifest, base) {
        (false, _) => ConnectorVerb::Delete,
        (true, "absent") => ConnectorVerb::Create,
        (true, _) => ConnectorVerb::Edit,
    }
}

/// The manifest a change proposes: the one given, or the object now with the patch merged in.
fn manifest_of(
    state: &AppState,
    at: &str,
    home: &str,
    change: &ChangeInput,
) -> Result<Option<Value>, OpError> {
    let current = state.mirror.get(home, &change.kind, &change.name);
    let exists = current.is_some();
    let named = |what: &str| given(at, change, what);
    let missing = || {
        invalid(
            format!("{at}/name"),
            format!(
                "{} '{}' does not exist in project {home}: use op create",
                change.kind, change.name
            ),
        )
    };
    let manifest = match change.op {
        Op::Create if exists => {
            return Err(invalid(
                format!("{at}/op"),
                format!(
                    "{} '{}' exists already: use op replace or patch",
                    change.kind, change.name
                ),
            ))
        }
        Op::Create => named("manifest")?,
        Op::Replace if !exists => return Err(missing()),
        Op::Replace => named("manifest")?,
        Op::Patch => patched(
            at,
            change,
            serde_json::to_value(current.ok_or_else(missing)?)?,
        )?,
        Op::Delete if !exists => return Err(missing()),
        Op::Delete if change.manifest.is_some() || change.patch.is_some() => {
            return Err(invalid(
                at.to_owned(),
                "op delete takes neither manifest nor patch".into(),
            ))
        }
        Op::Delete => return Ok(None),
    };
    names_the_change(at, home, change, &manifest)?;
    Ok(Some(manifest))
}

/// The object now, without its status, with the change's patch merged in; a patch that only
/// carries a LinkML source leaves the manifest as it is.
fn patched(at: &str, change: &ChangeInput, mut whole: Value) -> Result<Value, OpError> {
    let patch = match (&change.patch, &change.source) {
        (None, Some(_)) => json!({}),
        _ => given(at, change, "patch")?,
    };
    if let Some(map) = whole.as_object_mut() {
        map.remove("status");
    }
    crate::plan::merge_patch(&mut whole, &patch);
    Ok(whole)
}

/// The manifest or the patch a change carries, refused when it carries the other one too.
fn given(at: &str, change: &ChangeInput, what: &str) -> Result<Value, OpError> {
    match (what, &change.manifest, &change.patch) {
        ("manifest", Some(m), None) => Ok(m.clone()),
        ("patch", None, Some(p)) => Ok(p.clone()),
        _ => Err(invalid(
            format!("{at}/{what}"),
            format!("op {:?} takes {what} alone", change.op).to_lowercase(),
        )),
    }
}

/// The manifest names the change's kind and object, in the namespace the object lives in.
fn names_the_change(
    at: &str,
    home: &str,
    change: &ChangeInput,
    manifest: &Value,
) -> Result<(), OpError> {
    for (member, want) in [("kind", &change.kind), ("metadata/name", &change.name)] {
        if manifest
            .pointer(&format!("/{member}"))
            .and_then(Value::as_str)
            != Some(want)
        {
            return Err(invalid(
                format!("{at}/manifest/{member}"),
                format!("the manifest's {member} must be the change's, '{want}'"),
            ));
        }
    }
    // A kind kept in the organization's namespace (a RoleBinding) is written there whatever
    // project the call names: say where, at the member, before the dry run's generic mismatch.
    if let Some(namespace) = manifest
        .pointer("/metadata/namespace")
        .and_then(Value::as_str)
        .filter(|ns| !ns.is_empty() && *ns != home)
    {
        return Err(invalid(
            format!("{at}/manifest/metadata/namespace"),
            format!(
                "{} lives in namespace '{home}', not '{namespace}': set metadata.namespace to \
                 '{home}' or leave it out",
                change.kind
            ),
        ));
    }
    Ok(())
}

/// One change checked: its diff, its verdict and what the plan keeps of it.
async fn checked(
    caller: &Caller,
    state: &AppState,
    project: &str,
    at: &str,
    change: &ChangeInput,
    workspace: Option<&str>,
) -> Result<(Value, Planned), OpError> {
    let info = super::resources::kind_named(&change.kind)?;
    if change.source.is_some() {
        source_fits(at, info.kind, change, workspace)?;
    }
    let home = super::resources::home_holding(state, info, project, &change.name);
    // Create or edit is the forge's answer, never the `op` the client sent (T-6302).
    let current = state.mirror.get(&home, info.kind, &change.name);
    let verb = change_verb(&change.op, current.is_some());
    // The connector is asked before a manifest error can tell it what exists outside its scope.
    let manifest = manifest_of(state, at, &home, change);
    let space = crate::connectors::change_space(
        current.and_then(|c| serde_json::to_value(c).ok()).as_ref(),
        manifest.as_ref().ok().and_then(Option::as_ref),
    );
    let target = (info.kind, change.name.as_str(), space.as_deref());
    connector_admits(caller, state, project, target, false, verb)?;
    let mut manifest = manifest?;
    // The slug is the platform's to mint (EP-75): a typed one is refused under this change's own
    // path here, before the check below mints one and records it on the Endpoint's draft.
    slug_settled(caller, state, at, &home, manifest.clone().as_mut()).await?;
    let base = etag(state, &home, info.kind, &change.name);
    let answer = match &manifest {
        Some(m) => {
            let check = super::find("jc_manifest_dry_run").ok_or(OpError::Api(
                ApiError::Internal("jc_manifest_dry_run is not registered".into()),
            ))?;
            (check.run)(caller, state, &home, json!({ "manifest": m })).await?
        }
        None => removal_dry_run(caller, state, &home, info.plural, &change.name, workspace).await?,
    };
    // The slug the check minted, from the draft it recorded, so the change proposes what was
    // checked.
    slug_settled(caller, state, at, &home, manifest.as_mut()).await?;
    let mut answer = answer;
    if let (Some(m), "Pipeline") = (&manifest, info.kind) {
        // The dry run answers a verdict with its findings, a refused check's included.
        let warnings = super::replace_check::warnings(state, project, &home, m).await;
        let mut findings: Vec<Value> =
            serde_json::from_value(answer["verdict"]["findings"].take()).unwrap_or_default();
        findings.extend(warnings);
        answer["verdict"]["findings"] = Value::Array(findings);
    }
    super::endpoint_reach::warn_outside(
        &mut answer,
        state,
        &caller.identity,
        &home,
        manifest.as_ref(),
    );
    // A source never rides on a removal (`source_fits`), so a model change carries a manifest.
    let model = match (&change.source, &manifest) {
        (Some(_), Some(m)) => Some(model_checked(caller, state, &home, change, m).await?),
        _ => None,
    };
    // Where a person opens this change in the Portal (S-41): the kind's editor on the object, or
    // its create form; the change itself is applied with the plan, never by following the link.
    let portal = match change.op {
        Op::Create => crate::agents::change::create_route(project, info.kind, info.plural),
        op => crate::agents::change::route(
            project,
            info.kind,
            info.plural,
            &change.name,
            op == Op::Delete,
        ),
    };
    let mut diff = json!({
        "op": change.op_word(),
        "portal": crate::agents::change::address(&state.config, &portal),
        "kind": info.kind,
        "name": change.name,
        "lane": answer.get("lane"),
        "plan": answer.get("plan"),
        "goesWith": answer.get("goesWith"),
        "restartsStream": answer.get("restartsStream"),
        "verdict": answer.get("verdict"),
    });
    let model = match model {
        None => None,
        Some((shown, Err(findings))) => {
            diff["model"] = shown;
            diff["verdict"]["ok"] = json!(false);
            let mut all: Vec<Value> =
                serde_json::from_value(diff["verdict"]["findings"].take()).unwrap_or_default();
            all.extend(findings);
            diff["verdict"]["findings"] = Value::Array(all);
            None
        }
        Some((shown, Ok(model))) => {
            diff["lane"] = shown["lane"].clone();
            diff["model"] = shown;
            Some(model)
        }
    };
    let planned = Planned {
        kind: info.kind.to_owned(),
        plural: info.plural,
        home,
        name: change.name.clone(),
        manifest,
        base,
        model,
    };
    Ok((diff, planned))
}

async fn slug_settled(
    caller: &Caller,
    state: &AppState,
    at: &str,
    home: &str,
    manifest: Option<&mut Value>,
) -> Result<(), OpError> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    // The check records its draft outside every workspace, so the slug is read there.
    crate::endpoint_slug::settle_in(state, home, None, caller.brings_its_own_slug(), manifest)
        .await
        .map_err(|message| {
            invalid(
                format!("{at}/manifest{}", crate::endpoint_slug::PATH),
                message,
            )
        })
}

/// A LinkML source rides only on a DataModel change that is no removal and no workspace commit.
fn source_fits(
    at: &str,
    kind: &str,
    change: &ChangeInput,
    workspace: Option<&str>,
) -> Result<(), OpError> {
    let refuse = |why: &str| Err(invalid(format!("{at}/source"), why.to_owned()));
    if kind != "DataModel" {
        return refuse("only a DataModel change carries a LinkML source");
    }
    if change.op == Op::Delete {
        return refuse("op delete takes no source");
    }
    if workspace.is_some() {
        return refuse("a model's source opens its own change request: plan it without workspace");
    }
    Ok(())
}

/// The dry run of a removal, in the shape `jc_manifest_dry_run` answers.
async fn removal_dry_run(
    caller: &Caller,
    state: &AppState,
    home: &str,
    plural: &'static str,
    name: &str,
    workspace: Option<&str>,
) -> Result<Value, OpError> {
    Ok(
        match delete::delete_with_identity(
            &caller.identity,
            state,
            home,
            plural,
            name,
            true,
            workspace,
        )
        .await?
        {
            DeleteOutcome::DryRun(result) => {
                let mut out = serde_json::to_value(&result)?;
                out["verdict"] = json!({ "ok": result.valid, "findings": [] });
                out
            }
            DeleteOutcome::Referenced { here, elsewhere } => json!({
                "valid": false,
                "verdict": { "ok": false, "findings": [{
                    "level": "error",
                    "path": "",
                    "message": DeleteOutcome::conflict_message(&here, elsewhere),
                }] }
            }),
            DeleteOutcome::Proposed(_) | DeleteOutcome::Workspace(_) => {
                return Err(OpError::Api(ApiError::Internal(
                    "a dry run of a removal wrote something".into(),
                )))
            }
        },
    )
}

/// The LinkML half of a DataModel change (S-10, S-15): the check `PUT .../source` runs (no
/// literal secret, versioning, ranges that name a defined class, Model Tools), and for each
/// breaking change the stored entities of the classes it touches. Answers what the diff shows,
/// and the plan's half or the findings that refuse it.
async fn model_checked(
    caller: &Caller,
    state: &AppState,
    home: &str,
    change: &ChangeInput,
    manifest: &Value,
) -> Result<(Value, Result<ModelPlan, Vec<Value>>), OpError> {
    let source = change.source.as_deref().unwrap_or_default();
    let current = state.mirror.get(home, "DataModel", &change.name);
    let creating = current.is_none();
    let spec = current.map_or_else(|| manifest["spec"].clone(), |envelope| envelope.spec);
    let wanted = manifest.pointer("/spec/version").and_then(Value::as_str);
    let version = match (creating, wanted) {
        (true, wanted) => Some(wanted.unwrap_or("0.1.0")),
        (false, Some(wanted)) if spec["version"].as_str() != Some(wanted) => Some(wanted),
        (false, _) => None,
    };
    let base = source_etag(state, home, &change.name).await?;
    let checked = datamodels::check_source(
        state,
        &caller.identity,
        home,
        &change.name,
        &spec,
        source,
        version,
    )
    .await;
    let finding = |path: &str, message: String| json!({ "level": "error", "at": format!("/source{path}"), "message": message });
    let (checked, next_val, imports) = match checked {
        Ok(done) => done,
        Err(ApiError::Invalid { detail, errors }) => {
            let findings = errors
                .iter()
                .map(|error| match error.split_once(": ") {
                    Some((path, message)) if !path.contains(' ') => {
                        finding(&format!("/{path}"), message.to_owned())
                    }
                    _ => finding("", format!("{detail}: {error}")),
                })
                .collect();
            return Ok((json!({ "ok": false }), Err(findings)));
        }
        Err(ApiError::BadRequest(message)) => {
            return Ok((json!({ "ok": false }), Err(vec![finding("", message)])))
        }
        Err(err) => return Err(OpError::Api(err)),
    };
    let lane = datamodels::model_lane(home, creating, &checked.severity);
    let changes = affected(state, home, &change.name, manifest, &checked.changes).await;
    let shown = json!({
        "ok": true,
        "file": manifest.pointer("/spec/linkml"),
        "severity": checked.severity,
        "version": checked.version,
        "lane": lane,
        "changes": changes,
    });
    let plan = ModelPlan {
        source: source.to_owned(),
        checked,
        next_val,
        imports,
        creating,
        base,
    };
    Ok((shown, Ok(plan)))
}

/// The model's changes, each breaking one with `affectedEntities`: how many stored entities of
/// the classes it touches the model's space holds, as its broker counts them by type.
// ponytail: counts the class itself, not its `is_a` subclasses; add those when a seeded model
// stores entities of a subclass.
async fn affected(
    state: &AppState,
    home: &str,
    name: &str,
    manifest: &Value,
    changes: &[ModelChange],
) -> Vec<Value> {
    let mut rows: Vec<Value> = changes
        .iter()
        .map(|change| serde_json::to_value(change).unwrap_or_default())
        .collect();
    if !changes.iter().any(|change| change.severity == "breaking") {
        return rows;
    }
    let published: Value = datamodels::read_source(state, home, name)
        .await
        .ok()
        .and_then(|text| serde_norway::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let space = manifest
        .pointer("/spec/contextSpaceRef")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let counted: Result<BTreeMap<String, u64>, String> = if space.is_empty() {
        Err("the model names no contextSpaceRef".to_owned())
    } else {
        let broker = crate::spaces::broker(&state.mirror, home, space);
        match usage_of(state, &broker, home, space).await {
            Ok(usage) => match usage.bytes {
                Some(bytes) => Ok(bytes.by_type),
                None if usage.entities == 0 => Ok(BTreeMap::new()),
                None => Err(format!("broker '{broker}' does not count by type")),
            },
            Err(why) => Err(why),
        }
    };
    for (row, change) in rows.iter_mut().zip(changes) {
        let classes = classes_of(&published, &change.subject);
        if change.severity != "breaking" || classes.is_empty() {
            continue;
        }
        match &counted {
            Ok(by_type) => {
                let held: u64 = by_type
                    .iter()
                    .filter(|(iri, _)| {
                        let term = iri.rsplit(['/', '#']).next().unwrap_or(iri);
                        classes.iter().any(|class| class == term)
                    })
                    .map(|(_, n)| *n)
                    .sum();
                row["affectedEntities"] = json!(held);
                row["space"] = json!(space);
            }
            Err(why) => {
                row["affectedEntities"] = Value::Null;
                row["notCounted"] = json!(format!(
                    "the stored {} entities could not be counted: {why}",
                    classes.join(", ")
                ));
            }
        }
    }
    rows
}

/// What the space's broker counts in its tenant, or why it cannot.
async fn usage_of(
    state: &AppState,
    broker: &str,
    home: &str,
    space: &str,
) -> Result<crate::space_usage::SpaceUsage, String> {
    let tenant = crate::spaces::tenant(&state.mirror, home, space)?;
    state.space_usage.usage(broker, &tenant).await
}

/// The classes of the published model a change's subject touches: `Class` or `Class.slot`, or
/// every class that lists a top-level slot.
fn classes_of(published: &Value, subject: &str) -> Vec<String> {
    let Some(classes) = published["classes"].as_object() else {
        return Vec::new();
    };
    let head = subject.split('.').next().unwrap_or(subject);
    if classes.contains_key(head) {
        return vec![head.to_owned()];
    }
    classes
        .iter()
        .filter(|(_, class)| {
            class["slots"]
                .as_array()
                .is_some_and(|slots| slots.iter().any(|slot| slot == subject))
        })
        .map(|(class, _)| class.clone())
        .collect()
}

impl ChangeInput {
    fn op_word(&self) -> &'static str {
        match self.op {
            Op::Create => "create",
            Op::Replace => "replace",
            Op::Patch => "patch",
            Op::Delete => "delete",
        }
    }
}

/// The findings of a red diff, as `path: message` per finding for the 422's `errors[]`.
fn findings_of(at: &str, diff: &Value) -> Vec<String> {
    let findings = diff
        .pointer("/verdict/findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<String> = findings
        .iter()
        .filter(|f| f["level"].as_str() != Some("warning") && f["level"].as_str() != Some("info"))
        .map(|f| {
            let path = f["path"].as_str().unwrap_or_default().replace('.', "/");
            let path = if let Some(within) = f["at"].as_str() {
                format!("{at}{within}")
            } else if path.is_empty() {
                format!("{at}/manifest")
            } else {
                format!("{at}/manifest/{path}")
            };
            format!(
                "{path}: {}",
                f["message"].as_str().unwrap_or("the check failed")
            )
        })
        .collect();
    if out.is_empty() {
        out.push(format!("{at}/manifest: the check failed"));
    }
    out
}

/// What the proposal's verdict gate (PF-57) refuses for a planned manifest whose dry run was
/// green: a Pipeline's records are tested on a sample, not dry-run (T-4840). Asked at plan time,
/// so a plan never promises a change its apply cannot open, and again before an apply commits
/// anything, so an apply never opens half a plan.
async fn gate_refusal(state: &AppState, item: &Planned) -> Result<Option<String>, OpError> {
    let Some(manifest) = &item.manifest else {
        return Ok(None);
    };
    match super::verdict_for_manifest(state, &item.home, manifest).await {
        Ok(_) => Ok(None),
        Err(OpError::Conflict(body)) => Ok(Some(format!(
            "{} Run `{}` on it, then call {PLAN} again",
            body["detail"].as_str().unwrap_or("its check is not fresh."),
            body["check"].as_str().unwrap_or("jc_manifest_dry_run")
        ))),
        Err(other) => Err(other),
    }
}

async fn plan(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: PlanInput,
) -> Result<Value, OpError> {
    let mut diffs = Vec::new();
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    // A plan waits in memory with its sources: all of them together stay under one source's cap.
    let sources: usize = input
        .changes
        .iter()
        .filter_map(|change| change.source.as_ref())
        .map(String::len)
        .sum();
    if sources > MAX_REQUEST_BYTES {
        return Err(invalid(
            "/changes".into(),
            format!(
                "the plan's LinkML sources take {sources} bytes together, more than the \
                 {MAX_REQUEST_BYTES} byte limit: plan the models one at a time"
            ),
        ));
    }
    // Changes applied as one Change are checked as one (T-6282): a reference to a sibling
    // resolves and a write a sibling Endpoint binds is made, as in `jc_space_complete`.
    for (n, change) in input.changes.iter().enumerate() {
        if !seen.insert((change.kind.clone(), change.name.clone())) {
            return Err(invalid(
                format!("/changes/{n}"),
                format!("{} '{}' is in this plan twice", change.kind, change.name),
            ));
        }
    }
    let mut bundle = together(state, project, &input);
    // In a bundle the Endpoints go first: the slug their check mints joins the bundle, so a
    // Pipeline or Policy beside them reads a whole Endpoint whatever order the plan lists.
    let mut order: Vec<usize> = (0..input.changes.len()).collect();
    if bundle.is_some() {
        order.sort_by_key(|&n| input.changes[n].kind != "Endpoint");
    }
    let mut results: Vec<Option<(Value, Planned, Vec<String>)>> =
        (0..input.changes.len()).map(|_| None).collect();
    for n in order {
        let change = &input.changes[n];
        let at = format!("/changes/{n}");
        let check = checked(
            caller,
            state,
            project,
            &at,
            change,
            input.workspace.as_deref(),
        );
        let (diff, planned) = match &mut bundle {
            Some(manifests) => {
                let checked = crate::references::BUNDLE
                    .scope(manifests.clone(), check)
                    .await?;
                if let (Some(slot), Some(m)) = (manifests.get_mut(n), &checked.1.manifest) {
                    *slot = m.clone();
                }
                checked
            }
            None => check.await?,
        };
        let mut found = Vec::new();
        if diff.pointer("/verdict/ok") != Some(&json!(true)) {
            found.extend(findings_of(&at, &diff));
        } else if let Some(refusal) = gate_refusal(state, &planned).await? {
            found.push(format!("{at}/manifest: {refusal}"));
        }
        results[n] = Some((diff, planned, found));
    }
    for (diff, planned, found) in results.into_iter().flatten() {
        errors.extend(found);
        diffs.push(diff);
        items.push(planned);
    }
    if !errors.is_empty() {
        return Err(OpError::Api(ApiError::Unprocessable {
            detail: format!(
                "{} of the plan's changes fail their check; nothing was kept. Fix what errors[] \
                 names and plan again",
                errors.len()
            ),
            errors,
        }));
    }
    let now = TimestampTz::now();
    let expires_at = now + jiff::SignedDuration::from_mins(TTL_MINUTES);
    let base_etag = {
        let mut hash = Sha256::new();
        for item in &items {
            hash.update(item.base.as_bytes());
        }
        format!("{:x}", hash.finalize())
    };
    let needs_secret = needs_secret(state, project, &items).await;
    let id = new_id();
    let lane = diffs
        .iter()
        .filter_map(|d| d["lane"].as_str())
        .max_by_key(|lane| match *lane {
            "red" => 2,
            "yellow" => 1,
            _ => 0,
        })
        .unwrap_or("green")
        .to_owned();
    let writes = caller
        .access
        .as_ref()
        .and_then(|a| a.connector())
        .is_none_or(|c| c.writes);
    state.plans.keep(
        id.clone(),
        Plan {
            owner: caller.identity.subject.clone(),
            connector: caller
                .access
                .as_ref()
                .and_then(|a| a.connector())
                .map(|c| c.id.clone()),
            project: project.to_owned(),
            workspace: input.workspace.clone(),
            items,
            expires_at,
        },
        now,
    );
    Ok(json!({
        "planId": id,
        "baseEtag": base_etag,
        "expiresAt": expires_at.to_rfc3339(),
        "lane": lane,
        "diff": diffs,
        "verdict": { "ok": true, "findings": [] },
        "needsSecret": needs_secret,
        "next": if !needs_secret.is_empty() {
            "a person enters each secret of needsSecret at its form link in the Portal (never in this chat); then apply".to_owned()
        } else if writes {
            format!("call {APPLY} with this planId within {TTL_MINUTES} minutes: {APPLY_OPENS}")
        } else {
            "this connector has writes off: a person applies this change in the Portal".to_owned()
        },
    }))
}

/// The manifests of a plan applied as one Change, `None` for a plan opened change by change.
/// One Change holds two or more creates, replaces or patches of the project's own objects
/// outside a workspace (T-6282), a DataModel with its LinkML file among them (JC2-10); a removal
/// and an object kept in another namespace each open a change request of their own, so their
/// siblings are checked against the project as it is.
fn together(state: &AppState, project: &str, input: &PlanInput) -> Option<Vec<Value>> {
    if input.workspace.is_some() || input.changes.len() < 2 {
        return None;
    }
    input
        .changes
        .iter()
        .enumerate()
        .map(|(n, change)| {
            if change.op == Op::Delete {
                return None;
            }
            let info = super::resources::kind_named(&change.kind).ok()?;
            let home = super::resources::home_holding(state, info, project, &change.name);
            if home != project {
                return None;
            }
            manifest_of(state, &format!("/changes/{n}"), &home, change)
                .ok()
                .flatten()
        })
        .collect()
}

/// Whether a kept plan opens one Change: the plan [`together`] checked as one.
fn bundled(plan: &Plan) -> bool {
    plan.workspace.is_none()
        && plan.items.len() > 1
        && plan
            .items
            .iter()
            .all(|item| item.manifest.is_some() && item.home == plan.project)
}

/// Opens a [`bundled`] plan as one Change (T-6282): every change through the propose engine's
/// checks onto one branch, then one merge request a person approves whole. A change refused on
/// the way removes the branch, so nothing of the plan is left open.
async fn open_together(
    caller: &Caller,
    state: &AppState,
    plan_id: &str,
    plan: &Plan,
) -> Result<Vec<Value>, OpError> {
    let project = plan.project.as_str();
    let shako = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let branch = format!("portal/import-plan-{plan_id}");
    let mut lanes = Vec::new();
    let mut waits_on = std::collections::BTreeSet::new();
    let mut lines = Vec::new();
    let mut summary = crate::change::PlanSummary::default();
    for item in &plan.items {
        let Some(manifest) = &item.manifest else {
            continue;
        };
        let (op, word) = if item.base == "absent" {
            summary.create += 1;
            (ChangeOp::Create, "create")
        } else {
            summary.update += 1;
            (ChangeOp::Update, "update")
        };
        let written = match model_proposal(caller, state, item, manifest) {
            // A model writes its manifest, source and artifacts onto the plan's branch (JC2-10).
            Ok(Some(proposal)) => {
                datamodels::propose_model_onto(state, &caller.identity, proposal, &branch)
                    .await
                    .map(|lane| (lane, Vec::new()))
            }
            Ok(None) => mutate::propose_gated_onto(
                &caller.identity,
                state,
                &item.home,
                item.plural,
                Some(&item.name),
                op,
                manifest.clone(),
                &branch,
            )
            .await
            .map(|commit| (commit.lane, commit.waits_on)),
            Err(err) => Err(err),
        };
        super::forget_check(state, &item.home, manifest).await;
        match written {
            Ok((lane, waits)) => {
                lanes.push(lane);
                waits_on.extend(waits);
                lines.push(format!("- {word} {} `{}`", item.kind, item.name));
            }
            Err(err) => {
                if !lanes.is_empty() {
                    if let Err(gone) = shako.delete_branch(&branch).await {
                        tracing::warn!(%branch, error = %gone, "a refused plan left its branch");
                    }
                }
                return Err(OpError::Api(ApiError::Conflict(format!(
                    "{} {} was refused ({err}); nothing of the plan was opened",
                    item.kind, item.name
                ))));
            }
        }
    }
    let lane = lanes
        .into_iter()
        .reduce(crate::api::import::riskiest)
        .unwrap_or(Lane::Green);
    let mut body = format!(
        "Proposed through plan {plan_id} (AG-149) in project `{project}`, approved as one \
         change:\n\n{}",
        lines.join("\n")
    );
    for line in &waits_on {
        body.push_str("\n\n");
        body.push_str(line);
    }
    let title = format!("apply {} changes in {project}", plan.items.len());
    let pull = shako
        .create_pull_request(
            &branch,
            &shako.default_branch().await.map_err(ApiError::from)?,
            &title,
            &body,
        )
        .await
        .map_err(ApiError::from)?;
    let change = crate::change::Change::new(
        crate::api::changes::change_meta(state, &shako, pull.number, project),
        crate::change::ChangeStatus::new(
            lane,
            crate::change::ChangePhase::PendingApproval,
            summary,
        )
        .in_repository(&pull.repository)
        .proposed_through(&pull.body)
        .with_merge_request(pull.url),
    );
    Ok(vec![opened_row(
        state,
        project,
        ProposeOutcome::Change(change),
    )])
}

/// Every `secretRef` the plan's manifests name that the repository's encrypted files do not hold,
/// with the Portal form a person enters it in (MF-24, T-5971). A forge that cannot be read counts
/// every reference as missing: the form replaces a secret whole, so entering it again is safe.
async fn needs_secret(state: &AppState, project: &str, items: &[Planned]) -> Vec<Value> {
    let wanted: std::collections::BTreeSet<(String, Option<String>)> = items
        .iter()
        .filter_map(|item| item.manifest.as_ref())
        .flat_map(crate::api::secrets::references)
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let stored = crate::api::secrets::stored(state, project)
        .await
        .unwrap_or_default();
    wanted
        .into_iter()
        .filter(|(name, key)| {
            !stored
                .get(name)
                .is_some_and(|held| key.as_ref().is_none_or(|key| held.keys.contains(key)))
        })
        .map(|(name, key)| {
            json!({
                "name": name,
                "key": key,
                "form": crate::api::secrets::form_url(state, project, &name, key.as_deref()),
            })
        })
        .collect()
}

async fn apply(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ApplyInput,
) -> Result<Value, OpError> {
    let connector = caller.access.as_ref().and_then(|a| a.connector());
    let now = TimestampTz::now();
    let not_found = || {
        OpError::Api(ApiError::NotFound(format!(
            "plan '{}' is not there: it expired ({TTL_MINUTES} minutes), was applied, or was \
             made by someone else. Call {PLAN} again",
            input.plan_id
        )))
    };
    // The connector's switch first: a plan it may not apply stays for the Portal.
    if let Some(connector) = connector {
        if !connector.writes {
            return Err(OpError::Api(ApiError::Denied(format!(
                "this connector has writes off: apply the plan in the Portal, or turn writes on \
                 for connector {} on the My connectors page",
                connector.name
            ))));
        }
    }
    let plan = state
        .plans
        .take(
            &input.plan_id,
            &caller.identity.subject,
            project,
            connector.map(|c| c.id.as_str()),
            now,
        )
        .ok_or_else(not_found)?;
    for item in &plan.items {
        let verb = item_verb(item.manifest.is_some(), &item.base);
        let now = state.mirror.get(&item.home, &item.kind, &item.name);
        let space = crate::connectors::change_space(
            now.and_then(|n| serde_json::to_value(n).ok()).as_ref(),
            item.manifest.as_ref(),
        );
        let target = (item.kind.as_str(), item.name.as_str(), space.as_deref());
        connector_admits(caller, state, project, target, true, verb)?;
    }
    unchanged_and_gated(state, &plan.items).await?;
    let workspace = plan.workspace.as_deref();
    let opening = async {
        if bundled(&plan) {
            let manifests = plan
                .items
                .iter()
                .filter_map(|i| i.manifest.clone())
                .collect();
            return crate::references::BUNDLE
                .scope(
                    manifests,
                    open_together(caller, state, &input.plan_id, &plan),
                )
                .await;
        }
        let mut opened = Vec::new();
        for item in &plan.items {
            match open_one(caller, state, item, workspace).await {
                Ok(outcome) => opened.push(opened_row(state, project, outcome)),
                Err(err) if opened.is_empty() => return Err(OpError::Api(err)),
                Err(err) => {
                    return Err(OpError::Api(ApiError::Conflict(format!(
                        "{} {} was not applied ({err}); these were: {}",
                        item.kind,
                        item.name,
                        serde_json::to_string(&opened).unwrap_or_default()
                    ))))
                }
            }
        }
        Ok(opened)
    };
    // Every commit and merge request the apply opens names the plan it took (T-5973).
    let opened = match crate::change::ProposedVia::current() {
        Some(via) => {
            crate::change::PROPOSED_VIA
                .scope(via.with_plan(&input.plan_id), opening)
                .await
        }
        None => opening.await,
    }?;
    let mut opened = opened;
    if let (Some(connector), None) = (connector, workspace) {
        let waiting = if connector.autonomous {
            approve_autonomously(state, caller, project, connector, &mut opened).await
        } else {
            opened.clone()
        };
        notify_approvers(state, caller, project, &connector.name, &waiting).await;
    }
    let key = if workspace.is_some() {
        "workspaceChanges"
    } else {
        "changeRequests"
    };
    Ok(json!({
        key: opened,
        "approve": "a change with approvedVia is approved already (an autonomous connector); a person with approve on these kinds decides each other change at its url, its approval page in the Portal",
    }))
}

/// Approves each change an autonomous connector just opened as its owner (T-6552), through the
/// approval page's own path and checks. A change the checks hold back (access, a public
/// publication, a deletion, a missing right) stays pending, its row saying why; those rows come
/// back for the approvers' notice.
async fn approve_autonomously(
    state: &AppState,
    caller: &Caller,
    project: &str,
    connector: &crate::connectors::Connector,
    opened: &mut [Value],
) -> Vec<Value> {
    // The ceiling as it is at this apply, not as it was when the connector was made (AG-150,
    // T-7541).
    if !crate::connectors::Ceiling::of(&state.mirror, &caller.identity)
        .with_approved(connector)
        .allows_autonomous(project)
    {
        for row in opened.iter_mut() {
            row["waits"] = json!(format!(
                "autonomous: your DelegationPolicy does not let a connector approve its own changes in project {project}; an approver decides this one"
            ));
        }
        return opened.to_vec();
    }
    let connector = connector.name.as_str();
    let mut waiting = Vec::new();
    for row in opened.iter_mut() {
        let Some(id) = row["id"].as_str().map(str::to_owned) else {
            continue;
        };
        match crate::api::changes::approve_change_for(
            state,
            &caller.identity,
            project,
            &id,
            None,
            crate::api::changes::ApprovedBy::Connector(connector.to_owned()),
        )
        .await
        {
            Ok(change) => {
                row["state"] = json!(change.status.phase);
                row["approvedVia"] = json!(connector);
            }
            Err(err) => {
                tracing::info!(change = %id, %connector, error = %err, "an autonomous connector's change waits for an approver");
                row["waits"] = json!(err.to_string());
                waiting.push(row.clone());
            }
        }
    }
    waiting
}

/// Tells the project's approvers of each change a connector opened (T-5979): one notice per
/// change, naming the person, the connector and the change, linking its approval page. A notice
/// that cannot be written is logged and the change stays opened.
async fn notify_approvers(
    state: &AppState,
    caller: &Caller,
    project: &str,
    connector: &str,
    opened: &[Value],
) {
    let identity = &caller.identity;
    let author = identity.username.to_ascii_lowercase();
    let name = identity.name.as_deref().unwrap_or(&identity.username);
    for id in opened.iter().filter_map(|row| row["id"].as_str()) {
        let link = format!("/projects/{project}/approvals/{id}");
        let key = format!("change.opened:{project}/{id}");
        let notice = crate::entity_comments::Notice {
            kind: "change.opened",
            project,
            about: id,
            author: &author,
            author_name: name,
            connector,
            link: &link,
            key: &key,
        };
        let audience = crate::entity_comments::approvers_of(project);
        if let Err(err) = state.comments.notify(&audience, notice).await {
            tracing::warn!(error = %err, project, change = id, "approvers not notified");
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevertInput {
    id: String,
    #[serde(default)]
    workspace: Option<String>,
}

/// A file of the forge as a value: the YAML parsed, or its text when it is no YAML, so two reads
/// of one file compare by content and not by formatting.
async fn file_at(
    forge: &crate::forge::ShakoClient,
    path: &str,
    at: &str,
) -> Result<Option<Value>, OpError> {
    Ok(forge
        .get_file(path, at)
        .await
        .map_err(ApiError::from)?
        .map(|file| {
            serde_norway::from_str::<Value>(&file.content).unwrap_or(Value::String(file.content))
        }))
}

/// The kind and name a manifest names, when it is one of this Portal's kinds.
fn manifest_names(manifest: &Value) -> Option<(String, String)> {
    let kind = manifest.get("kind").and_then(Value::as_str)?;
    let name = manifest.pointer("/metadata/name").and_then(Value::as_str)?;
    crate::resource::by_kind(kind).map(|info| (info.kind.to_owned(), name.to_owned()))
}

/// The merged pull request `number` of change `id` with the commit it merged as: `NotFound` for a change
/// that is no Portal change of `project`, `Conflict` for one not merged (yet).
async fn merged_change(
    forge: &crate::forge::ShakoClient,
    number: u64,
    project: &str,
    id: &str,
) -> Result<(crate::forge::PullRequest, String), OpError> {
    let missing = || {
        OpError::Api(ApiError::NotFound(format!(
            "change '{id}' not found in project '{project}'"
        )))
    };
    let pr = match forge.pull_request(number).await {
        Ok(pr) => pr,
        Err(crate::forge::GitError::NotFound | crate::forge::GitError::Api { status: 404, .. }) => {
            return Err(missing())
        }
        Err(err) => return Err(OpError::Api(err.into())),
    };
    // Only a change the Portal opened is the Portal's to revert, as `/undo` holds (T-3274).
    let branch = pr
        .head_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(&pr.head_branch);
    if !branch.starts_with("portal/") && !branch.starts_with("workspace/") {
        return Err(missing());
    }
    if !pr.merged || pr.merge_base.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "change '{id}' is not merged, so there is nothing to revert: reject it on its page instead"
        ))));
    }
    let after = if pr.merge_commit_sha.is_empty() {
        pr.head_ref().to_owned()
    } else {
        pr.merge_commit_sha.clone()
    };
    Ok((pr, after))
}

/// Refuses a revert of change `id` that would undo what main changed since (`moved`), that
/// touched files no plan writes (`outside`), or that leaves nothing to do.
fn revertible(
    id: &str,
    moved: &[String],
    outside: Vec<String>,
    changes: &[ChangeInput],
) -> Result<(), OpError> {
    if !moved.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "main changed {} since change '{id}' was merged, so a revert would undo that too; \
             nothing was planned. Plan the change you want with {PLAN} instead",
            moved.join(", ")
        ))));
    }
    if !outside.is_empty() {
        return Err(OpError::Api(ApiError::Unprocessable {
            detail: format!("change '{id}' touched files a plan cannot write; nothing was planned"),
            errors: outside,
        }));
    }
    if changes.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "change '{id}' left nothing to revert"
        ))));
    }
    Ok(())
}

/// A manifest put back is judged as its proposal is, its space and constraints included; an
/// object the revert deletes needs `delete` on its kind.
fn may_put_back(
    caller: &Caller,
    state: &AppState,
    project: &str,
    effective: &crate::permissions::Effective,
    kind: &str,
    path: &str,
    manifest: Option<&Value>,
) -> Result<(), OpError> {
    match manifest {
        Some(m) => {
            let envelope = serde_json::from_value::<crate::resource::ResourceEnvelope>(m.clone())
                .map_err(|err| invalid("/id".into(), format!("{path}: {err}")))?;
            crate::api::import::authorize_manifest(
                state,
                &caller.identity,
                project,
                effective,
                &envelope,
            )?;
        }
        None => effective.check(kind, jc_core::kinds::Verb::Delete, None)?,
    }
    Ok(())
}

/// The plan that undoes merged change `id` (T-5974, S-53): every file it touched goes back to what
/// it was at the commit the change was cut from. A file main changed since the merge is a
/// conflict, and a file that is no manifest of the project cannot be planned: both are named and
/// nothing is kept. The revert is then an ordinary plan, so it is checked, held to the caller's
/// rights and the connector's scope, and opened by `jc_change_apply` for a person to approve.
async fn revert(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: RevertInput,
) -> Result<Value, OpError> {
    let missing = || {
        OpError::Api(ApiError::NotFound(format!(
            "change '{}' not found in project '{project}'",
            input.id
        )))
    };
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    if !effective.may_read_project() {
        return Err(missing());
    }
    let (forge, number) = crate::api::changes::resolve_change(state, project, &input.id)?;
    let (pr, after) = merged_change(&forge, number, project, &input.id).await?;
    let home = format!("projects/{project}/");
    let files = forge
        .pull_request_files(number)
        .await
        .map_err(ApiError::from)?;
    let mut changes = Vec::new();
    let mut moved = Vec::new();
    let mut outside = Vec::new();
    for file in &files {
        if !file.path.starts_with(&home) {
            outside.push(format!(
                "{}: outside project {project}; revert it in the forge",
                file.path
            ));
            continue;
        }
        let before = file_at(&forge, &file.path, &pr.merge_base).await?;
        let now = file_at(&forge, &file.path, &pr.base_branch).await?;
        let made = file_at(&forge, &file.path, &after).await?;
        let Some((kind, name)) = [&before, &made, &now]
            .into_iter()
            .find_map(|v| v.as_ref().and_then(manifest_names))
        else {
            outside.push(format!(
                "{}: no manifest, so no plan can write it; revert it in the forge",
                file.path
            ));
            continue;
        };
        // A change touching a kind this caller may not read is not there for them, its paths
        // included: the rule `change_readable` holds on the REST and MCP reads (F-R2POR4-007).
        if !effective.may_read(&kind) {
            return Err(missing());
        }
        // A narrowed connector hears of nothing outside its scope, a conflict included.
        let space = crate::connectors::change_space(now.as_ref(), before.as_ref());
        through_connector(caller, project, &kind, &name, space.as_deref(), false)?;
        if now != made {
            moved.push(file.path.clone());
            continue;
        }
        let (op, manifest) = match (before, now.is_some()) {
            (Some(before), true) => (Op::Replace, Some(before)),
            (Some(before), false) => (Op::Create, Some(before)),
            // Absent before and now: the manifest named it only as made, and `now != made`
            // above moved it, so the one case left is a file the change added.
            (None, _) => (Op::Delete, None),
        };
        // The revert writes what the original wrote, so it needs the same right on each item.
        may_put_back(
            caller,
            state,
            project,
            &effective,
            &kind,
            &file.path,
            manifest.as_ref(),
        )?;
        changes.push(ChangeInput {
            op,
            kind,
            name,
            manifest,
            patch: None,
            source: None,
        });
    }
    revertible(&input.id, &moved, outside, &changes)?;
    let mut answer = plan(
        caller,
        state,
        project,
        PlanInput {
            changes,
            workspace: input.workspace,
        },
    )
    .await?;
    answer["reverts"] = json!(input.id);
    Ok(answer)
}

/// Refuses the whole plan when an object moved since it was planned or its gate turned red.
async fn unchanged_and_gated(state: &AppState, items: &[Planned]) -> Result<(), OpError> {
    let mut moved: Vec<String> = Vec::new();
    for item in items {
        let file_moved = match &item.model {
            Some(model) => source_etag(state, &item.home, &item.name).await? != model.base,
            None => false,
        };
        if file_moved || etag(state, &item.home, &item.kind, &item.name) != item.base {
            moved.push(format!("{} {}", item.kind, item.name));
        }
    }
    if !moved.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "{} changed since the plan was made; nothing was applied. Read it again and call {PLAN} \
             anew",
            moved.join(", ")
        ))));
    }
    let mut refused = Vec::new();
    for item in items {
        if let Some(refusal) = gate_refusal(state, item).await? {
            refused.push(format!("{} {}: {refusal}", item.kind, item.name));
        }
    }
    if !refused.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "nothing was applied: {}",
            refused.join("; ")
        ))));
    }
    Ok(())
}

/// Opens one planned change: its change request, or its commit on the workspace.
async fn open_one(
    caller: &Caller,
    state: &AppState,
    item: &Planned,
    workspace: Option<&str>,
) -> Result<ProposeOutcome, ApiError> {
    match &item.manifest {
        None => match delete::delete_with_identity(
            &caller.identity,
            state,
            &item.home,
            item.plural,
            &item.name,
            false,
            workspace,
        )
        .await
        {
            Ok(DeleteOutcome::Proposed(change)) => Ok(ProposeOutcome::Change(change)),
            Ok(DeleteOutcome::Workspace(commit)) => Ok(ProposeOutcome::Workspace(commit)),
            Ok(other) => Err(ApiError::Conflict(match other {
                DeleteOutcome::Referenced { here, elsewhere } => {
                    DeleteOutcome::conflict_message(&here, elsewhere)
                }
                _ => "the removal answered a dry run".into(),
            })),
            Err(err) => Err(err),
        },
        Some(manifest) if item.model.is_some() => {
            let outcome = propose_source(caller, state, item, manifest).await;
            super::forget_check(state, &item.home, manifest).await;
            outcome.map(ProposeOutcome::Change)
        }
        Some(manifest) => {
            let op = if item.base == "absent" {
                ChangeOp::Create
            } else {
                ChangeOp::Update
            };
            // Gated: the proposal needs the green verdict the plan's check recorded (PF-57).
            let outcome = mutate::propose_gated_in(
                &caller.identity,
                state,
                &item.home,
                item.plural,
                Some(&item.name),
                op,
                manifest.clone(),
                workspace,
            )
            .await;
            super::forget_check(state, &item.home, manifest).await;
            outcome
        }
    }
}

/// Opens the change of a DataModel planned with its LinkML file: manifest, source and generated
/// artifacts in one change request, as `PUT .../source` opens it (DM-22).
async fn propose_source(
    caller: &Caller,
    state: &AppState,
    item: &Planned,
    manifest: &Value,
) -> Result<crate::change::Change, ApiError> {
    let proposal = model_proposal(caller, state, item, manifest)?
        .ok_or_else(|| ApiError::Internal("a model change without its source".into()))?;
    datamodels::propose_model(state, &caller.identity, proposal).await
}

/// The model proposal of a DataModel planned with its LinkML file, once the caller may propose
/// it; `None` for any other change.
fn model_proposal<'a>(
    caller: &Caller,
    state: &AppState,
    item: &'a Planned,
    manifest: &Value,
) -> Result<Option<datamodels::ModelProposal<'a>>, ApiError> {
    let Some(model) = item.model.as_ref() else {
        return Ok(None);
    };
    crate::permissions::for_request(state, &caller.identity, &item.home).check(
        "DataModel",
        jc_core::kinds::Verb::Propose,
        Some(manifest),
    )?;
    let envelope: crate::resource::ResourceEnvelope = serde_json::from_value(manifest.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid DataModel manifest: {e}")))?;
    let (verb, noun) = if model.creating {
        ("create", "creation")
    } else {
        ("update", "update")
    };
    Ok(Some(datamodels::ModelProposal {
        namespace: &item.home,
        name: &item.name,
        envelope,
        source: &model.source,
        checked: model.checked.clone(),
        next_val: model.next_val.clone(),
        imports: model.imports.clone(),
        creating: model.creating,
        lane: datamodels::model_lane(&item.home, model.creating, &model.checked.severity),
        title: format!("{verb} DataModel {}", item.name),
        body: format!(
            "Proposed {noun} of DataModel `{}` source, manifest and generated artifacts in \
             project `{}` through a plan (AG-149).",
            item.name, item.home
        ),
    }))
}

/// What an apply opened, as API/14 §4.3 names it; never an approval. `url` is the Portal page
/// where a person decides the change, the forge's pull request only `mergeRequest` (T-6293).
fn opened_row(state: &AppState, project: &str, outcome: ProposeOutcome) -> Value {
    match outcome {
        ProposeOutcome::Change(change) => json!({
            "url": super::changes::approval_url(state, project, &change.metadata.name),
            "id": change.metadata.name,
            "mergeRequest": change.status.merge_request,
            "lane": change.status.lane,
            "state": change.status.phase,
        }),
        ProposeOutcome::Workspace(commit) => json!({
            "workspace": commit.workspace,
            "id": commit.path,
            "branch": commit.branch,
            "lane": commit.lane,
        }),
        ProposeOutcome::DryRun(result) => serde_json::to_value(result).unwrap_or_default(),
        ProposeOutcome::Bundled(commit) => serde_json::to_value(commit).unwrap_or_default(),
    }
}

fn plan_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changes": {
                "type": "array",
                "description": "The changes to plan together. A Pipeline is planned only after jc_pipeline_test passed on the same spec",
                "minItems": 1,
                "maxItems": MAX_CHANGES,
                "items": {
                    "type": "object",
                    "properties": {
                        "op": {
                            "type": "string",
                            "enum": ["create", "replace", "patch", "delete"],
                            "description": "create: a new object (an existing name is refused: patch it). patch: the usual edit of some fields, e.g. off the catalogue {\"spec\": {\"publish\": null, \"catalog\": {\"harvest\": false}}}. replace: the whole manifest. delete: removes the object, never to unpublish it"
                        },
                        "kind": text("The manifest kind, e.g. DataModel", TERM),
                        "name": text("metadata.name, e.g. bike-counter", NAME),
                        "manifest": manifest("With create or replace, no patch: the whole manifest"),
                        "patch": { "type": "object", "maxProperties": super::bounds::MANIFEST_MEMBERS, "description": "With patch, no manifest: a JSON merge patch (RFC 7386); null removes a member" },
                        "source": { "type": "string", "maxLength": MAX_REQUEST_BYTES, "description": "With a DataModel: its whole LinkML file (spec.linkml); the plan shows each change, its version and the stored entities a breaking one affects" }
                    },
                    "required": ["op", "kind", "name"],
                    "additionalProperties": false
                }
            },
            "workspace": text("Commit into this workspace instead of change requests", NAME)
        },
        "required": ["changes"],
        "additionalProperties": false
    })
}

fn plan_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "planId": { "type": "string" },
            "baseEtag": { "type": "string" },
            "expiresAt": { "type": "string", "format": "date-time" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "diff": { "type": "array", "items": { "type": "object" } },
            "verdict": { "type": "object" },
            "needsSecret": { "type": "array", "items": { "type": "object" } },
            "next": { "type": "string" }
        },
        "required": ["planId", "baseEtag", "expiresAt", "diff", "verdict"]
    })
}

fn apply_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "planId": text("The planId jc_change_plan answered", TERM)
        },
        "required": ["planId"],
        "additionalProperties": false
    })
}

fn apply_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changeRequests": { "type": "array", "items": { "type": "object" } },
            "workspaceChanges": { "type": "array", "items": { "type": "object" } },
            "approve": { "type": "string" }
        }
    })
}

fn revert_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("The merged Change to undo, e.g. chg-0000002a", TERM),
            "workspace": text("Commit the revert into this workspace instead of change requests", NAME)
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn revert_output_schema() -> Value {
    let mut schema = plan_output_schema();
    schema["properties"]["reverts"] = json!({ "type": "string" });
    schema
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: PLAN,
            title: "Plan Change",
            description: "Dry-runs one or more changes of any kind (nothing saved): the diff, the verdict and a planId valid 30 minutes; a failing check answers 422 naming each field",
            input: plan_input_schema,
            output: plan_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<PlanInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { plan(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: REVERT,
            title: "Plan Revert",
            description: "Plans the undo of a merged Change, every file back to before it, for jc_change_apply; 409 names files changed since",
            input: revert_input_schema,
            output: revert_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RevertInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { revert(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: APPLY,
            title: "Apply Change",
            description: "Opens the change requests (or workspace commits) of an unchanged jc_change_plan plan; 409 when an object moved since. Nothing goes live until a person approves each in the Portal, so apply what the person asked for without asking again",
            input: apply_input_schema,
            output: apply_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            // The plan was the look before the leap and a person still approves every change
            // it opens (AG-149): no second question at the door.
            lane: Lane::Green,
            validate: |val| parse_input::<ApplyInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { apply(caller, state, project, parse_input(val)?).await })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JC2-11: a peeked plan carries a model's planned LinkML, so the paired tab's editor shows
    /// the source the plan changes and not only its manifest.
    #[test]
    fn a_peek_carries_the_planned_model_source() {
        let now = TimestampTz::now();
        let store = PlanStore::default();
        let item = |name: &str, model: Option<ModelPlan>| Planned {
            kind: "DataModel".into(),
            plural: "datamodels",
            home: "helsinki".into(),
            name: name.into(),
            manifest: Some(json!({ "kind": "DataModel" })),
            base: "absent".into(),
            model,
        };
        let model = air_model();
        store.keep(
            "p1".into(),
            Plan {
                owner: "ana".into(),
                connector: None,
                project: "helsinki".into(),
                workspace: None,
                items: vec![item("air", Some(model)), item("bus", None)],
                expires_at: now + jiff::SignedDuration::from_mins(5),
            },
            now,
        );
        let peeked = store.peek("p1", "ana", "helsinki", now).expect("the plan");
        assert_eq!(
            peeked[0].source.as_deref(),
            Some("id: https://hel.fi/air\nname: air\n")
        );
        assert_eq!(peeked[1].source, None);
        assert!(
            store.peek("p1", "eva", "helsinki", now).is_none(),
            "another person's plan"
        );
    }

    fn air_model() -> ModelPlan {
        ModelPlan {
            source: "id: https://hel.fi/air\nname: air\n".into(),
            checked: SourceDryRunResult {
                severity: "patch".into(),
                changes: Vec::new(),
                version: "1.0.1".into(),
                artifacts: Default::default(),
            },
            next_val: Value::Null,
            imports: BTreeMap::new(),
            creating: false,
            base: "etag".into(),
        }
    }

    /// JC2-11: the paired tab opens a planned model's editor on its LinkML source, not its
    /// manifest; a kept plan that changes nothing opens nothing and says so.
    #[test]
    fn the_tab_opens_a_planned_model_on_its_source_and_an_empty_plan_on_nothing() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let admin = crate::ops::testkit::admin(&state);
        let now = TimestampTz::now();
        let keep = |id: &str, items: Vec<Planned>| {
            state.plans.keep(
                id.into(),
                Plan {
                    owner: admin.identity.subject.clone(),
                    connector: None,
                    project: "helsinki".into(),
                    workspace: None,
                    items,
                    expires_at: now + jiff::SignedDuration::from_mins(5),
                },
                now,
            )
        };
        keep(
            "p-model",
            vec![Planned {
                kind: "DataModel".into(),
                plural: "datamodels",
                home: "helsinki".into(),
                name: "air".into(),
                manifest: Some(json!({ "kind": "DataModel" })),
                base: "absent".into(),
                model: Some(air_model()),
            }],
        );
        keep("p-empty", Vec::new());
        let asker = crate::agents::oneshot::ui_actions::Asker {
            state: &state,
            identity: &admin.identity,
            project: "helsinki",
            connector: None,
        };
        let (frame, payload) = asker
            .show_diff(&json!({ "planId": "p-model" }))
            .expect("opened");
        assert_eq!(frame, "navigate");
        assert_eq!(
            payload["prefill"],
            json!({ "source": "id: https://hel.fi/air\nname: air\n" }),
            "{payload}"
        );
        assert_eq!(
            asker.show_diff(&json!({ "planId": "p-empty" })),
            Err("plan 'p-empty' changes nothing".to_owned())
        );
    }

    /// A full store drops its oldest plan for a new one, and a store whose lock a panicking
    /// thread poisoned still keeps, takes and shows plans (T-7817).
    #[test]
    fn a_full_or_poisoned_store_still_keeps_its_newest_plans() {
        let now = TimestampTz::now();
        let store = PlanStore::default();
        let plan = |owner: &str, minutes: i64| Plan {
            owner: owner.into(),
            connector: None,
            project: "helsinki".into(),
            workspace: None,
            items: Vec::new(),
            expires_at: now + jiff::SignedDuration::from_mins(minutes),
        };
        for n in 0..MAX_PLANS {
            store.keep(format!("p{n}"), plan("ana", 10 + n as i64), now);
        }
        store.keep("newest".into(), plan("ana", 5_000), now);
        assert!(
            store.peek("p0", "ana", "helsinki", now).is_none(),
            "the oldest went"
        );
        assert!(store.peek("p1", "ana", "helsinki", now).is_some());
        let poisoned = std::thread::scope(|s| {
            s.spawn(|| {
                let _held = store.plans.lock().expect("lock");
                std::panic::panic_any("a panic while holding the plans");
            })
            .join()
        });
        assert!(poisoned.is_err() && store.plans.is_poisoned());
        store.keep("after".into(), plan("eva", 30), now);
        assert!(store.peek("after", "eva", "helsinki", now).is_some());
        assert!(
            store.take("after", "ana", "helsinki", None, now).is_none(),
            "not ana's"
        );
        assert!(store.take("after", "eva", "helsinki", None, now).is_some());
        assert!(
            store.take("after", "eva", "helsinki", None, now).is_none(),
            "applied once"
        );
    }

    /// S-10: a breaking change counts the stored entities of the class it touches, also for a
    /// top-level slot a class lists.
    /// A breaking change's stored entities are not counted when the model names no space or the
    /// space's broker cannot count them, and the row says why; an additive change, or one of a
    /// class the published model does not hold, carries no count at all.
    #[tokio::test]
    async fn a_breaking_change_says_why_its_entities_were_not_counted() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, ResponseTemplate};
        let held = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(".*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "DefaultBranch": "main", "Content": "Y2xhc3NlczoKICBQYXJraW5nOgogICAgc2xvdHM6IFtzcGFjZXNdCnNsb3RzOgogIHNwYWNlczoKICAgIHJhbmdlOiBpbnRlZ2VyCg==", "Encoding": "base64",
                "Sha": "s1", "Path": "doprava/parking.linkml.yaml"
            })))
            .mount(&held)
            .await;
        let state = on_forge(&held).await;
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "DataModel".into(),
            metadata: crate::resource::ObjectMeta::new("parking", "doprava"),
            spec: json!({ "contextSpaceRef": "mobility", "linkml": "./parking.linkml.yaml" }),
            status: None,
        });
        let change = |severity: &str, subject: &str| datamodels::ModelChange {
            severity: severity.into(),
            subject: subject.into(),
            reason: "r".into(),
        };
        let changes = [
            change("breaking", "Parking.spaces"),
            change("additive", "Parking.name"),
            change("breaking", "Kiosk.name"),
        ];
        for (manifest, why) in [
            (json!({ "spec": {} }), "the model names no contextSpaceRef"),
            (
                json!({ "spec": { "contextSpaceRef": "mobility" } }),
                "could not be counted",
            ),
        ] {
            let rows = affected(&state, "doprava", "parking", &manifest, &changes).await;
            assert!(rows[0]["affectedEntities"].is_null(), "{rows:?}");
            assert!(
                rows[0]["notCounted"]
                    .as_str()
                    .is_some_and(|text| text.contains(why)),
                "{rows:?}"
            );
            for row in &rows[1..] {
                assert!(row.get("notCounted").is_none(), "{row}");
            }
        }
    }

    #[test]
    fn a_change_touches_its_class_or_every_class_listing_its_slot() {
        let published = json!({ "classes": {
            "BikeCounter": { "slots": ["owner"] },
            "Station": { "slots": ["owner", "name"] },
            "Kiosk": {}
        } });
        assert_eq!(classes_of(&published, "BikeCounter.count"), ["BikeCounter"]);
        assert_eq!(classes_of(&published, "Kiosk"), ["Kiosk"]);
        assert_eq!(classes_of(&published, "owner"), ["BikeCounter", "Station"]);
        assert!(classes_of(&published, "Direction.in").is_empty());
        assert!(classes_of(&Value::Null, "BikeCounter.count").is_empty());
    }

    fn caller(access: Option<crate::agents::access::Access>) -> Caller {
        Caller {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub-jana".into(),
                username: "jana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via: super::super::Via::Mcp,
            access,
            bearer: None,
        }
    }

    fn create(kind: &str, name: &str) -> ChangeInput {
        ChangeInput {
            op: Op::Create,
            kind: kind.into(),
            name: name.into(),
            manifest: Some(json!({
                "apiVersion": crate::resource::API_VERSION,
                "kind": kind,
                "metadata": { "name": name, "namespace": "doprava" },
                "spec": {}
            })),
            patch: None,
            source: None,
        }
    }

    /// Each op is refused at the member that breaks it: a create over an object that exists, a
    /// replace, patch or delete of one that does not, a delete carrying a body, an op carrying
    /// the other body, and a manifest naming another kind, name or namespace. A delete without a
    /// body removes; a patch merges into the object without its status.
    #[test]
    fn each_op_is_refused_at_the_member_that_breaks_it() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: crate::resource::ObjectMeta::new("held", "doprava"),
            spec: json!({ "contextSpaceRef": "parking", "audience": "public" }),
            status: None,
        });
        let checked = |change: &ChangeInput| manifest_of(&state, "/changes/0", "doprava", change);
        let refused = |change: ChangeInput| checked(&change).expect_err("refused").to_string();
        let with = |op: Op, name: &str| {
            let mut change = create("Endpoint", name);
            change.op = op;
            change
        };
        let bare = |op: Op, name: &str| {
            let mut change = with(op, name);
            change.manifest = None;
            change
        };
        let gone = "invalid input at /changes/0/name: Endpoint 'gone' does not exist in project \
                    doprava: use op create";
        assert_eq!(
            refused(with(Op::Create, "held")),
            "invalid input at /changes/0/op: Endpoint 'held' exists already: use op replace or patch"
        );
        assert_eq!(refused(with(Op::Replace, "gone")), gone);
        let mut patch = bare(Op::Patch, "gone");
        patch.patch = Some(json!({ "spec": {} }));
        assert_eq!(refused(patch), gone);
        assert_eq!(refused(bare(Op::Delete, "gone")), gone);
        assert_eq!(
            refused(with(Op::Delete, "held")),
            "invalid input at /changes/0: op delete takes neither manifest nor patch"
        );
        assert_eq!(checked(&bare(Op::Delete, "held")).expect("a removal"), None);
        let mut both = with(Op::Replace, "held");
        both.patch = Some(json!({}));
        assert_eq!(
            refused(both),
            "invalid input at /changes/0/manifest: op replace takes manifest alone"
        );
        let mut kind = with(Op::Create, "new");
        kind.manifest = Some(json!({ "kind": "Pipeline", "metadata": { "name": "new" } }));
        assert_eq!(
            refused(kind),
            "invalid input at /changes/0/manifest/kind: the manifest's kind must be the \
             change's, 'Endpoint'"
        );
        let mut name = with(Op::Create, "new");
        name.manifest = Some(json!({ "kind": "Endpoint", "metadata": { "name": "other" } }));
        assert_eq!(
            refused(name),
            "invalid input at /changes/0/manifest/metadata/name: the manifest's metadata/name \
             must be the change's, 'new'"
        );
        let mut elsewhere = with(Op::Create, "new");
        elsewhere.manifest = Some(
            json!({ "kind": "Endpoint", "metadata": { "name": "new", "namespace": "kosice" } }),
        );
        assert_eq!(
            refused(elsewhere),
            "invalid input at /changes/0/manifest/metadata/namespace: Endpoint lives in \
             namespace 'doprava', not 'kosice': set metadata.namespace to 'doprava' or leave it out"
        );
        let mut carried = with(Op::Patch, "held");
        carried.patch = Some(json!({}));
        assert_eq!(
            refused(carried),
            "invalid input at /changes/0/patch: op patch takes patch alone"
        );
        let mut merge = bare(Op::Patch, "held");
        merge.patch = Some(json!({ "spec": { "audience": "organization" } }));
        let merged = checked(&merge).expect("a patch").expect("a manifest");
        assert_eq!(merged["spec"]["audience"], "organization");
        assert_eq!(merged["spec"]["contextSpaceRef"], "parking");
        assert!(merged.get("status").is_none(), "{merged}");
    }

    /// A plan naming one object twice is refused at the second change; a model's source on a
    /// delete of a model that exists is refused at the source.
    #[tokio::test]
    async fn a_plan_naming_an_object_twice_or_a_source_on_a_removal_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let input = PlanInput {
            changes: vec![create("Endpoint", "twice"), create("Endpoint", "twice")],
            workspace: None,
        };
        let why = plan(&caller(None), &state, "doprava", input)
            .await
            .expect_err("refused")
            .to_string();
        assert_eq!(
            why,
            "invalid input at /changes/1: Endpoint 'twice' is in this plan twice"
        );
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "DataModel".into(),
            metadata: crate::resource::ObjectMeta::new("bike-counter", "doprava"),
            spec: json!({ "contextSpaceRef": "parking", "linkml": "./bike.linkml.yaml" }),
            status: None,
        });
        let removal = ChangeInput {
            op: Op::Delete,
            kind: "DataModel".into(),
            name: "bike-counter".into(),
            manifest: None,
            patch: None,
            source: Some("classes: {}".into()),
        };
        let input = PlanInput {
            changes: vec![removal],
            workspace: None,
        };
        let why = plan(&caller(None), &state, "doprava", input)
            .await
            .expect_err("refused")
            .to_string();
        assert!(why.contains("/changes/0/source"), "{why}");
        assert!(why.contains("op delete takes no source"), "{why}");
    }

    /// T-6282: two or more creates of the project's own objects are checked and opened as one
    /// Change, a model with its LinkML file among them (JC2-10); a single change, a workspace or
    /// a removal is not.
    #[test]
    fn a_plan_of_several_changes_is_checked_as_one_bundle() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let plan = |changes: Vec<ChangeInput>, workspace: Option<&str>| PlanInput {
            changes,
            workspace: workspace.map(str::to_owned),
        };
        let space = || create("ContextSpace", "parking");
        let endpoint = || create("Endpoint", "doprava-parking");
        let bundle = together(&state, "doprava", &plan(vec![space(), endpoint()], None))
            .expect("two creates are one bundle");
        assert_eq!(bundle.len(), 2);
        assert_eq!(bundle[1]["metadata"]["name"], "doprava-parking");
        assert!(together(&state, "doprava", &plan(vec![space()], None)).is_none());
        assert!(together(
            &state,
            "doprava",
            &plan(vec![space(), endpoint()], Some("w"))
        )
        .is_none());
        let mut removal = endpoint();
        removal.op = Op::Delete;
        removal.manifest = None;
        assert!(together(&state, "doprava", &plan(vec![space(), removal], None)).is_none());
        let mut model = create("DataModel", "parking");
        model.source = Some("id: x".into());
        let bundle = together(&state, "doprava", &plan(vec![model, space()], None))
            .expect("JC2-10: a model with its source stays in the bundle");
        assert_eq!(bundle[0]["kind"], "DataModel");
    }

    /// EP-75 (T-6285): a connector planning an Endpoint with a slug it wrote is refused at the
    /// change's own path, before anything is checked or kept; so is an update moving the slug.
    #[tokio::test]
    async fn a_planned_endpoint_slug_the_caller_wrote_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let typed = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let input = PlanInput {
            changes: vec![ChangeInput {
                op: Op::Create,
                kind: "Endpoint".into(),
                name: "t6242-probe".into(),
                manifest: Some(json!({
                    "apiVersion": crate::resource::API_VERSION,
                    "kind": "Endpoint",
                    "metadata": { "name": "t6242-probe", "namespace": "doprava" },
                    "spec": { "contextSpaceRef": "air", "audience": "public", "slug": typed }
                })),
                patch: None,
                source: None,
            }],
            workspace: None,
        };
        let got = plan(&caller(None), &state, "doprava", input).await;
        assert!(
            matches!(&got, Err(OpError::InvalidInput { path, message })
                if path == "/changes/0/manifest/spec/slug"
                    && message.contains("leave spec.slug out")
                    && !message.contains(typed)),
            "expected a refusal at the slug, got {got:?}"
        );
    }

    /// T-6391: a Policy whose assigner an AI client filled with an e-mail is refused naming
    /// spec.assigner, so the address never reaches a proposal branch.
    #[tokio::test]
    async fn a_planned_policy_with_an_e_mail_assigner_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let input = PlanInput {
            changes: vec![ChangeInput {
                op: Op::Create,
                kind: "Policy".into(),
                name: "t6377-read".into(),
                manifest: Some(json!({
                    "apiVersion": crate::resource::API_VERSION,
                    "kind": "Policy",
                    "metadata": { "name": "t6377-read", "namespace": "doprava" },
                    "spec": {
                        "contextSpaceRef": "air", "assigner": "owner@example.com",
                        "assignee": { "kind": "role", "id": "public" },
                        "operations": ["retrieveOps"],
                        "information": [{ "entities": [{ "type": "AirQualityObserved" }] }]
                    }
                })),
                patch: None,
                source: None,
            }],
            workspace: None,
        };
        let mut approver = caller(None);
        approver.identity.groups = vec!["portal-approver".into()];
        let refusal = format!("{:?}", plan(&approver, &state, "doprava", input).await);
        assert!(refusal.starts_with("Err("), "{refusal}");
        assert!(refusal.contains("spec.assigner"), "{refusal}");
        assert!(refusal.contains("did:web:{orgDomain}"), "{refusal}");
    }

    fn kept(store: &PlanStore, owner: &str, connector: Option<&str>, minutes: i64) -> String {
        let id = new_id();
        store.keep(
            id.clone(),
            Plan {
                owner: owner.into(),
                connector: connector.map(str::to_owned),
                project: "doprava".into(),
                workspace: None,
                items: vec![],
                expires_at: TimestampTz::now() + jiff::SignedDuration::from_mins(minutes),
            },
            TimestampTz::now(),
        );
        id
    }

    /// A plan answers only its own caller, connector and project, before it expires, and once.
    #[test]
    fn a_plan_is_taken_once_by_its_owner_through_its_connector() {
        let store = PlanStore::default();
        let now = TimestampTz::now();
        let id = kept(&store, "sub-jana", Some("c1"), TTL_MINUTES);
        let take = |id: &str, owner: &str, project: &str, connector: Option<&str>| {
            store.take(id, owner, project, connector, now).is_some()
        };
        assert!(!take(&id, "sub-eva", "doprava", Some("c1")));
        assert!(!take(&id, "sub-jana", "doprava", Some("c2")));
        assert!(!take(&id, "sub-jana", "doprava", None));
        // F-R2POR4-017: another project is not there, and the plan stays for its own.
        assert!(!take(&id, "sub-jana", "espoo", Some("c1")));
        assert!(take(&id, "sub-jana", "doprava", Some("c1")));
        assert!(!take(&id, "sub-jana", "doprava", Some("c1")));
        let stale = kept(&store, "sub-jana", None, -1);
        assert!(!take(&stale, "sub-jana", "doprava", None));
    }

    /// A LinkML source rides only with a DataModel, never with a delete or into a workspace, and a
    /// plan's sources together stay under one source's cap: each refused at its path before
    /// anything is read or kept.
    #[tokio::test]
    async fn a_source_is_refused_off_a_model_in_a_workspace_and_over_the_cap() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let change = |op: Op, kind: &str, source: &str| ChangeInput {
            op,
            kind: kind.into(),
            name: "bike-counter".into(),
            manifest: None,
            patch: None,
            source: Some(source.into()),
        };
        let refused = |changes: Vec<ChangeInput>, workspace: Option<&str>| {
            let input = PlanInput {
                changes,
                workspace: workspace.map(str::to_owned),
            };
            let state = &state;
            async move {
                plan(&caller(None), state, "doprava", input)
                    .await
                    .expect_err("refused")
                    .to_string()
            }
        };
        let off_model = refused(vec![change(Op::Patch, "ContextSpace", "classes: {}")], None).await;
        assert!(off_model.contains("/changes/0/source"), "{off_model}");
        assert!(off_model.contains("only a DataModel"), "{off_model}");
        let delete = refused(vec![change(Op::Delete, "DataModel", "classes: {}")], None).await;
        assert!(delete.contains("op delete takes no source"), "{delete}");
        let workspace = refused(
            vec![change(Op::Patch, "DataModel", "classes: {}")],
            Some("w1"),
        )
        .await;
        assert!(workspace.contains("without workspace"), "{workspace}");
        let half = "x".repeat(MAX_REQUEST_BYTES / 2 + 1);
        let mut other = change(Op::Patch, "DataModel", &half);
        other.name = "station".into();
        let over = refused(vec![change(Op::Patch, "DataModel", &half), other], None).await;
        assert!(over.contains("one at a time"), "{over}");
        let kept = state.plans.plans.lock().map(|p| p.len()).unwrap_or(1);
        assert_eq!(kept, 0);
    }

    /// A connector with writes off is refused at the apply before any plan is looked at.
    #[tokio::test]
    async fn a_connector_with_writes_off_is_refused_at_the_apply() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector = crate::connectors::Connector {
            id: "c1".into(),
            name: "doprava-in-claude".into(),
            owner: "sub-jana".into(),
            segments: vec!["context".into()],
            scope: vec![],
            kinds: None,
            writes: false,
            autonomous: false,
            expires_at: TimestampTz::now() + jiff::SignedDuration::from_hours(24),
            allowed_clients: vec![],
            client_is_boundary: false,
            state: "active".into(),
            created_at: TimestampTz::now(),
            granted_at: TimestampTz::now(),
            over: vec![],
            last_used_at: None,
            url: "https://portal.test/c/c1".into(),
            limits: crate::connectors::DEFAULT_LIMITS,
            above_ceiling: vec![],
            approval: None,
            narrowed_by: vec![],
        };
        let id = kept(&state.plans, "sub-jana", Some("c1"), TTL_MINUTES);
        let err = apply(
            &caller(Some(connector.access())),
            &state,
            "doprava",
            ApplyInput {
                plan_id: id.clone(),
            },
        )
        .await
        .expect_err("writes off");
        assert!(err.to_string().contains("writes off"), "{err}");
        // The plan stays for the Portal.
        assert!(state
            .plans
            .take(&id, "sub-jana", "doprava", Some("c1"), TimestampTz::now())
            .is_some());
    }

    /// T-7541 (S1-14): an autonomous connector approves nothing at an apply its owner's ceiling
    /// does not allow; under one that does, the approval page's own checks decide.
    #[tokio::test]
    async fn an_autonomous_apply_waits_when_the_ceiling_refuses() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector: crate::connectors::Connector = state
            .connectors
            .create(
                "sub-jana",
                serde_json::from_value(json!({
                    "name": "auto", "segments": ["context"], "writes": true, "autonomous": true,
                    "scope": [{ "project": "doprava", "access": "write" }],
                    "expiresAt": "2999-01-01T00:00:00Z",
                }))
                .expect("request"),
                Default::default(),
                "https://portal.test",
            )
            .await
            .expect("kept");
        let envelope = |kind: &str, spec: Value| crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(
                "stewards",
                crate::permissions::ORG_NAMESPACE,
            ),
            spec,
            status: None,
        };
        state.mirror.upsert(envelope(
            "Group",
            json!({ "members": [{ "user": "jana" }] }),
        ));
        let policy = |autonomous: bool| {
            envelope(
                "DelegationPolicy",
                json!({ "subjects": [{ "kind": "Group", "name": "stewards" }],
                        "projects": ["doprava"], "segments": ["context"],
                        "writes": true, "autonomous": autonomous }),
            )
        };
        let applied = |state: AppState, connector: crate::connectors::Connector| async move {
            let mut opened = vec![json!({ "id": "chg-00000001" })];
            approve_autonomously(&state, &caller(None), "doprava", &connector, &mut opened).await
        };
        state.mirror.upsert(policy(false));
        let waiting = applied(state.clone(), connector.clone()).await;
        let why = waiting[0]["waits"].as_str().expect("waits");
        assert!(why.starts_with("autonomous:"), "{why}");
        // Allowed, it reaches the approval path, which has no forge to merge on here.
        state.mirror.upsert(policy(true));
        let waiting = applied(state.clone(), connector.clone()).await;
        let why = waiting[0]["waits"].as_str().expect("waits");
        assert!(!why.starts_with("autonomous:"), "{why}");
        // A row that opened no change (it carries no id) has nothing to approve.
        let mut opened = vec![json!({ "error": "refused" })];
        let waiting =
            approve_autonomously(&state, &caller(None), "doprava", &connector, &mut opened).await;
        assert!(waiting.is_empty(), "{waiting:?}");
        assert_eq!(opened, [json!({ "error": "refused" })]);
    }

    /// Under a ceiling that lets it, an autonomous connector's change to a DataModel is approved
    /// as its owner: the row carries the change's state and the connector, and nothing waits.
    #[tokio::test]
    async fn an_autonomous_apply_approves_its_owners_data_model() {
        use base64::Engine;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        const PROPOSER: &str = "jana.kovacova@banskabystrica.sk";
        let server = wiremock::MockServer::start().await;
        let repo = "/odata/Repositories(Owner='test-owner',Name='test-repo')";
        let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
        let yaml = format!(
            "apiVersion: {}\nkind: DataModel\nmetadata:\n  name: datamodel-x\n  namespace: doprava\n\
             spec: {{\"version\": \"1.0.0\", \"linkml\": \"models/air.linkml.yaml\", \"lifecycle\": \"draft\"}}\n",
            crate::resource::API_VERSION
        );
        let info = crate::resource::by_kind("DataModel").expect("a served kind");
        let file =
            crate::resource::repository_path(info, "doprava", None, "datamodel-x").expect("a path");
        Mock::given(method("GET"))
            .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
            .respond_with(ok(json!({
                "Sha": "blob-1",
                "Content": base64::engine::general_purpose::STANDARD.encode(&yaml)
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{repo}/PullRequests(1)")))
            .respond_with(ok(json!({
                "Number": 1, "HtmlUrl": "https://shako.example.sk/pulls/1", "State": "open",
                "Title": "create DataModel", "Head": "portal/create-datamodel-x-11111111",
                "HeadSha": "head-1", "Base": "main", "CreatedAt": "2026-11-03T05:14:22Z",
                "Author": "jana.kovacova", "AuthorName": "jana.kovacova",
                "AuthorEmail": PROPOSER, "Mergeable": true, "Merged": false
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{repo}/PullRequests(1)/Files")))
            .respond_with(ok(
                json!({ "value": [{ "Filename": file, "Status": "added" }] }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/PullRequests(1)/Shako.Merge")))
            .respond_with(ok(json!({})))
            .mount(&server)
            .await;
        let client = crate::forge::ShakoClient::new(
            server.uri().parse().expect("url"),
            "test-owner",
            "test-repo",
            "token-xyz",
        )
        .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(client));
        let org = |kind: &str, name: &str, spec: Value| crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, crate::permissions::ORG_NAMESPACE),
            spec,
            status: None,
        };
        state.mirror.upsert(org(
            "Role",
            "admin-role",
            json!({ "rules": [{ "kinds": ["DataModel"], "verbs": ["read", "propose", "approve", "delete"] }] }),
        ));
        state.mirror.upsert(org(
            "RoleBinding",
            "admin-binding",
            json!({ "subjects": [{ "user": PROPOSER }], "role": "admin-role",
                    "scope": { "organization": "bb" } }),
        ));
        state.mirror.upsert(org(
            "Group",
            "stewards",
            json!({ "members": [{ "user": "jana.kovacova" }] }),
        ));
        state.mirror.upsert(org(
            "DelegationPolicy",
            "stewards",
            json!({ "subjects": [{ "kind": "Group", "name": "stewards" }],
                    "projects": ["doprava"], "segments": ["context"],
                    "writes": true, "autonomous": true }),
        ));
        let connector: crate::connectors::Connector = state
            .connectors
            .create(
                "sub-jana",
                serde_json::from_value(json!({
                    "name": "auto", "segments": ["context"], "writes": true, "autonomous": true,
                    "scope": [{ "project": "doprava", "access": "write" }],
                    "expiresAt": "2999-01-01T00:00:00Z",
                }))
                .expect("request"),
                Default::default(),
                "https://portal.test",
            )
            .await
            .expect("kept");
        let mut jana = caller(None);
        jana.identity.client = Some("claude-code".into());
        jana.identity.username = "jana.kovacova".into();
        jana.identity.email = Some(PROPOSER.into());
        jana.identity.name = Some("jana.kovacova".into());
        let mut opened = vec![json!({ "id": "chg-00000001" })];
        let waiting = approve_autonomously(&state, &jana, "doprava", &connector, &mut opened).await;
        assert!(waiting.is_empty(), "{waiting:?}");
        assert_eq!(opened[0]["approvedVia"], "auto", "{opened:?}");
        assert!(opened[0]["state"].is_string(), "{opened:?}");
        assert!(opened[0].get("waits").is_none(), "{opened:?}");
    }

    /// T-5979: a notice the approvers' inbox cannot take is logged; the change stays opened and
    /// the apply goes on (T-7921).
    #[tokio::test]
    async fn an_approvers_notice_that_cannot_be_written_leaves_the_apply_going() {
        let mirror = crate::test_db::placed_mirror("doprava");
        let mut state =
            AppState::new(crate::config::Config::for_tests(), None).with_mirror(mirror.clone());
        state.comments = std::sync::Arc::new(crate::entity_comments::CommentStore::new(
            Some(crate::test_db::dead_pool()),
            mirror,
        ));
        let opened = [
            json!({ "id": "chg-00000001" }),
            json!({ "error": "refused" }),
        ];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            notify_approvers(&state, &caller(None), "doprava", "auto", &opened),
        )
        .await
        .expect("the notice gives up instead of holding the apply");
    }

    /// T-6302: create or edit is judged from the forge, never from the client's `op`; a
    /// connector holding only `create` on Endpoint cannot touch an existing one, by any op.
    #[tokio::test]
    async fn a_create_only_connector_is_refused_an_existing_object_whatever_op_it_sends() {
        use crate::connectors::KindGrant;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let held: crate::resource::ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "counters", "namespace": "doprava" },
            "spec": {}
        }))
        .expect("an envelope");
        state.mirror.upsert(held);
        let mut connector = crate::connectors::tests::sample();
        connector.segments = vec!["context".into()];
        connector.scope = vec![crate::connectors::ScopeItem {
            project: "doprava".into(),
            space: None,
            endpoint: None,
            pipeline: None,
            access: crate::connectors::ItemAccess::Read,
            personal_data: false,
        }];
        connector.kinds = Some(vec![KindGrant {
            kind: "Endpoint".into(),
            verbs: vec![ConnectorVerb::Read, ConnectorVerb::Create],
        }]);
        let through = caller(Some(connector.access()));
        let denied = |answer: Result<Value, OpError>| {
            let refusal = answer.expect_err("a refusal");
            assert!(
                matches!(refusal, OpError::Api(ApiError::Denied(_))),
                "{refusal:?}"
            );
            refusal.to_string()
        };
        let plan_of = |change: ChangeInput| PlanInput {
            changes: vec![change],
            workspace: None,
        };
        // An `op: create` naming the existing Endpoint is an edit.
        let why = denied(
            plan(
                &through,
                &state,
                "doprava",
                plan_of(create("Endpoint", "counters")),
            )
            .await,
        );
        assert!(why.contains("may not edit Endpoint counters"), "{why}");
        assert!(why.contains("read, create"), "{why}");
        let mut removal = create("Endpoint", "counters");
        removal.op = Op::Delete;
        removal.manifest = None;
        let why = denied(plan(&through, &state, "doprava", plan_of(removal)).await);
        assert!(why.contains("may not delete Endpoint counters"), "{why}");
        // A new Endpoint passes the verb check; what follows is the kind's own validation.
        let fresh = plan(
            &through,
            &state,
            "doprava",
            plan_of(create("Endpoint", "fresh")),
        )
        .await;
        // (The test person holds no role, so the person's own check refuses it next.)
        let why = format!("{fresh:?}");
        assert!(
            !why.contains("may not"),
            "a create was refused as a verb: {why}"
        );
        // A kind the list leaves out is read only.
        let why = denied(
            plan(
                &through,
                &state,
                "doprava",
                plan_of(create("ContextSpace", "parking")),
            )
            .await,
        );
        assert!(why.contains("may not create ContextSpace parking"), "{why}");
    }

    /// A forge that holds nothing yet and takes every write; `open` is the head branch of a
    /// change already open on it.
    /// A revert is refused, with nothing planned, when main moved the files since, when the change
    /// touched files no plan writes, or when it left nothing; otherwise it may be planned.
    #[test]
    fn a_revert_is_refused_when_main_moved_or_outside_files_or_nothing_is_left() {
        let one = [create("Endpoint", "air")];
        let answer = revertible("c1", &["projects/p/a.yaml".into()], Vec::new(), &one);
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Conflict(why))) if why.starts_with("main changed projects/p/a.yaml since change 'c1'") && why.contains(PLAN)),
            "{answer:?}"
        );
        let answer = revertible("c1", &[], vec!["README.md".into()], &one);
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Unprocessable { detail, errors })) if detail.contains("touched files a plan cannot write") && *errors == ["README.md"]),
            "{answer:?}"
        );
        let answer = revertible("c1", &[], Vec::new(), &[]);
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Conflict(why))) if *why == "change 'c1' left nothing to revert"),
            "{answer:?}"
        );
        assert!(revertible("c1", &[], Vec::new(), &one).is_ok());
    }

    /// Only a manifest of one of this Portal's kinds, with a name, is named.
    #[test]
    fn a_manifest_is_named_only_when_it_is_a_kind_of_this_portal() {
        assert_eq!(
            manifest_names(&json!({ "kind": "Endpoint", "metadata": { "name": "air" } })),
            Some(("Endpoint".to_owned(), "air".to_owned()))
        );
        assert_eq!(
            manifest_names(&json!({ "kind": "Deployment", "metadata": { "name": "x" } })),
            None
        );
        assert_eq!(
            manifest_names(&json!({ "kind": "Endpoint", "metadata": {} })),
            None
        );
        assert_eq!(
            manifest_names(&json!({ "metadata": { "name": "air" } })),
            None
        );
    }

    /// The forge's pull request 7 as `row` adds to a merged Portal change.
    async fn forge_with_pull(row: Value, status: u16) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let mut pull = json!({ "Number": 7, "Head": "portal/air-1", "HeadSha": "h1", "Base": "main",
                               "Merged": true, "MergeBase": "b1", "MergeCommitSha": "m1" });
        for (key, value) in row.as_object().into_iter().flatten() {
            pull[key] = value.clone();
        }
        Mock::given(method("GET"))
            .and(path(
                "/odata/Repositories(Owner='o',Name='r')/PullRequests(7)",
            ))
            .respond_with(ResponseTemplate::new(status).set_body_json(pull))
            .mount(&server)
            .await;
        server
    }

    async fn merged_of(
        row: Value,
        status: u16,
    ) -> Result<(crate::forge::PullRequest, String), OpError> {
        let server = forge_with_pull(row, status).await;
        let forge =
            crate::forge::ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t")
                .expect("client");
        merged_change(&forge, 7, "doprava", "c7").await
    }

    /// A merged Portal or workspace change is answered with the commit it merged as, or its head
    /// when the forge names no merge commit.
    #[tokio::test]
    async fn a_merged_portal_change_answers_the_commit_it_merged_as() {
        let (pull, after) = merged_of(json!({}), 200).await.expect("merged");
        assert_eq!((pull.number, after.as_str()), (7, "m1"));
        let (_, after) = merged_of(
            json!({ "Head": "refs/heads/workspace/air", "MergeCommitSha": "" }),
            200,
        )
        .await
        .expect("merged");
        assert_eq!(after, "h1");
    }

    /// A pull request the forge does not have, or one the Portal did not open, is not found; one
    /// not merged yet is a conflict that says to reject it instead; a forge failure is its own.
    #[tokio::test]
    async fn a_change_that_is_not_a_merged_portal_change_is_refused() {
        for (row, status) in [(json!({}), 404), (json!({ "Head": "feature/x" }), 200)] {
            let answer = merged_of(row, status).await;
            assert!(
                matches!(&answer, Err(OpError::Api(ApiError::NotFound(why))) if *why == "change 'c7' not found in project 'doprava'"),
                "{answer:?}"
            );
        }
        for row in [json!({ "Merged": false }), json!({ "MergeBase": "" })] {
            let answer = merged_of(row, 200).await;
            assert!(
                matches!(&answer, Err(OpError::Api(ApiError::Conflict(why))) if why.contains("is not merged") && why.contains("reject it")),
                "{answer:?}"
            );
        }
        let answer = merged_of(json!({}), 500).await;
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Unavailable(why))) if why.starts_with("The Git repository did not answer")),
            "{answer:?}"
        );
    }

    async fn empty_forge(open: Option<String>) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        Mock::given(method("GET"))
            .and(path(repo))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })),
            )
            .mount(&server)
            .await;
        let pull = |number: u64, head: &str| {
            json!({
                "Number": number,
                "HtmlUrl": format!("https://forge.test/pulls/{number}"),
                "State": "open",
                "Title": "t",
                "Body": "",
                "Head": head,
                "Base": "main",
                "CreatedAt": "2026-11-01T20:00:00Z",
                "Author": "jana",
                "AuthorName": "Jana",
                "Mergeable": true,
                "Merged": false
            })
        };
        let pulls: Vec<Value> = open.iter().map(|head| pull(175, head)).collect();
        Mock::given(method("GET"))
            .and(path(format!("{repo}/PullRequests")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": pulls })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.OpenPullRequest")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(pull(176, "portal/import-plan-p1")),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CreateBranch")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c0ffee" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{}/Shako.File(Path=@p,Ref=@r)", repo)))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
            )
            .mount(&server)
            .await;
        server
    }

    /// A dry run of an inline manifest that names no draft answers its verdict from the forge
    /// (T-7921).
    #[tokio::test]
    async fn an_inline_dry_run_answers_its_verdict() {
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        let space = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "ContextSpace",
            "metadata": { "name": "parks", "namespace": "doprava" },
            "spec": { "isSandbox": true }
        });
        let op = crate::ops::find("jc_manifest_dry_run").expect("registered");
        let checked = (op.run)(&steward(), &state, "doprava", json!({ "manifest": space }))
            .await
            .expect("a verdict");
        assert_eq!(checked["valid"], true, "{checked}");
        assert_eq!(checked["verdict"]["ok"], true, "{checked}");
    }

    /// A revert asked by someone who does not read the project is the same 404 as a change
    /// that is not there (T-7921).
    #[tokio::test]
    async fn a_revert_for_an_outsider_is_a_change_not_found() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let op = crate::ops::find("jc_change_revert_plan").expect("registered");
        let refused = (op.run)(
            &caller(None),
            &state,
            "doprava",
            json!({ "id": "chg-00000035" }),
        )
        .await
        .expect_err("refused");
        assert!(
            matches!(&refused, OpError::Api(ApiError::NotFound(why))
                if why == "change 'chg-00000035' not found in project 'doprava'"),
            "{refused:?}"
        );
    }

    fn steward() -> Caller {
        let mut steward = caller(None);
        steward.identity.groups = vec!["portal-approver".into()];
        steward
    }

    /// A plan creating DataModel `parking` with its LinkML file and the space naming it.
    fn model_and_space() -> Plan {
        let model = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "DataModel",
            "metadata": { "name": "parking", "namespace": "doprava" },
            "spec": { "contextSpaceRef": "mobility", "linkml": "./parking.linkml.yaml",
                      "version": "1.0.0", "lifecycle": "draft" }
        });
        let space = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "ContextSpace",
            "metadata": { "name": "mobility", "namespace": "doprava" },
            "spec": { "isSandbox": true, "dataModelRef": "parking" }
        });
        let item = |kind: &str, plural: &'static str, name: &str, manifest: Value| Planned {
            kind: kind.into(),
            plural,
            home: "doprava".into(),
            name: name.into(),
            manifest: Some(manifest),
            base: "absent".into(),
            model: None,
        };
        let mut parking = item("DataModel", "datamodels", "parking", model);
        parking.model = Some(ModelPlan {
            source: "id: https://example.org/parking\nname: parking\n".into(),
            checked: SourceDryRunResult {
                severity: "none".into(),
                changes: vec![],
                version: "1.0.0".into(),
                artifacts: Default::default(),
            },
            next_val: json!({ "classes": { "Parking": {} } }),
            imports: BTreeMap::new(),
            creating: true,
            base: String::new(),
        });
        Plan {
            owner: "sub-jana".into(),
            connector: None,
            project: "doprava".into(),
            workspace: None,
            items: vec![parking, item("ContextSpace", "spaces", "mobility", space)],
            expires_at: TimestampTz::now() + jiff::SignedDuration::from_mins(TTL_MINUTES),
        }
    }

    async fn on_forge(server: &wiremock::MockServer) -> AppState {
        let forge =
            crate::forge::ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t")
                .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(forge));
        let plan = model_and_space();
        let space = plan.items[1].manifest.as_ref().expect("manifest");
        super::super::proposals::record_check(
            &steward(),
            &state,
            "doprava",
            space,
            &super::super::verdict::Verdict::green(space, None),
        )
        .await;
        state
    }

    /// [`open_together`] with the plan's manifests as the bundle in scope, as [`apply`] runs it.
    async fn opened_together(state: &AppState, plan: &Plan) -> Result<Vec<Value>, OpError> {
        let manifests = plan
            .items
            .iter()
            .filter_map(|i| i.manifest.clone())
            .collect();
        crate::references::BUNDLE
            .scope(manifests, open_together(&steward(), state, "p1", plan))
            .await
    }

    /// JC2-10: a bundled plan holding a DataModel with its LinkML file commits the model's
    /// manifest, source and artifacts onto the plan's one branch and opens one Change for it all.
    #[tokio::test]
    async fn a_bundled_plan_commits_the_model_with_its_source_onto_the_plan_branch() {
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        let plan = model_and_space();
        assert!(bundled(&plan));
        let rows = opened_together(&state, &plan).await;
        let requests = server.received_requests().await.expect("requests");
        let seen: Vec<String> = requests
            .iter()
            .map(|r| format!("{} {}", r.method, r.url))
            .collect();
        assert!(
            rows.is_ok(),
            "the plan opens: {rows:?}; forge saw {seen:#?}"
        );
        let rows = rows.expect("checked above");
        assert_eq!(rows.len(), 1, "one Change: {rows:?}");

        let commits: Vec<Value> = requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.CommitFiles"))
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(commits.len(), 2, "one commit per change");
        let model = commits
            .iter()
            .find(|c| c["Files"].as_array().is_some_and(|f| f.len() > 2))
            .expect("the model's commit");
        assert_eq!(model["Branch"], "portal/import-plan-p1");
        let paths: Vec<&str> = model["Files"]
            .as_array()
            .expect("files")
            .iter()
            .filter_map(|f| f["path"].as_str())
            .collect();
        for wanted in [
            "parking.yaml",
            "parking.linkml.yaml",
            "json-schema/parking.v1.json",
            "context/parking.v1.jsonld",
        ] {
            assert!(
                paths.iter().any(|p| p.ends_with(wanted)),
                "{wanted} in {paths:?}"
            );
        }
        assert!(
            commits
                .iter()
                .all(|c| c["Branch"] == "portal/import-plan-p1"),
            "every write lands on the plan's branch"
        );
        let opened: Vec<Value> = requests
            .iter()
            .filter(|r| {
                r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.OpenPullRequest")
            })
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(opened.len(), 1, "one merge request");
        assert_eq!(opened[0]["Head"], "portal/import-plan-p1");
    }

    /// CC-34 (JC2-10): a change already open on the model refuses the plan before a byte of it is
    /// written, and names the open change.
    /// A plan whose second write the forge refuses opens nothing and says which item failed;
    /// a branch the forge will not delete afterwards is logged, not a second error.
    #[tokio::test]
    async fn a_plan_the_forge_refuses_halfway_opens_nothing_and_names_the_item() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        let server = empty_forge(None).await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c0ffee" })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.DeleteBranch")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let state = on_forge(&server).await;
        let why = opened_together(&state, &model_and_space())
            .await
            .expect_err("the second write is refused")
            .to_string();
        assert!(why.contains("ContextSpace mobility was refused"), "{why}");
        assert!(why.contains("nothing of the plan was opened"), "{why}");
        let requests = server.received_requests().await.expect("requests");
        assert!(
            requests
                .iter()
                .any(|r| r.url.path().ends_with("/Shako.DeleteBranch")),
            "the half-written branch is removed"
        );
        assert!(
            !requests
                .iter()
                .any(|r| r.url.path().ends_with("/Shako.OpenPullRequest")),
            "no Change opened"
        );
    }

    #[tokio::test]
    async fn a_bundled_plan_is_refused_while_the_model_has_an_open_change() {
        let own = crate::api::mutate::branch_name(
            "doprava",
            "datamodel",
            "parking",
            crate::change::Operation::Create,
        );
        let server = empty_forge(Some(own)).await;
        let state = on_forge(&server).await;
        let err = opened_together(&state, &model_and_space())
            .await
            .expect_err("an open change on the model");
        let why = err.to_string();
        assert!(why.contains("DataModel 'parking' is already open"), "{why}");
        assert!(why.contains("nothing of the plan was opened"), "{why}");
        let requests = server.received_requests().await.expect("requests");
        let written: Vec<&str> = requests
            .iter()
            .filter_map(|r| (r.method.as_str() != "GET").then_some(r.url.path()))
            .collect();
        assert!(written.is_empty(), "nothing written: {written:?}");
    }

    fn scoped_connector(writes: bool) -> crate::connectors::Connector {
        crate::connectors::Connector {
            id: "c9".into(),
            name: "doprava-in-claude".into(),
            owner: "sub-jana".into(),
            segments: vec!["context".into()],
            scope: vec![],
            kinds: None,
            writes,
            autonomous: false,
            expires_at: TimestampTz::now() + jiff::SignedDuration::from_hours(24),
            allowed_clients: vec![],
            client_is_boundary: false,
            state: "active".into(),
            created_at: TimestampTz::now(),
            granted_at: TimestampTz::now(),
            over: vec![],
            last_used_at: None,
            url: "https://portal.test/c/c9".into(),
            limits: crate::connectors::DEFAULT_LIMITS,
            above_ceiling: vec![],
            approval: None,
            narrowed_by: vec![],
        }
    }

    /// A connector reaching nothing of the project is refused a change there, as a denial (AG-144);
    /// a person's own call passes the connector's half untouched.
    #[test]
    fn a_connector_reaching_nothing_is_denied_the_change() {
        let through = caller(Some(scoped_connector(true).access()));
        let err = through_connector(&through, "doprava", "Endpoint", "air", None, true)
            .expect_err("no scope item");
        assert!(
            matches!(&err, OpError::Api(ApiError::Denied(why)) if why.contains("does not reach Endpoint air")),
            "{err:?}"
        );
        assert!(through_connector(&caller(None), "doprava", "Endpoint", "air", None, true).is_ok());
    }

    /// A model's published file: `absent` while no model of that name is mirrored or the forge
    /// holds no file, its hash once it does, and an unreachable forge an error, never `absent`.
    #[tokio::test]
    async fn a_model_source_etag_is_absent_hashed_or_an_error() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, ResponseTemplate};
        let bare = AppState::new(crate::config::Config::for_tests(), None);
        assert_eq!(
            source_etag(&bare, "doprava", "parking")
                .await
                .expect("absent"),
            "absent"
        );
        let model = crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "DataModel".into(),
            metadata: crate::resource::ObjectMeta::new("parking", "doprava"),
            spec: json!({ "contextSpaceRef": "mobility", "linkml": "./parking.linkml.yaml" }),
            status: None,
        };
        bare.mirror.upsert(model.clone());
        assert!(
            source_etag(&bare, "doprava", "parking").await.is_err(),
            "no forge: the file cannot be read, so its etag is not guessed"
        );
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        state.mirror.upsert(model.clone());
        assert_eq!(
            source_etag(&state, "doprava", "parking")
                .await
                .expect("absent"),
            "absent",
            "the forge holds no file"
        );
        let held = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(".*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "DefaultBranch": "main", "Content": "aWQ6IHgK", "Encoding": "base64",
                "Sha": "s1", "Path": "doprava/parking.linkml.yaml"
            })))
            .mount(&held)
            .await;
        let state = on_forge(&held).await;
        state.mirror.upsert(model);
        let etag = source_etag(&state, "doprava", "parking")
            .await
            .expect("a held file is hashed");
        assert_eq!(etag.len(), 64, "a sha256: {etag}");
    }

    /// The 422's lines: a finding's `at` wins over its dotted path, a path-less finding names the
    /// manifest, warnings and notes are left out, and a red diff with no finding still says the
    /// check failed.
    #[test]
    fn a_red_diff_lists_its_findings_or_says_the_check_failed() {
        let diff = json!({ "verdict": { "findings": [
            { "level": "error", "path": "spec.slug", "message": "taken" },
            { "level": "error", "at": "/source/classes", "message": "unknown" },
            { "level": "error", "path": "" },
            { "level": "warning", "path": "spec", "message": "w" },
            { "level": "info", "message": "i" }
        ] } });
        assert_eq!(
            findings_of("/changes/0", &diff),
            vec![
                "/changes/0/manifest/spec/slug: taken",
                "/changes/0/source/classes: unknown",
                "/changes/0/manifest: the check failed",
            ]
        );
        for empty in [
            json!({}),
            json!({ "verdict": { "findings": [{ "level": "info" }] } }),
        ] {
            assert_eq!(
                findings_of("/changes/2", &empty),
                vec!["/changes/2/manifest: the check failed"]
            );
        }
    }

    /// Without a forge a bundled plan opens nothing and says the forge is missing.
    #[tokio::test]
    async fn a_bundled_plan_without_a_forge_is_unavailable() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let err = opened_together(&state, &model_and_space())
            .await
            .expect_err("no forge");
        assert!(
            err.to_string().contains("git forge is not configured"),
            "{err}"
        );
    }

    /// An item with no manifest is passed over and an object that exists is proposed as an
    /// update; a forge that deletes the refused branch leaves no warning behind.
    #[tokio::test]
    async fn a_bundle_skips_a_removal_proposes_an_update_and_removes_a_refused_branch() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        let mut plan = model_and_space();
        let mut removal = plan.items[1].clone();
        removal.manifest = None;
        plan.items.insert(0, removal);
        let opened = opened_together(&state, &plan).await;
        assert!(
            opened.is_ok(),
            "a bundle passes over its removal: {opened:?}"
        );

        let server = empty_forge(None).await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c0ffee" })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.CommitFiles")))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/Shako.DeleteBranch")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let state = on_forge(&server).await;
        let mut plan = model_and_space();
        plan.items[1].base = "e1".into();
        let why = opened_together(&state, &plan)
            .await
            .expect_err("the second write is refused")
            .to_string();
        assert!(why.contains("ContextSpace mobility was refused"), "{why}");
    }

    /// A model in a bundle is refused to a person who may not propose models, and a model whose
    /// manifest is no manifest is refused as a bad request; nothing is opened either way.
    #[tokio::test]
    async fn a_bundled_model_is_refused_to_a_non_approver_and_when_malformed() {
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        let plan = model_and_space();
        let manifests = plan
            .items
            .iter()
            .filter_map(|i| i.manifest.clone())
            .collect();
        let why = crate::references::BUNDLE
            .scope(manifests, open_together(&caller(None), &state, "p1", &plan))
            .await
            .expect_err("no propose right")
            .to_string();
        assert!(why.contains("DataModel parking was refused"), "{why}");
        let mut plan = model_and_space();
        plan.items[0].manifest = Some(json!({ "kind": "DataModel", "spec": {} }));
        let err = model_proposal(
            &steward(),
            &state,
            &plan.items[0],
            &json!({ "kind": "DataModel" }),
        );
        assert!(
            matches!(&err, Err(ApiError::BadRequest(why)) if why.contains("invalid DataModel manifest")),
            "{:?}",
            err.as_ref().err()
        );
        let why = propose_source(&steward(), &state, &plan.items[1], &json!({}))
            .await
            .expect_err("a space has no model half")
            .to_string();
        assert!(why.contains("a model change without its source"), "{why}");
    }

    fn held(state: &AppState, kind: &str, name: &str, namespace: &str, spec: Value) {
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: crate::resource::ObjectMeta::new(name, namespace),
            spec,
            status: None,
        });
    }

    fn removal_of(kind: &str, plural: &'static str, name: &str, base: &str) -> Planned {
        Planned {
            kind: kind.into(),
            plural,
            home: "doprava".into(),
            name: name.into(),
            manifest: None,
            base: base.into(),
            model: None,
        }
    }

    /// A patch carrying only a model's LinkML source leaves the manifest as the object is now,
    /// without its status.
    #[test]
    fn a_patch_carrying_only_a_source_keeps_the_manifest() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        held(
            &state,
            "DataModel",
            "bike-counter",
            "doprava",
            json!({ "contextSpaceRef": "parking", "linkml": "./bike.linkml.yaml" }),
        );
        let change = ChangeInput {
            op: Op::Patch,
            kind: "DataModel".into(),
            name: "bike-counter".into(),
            manifest: None,
            patch: None,
            source: Some("classes: {}".into()),
        };
        let manifest = manifest_of(&state, "/changes/0", "doprava", &change)
            .expect("a patch")
            .expect("a manifest");
        assert_eq!(manifest["spec"]["linkml"], "./bike.linkml.yaml");
        assert_eq!(manifest["metadata"]["name"], "bike-counter");
        assert!(manifest.get("status").is_none(), "{manifest}");
    }

    /// An object kept in the organization's namespace (a RoleBinding) opens a change request of
    /// its own, so a plan holding one is no bundle.
    #[test]
    fn a_plan_with_an_organization_object_is_no_bundle() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let input = PlanInput {
            changes: vec![
                create("ContextSpace", "parking"),
                create("RoleBinding", "readers"),
            ],
            workspace: None,
        };
        assert!(together(&state, "doprava", &input).is_none());
    }

    /// MF-07: a removal of a space an Endpoint still names is refused at the plan, naming the
    /// Endpoint, and an apply of it opens nothing and says the same.
    #[tokio::test]
    async fn a_removal_of_a_referenced_space_is_refused_at_plan_and_at_apply() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        held(
            &state,
            "ContextSpace",
            "parking",
            "doprava",
            json!({ "isSandbox": true }),
        );
        held(
            &state,
            "Endpoint",
            "counters",
            "doprava",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "parking" } }),
        );
        let removal = ChangeInput {
            op: Op::Delete,
            kind: "ContextSpace".into(),
            name: "parking".into(),
            manifest: None,
            patch: None,
            source: None,
        };
        let refused = plan(
            &steward(),
            &state,
            "doprava",
            PlanInput {
                changes: vec![removal],
                workspace: None,
            },
        )
        .await;
        let named = "1 dependent resource blocks deletion: Endpoint counters";
        assert!(
            matches!(&refused, Err(OpError::Api(ApiError::Unprocessable { errors, .. }))
                if errors.iter().any(|e| *e == format!("/changes/0/manifest: {named}"))),
            "{refused:?}"
        );
        assert_eq!(state.plans.plans.lock().map(|p| p.len()).unwrap_or(1), 0);
        let item = removal_of(
            "ContextSpace",
            "spaces",
            "parking",
            &etag(&state, "doprava", "ContextSpace", "parking"),
        );
        let opened = open_one(&steward(), &state, &item, None).await;
        assert!(
            matches!(&opened, Err(ApiError::Conflict(why)) if why.starts_with(named)),
            "{opened:?}"
        );
    }

    /// A forge that takes a commit on workspace `w1`'s branch, with ContextSpace `parking` held
    /// and the workspace opened.
    async fn workspace_world() -> (wiremock::MockServer, AppState) {
        use base64::Engine as _;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        let manifest = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "ContextSpace",
            "metadata": { "name": "parking", "namespace": "doprava" },
            "spec": { "isSandbox": true }
        });
        let content = base64::engine::general_purpose::STANDARD.encode(manifest.to_string());
        for (verb, at, answer) in [
            ("GET", repo.to_owned(), json!({ "DefaultBranch": "main" })),
            (
                "GET",
                format!("{repo}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"),
                json!({ "Sha": "t1", "Tree": [], "Truncated": false }),
            ),
            (
                "GET",
                format!("{repo}/Shako.File(Path=@p,Ref=@r)"),
                json!({ "Sha": "s1", "Content": content }),
            ),
            (
                "POST",
                format!("{repo}/Shako.CommitFiles"),
                json!({ "Sha": "c0ffee" }),
            ),
        ] {
            Mock::given(method(verb))
                .and(path(at))
                .respond_with(ResponseTemplate::new(200).set_body_json(answer))
                .mount(&server)
                .await;
        }
        let forge =
            crate::forge::ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t")
                .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(forge));
        held(
            &state,
            "ContextSpace",
            "parking",
            "doprava",
            json!({ "isSandbox": true }),
        );
        state
            .workspaces
            .create(crate::ops::workspaces::Opening {
                name: "w1",
                title: None,
                project: "doprava",
                owner: "jana",
                base_revision: "b0",
                scope: crate::ops::workspaces::Scope::Project {},
                ttl_hours: 1,
            })
            .await
            .expect("a workspace");
        (server, state)
    }

    /// A removal into a workspace commits on the workspace's branch and opens no Change (CC-76).
    #[tokio::test]
    async fn a_removal_into_a_workspace_commits_on_its_branch() {
        let (_server, state) = workspace_world().await;
        let item = removal_of(
            "ContextSpace",
            "spaces",
            "parking",
            &etag(&state, "doprava", "ContextSpace", "parking"),
        );
        let opened = open_one(&steward(), &state, &item, Some("w1")).await;
        assert!(
            matches!(&opened, Ok(ProposeOutcome::Workspace(commit))
                if commit.workspace == "w1" && commit.branch == "workspace/w1"),
            "{opened:?}"
        );
    }

    /// An apply whose first change does not open answers that change's own error; one whose later
    /// change does not open is a 409 naming the change and what was opened before it.
    #[tokio::test]
    async fn an_apply_that_stops_names_the_change_and_what_opened_before_it() {
        let (_server, state) = workspace_world().await;
        held(
            &state,
            "ContextSpace",
            "blocked",
            "doprava",
            json!({ "isSandbox": true }),
        );
        held(
            &state,
            "Endpoint",
            "counters",
            "doprava",
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "blocked" } }),
        );
        let removal = |name: &str| {
            removal_of(
                "ContextSpace",
                "spaces",
                name,
                &etag(&state, "doprava", "ContextSpace", name),
            )
        };
        let kept = |items: Vec<Planned>, workspace: Option<&str>| {
            let id = new_id();
            state.plans.keep(
                id.clone(),
                Plan {
                    owner: "sub-jana".into(),
                    connector: None,
                    project: "doprava".into(),
                    workspace: workspace.map(str::to_owned),
                    items,
                    expires_at: TimestampTz::now() + jiff::SignedDuration::from_mins(5),
                },
                TimestampTz::now(),
            );
            ApplyInput { plan_id: id }
        };
        let named = "1 dependent resource blocks deletion: Endpoint counters";
        let first = apply(
            &steward(),
            &state,
            "doprava",
            kept(vec![removal("blocked")], None),
        )
        .await;
        assert!(
            matches!(&first, Err(OpError::Api(ApiError::Conflict(why))) if why.starts_with(named)),
            "{first:?}"
        );
        let later = apply(
            &steward(),
            &state,
            "doprava",
            kept(vec![removal("parking"), removal("blocked")], Some("w1")),
        )
        .await;
        assert!(
            matches!(&later, Err(OpError::Api(ApiError::Conflict(why)))
                if why.starts_with("ContextSpace blocked was not applied (")
                    && why.contains(named)
                    && why.contains("these were: [")
                    && why.contains("workspace/w1")),
            "{later:?}"
        );
    }

    /// A model's source is checked at the plan: unreadable YAML and a range naming no class are
    /// findings at `/source` (a slot whose name has a space keeps the check's own words), and a
    /// check that cannot reach Model Tools is that error, for a model patched or created.
    #[tokio::test]
    async fn a_model_source_the_check_refuses_is_named_at_the_source() {
        let server = empty_forge(None).await;
        let state = on_forge(&server).await;
        held(
            &state,
            "DataModel",
            "bike-counter",
            "doprava",
            json!({ "contextSpaceRef": "parking", "linkml": "./bike.linkml.yaml" }),
        );
        held(
            &state,
            "ContextSpace",
            "parking",
            "doprava",
            json!({ "isSandbox": true }),
        );
        let patch = |source: &str| PlanInput {
            changes: vec![ChangeInput {
                op: Op::Patch,
                kind: "DataModel".into(),
                name: "bike-counter".into(),
                manifest: None,
                patch: None,
                source: Some(source.into()),
            }],
            workspace: None,
        };
        let yaml = plan(&steward(), &state, "doprava", patch("classes: [")).await;
        assert!(
            matches!(&yaml, Err(OpError::Api(ApiError::Unprocessable { errors, .. }))
                if errors.iter().any(|e| e.starts_with("/changes/0/source: invalid yaml body"))),
            "{yaml:?}"
        );
        let spaced = "classes:\n  Parking:\n    slots: [\"my slot\"]\nslots:\n  my slot:\n    range: Nowhere\n";
        let ranged = plan(&steward(), &state, "doprava", patch(spaced)).await;
        assert!(
            matches!(&ranged, Err(OpError::Api(ApiError::Unprocessable { errors, .. }))
            if errors.iter().any(|e| e.starts_with(
                "/changes/0/source: a slot's range names a class the model does not define: \
                 slots/my slot/range: class 'Nowhere'"
            ))),
            "{ranged:?}"
        );
        let unreached = |answer: &Result<Value, OpError>| matches!(answer, Err(OpError::Api(ApiError::Unavailable(why))) if why.contains("model tools"));
        let tools = plan(
            &steward(),
            &state,
            "doprava",
            patch("classes:\n  Parking: {}\n"),
        )
        .await;
        assert!(unreached(&tools), "{tools:?}");
        let mut created = create("DataModel", "parking");
        created.source = Some("classes:\n  Parking: {}\n".into());
        let new = plan(
            &steward(),
            &state,
            "doprava",
            PlanInput {
                changes: vec![created],
                workspace: None,
            },
        )
        .await;
        assert!(unreached(&new), "{new:?}");
        assert_eq!(state.plans.plans.lock().map(|p| p.len()).unwrap_or(1), 0);
    }

    /// The stored entities of a breaking change: none in a space whose broker holds nothing, not
    /// counted where the broker counts no types, and those of the touched class where it does.
    #[tokio::test]
    async fn a_breaking_change_counts_what_the_broker_holds_by_type() {
        use wiremock::matchers::{method, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let token = std::env::temp_dir().join("jc-plans-broker-token");
        std::fs::write(&token, "t").expect("a token file");
        let published = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(".*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "DefaultBranch": "main", "Content": "Y2xhc3NlczoKICBQYXJraW5nOgogICAgc2xvdHM6IFtzcGFjZXNdCnNsb3RzOgogIHNwYWNlczoKICAgIHJhbmdlOiBpbnRlZ2VyCg==", "Encoding": "base64",
                "Sha": "s1", "Path": "doprava/parking.linkml.yaml"
            })))
            .mount(&published)
            .await;
        let changes = [datamodels::ModelChange {
            severity: "breaking".into(),
            subject: "Parking.spaces".into(),
            reason: "r".into(),
        }];
        let manifest = json!({ "spec": { "contextSpaceRef": "mobility" } });
        let mut rows = Vec::new();
        for counts in [
            json!({ "counts": { "entities": 0 } }),
            json!({ "counts": { "entities": 3 } }),
            json!({ "counts": { "entities": 3 }, "bytes": { "entities": 3, "attrInstances": 0,
                "byType": { "https://example.org/Parking": 2, "https://example.org/Kiosk": 1 } } }),
        ] {
            let broker = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path_regex("^/q/tenants/[^/]+$"))
                .respond_with(ResponseTemplate::new(200).set_body_json(counts))
                .mount(&broker)
                .await;
            let mut state = on_forge(&published).await;
            state.space_usage = std::sync::Arc::new(crate::space_usage::Counter::new(
                Some(broker.uri()),
                Some(token.clone()),
            ));
            held(
                &state,
                "DataModel",
                "parking",
                "doprava",
                json!({ "contextSpaceRef": "mobility", "linkml": "./parking.linkml.yaml" }),
            );
            held(
                &state,
                "ContextSpace",
                "mobility",
                "doprava",
                json!({ "isSandbox": true }),
            );
            let row = affected(&state, "doprava", "parking", &manifest, &changes).await;
            assert_eq!(row.len(), 1, "{row:?}");
            rows.push(row[0].clone());
        }
        assert_eq!(rows[0]["affectedEntities"], 0, "{rows:?}");
        assert_eq!(rows[0]["space"], "mobility", "{rows:?}");
        assert!(rows[1]["affectedEntities"].is_null(), "{rows:?}");
        assert!(
            rows[1]["notCounted"]
                .as_str()
                .is_some_and(|why| why.contains("does not count by type")),
            "{rows:?}"
        );
        assert_eq!(rows[2]["affectedEntities"], 2, "{rows:?}");
    }

    /// A gate whose recorded check cannot be read is an error, never a pass; an item never
    /// checked refuses the whole apply, naming it.
    #[tokio::test]
    async fn an_unreadable_or_absent_check_refuses_the_apply() {
        let fresh = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "ContextSpace",
            "metadata": { "name": "fresh", "namespace": "doprava" },
            "spec": { "isSandbox": true }
        });
        let mut item = removal_of("ContextSpace", "spaces", "fresh", "absent");
        item.manifest = Some(fresh);
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let refused = unchanged_and_gated(&state, std::slice::from_ref(&item)).await;
        assert!(
            matches!(&refused, Err(OpError::Api(ApiError::Conflict(why)))
                if why.starts_with("nothing was applied: ContextSpace fresh: ") && why.contains(PLAN)),
            "{refused:?}"
        );
        let mut broken = AppState::new(crate::config::Config::for_tests(), None);
        broken.drafts = crate::ops::drafts::DraftStore::new(
            Some(crate::test_db::dead_pool()),
            crate::test_db::placed_mirror("doprava"),
        );
        let gate = gate_refusal(&broken, &item).await;
        assert!(
            matches!(&gate, Err(OpError::Api(ApiError::Internal(_)))),
            "{gate:?}"
        );
    }

    /// MF-24: only a `secretRef` the repository's encrypted files do not hold, or a key of it
    /// they lack, is asked for, each with its form.
    #[tokio::test]
    async fn only_a_secret_the_repository_does_not_hold_is_needed() {
        use base64::Engine as _;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        let sealed = concat!(
            "counters-api:\n  apiKey: ENC[AES256_GCM",
            ",data:x]\nsops:\n  version: 3.9.0\n"
        );
        for (at, answer) in [
            (repo.to_owned(), json!({ "DefaultBranch": "main" })),
            (
                format!("{repo}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"),
                json!({ "Sha": "t1", "Truncated": false, "Tree": [
                    { "Path": "projects/doprava/secrets/counters.enc.yaml", "Type": "blob", "Sha": "b1" }
                ] }),
            ),
            (
                format!("{repo}/Shako.File(Path=@p,Ref=@r)"),
                json!({ "Sha": "s1", "Content": base64::engine::general_purpose::STANDARD.encode(sealed) }),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(at))
                .respond_with(ResponseTemplate::new(200).set_body_json(answer))
                .mount(&server)
                .await;
        }
        let forge =
            crate::forge::ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t")
                .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(forge));
        let source = |name: &str, key: Option<&str>| {
            let mut item = removal_of("DataSource", "datasources", "counters", "absent");
            item.manifest = Some(json!({
                "kind": "DataSource", "metadata": { "name": "counters" },
                "spec": { "secretRef": { "name": name, "key": key } }
            }));
            item
        };
        let items = [
            source("counters-api", Some("apiKey")),
            source("counters-api", Some("token")),
            source("gone", None),
            removal_of("DataSource", "datasources", "old", "e1"),
        ];
        let needed = needs_secret(&state, "doprava", &items).await;
        let named: Vec<(&str, Option<&str>)> = needed
            .iter()
            .map(|n| (n["name"].as_str().unwrap_or_default(), n["key"].as_str()))
            .collect();
        assert_eq!(
            named,
            [("counters-api", Some("token")), ("gone", None)],
            "{needed:?}"
        );
        assert!(
            needed[0]["form"]
                .as_str()
                .is_some_and(|form| form.contains("/projects/doprava/secrets/new")),
            "{needed:?}"
        );
    }

    /// MF-48: a bundled change whose reference an open change creates opens its one Change with
    /// the line that holds it until that change is approved.
    #[tokio::test]
    async fn a_bundled_plan_waiting_on_an_open_change_says_so_in_its_merge_request() {
        let open = crate::api::mutate::branch_name(
            "doprava",
            "DataModel",
            "transit",
            crate::change::Operation::Create,
        );
        let server = empty_forge(Some(open)).await;
        let state = on_forge(&server).await;
        let space = json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "ContextSpace",
            "metadata": { "name": "mobility", "namespace": "doprava" },
            "spec": { "isSandbox": true, "dataModelRef": "transit" }
        });
        super::super::proposals::record_check(
            &steward(),
            &state,
            "doprava",
            &space,
            &super::super::verdict::Verdict::green(&space, None),
        )
        .await;
        let mut plan = model_and_space();
        plan.items[1].manifest = Some(space);
        let rows = opened_together(&state, &plan).await;
        assert!(rows.as_ref().is_ok_and(|rows| rows.len() == 1), "{rows:?}");
        let requests = server.received_requests().await.expect("requests");
        let bodies: Vec<String> = requests
            .iter()
            .filter(|r| {
                r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.OpenPullRequest")
            })
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .collect();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert!(bodies[0].contains("#175"), "{bodies:?}");
    }

    /// An autonomous connector's apply of a plan that opens nothing answers an empty list,
    /// also outside any proposal channel.
    #[tokio::test]
    async fn an_autonomous_apply_of_an_empty_plan_opens_nothing() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut connector = scoped_connector(true);
        connector.autonomous = true;
        let id = kept(&state.plans, "sub-jana", Some("c9"), TTL_MINUTES);
        let applied = apply(
            &caller(Some(connector.access())),
            &state,
            "doprava",
            ApplyInput { plan_id: id },
        )
        .await
        .expect("nothing to open");
        assert_eq!(applied["changeRequests"], json!([]), "{applied}");
    }

    /// A dry run and a bundled commit are shown as they are.
    #[test]
    fn an_opened_row_shows_a_dry_run_or_a_bundled_commit_whole() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let dry = crate::api::dry_run::DryRunResult {
            valid: true,
            lane: Lane::Green,
            plan: crate::plan::diff(None, None),
            restarts_stream: false,
            probe: None,
            verdict: None,
            findings: Vec::new(),
            awaited: Vec::new(),
            goes_with: Vec::new(),
            binds_endpoints: Vec::new(),
        };
        let row = opened_row(&state, "doprava", ProposeOutcome::DryRun(dry));
        assert_eq!(row["valid"], true, "{row}");
        let bundled = crate::api::mutate::BundledCommit {
            path: "projects/doprava/a.yaml".into(),
            lane: Lane::Green,
            waits_on: vec!["Waits-On: #1".into()],
        };
        let row = opened_row(&state, "doprava", ProposeOutcome::Bundled(bundled));
        assert_eq!(row["path"], "projects/doprava/a.yaml", "{row}");
        assert_eq!(row["waitsOn"][0], "Waits-On: #1", "{row}");
    }

    /// A manifest put back must be a resource, and an object a revert deletes needs `delete` on
    /// its kind.
    #[test]
    fn a_put_back_needs_a_resource_and_delete_for_a_removal() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let nobody = caller(None);
        let effective = crate::permissions::for_request(&state, &nobody.identity, "doprava");
        let removal = may_put_back(
            &nobody,
            &state,
            "doprava",
            &effective,
            "Endpoint",
            "projects/doprava/a.yaml",
            None,
        );
        assert!(removal.is_err(), "{removal:?}");
        let admin = steward();
        let effective = crate::permissions::for_request(&state, &admin.identity, "doprava");
        let broken = may_put_back(
            &admin,
            &state,
            "doprava",
            &effective,
            "Endpoint",
            "projects/doprava/a.yaml",
            Some(&json!({ "kind": 7 })),
        );
        assert!(
            matches!(&broken, Err(OpError::InvalidInput { path, message })
                if path == "/id" && message.starts_with("projects/doprava/a.yaml: ")),
            "{broken:?}"
        );
        assert!(may_put_back(
            &admin,
            &state,
            "doprava",
            &effective,
            "Endpoint",
            "projects/doprava/a.yaml",
            None
        )
        .is_ok());
    }

    /// The forge of [`forge_with_pull`]'s merged change 7, which touched
    /// `projects/doprava/endpoints/air.yaml`: the file as it was before (`b1`), on main now and as
    /// merged (`m1`), each `None` when absent there.
    async fn reverting(
        before: Option<&Value>,
        now: Option<&Value>,
        made: Option<&Value>,
    ) -> (wiremock::MockServer, AppState) {
        use base64::Engine as _;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, ResponseTemplate};
        let server = forge_with_pull(json!({}), 200).await;
        let repo = "/odata/Repositories(Owner='o',Name='r')";
        Mock::given(method("GET"))
            .and(path(format!("{repo}/PullRequests(7)/Files")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
                { "Filename": "projects/doprava/endpoints/air.yaml", "Status": "added" }
            ] })))
            .mount(&server)
            .await;
        for (at, file) in [("'b1'", before), ("'main'", now), ("'m1'", made)] {
            let answer = match file {
                Some(manifest) => ResponseTemplate::new(200).set_body_json(json!({
                    "Sha": "s1",
                    "Content": base64::engine::general_purpose::STANDARD.encode(manifest.to_string()),
                })),
                None => ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
            };
            Mock::given(method("GET"))
                .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
                .and(query_param("@r", at))
                .respond_with(answer)
                .mount(&server)
                .await;
        }
        let forge =
            crate::forge::ShakoClient::new(server.uri().parse().expect("url"), "o", "r", "t")
                .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_shako(std::sync::Arc::new(forge));
        (server, state)
    }

    fn air() -> Value {
        json!({
            "apiVersion": crate::resource::API_VERSION, "kind": "Endpoint",
            "metadata": { "name": "air", "namespace": "doprava" },
            "spec": {}
        })
    }

    /// S-53: a file the change added and main still holds as merged is planned as its removal.
    #[tokio::test]
    async fn a_revert_of_an_added_file_plans_its_removal() {
        let (_forge, state) = reverting(None, Some(&air()), Some(&air())).await;
        held(&state, "Endpoint", "air", "doprava", json!({}));
        let answer = revert(
            &steward(),
            &state,
            "doprava",
            RevertInput {
                id: "chg-00000007".into(),
                workspace: None,
            },
        )
        .await;
        assert!(
            answer.as_ref().is_ok_and(|a| a["reverts"] == "chg-00000007"
                && a["diff"][0]["op"] == "delete"
                && a["diff"][0]["name"] == "air"),
            "{answer:?}"
        );
    }

    /// F-R2POR4-007: a reader of the project who may not read a kind the change touched is told
    /// the change is not there.
    #[tokio::test]
    async fn a_revert_touching_a_kind_the_caller_does_not_read_is_not_found() {
        let (_forge, state) = reverting(None, Some(&air()), Some(&air())).await;
        let org = crate::permissions::ORG_NAMESPACE;
        held(
            &state,
            "Role",
            "space-reader",
            org,
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
        );
        held(
            &state,
            "RoleBinding",
            "space-readers",
            org,
            json!({ "role": "space-reader", "subjects": [{ "group": "readers" }],
                    "scope": { "project": "doprava" } }),
        );
        let mut reader = caller(None);
        reader.identity.groups = vec!["readers".into()];
        let refused = revert(
            &reader,
            &state,
            "doprava",
            RevertInput {
                id: "chg-00000007".into(),
                workspace: None,
            },
        )
        .await;
        assert!(
            matches!(&refused, Err(OpError::Api(ApiError::NotFound(why)))
                if why == "change 'chg-00000007' not found in project 'doprava'"),
            "{refused:?}"
        );
    }
}
