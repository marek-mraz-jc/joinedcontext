//! The pure JSON rendering of a Stream (T-5340 split it out of `streams`): an approved
//! Pipeline, its DataSource or Endpoint source, merged sources, KPI joins and routes become
//! one native Bento stream configuration. No I/O: the deployer in `streams` sends what these
//! return.

use std::time::Duration;

use jc_core::kinds::data_source::{check_class, DataSourceSpec, DataSourceType};
use jc_core::kinds::pipeline::{Compute, PipelineSource, PipelineSpec, Step};
use jcctl::bento::InputContext;
use serde_json::Value;

use super::identity::{
    bound_in_flight, in_flight, pipeline_oauth2, reads_in_batches, replaces, writes, written,
};
use super::RenderError;
use crate::store::Mirror;

/// Renders a native Bento stream configuration for an approved DataSource pipeline.
///
/// `bento` is the author's `bento.yaml` beside the manifest, used when the pipeline has no
/// inline `compute` (PL-03).
#[allow(clippy::too_many_arguments)]
pub fn render_stream(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    source_name: &str,
    slug: &str,
    bento: Option<&str>,
) -> Result<Value, RenderError> {
    let (input, mut processors) = datasource_input(pipeline, name, project, source, source_name)?;

    match (first_compute(pipeline).as_ref(), bento) {
        (Some(compute), _) => {
            if !compute.kind.is_mapping() {
                return Err(RenderError::MissingCompute);
            }
            if let Some(bloblang) = compute
                .mapping()
                .map_err(|err| RenderError::Custom(err.to_string()))?
            {
                processors.push(labelled(
                    serde_json::json!({ "mapping": bloblang }),
                    COMPUTE_LABEL,
                ));
            }
        }
        (None, Some(bento)) => {
            processors.extend(bento_processors(bento, &pipeline.own_variables())?)
        }
        (None, None) => {}
    }

    processors.extend(history_step(pipeline)?);
    processors.extend(source_events_step(pipeline));
    processors.extend(lifetime_step(pipeline)?);
    processors.extend(batching());
    processors.extend(writes(
        project,
        &[(slug.to_owned(), replaces(pipeline, 0))],
        "",
    )?);

    // T-1125, PL-24: Bento reports its counters under the component's `label`, so an unlabelled
    // stream can only be read as one total. Labelling the input, the output and each processor
    // by its place is what lets the studio say which node a message stopped at. The labels are
    // the stream's own and never leave it, so they carry no name a caller chose.
    let mut stream = serde_json::json!({
        "input": labelled(input, "input"),
        "pipeline": {
            "processors": processors
                .into_iter()
                .enumerate()
                .map(|(at, processor)| labelled(processor, &format!("processor_{at}")))
                .collect::<Vec<_>>()
        },
        "output": labelled(written(), "output")
    });
    bound_in_flight(&mut stream, in_flight([Some(source)]));
    Ok(stream)
}

/// The label of a pipeline's compute step, the studio's `compute` node (PL-66).
pub const COMPUTE_LABEL: &str = "compute";

/// The label of the step `spec.history` compiles to (PL-107).
pub const HISTORY_LABEL: &str = "history";

/// A page whose read failed is dropped; a kpi source keeps it as an error instead (PL-104).
pub(crate) const DROP_ERRORED: &str = "root = if errored() { deleted() }";

/// The label of the step `spec.lifetime` compiles to (PL-82).
pub const LIFETIME_LABEL: &str = "lifetime";

/// `spec.lifetime` as its one Bento step (PL-82, T-4839): after the author's steps and routes,
/// before the batching, so the validation stage at the split checks the `expiresAt` it wrote.
pub(crate) fn lifetime_step(pipeline: &PipelineSpec) -> Result<Option<Value>, RenderError> {
    let step = jcctl::lifetime::step(pipeline).map_err(|e| RenderError::Custom(e.to_string()))?;
    Ok(step.map(|text| labelled(serde_json::json!({ "mapping": text }), LIFETIME_LABEL)))
}

/// The label of the step `spec.sourceDeletes.deletedWhen` compiles to (PL-98).
pub const SOURCE_EVENTS_LABEL: &str = "source_events";

/// `spec.sourceDeletes` in `events` mode as Bento steps (PL-98, T-5360): after the author's steps
/// and routes, before the lifetime step. `deletedWhen` is read on each mapped record: `true` takes
/// it out of the write and marks its id, `false` writes it, an error or any other answer takes it
/// out as failed. The marks ride in metadata to the Portal's post the deployer puts right after
/// [`SOURCE_EVENTS_LABEL`]; a message left without a record is dropped.
pub(crate) fn source_events_step(pipeline: &PipelineSpec) -> Vec<Value> {
    let Some(query) = pipeline
        .source_deletes
        .as_ref()
        .filter(|rule| rule.mode == jc_core::kinds::SourceDeleteMode::Events)
        .and_then(|rule| rule.deleted_when.as_deref())
        .filter(|query| !query.trim().is_empty())
    else {
        return Vec::new();
    };
    let mapping = format!(
        "# spec.sourceDeletes.deletedWhen (PL-98), rendered by the Portal.\n\
         map jc_deleted_when {{\n  root = (\n{query}\n  )\n}}\n\
         let records = if this.type() == \"array\" {{ this }} else {{ [this] }}\n\
         let judged = $records.map_each(e -> {{\"record\": e, \"verdict\": if e.type() == \"object\" {{ e.apply(\"jc_deleted_when\").catch(err -> {{\"error\": err}}) }} else {{ false }}}})\n\
         let kept = $judged.filter(j -> j.verdict == false).map_each(j -> j.record)\n\
         meta {MARKED_META} = $judged.filter(j -> j.verdict == true).map_each(j -> j.record.id.or(\"\").string()).format_json(no_indent: true).string()\n\
         meta {MARK_FAILED_META} = $judged.filter(j -> j.verdict.type() != \"bool\").map_each(j -> {{\"id\": j.record.id.or(\"\").string(), \"message\": if j.verdict.type() == \"object\" {{ j.verdict.error.or(\"\").string() }} else {{ \"deletedWhen answered \" + j.verdict.format_json(no_indent: true).string() + \", not true or false\" }}}}).format_json(no_indent: true).string()\n\
         root = if this.type() == \"array\" {{ $kept }} else if $kept.length() == 1 {{ $kept.index(0) }} else {{ [] }}"
    );
    vec![
        labelled(
            serde_json::json!({ "mapping": mapping }),
            SOURCE_EVENTS_LABEL,
        ),
        labelled(
            serde_json::json!({ "mapping": "root = if this.type() == \"array\" && this.length() == 0 { deleted() } else { this }" }),
            "source_events_kept",
        ),
    ]
}

/// The metadata keys the [`source_events_step`] leaves its marks and its failed records in.
const MARKED_META: &str = "jc_marked";
const MARK_FAILED_META: &str = "jc_marked_failed";

/// Posts what [`source_events_step`] marked to the Portal's `url` (PL-98), right after the step.
/// A Portal that does not answer loses the marks: nothing is deleted, the record is not written,
/// and the stream is never held back. A stream without the step is left as it is.
pub(super) fn with_source_events(stream: &mut Value, url: &str) {
    let Some(processors) = stream
        .pointer_mut("/pipeline/processors")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let Some(at) = processors
        .iter()
        .position(|p| p.get("label").and_then(Value::as_str) == Some(SOURCE_EVENTS_LABEL))
    else {
        return;
    };
    let post = serde_json::json!({
        "label": "source_events_post",
        "switch": [{
            "check": "!errored()",
            "processors": [
                { "branch": {
                    "request_map": format!(
                        "let ids = meta(\"{MARKED_META}\").or(\"[]\").parse_json()\n\
                         let failed = meta(\"{MARK_FAILED_META}\").or(\"[]\").parse_json()\n\
                         root = if $ids.length() + $failed.length() == 0 {{ deleted() }} else {{ {{\"run\": {}, \"ids\": $ids, \"failed\": $failed}} }}",
                        crate::pipeline_log::RUN
                    ),
                    "processors": [{ "http": {
                        "url": url,
                        "verb": "POST",
                        "headers": { "Content-Type": "application/json" },
                        "timeout": "5s",
                        "retries": 1,
                        "oauth2": {
                            "enabled": true,
                            "client_key": "${JC_CLIENT_ID}",
                            "client_secret": "${JC_CLIENT_SECRET}",
                            "token_url": "${JC_TOKEN_URL}",
                        },
                    }}],
                }},
                { "catch": [{ "log": {
                    "level": "WARN",
                    "message": "the source's delete marks did not reach the Portal; nothing is deleted: ${! error() }",
                }}]},
            ],
        }],
    });
    processors.insert(at + 1, post);
}

/// Reports a source read that failed to the run's log (T-5721, T-4428): each read drops its
/// errored page ([`DROP_ERRORED`]), so a source that timed out or refused the request left a run
/// of sent 0, failed 0, the same as a source with nothing in it. A read in the stream's own
/// processors gets the report in place of its drop. Reads inside the input (a merged source or a
/// broker of several) lose their drop and one report at the head of the stream's processors,
/// ahead of the pass report's `catch`, serves them all: Bento's labels are unique per stream. The run
/// queue's `one_run` reports an `http_client` input's failure itself, so a stream with one keeps
/// its inputs' drops.
pub(super) fn with_source_failures(stream: &mut Value, url: &str) {
    let drops = |p: &Value| p.get("mutation").and_then(Value::as_str) == Some(DROP_ERRORED);
    let Some(processors) = stream
        .pointer_mut("/pipeline/processors")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Some(at) = processors.iter().position(drops) {
        let mut report = crate::pipeline_log::source_failure(url);
        if let Some(slot) = processors.get_mut(at) {
            if let Some(label) = slot.get("label").cloned() {
                report["label"] = label;
            }
            *slot = report;
        }
        processors.retain(|p| !drops(p));
        return;
    }
    let mut dropped = false;
    if let Some(input) = stream.get_mut("input") {
        if input.to_string().contains("\"http_client\"") {
            return;
        }
        let mut inputs = vec![input];
        while let Some(input) = inputs.pop() {
            if let Some(own) = input.get_mut("processors").and_then(Value::as_array_mut) {
                let before = own.len();
                own.retain(|p| !drops(p));
                dropped |= own.len() != before;
            }
            if let Some(broker) = input
                .pointer_mut("/broker/inputs")
                .and_then(Value::as_array_mut)
            {
                inputs.extend(broker.iter_mut());
            }
        }
    }
    if dropped {
        if let Some(processors) = stream
            .pointer_mut("/pipeline/processors")
            .and_then(Value::as_array_mut)
        {
            processors.insert(0, crate::pipeline_log::source_failure(url));
        }
    }
}

/// `spec.history` as its one Bento step (PL-107, T-4845): the planned `observedAt` of each
/// attribute, after the author's steps and before the lifetime step, whose `from: observedAt`
/// then reads the same time.
pub(crate) fn history_step(pipeline: &PipelineSpec) -> Result<Option<Value>, RenderError> {
    let step = jcctl::history::step(pipeline).map_err(|e| RenderError::Custom(e.to_string()))?;
    Ok(step.map(|text| labelled(serde_json::json!({ "mapping": text }), HISTORY_LABEL)))
}

/// One Bento component with a `label`, so its counters are attributable (PL-24, T-1125).
///
/// A component that already carries one keeps it: a label the author wrote is theirs, and the
/// metrics they read elsewhere are named by it. Anything that is not a map is returned as it
/// came, because only a map can carry the key. A `resource:` reference stays bare: Bento's lint
/// refuses a label on one (the resource carries its own), and the runner refuses the whole stream.
pub(super) fn labelled(component: serde_json::Value, label: &str) -> serde_json::Value {
    let mut component = component;
    match component.as_object_mut() {
        Some(fields) if !fields.contains_key("label") && !fields.contains_key("resource") => {
            fields.insert("label".to_owned(), serde_json::json!(label));
            component
        }
        _ => component,
    }
}

/// The author's processors from a `bento.yaml`. Its `input` is refused, the DataSource is the
/// input (PL-39); its `output` and everything else at the top level are dropped: every stream
/// writes through the endpoint upsert rendered here (PL-16), never back to a caller (PL-06), and
/// the runner's own resources (rate limits, caches) come from its resources file. `own` is the
/// `envVar` of each of the pipeline's `secretRefs`, the only runner variables beside the
/// platform's own a step may name (PL-16, T-3163).
fn bento_processors(bento: &str, own: &[&str]) -> Result<Vec<Value>, RenderError> {
    let config: Value =
        serde_norway::from_str(bento).map_err(|e| RenderError::Bento(e.to_string()))?;
    if config.get("input").is_some() {
        return Err(RenderError::BentoInput);
    }
    let processors = config
        .pointer("/pipeline/processors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // The author's processors pass the check a manifest's steps pass (PL-16, PL-50, PL-52): a
    // name the platform runs, no refused processor nested inside, no read of the runner's
    // environment but its own pipeline's variables, no call to the runner's own pod. This is
    // where an author's file meets the runner, which holds every project's credentials.
    for processor in &processors {
        jc_core::kinds::pipeline::validate_processor_reading("pipeline.processors", processor, own)
            .map_err(|e| RenderError::Bento(e.to_string()))?;
    }
    Ok(processors)
}

/// One query component, percent-encoded: the unreserved characters and `:` (every URN is full of
/// them) stay, every other UTF-8 byte becomes `%XX`. The runner reads `${...}` inside a URL as
/// its own interpolation, so `$`, `{`, `}` and `!` must never reach it as written (T-2542).
fn percent_encode(val: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(val.len());
    for b in val.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b':') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0F)]));
        }
    }
    out
}

/// Entities one request of an endpoint source reads: the gateway's page (PL-31).
pub const SOURCE_PAGE: u64 = 1000;
/// The most entities one run of an endpoint source reads (PL-31, T-3132): a source holding more
/// fails the read rather than handing the compute a cut list.
pub const SOURCE_CEILING: u64 = 50_000;
/// The metadata key a page request reads its offset from.
const PAGE_OFFSET_META: &str = "jc_page_offset";

/// What turns the first page and the source's stated total into one message per page still to
/// read (PL-31, T-3132): the first page as `{rows}`, every other page as `{offset}`. A total past
/// the ceiling, or a full first page whose total the gateway did not state, fails the read: one
/// page of a larger answer is a cut list, and a compute over it reports wrong numbers as right.
/// `range` with a step drops its last element in Bento, so pages are counted instead.
fn pages_mapping() -> String {
    format!(
        r#"let total = meta("Ngsild-Results-Count").or(meta("ngsild-results-count")).or("").string()
let total = if $total == "" {{ if this.length() < {SOURCE_PAGE} {{ this.length() }} else {{ throw("the source answered a full page of {SOURCE_PAGE} and did not say how many entities it holds, so a read of one page would cut the rest off") }} }} else {{ $total.number() }}
let pages = ($total / {SOURCE_PAGE}).ceil().int64()
root = if $total > {SOURCE_CEILING} {{ throw("the source holds %d entities, more than the {SOURCE_CEILING} one run reads".format($total.int64())) }} else {{ [{{"rows": this}}].concat(if $pages > 1 {{ range(1, $pages).map_each(page -> {{"offset": page * {SOURCE_PAGE}}}) }} else {{ [] }}) }}"#
    )
}

/// The read of an endpoint source, every page of it (PL-31, T-3132): the first page with the
/// total (`count=true`), then one request per further page, merged back into the one list the
/// compute reads. `read` is the request of the first page as the source is read.
fn paged_read(read: &Value, source_url: &str) -> Value {
    let mut first = read.clone();
    first["url"] = Value::String(format!("{source_url}&count=true"));
    first["extract_headers"] =
        serde_json::json!({ "include_patterns": ["(?i)^ngsild-results-count$"] });
    let mut page = read.clone();
    page["url"] = Value::String(format!(
        "{source_url}&offset=${{! meta(\"{PAGE_OFFSET_META}\") }}"
    ));
    serde_json::json!({ "try": [
        { "http": first },
        { "mapping": pages_mapping() },
        { "unarchive": { "format": "json_array" } },
        { "switch": [
            {
                "check": "this.exists(\"offset\")",
                "processors": [
                    { "mapping": format!("meta {PAGE_OFFSET_META} = this.offset.string()") },
                    { "http": page }
                ]
            },
            { "processors": [{ "mapping": "root = this.rows" }] }
        ] },
        { "archive": { "format": "json_array" } },
        { "mapping": "root = this.flatten()" }
    ] })
}

/// Builds the Context Gateway entity query URL for an endpoint-sourced pipeline (PL-31).
///
/// Every value is encoded on its own; the commas between ids and between attribute names are
/// the list separators the gateway splits on, so an attribute name that holds one is refused.
pub fn endpoint_source_url(
    source_slug: &str,
    query: &jc_core::kinds::pipeline::SourceQuery,
) -> Result<String, RenderError> {
    fn list<'a>(values: impl Iterator<Item = &'a str>) -> String {
        values.map(percent_encode).collect::<Vec<_>>().join(",")
    }

    let mut params = Vec::new();
    if !query.ids.is_empty() {
        let ids = query
            .ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        params.push(format!("id={}", list(ids.iter().map(String::as_str))));
    } else if let Some(entity_type) = &query.entity_type {
        params.push(format!("type={}", percent_encode(entity_type)));
    }

    if let Some(attr) = query.attrs.iter().find(|a| a.contains(',')) {
        return Err(RenderError::Custom(format!(
            "spec.source.query.attrs names {attr:?}, and a comma separates attribute names, so it would be read as two"
        )));
    }
    if !query.attrs.is_empty() {
        params.push(format!(
            "attrs={}",
            list(query.attrs.iter().map(String::as_str))
        ));
    }

    params.push(format!("limit={SOURCE_PAGE}"));

    for (name, value) in [
        ("q", &query.q),
        ("scopeQ", &query.scope_q),
        ("geoQ", &query.geo_q),
    ] {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            params.push(format!("{name}={}", percent_encode(value)));
        }
    }

    let query_str = params.join("&");
    Ok(format!(
        "${{JC_GATEWAY_URL}}/api/endpoint/{source_slug}/ngsi-ld/v1/entities?{query_str}"
    ))
}

/// The Temporal API read of a kpi source one period back (PL-117): the source's own type and
/// filters, the attributes the `prev` terms fold, and each entity's newest reading at that time.
/// The time is taken per request in the run; a period reaches this far only through jc-core's
/// check, so the text is a duration and nothing else.
fn earlier_source_url(
    source_slug: &str,
    query: &jc_core::kinds::pipeline::SourceQuery,
    read: &jc_core::kpi_join::Earlier,
) -> Result<String, RenderError> {
    let mut query = query.clone();
    query.attrs.clone_from(&read.attrs);
    let url = endpoint_source_url(source_slug, &query)?;
    Ok(format!(
        "{}&timerel=before&lastN=1&timeAt=${{! now().ts_sub_iso8601(\"{}\").ts_format(\"2006-01-02T15:04:05Z\", \"UTC\") }}",
        url.replacen("/ngsi-ld/v1/entities?", "/ngsi-ld/v1/temporal/entities?", 1),
        read.period
    ))
}

/// The runner's cache in which an on-change stream keeps the hash of the last page it passed
/// (PL-51); the runner declares it per project as `cache_label` makes it (T-5337).
pub const CHANGE_CACHE: &str = "pipeline_changes";
/// How often an on-change stream looks at its source when the pipeline names no period.
const CHANGE_POLL: &str = "10s";
/// The shortest look an on-change stream takes: a period under it is raised to it.
const CHANGE_POLL_FLOOR: Duration = Duration::from_secs(5);

/// `10s`, `2m`, `500ms`, `1h` as a duration; anything else is not one.
pub(crate) fn duration_of(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(number)),
        "s" => Some(Duration::from_secs(number)),
        "m" => Some(Duration::from_secs(number * 60)),
        "h" => Some(Duration::from_secs(number * 3600)),
        _ => None,
    }
}

/// The processors that pass a source page on only when a watched value changed since the page
/// the stream last passed (PL-51): the hash of every entity's id and watched values goes to the
/// message's metadata, the last hash comes out of the shared cache (a first run misses it, and
/// `catch` clears that miss), an equal hash drops the page, and a new one is stored before the
/// compute sees the page. Unlike a `dedupe`, a value that returns to an earlier one passes.
fn change_gate(stream_key: &str, watched: &[String]) -> Vec<Value> {
    let values = if watched.is_empty() {
        "e".to_owned()
    } else {
        watched
            .iter()
            .map(|attr| {
                let path = Value::String(format!("{attr}.value")).to_string();
                format!("e.get({path})")
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    vec![
        serde_json::json!({
            "mutation": format!(
                "meta jc_change = (if this.type() == \"array\" {{ this }} else {{ [this] }}).map_each(e -> [e.id, {values}]).format_json().string().hash(\"xxhash64\").encode(\"hex\")"
            )
        }),
        serde_json::json!({
            "branch": {
                "request_map": "root = \"\"",
                "processors": [{
                    "cache": { "resource": CHANGE_CACHE, "operator": "get", "key": stream_key }
                }],
                "result_map": "meta jc_last = this"
            }
        }),
        serde_json::json!({ "catch": [] }),
        serde_json::json!({ "mutation": "root = if @jc_last == @jc_change { deleted() }" }),
        serde_json::json!({
            "cache": {
                "resource": CHANGE_CACHE,
                "operator": "set",
                "key": stream_key,
                "value": "\"${! @jc_change }\""
            }
        }),
    ]
}

/// Renders a native Bento stream configuration for an endpoint-sourced pipeline (PL-31, PL-45):
/// on its clock, or, with `source.trigger.subscription`, on every change of a watched value
/// (PL-51). `stream_key` names the stream in the runner's shared change cache.
pub fn render_endpoint_stream(
    pipeline: &PipelineSpec,
    project: &str,
    stream_key: &str,
    source_slug: &str,
    source_is_public: bool,
    target_slug: &str,
) -> Result<Value, RenderError> {
    let source = pipeline.sources().into_iter().next();
    let (input, mut processors) = endpoint_input(
        pipeline,
        project,
        source.as_ref(),
        stream_key,
        source_slug,
        source_is_public,
        None,
    )?;

    let bloblang = first_compute(pipeline)
        .and_then(|c| c.bloblang)
        .filter(|b| !b.trim().is_empty())
        .ok_or(RenderError::MissingCompute)?;

    processors.push(labelled(
        serde_json::json!({ "mapping": bloblang }),
        COMPUTE_LABEL,
    ));

    processors.extend(history_step(pipeline)?);
    processors.extend(source_events_step(pipeline));
    processors.extend(lifetime_step(pipeline)?);
    processors.extend(batching());
    processors.extend(writes(
        project,
        &[(target_slug.to_owned(), replaces(pipeline, 0))],
        "",
    )?);

    let mut stream = serde_json::json!({
        "input": input,
        "pipeline": {
            "processors": processors
        },
        "output": written()
    });
    // An Endpoint source polls on a clock (`generate`), whatever its trigger.
    bound_in_flight(&mut stream, in_flight([None]));
    Ok(stream)
}

/// The first compute step of either shape (PL-54): `v1alpha1`'s `compute`.
pub(super) fn first_compute(pipeline: &PipelineSpec) -> Option<Compute> {
    pipeline.steps().into_iter().find_map(|step| match step {
        Step::Compute(compute) => Some(compute),
        Step::Processor(_) => None,
    })
}

/// The most bytes of entities one write carries: a quarter of the broker's 4 MiB body bound
/// (Antares `MAX_BODY_BYTES`), so the names expanded to IRIs on the way still fit. 1000 census
/// rows of 500 Attributes, or 1000 investment polygons, went out as one body and were refused
/// 413 whole (T-4753: 8,132 BBSK rows on prod).
const WRITE_BATCH_BYTES: u64 = 1024 * 1024;

/// What turns a page into gateway-sized batches: one entity per message, 1000 per request, and
/// no more than [`WRITE_BATCH_BYTES`] per request.
///
/// The gateway takes at most 1000 entities per batch operation; a page of 4000 stations goes
/// out as four requests, not one 400.
pub(super) fn batching() -> Vec<Value> {
    vec![
        serde_json::json!({
            "mapping": "root = if this.type() == \"array\" { this } else { [this] }"
        }),
        serde_json::json!({ "unarchive": { "format": "json_array" } }),
        serde_json::json!({ "split": { "size": 1000, "byte_size": WRITE_BATCH_BYTES } }),
        serde_json::json!({ "archive": { "format": "json_array" } }),
    ]
}

/// How many messages of a resident source one write carries at most, the gateway's batch
/// limit, and how long the first of them waits for the rest (T-5046).
const RESIDENT_BATCH: u64 = 1000;
const RESIDENT_BATCH_PERIOD: &str = "1s";

/// A resident source (MQTT, WebSocket) read in batches (T-5046): a message is one record, and
/// unbatched every record was its own upsert, 64 waiting on the gateway at once per stream (the
/// HSL feed held the gateway's in-flight bound and saw 503s). A batch is one upsert of up to
/// [`RESIDENT_BATCH`] records, or of what came in [`RESIDENT_BATCH_PERIOD`]. The source keeps
/// the `input` label: Bento counts what a broker's input received under the input's own label,
/// never the broker's.
fn resident_batches(input: Value) -> Value {
    serde_json::json!({
        "label": "input_batches",
        "broker": {
            "inputs": [labelled(input, "input")],
            "batching": { "count": RESIDENT_BATCH, "period": RESIDENT_BATCH_PERIOD },
        },
    })
}

/// A DataSource's input and the processors that belong to it (PL-47, PL-50): an `http`
/// DataSource polls on a `generate` clock through the `http` processor, because Bento's
/// `http_client` input polls without pause.
pub(super) fn datasource_input(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    source: &DataSourceSpec,
    source_name: &str,
) -> Result<(Value, Vec<Value>), RenderError> {
    check_class(pipeline, source).map_err(|e| RenderError::Class(e.to_string()))?;

    let context = InputContext {
        source: source_name,
        project,
        pipeline: name,
        pipeline_spec: Some(pipeline),
    };
    let input_yaml = jcctl::bento::input_of(source, &context);
    let input_json: Value = serde_json::to_value(&input_yaml)?;

    let (input, mut processors) = if matches!(source.source_type, DataSourceType::Http) {
        // A scheduled pipeline's clock is its cron `schedule`; without it the 60 s default
        // ran 92 daily, weekly and monthly open-data pipelines every minute (T-4255).
        let interval = [&pipeline.period, &pipeline.schedule]
            .into_iter()
            .find_map(|v| v.as_deref().filter(|s| !s.trim().is_empty()))
            .unwrap_or("60s");
        let http_client = match input_json {
            Value::Object(mut map) => map
                .remove("http_client")
                .unwrap_or_else(|| serde_json::json!({})),
            _ => serde_json::json!({}),
        };
        let p1 = serde_json::json!({
            "try": [{
                "http": http_client
            }]
        });
        let p2 = serde_json::json!({
            "mutation": DROP_ERRORED
        });
        // Each tick is one run of the pipeline's log (PL-62).
        let inp = serde_json::json!({
            "generate": {
                "interval": interval,
                "mapping": format!("root = \"\"\n{}", crate::pipeline_log::TICK)
            }
        });
        (inp, vec![p1, p2])
    } else if reads_in_batches(source) {
        (resident_batches(input_json), Vec::new())
    } else {
        (input_json, Vec::new())
    };

    for p in jcctl::bento::prepended_processors(source, &context) {
        processors.push(serde_json::to_value(&p)?);
    }
    Ok((input, processors))
}

/// An Endpoint source's input and processors (PL-31, PL-51): a clock, the read, and with a
/// subscription trigger the change gate keyed by `stream_key`.
fn endpoint_input(
    pipeline: &PipelineSpec,
    project: &str,
    source: Option<&PipelineSource>,
    stream_key: &str,
    source_slug: &str,
    source_is_public: bool,
    earlier: Option<&jc_core::kpi_join::Earlier>,
) -> Result<(Value, Vec<Value>), RenderError> {
    let query = source
        .and_then(|s| s.query.as_ref())
        .ok_or_else(|| RenderError::Custom("endpoint pipeline missing query".to_string()))?;
    let trigger = source.and_then(|s| s.trigger.as_ref());

    fn non_empty(v: &Option<String>) -> Option<&str> {
        v.as_deref().filter(|s| !s.trim().is_empty())
    }
    let interval = match trigger {
        Some(_) => match non_empty(&pipeline.period) {
            Some(period) => match duration_of(period) {
                Some(d) if d < CHANGE_POLL_FLOOR => {
                    format!("{}s", CHANGE_POLL_FLOOR.as_secs())
                }
                Some(_) => period.to_owned(),
                None => CHANGE_POLL.to_owned(),
            },
            None => CHANGE_POLL.to_owned(),
        },
        None => non_empty(&pipeline.period)
            .or_else(|| non_empty(&pipeline.schedule))
            .unwrap_or("60s")
            .to_owned(),
    };

    // A page the change gate hashes carries the watched attributes, whatever the query names.
    let mut query = query.clone();
    let watched: Vec<String> = match trigger {
        Some(trigger) if !trigger.subscription.watched_attributes.is_empty() => {
            trigger.subscription.watched_attributes.clone()
        }
        Some(_) => query.attrs.clone(),
        None => Vec::new(),
    };
    if !query.attrs.is_empty() {
        for attr in &watched {
            if !query.attrs.contains(attr) {
                query.attrs.push(attr.clone());
            }
        }
    }
    let source_url = match earlier {
        Some(read) => earlier_source_url(source_slug, &query, read)?,
        None => endpoint_source_url(source_slug, &query)?,
    };

    // Each tick is one run of the pipeline's log (PL-62).
    let input = serde_json::json!({
        "generate": {
            "interval": interval,
            "mapping": format!("root = \"\"\n{}", crate::pipeline_log::TICK)
        }
    });

    // A public endpoint serves the public grant and expects no credential; the runner's token
    // is minted for the endpoints its ServiceAccount is bound to, and the gateway answers 401
    // to a token whose audience is another endpoint's slug — so sending one where none is
    // wanted is how a Live pipeline reads nothing (EP-27, PL-45, T-0914).
    let mut read = serde_json::json!({
        "url": source_url,
        "verb": "GET",
        "headers": {
            "Accept": "application/json"
        },
        "timeout": "30s"
    });
    if !source_is_public {
        read["oauth2"] = pipeline_oauth2(project);
    }
    let p1 = paged_read(&read, &source_url);
    let p2 = serde_json::json!({ "mutation": DROP_ERRORED });

    let mut processors = vec![p1, p2];
    if trigger.is_some() {
        processors.extend(change_gate(stream_key, &watched));
    }
    Ok((input, processors))
}

/// Looks one source up in the mirror: its DataSource, or its Endpoint's slug and audience.
/// Writes each source Endpoint's space segment into the rendered stream (PL-57, PF-84).
///
/// The companion of `jcctl::bento::inject_space`, which does the same for the spaces a pipeline
/// writes. `JC_SOURCE_SPACE` is the first source, `JC_SOURCE_SPACE_2` the second, and a variable
/// with no source behind it is left alone rather than rendered as an empty segment: a mapping
/// then still says `env("JC_SOURCE_SPACE")`, which is a stream the runner refuses out loud,
/// instead of minting a URN with a hole in it that every write is refused for one by one.
pub(super) fn inject_source_space(value: &mut Value, segments: &[String]) {
    match value {
        Value::String(text) => {
            if !text.contains("JC_SOURCE_SPACE") {
                return;
            }
            // Longest name first, so `JC_SOURCE_SPACE` never rewrites the head of
            // `JC_SOURCE_SPACE_2`.
            for (index, segment) in segments.iter().enumerate().rev() {
                if segment.is_empty() {
                    continue;
                }
                let var = if index == 0 {
                    "JC_SOURCE_SPACE".to_owned()
                } else {
                    format!("JC_SOURCE_SPACE_{}", index + 1)
                };
                *text = text
                    .replace(&format!("env(\"{var}\")"), &format!("\"{segment}\""))
                    .replace(&format!("${{{var}}}"), segment);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| inject_source_space(item, segments)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| inject_source_space(item, segments)),
        _ => {}
    }
}

/// The Endpoint a pipeline source reads, from the project its reference names (PL-90, PL-55).
///
/// A typed `endpointRef` may name another project's Endpoint; that Endpoint must admit the
/// pipeline's project by its audience (public, its own organization, or `allowedProjects`), and
/// the gateway decides the read itself on that project's Policies at run time (PL-92).
pub(crate) fn source_endpoint(
    mirror: &Mirror,
    ns: &str,
    reference: &jc_core::Ref,
) -> Result<crate::resource::ResourceEnvelope, String> {
    let name = reference.name();
    let owner = reference.namespace().unwrap_or(ns);
    let endpoint = mirror
        .get(owner, "Endpoint", name)
        .ok_or_else(|| match owner == ns {
            true => format!("source endpoint {name} is not in the mirror"),
            false => format!("source endpoint {owner}/{name} is not in the mirror"),
        })?;
    if owner != ns && !admits(mirror, &endpoint.spec, owner, ns) {
        return Err(format!(
            "source endpoint {owner}/{name} does not admit project {ns}: ask its owner for \
             access (PL-90)"
        ));
    }
    Ok(endpoint)
}

/// Whether an Endpoint of project `owner` admits a reader from project `reader` by its audience;
/// one without an audience admits nobody.
pub(super) fn admits(mirror: &Mirror, spec: &Value, owner: &str, reader: &str) -> bool {
    match spec.get("audience").and_then(Value::as_str) {
        Some("public") => true,
        Some("organization") => {
            crate::permissions::organization_of(mirror, owner)
                == crate::permissions::organization_of(mirror, reader)
        }
        Some("project-list") => spec
            .get("allowedProjects")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|p| p.as_str() == Some(reader))),
        _ => false,
    }
}

pub(super) fn resolve_source(
    mirror: &Mirror,
    ns: &str,
    source: PipelineSource,
) -> Result<ResolvedSource, String> {
    if let Some(reference) = &source.data_source_ref {
        let ds_name = reference.name().to_owned();
        let ds_env = mirror
            .get(ns, "DataSource", &ds_name)
            .ok_or_else(|| format!("data source {ds_name} is not in the mirror"))?;
        let spec: DataSourceSpec = serde_json::from_value(ds_env.spec.clone())
            .map_err(|err| format!("invalid data source spec: {err}"))?;
        return Ok(ResolvedSource::Data {
            spec: Box::new(spec),
            name: ds_name,
        });
    }
    let reference = source
        .endpoint_ref
        .as_ref()
        .ok_or_else(|| "a source names neither a DataSource nor an Endpoint".to_owned())?;
    let ep_name = reference.name().to_owned();
    let ep_env = source_endpoint(mirror, ns, reference)?;
    let slug = ep_env
        .spec
        .get("slug")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("source endpoint {ep_name} has no slug"))?
        .to_owned();
    let public = ep_env
        .spec
        .get("audience")
        .and_then(Value::as_str)
        .is_none_or(|audience| audience == "public");
    Ok(ResolvedSource::Endpoint {
        source: Box::new(source),
        slug,
        public,
    })
}

/// One source of a pipeline, resolved from the mirror for [`render_merged`].
pub enum ResolvedSource {
    /// A DataSource of the project, by name.
    Data {
        /// Its spec.
        spec: Box<DataSourceSpec>,
        /// Its name.
        name: String,
    },
    /// An Endpoint read with a query (PL-31).
    Endpoint {
        /// The pipeline's source entry, with its query and trigger.
        source: Box<PipelineSource>,
        /// The endpoint's slug.
        slug: String,
        /// Whether the endpoint serves the public grant (no token sent).
        public: bool,
    },
}

/// Whether a pipeline needs the general renderer (PL-53): more than one source or output, a
/// processor step, more than one step, or a kpi step, whose reads back are inputs too (PL-117).
/// Everything else renders through [`render_stream`]
/// or [`render_endpoint_stream`], so a `v1alpha1` pipeline renders the bytes it always did.
pub fn is_merged(spec: &PipelineSpec) -> bool {
    let steps = spec.steps();
    spec.sources().len() > 1
        || spec.outputs().len() > 1
        || steps.len() > 1
        || kpi_step(spec).is_some()
        || steps.iter().any(|step| matches!(step, Step::Processor(_)))
}

/// Renders a `v1alpha2` pipeline of several sources, steps or outputs as one stream (PL-53):
/// a `broker` input over the sources, each with the processors its own input needs; the
/// steps in order; the batching; one output, or a `broker` output fanning out to every one.
pub fn render_merged(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    sources: &[ResolvedSource],
    target_slugs: &[String],
) -> Result<Value, RenderError> {
    let kpi = kpi_step(pipeline);
    if kpi.is_some() && !pipeline.is_scheduled() {
        return Err(RenderError::Custom(
            "a kpi step runs on its schedule: set class scheduled and a schedule (PL-102)"
                .to_owned(),
        ));
    }
    let input = merged_input(pipeline, name, project, sources, kpi)?;

    let mut processors = Vec::new();
    // Each step by its place in the manifest (PL-66): the `j`-th processor step is `step_{j}`,
    // the first compute step `compute` and a later `j`-th one `compute_{j}`, which are the
    // studio's own node names; the runner refuses a stream with a label twice.
    let mut processor_steps = 0usize;
    let mut compute_steps = 0usize;
    for step in pipeline.steps() {
        let label = match &step {
            Step::Processor(_) => {
                processor_steps += 1;
                format!("step_{}", processor_steps - 1)
            }
            Step::Compute(_) => {
                compute_steps += 1;
                match compute_steps {
                    1 => COMPUTE_LABEL.to_owned(),
                    n => format!("{COMPUTE_LABEL}_{}", n - 1),
                }
            }
        };
        if let Some(kpi) = kpi {
            processors.push(labelled(kpi_mapping(pipeline, kpi, name, project)?, &label));
            processors.push(labelled(kpi_problem(), KPI_PROBLEM_LABEL));
            continue;
        }
        if let Some(processor) = step_processor(step)? {
            processors.push(labelled(processor, &label));
        }
    }

    // `outputs()` is the default lane's, then each route's in order (PL-73).
    // The slugs follow `outputs()`, so the n-th slug is written in the n-th output's mode.
    let targets: Vec<(String, bool)> = target_slugs
        .iter()
        .enumerate()
        .map(|(at, slug)| (slug.clone(), replaces(pipeline, at)))
        .collect();
    let lane = pipeline.lane_outputs().len().min(targets.len());
    let lane_writes = writes(project, &targets[..lane], "")?;
    let routed = if pipeline.routes.is_empty() {
        None
    } else {
        let (routing, cases) = routes(pipeline, project, &targets[lane..])?;
        processors.extend(routing);
        Some(cases)
    };
    processors.extend(history_step(pipeline)?);
    processors.extend(source_events_step(pipeline));
    processors.extend(lifetime_step(pipeline)?);
    processors.extend(batching());
    match routed {
        None => processors.extend(lane_writes),
        Some(mut cases) => {
            cases.push(serde_json::json!({ "processors": lane_writes }));
            processors.push(labelled(
                serde_json::json!({ "switch": cases }),
                ROUTES_LABEL,
            ));
        }
    }
    let threads = in_flight(sources.iter().map(|source| match source {
        ResolvedSource::Data { spec, .. } => Some(spec.as_ref()),
        ResolvedSource::Endpoint { .. } => None,
    }));

    let mut stream = serde_json::json!({
        "input": labelled(input, "input"),
        "pipeline": {
            "processors": processors
                .into_iter()
                .enumerate()
                .map(|(at, processor)| labelled(processor, &format!("processor_{at}")))
                .collect::<Vec<_>>()
        },
        "output": labelled(written(), "output")
    });
    bound_in_flight(&mut stream, threads);
    Ok(stream)
}

/// The stream's input over every source: one source as it is, several through one broker, a kpi
/// run's pages gathered into one message (PL-104).
fn merged_input(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    sources: &[ResolvedSource],
    kpi: Option<&jc_core::kpi_join::KpiStep>,
) -> Result<Value, RenderError> {
    let mut inputs = Vec::with_capacity(sources.len());
    for (at, source) in sources.iter().enumerate() {
        let (input, processors) = match source {
            ResolvedSource::Data {
                spec,
                name: source_name,
            } => datasource_input(pipeline, name, project, spec, source_name)?,
            ResolvedSource::Endpoint {
                source,
                slug,
                public,
            } => endpoint_input(
                pipeline,
                project,
                Some(source),
                &format!("{project}/{name}#{at}"),
                slug,
                *public,
                None,
            )?,
        };
        let processors = match kpi {
            Some(_) => kpi_source(source, processors, None)?,
            None => processors,
        };
        inputs.push(with_processors(input, processors));
    }
    if let Some(kpi) = kpi {
        inputs.extend(kpi_earlier_inputs(pipeline, name, project, sources, kpi)?);
    }
    let count = inputs.len();
    match count {
        0 => Err(RenderError::Custom(
            "a pipeline reads at least one source".to_owned(),
        )),
        1 => Ok(inputs.remove(0)),
        // The run's pages meet in one message, one wrapped page per source (PL-104).
        _ if kpi.is_some() => Ok(serde_json::json!({ "broker": {
            "inputs": inputs,
            "batching": {
                "count": count,
                "period": KPI_GATHER,
                "processors": [{ "archive": { "format": "json_array" } }],
            },
        } })),
        _ => Ok(serde_json::json!({ "broker": { "inputs": inputs } })),
    }
}

/// One input per read back of a kpi formula (PL-117), each with its source's Endpoint and token.
fn kpi_earlier_inputs(
    pipeline: &PipelineSpec,
    name: &str,
    project: &str,
    sources: &[ResolvedSource],
    kpi: &jc_core::kpi_join::KpiStep,
) -> Result<Vec<Value>, RenderError> {
    let expr = jc_core::kpi_join::parse(&kpi.formula)
        .map_err(|err| RenderError::Custom(err.to_string()))?;
    let mut inputs = Vec::new();
    for read in jc_core::kpi_join::earlier_reads(&expr) {
        for resolved in sources {
            let ResolvedSource::Endpoint {
                source,
                slug,
                public,
            } = resolved
            else {
                continue;
            };
            if source.name.as_deref() != Some(read.input.as_str()) {
                continue;
            }
            let (input, processors) = endpoint_input(
                pipeline,
                project,
                Some(source),
                &format!("{project}/{name}#{}", read.page()),
                slug,
                *public,
                Some(&read),
            )?;
            let processors = kpi_source(resolved, processors, Some(&read))?;
            inputs.push(with_processors(input, processors));
        }
    }
    Ok(inputs)
}

/// How long the broker waits for every source's page of a kpi run before it folds what came;
/// a source still silent then is the run's problem (PL-104).
const KPI_GATHER: &str = "120s";

/// The label of the step that reports a kpi run's problem to its log and drops it (PL-104).
pub const KPI_PROBLEM_LABEL: &str = "kpi_problem";

/// The `kpi` step of a pipeline (PL-102), when it has one.
fn kpi_step(pipeline: &PipelineSpec) -> Option<&jc_core::kpi_join::KpiStep> {
    pipeline.steps.iter().find_map(|step| match step {
        Step::Compute(compute) => compute.kpi.as_deref(),
        Step::Processor(_) => None,
    })
}

/// A kpi source's processors (PL-104): its read keeps the error instead of dropping the page,
/// and the page is wrapped with the source's name for the broker's batch. The error loses every
/// URL query, as the run log's source failure does (T-4428). `earlier` is a read one period back
/// of this source (PL-117), its page named apart and an empty one meaning no history.
fn kpi_source(
    source: &ResolvedSource,
    mut processors: Vec<Value>,
    earlier: Option<&jc_core::kpi_join::Earlier>,
) -> Result<Vec<Value>, RenderError> {
    let ResolvedSource::Endpoint { source, .. } = source else {
        return Err(RenderError::Custom(
            "a kpi step reads Endpoints, not DataSources (PL-102)".to_owned(),
        ));
    };
    if source.trigger.is_some() {
        return Err(RenderError::Custom(
            "a kpi step runs on its schedule, not on a subscription (PL-102)".to_owned(),
        ));
    }
    let name = source.name.as_deref().ok_or_else(|| {
        RenderError::Custom("each source of a kpi step has a name (PL-102)".to_owned())
    })?;
    let (page, empty) = match earlier {
        Some(read) => (read.page(), jc_core::kpi_join::no_history(&read.period)),
        None => (name.to_owned(), EMPTY_KPI_SOURCE.to_owned()),
    };
    processors.retain(|p| p.get("mutation").and_then(Value::as_str) != Some(DROP_ERRORED));
    processors.push(serde_json::json!({ "catch": [{ "mapping": r#"root = { "jc_kpi_error": error().or("the source refused the request").re_replace_all("""\?[^'"\s]*""", "") }"# }] }));
    processors.push(serde_json::json!({ "mapping": format!(
        r#"root = if this.type() == "array" && this.length() == 0 {{ {{ "jc_kpi_error": "{empty}" }} }} else {{ this }}"#
    ) }));
    processors.push(serde_json::json!({ "mapping": jc_core::kpi_join::wrap_mapping(&page) }));
    Ok(processors)
}

/// Why a kpi source with no rows gives no value: a sum over nothing would read as a real 0.
pub const EMPTY_KPI_SOURCE: &str =
    "it has no rows now, so there is no value; check its entity type and its filter";

/// The mapping a kpi step compiles to (PL-104): jc-core checks the step against the sources'
/// names and attributes again, then renders the fold.
fn kpi_mapping(
    pipeline: &PipelineSpec,
    kpi: &jc_core::kpi_join::KpiStep,
    name: &str,
    project: &str,
) -> Result<Value, RenderError> {
    let custom = |err: jc_core::Error| RenderError::Custom(err.to_string());
    let mut inputs = pipeline.kpi_inputs().map_err(custom)?;
    for (input, source) in inputs.iter_mut().zip(&pipeline.sources) {
        if let Some(reference) = &source.endpoint_ref {
            input.endpoint = reference.name().to_owned();
        }
    }
    let expr = kpi.validate(&inputs).map_err(custom)?;
    let target = jc_core::kpi_join::Target {
        pipeline_space: project,
        pipeline: name,
    };
    Ok(serde_json::json!({
        "mapping": jc_core::kpi_join::bloblang(kpi, &expr, &inputs, &target)
    }))
}

/// Stands where a kpi run's problem is reported; the log stage puts the report in (PL-62), and
/// without one the run is dropped, never written.
fn kpi_problem() -> Value {
    serde_json::json!({ "switch": [{
        "check": "errored()",
        "processors": [{ "mapping": "root = deleted()" }],
    }] })
}

/// The label of the output that routes by condition (PL-73). A route's own components are
/// labelled by its place, `route_{i}` its output and `route_{i}_{j}` its `j`-th step, because a
/// Bento label takes no hyphen and a route's name may hold one.
pub const ROUTES_LABEL: &str = "routes";

/// One step as its Bento processor (PL-52): a runner processor verbatim, a `bloblang` step as its
/// `mapping`, nothing for a `bloblang` step whose mapping lives in `bento.yaml`.
fn step_processor(step: Step) -> Result<Option<Value>, RenderError> {
    match step {
        Step::Processor(step) => Ok(Some(serde_json::to_value(&step.processor)?)),
        Step::Compute(compute) if !compute.kind.is_mapping() => Err(RenderError::MissingCompute),
        Step::Compute(compute) => Ok(compute
            .mapping()
            .map_err(|err| RenderError::Custom(err.to_string()))?
            .map(|bloblang| serde_json::json!({ "mapping": bloblang }))),
    }
}

/// Routes by condition (PL-73): the processors that pick each record's route and run its steps,
/// and the `switch` cases that write each route's records through its own outputs (T-4056);
/// `slugs` are the routes' outputs in order.
///
/// The batching archives records into arrays before the output, so the route is picked per
/// record before it: the records are split one per message, `meta jc_route` names the first
/// route whose `when` holds (empty for the default lane), a `switch` processor runs each route's
/// steps, and `group_by_value` keeps every batch to one route, whose `archive` keeps the
/// metadata the writes' `switch` reads.
pub(super) fn routes(
    pipeline: &PipelineSpec,
    project: &str,
    slugs: &[(String, bool)],
) -> Result<(Vec<Value>, Vec<Value>), RenderError> {
    let mut processors: Vec<Value> = batching().into_iter().take(2).collect();
    let mut pick = String::from("meta jc_route = ");
    let mut steps = Vec::new();
    let mut cases = Vec::new();
    let mut rest = slugs;
    for (index, route) in pipeline.routes.iter().enumerate() {
        // The condition on lines of its own, so a comment in it ends with its line.
        pick.push_str(&format!(
            "if (\n{}\n) {{ \"{}\" }} else ",
            route.when.trim(),
            route.name
        ));
        let check = format!("meta(\"jc_route\") == \"{}\"", route.name);
        let mut own = Vec::new();
        for (at, step) in route.steps.iter().cloned().enumerate() {
            if let Some(processor) = step_processor(step)? {
                own.push(labelled(processor, &format!("route_{index}_{at}")));
            }
        }
        if !own.is_empty() {
            steps.push(serde_json::json!({ "check": check, "processors": own }));
        }
        let (written, tail) = rest.split_at(route.outputs.len().min(rest.len()));
        rest = tail;
        cases.push(serde_json::json!({
            "check": check,
            "processors": writes(project, written, &format!("route_{index}_"))?,
        }));
    }
    pick.push_str("{ \"\" }");
    processors.push(labelled(
        serde_json::json!({ "mutation": pick }),
        "route_pick",
    ));
    if !steps.is_empty() {
        processors.push(labelled(
            serde_json::json!({ "switch": steps }),
            "route_steps",
        ));
    }
    processors
        .push(serde_json::json!({ "group_by_value": { "value": "${! meta(\"jc_route\") }" } }));
    Ok((processors, cases))
}

/// A Bento input with the processors that belong to it alone, as every Bento input carries them.
fn with_processors(input: Value, processors: Vec<Value>) -> Value {
    let mut input = input;
    if let (Some(fields), false) = (input.as_object_mut(), processors.is_empty()) {
        fields.insert("processors".to_owned(), Value::Array(processors));
    }
    input
}

/// Whether a rendered input is one that ends on its own (PL-04, PL-50, T-2336).
///
/// A file, an object store or a `select` reads what is there and finishes, so a stream on one of
/// them is done and not dead when the runner reports it inactive. A clock, a socket and a broker
/// never finish, so an inactive stream on one of those died. A broker of several inputs ends only
/// when every one of them does.
pub(super) fn input_ends_by_itself(input: &Value) -> bool {
    if let Some(inputs) = input.get("broker").and_then(|b| b.get("inputs")) {
        return inputs
            .as_array()
            .is_some_and(|inputs| !inputs.is_empty() && inputs.iter().all(input_ends_by_itself));
    }
    let Some(kind) = input.as_object().and_then(|fields| {
        fields
            .keys()
            .map(String::as_str)
            .find(|key| !matches!(*key, "label" | "processors"))
    }) else {
        return false;
    };
    // A clock is in the terminating list because a `count` makes it finite; the clock every
    // pipeline here is rendered with has no count, and an unbounded one ticks forever.
    if kind == "generate" {
        return input["generate"]
            .get("count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0);
    }
    jc_core::kinds::bento_inputs::TERMINATING.contains(&kind)
}
