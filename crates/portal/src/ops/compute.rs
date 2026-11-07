//! The operations that compute an answer and write nothing: the catalogue search, a KPI, a pipeline
//! test and a model inferred from a sample (AG-58, AG-62, PL-38, DM-10).

use super::bounds;
use super::data_query;
use super::drafts::DraftRef;
use super::parse_input;
use super::pipeline_steps::candidate;
use super::proposals::record_verdict;
use super::OpError;
use super::Operation;
use super::OperationAnnotations;
use super::{Finding, Level, Verdict};
use crate::agents::kpi;
use crate::api::assistant;
use crate::api::pipeline_test;
use crate::change::Lane;
use crate::error::ApiError;
use crate::tools::model_tools;
use base64::Engine as _;
use jc_core::kinds::Verb;
use jcctl::pipeline_test::Sample;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSearchInput {
    /// Required: a search without keywords lists nothing (T-6274).
    pub q: String,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KpiComputeInput {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub entity_type: String,
    #[serde(default)]
    pub attribute: String,
    pub agg: kpi::Agg,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    /// The Endpoint read, as the caller; or `rows`.
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub rows: Option<Vec<Value>>,
}

impl KpiComputeInput {
    /// Exactly one source, and an attribute for a fold that reads one.
    fn check(&self) -> Result<(), OpError> {
        let refused = |path: &str, message: &str| {
            Err(OpError::InvalidInput {
                path: path.into(),
                message: message.into(),
            })
        };
        let endpoint = self
            .endpoint
            .as_deref()
            .is_some_and(|e| !e.trim().is_empty());
        match (endpoint, self.rows.is_some()) {
            (true, true) => refused("/rows", "send rows or name an endpoint, not both"),
            (false, false) => refused(
                "/endpoint",
                "name the Endpoint to read, e.g. city-counters, or send rows",
            ),
            _ if self.agg != kpi::Agg::Count && self.attribute.trim().is_empty() => refused(
                "/attribute",
                "name the attribute to fold, e.g. count; only agg count folds none",
            ),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineTestInput {
    #[serde(default)]
    pub pipeline: Option<Value>,
    pub sample: Sample,
    #[serde(default)]
    pub draft: Option<DraftRef>,
    /// Also what each step made of the sample (T-6082).
    #[serde(default)]
    pub steps: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInferInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub sample: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    /// An API or file address, fetched once on the project's runner as a data source's Check
    /// fetches it, and inferred from the records it answers (T-3250).
    #[serde(default)]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

fn catalog_search_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "q": {
                "type": "string",
                "description": "Required keywords in names, titles and descriptions, e.g. bike",
                "pattern": "\\S",
                "maxLength": QUERY
            },
            "scope": text("One kind only; Dataset is the open-data catalogue", TERM)
        },
        "required": ["q"],
        "additionalProperties": false
    })
}

fn catalog_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "q": { "type": "string" },
            "items": { "type": "array", "items": { "type": "object" } },
            "datasets": {
                "type": "array",
                "description": "Open datasets: title, url, publisher, formats, resources (format, url), endpoint slug",
                "items": { "type": "object" }
            },
            "unavailable": { "type": "array", "items": { "type": "string" } }
        }
    })
}

/// The `scope` that searches only the open-data catalogue.
const DATASET: &str = "Dataset";
/// The most catalogue datasets one search answers.
const DATASETS_MAX: usize = 10;

fn kpi_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "name": text("The indicator, its URN's {localId}", NAME),
            "title": text("The indicator's title as people read it", TITLE),
            "type": text("The entity type the value is computed over", TERM),
            "attribute": text("The attribute folded; ignored by count", TERM),
            "agg": {
                "type": "string",
                "description": "How the attribute's values are folded into one",
                "enum": ["avg", "sum", "count", "min", "max"]
            },
            "unit": text("A UN/CEFACT common code for the value", TERM),
            "q": text("An NGSI-LD q narrowing the entities read", QUERY),
            "endpoint": text("The Endpoint read as you; or rows", NAME),
            "rows": {
                "type": "array",
                "description": "Entities to fold; or endpoint",
                "items": { "type": "object", "maxProperties": RECORD_MEMBERS }
            }
        },
        "required": ["name", "type", "agg"],
        "additionalProperties": false
    })
}

fn pipeline_test_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "pipeline": manifest("The Pipeline manifest to run; or draft"),
            "sample": {
                "type": "object",
                "description": "The sample inline or by URL",
                "properties": {
                    "text": text(
                        "The sample inline",
                        jcctl::pipeline_test::MAX_SAMPLE_BYTES as u64
                    ),
                    "url": text("Or an http(s) URL the runner fetches", URL),
                    "format": {
                        "type": "string",
                        "description": "How the sample is split into messages",
                        "enum": ["csv", "json", "text"]
                    }
                },
                "additionalProperties": false
            },
            "draft": draft_ref(),
            "steps": super::pipeline_steps::steps_flag()
        },
        "required": ["sample"],
        "additionalProperties": false
    })
}

fn model_infer_input_schema() -> Value {
    use bounds::*;
    let sample_bytes = crate::tools::model_tools::MAX_SAMPLE_BYTES as u64;
    json!({
        "type": "object",
        "properties": {
            "name": text("The name to give the inferred model", NAME),
            "content": text("The sample file, base64 encoded", sample_bytes.div_ceil(3) * 4),
            "sample": text("The sample as plain text, instead of content", sample_bytes),
            "format": {
                "type": "string",
                "description": "The sample's format",
                "enum": ["csv", "xlsx", "json", "pdf"]
            },
            "url": {
                "type": "string",
                "description": "Instead of a sample: an http(s) URL answering JSON records",
                "pattern": "^https?://",
                "maxLength": 2048
            }
        },
        "additionalProperties": false
    })
}

fn pipeline_test_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "input": { "type": "object" },
            "mapping": { "type": "array" },
            "validation": { "type": "array" },
            "errors": { "type": "array" },
            "verdict": { "type": "object" }
        }
    })
}

/// The project's manifests matching `q`, and the open-data catalogue's datasets (T-6027).
async fn catalog_search(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: CatalogSearchInput,
) -> Result<Value, OpError> {
    let q = input.q.trim();
    if q.is_empty() {
        return Err(OpError::Api(ApiError::BadRequest(
            "q must not be empty".into(),
        )));
    }
    let scope = input
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(other) = scope.filter(|kind| *kind != DATASET && !assistant::KINDS.contains(kind)) {
        return Err(OpError::Api(ApiError::BadRequest(format!(
            "scope '{other}' is not one of {}, {DATASET}",
            assistant::KINDS.join(", ")
        ))));
    }
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    let mut answer = if scope == Some(DATASET) {
        json!({ "q": q, "items": [] })
    } else {
        serde_json::to_value(assistant::search(state, project, &effective, q, scope).await)?
    };
    // Published open data is public: every catalogue answers anonymously (EP-67).
    if scope.is_none_or(|kind| kind == DATASET) {
        let (datasets, unavailable) =
            crate::api::catalogue::datasets_matching(state, q, DATASETS_MAX).await;
        answer["datasets"] = json!(datasets);
        if !unavailable.is_empty() {
            answer["unavailable"] = json!(unavailable);
        }
    }
    Ok(answer)
}

/// The indicator's value over `rows`. A fold over no number is refused at `/rows`, so an
/// empty or numberless page never reads as a measured zero; `count` of no rows is a true 0.
fn kpi_value(rows: &[Value], params: &kpi::ComputeKpi) -> Result<f64, OpError> {
    kpi::compute(rows, &params.attribute, params.agg)
        .0
        .ok_or_else(|| OpError::InvalidInput {
            path: "/rows".into(),
            message: format!(
                "no {} carries a number in '{}' ({} read)",
                params.entity_type,
                params.attribute,
                rows.len()
            ),
        })
}

/// The most entities one indicator folds: past it the read is refused, so a value never stands
/// for a part of what `q` matches.
const KPI_MAX_ENTITIES: u64 = 10_000;
/// The entities one read of an indicator asks for (the Endpoint's own ceiling).
const KPI_PAGE: u32 = 1000;

/// The indicator's value over what `endpoint` lets the caller read, every page of it: the
/// gateway's total decides when the read is over, since it may answer a shorter page (T-5891).
async fn kpi_read(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    endpoint: &str,
    params: &kpi::ComputeKpi,
) -> Result<f64, OpError> {
    let query = |limit, cursor| data_query::DataQueryInput {
        endpoint: endpoint.to_owned(),
        entity_type: Some(params.entity_type.clone()),
        q: params.q.clone(),
        attrs: match params.agg {
            kpi::Agg::Count => Vec::new(),
            _ => vec![params.attribute.clone()],
        },
        format: Some("keyValues".to_owned()),
        count: true,
        limit: Some(limit),
        cursor,
        ..Default::default()
    };
    let total_of = |page: &Value| {
        page["total"].as_u64().ok_or_else(|| {
            OpError::from(ApiError::Unavailable(format!(
                "Endpoint {endpoint} did not count its entities; try again shortly"
            )))
        })
    };
    if params.agg == kpi::Agg::Count {
        let page = data_query::run(caller, state, project, query(0, None)).await?;
        return Ok(total_of(&page)? as f64);
    }
    let mut rows: Vec<Value> = Vec::new();
    loop {
        let cursor = (!rows.is_empty()).then(|| rows.len().to_string());
        let page = data_query::run(caller, state, project, query(KPI_PAGE, cursor)).await?;
        let total = total_of(&page)?;
        if total > KPI_MAX_ENTITIES {
            return Err(OpError::InvalidInput {
                path: "/q".into(),
                message: format!(
                    "{total} {} match, over the {KPI_MAX_ENTITIES} one indicator folds: narrow q",
                    params.entity_type
                ),
            });
        }
        let read = rows.len();
        rows.extend(page["entities"].as_array().into_iter().flatten().cloned());
        if rows.len() == read || rows.len() as u64 >= total {
            return kpi_value(&rows, params);
        }
    }
}

/// The indicator rendered over the rows sent or the Endpoint named; writes nothing.
async fn kpi_compute(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: KpiComputeInput,
) -> Result<Value, OpError> {
    input.check()?;
    let params = kpi::ComputeKpi {
        name: input.name,
        title: input.title,
        entity_type: input.entity_type,
        attribute: input.attribute,
        agg: input.agg,
        unit: input.unit,
        q: input.q,
        endpoint: input.endpoint,
    };
    let (value, read) = match params.endpoint.as_deref().filter(|e| !e.trim().is_empty()) {
        Some(endpoint) => {
            let value = kpi_read(caller, state, project, endpoint, &params).await?;
            let space = state
                .mirror
                .get(project, "Endpoint", endpoint)
                .and_then(|e| crate::api::assistant::ref_name(&e.spec["contextSpaceRef"]))
                .unwrap_or_else(|| project.to_owned());
            (value, Some((space, endpoint.to_owned())))
        }
        None => (kpi_value(&input.rows.unwrap_or_default(), &params)?, None),
    };
    let org_domain = assistant::org_domain(state, project);
    // The Endpoint read, or for rows the indicator space's, as the provenance's source.
    let (ep_slug, ep_name) = read
        .or_else(|| kpi::kpi_endpoint(state, project))
        .unwrap_or_else(|| (format!("{project}-kpi"), format!("{project}-kpi")));
    let now = chrono::Utc::now().to_rfc3339();
    let prov = kpi::Provenance {
        org_domain: &org_domain,
        project,
        endpoint_space: &ep_slug,
        endpoint_name: &ep_name,
        run_id: "ops",
        now: &now,
    };
    let entity = kpi::entity(&params, value, &prov).map_err(ApiError::BadRequest)?;
    Ok(entity.to_json())
}

/// This module's operations in the registry (`super::init_registry`).
pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_catalog_search",
            title: "Search Catalog",
            description: "Find spaces, endpoints, models and open datasets by keywords",
            input: catalog_search_input_schema,
            output: catalog_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<CatalogSearchInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(
                    async move { catalog_search(caller, state, project, parse_input(val)?).await },
                )
            },
        },
        Operation {
            name: "jc_kpi_compute",
            title: "Compute KPI",
            description:
                "Folds an attribute over an Endpoint or rows into a KeyPerformanceIndicator entity",
            input: kpi_input_schema,
            output: || json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Endpoint",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<KpiComputeInput>(val.clone())?.check(),
            run: |caller, state, project, val| {
                Box::pin(
                    async move { kpi_compute(caller, state, project, parse_input(val)?).await },
                )
            },
        },
        Operation {
            name: "jc_pipeline_test",
            title: "Pipeline Test",
            description:
                "Tests candidate pipeline mapping and validation on runner without writing",
            input: pipeline_test_input_schema,
            output: pipeline_test_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Green,
            validate: |val| {
                let input: PipelineTestInput = parse_input(val.clone())?;
                if input.pipeline.is_none() && input.draft.is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/pipeline".into(),
                        message: "either pipeline or draft is required".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(pipeline_test_run(caller, state, project, val))
            },
        },
        Operation {
            name: "jc_model_infer",
            title: "Infer Schema",
            description: "Infers a draft LinkML data model from sample data bytes or text",
            input: model_infer_input_schema,
            output: || json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ModelInferInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ModelInferInput = parse_input(val)?;
                    if let Some(url) = input.url {
                        return infer_from_url(
                            &caller.identity,
                            state,
                            project,
                            input.name.as_deref(),
                            &url,
                        )
                        .await;
                    }
                    let bytes = if let Some(b64) = input.content {
                        base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .map_err(|e| OpError::InvalidInput {
                                path: "/content".into(),
                                message: format!("invalid base64 content: {e}"),
                            })?
                    } else if let Some(text) = input.sample {
                        text.into_bytes()
                    } else {
                        return Err(OpError::InvalidInput {
                            path: "/content".into(),
                            message: "either content (base64) or sample (text) is required".into(),
                        });
                    };
                    let res = model_tools::infer_schema_from_bytes(
                        state,
                        input.name.as_deref(),
                        &bytes,
                        input.format.as_deref(),
                    )
                    .await?;
                    Ok(res)
                })
            },
        },
    ]
}

/// `jc_pipeline_test` (PL-43, T-4840): the harness on the runner, the static output types, each
/// record where its write is judged, and the verdict kept on the draft.
async fn pipeline_test_run(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    let input: PipelineTestInput = parse_input(val)?;
    let (pipeline, draft_ref) =
        candidate(caller, state, project, input.pipeline, input.draft.as_ref()).await?;

    let req = pipeline_test::TestRequest {
        pipeline: pipeline.clone(),
        sample: input.sample,
    };
    let trace = pipeline_test::execute_test_pipeline(&caller.identity, state, project, req).await?;

    // What the spec alone says against the target spaces' models (T-3223): a
    // type the model lacks is red before any record of the sample is read.
    let static_problems =
        serde_json::from_value::<jc_core::kinds::PipelineSpec>(pipeline["spec"].clone())
            .map(|spec| {
                crate::pipeline_validation::output_type_problems(
                    &state.model_schemas,
                    &state.mirror,
                    project,
                    &spec,
                )
            })
            .unwrap_or_default();
    // Each record where its write is judged (T-4840): the model, the Endpoint
    // and the lifetime.
    let checked = crate::record_check::check_trace(state, project, &pipeline, &trace.mapping)?;
    let ok = static_problems.is_empty()
        && trace.errors.is_empty()
        && !trace.validation.is_empty()
        && trace.validation.iter().all(|v| v.ok)
        && checked.ok;
    let mut findings: Vec<Finding> = static_problems
        .into_iter()
        .map(|problem| Finding {
            level: Level::Error,
            path: problem.path,
            message: problem.message,
        })
        .collect();
    for err in &trace.errors {
        findings.push(Finding {
            level: Level::Error,
            path: err.stage.clone(),
            message: err.message.clone(),
        });
    }
    for val in &trace.validation {
        if !val.ok {
            for problem in &val.problems {
                findings.push(Finding {
                    level: Level::Error,
                    path: format!("validation[{}]", val.index),
                    message: problem.clone(),
                });
            }
        }
    }
    findings.extend(crate::record_check::verdict_findings(&checked));
    let checked_records =
        &trace.mapping[..trace.mapping.len().min(crate::record_check::MAX_RECORDS)];
    let mut verdict = Verdict::new(
        ok,
        findings,
        Some(serde_json::to_value(&trace).unwrap_or_default()),
        &pipeline,
    );
    verdict.records = Some(Box::new(checked.verdict(&pipeline, checked_records)));
    if let Some(d) = &draft_ref {
        record_verdict(caller, state, project, d, &pipeline, &verdict).await;
    }
    let mut out = serde_json::to_value(&trace)?;
    out["check"] = serde_json::to_value(&checked)?;
    out["verdict"] = serde_json::to_value(&verdict)?;
    if input.steps {
        out["steps"] = json!(super::pipeline_steps::step_trace(&pipeline, &trace));
    }
    Ok(out)
}

/// A draft model from what an address answers (T-3250): one fetch on the project's runner, the
/// way a data source's Check fetches (MF-39) and under the same grant, never from the Portal's
/// own network; the records are found inside the envelope around them (AG-79) and inferred as
/// JSON.
async fn infer_from_url(
    identity: &crate::auth::session::Identity,
    state: &crate::state::AppState,
    project: &str,
    name: Option<&str>,
    url: &str,
) -> Result<Value, OpError> {
    let invalid = |message: String| OpError::InvalidInput {
        path: "/url".into(),
        message,
    };
    let parsed = url::Url::parse(url).map_err(|err| invalid(format!("not an address: {err}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid("give an http or https address".into()));
    }
    // The address reaches the runner and its logs: a credential in it would sit there in the clear
    // (T-3250). A source that needs one is a DataSource with a secretRef; the message never repeats
    // the address.
    if credential_in_address(&parsed) {
        return Err(invalid(
            "the address carries a user name, a password or a key; give one without, or connect it as a data source whose credential is a secret".into(),
        ));
    }
    crate::permissions::for_request(state, identity, project).check(
        "DataSource",
        Verb::Propose,
        None,
    )?;
    // The runner fetches it inside the cluster and nobody approved it (AG-65, JC1-27).
    super::pipeline_steps::sample_address(url, "/url").await?;
    let spec = json!({ "type": "http", "http": { "url": url } });
    let probe = pipeline_test::probe_source(state, project, &spec)
        .await
        .ok_or_else(|| invalid("give an address the runner can fetch".into()))?;
    // A feed that failed is the address's fault, said at the field; a probe that never ran (no
    // runner, a test already running) is not, and says so as such.
    if let Some(reason) = pipeline_test::feed_failed(&probe) {
        return Err(invalid(reason.to_owned()));
    }
    if let Some(reason) = probe.skipped {
        return Err(OpError::from(ApiError::Unavailable(reason)));
    }
    let whole = probe
        .sample
        .ok_or_else(|| invalid("the address answered no records".into()))?;
    let sample = crate::ops::feed_shape::records(&whole)
        .as_ref()
        .map_or(whole, crate::ops::feed_shape::Records::inference_sample)
        .to_string();
    Ok(model_tools::infer_schema_from_bytes(state, name, sample.as_bytes(), Some("json")).await?)
}

/// Whether an address carries a credential: a user name or password before the host, or a query
/// parameter named like one (`token`, `apiKey`, `key`, `sig`, …) or holding a credential-shaped
/// value.
/// Unlike [`crate::secret_scan::secret_text`], a named parameter is refused even empty or as a
/// `${VAR}`: nothing resolves a variable in an address sampled once.
fn credential_in_address(url: &url::Url) -> bool {
    !url.username().is_empty()
        || url.password().is_some()
        || url.query_pairs().any(|(name, value)| {
            crate::secret_scan::secret_param(&name)
                || crate::pipeline_outcomes::credential_shaped(&value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLUG: &str = "citycounterszzzzzzzzzzzzzz";

    /// A compute through a Portal whose mirror holds a public Endpoint on `gateway`.
    async fn compute(gateway: &wiremock::MockServer, args: Value) -> Result<Value, OpError> {
        let mut config = crate::config::Config::for_tests();
        config.gateway_url = Some(gateway.uri());
        let state = crate::state::AppState::new(config, None);
        for (kind, name, namespace, spec) in [
            (
                "Endpoint",
                "counters",
                "city",
                json!({ "slug": SLUG, "audience": "public" }),
            ),
            (
                "Organization",
                "city",
                crate::api::blueprints::ORG_NAMESPACE,
                json!({ "domain": "city.example" }),
            ),
        ] {
            state.mirror.upsert(crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            });
        }
        let caller = crate::ops::Caller {
            identity: crate::auth::session::Identity {
                client: None,
                subject: "sub".into(),
                username: "person".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            via: crate::ops::Via::Mcp,
            access: None,
            bearer: None,
        };
        let mut input = json!({ "name": "n", "type": "BikeCounter", "endpoint": SLUG });
        input
            .as_object_mut()
            .expect("an object")
            .extend(args.as_object().expect("an object").clone());
        kpi_compute(&caller, &state, "elsewhere", parse_input(input)?).await
    }

    /// A gateway holding `total` counters of count 1, 2, ... that answers at most `page` a read.
    async fn gateway(total: u64, page: u64) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        let answer = move |request: &wiremock::Request| {
            let number = |key: &str| {
                request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| v.parse::<u64>().ok())
            };
            let start = number("offset").unwrap_or(0);
            let end = total.min(start + page.min(number("limit").unwrap_or(0)));
            let rows: Vec<Value> = (start..end)
                .map(|i| json!({ "id": format!("urn:ngsi-ld:BikeCounter:{i}"), "count": i + 1 }))
                .collect();
            wiremock::ResponseTemplate::new(200)
                .insert_header("NGSILD-Results-Count", total.to_string().as_str())
                .set_body_json(rows)
        };
        wiremock::Mock::given(wiremock::matchers::path(format!(
            "/api/endpoint/{SLUG}/ngsi-ld/v1/entities"
        )))
        .respond_with(answer)
        .mount(&server)
        .await;
        server
    }

    fn value_of(answer: &Value) -> f64 {
        answer["currentValue"]["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("no value in {answer}"))
    }

    /// The value is the direct aggregation over every entity, also when the gateway answers
    /// shorter pages than asked (T-6484).
    #[tokio::test]
    async fn an_endpoint_read_folds_every_page_the_gateway_answers() {
        let server = gateway(2500, 700).await;
        let sum = compute(&server, json!({ "attribute": "count", "agg": "sum" }))
            .await
            .expect("a value");
        assert_eq!(value_of(&sum), (1..=2500u64).sum::<u64>() as f64);
        let reads = server.received_requests().await.expect("recorded");
        assert_eq!(reads.len(), 4, "700 at a time");
        let attrs = reads[0].url.query().unwrap_or("");
        assert!(
            attrs.contains("attrs=count") && attrs.contains("type=BikeCounter"),
            "{attrs}"
        );
    }

    #[tokio::test]
    async fn a_count_reads_the_total_alone() {
        let server = gateway(50_000, 1000).await;
        let count = compute(&server, json!({ "agg": "count" }))
            .await
            .expect("a value");
        assert_eq!(value_of(&count), 50_000.0);
        let reads = server.received_requests().await.expect("recorded");
        assert_eq!(reads.len(), 1);
        assert!(reads[0].url.query().unwrap_or("").contains("limit=0"));
    }

    /// A fold over more than one indicator reads is refused, never a value of the first pages.
    #[tokio::test]
    async fn a_fold_past_the_cap_is_refused_at_q() {
        let server = gateway(KPI_MAX_ENTITIES + 1, 1000).await;
        match compute(&server, json!({ "attribute": "count", "agg": "avg" })).await {
            Err(OpError::InvalidInput { path, message }) => {
                assert_eq!(path, "/q");
                assert!(message.contains("10001 BikeCounter match"), "{message}");
            }
            other => panic!("not refused: {other:?}"),
        }
    }

    /// An empty Endpoint folds no number, and `count` of it is a true 0.
    #[tokio::test]
    async fn an_empty_endpoint_is_no_measured_zero() {
        let server = gateway(0, 1000).await;
        let refused = compute(&server, json!({ "attribute": "count", "agg": "sum" })).await;
        assert!(
            matches!(&refused, Err(OpError::InvalidInput { message, .. }) if message.contains("(0 read)")),
            "{refused:?}"
        );
        let count = compute(&server, json!({ "agg": "count" }))
            .await
            .expect("a value");
        assert_eq!(value_of(&count), 0.0);
    }

    /// No source is never a fold of nothing: T-5962's call read 0 rows without saying why.
    #[test]
    fn a_call_names_one_source_and_a_fold_its_attribute() {
        let path = |args: Value| {
            let mut input = json!({ "name": "n", "type": "T", "agg": "sum", "attribute": "a" });
            input
                .as_object_mut()
                .expect("an object")
                .extend(args.as_object().expect("an object").clone());
            match parse_input::<KpiComputeInput>(input)
                .expect("parses")
                .check()
            {
                Err(OpError::InvalidInput { path, .. }) => Some(path),
                _ => None,
            }
        };
        assert_eq!(path(json!({})).as_deref(), Some("/endpoint"));
        assert_eq!(
            path(json!({ "endpoint": " " })).as_deref(),
            Some("/endpoint")
        );
        assert_eq!(
            path(json!({ "endpoint": "e", "rows": [] })).as_deref(),
            Some("/rows")
        );
        assert_eq!(
            path(json!({ "endpoint": "e", "attribute": "" })).as_deref(),
            Some("/attribute")
        );
        assert_eq!(path(json!({ "endpoint": "e" })), None);
        assert_eq!(path(json!({ "rows": [] })), None);
        assert_eq!(
            path(json!({ "rows": [], "agg": "count", "attribute": "" })),
            None
        );
    }

    fn params(agg: kpi::Agg) -> kpi::ComputeKpi {
        kpi::ComputeKpi {
            name: "n".into(),
            title: None,
            entity_type: "BikeCounter".into(),
            attribute: "count".into(),
            agg,
            unit: None,
            q: None,
            endpoint: None,
        }
    }

    #[test]
    fn a_fold_over_no_number_is_refused_at_rows_not_a_zero() {
        for rows in [vec![], vec![json!({"id": "c1", "count": "n/a"})]] {
            match kpi_value(&rows, &params(kpi::Agg::Sum)) {
                Err(OpError::InvalidInput { path, message }) => {
                    assert_eq!(path, "/rows");
                    assert!(
                        message.contains("BikeCounter") && message.contains("count"),
                        "{message}"
                    );
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_count_of_no_rows_and_a_true_zero_stay_values() {
        assert_eq!(kpi_value(&[], &params(kpi::Agg::Count)).ok(), Some(0.0));
        let zero = [json!({"id": "c1", "count": 0})];
        assert_eq!(kpi_value(&zero, &params(kpi::Agg::Sum)).ok(), Some(0.0));
    }
}
