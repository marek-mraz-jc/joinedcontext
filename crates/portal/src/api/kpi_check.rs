//! Checking a KPI over several sources as it is typed (API/01 §55, PL-102…PL-105, T-4935).
//!
//! The KPI editor sends its `v1alpha2` Pipeline draft on every pause in typing. The answer names
//! each field jc-core refuses and why, with the units the sources' space models give, and says
//! of each source whether the pipeline's account may read it yet (a source it cannot read is
//! asked for with PL-90 once the pipeline exists). `preview: true` reads each source Endpoint's
//! current page as the person, the filter preview's way (EP-85), and computes the indicator with
//! jc-core's reference evaluator. A `prev` term's history (PL-117) is read through the gateway
//! with the person's token exchanged for the Endpoint, since the broker's view preview serves
//! current pages only. Nothing is stored or written.

use std::collections::BTreeMap;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::{PipelineSource, PipelineSpec, Step, Verb};
use jc_core::kpi_join::{earlier_reads, Earlier, Expr, Input, KpiStep};
use jc_time::{SecondsFormat, TimestampTz};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::access_requests::{already_granted, Operations, TypeGrant};
use crate::auth::{CurrentUser, Identity, Session};
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::state::AppState;
use crate::store::ListOptions;

/// The rows one source's preview page holds: the broker's view preview bound.
const PAGE: u32 = 100;

/// How long the gateway gets to answer a history read: a read over a type is slower than the
/// gateway's usual 10 s on a cold broker (T-6466: 15 s on dev).
const HISTORY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

/// The body of `POST …/kpis/check`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KpiCheckRequest {
    /// The draft `joinedcontext.com/v1alpha2` Pipeline with one `kpi` step.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub pipeline: Value,
    /// Also compute the indicator on the sources' current pages.
    #[serde(default)]
    pub preview: bool,
}

/// One field jc-core refuses, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct KpiProblem {
    /// The manifest path at fault, `spec.steps.kpi.formula`.
    pub field: String,
    /// The rule it breaks, in words.
    pub reason: String,
}

/// What the editor knows of one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KpiSource {
    /// The source's `name`, the input's name in the formula.
    pub name: String,
    /// The source Endpoint.
    pub endpoint: String,
    /// Whether the pipeline's account reads the source's type through the Endpoint now: a
    /// Policy grants it, its history too for a `prev` term, or the Endpoint is public (PL-55,
    /// PL-90, PL-117).
    pub readable: bool,
}

/// One computed indicator.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct KpiValue {
    /// The entity or area key; absent for a join by time.
    pub key: Option<String>,
    /// The indicator's `{localId}`.
    pub name: String,
    pub value: f64,
}

impl From<jc_core::kpi_join::Computed> for KpiValue {
    fn from(c: jc_core::kpi_join::Computed) -> Self {
        Self {
            key: c.key,
            name: c.name,
            value: c.value,
        }
    }
}

/// The indicator on the sources' current pages.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KpiPreview {
    /// One value per key, one for a join by time.
    pub values: Vec<KpiValue>,
    /// The rows each source's page held, at most 100; a read back goes by `parking@P7D`.
    pub rows: BTreeMap<String, usize>,
    /// Why nothing is computed: a division by zero, an average over nothing, a source that did
    /// not answer (PL-104), no history for a `prev` term (PL-117).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// The check's answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct KpiChecked {
    /// Empty when the draft is a KPI admission takes.
    pub problems: Vec<KpiProblem>,
    pub sources: Vec<KpiSource>,
    /// Present when asked for and the draft has no problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<KpiPreview>,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/kpis/check",
    summary = "Check A KPI Over Several Sources",
    description = "Checks a v1alpha2 Pipeline draft with one kpi step by jc-core's admission rules (PL-102, PL-103), with each attribute's unit from its space's model, and says of each source whether the pipeline's account may read it. `preview: true` also computes the indicator on each source Endpoint's current page, read as the person (PL-105), and a `prev` term on its history (PL-117). Needs propose on Pipeline in the project, from a person's session; a source in a space the caller may not read is named as unknown. Changes no data.",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project name")),
    request_body(
        content = KpiCheckRequest,
        content_type = "application/json",
        example = json!({
            "pipeline": {
                "apiVersion": "joinedcontext.com/v1alpha2", "kind": "Pipeline",
                "metadata": { "name": "parking-per-stop" },
                "spec": {
                    "class": "scheduled", "schedule": "0 * * * *",
                    "sources": [
                        { "name": "parking", "endpointRef": { "kind": "Endpoint", "name": "helsinki-parking" },
                          "query": { "type": "ParkingSite", "attrs": ["occupiedSpotNumber", "district"] } },
                        { "name": "stops", "endpointRef": { "kind": "Endpoint", "name": "helsinki-transit" },
                          "query": { "type": "TransitStop", "attrs": ["district"] } }
                    ],
                    "steps": [{ "kind": "kpi", "kpi": {
                        "name": "parking-per-stop", "formula": "sum(parking.occupiedSpotNumber) / count(stops)",
                        "unitCode": "C62", "join": { "by": "area", "keys": { "parking": "district", "stops": "district" } } } }],
                    "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-writer" }]
                }
            },
            "preview": true
        })
    ),
    responses(
        (status = 200, description = "The problems, the sources and, when asked, the preview", body = KpiChecked),
        (status = 400, description = "The body is not a Pipeline of this project", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "No propose on Pipeline here, or a workload token", body = crate::error::ProblemDetails),
        (status = 404, description = "No such project", body = crate::error::ProblemDetails),
    )
)]
pub async fn check_kpi(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    body: Result<Json<KpiCheckRequest>, JsonRejection>,
) -> Result<Json<KpiChecked>, ApiError> {
    let identity = &user.0.identity;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let pipeline = admitted(&state, identity, &project, request.pipeline)?;
    let (mut checked, compiled) = check(&state, identity, &project, &pipeline);
    if let (true, Some((step, expr, inputs))) = (request.preview, compiled) {
        let reader = Reader::of(&state, &user.0, &project, &headers);
        checked.preview = Some(preview(&reader, &step, &expr, &inputs).await);
    }
    Ok(Json(checked))
}

/// The draft as this caller may check it: a person who may propose a Pipeline in the project,
/// and a Pipeline of this project.
fn admitted(
    state: &AppState,
    identity: &Identity,
    project: &str,
    mut pipeline: Value,
) -> Result<Value, ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    crate::permissions::for_request(state, identity, project).check(
        "Pipeline",
        Verb::Propose,
        Some(&pipeline),
    )?;
    // Decided (worker-t4935, owner may override): the preview reads as the person, so a
    // workload's token, whose reading is its ServiceAccount's grants, is refused here.
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "the KPI check is for a person, not a workload token".into(),
        ));
    }
    if pipeline["kind"] != "Pipeline" {
        return Err(ApiError::BadRequest(
            "pipeline.kind must be 'Pipeline'".into(),
        ));
    }
    match pipeline["metadata"]["namespace"].as_str() {
        Some(namespace) if namespace != project => Err(ApiError::BadRequest(format!(
            "pipeline.metadata.namespace '{namespace}' is not the project '{project}'"
        ))),
        _ => {
            pipeline["metadata"]["namespace"] = Value::from(project);
            Ok(pipeline)
        }
    }
}

type Compiled = (KpiStep, Expr, Vec<Input>);

/// The record check of a Pipeline with a kpi step (PL-85), `None` for any other. A kpi step has no
/// harness (`jc_pipeline_test` runs one Bento mapping over one sample, a kpi run folds a batch of
/// pages), so the indicator it writes is judged where a write is judged: model, target Endpoint
/// and lifetime. The formula itself was checked as it was typed.
/// ponytail: only a value's shape is judged, the REST door's check; a draft of the MCP door still
/// meets `jc_pipeline_test`'s refusal, upgrade path a kpi harness in jcctl.
pub fn records_verdict(
    state: &AppState,
    project: &str,
    manifest: &Value,
) -> Option<crate::ops::verdict::RecordsVerdict> {
    let spec: PipelineSpec = serde_json::from_value(manifest["spec"].clone()).ok()?;
    let target = spec.outputs().first()?.target_endpoint.to_string();
    let pipeline = manifest["metadata"]["name"].as_str()?;
    let record = indicator(kpi_step(&spec)?, &target, project, pipeline)?;
    let records = [record];
    let checked = crate::record_check::check_records(
        state,
        project,
        manifest,
        &records,
        jc_time::TimestampTz::now(),
    )
    .ok()?;
    Some(checked.verdict(manifest, &records))
}

/// The indicator a run writes, as jc-core's mapping writes it (`kpi_join::bloblang`), `value` 0.
fn indicator(step: &KpiStep, target: &str, project: &str, pipeline: &str) -> Option<Value> {
    let (scope, _) = target
        .strip_prefix("urn:ngsi-ld:Endpoint:")?
        .rsplit_once(':')?;
    let (domain, _) = scope.split_once(':')?;
    let now = TimestampTz::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    Some(serde_json::json!({
        "id": format!("urn:ngsi-ld:KeyPerformanceIndicator:{scope}:{}", step.name),
        "type": "KeyPerformanceIndicator",
        "name": { "type": "Property", "value": step.name },
        "calculationFormula": { "type": "Property", "value": step.formula },
        "currentValue": { "type": "Property", "value": 0, "unitCode": step.unit_code, "observedAt": now },
        "calculationPeriod": { "type": "Property", "value": { "start": now, "end": now } },
        "updatedAt": { "type": "Property", "value": { "@type": "DateTime", "@value": now } },
        "derivedFrom": { "type": "Property", "value": [target] },
        "computedBy": { "type": "Property", "value": format!("urn:ngsi-ld:Pipeline:{domain}:{project}:{pipeline}") },
    }))
}

/// The problems and the sources of `pipeline`, and the step compiled when it has no problem.
fn check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    pipeline: &Value,
) -> (KpiChecked, Option<Compiled>) {
    let mut checked = KpiChecked {
        problems: Vec::new(),
        sources: Vec::new(),
        preview: None,
    };
    let spec = match parsed(pipeline) {
        Ok(spec) => spec,
        Err(problem) => {
            checked.problems.push(problem);
            return (checked, None);
        }
    };
    let effective = crate::permissions::for_request(state, identity, project);
    let account = format!(
        "{project}/pl-{}",
        pipeline["metadata"]["name"].as_str().unwrap_or_default()
    );
    let policies = policies(state, project);
    let reads = kpi_step(&spec)
        .and_then(|step| jc_core::kpi_join::parse(&step.formula).ok())
        .map(|expr| earlier_reads(&expr))
        .unwrap_or_default();
    let mut inputs = match spec.kpi_inputs() {
        Ok(inputs) => inputs,
        Err(err) => {
            checked.problems.push(problem_of(&err));
            return (checked, None);
        }
    };
    for (at, (input, source)) in inputs.iter_mut().zip(&spec.sources).enumerate() {
        match known_endpoint(state, &effective, project, source) {
            Some((name, endpoint)) => {
                input.endpoint.clone_from(&name);
                input.units = units(state, project, &endpoint, &input.entity_type);
                checked
                    .problems
                    .extend(
                        unknown_attribute(state, project, &endpoint, input).map(|reason| {
                            KpiProblem {
                                field: format!("spec.sources[{at}].query.attrs"),
                                reason,
                            }
                        }),
                    );
                checked.sources.push(KpiSource {
                    name: input.name.clone(),
                    readable: endpoint["spec"]["audience"] == "public"
                        || already_granted(
                            &policies,
                            &account,
                            &[TypeGrant {
                                entity_type: input.entity_type.clone(),
                                attrs: Vec::new(),
                            }],
                            if reads.iter().any(|read| read.input == input.name) {
                                Operations::History
                            } else {
                                Operations::Read
                            },
                        ),
                    endpoint: name,
                });
            }
            None => checked.problems.push(KpiProblem {
                field: format!("spec.sources[{at}].endpointRef"),
                reason: format!(
                    "no Endpoint '{}' in project '{project}' that you may read",
                    source
                        .endpoint_ref
                        .as_ref()
                        .map(|r| r.name())
                        .unwrap_or_default()
                ),
            }),
        }
    }
    let Some(step) = kpi_step(&spec) else {
        checked.problems.push(KpiProblem {
            field: "spec.steps".into(),
            reason: "a KPI pipeline has one step of kind kpi (PL-102)".into(),
        });
        return (checked, None);
    };
    match step.validate(&inputs) {
        Ok(expr) if checked.problems.is_empty() => (checked, Some((step.clone(), expr, inputs))),
        Ok(_) => (checked, None),
        Err(err) => {
            checked.problems.push(problem_of(&err));
            (checked, None)
        }
    }
}

/// The draft's spec, after the manifest rules every Pipeline is held to.
fn parsed(pipeline: &Value) -> Result<PipelineSpec, KpiProblem> {
    // A JSON value always prints, and JSON is YAML; jc-core types the Pipeline kind, so the
    // manifest rules always run.
    let text = pipeline.to_string();
    jc_core::registry::validate_yaml("Pipeline", &text)
        .unwrap_or(Ok(()))
        .map_err(|e| problem_of(&e))?;
    serde_json::from_value(pipeline["spec"].clone()).map_err(spec_refused)
}

/// A spec that does not read as a Pipeline's, as the problem of its `spec` field.
fn spec_refused(err: serde_json::Error) -> KpiProblem {
    KpiProblem {
        field: "spec".into(),
        reason: err.to_string(),
    }
}

/// The field a jc-core refusal names, and its reason.
fn problem_of(err: &jc_core::Error) -> KpiProblem {
    match err {
        jc_core::Error::Invalid { field, reason } => KpiProblem {
            field: field.clone(),
            reason: reason.clone(),
        },
        jc_core::Error::Name { field, reason, .. } => KpiProblem {
            field: (*field).to_owned(),
            reason: (*reason).to_owned(),
        },
        other => KpiProblem {
            field: "spec".into(),
            reason: other.to_string(),
        },
    }
}

fn kpi_step(spec: &PipelineSpec) -> Option<&KpiStep> {
    spec.steps.iter().find_map(|step| match step {
        Step::Compute(compute) => compute.kpi.as_deref(),
        Step::Processor(_) => None,
    })
}

/// The source's Endpoint, by name and manifest, when it is one of this project the caller may
/// read; any other is unknown, so a draft learns nothing of spaces the caller does not see.
fn known_endpoint(
    state: &AppState,
    effective: &crate::permissions::Effective,
    project: &str,
    source: &PipelineSource,
) -> Option<(String, Value)> {
    let reference = source.endpoint_ref.as_ref()?;
    if reference.namespace().is_some_and(|ns| ns != project) || !is_dns1123(reference.name()) {
        return None;
    }
    let endpoint = state.mirror.get(project, "Endpoint", reference.name())?;
    let endpoint = serde_json::to_value(endpoint).ok()?;
    let space = crate::permissions::space_ref(&endpoint)?;
    (effective.may_read_project() && effective.may_read_in("ContextSpace", Some(&space)))
        .then(|| (reference.name().to_owned(), endpoint))
}

/// `entity_type` as the Endpoint's space model describes it, when the model is known.
fn class_of(
    state: &AppState,
    project: &str,
    endpoint: &Value,
    entity_type: &str,
) -> Option<crate::pipeline_validation::ClassAttributes> {
    crate::permissions::space_ref(endpoint)
        .and_then(|space| state.model_schemas.get(project, &space))
        .and_then(|schema| schema.attributes(entity_type))
}

/// The UN/CEFACT code of each attribute of `entity_type` the Endpoint's space model gives one.
/// A model names most units in UCUM (`ug/m3`), and the indicator's `unitCode` is UN/CEFACT.
fn units(
    state: &AppState,
    project: &str,
    endpoint: &Value,
    entity_type: &str,
) -> BTreeMap<String, String> {
    class_of(state, project, endpoint, entity_type)
        .map(|class| {
            class
                .attributes
                .into_iter()
                .filter_map(|a| {
                    let unit = a.unit?;
                    let code = unit.code.or_else(|| cefact(unit.ucum.as_deref()?))?;
                    Some((a.name, code))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The current UN/CEFACT code of a UCUM unit.
fn cefact(ucum: &str) -> Option<String> {
    jc_core::units::all()
        .iter()
        .find(|unit| unit.ucum == Some(ucum) && !unit.deprecated)
        .map(|unit| unit.code.to_owned())
}

/// The first attribute a source reads that its type's model does not hold: the broker would
/// answer no rows for it, and a sum over nothing reads as a real 0.
fn unknown_attribute(
    state: &AppState,
    project: &str,
    endpoint: &Value,
    input: &Input,
) -> Option<String> {
    let class = class_of(state, project, endpoint, &input.entity_type)?;
    let known = |name: &&String| class.attributes.iter().any(|a| &a.name == *name);
    let unknown = input.attrs.iter().find(|name| !known(name))?;
    Some(format!(
        "`{unknown}` is not an attribute of {} in this Endpoint's model; pick one from the list",
        input.entity_type
    ))
}

fn policies(state: &AppState, project: &str) -> Vec<Value> {
    state
        .mirror
        .list(project, "Policy", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|policy| serde_json::to_value(policy).ok())
        .collect()
}

/// Who a preview reads as, and where.
struct Reader<'a> {
    state: &'a AppState,
    session: &'a Session,
    project: &'a str,
    /// The person's own access token, when this session carries one.
    token: Option<&'a str>,
}

impl<'a> Reader<'a> {
    /// The caller of this request.
    fn of(
        state: &'a AppState,
        session: &'a Session,
        project: &'a str,
        headers: &'a HeaderMap,
    ) -> Self {
        Self {
            state,
            session,
            project,
            token: crate::agents::identity::persons_token(headers, state.config.trust_edge_token),
        }
    }
}

/// Each source's current page read as the person, each read back from its history, and the
/// indicator computed on them.
async fn preview(reader: &Reader<'_>, step: &KpiStep, expr: &Expr, inputs: &[Input]) -> KpiPreview {
    let mut pages = BTreeMap::new();
    let mut problem = None;
    for input in inputs {
        match page(reader, input, step).await {
            Ok(rows) => {
                pages.insert(input.name.clone(), rows);
            }
            Err(reason) => {
                problem.get_or_insert(format!("input {}: {reason}", input.name));
            }
        }
    }
    let reads = earlier_reads(expr);
    for read in &reads {
        let found = inputs.iter().find(|input| input.name == read.input);
        let Some(input) = found.filter(|_| problem.is_none()) else {
            continue;
        };
        match history(reader, input, read).await {
            Ok(rows) if rows.is_empty() => {
                problem = Some(format!(
                    "input {}: {}",
                    read.input,
                    jc_core::kpi_join::no_history(&read.period)
                ));
            }
            Ok(rows) => {
                pages.insert(read.page(), rows);
            }
            Err(reason) => problem = Some(format!("input {}: {reason}", read.input)),
        }
    }
    let rows = pages
        .iter()
        .map(|(name, rows)| (name.clone(), rows.len()))
        .collect();
    // A run over an empty source writes nothing (render.rs kpi_source): no 0 that reads as a value.
    problem = problem.or_else(|| {
        pages
            .iter()
            .find(|(_, rows)| rows.is_empty())
            .map(|(name, _)| empty_source(name))
    });
    let values = match problem {
        Some(_) => Vec::new(),
        None => match jc_core::kpi_join::evaluate(step, expr, &pages) {
            Ok(values) => values.into_iter().map(KpiValue::from).collect(),
            Err(reason) => {
                problem = Some(reason);
                Vec::new()
            }
        },
    };
    KpiPreview {
        values,
        rows,
        problem,
    }
}

/// Why a source with no rows gives no value, and what to do about it.
fn empty_source(name: &str) -> String {
    format!(
        "input {name}: {}",
        crate::reconciler::streams::EMPTY_KPI_SOURCE
    )
}

/// One source's page through its Endpoint's compiled view, as the person (or the anonymous
/// caller on a public Endpoint) reads it.
async fn page(reader: &Reader<'_>, input: &Input, step: &KpiStep) -> Result<Vec<Value>, String> {
    let Reader {
        state,
        session,
        project,
        ..
    } = *reader;
    let views = state
        .views
        .as_ref()
        .ok_or("this Portal has no broker address or admin token")?;
    let (id, view) = views
        .saved_view(project, &input.endpoint)?
        .ok_or("the Endpoint's view is not compiled yet; try again shortly")?;
    let subject =
        crate::api::endpoint_preview::subject(state, views, session, project, &input.endpoint)
            .map_err(|refused| refused.to_string())?;
    let (status, answer) = views
        .preview(&id, &view, &query_of(input, step), &subject)
        .await?;
    match status {
        200 => Ok(answer["entities"].as_array().cloned().unwrap_or_default()),
        _ => Err(answer["detail"]
            .as_str()
            .map_or_else(|| format!("the broker answered {status}"), str::to_owned)),
    }
}

/// One page of a source one period back (PL-117), through its Endpoint's Temporal API: anonymous
/// on a public Endpoint, else with the person's token exchanged for this Endpoint, so the
/// gateway decides on their grants as it does for every read of theirs.
async fn history(reader: &Reader<'_>, input: &Input, read: &Earlier) -> Result<Vec<Value>, String> {
    let state = reader.state;
    let base = state
        .config
        .gateway_url
        .as_deref()
        .ok_or("this Portal has no gateway address, so it cannot read history")?;
    let endpoint = state
        .mirror
        .get(reader.project, "Endpoint", &input.endpoint)
        .ok_or("the Endpoint is gone")?;
    let slug = endpoint.spec["slug"]
        .as_str()
        .ok_or("the Endpoint has no slug yet; try again shortly")?;
    let bearer = if endpoint.spec["audience"].as_str() == Some("public") {
        None
    } else {
        let token = reader.token.ok_or(
            "history is read with your own access token, which this session does not carry; \
             sign in through the platform's address",
        )?;
        let oidc = crate::auth::realms::exchanger(state, token)
            .await
            .map_err(|err| err.to_string())?;
        let exchanged = oidc.exchange_for_endpoint(token, slug).await;
        Some(exchanged.map_err(|err| err.to_string())?)
    };
    let answer = crate::ops::data_query::gateway_read(
        base,
        slug,
        "ngsi-ld/v1/temporal/entities",
        &history_query(input, read),
        bearer,
        HISTORY_TIMEOUT,
    )
    .await
    .map_err(|err| {
        tracing::warn!(error = %err.without_url(), endpoint = slug, "the gateway did not answer a KPI history read");
        "the gateway did not answer; try again shortly".to_owned()
    })?;
    match answer.status().as_u16() {
        200 => {
            let body: Result<Value, _> =
                crate::capped::json(answer, crate::capped::ANSWER_BYTES).await;
            Ok(body
                .ok()
                .and_then(|answer| answer.as_array().cloned())
                .unwrap_or_default())
        }
        401 | 403 | 404 => Err("you may not read its history through this Endpoint".to_owned()),
        status => Err(format!("the gateway answered {status} reading its history")),
    }
}

/// The history read of a source: its type and `q`, the attributes the `prev` terms fold, each
/// row's newest reading at the time one period back, one page.
fn history_query(input: &Input, read: &Earlier) -> Vec<(&'static str, String)> {
    let at = TimestampTz::now()
        - jiff::SignedDuration::from_secs(i64::from(jc_core::kpi_join::period_seconds(
            &read.period,
        )));
    let mut query = vec![
        ("type", input.entity_type.clone()),
        ("attrs", read.attrs.join(",")),
        ("timerel", "before".to_owned()),
        ("timeAt", at.to_rfc3339_opts(SecondsFormat::Secs, true)),
        ("lastN", "1".to_owned()),
        ("limit", PAGE.to_string()),
    ];
    if let Some(q) = &input.q {
        query.push(("q", q.clone()));
    }
    query
}

/// The source's query: its type, `q`, the attributes it reads and its join key, one page.
fn query_of(input: &Input, step: &KpiStep) -> String {
    let mut attrs = input.attrs.clone();
    if let Some(key) = step.join.keys.get(&input.name).filter(|k| *k != "id") {
        if !attrs.contains(key) {
            attrs.push(key.clone());
        }
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("type", &input.entity_type);
    if !attrs.is_empty() {
        query.append_pair("attrs", &attrs.join(","));
    }
    if let Some(q) = &input.q {
        query.append_pair("q", q);
    }
    query.append_pair("limit", &PAGE.to_string());
    query.append_pair("offset", "0");
    query.finish()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/kpis/check", post(check_kpi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reads_the_attributes_the_join_key_and_the_filter() {
        let input = Input {
            name: "parking".into(),
            entity_type: "ParkingSite".into(),
            attrs: vec!["occupied".into()],
            q: Some("occupied>0".into()),
            ..Default::default()
        };
        let mut step: KpiStep = serde_json::from_value(serde_json::json!({
            "name": "n", "formula": "sum(parking.occupied)", "unitCode": "C62",
            "join": { "by": "area", "keys": { "parking": "district" } }
        }))
        .expect("a step");
        assert_eq!(
            query_of(&input, &step),
            "type=ParkingSite&attrs=occupied%2Cdistrict&q=occupied%3E0&limit=100&offset=0"
        );
        step.join.keys.insert("parking".into(), "id".into());
        assert_eq!(
            query_of(&input, &step),
            "type=ParkingSite&attrs=occupied&q=occupied%3E0&limit=100&offset=0"
        );
    }

    #[test]
    fn a_history_page_reads_the_folded_attributes_at_the_time_one_period_back() {
        let input = Input {
            name: "parking".into(),
            entity_type: "ParkingSite".into(),
            attrs: vec!["occupied".into(), "capacity".into()],
            q: Some("occupied>0".into()),
            ..Default::default()
        };
        let read = Earlier {
            input: "parking".into(),
            period: "P7D".into(),
            attrs: vec!["occupied".into()],
        };
        let query: BTreeMap<_, _> = history_query(&input, &read).into_iter().collect();
        assert_eq!(query["attrs"], "occupied");
        assert_eq!(query["timerel"], "before");
        assert_eq!(query["lastN"], "1");
        assert_eq!(query["q"], "occupied>0");
        let at = jc_time::parse_rfc3339(&query["timeAt"]).expect("a time");
        let back = (TimestampTz::now() - TimestampTz(at)).as_secs();
        assert!((604_790..=604_810).contains(&back), "{back}");
    }

    fn session(state: &AppState, client: Option<&str>) -> Session {
        Session {
            identity: Identity {
                client: client.map(str::to_owned),
                subject: "sub-ana".into(),
                username: "ana".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: vec![state.config.bootstrap_admins.clone()],
                second_factor: false,
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            signed_in_at: None,
            stepped_up_at: None,
            id_token: String::new(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        }
    }

    /// A workload's token is refused even when its groups would admit the draft.
    #[test]
    fn a_workload_token_is_refused_the_check() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let draft = serde_json::json!({ "kind": "Pipeline", "metadata": { "name": "kpi" } });
        let workload = session(&state, Some("claude-code"));
        let refused = admitted(&state, &workload.identity, "city", draft.clone());
        assert!(
            matches!(&refused, Err(ApiError::Denied(reason)) if reason.contains("not a workload token")),
            "{refused:?}"
        );
        let person = session(&state, None);
        let admitted = admitted(&state, &person.identity, "city", draft).expect("a person's");
        assert_eq!(admitted["metadata"]["namespace"], "city");
    }

    /// A source reading no attributes asks only for its type, and no join key is added.
    #[test]
    fn a_source_of_no_attributes_reads_its_type_alone() {
        let input = Input {
            name: "stops".into(),
            entity_type: "TransitStop".into(),
            ..Default::default()
        };
        let step: KpiStep = serde_json::from_value(serde_json::json!({
            "name": "n", "formula": "count(stops)", "unitCode": "C62", "join": { "by": "time" }
        }))
        .expect("a step");
        assert_eq!(
            query_of(&input, &step),
            "type=TransitStop&limit=100&offset=0"
        );
    }

    /// The kpi step is found after a processor step.
    #[test]
    fn the_kpi_step_is_found_after_a_processor() {
        let spec: PipelineSpec = serde_json::from_value(serde_json::json!({
            "class": "scheduled", "schedule": "0 * * * *",
            "sources": [{ "name": "stops", "endpointRef": { "kind": "Endpoint", "name": "stops" },
                          "query": { "type": "TransitStop" } }],
            "steps": [
                { "processor": { "log": {} } },
                { "kind": "kpi", "kpi": { "name": "stops-now", "formula": "count(stops)",
                  "unitCode": "C62", "join": { "by": "time" } } }
            ],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-kpi:kpi-writer" }]
        }))
        .expect("a spec");
        assert_eq!(
            kpi_step(&spec).map(|step| step.name.as_str()),
            Some("stops-now")
        );
        let only: PipelineSpec = serde_json::from_value(serde_json::json!({
            "class": "scheduled", "schedule": "0 * * * *",
            "steps": [{ "processor": { "log": {} } }],
            "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:city-kpi:kpi-writer" }]
        }))
        .expect("a spec");
        assert!(kpi_step(&only).is_none());
    }

    const SLUG: &str = "p4y7pq2mzt6vhx3nbwrs5cjdaf";

    /// A realm whose token endpoint answers `status` to the exchange.
    async fn realm(status: u16) -> (wiremock::MockServer, crate::auth::oidc::OidcClient) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let realm = MockServer::start().await;
        let issuer = format!("{}/realms/hel", realm.uri());
        Mock::given(method("GET"))
            .and(path("/realms/hel/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
                "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
                "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["ES256"]
            })))
            .mount(&realm)
            .await;
        Mock::given(method("GET"))
            .and(path("/realms/hel/protocol/openid-connect/certs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "keys": [] })),
            )
            .mount(&realm)
            .await;
        Mock::given(method("POST"))
            .and(path("/realms/hel/protocol/openid-connect/token"))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(serde_json::json!({
                    "access_token": "an-exchanged-token", "token_type": "Bearer", "expires_in": 300
                })),
            )
            .mount(&realm)
            .await;
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::config::OidcConfig {
                issuer: issuer.parse().expect("an issuer"),
                client_id: "portal-api".into(),
                auth: crate::config::ClientAuth::Secret("test-secret".into()),
                extra_ca_pem: None,
                admin_url: None,
            },
            "https://portal.test/api/v1/auth/callback",
        )
        .await
        .expect("discovery");
        (realm, client)
    }

    /// A state reading history through `gateway`, its Endpoint `parking` of `audience`.
    fn reading(
        gateway: &str,
        oidc: Option<crate::auth::oidc::OidcClient>,
        audience: &str,
    ) -> AppState {
        let mut config = crate::config::Config::for_tests();
        config.gateway_url = Some(gateway.to_owned());
        let state = AppState::new(config, oidc);
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: crate::resource::ObjectMeta::new("parking", "city"),
            spec: serde_json::json!({ "contextSpaceRef": "mobility", "slug": SLUG, "audience": audience }),
            status: None,
        });
        state
    }

    fn parking() -> (Input, Earlier) {
        let input = Input {
            name: "parking".into(),
            entity_type: "ParkingSite".into(),
            attrs: vec!["occupied".into()],
            endpoint: "parking".into(),
            ..Default::default()
        };
        let read = Earlier {
            input: "parking".into(),
            period: "P7D".into(),
            attrs: vec!["occupied".into()],
        };
        (input, read)
    }

    async fn history_of(state: &AppState, token: Option<&str>) -> Result<Vec<Value>, String> {
        let session = session(state, None);
        let reader = Reader {
            state,
            session: &session,
            project: "city",
            token,
        };
        let (input, read) = parking();
        history(&reader, &input, &read).await
    }

    /// PL-117: on an Endpoint that is not public the person's token is exchanged for this
    /// Endpoint and the gateway's rows are the read back; a refused exchange reads nothing.
    #[tokio::test]
    async fn a_persons_history_is_read_with_their_exchanged_token() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/endpoint/{SLUG}/ngsi-ld/v1/temporal/entities")))
            .and(header("authorization", "Bearer an-exchanged-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": "p1", "type": "ParkingSite", "occupied": [{ "type": "Property", "value": 3 }] }
            ])))
            .mount(&gateway)
            .await;
        let (_granting, client) = realm(200).await;
        let state = reading(&gateway.uri(), Some(client), "organization");
        let rows = history_of(&state, Some("a-persons-token"))
            .await
            .expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "p1");

        let (_refusing, client) = realm(400).await;
        let refused = reading(&gateway.uri(), Some(client), "organization");
        let reason = history_of(&refused, Some("a-persons-token"))
            .await
            .expect_err("a refused exchange");
        assert!(!reason.is_empty());

        let no_client = reading(&gateway.uri(), None, "organization");
        let reason = history_of(&no_client, Some("a-persons-token"))
            .await
            .expect_err("no client to exchange at");
        assert!(reason.contains("no Keycloak client"), "{reason}");
        assert_eq!(
            gateway.received_requests().await.unwrap_or_default().len(),
            1
        );
    }

    /// A gateway that fails or does not answer gives no history, in words.
    #[tokio::test]
    async fn a_failing_or_silent_gateway_reads_no_history() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let gateway = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&gateway)
            .await;
        let failing = reading(&gateway.uri(), None, "public");
        assert_eq!(
            history_of(&failing, None).await,
            Err("the gateway answered 500 reading its history".to_owned())
        );
        let silent = reading(
            &format!("http://{}:9", std::net::Ipv4Addr::LOCALHOST),
            None,
            "public",
        );
        assert_eq!(
            history_of(&silent, None).await,
            Err("the gateway did not answer; try again shortly".to_owned())
        );
    }

    #[test]
    fn a_refusal_keeps_the_field_jc_core_names() {
        let err = jc_core::Error::Invalid {
            field: "spec.steps.kpi.formula".into(),
            reason: "unknown function".into(),
        };
        assert_eq!(
            problem_of(&err),
            KpiProblem {
                field: "spec.steps.kpi.formula".into(),
                reason: "unknown function".into(),
            }
        );
        assert_eq!(
            problem_of(&jc_core::Error::ApiVersion("v9".into())).field,
            "spec"
        );
    }

    /// T-7931: a spec that does not read is the problem of the `spec` field, in the parser's
    /// words.
    #[test]
    fn a_spec_that_does_not_read_is_the_spec_fields_problem() {
        let err = serde_json::from_value::<PipelineSpec>(serde_json::json!("a string"))
            .expect_err("no spec");
        let words = err.to_string();
        let problem = spec_refused(err);
        assert_eq!(problem.field, "spec");
        assert_eq!(problem.reason, words);
    }
}
