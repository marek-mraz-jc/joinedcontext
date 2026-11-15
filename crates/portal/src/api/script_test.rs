//! `POST /api/v1/projects/{project}/scripts/test` (PL-118, T-7084): a Script version's tests,
//! run on the project's runner in the same sandbox the live stream uses, before anybody
//! proposes it.
//!
//! Each test is one PL-43 harness run: the test's input as the one message, the version's code as
//! the one `javascript` step, and what came out compared with the test's `expected`. A run per
//! test keeps each result tied to its own input, because the harness trace keeps the outputs and
//! the errors of one run apart. Nothing reaches an Endpoint, and neither the code nor a test's
//! values is logged.

use std::time::Instant;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::script::{MAX_CODE_BYTES, MAX_TESTS};
use jc_core::kinds::{PipelineSpec, ScriptLimits, ScriptTest, Verb};
use jcctl::pipeline_test::{Sample, SampleFormat};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The largest test input or expected record, as JSON text.
const MAX_RECORD_BYTES: usize = 64 * 1024;

/// What a Script test runs: the code of one version, its limits and its tests.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptTestRequest {
    /// The version's source, at most 64 KiB.
    pub code: String,
    /// Time and memory per message; absent, the runner's defaults.
    #[serde(default, deserialize_with = "limits_of_request")]
    #[schema(value_type = Option<ScriptLimitsBody>)]
    pub limits: Option<ScriptLimits>,
    /// One to 20 tests, each `{name, input, expected}`.
    #[schema(value_type = Vec<ScriptTestBody>)]
    pub tests: Vec<ScriptTest>,
}

/// jc-core's `ScriptLimits` as the document publishes it (PL-118).
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScriptLimitsBody {
    /// Milliseconds per message, 1 to 200, e.g. `50`.
    timeout_ms: Option<u32>,
    /// MiB per message, 1 to 64, e.g. `32`.
    memory_mb: Option<u32>,
}

fn limits_of_request<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ScriptLimits>, D::Error> {
    Option::<ScriptLimitsBody>::deserialize(deserializer).map(|body| {
        body.map(|b| ScriptLimits {
            timeout_ms: b.timeout_ms,
            memory_mb: b.memory_mb,
        })
    })
}

/// jc-core's `ScriptTest` as the document publishes it (PL-118); only the schema reads it.
#[derive(ToSchema)]
#[schema(as = ScriptTestBody)]
pub struct ScriptTestBody {
    /// A short name, e.g. `converts-one-point`.
    pub name: String,
    /// The input message, as JSON.
    pub input: Value,
    /// The message the script must produce, as JSON.
    pub expected: Value,
}

/// One test's result.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScriptTestResult {
    /// The test's name.
    pub name: String,
    /// Whether the script made exactly `expected` of `input`.
    pub ok: bool,
    /// What the script made of the input, when it made something.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// Why the script failed on the input (a throw, the time or memory limit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The run on the runner, stream start to answer, in milliseconds.
    pub millis: u64,
}

/// Every test's result, in the order the tests were sent.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScriptTestAnswer {
    pub results: Vec<ScriptTestResult>,
}

fn invalid(field: &str, detail: String) -> ApiError {
    ApiError::Unprocessable {
        detail,
        errors: vec![field.to_owned()],
    }
}

/// The request's own limits, each refusal naming its field.
fn checked(request: &ScriptTestRequest) -> Result<(), ApiError> {
    if request.code.trim().is_empty() || request.code.len() > MAX_CODE_BYTES {
        return Err(invalid(
            "code",
            format!("the code is 1 to {MAX_CODE_BYTES} bytes"),
        ));
    }
    if request.tests.is_empty() || request.tests.len() > MAX_TESTS {
        return Err(invalid(
            "tests",
            format!("send 1 to {MAX_TESTS} tests, each {{name, input, expected}}"),
        ));
    }
    let limits = request.limits.unwrap_or_default();
    let bounded = |field: &str, value: Option<u32>, max: u32| match value {
        Some(v) if v == 0 || v > max => Err(invalid(field, format!("{v} is outside 1 to {max}"))),
        _ => Ok(()),
    };
    bounded(
        "limits.timeoutMs",
        limits.timeout_ms,
        jc_core::kinds::script::MAX_TIMEOUT_MS,
    )?;
    bounded(
        "limits.memoryMb",
        limits.memory_mb,
        jc_core::kinds::script::MAX_MEMORY_MB,
    )?;
    for (i, test) in request.tests.iter().enumerate() {
        for (member, value) in [("input", &test.input), ("expected", &test.expected)] {
            if value.to_string().len() > MAX_RECORD_BYTES {
                return Err(invalid(
                    &format!("tests[{i}].{member}"),
                    format!("a test record is at most {MAX_RECORD_BYTES} bytes of JSON"),
                ));
            }
        }
    }
    Ok(())
}

/// The candidate stream of one test: the code as the one step, writing nowhere.
fn spec_of(request: &ScriptTestRequest) -> Result<PipelineSpec, ApiError> {
    let limits = request.limits.unwrap_or_default();
    serde_json::from_value(json!({
        "class": "auto",
        "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "script-test" } }],
        "steps": [{ "processor": { "javascript": {
            "code": request.code,
            "timeout_ms": limits.timeout_ms(),
            "memory_mb": limits.memory_mb(),
        } } }],
        "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe" }]
    }))
    .map_err(|e| ApiError::Internal(e.to_string()))
}

/// The result of one test from its run's trace.
fn result_of(
    test: &ScriptTest,
    trace: &jcctl::pipeline_test::TestTrace,
    millis: u64,
) -> ScriptTestResult {
    let output = trace.mapping.first().cloned();
    let error = trace.errors.first().map(|e| e.message.clone()).or_else(|| {
        output
            .is_none()
            .then(|| "the script made no message of the input".to_owned())
    });
    ScriptTestResult {
        name: test.name.clone(),
        ok: error.is_none() && output.as_ref() == Some(&test.expected),
        output,
        error,
        millis,
    }
}

/// `POST /api/v1/projects/{project}/scripts/test` (PL-118, T-7084).
#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/scripts/test",
    summary = "Script Test",
    description = "Runs a Script version's tests on the project's runner, in the sandbox the live stream uses, without writing.",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project slug")),
    request_body(
        content = ScriptTestRequest,
        description = "`code`: the version's source. `limits`: `timeoutMs` and `memoryMb`. \
                       `tests`: 1 to 20 `{name, input, expected}`.",
        content_type = "application/json",
        example = json!({
            "code": "let m = bento.v0_msg_as_structured(); m.ok = true; bento.v0_msg_set_structured(m);",
            "tests": [{ "name": "flags-one", "input": { "id": 1 }, "expected": { "id": 1, "ok": true } }]
        })
    ),
    responses(
        (status = 200, description = "Each test's result, in the order sent", body = ScriptTestAnswer),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No grant proposes a Script in this project", body = ProblemDetails),
        (status = 404, description = "No such project", body = ProblemDetails),
        (status = 409, description = "A test of this project is already running", body = ProblemDetails),
        (status = 422, description = "The code, a limit or a test is outside its bounds, naming the field", body = ProblemDetails),
        (status = 503, description = "No pipeline runner is configured, or it did not answer", body = ProblemDetails)
    )
)]
pub async fn test_script(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<ScriptTestRequest>,
) -> Result<Json<ScriptTestAnswer>, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "Script",
        Verb::Propose,
        None,
    )?;
    checked(&request)?;
    let spec = spec_of(&request)?;
    let mut results = Vec::with_capacity(request.tests.len());
    for test in &request.tests {
        let sample = Sample {
            text: Some(test.input.to_string()),
            url: None,
            format: SampleFormat::Json,
            records: None,
        };
        let started = Instant::now();
        let trace =
            crate::api::pipeline_test::run_harness(&state, &project, &spec, &sample).await?;
        let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        results.push(result_of(test, &trace, millis));
    }
    Ok(Json(ScriptTestAnswer { results }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/scripts/test", post(test_script))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcctl::pipeline_test::{TestError, TestTrace};

    fn request(value: Value) -> ScriptTestRequest {
        serde_json::from_value(value).expect("request")
    }

    fn field_of(err: ApiError) -> Vec<String> {
        match err {
            ApiError::Unprocessable { errors, .. } => errors,
            other => panic!("unexpected {other:?}"),
        }
    }

    const CODE: &str = "bento.v0_msg_set_structured({ ok: true })";

    /// T-7084: each bound of the request is refused naming its field.
    #[test]
    fn each_bound_is_refused_by_its_field() {
        let test = json!({ "name": "a", "input": {}, "expected": {} });
        let big = "x".repeat(MAX_CODE_BYTES + 1);
        let cases = [
            (json!({ "code": " ", "tests": [test] }), "code"),
            (json!({ "code": big, "tests": [test] }), "code"),
            (json!({ "code": CODE, "tests": [] }), "tests"),
            (
                json!({ "code": CODE, "tests": vec![test.clone(); 21] }),
                "tests",
            ),
            (
                json!({ "code": CODE, "limits": { "timeoutMs": 500 }, "tests": [test] }),
                "limits.timeoutMs",
            ),
            (
                json!({ "code": CODE, "limits": { "memoryMb": 0 }, "tests": [test] }),
                "limits.memoryMb",
            ),
            (
                json!({ "code": CODE, "tests": [{ "name": "a", "input": { "x": "y".repeat(MAX_RECORD_BYTES) }, "expected": {} }] }),
                "tests[0].input",
            ),
        ];
        for (body, field) in cases {
            assert_eq!(
                field_of(checked(&request(body)).expect_err(field)),
                vec![field.to_owned()]
            );
        }
        checked(&request(json!({ "code": CODE, "tests": [test] }))).expect("in bounds");
        assert!(serde_json::from_value::<ScriptTestRequest>(
            json!({ "code": CODE, "tests": [], "file": "x" })
        )
        .is_err());
    }

    /// T-7084: the candidate stream runs the code as its one step, with the limits asked for.
    #[test]
    fn the_candidate_runs_the_code_as_its_one_step() {
        let spec = spec_of(&request(
            json!({ "code": CODE, "limits": { "timeoutMs": 120 },
            "tests": [{ "name": "a", "input": {}, "expected": {} }] }),
        ))
        .expect("spec");
        let step = serde_json::to_value(&spec.steps[0]).expect("step");
        assert_eq!(step["processor"]["javascript"]["code"], CODE);
        assert_eq!(step["processor"]["javascript"]["timeout_ms"], 120);
        assert_eq!(step["processor"]["javascript"]["memory_mb"], 32);
    }

    /// T-7084: a test passes on exactly the expected record; a throw or another record fails.
    #[test]
    fn a_result_compares_the_output_with_expected() {
        let test: ScriptTest =
            serde_json::from_value(json!({ "name": "a", "input": {}, "expected": { "ok": true } }))
                .expect("test");
        let made = |output: Value| TestTrace {
            mapping: vec![output],
            ..TestTrace::default()
        };
        assert!(result_of(&test, &made(json!({ "ok": true })), 3).ok);
        let other = result_of(&test, &made(json!({ "ok": false })), 3);
        assert!(!other.ok && other.error.is_none());
        let thrown = TestTrace {
            errors: vec![TestError {
                stage: "mapping".into(),
                step: Some(0),
                line: None,
                message: "script timed out after 50 ms".into(),
            }],
            ..TestTrace::default()
        };
        let failed = result_of(&test, &thrown, 51);
        assert!(!failed.ok);
        assert_eq!(
            failed.error.as_deref(),
            Some("script timed out after 50 ms")
        );
        let nothing = result_of(&test, &TestTrace::default(), 1);
        assert!(!nothing.ok && nothing.error.is_some());
    }
}
