//! The check-then-propose operations and the verdict gate they share (PF-57, AG-62, ADR-N-021): a
//! manifest is dry-run, its verdict recorded, and a proposal goes out only on a fresh green one.

use super::bounds;
use super::changes::change_schema;
use super::drafts::DraftRef;
use super::parse_input;
use super::verdict_schema;
use super::Caller;
use super::OpError;
use super::Operation;
use super::OperationAnnotations;
use super::Via;
use super::{verdict, Draft, DraftError, Finding, Level, Verdict};
use crate::agents::share;
use crate::api::assistant;
use crate::api::dry_run;
use crate::api::mutate;
use crate::auth::session::Identity;
use crate::change::{Lane, Operation as ChangeOp};
use crate::error::ApiError;
use crate::state::AppState;
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointProposeInput {
    #[serde(default)]
    pub context_space: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub allowed_projects: Vec<String>,
    #[serde(default)]
    pub representations: Vec<String>,
    #[serde(default)]
    pub hidden_attributes: Vec<String>,
    #[serde(default)]
    pub entity_types: Vec<String>,
    #[serde(default)]
    pub rate_limits: Option<share::RateLimits>,
    #[serde(default)]
    pub manifest: Option<Value>,
    #[serde(default)]
    pub draft: Option<DraftRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInput {
    #[serde(default)]
    pub manifest: Option<Value>,
    #[serde(default)]
    pub draft: Option<DraftRef>,
}

fn endpoint_propose_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "contextSpace": text("The ContextSpace served", NAME),
            "name": text("The endpoint's name (DNS-1123)", NAME),
            "title": text("The endpoint's title", TITLE),
            "audience": {
                "type": "string",
                "description": "Who may read it",
                "enum": ["project-list", "organization", "public"]
            },
            "allowedProjects": texts("With project-list: the projects that may read", NAME),
            "representations": {
                "type": "array",
                "description": "Default ngsi-ld, csv",
                "items": { "type": "string", "enum": crate::agents::share::known_representations() }
            },
            "hiddenAttributes": texts("Attributes the endpoint never returns", TERM),
            "entityTypes": texts("The entity types it serves", TERM),
            "rateLimits": {
                "type": "object",
                "description": "Only when the person asks for a limit; none by default",
                "properties": {
                    "requestsPerMinute": {
                        "type": "integer",
                        "description": "Requests a minute per caller",
                        "minimum": 1,
                        "maximum": crate::agents::share::MAX_REQUESTS_PER_MINUTE
                    },
                    "burst": {
                        "type": "integer",
                        "description": "Requests at once above the steady rate",
                        "minimum": 0,
                        "maximum": crate::agents::share::MAX_REQUESTS_PER_MINUTE
                    }
                },
                "required": ["requestsPerMinute"],
                "additionalProperties": false
            },
            "manifest": manifest("A whole Endpoint manifest instead of the fields"),
            "draft": draft_ref()
        },
        "additionalProperties": false
    })
}

pub(crate) fn manifest_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "manifest": manifest("The whole manifest, or draft"),
            "draft": draft_ref()
        },
        "additionalProperties": false
    })
}

/// What a check answers: whether the manifest is valid, the lane it would take, the fields it
/// would change, what one fetch of a source returned, and the verdict filed on the draft.
pub(crate) fn dry_run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "valid": { "type": "boolean" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "plan": {
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "object",
                        "properties": {
                            "create": { "type": "integer" },
                            "update": { "type": "integer" },
                            "delete": { "type": "integer" }
                        }
                    },
                    "fields": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "from": {},
                                "to": {}
                            }
                        }
                    }
                }
            },
            "probe": {
                "type": "object",
                "description": "One fetch of an http DataSource (MF-39); absent for every other kind",
                "properties": {
                    "records": { "type": "integer" },
                    "bytes": { "type": "integer" },
                    "sample": {},
                    "skipped": { "type": "string" }
                }
            },
            "verdict": verdict_schema()
        },
        "required": ["valid", "lane", "plan"]
    })
}

/// What `jc_endpoint_propose` answers: a Change when it was given a manifest or a draft, and
/// the rendered Endpoint with its draft policies when it was given the parameters to share
/// data (T-0838). One tool, two answers, both published.
fn proposal_output_schema() -> Value {
    json!({
        "oneOf": [
            change_schema(),
            {
                "type": "object",
                "description": "The rendered proposal a form fills itself from; nothing is written yet",
                "properties": {
                    "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                    "slug": { "type": "string" },
                    "endpoint": { "type": "object" },
                    "policies": { "type": "array", "items": { "type": "object" } },
                    "prefill": { "type": "object" }
                },
                "required": ["endpoint"]
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Registry Initialisation
// ---------------------------------------------------------------------------

async fn mutate_manifest(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    manifest: Value,
    dry_run: bool,
    gated: bool,
) -> Result<Value, OpError> {
    let kind = manifest
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let op = match name
        .as_deref()
        .and_then(|n| state.mirror.get(project, kind, n))
    {
        Some(_) => ChangeOp::Update,
        None => ChangeOp::Create,
    };

    let kind = kind.to_owned();
    let outcome = if gated && !dry_run {
        mutate::propose_gated(
            &caller.identity,
            state,
            project,
            plural,
            name.as_deref(),
            op,
            manifest,
        )
        .await?
    } else {
        mutate::propose_with_identity(
            &caller.identity,
            state,
            project,
            plural,
            name.as_deref(),
            op,
            dry_run,
            manifest,
        )
        .await?
    };
    // A person proposing in the Portal who administers every kind of the change has it approved
    // as it is proposed (PF-58); an agent run, a bearer caller and MCP never do (AG-11, AG-82).
    let outcome = match outcome {
        mutate::ProposeOutcome::Change(change) if caller.via == Via::Session => {
            mutate::ProposeOutcome::Change(
                crate::api::changes::approve_as_proposed(
                    state,
                    &caller.identity,
                    project,
                    &kind,
                    change,
                    None,
                )
                .await,
            )
        }
        other => other,
    };

    Ok(outcome.into_value())
}

async fn resolve_manifest_input(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: &ManifestInput,
) -> Result<(Value, Option<DraftRef>), OpError> {
    let (mut manifest, draft) = given_manifest(caller, state, project, input).await?;
    settle_slug(caller, state, project, &mut manifest).await?;
    Ok((manifest, draft))
}

/// The Endpoint slug of a manifest a caller sent, filled in or refused (EP-75).
pub(crate) async fn settle_slug(
    caller: &Caller,
    state: &AppState,
    project: &str,
    manifest: &mut Value,
) -> Result<(), OpError> {
    crate::endpoint_slug::settle_in(state, project, None, caller.brings_its_own_slug(), manifest)
        .await
        .map_err(|message| OpError::InvalidInput {
            path: format!("/manifest{}", crate::endpoint_slug::PATH),
            message,
        })
}

async fn given_manifest(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: &ManifestInput,
) -> Result<(Value, Option<DraftRef>), OpError> {
    if let Some(d) = &input.draft {
        if let Some(m) = &input.manifest {
            same_draft(m, d)?;
            return Ok((m.clone(), Some(d.clone())));
        }
        let draft = super::drafts::readable_draft(state, &caller.identity, project, d).await?;
        return Ok((draft.manifest, Some(d.clone())));
    }
    if let Some(m) = &input.manifest {
        // Checked under its own kind and name, so its proposal finds the verdict (T-0956).
        return Ok((m.clone(), own_draft(m)));
    }
    Err(OpError::InvalidInput {
        path: "/manifest".into(),
        message: "either manifest or draft is required".into(),
    })
}

/// Records a check's verdict on the draft it names, for a caller who may propose its kind (AG-61,
/// AG-62, T-4840); anybody else's check is answered and kept nowhere. A form saves its draft after
/// a debounce, so a check can name a draft that does not exist yet: the check then creates it
/// from the manifest it judged, for a caller who may propose the kind, and a Check followed at
/// once by Propose finds the draft and its verdict instead of a 404.
pub(crate) async fn record_verdict(
    caller: &Caller,
    state: &AppState,
    project: &str,
    draft: &DraftRef,
    manifest: &Value,
    verdict: &Verdict,
) {
    // An App's default group is checked by whoever may propose the App (AP-119).
    let update = state
        .mirror
        .get(project, &draft.kind, &draft.name)
        .is_some();
    let permissions = crate::groups::for_proposal(
        state,
        &caller.identity,
        project,
        &draft.kind,
        update,
        manifest,
    );
    // The build lane's check of its `status.build` is kept like anybody's, for the proposal
    // that follows it: its rule authorizes that write alone (AP-73, T-2636).
    let may_propose = permissions.check(&draft.kind, Verb::Propose, None).is_ok()
        || (manifest.pointer("/status/build").is_some()
            && permissions.may_write_status_field(&draft.kind, "status.build"));
    // Only a verdict of a person who may propose counts, on an existing draft as on a new
    // one: a reader's check never replaces the proposer's (T-4840).
    if !may_propose {
        return;
    }
    let store = state.drafts.clone();
    if !matches!(
        store
            .set_verdict(project, &draft.kind, &draft.name, verdict.clone())
            .await,
        Err(DraftError::NotFound { .. })
    ) {
        return;
    }
    let created = store
        .put(
            project,
            &draft.kind,
            &draft.name,
            manifest.clone(),
            Some(0),
            &caller.identity.username,
            caller.via.touched_kind(),
        )
        .await;
    if created.is_ok() {
        let _ = store
            .set_verdict(project, &draft.kind, &draft.name, verdict.clone())
            .await;
    }
}

/// Keeps the record check of a harness run (T-4840) on the verdict of `draft`, for a caller who
/// may propose it. The verdict's own ok and findings stay those of the check that wrote them, so
/// a run of the mapping never turns a red check green; with no verdict yet the record check waits
/// on a red one, until the manifest's own check replaces it and keeps the records.
/// ponytail: read then write, not one transaction; a dry run landing in between loses to the
/// verdict read before it, and the gate then asks for the check again.
pub(crate) async fn record_records(
    caller: &Caller,
    state: &AppState,
    project: &str,
    draft: &DraftRef,
    manifest: &Value,
    records: verdict::RecordsVerdict,
) {
    let earlier = state
        .drafts
        .get(project, &draft.kind, &draft.name)
        .await
        .ok()
        .flatten()
        .and_then(|d| d.verdict);
    let mut verdict = earlier.unwrap_or_else(|| {
        Verdict::red(
            manifest,
            vec![Finding {
                level: Level::Error,
                path: String::new(),
                message: "The Pipeline itself has not been checked yet: run its check.".into(),
            }],
            None,
        )
    });
    verdict.records = Some(Box::new(records));
    record_verdict(caller, state, project, draft, manifest, &verdict).await;
}

/// The check that judges a manifest of `kind`, as the gate names it when it refuses a draft
/// nobody has checked (PF-57). One table, read by the operation itself and by the MCP door
/// before it offers an elicitation, so both doors name the same check (ADR-N-021).
pub fn check_for_kind(kind: &str) -> &'static str {
    match kind {
        "DataSource" => "jc_datasource_check",
        "Pipeline" => "jc_pipeline_test",
        _ => "jc_manifest_dry_run",
    }
}

/// The draft a manifest proposed without one is checked under: its own kind and name.
/// Refuses an inline manifest sent with a draft of another kind or name: its check would land on
/// that draft and replace the verdict of what the draft holds (F-POR3-020).
pub(crate) fn same_draft(manifest: &Value, draft: &DraftRef) -> Result<(), OpError> {
    let differs = |written: Option<&str>, named: &str| written.is_some_and(|w| w != named);
    if differs(manifest.get("kind").and_then(Value::as_str), &draft.kind)
        || differs(
            manifest.pointer("/metadata/name").and_then(Value::as_str),
            &draft.name,
        )
    {
        return Err(OpError::InvalidInput {
            path: "/draft".into(),
            message: format!(
                "the manifest is not the draft '{}/{}'; send the manifest alone, or the draft alone",
                draft.kind, draft.name
            ),
        });
    }
    Ok(())
}

pub(crate) fn own_draft(manifest: &Value) -> Option<DraftRef> {
    Some(DraftRef {
        kind: manifest.get("kind")?.as_str()?.to_owned(),
        name: manifest.pointer("/metadata/name")?.as_str()?.to_owned(),
    })
}

/// PF-57 for a manifest proposed without a draft, whatever door it comes through (owner decision
/// T-0956): the verdict its own check recorded under its kind and name, green and fresh for this
/// exact manifest. `Ok(true)` is a lax installation letting an unchecked one through.
pub async fn verdict_for_manifest(
    state: &AppState,
    project: &str,
    manifest: &Value,
) -> Result<bool, OpError> {
    let Some(own) = own_draft(manifest) else {
        // No kind or no name: the proposal refuses the shape itself, naming the field.
        return Ok(false);
    };
    let recorded = state
        .drafts
        .get(project, &own.kind, &own.name)
        .await
        .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
        .and_then(|draft| draft.verdict);
    verdict_gate(
        state,
        recorded.as_ref(),
        manifest,
        check_for_kind(&own.kind),
        "manifest",
    )
}

/// After a manifest proposed without a draft became a Change, the draft its check created goes,
/// but only while it still holds that manifest: a person's draft of the same resource with other
/// content stays theirs.
pub async fn forget_check(state: &AppState, project: &str, manifest: &Value) {
    let Some(own) = own_draft(manifest) else {
        return;
    };
    let store = state.drafts.clone();
    if let Ok(Some(draft)) = store.get(project, &own.kind, &own.name).await {
        if verdict::digest_of(&draft.manifest) == verdict::digest_of(manifest) {
            let _ = store.drop(project, &own.kind, &own.name).await;
        }
    }
}

/// Records the verdict of a check of a manifest that names no draft, under its own kind and
/// name, so the proposal of the same manifest finds it (T-0956).
pub async fn record_check(
    caller: &Caller,
    state: &AppState,
    project: &str,
    manifest: &Value,
    verdict: &Verdict,
) {
    if let Some(own) = own_draft(manifest) {
        record_verdict(caller, state, project, &own, manifest, verdict).await;
        if let Some(records) = crate::api::kpi_check::records_verdict(state, project, manifest) {
            record_records(caller, state, project, &own, manifest, records).await;
        }
    }
}

/// The same for a check the platform refused, except that it writes no draft of its own.
///
/// A red verdict reaches the draft the person is working in, and nothing more: the manifest a check
/// rejected is not stored anywhere by this path. A refusal can be about a credential typed into the
/// manifest (T-2238, T-2239), and keeping that manifest because its check failed would put the
/// value the refusal exists to stop into the draft store (T-2234).
pub async fn record_refused_check(
    state: &AppState,
    project: &str,
    manifest: &Value,
    verdict: &Verdict,
) {
    let Some(own) = own_draft(manifest) else {
        return;
    };
    let _ = state
        .drafts
        .set_verdict(project, &own.kind, &own.name, verdict.clone())
        .await;
}

/// A check that rejected the manifest, as the red verdict that says why (T-2234).
///
/// One shape for one outcome, wherever a check is run: the operation the form calls, the one the
/// assistant calls and the plain REST door all answer this. Anything that is not a judgement about
/// the manifest — the caller, a project that is not there, a broken forge — stays the error it was.
async fn refused_check_output(
    state: &AppState,
    project: &str,
    manifest: &Value,
    error: OpError,
) -> Result<Value, OpError> {
    let OpError::Api(api) = &error else {
        return Err(error);
    };
    let Some(refused) = crate::api::dry_run::refused_check(api, manifest) else {
        return Err(error);
    };
    let verdict = refused
        .verdict
        .clone()
        .expect("refused_check sets a verdict");
    record_refused_check(state, project, manifest, &verdict).await;
    let mut out = serde_json::to_value(&refused)?;
    out["verdict"] = serde_json::to_value(&verdict)?;
    Ok(out)
}

/// A bundle has no kind and name of its own, so its check is held under this kind and the
/// caller's name: one checked import per person and project (T-1460).
const IMPORT_CHECK_KIND: &str = "ImportBundle";

/// Records the dry run of a bundle as its check (PF-57 on the import door, T-1460). A plan the
/// import could draw is green; one it could not draw was refused before this with its reason.
pub async fn record_import_check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    subject: &Value,
) {
    let store = state.drafts.clone();
    let who = &identity.username;
    if store
        .put(
            project,
            IMPORT_CHECK_KIND,
            who,
            subject.clone(),
            None,
            who,
            "import",
        )
        .await
        .is_ok()
    {
        let _ = store
            .set_verdict(
                project,
                IMPORT_CHECK_KIND,
                who,
                Verdict::green(subject, None),
            )
            .await;
    }
}

/// PF-57 for an import: the caller's recorded check of this bundle, green and fresh for the
/// files about to be written. `Ok(true)` is a lax installation letting an unchecked one through.
pub async fn verdict_for_import(
    state: &AppState,
    identity: &Identity,
    project: &str,
    subject: &Value,
) -> Result<bool, OpError> {
    let recorded = state
        .drafts
        .get(project, IMPORT_CHECK_KIND, &identity.username)
        .await
        .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
        .and_then(|draft| draft.verdict);
    verdict_gate(
        state,
        recorded.as_ref(),
        subject,
        "jc_project_import",
        "bundle",
    )
}

/// After an import became a Change, its check goes, so the next import is checked again.
pub async fn forget_import_check(state: &AppState, identity: &Identity, project: &str) {
    let _ = state
        .drafts
        .drop(project, IMPORT_CHECK_KIND, &identity.username)
        .await;
}

/// The refusal an operation's verdict gate already holds for these arguments, or `None` when
/// it lets them through (PF-57, AG-62).
///
/// Asked by a door that answers something else before the operation runs — the MCP door
/// offers an elicitation — so the refusal it carries is the route's own, word for word, and
/// a client reads `verdict_required` wherever it knocks (ADR-N-021, T-0947).
pub async fn verdict_refusal(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    op: &Operation,
    project: &str,
    input: &Value,
) -> Option<Value> {
    if op.verb != Some(Verb::Propose) && op.name != "jc_resource_propose" {
        return None;
    }
    let draft_ref: DraftRef = serde_json::from_value(input.get("draft")?.clone()).ok()?;
    // A draft in a space the caller may not read is not there: its verdict says nothing (JC2-38).
    let draft = super::drafts::readable_draft(state, identity, project, &draft_ref)
        .await
        .ok()?;
    match apply_verdict_gate(state, &draft, check_for_kind(&draft_ref.kind)) {
        Err(OpError::Conflict(body)) => Some(body),
        _ => None,
    }
}

fn apply_verdict_gate(state: &AppState, draft: &Draft, check_op: &str) -> Result<bool, OpError> {
    verdict_gate(
        state,
        draft.verdict.as_ref(),
        &draft.manifest,
        check_op,
        "draft",
    )
}

/// The gate itself, for a verdict and the manifest it must be fresh for: `Ok(true)` lets an
/// unchecked manifest through with a warning (lax), `Ok(false)` finds nothing to say.
fn verdict_gate(
    state: &AppState,
    verdict: Option<&Verdict>,
    manifest: &Value,
    check_op: &str,
    subject: &str,
) -> Result<bool, OpError> {
    let mode = verdict::get_validation_mode(state);
    let reason = match verdict {
        None => Some("verdict_absent"),
        Some(v) if !v.ok => Some("verdict_failed"),
        Some(v) if !v.is_fresh_for(manifest) => Some("stale"),
        Some(v) => verdict::records_refusal(v, manifest),
    };
    // A Pipeline's records are checked by its test, whatever check the door names (T-4840).
    let check_op = match reason {
        Some(r) if r.starts_with("records_") => "jc_pipeline_test",
        _ => check_op,
    };

    if let Some(reason) = reason {
        if mode == verdict::Validation::Strict {
            let detail = match reason {
                "verdict_absent" => {
                    format!("The {subject} has not been checked; check it, then propose it.")
                }
                "verdict_failed" => {
                    format!(
                        "The {subject}'s check found problems; resolve them and check it again."
                    )
                }
                "records_unchecked" => format!(
                    "The {subject}'s records have not been checked against the space's model, the \
                     target Endpoint and the lifetime; test it on a sample, then propose it."
                ),
                "records_failed" => format!(
                    "Records of the {subject}'s sample would not be written; resolve what the test \
                     names and test it again."
                ),
                "records_stale" => format!(
                    "The {subject} changed since its records were checked; test it again, then \
                     propose it."
                ),
                _ => format!(
                    "The {subject} changed since its check; check it again, then propose it."
                ),
            };
            return Err(OpError::Conflict(json!({
                "error": "verdict_required",
                "check": check_op,
                "reason": reason,
                "detail": detail,
            })));
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) async fn propose_with_optional_draft(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    check_op: &'static str,
    input: ManifestInput,
) -> Result<Value, OpError> {
    if let Some(d) = &input.draft {
        let draft = super::drafts::readable_draft(state, &caller.identity, project, d).await?;
        let warning = apply_verdict_gate(state, &draft, check_op)?;
        let mut out = mutate_manifest(
            caller,
            state,
            project,
            plural,
            draft.manifest.clone(),
            false,
            false,
        )
        .await?;
        let _ = state.drafts.drop(project, &d.kind, &d.name).await;
        if warning {
            out["warning"] = json!("proposed without a fresh green verdict");
        }
        return Ok(out);
    }
    let mut manifest = input.manifest.ok_or_else(|| OpError::InvalidInput {
        path: "/manifest".into(),
        message: "either manifest or draft is required".into(),
    })?;
    settle_slug(caller, state, project, &mut manifest).await?;
    propose_bare(caller, state, project, plural, manifest).await
}

/// A manifest proposed without a draft: proposed with its own check's verdict required once it has
/// passed its own checks (PF-57, T-0956), then the draft that check created is forgotten.
async fn propose_bare(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    manifest: Value,
) -> Result<Value, OpError> {
    let out = mutate_manifest(
        caller,
        state,
        project,
        plural,
        manifest.clone(),
        false,
        true,
    )
    .await?;
    forget_check(state, project, &manifest).await;
    Ok(out)
}

/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_endpoint_propose",
            title: "Propose Endpoint",
            description:
                "Renders an Endpoint and its draft Policy manifests from a request to share data",
            input: endpoint_propose_input_schema,
            output: proposal_output_schema,
            // Parameters render a proposal and nothing is written, but a `manifest` or a
            // `draft` opens a merge request, so the annotation says what the operation can do
            // and not what its lightest path does: a client that reads `readOnlyHint` decides
            // from it whether to ask a person first (AG-07, AG-63).
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "Endpoint",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<EndpointProposeInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: EndpointProposeInput = parse_input(val)?;
                    if input.draft.is_some() || input.manifest.is_some() {
                        let manifest = ManifestInput {
                            manifest: input.manifest,
                            draft: input.draft,
                        };
                        return propose_with_optional_draft(
                            caller,
                            state,
                            project,
                            "endpoints",
                            "jc_manifest_dry_run",
                            manifest,
                        )
                        .await;
                    }
                    let params = share::ProposeEndpoint {
                        context_space: input.context_space.unwrap_or_default(),
                        name: input.name.unwrap_or_default(),
                        title: input.title,
                        audience: input.audience,
                        allowed_projects: input.allowed_projects,
                        representations: input.representations,
                        hidden_attributes: input.hidden_attributes,
                        entity_types: input.entity_types,
                        rate_limits: input.rate_limits,
                        access: None,
                        write_attributes: Vec::new(),
                        write_relationships: Vec::new(),
                        creates_per_day: None,
                        embed_origins: Vec::new(),
                    };
                    let proposal = assistant::execute_propose_endpoint(
                        &caller.identity,
                        state,
                        project,
                        params,
                    )
                    .await?;
                    Ok(serde_json::to_value(proposal)?)
                })
            },
        },
        Operation {
            name: "jc_manifest_dry_run",
            title: "Manifest Dry Run",
            description:
                "Dry-runs candidate manifest changes and returns validation result and plan diff",
            input: manifest_input_schema,
            output: dry_run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                let input: ManifestInput = parse_input(val.clone())?;
                if input.manifest.is_none() && input.draft.is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/manifest".into(),
                        message: "either manifest or draft is required".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    let (manifest, draft_ref) =
                        resolve_manifest_input(caller, state, project, &input).await?;
                    let answer = dry_run::execute_dry_run(
                        &caller.identity,
                        state,
                        project,
                        manifest.clone(),
                    )
                    .await;
                    // A check that rejects the manifest answers the red verdict that says why, and
                    // not an error the caller has to read a second way (T-2234): this is the check
                    // every form and the assistant run. The red verdict reaches a draft that is
                    // already there and writes none of its own, because what was refused can be a
                    // credential typed into the manifest (T-2238).
                    let res = match answer {
                        Ok(res) => res,
                        Err(err) => {
                            return refused_check_output(state, project, &manifest, err.into())
                                .await
                        }
                    };
                    let mut findings = Vec::new();
                    if !res.valid {
                        findings.push(Finding {
                            level: Level::Error,
                            path: "".into(),
                            message: "plan validation failed".into(),
                        });
                    }
                    // A Policy attribute its space's model lacks narrows the grant to nothing
                    // (T-6283): an error on its field, not a green plan.
                    let unknown = super::policy_names::findings(state, project, &manifest).await;
                    let ok = res.valid && unknown.is_empty();
                    findings.extend(unknown);
                    findings.extend(super::policy_names::superseded_audit(&manifest));
                    // A reference an open change creates passes and names the change (MF-48).
                    findings.extend(res.awaited.iter().map(crate::references::Awaited::finding));
                    let mut verdict = Verdict::new(
                        ok,
                        findings,
                        Some(serde_json::to_value(&res.plan).unwrap_or_default()),
                        &manifest,
                    );
                    verdict.waits_on = res.awaited.iter().map(|a| a.change.clone()).collect();
                    if let Some(d) = &draft_ref {
                        record_verdict(caller, state, project, d, &manifest, &verdict).await;
                    }
                    let mut out = serde_json::to_value(&res)?;
                    out["verdict"] = serde_json::to_value(&verdict)?;
                    Ok(out)
                })
            },
        },
        Operation {
            name: "jc_datasource_check",
            title: "Check DataSource",
            description: "Dry-runs a DataSource manifest and probes the external feed",
            input: manifest_input_schema,
            output: dry_run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataSource",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    let (manifest, draft_ref) =
                        resolve_manifest_input(caller, state, project, &input).await?;
                    let mut out = match mutate_manifest(
                        caller,
                        state,
                        project,
                        "datasources",
                        manifest.clone(),
                        true,
                        false,
                    )
                    .await
                    {
                        Ok(out) => out,
                        // The same refusal a `DataSource` used to answer as a 4xx: a probe that
                        // failed was always a red verdict, a manifest the door rejected never was
                        // (T-2234).
                        Err(error) => {
                            return refused_check_output(state, project, &manifest, error).await
                        }
                    };
                    let verdict = datasource_verdict(&out, &manifest);
                    if let Some(d) = &draft_ref {
                        record_verdict(caller, state, project, d, &manifest, &verdict).await;
                    }
                    out["verdict"] = serde_json::to_value(&verdict)?;
                    Ok(out)
                })
            },
        },
    ]
}

/// The verdict of a DataSource check: the dry run's validity and, for an `http` source, the
/// probe of its feed (MF-39).
pub(crate) fn datasource_verdict(out: &Value, manifest: &Value) -> Verdict {
    let ok = out.get("valid").and_then(Value::as_bool).unwrap_or(false);
    let mut findings = Vec::new();
    if !ok {
        findings.push(Finding {
            level: Level::Error,
            path: String::new(),
            message: "the manifest did not pass the dry run".into(),
        });
    }
    if let Some(skipped) = out.pointer("/probe/skipped").and_then(Value::as_str) {
        findings.push(Finding {
            level: Level::Info,
            path: "spec.http.url".into(),
            message: skipped.to_owned(),
        });
    }
    Verdict::new(ok, findings, Some(out.clone()), manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// JC2-38: a draft the caller may not read answers no verdict refusal, so its verdict state
    /// tells a stranger nothing; the approver who reads it still gets `verdict_required`.
    #[tokio::test]
    async fn an_unreadable_draft_answers_no_verdict_refusal() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let manifest = json!({ "kind": "Policy", "metadata": { "name": "hidden", "namespace": "p" },
            "spec": { "contextSpaceRef": { "name": "closed" } } });
        state
            .drafts
            .put("p", "Policy", "hidden", manifest, None, "sub-a", "person")
            .await
            .expect("kept");
        let person = |groups: Vec<String>| crate::auth::session::Identity {
            client: None,
            subject: "f:1:x".into(),
            username: "x".into(),
            email: None,
            name: None,
            roles: vec![],
            groups,
            second_factor: true,
        };
        let op = crate::ops::find("jc_resource_propose").expect("registered");
        let input = json!({ "draft": { "kind": "Policy", "name": "hidden" } });
        let stranger = person(vec![]);
        assert!(verdict_refusal(&state, &stranger, op, "p", &input)
            .await
            .is_none());
        let approver = person(vec!["portal-approver".into()]);
        assert!(verdict_refusal(&state, &approver, op, "p", &input)
            .await
            .is_some());
    }

    /// JC2-38: a proposal from a draft the caller may not read answers 404, the answer of a draft
    /// that is not there, and leaves the draft in place.
    #[tokio::test]
    async fn a_proposal_from_an_unreadable_draft_is_not_found() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let manifest = json!({ "kind": "Policy", "metadata": { "name": "hidden", "namespace": "p" },
            "spec": { "contextSpaceRef": { "name": "closed" } } });
        state
            .drafts
            .put("p", "Policy", "hidden", manifest, None, "sub-a", "person")
            .await
            .expect("kept");
        let stranger = Caller::new(
            crate::auth::session::Identity {
                client: None,
                subject: "f:1:x".into(),
                username: "x".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: true,
            },
            Via::Session,
        );
        let input = ManifestInput {
            manifest: None,
            draft: Some(DraftRef {
                kind: "Policy".into(),
                name: "hidden".into(),
            }),
        };
        let refused = propose_with_optional_draft(
            &stranger,
            &state,
            "p",
            "policies",
            "jc_manifest_dry_run",
            input,
        )
        .await
        .expect_err("refused");
        assert!(
            matches!(refused, OpError::Api(ApiError::NotFound(_))),
            "{refused}"
        );
        assert!(state
            .drafts
            .get("p", "Policy", "hidden")
            .await
            .expect("read")
            .is_some());
    }

    /// F-POR3-020: a check of manifest M is never recorded on draft D of another kind or name.
    #[test]
    fn an_inline_manifest_pairs_only_with_its_own_draft() {
        let draft = DraftRef {
            kind: "Endpoint".into(),
            name: "air".into(),
        };
        let own = json!({ "kind": "Endpoint", "metadata": { "name": "air" } });
        let other_name = json!({ "kind": "Endpoint", "metadata": { "name": "bikes" } });
        let other_kind = json!({ "kind": "Pipeline", "metadata": { "name": "air" } });
        assert!(same_draft(&own, &draft).is_ok());
        assert!(
            matches!(same_draft(&other_name, &draft), Err(OpError::InvalidInput { path, .. }) if path == "/draft")
        );
        assert!(same_draft(&other_kind, &draft).is_err());
    }

    // ---------------------------------------------------------------------------------------------
    // T-2120 `datasource_verdict` (AG-62, PF-57, MF-39)
    //
    // The contract: the verdict is green only when the dry run said `valid: true` — in those words,
    // as that type — and it is bound to the manifest it judged, so it can never satisfy another one.
    // A red verdict always says why. The dry run's own output travels on as the trace, which is what
    // the form shows the person; the probe that produces it is what must keep a credential out of it
    // (T-2238, T-2239), not this function, and the last case pins that the trace is passed through
    // unchanged so there is no second place to look.
    // ---------------------------------------------------------------------------------------------

    #[test]
    fn a_datasource_is_green_only_when_the_dry_run_said_valid_in_those_words() {
        let manifest = json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
            "spec": { "http": { "url": "https://example.sk/feed" } },
        });
        assert!(datasource_verdict(&json!({ "valid": true }), &manifest).ok);

        for out in [
            json!({ "valid": false }),
            json!({ "valid": "true" }),
            json!({ "valid": 1 }),
            json!({ "valid": null }),
            json!({ "Valid": true }),
            json!({ "valid": { "ok": true } }),
            json!({ "probe": { "status": 200 } }),
            json!({}),
            json!("valid"),
            Value::Null,
        ] {
            let verdict = datasource_verdict(&out, &manifest);
            assert!(!verdict.ok, "{out} was read as a passing dry run");
            // A red verdict says why, at the level a form shows as an error.
            assert!(
                verdict
                    .findings
                    .iter()
                    .any(|finding| finding.level == Level::Error && !finding.message.is_empty()),
                "{out} produced a red verdict with nothing to read: {:?}",
                verdict.findings,
            );
        }
    }

    #[test]
    fn a_verdict_of_one_datasource_is_never_fresh_for_another() {
        let manifest = json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
            "spec": { "http": { "url": "https://example.sk/feed" } },
        });
        let verdict = datasource_verdict(&json!({ "valid": true }), &manifest);
        assert!(verdict.is_fresh_for(&manifest));

        // The same manifest written in another key order is the same manifest (canonical digest).
        let reordered = json!({
            "spec": { "http": { "url": "https://example.sk/feed" } },
            "metadata": { "namespace": "ovzdusie", "name": "mqtt-mesto" },
            "kind": "DataSource",
            "apiVersion": crate::resource::API_VERSION,
        });
        assert!(verdict.is_fresh_for(&reordered));

        // Anything else is not: another url, another name, another project, a field added or removed.
        for other in [
            json!({ "apiVersion": crate::resource::API_VERSION, "kind": "DataSource",
                    "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
                    "spec": { "http": { "url": "https://evil.example/feed" } } }),
            json!({ "apiVersion": crate::resource::API_VERSION, "kind": "DataSource",
                    "metadata": { "name": "mqtt-inak", "namespace": "ovzdusie" },
                    "spec": { "http": { "url": "https://example.sk/feed" } } }),
            json!({ "apiVersion": crate::resource::API_VERSION, "kind": "DataSource",
                    "metadata": { "name": "mqtt-mesto", "namespace": "doprava" },
                    "spec": { "http": { "url": "https://example.sk/feed" } } }),
            json!({ "apiVersion": crate::resource::API_VERSION, "kind": "DataSource",
                    "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
                    "spec": { "http": { "url": "https://example.sk/feed" },
                              "secretRef": { "name": "feed-token" } } }),
            json!({}),
            Value::Null,
        ] {
            assert!(
                !verdict.is_fresh_for(&other),
                "a verdict of one manifest was fresh for {other}",
            );
        }
        // A red verdict is never fresh for anything, its own manifest included.
        let red = datasource_verdict(&json!({ "valid": false }), &manifest);
        assert!(!red.is_fresh_for(&manifest));
    }

    #[test]
    fn a_skipped_probe_is_said_at_the_field_it_is_about_and_the_trace_is_the_dry_runs_own() {
        let manifest = json!({
            "apiVersion": crate::resource::API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
            "spec": { "http": { "url": "https://example.sk/feed" } },
        });
        let out = json!({
            "valid": true,
            "probe": { "skipped": "the feed needs a credential this check cannot resolve" },
        });
        let verdict = datasource_verdict(&out, &manifest);
        assert!(verdict.ok, "a skipped probe is not a failed check (MF-39)");
        let info: Vec<&Finding> = verdict
            .findings
            .iter()
            .filter(|finding| finding.level == Level::Info)
            .collect();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].path, "spec.http.url");
        assert_eq!(
            info[0].message,
            "the feed needs a credential this check cannot resolve",
        );
        // The trace is the dry run's output, passed through: one place produces it, one place shows
        // it, and nothing here adds to it or reads the manifest into it.
        assert_eq!(verdict.trace.as_ref(), Some(&out));

        // A `skipped` that is not a string is not a finding: only the words the probe wrote are.
        for odd in [
            json!({ "skipped": 1 }),
            json!({ "skipped": null }),
            json!(["skipped"]),
        ] {
            let verdict = datasource_verdict(&json!({ "valid": true, "probe": odd }), &manifest);
            assert!(verdict.findings.is_empty(), "{odd} became a finding");
        }
    }
}
