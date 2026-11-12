//! Model Tools inside `jc`: `POST /generate` compiles a LinkML source into the broker schema
//! and the JSON-LD `@context`, the two artifacts the Portal's reconciler needs to write a Context
//! Space's schema to Antares (`PUT /q/schemas/{tenant}`, T-3703). A port of the Python service's
//! `/generate` (platform `tools/model-tools/src/service.py`, `gen_broker_schema.py`,
//! `gen_context.py` and the loader checks of `common.py`), plus the JSON Schema, the Markdown
//! page, the example and `jc-types.ts` a save commits (`save.rs`). The Smart Data Models
//! catalogue and its imports are the Portal's, read from the mirror the image carries
//! (`JC_PORTAL_SDM_MIRROR_DIR`, T-6353): `/catalog` and `/import-sdm` here fetch nothing and
//! only say a model is not in that mirror. Inference is not here and answers 404.
//!
//! The module has no public mount: it listens on a loopback port of its own, and `jc` hands
//! the Portal that address (`JC_PORTAL_MODEL_TOOLS_URL`). Nothing it reads is fetched: a model
//! imports `linkml:types`, the shipped `ngsi-ld-core` and the platform models the request
//! carries, and any other import is an `errors` answer. The body cap, the nesting depth and the
//! node count bound the work of one request (a blocking task cannot be stopped); the timeout and
//! the four work slots come second.

mod broker;
mod checks;
mod context;
mod fields;
mod save;
mod schema;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use checks::LoadError;
use schema::{Imports, Limits, View};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Largest body read, the Portal's own cap (DM-18, service.py `MAX_BODY_BYTES`).
pub const MAX_BODY_BYTES: usize = 512 * 1024;
/// Platform models one request may import, transitively.
pub const MAX_IMPORTS: usize = 32;
/// What one request may parse, source and imports together.
const LIMITS: Limits = Limits {
    max_depth: 32,
    max_elements: 200_000,
};
/// Compilations at once; past this a request waits for a slot (service.py `WORK_SLOTS`).
const WORK_SLOTS: usize = 4;
/// How long a request waits for its compilation. The task itself runs on: its limits bound it.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(20);

/// What `/import-sdm` answers: jc fetches no catalogue model, the Portal's mirror holds them.
const NOT_MIRRORED: &str = "is not in the Smart Data Models mirror this jc image carries, and jc \
                            fetches no model from the catalogue; jc_model_sdm_search lists the \
                            models it holds";

/// The version that rendered an artifact set (DM-19).
#[must_use]
pub fn generator_version() -> String {
    format!("jc-model-tools-{}", env!("CARGO_PKG_VERSION"))
}

#[derive(Clone)]
struct State {
    slots: Arc<Semaphore>,
}

/// The module's router: `/healthz`, `/generate`, and a JSON 404 for anything else.
pub fn router() -> Router {
    let state = State {
        slots: Arc::new(Semaphore::new(WORK_SLOTS)),
    };
    Router::new().fallback(move |req: Request| dispatch(state.clone(), req))
}

/// Serves [`router`] on a loopback port of its own until `shutdown`; answers its address.
pub async fn spawn(
    shutdown: CancellationToken,
    tasks: &TaskTracker,
) -> Result<SocketAddr, std::io::Error> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let app = router();
    tasks.spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        {
            tracing::error!(error = %e, "model-tools: listener stopped");
        }
    });
    Ok(addr)
}

fn answer(status: StatusCode, payload: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        payload.to_string(),
    )
        .into_response()
}

fn errors(status: StatusCode, messages: &[String]) -> Response {
    answer(status, &json!({ "errors": messages }))
}

async fn dispatch(state: State, req: Request) -> Response {
    let method = req.method().clone();
    let path = format!("/{}", req.uri().path().trim_matches('/'));
    match (&method, path.as_str()) {
        (&Method::GET, "/healthz") => answer(
            StatusCode::OK,
            &json!({"status": "ok", "generatorVersion": generator_version()}),
        ),
        (&Method::POST, "/generate") => generate_route(state, req.into_body()).await,
        // The Portal answers the catalogue from its mirror and asks here only without one.
        (&Method::GET, "/catalog") => answer(
            StatusCode::OK,
            &json!({"subjects": [], "refreshedAt": null, "stale": true}),
        ),
        (&Method::POST, "/import-sdm") => import_sdm_route(req.into_body()).await,
        _ => errors(
            StatusCode::NOT_FOUND,
            &[format!("{method} {path} is not a Model Tools route")],
        ),
    }
}

/// `POST /import-sdm {model}`: the Portal asks only for a model its mirror lacks, and the
/// answer is the wizard's message, a 200 with `errors` as service.py answers a failed fetch.
async fn import_sdm_route(body: Body) -> Response {
    let model = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("model").and_then(Value::as_str).map(str::to_owned));
    let Some(model) = model else {
        return errors(
            StatusCode::BAD_REQUEST,
            &["'model' must be a Smart Data Models catalogue identifier".to_owned()],
        );
    };
    let shown: String = model.chars().take(128).collect();
    answer(
        StatusCode::OK,
        &json!({"generatorVersion": generator_version(), "errors": [format!("{shown} {NOT_MIRRORED}")]}),
    )
}

async fn generate_route(state: State, body: Body) -> Response {
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
        return errors(
            StatusCode::PAYLOAD_TOO_LARGE,
            &["body larger than the payload limit".to_owned()],
        );
    };
    let body: Value = if bytes.is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return errors(StatusCode::BAD_REQUEST, &[e.to_string()]),
        }
    };
    let Value::Object(body) = body else {
        return errors(
            StatusCode::BAD_REQUEST,
            &["the body must be a JSON object".to_owned()],
        );
    };
    let Ok(permit) = state.slots.acquire_owned().await else {
        return errors(
            StatusCode::SERVICE_UNAVAILABLE,
            &["model tools is stopping".to_owned()],
        );
    };
    // The permit goes with the job: a compilation past the timeout still holds its slot.
    let started = std::time::Instant::now();
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        generate(&body)
    });
    match tokio::time::timeout(COMPILE_TIMEOUT, job).await {
        Ok(Ok((status, payload))) => {
            let errors = payload["errors"].as_array().map_or(0, Vec::len);
            tracing::info!(
                status = status.as_u16(),
                errors,
                broker_schema = payload.get("brokerSchema").is_some(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "model-tools: generate answered"
            );
            answer(status, &payload)
        }
        Ok(Err(e)) => errors(
            StatusCode::INTERNAL_SERVER_ERROR,
            &[format!(
                "the generator stopped before it answered ({e}); try again"
            )],
        ),
        Err(_) => errors(
            StatusCode::SERVICE_UNAVAILABLE,
            &[format!(
                "the generator did not answer within {} s",
                COMPILE_TIMEOUT.as_secs()
            )],
        ),
    }
}

/// `POST /generate` on a parsed body: `{source, imports?}` (service.py `generate`).
#[must_use]
pub fn generate(body: &Map<String, Value>) -> (StatusCode, Value) {
    let Some(source) = body
        .get("source")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return (
            StatusCode::BAD_REQUEST,
            json!({"errors": ["'source' must be the LinkML document as a string"]}),
        );
    };
    let empty = Map::new();
    let imports = match body.get("imports") {
        None => Some(&empty),
        Some(Value::Object(m)) => Some(m),
        Some(_) => None,
    };
    let imports: Option<BTreeMap<String, String>> =
        imports.filter(|m| m.len() <= MAX_IMPORTS).and_then(|m| {
            m.iter()
                .map(|(name, text)| match text.as_str() {
                    Some(t) if schema::is_model_import(name) && !t.trim().is_empty() => {
                        Some((name.clone(), t.to_owned()))
                    }
                    _ => None,
                })
                .collect()
        });
    let Some(imports) = imports else {
        return (
            StatusCode::BAD_REQUEST,
            json!({"errors": [format!(
                "'imports' maps at most {MAX_IMPORTS} names `org.{{name}}.v{{major}}` or \
                 `project.{{name}}.v{{major}}` to that model's LinkML source (DM-76)"
            )]}),
        );
    };
    let mut named = std::collections::BTreeSet::new();
    for text in std::iter::once(source).chain(imports.values().map(String::as_str)) {
        named.extend(model_imports(text));
    }
    let unresolved: Vec<String> = named
        .into_iter()
        .filter(|entry| !imports.contains_key(entry))
        .map(|entry| {
            format!(
                "import '{entry}' is not resolved: name a published organization model as \
                 org.{{name}}.v{{major}} or a model of this project as project.{{name}}.v{{major}} \
                 (DM-76)"
            )
        })
        .collect();
    if !unresolved.is_empty() {
        return (
            StatusCode::OK,
            json!({"generatorVersion": generator_version(), "errors": unresolved}),
        );
    }
    (StatusCode::OK, artifacts(source, &imports))
}

/// The `org.` and `project.` entries a document imports, malformed ones included
/// (common.py `model_imports`).
fn model_imports(text: &str) -> Vec<String> {
    let mut used = 0;
    let Ok(Value::Object(doc)) = schema::parse_yaml(text, LIMITS, &mut used) else {
        return Vec::new();
    };
    let Some(Value::Array(entries)) = doc.get("imports") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(Value::as_str)
        .filter(|e| e.contains('.') && matches!(e.split('.').next(), Some("org" | "project")))
        .map(str::to_owned)
        .collect()
}

/// Every artifact of one source, or why it does not compile (service.py `artifacts`): a
/// model the loader refuses is said once, a renderer's own failure names the renderer.
fn artifacts(source: &str, imports: &BTreeMap<String, String>) -> Value {
    let version = generator_version();
    let loaded = View::load(
        source,
        &Imports {
            models: imports,
            limits: LIMITS,
        },
    )
    .map_err(LoadError::Model)
    .and_then(|view| checks::check(&view).map(|()| view));
    let view = match loaded {
        Ok(view) => view,
        Err(LoadError::Model(message)) => {
            return json!({"generatorVersion": version, "errors": [message]});
        }
        Err(LoadError::Relationships(problems)) => {
            return json!({"generatorVersion": version, "errors": problems});
        }
    };
    let mut out = Map::new();
    out.insert(
        "generatorVersion".to_owned(),
        Value::String(version.clone()),
    );
    let mut failures: Vec<(String, Vec<&str>)> = Vec::new();
    let mut fail = |field: &'static str, message: String| match failures
        .iter_mut()
        .find(|(m, _)| *m == message)
    {
        Some((_, fields)) => fields.push(field),
        None => failures.push((message, vec![field])),
    };
    let context = match context::context(&view, &version) {
        Ok(doc) => {
            out.insert("context".to_owned(), doc.clone());
            Some(doc)
        }
        Err(e) => {
            fail("context", e);
            None
        }
    };
    let model = match broker::broker_schema(&view)
        .and_then(|schema| admitted(&schema).map(|model| (schema, model)))
    {
        Ok((schema, model)) => {
            out.insert("brokerSchema".to_owned(), schema);
            Some(model)
        }
        Err(e) => {
            fail("brokerSchema", e);
            None
        }
    };
    // The Portal saves a model only with every artifact it commits (DM-02): each is rendered
    // from the model the broker reads, or its field is named in `errors` and the save refused.
    match &model {
        Some(model) => {
            match save::json_schema(model, &version) {
                Ok(schema) => {
                    out.insert("jsonSchema".to_owned(), schema);
                }
                Err(e) => fail("jsonSchema", e),
            }
            out.insert("docs".to_owned(), Value::String(save::docs(model)));
            match &context {
                Some(context) => match save::example(model, &view, context) {
                    Ok(example) => {
                        out.insert("example".to_owned(), example);
                    }
                    Err(e) => fail("example", e),
                },
                None => fail("example", "the @context did not render".to_owned()),
            }
            match save::typescript(model, &view, &version) {
                Ok(types) => {
                    out.insert("typescript".to_owned(), Value::String(types));
                }
                Err(e) => fail("typescript", e),
            }
        }
        None => {
            for field in ["jsonSchema", "docs", "example", "typescript"] {
                fail(field, "the broker schema did not render".to_owned());
            }
        }
    }
    let errors: Vec<Value> = failures
        .into_iter()
        .map(|(message, fields)| Value::String(format!("{}: {message}", fields.join(", "))))
        .collect();
    out.insert("errors".to_owned(), Value::Array(errors));
    Value::Object(out)
}

/// The broker reads a tenant's schema with `linkml-model` under its surface limits: a schema
/// it would refuse is refused here, before the reconciler sends it.
fn admitted(schema: &Value) -> Result<linkml_model::Model, String> {
    let bytes = serde_json::to_vec(schema).map_err(|e| e.to_string())?;
    linkml_model::Model::read(&bytes, &linkml_model::SURFACE_LIMITS)
        .map_err(|e| format!("the broker refuses this schema: {e}"))
}

#[cfg(test)]
mod tests;
