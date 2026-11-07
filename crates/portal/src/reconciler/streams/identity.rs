//! A pipeline's identity and writes inside its stream (T-5340 split it out of `streams`): the
//! project's pipeline client and its secret variable, the caches it owns, the in-flight bounds
//! and the gateway outputs that write and record refusals. The client secret is only ever named
//! as a `${JC_CLIENT_SECRET_<PROJECT>}` variable, never rendered.

use jc_core::kinds::data_source::{DataSourceSpec, DataSourceType};
use jc_core::kinds::pipeline::{OutputMode, PipelineSpec};
use serde_json::Value;

use super::RenderError;

/// The `ServiceAccount` every project's streams run as. Derived, not configured: a Pipeline
/// names no account, and the seeds declare `pipelines` in each project that has one.
/// ponytail: one account per project; a `spec.serviceAccountRef` on Pipeline is the upgrade.
pub(crate) const PIPELINE_ACCOUNT: &str = "pipelines";

/// The runner environment variable that holds one project's pipeline client secret:
/// `banskabystrica` becomes `JC_CLIENT_SECRET_BANSKABYSTRICA`. A project name is a DNS-1123
/// label, so the only character to translate is the hyphen.
fn client_secret_var(project: &str) -> String {
    let mut name = String::from("JC_CLIENT_SECRET_");
    for ch in project.chars() {
        name.push(if ch.is_ascii_alphanumeric() {
            ch.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    name
}

/// The credential a stream presents, on the read of a private source and on every write: its
/// own project's `pipelines` ServiceAccount (PF-46, AG-52).
///
/// One runner-wide `${JC_CLIENT_ID}` used to stand here, so every stream of every project
/// presented Helsinki's client and the gateway refused each write to another project's endpoint
/// with `token is not bound to this resource` — about 1900 refusals in 45 minutes on dev
/// (T-2384). The client id is `jc_core`'s derived `{project}-{account}` and is an `azp`, not a
/// secret, so it is written into the stream; the secret is never written, the stream names the
/// environment variable the deployment resolves from that client's Secret.
pub(crate) fn pipeline_oauth2(project: &str) -> serde_json::Value {
    serde_json::json!({
        "enabled": true,
        "client_key": jc_core::kinds::service_account::keycloak_client_id(project, PIPELINE_ACCOUNT),
        "client_secret": format!("${{{}}}", client_secret_var(project)),
        "token_url": "${JC_TOKEN_URL}"
    })
}

/// The client id a stream names to the runner's token sidecar: its pipeline, `{project}/{pipeline}`.
pub fn sidecar_client(project: &str, pipeline: &str) -> String {
    format!("{project}/{pipeline}")
}

/// Turns every credential of `project`'s `pipelines` account in a rendered stream into its own
/// pipeline's (PL-19): the stream names `{project}/{pipeline}` to the runner's token sidecar at
/// `${JC_PIPELINE_TOKEN_URL}`, which presents that pipeline's own Kubernetes token to Keycloak.
/// No secret is named, not even a placeholder: the sidecar reads the client id from Basic and
/// wants none, and a stream without the key has none for the runner's API to serve (T-4044).
/// The sinks to the Portal present the project's client too (T-4038), so they become the
/// pipeline's as well, which the internal listener admits (T-3193).
pub fn with_pipeline_identity(stream: &mut Value, project: &str, pipeline: &str) {
    let account = jc_core::kinds::service_account::keycloak_client_id(project, PIPELINE_ACCOUNT);
    match stream {
        Value::Object(map) => {
            if let Some(oauth2) = map.get_mut("oauth2") {
                if oauth2.get("client_key").and_then(Value::as_str) == Some(account.as_str()) {
                    *oauth2 = serde_json::json!({
                        "enabled": true,
                        "client_key": sidecar_client(project, pipeline),
                        "token_url": "${JC_PIPELINE_TOKEN_URL}"
                    });
                }
            }
            for value in map.values_mut() {
                with_pipeline_identity(value, project, pipeline);
            }
        }
        Value::Array(items) => {
            for item in items {
                with_pipeline_identity(item, project, pipeline);
            }
        }
        _ => {}
    }
}

/// Makes a rendered stream its project's own before the runner gets it: the credential
/// ([`own_client`]) and every cache it names ([`own_caches`]).
pub(crate) fn own_credential(value: &mut Value, project: &str) {
    own_client(value, project);
    own_caches(value, project);
}

/// Rewrites an author's `${JC_CLIENT_ID}` and `${JC_CLIENT_SECRET}` into the stream's own
/// project's pipeline client (PL-16, T-3163).
///
/// The runner's environment holds every project's client secret, and its plain
/// `JC_CLIENT_SECRET` is one project's, so a step that wrote it as it stands would present that
/// project's client from any other. jc-core lets a step name only these two aliases of a
/// credential; here they become the same client [`pipeline_oauth2`] renders for the project.
fn own_client(value: &mut Value, project: &str) {
    match value {
        Value::String(text) if text.contains("${JC_CLIENT_") => {
            *text = text
                .replace(
                    "${JC_CLIENT_ID}",
                    &jc_core::kinds::service_account::keycloak_client_id(project, PIPELINE_ACCOUNT),
                )
                .replace(
                    "${JC_CLIENT_SECRET}",
                    &format!("${{{}}}", client_secret_var(project)),
                );
        }
        Value::Array(items) => items.iter_mut().for_each(|item| own_client(item, project)),
        Value::Object(map) => map.values_mut().for_each(|item| own_client(item, project)),
        _ => {}
    }
}

/// The processors that name a runner cache, and the field that holds its label.
const CACHE_FIELDS: &[(&str, &str)] = &[
    ("cache", "resource"),
    ("cached", "cache"),
    ("dedupe", "cache"),
];

/// The label one project's cache `name` has on the runner (PL-50, T-5337): `<project>__<name>`,
/// the project with `-` as `_`, since a Bento label holds only `[a-z0-9_]`. jc-core refuses a
/// step's cache name with `__` or an edge `_`, so no two projects' labels meet.
pub(super) fn cache_label(project: &str, name: &str) -> String {
    format!("{}__{name}", project.replace('-', "_"))
}

/// The rate limit every gateway write of a stream waits on, as a step names it.
pub(crate) const EGRESS: &str = "pipeline_egress";

/// How many gateway calls a second one project's streams make at most, all of them together:
/// the budget the runner's one shared `pipeline_egress` gave every project at once.
pub(crate) const EGRESS_PER_SECOND: u64 = 500;

/// The project's own [`EGRESS`] on the runner, `<project>__pipeline_egress` (F-BNT3-002,
/// T-5803): one bucket per project, so a project at its cap leaves every other project's rate
/// untouched. The Portal declares it ([`declare_egress`]) before it sends the project a stream.
pub(crate) fn egress_label(project: &str) -> String {
    cache_label(project, EGRESS)
}

/// Declares the project's [`egress_label`] on the runner: Bento refuses a stream naming a rate
/// limit it does not hold (502, "rate limit resource ... was not found"), and the resources file
/// it starts with names no project, so one created in the Portal has its bucket too. Bento's
/// `POST /resources/rate_limit/{label}` adds or replaces it; a stream already waiting on it goes
/// on with the new one. `Ok(Some(why))` is a runner that refused it, `Err` one that gave no
/// answer, which the caller treats as it treats a stream create that got none.
/// ponytail: declared before every create, which refills the project's bucket once per send;
/// keep a declared set per runner if a project's sends ever come faster than its runs.
pub(crate) async fn declare_egress(
    http: &reqwest::Client,
    runner: &str,
    project: &str,
) -> Result<Option<String>, String> {
    if !crate::resource::is_dns1123(project) {
        return Ok(Some(format!(
            "the project name `{project}` is not a DNS-1123 label, so it has no write budget"
        )));
    }
    let label = egress_label(project);
    let url = format!(
        "{}/resources/rate_limit/{label}",
        runner.trim_end_matches('/')
    );
    let local = serde_json::json!({ "local": { "count": EGRESS_PER_SECOND, "interval": "1s" } });
    let answer = http
        .post(&url)
        .json(&local)
        .send()
        .await
        .map_err(|err| format!("the runner did not take the write budget `{label}`: {err}"))?;
    Ok((!answer.status().is_success()).then(|| {
        format!(
            "the runner refused the write budget `{label}`: {}",
            answer.status()
        )
    }))
}

/// Puts every cache a stream names, the platform's own [`CHANGE_CACHE`] among them, under its
/// project's prefix (PL-50, T-5337). One runner may carry every project's streams, and the
/// caches its resources file declares are visible to each of them: under the bare name one
/// project's step read and overwrote what another's cached. A label already under the prefix
/// stays as it is: the reconciler owns a stream when it renders its reporting and again before it
/// sends it, and the second pass named `helsinki__helsinki__departures`, which no runner declares.
fn own_caches(value: &mut Value, project: &str) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|item| own_caches(item, project)),
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                let field = CACHE_FIELDS
                    .iter()
                    .find(|(processor, _)| processor == key)
                    .map(|(_, at)| *at);
                if let Some(Value::String(name)) = field.and_then(|at| inner.get_mut(at)) {
                    if !name.starts_with(&cache_label(project, "")) {
                        *name = cache_label(project, name);
                    }
                }
                if key == "rate_limit" {
                    own_egress(inner, project);
                }
                own_caches(inner, project);
            }
        }
        _ => {}
    }
}

/// Turns a step's `rate_limit` naming [`EGRESS`], or any project's `<project>__pipeline_egress`,
/// into this project's [`egress_label`]: an `http` step's `rate_limit: pipeline_egress` and a
/// `rate_limit: { resource: ... }` processor alike. Another project's bucket is never a step's to
/// drain. A name holding `${` is the runner's to interpolate (`${JC_SPACE_9:helsinki__pipeline_egress}`
/// with the variable unset is helsinki's bucket), so it becomes this project's budget too. Every
/// other rate limit (a source host's own pace) stays as written.
fn own_egress(value: &mut Value, project: &str) {
    let name = match value {
        Value::String(name) => name,
        Value::Object(map) => match map.get_mut("resource") {
            Some(Value::String(name)) => name,
            _ => return,
        },
        _ => return,
    };
    if name.contains(EGRESS) || name.contains("${") {
        *name = egress_label(project);
    }
}

/// How many writes of a polled stream may wait on the gateway at once (T-4069). A poll's page
/// is up to 1000 entities per request, and Bento's default of 64 in flight let every stream hold
/// 64 pages while a slow gateway timed them out and they were retried, which grew the runner
/// until it was OOMKilled. A polled source loses nothing by waiting: its input stops reading
/// while the slots are full, and the next poll reads the newer state. A resident source (MQTT,
/// WebSocket) has its own window ([`in_flight`]), since its messages arrive whether or not it
/// reads them.
/// ponytail: one bound for every polled stream; size it per stream when one needs more.
pub(super) const POLLED_IN_FLIGHT: u64 = 4;

/// How many writes of a resident stream wait on the gateway at once when the source is not read
/// in batches: the 64 Bento's `http_client` output kept in flight when the write was the output
/// (T-4036). The write is a processor now (T-4056), so the pipeline's threads are its window: one
/// thread is one write waiting, and at the default (`GOMAXPROCS`, one on dev's runner) a feed of
/// single records would be written one at a time. Measured with Bento 1.22 and a stub gateway: 64
/// threads write about 150 a second at 0.42 s a write and keep up with 500 a second at 30 ms.
pub(super) const WRITES_IN_FLIGHT: u64 = 64;

/// How many writes of a resident source read in batches (MQTT, WebSocket, T-5046) wait on the
/// gateway at once: one. A batch is one upsert of up to 1000 records, so one write a second
/// carries the HSL feed; 64 batches in flight held overlapping vehicle ids each, waited on each
/// other's row locks past the broker's 5 s `lock_timeout`, failed with 500 and were replayed
/// (T-5636: 2.7 s per UPDATE, Postgres at 1.9 cores). One at a time also writes a vehicle's
/// positions in the order they came, so a late batch never sets an older position.
pub(super) const BATCHES_IN_FLIGHT: u64 = 1;

/// How many writes a stream reading `sources` keeps waiting on the gateway: the window of an
/// unbatched resident source if it reads one, else [`BATCHES_IN_FLIGHT`] if it reads a batched
/// one, else [`POLLED_IN_FLIGHT`]. `None` is an Endpoint source, which polls on a clock.
pub(super) fn in_flight<'a>(sources: impl IntoIterator<Item = Option<&'a DataSourceSpec>>) -> u64 {
    let mut threads = POLLED_IN_FLIGHT;
    for source in sources.into_iter().flatten() {
        if source.terminates() {
            continue;
        }
        if !reads_in_batches(source) {
            return WRITES_IN_FLIGHT;
        }
        threads = BATCHES_IN_FLIGHT;
    }
    threads
}

/// Whether a resident source is read through the broker input that batches it (T-5046).
pub(super) fn reads_in_batches(source: &DataSourceSpec) -> bool {
    matches!(
        source.source_type,
        DataSourceType::Mqtt | DataSourceType::WebSocket
    )
}

/// Sets the stream's write window to `threads` (see [`in_flight`]).
pub(super) fn bound_in_flight(stream: &mut Value, threads: u64) {
    if let Some(pipeline) = stream.get_mut("pipeline").and_then(Value::as_object_mut) {
        pipeline.insert("threads".to_owned(), threads.into());
    }
}

/// Where every stream writes: the endpoint's batch upsert, as the pipeline's service account.
/// No pipeline answers a caller: a client-facing API is the Context Gateway's (PL-06, EP-05).
/// `replace` drops `options=update`, so the entity becomes exactly the record (PL-79); every
/// other write merges the attributes it names into what is there.
///
/// An answer no retry can fix (a malformed entity, a batch too large) is not retried for good,
/// which stalled the messages behind it and flooded the gateway's log (T-1465). A token, a
/// policy or the gateway can recover, so 401, 403, 429 and 5xx stay retried.
pub(super) fn upsert(project: &str, slug: &str, replace: bool) -> Value {
    let options = if replace { "" } else { "?options=update" };
    serde_json::json!({
        "url": format!("${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/upsert{options}"),
        "verb": "POST",
        "headers": {
            "Content-Type": "application/json"
        },
        "oauth2": pipeline_oauth2(project),
        "timeout": "30s",
        "rate_limit": egress_label(project),
        "drop_on": [400, 413, 422]
    })
}

/// The upsert as an output, for a replay of rejected records, whose answer no run log waits on.
pub(crate) fn gateway_output(project: &str, slug: &str) -> serde_json::Value {
    serde_json::json!({ "http_client": upsert(project, slug, false) })
}

/// The label of a stream's write to its `at`-th target, `prefix` naming the route it belongs to.
pub(super) fn write_label(prefix: &str, at: usize) -> String {
    format!("{prefix}write_{at}")
}

/// Whether a component label is one of [`write_label`]'s: its latency is the stream's write
/// latency (T-4056), the refusals switch beside it is not.
pub(crate) fn is_write_label(label: &str) -> bool {
    label.rsplit_once("write_").is_some_and(|(prefix, at)| {
        (prefix.is_empty() || prefix.ends_with('_'))
            && !at.is_empty()
            && at.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The metadata key a write leaves the records its target refused in (T-4056), read by the
/// outcome sink ([`crate::pipeline_log::REFUSED_PREFIX`]).
fn refused_key(label: &str) -> String {
    format!("{}{label}", crate::pipeline_log::REFUSED_PREFIX)
}

/// Whether the `at`-th output of `outputs()` replaces its entities (PL-79).
pub(super) fn replaces(pipeline: &PipelineSpec, at: usize) -> bool {
    pipeline
        .outputs()
        .get(at)
        .is_some_and(|output| output.mode == Some(OutputMode::Replace))
}

/// Whether any output replaces its entities (PL-79): a changed stream then runs at once (T-5824).
pub(super) fn replaces_any(pipeline: &PipelineSpec) -> bool {
    pipeline
        .outputs()
        .iter()
        .any(|output| output.mode == Some(OutputMode::Replace))
}

/// The writes to `targets` (an Endpoint slug and whether its output replaces the entity, PL-79),
/// one after the other, each a `branch` around the upsert (T-4056).
///
/// Bento's `http_client` output never shows its answer to anything, so a `207` whose items the
/// endpoint refused was logged as sent. As a processor the write keeps the answer: the branch's
/// `result_map` leaves each refused item's id and the gateway's words for it in metadata, and
/// the outcome sink logs those records `failed` instead of `sent`. A write that still fails after
/// its retries leaves the message errored, and [`written`] nacks it back to the input, which
/// replays it (T-2983) as the output's failure did.
pub(super) fn writes(
    project: &str,
    targets: &[(String, bool)],
    prefix: &str,
) -> Result<Vec<Value>, RenderError> {
    if targets.is_empty() {
        return Err(RenderError::Custom(
            "a pipeline writes through at least one output".to_owned(),
        ));
    }
    let mut processors: Vec<Value> = targets
        .iter()
        .enumerate()
        .map(|(at, (slug, replace))| {
            let label = write_label(prefix, at);
            let key = refused_key(&label);
            serde_json::json!({
                "label": label,
                "branch": {
                    "processors": [{ "http": upsert(project, slug, *replace) }],
                    "result_map": format!(
                        "let errors = content().parse_json().catch({{}}).errors.or([])\n\
                         meta {key} = $errors.map_each(e -> {{ \"id\": e.entityId.or(\"\"), \
                         \"message\": e.error.detail.or(e.error.title).or(\"\") }}).format_json(no_indent: true)"
                    ),
                }
            })
        })
        .collect();
    processors.push(refusals(prefix));
    Ok(processors)
}

/// A batch the gateway answered `400`, `413` or `422` is not retried (the upsert's `drop_on`):
/// its error is cleared and every record of it is logged `failed` with the gateway's words, the
/// problem's `detail` or `title` (T-4056). Any other failure stays an error for [`written`].
pub(super) fn refusals(prefix: &str) -> Value {
    serde_json::json!({
        "label": format!("{prefix}write_refused"),
        "switch": [{
            "check": r#"errored() && error().re_match("unexpected response code \\((400|413|422)\\)")"#,
            "processors": [{ "catch": [{ "mutation": format!(
                "let problem = error().re_replace_all(\"(?s)^.*?Error: \", \"\").parse_json().catch({{}})\n\
                 meta {} = $problem.detail.or($problem.title).or(error())",
                crate::pipeline_log::FAILED_KEY
            ) }] }],
        }],
    })
}

/// The stream's output once its writes ran (T-4056): a message whose write failed is nacked back
/// to the input, which replays it to every target, each an idempotent upsert (PL-53, T-2983); a
/// written one goes on to the outcome sink, which the runner's deployer puts after it.
pub(super) fn written() -> Value {
    serde_json::json!({ "switch": {
        "retry_until_success": false,
        "cases": [
            { "check": "errored()", "output": { "reject": "${! error() }" } },
            { "output": { "drop": {} } },
        ],
    }})
}
