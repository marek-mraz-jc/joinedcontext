//! One record check for the editor and the proposal gate (T-4840, PL-59, PL-81, GW17): each
//! record a pipeline's sample makes, judged where its write would be judged, before it is
//! proposed. Three sources say why a record would not be written:
//!
//! - `model`: the space's compiled model,
//!   [`ModelSchema::check`](crate::pipeline_validation::ModelSchema::check), the check the
//!   runner's validation stage compiles into its JSON Schema (`pipeline_validation::stage`);
//! - `endpoint`: the gateway's own decision ([`evaluate`] and [`write_guard::check`], the crate
//!   the gateway runs) over the Policies bound to the target Endpoint, the pipeline's own derived
//!   from the draft, then the broker's view of the Endpoint as the reconciler writes it: a write
//!   is admitted only when one rule of the record's type names every Attribute it carries;
//! - `lifetime`: the `expiresAt` the lifetime step wrote (the test runs the step, PL-82), absent
//!   where an entry should have set one, unreadable or already past.
//!
//! The check reads records only; it logs none of them and keeps none.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use context_gateway::auth::subject::Subject;
use context_gateway::pdp::evaluator::{evaluate, Constraints, Request, Verdict as Decision};
use context_gateway::pdp::write_guard::{self, Refusal};
use jc_core::kinds::pipeline_identity;
use jc_core::kinds::{Operation, PipelineSpec, PolicySpec};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::ops::verdict::{digest_of, RecordsVerdict};
use crate::reconciler::views::WriteView;
use crate::state::AppState;

/// Records one check reads at most.
pub const MAX_RECORDS: usize = 100;
/// The largest record one check reads, as JSON.
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

/// The members the broker keeps as no Attribute of an entity (Antares `ENTITY_META_KEYS`).
const NO_ATTRIBUTE: [&str; 8] = [
    "id",
    "type",
    "scope",
    "createdAt",
    "modifiedAt",
    "deletedAt",
    "expiresAt",
    "@context",
];

/// Every pipeline writes through its Endpoint's batch upsert (`streams::upsert`).
const WRITE: Operation = Operation::UpsertBatch;

/// Where a finding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Model,
    Endpoint,
    Lifetime,
    History,
}

/// One reason a record would not be written as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Finding {
    pub source: Source,
    /// The rule: a SHACL component for the model, `policy`, `grant`, `view` or `NOT_WRITABLE`
    /// for the Endpoint, `from` or `expiresAt` for the lifetime, and for the history plan
    /// `latest`, `historyOff`, `received`, `spaceRule`, `otherPipeline` or `observedAt` (PL-109).
    pub rule: String,
    /// The member at fault, empty for the record itself.
    pub field: String,
    /// What is wrong, in words; never a value of the record.
    pub reason: String,
    /// The access request that would cure it, when the finding is a grant the pipeline lacks on
    /// an Endpoint of another Context Space (PL-90): the editor opens the request form with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
}

/// What an access request for a missing grant names (PL-90): the Endpoint, the operation and
/// the entity type, when the pipeline names one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Access {
    pub project: String,
    pub name: String,
    /// `read` for a source, `write` for an output.
    pub operations: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
}

/// The `expiresAt` a record would be written with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct Expires {
    /// The entity's, when it gets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// Each Attribute's, by name; the first instance's when it has several.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

/// One record's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RecordCheck {
    pub index: usize,
    pub ok: bool,
    /// The Endpoint the record is written through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub findings: Vec<Finding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<Expires>,
}

/// Every record's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Checked {
    pub ok: bool,
    pub checked: usize,
    pub rejected: usize,
    pub records: Vec<RecordCheck>,
}

impl Checked {
    /// What a proposal of `pipeline` needs from this check (PF-57): green, of this spec, on
    /// these records. No record checked is no green verdict.
    pub fn verdict(&self, pipeline: &Value, records: &[Value]) -> RecordsVerdict {
        RecordsVerdict {
            ok: self.ok && self.checked > 0,
            checked: self.checked,
            rejected: self.rejected,
            spec_digest: digest_of(&pipeline["spec"]),
            sample_digest: digest_of(&Value::Array(records.to_vec())),
        }
    }
}

/// Each finding of a rejected record as a verdict's finding (AG-62): `records[i].field`.
pub fn verdict_findings(checked: &Checked) -> Vec<crate::ops::verdict::Finding> {
    use crate::ops::verdict::{Finding as VerdictFinding, Level};
    checked
        .records
        .iter()
        .flat_map(|record| {
            record.findings.iter().map(move |f| VerdictFinding {
                level: Level::Error,
                path: if f.field.is_empty() {
                    format!("records[{}]", record.index)
                } else {
                    format!("records[{}].{}", record.index, f.field)
                },
                message: format!("{}: {}", f.field_label(), f.reason),
            })
        })
        .collect()
}

impl Finding {
    /// What a person reads before the reason: the member, or the record's source.
    fn field_label(&self) -> String {
        let source = match self.source {
            Source::Model => "model",
            Source::Endpoint => "endpoint",
            Source::Lifetime => "lifetime",
            Source::History => "history",
        };
        if self.field.is_empty() {
            source.to_owned()
        } else {
            self.field.clone()
        }
    }
}

/// What one output's Endpoint judges a write by.
struct Target {
    endpoint: String,
    space: Option<String>,
    /// The pipeline's project when the Endpoint names a Context Space the request door counts as
    /// another one (`space != permissions::space_ref(pipeline)`), so a missing grant there is one
    /// an access request can ask for (PL-90). A Pipeline manifest carries no contextSpaceRef, so
    /// space_ref is None today and every held output Endpoint is requestable, the pipeline's own
    /// space included; that matches the door's rule.
    requestable: Option<String>,
    /// `None` when this Portal writes no views (no broker configured); `Some(None)` when it does
    /// but has not read the Endpoint yet.
    write: Option<Option<WriteView>>,
}

/// Judges `records` as the pipeline `pipeline` (its manifest) would write them, at `now`.
pub fn check_records(
    state: &AppState,
    project: &str,
    pipeline: &Value,
    records: &[Value],
    now: DateTime<Utc>,
) -> Result<Checked, ApiError> {
    if records.len() > MAX_RECORDS {
        return Err(ApiError::BadRequest(format!(
            "records: at most {MAX_RECORDS} at a time, {} given",
            records.len()
        )));
    }
    let Writer {
        spec,
        subject,
        types,
        targets,
    } = writer(state, project, pipeline)?;
    let name = pipeline
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let history = SpaceHistory::of(state, project, name, &spec, &targets);
    let unreadable = judge_sources(state, project, &spec);

    let checks: Vec<RecordCheck> = records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let entity_type = record.get("type").and_then(Value::as_str).unwrap_or_default();
            let target = target_of(&types, &targets, entity_type);
            let mut findings = Vec::new();
            let size = serde_json::to_vec(record).map_or(usize::MAX, |bytes| bytes.len());
            if !record.is_object() {
                findings.push(model("object", "", "a record is one JSON object, the entity"));
            } else if size > MAX_RECORD_BYTES {
                findings.push(model(
                    "size",
                    "",
                    &format!("the record is larger than {MAX_RECORD_BYTES} bytes, more than one check reads"),
                ));
            } else {
                match target {
                    None => findings.push(endpoint_finding(
                        "targetEndpoint",
                        "",
                        "the pipeline names no Endpoint to write through".into(),
                    )),
                    Some(target) => {
                        judge_model(state, project, target, record, &mut findings);
                        judge_endpoint(&subject, target, entity_type, record, now, &mut findings);
                        history.judge(target, entity_type, record, &mut findings);
                    }
                }
                judge_lifetime(&spec, entity_type, record, now, &mut findings);
            }
            findings.extend(unreadable.iter().cloned());
            RecordCheck {
                index,
                ok: findings.is_empty(),
                endpoint: target.map(|t| t.endpoint.clone()),
                findings,
                expires: expires_of(record),
            }
        })
        .collect();
    let rejected = checks.iter().filter(|c| !c.ok).count();
    Ok(Checked {
        ok: rejected == 0,
        checked: checks.len(),
        rejected,
        records: checks,
    })
}

/// The history plans of the pipeline with what each target space holds to judge them by
/// (PL-109): its retention rules and the other pipelines' plans, read once per check.
struct SpaceHistory {
    plans: Vec<crate::pipeline_history::Plan>,
    spaces: BTreeMap<
        String,
        (
            Vec<jc_core::kinds::RetentionRule>,
            Vec<crate::pipeline_history::Plan>,
        ),
    >,
}

impl SpaceHistory {
    fn of(
        state: &AppState,
        project: &str,
        name: &str,
        spec: &PipelineSpec,
        targets: &[Target],
    ) -> Self {
        let plans = crate::pipeline_history::plans(name, spec);
        let mut spaces = BTreeMap::new();
        if !plans.is_empty() {
            for space in targets.iter().filter_map(|t| t.space.as_deref()) {
                spaces.entry(space.to_owned()).or_insert_with(|| {
                    (
                        crate::pipeline_history::space_rules(&state.mirror, project, space),
                        crate::pipeline_history::other_plans(&state.mirror, project, space, name),
                    )
                });
            }
        }
        SpaceHistory { plans, spaces }
    }

    /// Each plan of the record's type for an attribute the record carries: the space's and
    /// broker's rules, then the record's own time.
    fn judge(
        &self,
        target: &Target,
        entity_type: &str,
        record: &Value,
        findings: &mut Vec<Finding>,
    ) {
        let Some(space) = target.space.as_deref() else {
            return;
        };
        let Some((rules, others)) = self.spaces.get(space) else {
            return;
        };
        for plan in self
            .plans
            .iter()
            .filter(|p| p.entity_type == entity_type && record.get(&p.attribute).is_some())
        {
            let refused = crate::pipeline_history::refusal(rules, space, plan, others)
                .or_else(|| crate::pipeline_history::record_refusal(plan, record));
            if let Some((rule, reason)) = refused {
                findings.push(Finding {
                    source: Source::History,
                    rule: rule.to_owned(),
                    field: plan.attribute.clone(),
                    reason,
                    access: None,
                });
            }
        }
    }
}

/// The pipeline as the writer its records are judged as: its spec, its account, and per output
/// the type it writes and the Endpoint it writes through.
struct Writer {
    spec: PipelineSpec,
    subject: Subject,
    types: Vec<Option<String>>,
    targets: Vec<Target>,
}

fn writer(state: &AppState, project: &str, pipeline: &Value) -> Result<Writer, ApiError> {
    let spec: PipelineSpec = serde_json::from_value(pipeline["spec"].clone())
        .map_err(|e| ApiError::BadRequest(format!("pipeline.spec: {e}")))?;
    let name = pipeline
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (account, derived) = identity(state, project, name, &spec);
    let outputs = spec.outputs();
    let types = outputs.iter().map(|o| o.entity_type.clone()).collect();
    let own_space = crate::permissions::space_ref(pipeline);
    let targets: Vec<Target> = outputs
        .iter()
        .map(|output| {
            let endpoint = output.target_endpoint.local_id().to_owned();
            let space =
                crate::pipeline_expiry::space_of_endpoint(&state.mirror, project, &endpoint);
            Target {
                requestable: (space.is_some() && space != own_space).then(|| project.to_owned()),
                space,
                write: state.views.as_ref().map(|views| {
                    views.write_view(project, &endpoint, &account, derived.as_deref())
                }),
                endpoint,
            }
        })
        .collect();
    let subject = Subject {
        service_account: Some(account),
        ..Subject::default()
    };
    Ok(Writer {
        spec,
        subject,
        types,
        targets,
    })
}

/// The output a record of `entity_type` leaves by: the one of its type, else one of every type,
/// else the first.
fn target_of<'a>(
    types: &[Option<String>],
    targets: &'a [Target],
    entity_type: &str,
) -> Option<&'a Target> {
    types
        .iter()
        .position(|t| t.as_deref() == Some(entity_type))
        .or_else(|| types.iter().position(Option::is_none))
        .or((!types.is_empty()).then_some(0))
        .map(|at| &targets[at])
}

/// Whether the pipeline may write an attribute of a type through its Endpoint (T-4842).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Writable {
    pub name: String,
    pub writable: bool,
    /// Why not, or the limit it is written under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Each of `names` of `entity_type` judged alone, as [`check_records`] judges a record carrying
/// it: the gateway's grant of the attribute, then the broker's view. A grant limited to an area
/// admits the location only inside it, which a name alone cannot show: writable, with that limit.
pub fn writable(
    state: &AppState,
    project: &str,
    pipeline: &Value,
    entity_type: &str,
    names: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<Writable>, ApiError> {
    let writer = writer(state, project, pipeline)?;
    let target = target_of(&writer.types, &writer.targets, entity_type);
    Ok(names
        .iter()
        .map(|name| {
            let probe = serde_json::json!({ "type": entity_type, name: { "type": "Property", "value": 0 } });
            let mut findings = Vec::new();
            match target {
                None => findings.push(endpoint_finding(
                    "targetEndpoint",
                    "",
                    "the pipeline names no Endpoint to write through".into(),
                )),
                Some(target) => judge_write(
                    &writer.subject,
                    target,
                    entity_type,
                    &probe,
                    now,
                    write_guard::check_fragment,
                    &mut findings,
                ),
            }
            let area = findings.iter().position(|f| f.field == "location" && f.rule == "grant");
            let reason = match area {
                Some(at) if findings.len() == 1 => {
                    findings.remove(at);
                    Some("only inside the area its grant names".to_owned())
                }
                _ => findings.first().map(|f| f.reason.clone()),
            };
            Writable {
                name: name.clone(),
                writable: findings.is_empty(),
                reason,
            }
        })
        .collect())
}

/// [`check_records`] over what a test's mapping made: its first [`MAX_RECORDS`] records.
pub fn check_trace(
    state: &AppState,
    project: &str,
    pipeline: &Value,
    mapping: &[Value],
) -> Result<Checked, ApiError> {
    let records = &mapping[..mapping.len().min(MAX_RECORDS)];
    check_records(state, project, pipeline, records, Utc::now())
}

/// The account the pipeline writes as and, under per-pipeline identity, its Policies derived
/// from this draft (PL-19, PL-20); the shared `pipelines` account keeps the Policies it has.
pub(crate) fn identity(
    state: &AppState,
    project: &str,
    name: &str,
    spec: &PipelineSpec,
) -> (String, Option<Vec<(String, PolicySpec)>>) {
    if !state.config.pipeline_identity {
        return (
            crate::reconciler::streams::PIPELINE_ACCOUNT.to_owned(),
            None,
        );
    }
    let derived = pipeline_identity::derive(
        project,
        name,
        spec,
        "",
        &crate::api::assistant::org_domain(state, project),
        |endpoint| crate::pipeline_expiry::space_of_endpoint(&state.mirror, project, endpoint),
    );
    (derived.name, Some(derived.policies))
}

fn model(rule: &str, field: &str, reason: &str) -> Finding {
    Finding {
        source: Source::Model,
        rule: rule.into(),
        field: field.into(),
        reason: reason.into(),
        access: None,
    }
}

fn endpoint_finding(rule: &str, field: &str, reason: String) -> Finding {
    Finding {
        source: Source::Endpoint,
        rule: rule.into(),
        field: field.into(),
        reason,
        access: None,
    }
}

fn not_in_project(endpoint: &str) -> String {
    format!("Endpoint {endpoint} is not in this project")
}

fn not_read_yet(endpoint: &str) -> String {
    format!(
        "Endpoint {endpoint} has no view on the broker yet: the reconciler has not read it; test \
         again once it has"
    )
}

/// Whether a verdict finding's `message` only says `endpoint` is not in the project or not read
/// yet: what every test says of an Endpoint drafted beside the pipeline, which exists once both
/// are approved (T-6532: the assistant's KPI pipeline into a new indicator space).
pub fn awaits_endpoint(message: &str, endpoint: &str) -> bool {
    let label = endpoint_finding("", "", String::new()).field_label();
    message
        .strip_prefix(&format!("{label}: "))
        .is_some_and(|reason| {
            reason == not_in_project(endpoint) || reason == not_read_yet(endpoint)
        })
}

fn lifetime(rule: &str, field: &str, reason: String) -> Finding {
    Finding {
        source: Source::Lifetime,
        rule: rule.into(),
        field: field.into(),
        reason,
        access: None,
    }
}

/// Each source whose typed `endpointRef` names another project's Endpoint that does not admit
/// this project (PL-90, PL-92), as the reconciler refuses it. A missing Endpoint reads the same,
/// so the finding never tells whether another project holds that name.
fn judge_sources(state: &AppState, project: &str, spec: &PipelineSpec) -> Vec<Finding> {
    spec.sources()
        .iter()
        .filter_map(|source| {
            let reference = source.endpoint_ref.as_ref()?;
            let owner = reference.namespace().filter(|owner| *owner != project)?;
            crate::reconciler::streams::source_endpoint(&state.mirror, project, reference).err()?;
            let name = reference.name();
            Some(Finding {
                access: Some(Access {
                    project: owner.to_owned(),
                    name: name.to_owned(),
                    operations: "read".into(),
                    entity_type: source.query.as_ref().and_then(|q| q.entity_type.clone()),
                }),
                ..endpoint_finding(
                    "sourceEndpoint",
                    "",
                    format!(
                        "this project holds no access to source Endpoint {owner}/{name}: ask its \
                         owner for a read (PL-90)"
                    ),
                )
            })
        })
        .collect()
}

/// The space's model, when it names one: a space without one checks nothing on write either.
fn judge_model(
    state: &AppState,
    project: &str,
    target: &Target,
    record: &Value,
    findings: &mut Vec<Finding>,
) {
    let Some(space) = &target.space else {
        findings.push(endpoint_finding(
            "targetEndpoint",
            "",
            not_in_project(&target.endpoint),
        ));
        return;
    };
    if let Some(schema) = state.model_schemas.get(project, space) {
        findings.extend(
            schema
                .check(record)
                .into_iter()
                .map(|p| model(&p.rule, &p.path, &p.message)),
        );
    }
}

/// The gateway's decision over the Endpoint's Policies, then the broker's view.
fn judge_endpoint(
    subject: &Subject,
    target: &Target,
    entity_type: &str,
    record: &Value,
    now: DateTime<Utc>,
    findings: &mut Vec<Finding>,
) {
    judge_write(
        subject,
        target,
        entity_type,
        record,
        now,
        write_guard::check,
        findings,
    );
}

/// [`judge_endpoint`] with the gateway's guard for a whole entity or for an attribute fragment.
fn judge_write(
    subject: &Subject,
    target: &Target,
    entity_type: &str,
    record: &Value,
    now: DateTime<Utc>,
    guard: fn(&Value, &Constraints) -> Result<(), Refusal>,
    findings: &mut Vec<Finding>,
) {
    let endpoint = &target.endpoint;
    let write = match &target.write {
        None => return,
        Some(None) => {
            findings.push(endpoint_finding("view", "", not_read_yet(endpoint)));
            return;
        }
        Some(Some(write)) => write,
    };
    let request = Request {
        types: BTreeSet::from([entity_type.to_owned()]),
        ..Request::default()
    };
    match evaluate(
        subject,
        WRITE,
        &request,
        &write.tenant,
        &write.policies,
        now,
    ) {
        Decision::Deny => findings.push(Finding {
            access: target.requestable.as_ref().map(|project| Access {
                project: project.clone(),
                name: endpoint.clone(),
                operations: "write".into(),
                entity_type: Some(entity_type.to_owned()).filter(|t| !t.is_empty()),
            }),
            ..endpoint_finding(
                "policy",
                "",
                format!(
                    "no Policy bound to Endpoint {endpoint} grants this pipeline a batch upsert \
                     of {entity_type}"
                ),
            )
        }),
        Decision::Rewrite(constraints) => {
            if let Err(refusal) = guard(record, &constraints) {
                findings.push(endpoint_finding(
                    "grant",
                    refused_field(&refusal),
                    format!("{refusal}: not writable through {endpoint}"),
                ));
            }
        }
    }
    match &write.view {
        Ok(view) => findings.extend(judge_view(view, endpoint, entity_type, record)),
        Err(why) => findings.push(endpoint_finding(
            "view",
            "",
            format!("Endpoint {endpoint} serves no view on the broker: {why}"),
        )),
    }
}

/// The member a gateway refusal is about.
fn refused_field(refusal: &Refusal) -> &str {
    match refusal {
        Refusal::MalformedId(_) | Refusal::IdOutsideGrant(_) => "id",
        Refusal::TypeOutsideGrant(_) => "type",
        Refusal::AttributeOutsideGrant(name) | Refusal::SmuggledPolicyAttribute(name) => name,
        Refusal::ScopeOutsideGrant(_) | Refusal::ScopeMissing => "scope",
        Refusal::LocationOutsideGrant => "location",
    }
}

/// The broker's view (Antares `WriteGuard::judge_names`): the view's operations admit the
/// upsert, and one rule of the type names every Attribute the record carries.
pub(crate) fn judge_view(
    view: &Value,
    endpoint: &str,
    entity_type: &str,
    record: &Value,
) -> Vec<Finding> {
    let names_of = |value: &Value| -> BTreeSet<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let rows = names_of(&view["operations"]);
    if !crate::reconciler::views::rows_of(WRITE)
        .iter()
        .any(|row| rows.contains(*row))
    {
        return vec![endpoint_finding(
            "NOT_WRITABLE",
            "",
            format!("the view of Endpoint {endpoint} admits no batch upsert: its Policies grant no write"),
        )];
    }
    let rules: Vec<BTreeSet<String>> = view["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| names_of(&rule["types"]).contains(entity_type))
        .map(|rule| names_of(&rule["attributes"]))
        .collect();
    if rules.is_empty() {
        return vec![endpoint_finding(
            "NOT_WRITABLE",
            "type",
            format!("{entity_type} is not writable through {endpoint}: its view holds no rule for the type"),
        )];
    }
    let names: BTreeSet<String> = record
        .as_object()
        .into_iter()
        .flat_map(|members| members.keys())
        .filter(|name| !NO_ATTRIBUTE.contains(&name.as_str()))
        .cloned()
        .collect();
    if rules.iter().any(|rule| rule.is_superset(&names)) {
        return Vec::new();
    }
    let shown: BTreeSet<&String> = rules.iter().flatten().collect();
    let missing: Vec<Finding> = names
        .iter()
        .filter(|name| !shown.contains(name))
        .map(|name| {
            endpoint_finding(
                "NOT_WRITABLE",
                name,
                format!(
                    "not writable through {endpoint}: its view lets {entity_type} write {} \
                     Attributes and not this one",
                    shown.len()
                ),
            )
        })
        .collect();
    if missing.is_empty() {
        vec![endpoint_finding(
            "NOT_WRITABLE",
            "",
            format!(
                "not writable through {endpoint}: the record's Attributes sit in different rules \
                 of its view, and one write must fit one rule"
            ),
        )]
    } else {
        missing
    }
}

/// The lifetime entries of the record's type, against what the step wrote (PL-82), and every
/// `expiresAt` it carries: a date-time still ahead.
fn judge_lifetime(
    spec: &PipelineSpec,
    entity_type: &str,
    record: &Value,
    now: DateTime<Utc>,
    findings: &mut Vec<Finding>,
) {
    for entry in spec
        .lifetime
        .iter()
        .filter(|l| l.entity_type == entity_type)
    {
        if let Some(mode) = &entry.entity {
            if record.get("expiresAt").is_none() {
                let from = mode.from.as_deref().unwrap_or("expiresAt");
                findings.push(lifetime(
                    "from",
                    from,
                    format!("the record gives no date-time in {from}, so the entity would be written without expiresAt"),
                ));
            }
        }
        for attributes in &entry.attributes {
            for name in &attributes.names {
                let Some(attribute) = record.get(name) else {
                    continue;
                };
                if instances(attribute).any(|i| i.get("expiresAt").is_none()) {
                    let from = attributes.from.as_deref().unwrap_or(name);
                    findings.push(lifetime(
                        "from",
                        name,
                        format!("the record gives no date-time in {from}, so {name} would be written without expiresAt"),
                    ));
                }
            }
        }
    }
    let mut ends: Vec<(String, &Value)> = record
        .get("expiresAt")
        .map(|end| ("expiresAt".to_owned(), end))
        .into_iter()
        .collect();
    if let Some(members) = record.as_object() {
        for (name, attribute) in members {
            if NO_ATTRIBUTE.contains(&name.as_str()) {
                continue;
            }
            ends.extend(
                instances(attribute)
                    .filter_map(|i| i.get("expiresAt"))
                    .map(|end| (format!("{name}.expiresAt"), end)),
            );
        }
    }
    for (field, end) in ends {
        match end
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        {
            None => findings.push(lifetime(
                "expiresAt",
                &field,
                "is no RFC 3339 date-time".into(),
            )),
            Some(end) if end <= now => findings.push(lifetime(
                "expiresAt",
                &field,
                "lies in the past: the broker would hide it as soon as it is written".into(),
            )),
            Some(_) => {}
        }
    }
}

/// An Attribute's instances: the object itself, or each of an array of them.
fn instances(attribute: &Value) -> impl Iterator<Item = &Value> {
    match attribute {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![attribute],
        _ => Vec::new(),
    }
    .into_iter()
}

/// The ends the record carries, `None` when it carries none.
fn expires_of(record: &Value) -> Option<Expires> {
    let members = record.as_object()?;
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let expires = Expires {
        entity: members.get("expiresAt").and_then(text),
        attributes: members
            .iter()
            .filter(|(name, _)| !NO_ATTRIBUTE.contains(&name.as_str()))
            .filter_map(|(name, attribute)| {
                instances(attribute)
                    .find_map(|i| i.get("expiresAt").and_then(text))
                    .map(|end| (name.clone(), end))
            })
            .collect(),
    };
    (expires != Expires::default()).then_some(expires)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6532: an Endpoint drafted beside the pipeline is told apart by the verdict's own words,
    /// and only for that Endpoint.
    #[test]
    fn a_verdict_finding_on_an_endpoint_not_held_yet_is_told_apart() {
        let checked = Checked {
            ok: false,
            checked: 1,
            rejected: 1,
            records: vec![RecordCheck {
                index: 0,
                ok: false,
                endpoint: Some("t-kpi".into()),
                findings: vec![
                    endpoint_finding("targetEndpoint", "", not_in_project("t-kpi")),
                    endpoint_finding("view", "", not_read_yet("t-kpi")),
                    endpoint_finding("policy", "", "no Policy grants it".into()),
                ],
                expires: None,
            }],
        };
        let messages: Vec<String> = verdict_findings(&checked)
            .into_iter()
            .map(|f| f.message)
            .collect();
        assert!(awaits_endpoint(&messages[0], "t-kpi"), "{messages:?}");
        assert!(awaits_endpoint(&messages[1], "t-kpi"), "{messages:?}");
        assert!(!awaits_endpoint(&messages[2], "t-kpi"));
        assert!(!awaits_endpoint(&messages[0], "other"));
    }
    use crate::config::Config;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use chrono::TimeZone;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap()
    }

    /// Helsinki's vehicles space: its model's Bus has a date-time `observedOn`, a `speed` and a
    /// `position`; the Endpoint `helsinki-all` writes into it.
    fn world() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Endpoint".to_owned(),
            metadata: ObjectMeta::new("helsinki-all", "helsinki"),
            spec: json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "vehicles" }, "slug": "helsinki-all" }),
            status: None,
        });
        let schema = json!({ "definitions": { "Bus": { "type": "object", "properties": {
            "observedOn": { "type": "string", "format": "date-time", "x-ngsi-ld-kind": "Property" },
            "speed": { "type": "number", "x-ngsi-ld-kind": "Property" },
            "line": { "type": "string", "enum": ["550", "551"], "x-ngsi-ld-kind": "Property" },
            "plate": { "type": "string", "pattern": "^[A-Z]{3}-[0-9]{3}$", "x-ngsi-ld-kind": "Property" },
            "position": { "type": "object", "x-ngsi-ld-kind": "GeoProperty" }
        }}}});
        let model = crate::pipeline_validation::ModelSchema::compile(
            "transport",
            "1.0.0",
            &schema,
            &["Bus".to_owned()],
            false,
        );
        state.model_schemas.replace(HashMap::from([(
            ("helsinki".to_owned(), "vehicles".to_owned()),
            Arc::new(model),
        )]));
        state
    }

    fn pipeline(lifetime: Value) -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha2",
            "kind": "Pipeline",
            "metadata": { "name": "hsl-hfp-vehicles" },
            "spec": {
                "class": "resident",
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "hfp" } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:vehicles:helsinki-all", "type": "Bus" }],
                "lifetime": lifetime,
            }
        })
    }

    fn bus(speed: Value) -> Value {
        json!({
            "id": "urn:ngsi-ld:Bus:1104", "type": "Bus",
            "speed": { "type": "Property", "value": speed },
            "observedOn": { "type": "Property", "value": "2026-10-20T11:58:00Z" }
        })
    }

    fn check(state: &AppState, pipeline: &Value, records: &[Value]) -> Checked {
        check_records(state, "helsinki", pipeline, records, now()).expect("checked")
    }

    #[test]
    fn a_record_the_model_refuses_is_red_with_the_models_rule() {
        let checked = check(
            &world(),
            &pipeline(json!([])),
            &[bus(json!(41)), bus(json!("fast"))],
        );
        assert!(checked.records[0].ok, "{:?}", checked.records[0]);
        let red = &checked.records[1];
        assert!(!red.ok);
        assert_eq!(red.endpoint.as_deref(), Some("helsinki-all"));
        assert!(red.findings.iter().all(|f| f.source == Source::Model));
        assert!(
            red.findings.iter().any(|f| f.field == "speed"),
            "{:?}",
            red.findings
        );
        assert_eq!(
            (checked.checked, checked.rejected, checked.ok),
            (2, 1, false)
        );
    }

    /// A refusal names the rule and the attribute, never the value the record carries: a
    /// finding is shown and kept, a record value is not (sh:in, sh:pattern).
    #[test]
    fn a_model_refusal_never_quotes_the_records_value() {
        let mut record = bus(json!(41));
        record["line"] = json!({ "type": "Property", "value": "secret-line-77" });
        record["plate"] = json!({ "type": "Property", "value": "secret-plate-99" });
        let checked = check(&world(), &pipeline(json!([])), &[record]);
        let findings = &checked.records[0].findings;
        for field in ["line", "plate"] {
            assert!(
                findings.iter().any(|f| f.field == field),
                "{field}: {findings:?}"
            );
        }
        let text = serde_json::to_string(findings).expect("JSON");
        assert!(!text.contains("secret-"), "{text}");
    }

    /// The same record judged by the check and by the runtime stage: both are the space's
    /// compiled model, which `pipeline_validation::stage` turns into the stage's JSON Schema.
    #[test]
    fn the_model_verdict_is_the_runtime_stages() {
        let state = world();
        let schema = state
            .model_schemas
            .get("helsinki", "vehicles")
            .expect("the model");
        let records = [
            bus(json!(41)),
            bus(json!("fast")),
            json!({ "id": "urn:ngsi-ld:Tram:1", "type": "Tram" }),
            json!({ "id": "urn:ngsi-ld:Bus:2", "type": "Bus", "colour": { "type": "Property", "value": "red" } }),
        ];
        let checked = check(&state, &pipeline(json!([])), &records);
        for (record, verdict) in records.iter().zip(&checked.records) {
            let stage: Vec<(String, String)> = schema
                .check(record)
                .into_iter()
                .map(|p| (p.rule, p.path))
                .collect();
            let ours: Vec<(String, String)> = verdict
                .findings
                .iter()
                .filter(|f| f.source == Source::Model)
                .map(|f| (f.rule.clone(), f.field.clone()))
                .collect();
            assert_eq!(ours, stage, "{record}");
            assert_eq!(verdict.ok, stage.is_empty());
        }
    }

    fn view() -> Value {
        json!({ "operations": ["batch_create", "batch_upsert", "query_entities_post"],
                "rules": [{ "types": ["Bus"], "attributes": ["location", "observedOn", "position", "speed"] }] })
    }

    fn target(policies: Value, view: Result<Value, String>) -> Target {
        Target {
            endpoint: "helsinki-all".into(),
            space: Some("vehicles".into()),
            requestable: None,
            write: Some(Some(WriteView {
                space: "vehicles".into(),
                tenant: "helsinki-vehicles".into(),
                policies: serde_json::from_value(policies).expect("Policies"),
                view,
            })),
        }
    }

    fn grant(extra: Value) -> Value {
        let mut policy = json!({
            "contextSpaceRef": "vehicles", "assigner": "did:web:hel.fi",
            "assignee": { "kind": "serviceAccount", "id": "pl-hsl-hfp-vehicles" },
            "operations": ["upsertBatch"],
            "information": [{ "entities": [{ "type": "Bus" }] }]
        });
        if let (Some(policy), Some(extra)) = (policy.as_object_mut(), extra.as_object()) {
            policy.extend(extra.clone());
        }
        policy
    }

    fn endpoint_findings(target: &Target, record: &Value) -> Vec<Finding> {
        let subject = Subject {
            service_account: Some("pl-hsl-hfp-vehicles".into()),
            ..Subject::default()
        };
        let mut findings = Vec::new();
        judge_endpoint(&subject, target, "Bus", record, now(), &mut findings);
        findings
    }

    /// A record carrying an Attribute its grant does not name is refused by the gateway, which
    /// only the smoke found before (hsl-hfp-vehicles, dev 2026-10-20); the entity's `expiresAt`
    /// is a system member a grant need not name (DM-130).
    #[test]
    fn an_attribute_the_endpoint_does_not_grant_is_named_with_the_endpoint() {
        let mut record = bus(json!(41));
        record["expiresAt"] = json!("2026-10-20T12:10:00Z");
        let named = grant(json!({ "information": [{ "entities": [{ "type": "Bus" }],
            "propertyNames": ["speed"] }] }));
        let findings = endpoint_findings(&target(json!([named]), Ok(view())), &record);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            (findings[0].source, findings[0].rule.as_str()),
            (Source::Endpoint, "grant")
        );
        assert_eq!(findings[0].field, "observedOn");
        assert!(findings[0]
            .reason
            .ends_with("not writable through helsinki-all"));

        // A grant of the whole type lets it through, and so does the broker's view.
        assert!(
            endpoint_findings(&target(json!([grant(json!({}))]), Ok(view())), &record).is_empty()
        );
    }

    /// The Target step's lock column (T-4842): an attribute alone, judged by the grant and the
    /// view the record check uses; a grant limited to an area keeps the location writable there.
    #[test]
    fn each_attribute_is_writable_or_named_with_why_not() {
        let subject = Subject {
            service_account: Some("pl-hsl-hfp-vehicles".into()),
            ..Subject::default()
        };
        let named = grant(json!({ "information": [{ "entities": [{ "type": "Bus" }],
            "propertyNames": ["speed", "location"] }] }));
        let judged = |target: &Target, name: &str| {
            let probe = json!({ "type": "Bus", name: { "type": "Property", "value": 0 } });
            let mut findings = Vec::new();
            judge_write(
                &subject,
                target,
                "Bus",
                &probe,
                now(),
                write_guard::check_fragment,
                &mut findings,
            );
            findings
        };
        let granted = target(json!([named]), Ok(view()));
        assert!(judged(&granted, "speed").is_empty());
        let refused = judged(&granted, "observedOn");
        assert_eq!(
            (refused[0].rule.as_str(), refused[0].field.as_str()),
            ("grant", "observedOn")
        );
        let unviewed = judged(&target(json!([grant(json!({}))]), Ok(view())), "pm25");
        assert_eq!(unviewed[0].rule, "NOT_WRITABLE");
        assert_eq!(
            judged(&target(json!([]), Ok(view())), "speed")[0].rule,
            "policy"
        );

        // Through the whole path: no broker here, so the grant alone is unknown and all pass;
        // a pipeline without an Endpoint says so for every attribute.
        let all = writable(
            &world(),
            "helsinki",
            &pipeline(json!([])),
            "Bus",
            &["speed".into()],
            now(),
        )
        .expect("judged");
        assert_eq!(
            all,
            [Writable {
                name: "speed".into(),
                writable: true,
                reason: None
            }]
        );
        let mut orphan = pipeline(json!([]));
        orphan["spec"]["outputs"] = json!([]);
        if let Ok(none) = writable(
            &world(),
            "helsinki",
            &orphan,
            "Bus",
            &["speed".into()],
            now(),
        ) {
            assert!(!none[0].writable);
            assert!(none[0]
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("no Endpoint")));
        }
    }

    #[test]
    fn no_grant_a_view_without_the_attribute_or_without_a_write_is_red() {
        let record = bus(json!(41));
        let denied = endpoint_findings(&target(json!([]), Ok(view())), &record);
        assert_eq!(denied[0].rule, "policy");
        assert!(denied[0].reason.contains("batch upsert of Bus"));

        let mut stray = record.clone();
        stray["pm25"] = json!({ "type": "Property", "value": 3 });
        let unviewed = endpoint_findings(&target(json!([grant(json!({}))]), Ok(view())), &stray);
        assert_eq!(
            (unviewed[0].rule.as_str(), unviewed[0].field.as_str()),
            ("NOT_WRITABLE", "pm25")
        );

        let read_only = json!({ "operations": ["query_entities"], "rules": view()["rules"] });
        let refused = endpoint_findings(&target(json!([grant(json!({}))]), Ok(read_only)), &record);
        assert!(refused[0].reason.contains("admits no batch upsert"));

        let none = endpoint_findings(
            &target(json!([grant(json!({}))]), Err("no Policy".into())),
            &record,
        );
        assert_eq!(none[0].rule, "view");
        let unread = Target {
            write: Some(None),
            ..target(json!([]), Ok(view()))
        };
        assert!(endpoint_findings(&unread, &record)[0]
            .reason
            .contains("has not read it"));
    }

    /// A write no Policy grants on an Endpoint of another space names the access request that
    /// would cure it (PL-90, T-5369); one of the pipeline's own space names none, since a
    /// request there is refused.
    #[test]
    fn a_missing_grant_in_another_space_names_the_access_request() {
        let record = bus(json!(41));
        let other = Target {
            requestable: Some("helsinki".into()),
            ..target(json!([]), Ok(view()))
        };
        let findings = endpoint_findings(&other, &record);
        assert_eq!(findings[0].rule, "policy");
        assert_eq!(
            findings[0].access,
            Some(Access {
                project: "helsinki".into(),
                name: "helsinki-all".into(),
                operations: "write".into(),
                entity_type: Some("Bus".into()),
            })
        );
        let own = endpoint_findings(&target(json!([]), Ok(view())), &record);
        assert_eq!(own[0].rule, "policy");
        assert_eq!(own[0].access, None);
        // A grant writes: no finding, no request.
        let granted = Target {
            requestable: Some("helsinki".into()),
            ..target(json!([grant(json!({}))]), Ok(view()))
        };
        assert!(endpoint_findings(&granted, &record).is_empty());
    }

    /// An output Endpoint the project holds is one the request door takes (PL-90), by the rule
    /// it applies: any space but the pipeline's own; one the project does not hold offers none.
    #[test]
    fn only_an_output_endpoint_the_project_holds_is_requestable() {
        let state = world();
        let held = writer(&state, "helsinki", &pipeline(json!([]))).expect("writer");
        assert_eq!(held.targets[0].requestable.as_deref(), Some("helsinki"));
        let mut absent = pipeline(json!([]));
        absent["spec"]["outputs"][0]["targetEndpoint"] =
            json!("urn:ngsi-ld:Endpoint:hel.fi:vehicles:gone");
        let absent = writer(&state, "helsinki", &absent).expect("writer");
        assert_eq!(absent.targets[0].requestable, None);
    }

    /// A source of another project that does not admit this one reads as a missing read with
    /// the request that asks for it, and an absent Endpoint reads the same (PL-90, T-5369).
    #[test]
    fn a_source_another_project_does_not_admit_names_the_read_request() {
        let state = world();
        for (name, audience) in [
            ("roads", json!(["helsinki"])),
            ("closed", json!(["vantaa"])),
        ] {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: "Endpoint".to_owned(),
                metadata: ObjectMeta::new(name, "espoo"),
                spec: json!({ "contextSpaceRef": "roads", "slug": name, "audience": "project-list",
                              "allowedProjects": audience }),
                status: None,
            });
        }
        let reading = |name: &str| {
            let mut manifest = pipeline(json!([]));
            manifest["spec"]["sources"] = json!([{
                "endpointRef": { "kind": "Endpoint", "name": name, "namespace": "espoo" },
                "query": { "type": "RoadSegment" }
            }]);
            let spec: PipelineSpec =
                serde_json::from_value(manifest["spec"].clone()).expect("spec");
            judge_sources(&state, "helsinki", &spec)
        };
        assert!(reading("roads").is_empty(), "an admitted source reads");
        let closed = reading("closed");
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].rule, "sourceEndpoint");
        assert_eq!(
            closed[0].access,
            Some(Access {
                project: "espoo".into(),
                name: "closed".into(),
                operations: "read".into(),
                entity_type: Some("RoadSegment".into()),
            })
        );
        let gone = reading("gone");
        assert_eq!(gone[0].reason, closed[0].reason.replace("closed", "gone"));

        // Every record of the check carries it.
        let mut manifest = pipeline(json!([]));
        manifest["spec"]["sources"] = json!([{ "endpointRef": { "kind": "Endpoint", "name": "closed", "namespace": "espoo" } }]);
        let checked = check(&state, &manifest, &[bus(json!(41))]);
        assert!(!checked.ok);
        assert!(checked.records[0]
            .findings
            .iter()
            .any(|f| f.rule == "sourceEndpoint"));
    }

    #[test]
    fn the_lifetime_shows_each_end_and_refuses_a_missing_or_past_one() {
        let state = world();
        let rolling = pipeline(
            json!([{ "type": "Bus", "entity": { "from": "observedOn", "plus": "PT10M" },
            "attributes": [{ "names": ["speed"], "rolling": "PT5M" }] }]),
        );
        let mut written = bus(json!(41));
        written["expiresAt"] = json!("2026-10-20T12:08:00Z");
        written["speed"]["expiresAt"] = json!("2026-10-20T12:05:00Z");
        let checked = check(&state, &rolling, &[written.clone(), bus(json!(40))]);
        assert!(checked.records[0].ok, "{:?}", checked.records[0].findings);
        let expires = checked.records[0].expires.clone().expect("the ends");
        assert_eq!(expires.entity.as_deref(), Some("2026-10-20T12:08:00Z"));
        assert_eq!(expires.attributes["speed"], "2026-10-20T12:05:00Z");

        let missing = &checked.records[1];
        assert!(missing.expires.is_none());
        let fields: Vec<(&str, Source)> = missing
            .findings
            .iter()
            .map(|f| (f.field.as_str(), f.source))
            .collect();
        assert_eq!(
            fields,
            [
                ("observedOn", Source::Lifetime),
                ("speed", Source::Lifetime)
            ]
        );

        let mut past = written;
        past["expiresAt"] = json!("2026-10-20T11:00:00Z");
        past["speed"]["expiresAt"] = json!("soon");
        let red = &check(&state, &rolling, &[past]).records[0];
        let reasons: Vec<(&str, &str)> = red
            .findings
            .iter()
            .filter(|f| f.source == Source::Lifetime)
            .map(|f| (f.field.as_str(), f.reason.as_str()))
            .collect();
        assert_eq!(reasons.len(), 2, "{reasons:?}");
        assert!(reasons.contains(&(
            "expiresAt",
            "lies in the past: the broker would hide it as soon as it is written"
        )));
        assert!(reasons.contains(&("speed.expiresAt", "is no RFC 3339 date-time")));
    }

    #[test]
    fn a_green_check_names_its_spec_and_sample_and_none_is_no_green() {
        let pipeline = pipeline(json!([]));
        let records = [bus(json!(41))];
        let checked = check(&world(), &pipeline, &records);
        assert!(checked.ok);
        let verdict = checked.verdict(&pipeline, &records);
        assert!(verdict.ok);
        assert_eq!(verdict.spec_digest, digest_of(&pipeline["spec"]));
        assert_eq!(verdict.sample_digest, digest_of(&json!(records)));
        let empty = check(&world(), &pipeline, &[]);
        assert!(
            !empty.verdict(&pipeline, &[]).ok,
            "no record checked is no green verdict"
        );
    }

    #[test]
    fn records_are_capped_by_count_and_size() {
        let state = world();
        let many = vec![bus(json!(1)); MAX_RECORDS + 1];
        assert!(matches!(
            check_records(&state, "helsinki", &pipeline(json!([])), &many, now()),
            Err(ApiError::BadRequest(_))
        ));
        let mut huge = bus(json!(1));
        huge["note"] = json!({ "type": "Property", "value": "x".repeat(MAX_RECORD_BYTES) });
        let checked = check(&state, &pipeline(json!([])), &[huge, json!("text")]);
        assert_eq!(checked.records[0].findings[0].rule, "size");
        assert_eq!(checked.records[1].findings[0].rule, "object");
        let findings = verdict_findings(&checked);
        assert_eq!(findings[0].path, "records[0]");
        assert!(findings[0].message.starts_with("model: "));
    }
}
