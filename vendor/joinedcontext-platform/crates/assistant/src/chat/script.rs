//! A script the model writes over a large tool result, run in `jc-functions` with no network
//! (AG-112, T-3056, ADR-N-040 §3.6): the data goes in as the request body, the script's return
//! value comes back, and nothing else crosses.

use serde_json::{json, Value};

/// The most of the sandbox's answer the assistant holds (T-5510): the gateway's own body cap.
const MAX_OUTCOME_BYTES: usize = 8 * 1024 * 1024;

/// The longest script the model may write, in characters.
pub const MAX_CODE_CHARS: usize = 4_000;

/// The code of the one file `jc-functions` runs: the model's body inside an async function of
/// `data`, its return value as the answer's body. Escaping the wrapper gains nothing: the whole
/// file runs in the same sandbox.
pub fn file(code: &str) -> String {
    format!(
        "export default async (request) => {{\n  const data = request.body;\n  const result = await (async () => {{\n{code}\n  }})();\n  return {{ body: result === undefined ? null : result }};\n}};\n"
    )
}

/// The data a result is handed in as: its JSON when it is JSON, its text otherwise.
pub fn data_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// Runs `code` over `data` in `jc-functions` at `functions` with the service's `token`; the
/// script's output as text, or why there is none, in words for the model and the person.
pub async fn run(
    http: &reqwest::Client,
    functions: &str,
    token: &str,
    code: &str,
    data: Value,
) -> Result<String, String> {
    let body = json!({
        "files": { "script.js": file(code) },
        "entry": "script.js",
        "request": { "method": "POST", "query": {}, "body": data, "user": null },
        "config": {},
        "via": "none",
    });
    let answer = http
        .post(format!("{functions}/invoke"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|err| format!("the sandbox could not be reached: {}", err.without_url()))?;
    let status = answer.status();
    let body = jc_core::outbound::read_capped(answer, MAX_OUTCOME_BYTES)
        .await
        .map_err(|err| format!("the sandbox's answer was refused: {err}"))?;
    let outcome: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = outcome
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!(
            "the sandbox refused the script ({status}): {detail}"
        ));
    }
    if let Some(message) = outcome.pointer("/error/message").and_then(Value::as_str) {
        return Err(format!("the script failed: {message}"));
    }
    Ok(match outcome.get("body") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => "null".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_the_body_of_a_function_of_data() {
        let file = file("return data.length;");
        assert!(file.starts_with("export default async (request) => {"));
        assert!(file.contains("const data = request.body;"));
        assert!(file.contains("return data.length;"));
        assert_eq!(data_of("[1,2]"), json!([1, 2]));
        assert_eq!(data_of("plain text"), json!("plain text"));
    }

    async fn sandbox(status: u16, body: Value) -> wiremock::MockServer {
        use wiremock::matchers::{header, method, path};
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(method("POST"))
            .and(path("/invoke"))
            .and(header("authorization", "Bearer the-token"))
            .respond_with(wiremock::ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        server
    }

    async fn ran(server: &wiremock::MockServer) -> Result<String, String> {
        run(
            &reqwest::Client::new(),
            &server.uri(),
            "the-token",
            "return data;",
            json!([1]),
        )
        .await
    }

    #[tokio::test]
    async fn a_text_body_comes_back_as_it_is() {
        let server = sandbox(200, json!({ "body": "three rows" })).await;
        assert_eq!(ran(&server).await, Ok("three rows".to_owned()));
        let sent: Value = server.received_requests().await.expect("recorded")[0]
            .body_json()
            .expect("a JSON request");
        assert_eq!(sent["entry"], "script.js");
        assert_eq!(sent["request"]["body"], json!([1]));
        assert_eq!(sent["via"], "none");
    }

    #[tokio::test]
    async fn a_json_body_comes_back_as_its_text_and_no_body_as_null() {
        let server = sandbox(200, json!({ "body": { "n": 2 } })).await;
        assert_eq!(ran(&server).await, Ok(r#"{"n":2}"#.to_owned()));
        let server = sandbox(200, json!({})).await;
        assert_eq!(ran(&server).await, Ok("null".to_owned()));
    }

    #[tokio::test]
    async fn a_refusal_names_the_status_and_the_sandboxs_reason() {
        let server = sandbox(422, json!({ "detail": "the script is too long" })).await;
        let err = ran(&server).await.expect_err("refused");
        assert!(err.contains("422"), "{err}");
        assert!(err.contains("the script is too long"), "{err}");
        let server = sandbox(500, json!("not an object")).await;
        let err = ran(&server).await.expect_err("refused");
        assert!(err.contains("no reason given"), "{err}");
    }

    #[tokio::test]
    async fn a_thrown_error_is_the_scripts_failure() {
        let server = sandbox(200, json!({ "error": { "message": "data is not defined" } })).await;
        assert_eq!(
            ran(&server).await,
            Err("the script failed: data is not defined".to_owned())
        );
    }

    #[tokio::test]
    async fn an_unreachable_sandbox_is_said_without_its_address() {
        let err = run(
            &reqwest::Client::new(),
            "http://sandbox.invalid",
            "the-token",
            "return 1;",
            Value::Null,
        )
        .await
        .expect_err("nothing listens");
        assert!(err.starts_with("the sandbox could not be reached"), "{err}");
        assert!(!err.contains("sandbox.invalid"), "{err}");
    }

    #[tokio::test]
    async fn an_answer_over_the_cap_is_refused() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("x".repeat(MAX_OUTCOME_BYTES + 1)),
            )
            .mount(&server)
            .await;
        let err = ran(&server).await.expect_err("over the cap");
        assert!(err.starts_with("the sandbox's answer was refused"), "{err}");
    }
}
