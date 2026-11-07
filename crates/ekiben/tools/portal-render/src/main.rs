//! Renders every seed pipeline the way the Portal's reconciler hands it to the runner, one JSON
//! line per pipeline: `{"project", "name", "stream"}` or `{"project", "name", "skip": why}`.
//!
//! Ported from joinedcontext-portal d7f91eb28 `src/reconciler/streams/render.rs` (`render_stream`,
//! `datasource_input`, `bento_processors`, `batching`, `labelled`) and `identity.rs` (`upsert`,
//! `writes`, `refusals`, `written`, `in_flight`, `own_credential`), with the DataSource half (`input_of`,
//! `prepended_processors`), the history and lifetime steps and `inject_space` called from the
//! jc-manifest the Portal pins (9b0b859). Not ported: the run log sink (`reporting`), the token
//! sidecar rewrite (`identified`), merged and Endpoint sources; those pipelines print `skip`.
//!
//! Usage: portal-render <seed dir> (each child directory is one project).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jc_core::kinds::{DataSourceSpec, DataSourceType, PipelineSpec, Step};
use jc_manifest::bento::InputContext;
use serde_json::{json, Value};

const WRITE_BATCH_BYTES: u64 = 1024 * 1024;
const TICK: &str = r#"meta jc_run = now().ts_format("2006-01-02T15:04:05Z", "UTC")"#;
/// The installation domain the harness fills in; both runners see the same one.
const ORG_DOMAIN: &str = "example.org";
const DROP_ERRORED: &str = "root = if errored() { deleted() }";

struct Doc {
    spec: Value,
    path: PathBuf,
}

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: portal-render <seed dir>");
        std::process::exit(2);
    };
    let mut projects: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    projects.sort();
    for dir in projects {
        let docs = load(&dir);
        for ((project, kind, name), doc) in &docs {
            if kind == "Pipeline" {
                let line = match render(&docs, project, name, doc) {
                    Ok(stream) => json!({ "project": project, "name": name, "stream": stream }),
                    Err(why) => json!({ "project": project, "name": name, "skip": why }),
                };
                println!("{line}");
            }
        }
    }
}

type Docs = BTreeMap<(String, String, String), Doc>;

/// Every manifest of one project directory, by (namespace, kind, name).
fn load(dir: &Path) -> Docs {
    let mut docs = Docs::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for path in files {
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !file.ends_with(".yaml") || file.ends_with("-bento.yaml") || file.contains(".linkml.") {
            continue;
        }
        // The Portal fills `{orgDomain}` in a manifest before it parses it (api/mutate.rs).
        let Ok(text) = std::fs::read_to_string(&path).map(|t| t.replace("{orgDomain}", ORG_DOMAIN))
        else {
            continue;
        };
        for doc in serde_norway::Deserializer::from_str(&text) {
            let Ok(value) = <Value as serde::Deserialize>::deserialize(doc) else {
                continue;
            };
            let (Some(kind), Some(name)) = (
                value.get("kind").and_then(Value::as_str),
                value.pointer("/metadata/name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let namespace = value
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .unwrap_or_default();
            docs.insert(
                (namespace.to_owned(), kind.to_owned(), name.to_owned()),
                Doc {
                    spec: value.get("spec").cloned().unwrap_or(Value::Null),
                    path: path.clone(),
                },
            );
        }
    }
    docs
}

fn render(docs: &Docs, project: &str, name: &str, doc: &Doc) -> Result<Value, String> {
    let pipeline: PipelineSpec =
        serde_json::from_value(doc.spec.clone()).map_err(|e| format!("pipeline spec: {e}"))?;
    let sources = pipeline.sources();
    if sources.len() != 1 {
        return Err(format!(
            "{} sources: a merged stream is not ported",
            sources.len()
        ));
    }
    let Some(ds_ref) = sources[0].data_source_ref.as_ref() else {
        return Err("an Endpoint source reads the platform's own data: not ported".into());
    };
    let ds_doc = docs
        .get(&(
            project.to_owned(),
            "DataSource".to_owned(),
            ds_ref.name().to_owned(),
        ))
        .ok_or_else(|| format!("data source {} is not in the seed", ds_ref.name()))?;
    let source: DataSourceSpec = serde_json::from_value(ds_doc.spec.clone())
        .map_err(|e| format!("data source spec: {e}"))?;
    let outputs = pipeline.outputs();
    let output = outputs
        .first()
        .ok_or("the pipeline names no target endpoint")?;
    let endpoint = docs
        .get(&(
            project.to_owned(),
            "Endpoint".to_owned(),
            output.target_endpoint.local_id().to_owned(),
        ))
        .ok_or_else(|| {
            format!(
                "endpoint {} is not in the seed",
                output.target_endpoint.local_id()
            )
        })?;
    let slug = endpoint
        .spec
        .get("slug")
        .and_then(Value::as_str)
        .ok_or("target endpoint has no slug")?;
    let replace = output.mode == Some(jc_core::kinds::OutputMode::Replace);
    let bento_path = doc.path.with_file_name(format!(
        "{}-bento.yaml",
        doc.path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
    ));
    let bento = std::fs::read_to_string(&bento_path).ok();

    let (input, mut processors) =
        datasource_input(&pipeline, name, project, &source, ds_ref.name())?;
    let compute = pipeline.steps().into_iter().find_map(|step| match step {
        Step::Compute(compute) => Some(compute),
        Step::Processor(_) => None,
    });
    match (compute, bento) {
        (Some(compute), _) => {
            if let Some(bloblang) = compute.mapping().map_err(|e| e.to_string())? {
                processors.push(labelled(json!({ "mapping": bloblang }), "compute"));
            }
        }
        (None, Some(bento)) => processors.extend(bento_processors(&bento)?),
        (None, None) => {}
    }
    if let Some(text) = jc_manifest::history::step(&pipeline).map_err(|e| e.to_string())? {
        processors.push(labelled(json!({ "mapping": text }), "history"));
    }
    if pipeline.source_deletes.is_some() {
        return Err("spec.sourceDeletes posts its marks to the Portal: not ported".into());
    }
    if let Some(text) = jc_manifest::lifetime::step(&pipeline).map_err(|e| e.to_string())? {
        processors.push(labelled(json!({ "mapping": text }), "lifetime"));
    }
    processors.extend(batching());
    processors.extend(writes(project, slug, replace));

    let threads = if source.terminates() {
        4
    } else if matches!(
        source.source_type,
        DataSourceType::Mqtt | DataSourceType::WebSocket
    ) {
        1
    } else {
        64
    };
    let mut stream = json!({
        "input": labelled(input, "input"),
        "pipeline": {
            "threads": threads,
            "processors": processors.into_iter().enumerate()
                .map(|(at, p)| labelled(p, &format!("processor_{at}"))).collect::<Vec<_>>()
        },
        "output": labelled(written(), "output")
    });
    let segments: Vec<String> = outputs
        .iter()
        .map(|o| {
            let space = docs
                .get(&(
                    project.to_owned(),
                    "Endpoint".to_owned(),
                    o.target_endpoint.local_id().to_owned(),
                ))
                .and_then(|ep| ep.spec.get("contextSpaceRef"))
                .and_then(|r| r.as_str().or_else(|| r.get("name").and_then(Value::as_str)))
                .unwrap_or_default()
                .to_owned();
            let pin = docs
                .get(&(project.to_owned(), "ContextSpace".to_owned(), space.clone()))
                .and_then(|s| s.spec.get("urnSegment"))
                .and_then(Value::as_str);
            jc_core::kinds::urn_segment(project, &space, pin)
        })
        .collect();
    jc_manifest::bento::inject_space(&mut stream, &segments);
    own_credential(&mut stream, project);
    Ok(stream)
}

/// identity.rs `own_credential`: the reconciler's last pass before the send. An author's
/// `${JC_CLIENT_ID}`/`${JC_CLIENT_SECRET}` become the project's pipeline client, every cache a
/// step names becomes `<project>__<name>` and a step's egress rate limit the project's bucket.
fn own_credential(value: &mut Value, project: &str) {
    let prefix = format!("{}__", project.replace('-', "_"));
    match value {
        Value::String(text) if text.contains("${JC_CLIENT_") => {
            *text = text
                .replace(
                    "${JC_CLIENT_ID}",
                    &jc_core::kinds::service_account::keycloak_client_id(project, "pipelines"),
                )
                .replace(
                    "${JC_CLIENT_SECRET}",
                    &format!("${{{}}}", client_secret_var(project)),
                );
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| own_credential(item, project)),
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                let field = match key.as_str() {
                    "cache" => Some("resource"),
                    "cached" | "dedupe" => Some("cache"),
                    _ => None,
                };
                if let Some(Value::String(name)) = field.and_then(|at| inner.get_mut(at)) {
                    if !name.starts_with(&prefix) {
                        *name = format!("{prefix}{name}");
                    }
                }
                if key == "rate_limit" {
                    let name = match inner {
                        Value::String(name) => Some(name),
                        Value::Object(m) => match m.get_mut("resource") {
                            Some(Value::String(name)) => Some(name),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(name) =
                        name.filter(|n| n.contains("pipeline_egress") || n.contains("${"))
                    {
                        *name = format!("{prefix}pipeline_egress");
                    }
                }
                own_credential(inner, project);
            }
        }
        _ => {}
    }
}

fn datasource_input(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    source_name: &str,
) -> Result<(Value, Vec<Value>), String> {
    let context = InputContext {
        source: source_name,
        project,
        pipeline: name,
        pipeline_spec: Some(pipeline),
    };
    let input = serde_json::to_value(jc_manifest::bento::input_of(source, &context))
        .map_err(|e| e.to_string())?;
    let (input, mut processors) = match source.source_type {
        DataSourceType::Http => {
            let interval = [&pipeline.period, &pipeline.schedule]
                .into_iter()
                .find_map(|v| v.as_deref().filter(|s| !s.trim().is_empty()))
                .unwrap_or("60s");
            let http = input
                .get("http_client")
                .cloned()
                .unwrap_or_else(|| json!({}));
            (
                json!({ "generate": { "interval": interval, "mapping": format!("root = \"\"\n{TICK}") } }),
                vec![
                    json!({ "try": [{ "http": http }] }),
                    json!({ "mutation": DROP_ERRORED }),
                ],
            )
        }
        DataSourceType::Mqtt | DataSourceType::WebSocket => (
            json!({ "label": "input_batches", "broker": {
                "inputs": [labelled(input, "input")],
                "batching": { "count": 1000, "period": "1s" } } }),
            Vec::new(),
        ),
        _ => (input, Vec::new()),
    };
    for p in jc_manifest::bento::prepended_processors(source, &context) {
        processors.push(serde_json::to_value(&p).map_err(|e| e.to_string())?);
    }
    Ok((input, processors))
}

fn bento_processors(bento: &str) -> Result<Vec<Value>, String> {
    let config: Value = serde_norway::from_str(bento).map_err(|e| format!("bento.yaml: {e}"))?;
    if config.get("input").is_some() {
        return Err("bento.yaml declares an input, which the Portal refuses".into());
    }
    Ok(config
        .pointer("/pipeline/processors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn labelled(mut component: Value, label: &str) -> Value {
    if let Some(fields) = component.as_object_mut() {
        if !fields.contains_key("label") && !fields.contains_key("resource") {
            fields.insert("label".to_owned(), json!(label));
        }
    }
    component
}

fn batching() -> Vec<Value> {
    vec![
        json!({ "mapping": "root = if this.type() == \"array\" { this } else { [this] }" }),
        json!({ "unarchive": { "format": "json_array" } }),
        json!({ "split": { "size": 1000, "byte_size": WRITE_BATCH_BYTES } }),
        json!({ "archive": { "format": "json_array" } }),
    ]
}

fn client_secret_var(project: &str) -> String {
    let upper: String = project
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("JC_CLIENT_SECRET_{upper}")
}

fn writes(project: &str, slug: &str, replace: bool) -> Vec<Value> {
    let options = if replace { "" } else { "?options=update" };
    let upsert = json!({
        "url": format!("${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/upsert{options}"),
        "verb": "POST",
        "headers": { "Content-Type": "application/json" },
        "oauth2": {
            "enabled": true,
            "client_key": jc_core::kinds::service_account::keycloak_client_id(project, "pipelines"),
            "client_secret": format!("${{{}}}", client_secret_var(project)),
            "token_url": "${JC_TOKEN_URL}"
        },
        "timeout": "30s",
        "rate_limit": format!("{}__pipeline_egress", project.replace('-', "_")),
        "drop_on": [400, 413, 422]
    });
    vec![
        json!({ "label": "write_0", "branch": {
            "processors": [{ "http": upsert }],
            "result_map": "let errors = content().parse_json().catch({}).errors.or([])\n\
                meta jc_refused_write_0 = $errors.map_each(e -> { \"id\": e.entityId.or(\"\"), \
                \"message\": e.error.detail.or(e.error.title).or(\"\") }).format_json(no_indent: true)"
        }}),
        json!({ "label": "write_refused", "switch": [{
            "check": r#"errored() && error().re_match("unexpected response code \\((400|413|422)\\)")"#,
            "processors": [{ "catch": [{ "mutation":
                "let problem = error().re_replace_all(\"(?s)^.*?Error: \", \"\").parse_json().catch({})\n\
                 meta jc_failed = $problem.detail.or($problem.title).or(error())" }] }]
        }]}),
    ]
}

fn written() -> Value {
    json!({ "switch": {
        "retry_until_success": false,
        "cases": [
            { "check": "errored()", "output": { "reject": "${! error() }" } },
            { "output": { "drop": {} } },
        ],
    }})
}
