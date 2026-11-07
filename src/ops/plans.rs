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

use chrono::{DateTime, Duration, Utc};
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
    expires_at: DateTime<Utc>,
}

/// The plans waiting for their apply, in memory.
// ponytail: one replica (Deployment/02); plans live 30 minutes and a restart loses them, which a
// client answers by planning again. A second replica moves this into Postgres beside connectors.
#[derive(Debug, Default)]
pub struct PlanStore {
    plans: Mutex<HashMap<String, Plan>>,
}

impl PlanStore {
    fn keep(&self, id: String, plan: Plan, now: DateTime<Utc>) {
        let mut plans = self.plans.lock().unwrap_or_else(|p| p.into_inner());
        plans.retain(|_, p| p.expires_at > now);
        if plans.len() >= MAX_PLANS {
            if let Some(oldest) = plans
                .iter()
                .min_by_key(|(_, p)| p.expires_at)
                .map(|(id, _)| id.clone())
            {
                plans.remove(&oldest);
            }
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
        now: DateTime<Utc>,
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
        now: DateTime<Utc>,
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
    let ceiling = crate::connectors::Ceiling::of(&state.mirror, &caller.identity);
    if ceiling.honours(connector) || ceiling.allows_change(project, kind, name, verb) {
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
            let check = super::find("jc_manifest_dry_run").ok_or_else(|| {
                OpError::Api(ApiError::Internal(
                    "jc_manifest_dry_run is not registered".into(),
                ))
            })?;
            (check.run)(caller, state, &home, json!({ "manifest": m })).await?
        }
        None => removal_dry_run(caller, state, &home, info.plural, &change.name, workspace).await?,
    };
    // The slug the check minted, from the draft it recorded, so the change proposes what was
    // checked.
    slug_settled(caller, state, at, &home, manifest.as_mut()).await?;
    let mut answer = answer;
    if let (Some(m), "Pipeline") = (&manifest, info.kind) {
        let warnings = super::replace_check::warnings(state, project, &home, m).await;
        if let Some(findings) = answer
            .pointer_mut("/verdict/findings")
            .and_then(Value::as_array_mut)
        {
            findings.extend(warnings);
        }
    }
    super::endpoint_reach::warn_outside(
        &mut answer,
        state,
        &caller.identity,
        &home,
        manifest.as_ref(),
    );
    let model = match change.source {
        None => None,
        Some(_) => Some(model_checked(caller, state, at, &home, change, manifest.as_ref()).await?),
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
            if let Some(all) = diff["verdict"]["findings"].as_array_mut() {
                all.extend(findings);
            } else {
                diff["verdict"]["findings"] = Value::Array(findings);
            }
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
    at: &str,
    home: &str,
    change: &ChangeInput,
    manifest: Option<&Value>,
) -> Result<(Value, Result<ModelPlan, Vec<Value>>), OpError> {
    let source = change.source.as_deref().unwrap_or_default();
    let Some(manifest) = manifest else {
        return Err(invalid(
            format!("{at}/source"),
            "op delete takes no source".into(),
        ));
    };
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
        let tenant = crate::spaces::segment(&state.mirror, home, space);
        match state.space_usage.usage(&broker, &tenant).await {
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
    let now = Utc::now();
    let expires_at = now + Duration::minutes(TTL_MINUTES);
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
    let gitea = state
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
                    if let Err(gone) = gitea.delete_branch(&branch).await {
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
    let pull = gitea
        .create_pull_request(
            &branch,
            &gitea.default_branch().await.map_err(ApiError::from)?,
            &title,
            &body,
        )
        .await
        .map_err(ApiError::from)?;
    let change = crate::change::Change::new(
        crate::api::changes::change_meta(state, &gitea, pull.number, project),
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
    let now = Utc::now();
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
            approve_autonomously(state, caller, project, &connector.name, &mut opened).await
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
    connector: &str,
    opened: &mut [Value],
) -> Vec<Value> {
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
    forge: &crate::git::GiteaClient,
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
    forge: &crate::git::GiteaClient,
    number: u64,
    project: &str,
    id: &str,
) -> Result<(crate::git::PullRequest, String), OpError> {
    let missing = || {
        OpError::Api(ApiError::NotFound(format!(
            "change '{id}' not found in project '{project}'"
        )))
    };
    let pr = match forge.pull_request(number).await {
        Ok(pr) => pr,
        Err(crate::git::GitError::NotFound | crate::git::GitError::Api { status: 404, .. }) => {
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
            (None, true) => (Op::Delete, None),
            (None, false) => continue,
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
        let now = Utc::now();
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
        let model = ModelPlan {
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
        };
        store.keep(
            "p1".into(),
            Plan {
                owner: "ana".into(),
                connector: None,
                project: "helsinki".into(),
                workspace: None,
                items: vec![item("air", Some(model)), item("bus", None)],
                expires_at: now + chrono::Duration::minutes(5),
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

    /// S-10: a breaking change counts the stored entities of the class it touches, also for a
    /// top-level slot a class lists.
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
        match plan(&caller(None), &state, "doprava", input).await {
            Err(OpError::InvalidInput { path, message }) => {
                assert_eq!(path, "/changes/0/manifest/spec/slug");
                assert!(message.contains("leave spec.slug out"), "{message}");
                assert!(!message.contains(typed), "{message}");
            }
            other => panic!("expected a refusal at the slug, got {other:?}"),
        }
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
                expires_at: Utc::now() + Duration::minutes(minutes),
            },
            Utc::now(),
        );
        id
    }

    /// A plan answers only its own caller, connector and project, before it expires, and once.
    #[test]
    fn a_plan_is_taken_once_by_its_owner_through_its_connector() {
        let store = PlanStore::default();
        let now = Utc::now();
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
            expires_at: Utc::now() + Duration::days(1),
            allowed_clients: vec![],
            client_is_boundary: false,
            state: "active".into(),
            created_at: Utc::now(),
            granted_at: Utc::now(),
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
            .take(&id, "sub-jana", "doprava", Some("c1"), Utc::now())
            .is_some());
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
        let denied = |answer: Result<Value, OpError>| match answer {
            Err(OpError::Api(ApiError::Denied(why))) => why,
            other => panic!("expected a refusal, got {other:?}"),
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
        if let Err(OpError::Api(ApiError::Denied(why))) = &fresh {
            assert!(
                !why.contains("may not"),
                "a create was refused as a verb: {why}"
            );
        }
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
    async fn empty_forge(open: Option<String>) -> wiremock::MockServer {
        use wiremock::matchers::{method, path, path_regex};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let repo = "/api/v1/repos/o/r";
        Mock::given(method("GET"))
            .and(path(repo))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        let pull = |number: u64, head: &str| {
            json!({
                "number": number, "html_url": format!("https://forge.test/pulls/{number}"),
                "state": "open", "title": "t", "body": "",
                "head": { "ref": head }, "base": { "ref": "main" },
                "created_at": "2026-11-01T20:00:00Z",
                "user": { "login": "jana", "full_name": "Jana" },
                "mergeable": true, "merged": false
            })
        };
        let pulls: Vec<Value> = open.iter().map(|head| pull(175, head)).collect();
        Mock::given(method("GET"))
            .and(path(format!("{repo}/pulls")))
            .respond_with(ResponseTemplate::new(200).set_body_json(pulls))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/pulls")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(pull(176, "portal/import-plan-p1")),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/branches")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{repo}/contents")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c0ffee" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path_regex(format!("^{repo}/contents/.*")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c0ffee" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(format!("^{repo}/contents/.*")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
            )
            .mount(&server)
            .await;
        server
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
            expires_at: Utc::now() + Duration::minutes(TTL_MINUTES),
        }
    }

    async fn on_forge(server: &wiremock::MockServer) -> AppState {
        let forge = crate::git::GiteaClient::new(server.uri().parse().expect("url"), "o", "r", "t")
            .expect("client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_gitea(std::sync::Arc::new(forge));
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
        let rows = rows.unwrap_or_else(|err| {
            let seen: Vec<String> = requests
                .iter()
                .map(|r| format!("{} {}", r.method, r.url))
                .collect();
            panic!("the plan opens: {err}; forge saw {seen:#?}")
        });
        assert_eq!(rows.len(), 1, "one Change: {rows:?}");

        let commits: Vec<Value> = requests
            .iter()
            .filter(|r| {
                matches!(r.method.as_str(), "POST" | "PUT") && r.url.path().contains("/contents")
            })
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(commits.len(), 2, "one commit per change");
        let model = commits
            .iter()
            .find(|c| c["files"].as_array().is_some_and(|f| f.len() > 2))
            .expect("the model's commit");
        assert_eq!(model["branch"], "portal/import-plan-p1");
        let paths: Vec<&str> = model["files"]
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
                .all(|c| c["branch"] == "portal/import-plan-p1"),
            "every write lands on the plan's branch"
        );
        let opened: Vec<Value> = requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
            .map(|r| serde_json::from_slice(&r.body).expect("json"))
            .collect();
        assert_eq!(opened.len(), 1, "one merge request");
        assert_eq!(opened[0]["head"], "portal/import-plan-p1");
    }

    /// CC-34 (JC2-10): a change already open on the model refuses the plan before a byte of it is
    /// written, and names the open change.
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
        assert!(
            requests.iter().all(|r| r.method.as_str() == "GET"),
            "nothing written: {:?}",
            requests.iter().map(|r| r.url.path()).collect::<Vec<_>>()
        );
    }
}
