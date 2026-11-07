//! Completions through the proxy and what the prompts carry: the schema and the samples (AP-56, AP-57).

use super::*;

/// The pause before a model call the provider could not answer is asked again.
const RETRY_AFTER_MS: u64 = 1000;
/// The longest a provider's own `Retry-After` is waited for: a person waits in front of it.
const RETRY_AFTER_MAX_MS: u64 = 5000;

/// The limit a run reached, when the proxy's refusal names one of the profile's (AG-51, T-2997):
/// `stepsPerRun` counts the run's model calls and `maxTokensPerRun` its tokens. Anything else
/// the proxy or the provider says with a 429 is `None`, a busy service.
fn run_limit_reached(body: &str) -> Option<String> {
    let problem = serde_json::from_str::<Value>(body).ok()?;
    let detail = problem.get("detail")?.as_str()?.to_owned();
    // A daily budget (AG-97) is said by the proxy in words for the person: whose budget, and
    // when it starts again. Asked again it is refused again.
    if problem
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.ends_with("/daily-budget"))
    {
        return Some(detail);
    }
    let limit = if detail.contains("stepsPerRun") || detail.starts_with("step limit") {
        "its step limit (the agent profile's stepsPerRun, one step a model call)"
    } else if detail.contains("token budget") {
        "its token budget (the agent profile's maxTokensPerRun)"
    } else {
        return None;
    };
    Some(format!(
        "this run has used {limit}, so it stops here with the version on screen. An \
         administrator can raise the limit in the agent profile; a new message starts a new run"
    ))
}

/// A status worth one more try (T-2772): busy, too early, timed out at a gateway, or failing for a
/// moment. A refusal of the credentials or of the request is not.
fn retryable(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

/// How long to wait before the retry: the provider's `Retry-After` in seconds, at most
/// [`RETRY_AFTER_MAX_MS`], else [`RETRY_AFTER_MS`].
fn backoff(headers: &reqwest::header::HeaderMap) -> std::time::Duration {
    let asked = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000).min(RETRY_AFTER_MAX_MS));
    std::time::Duration::from_millis(asked.unwrap_or(RETRY_AFTER_MS))
}

/// How often a streamed answer's words are written as a `partial` event: at most two a second
/// (API/04 §4, ADR-N-032).
const PARTIAL_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

impl Driver {
    /// A provider that says the key's credit covers fewer output tokens than asked is asked
    /// once more within what it covers: most answers are far shorter than the budget, and a
    /// run should not stop over a ceiling it would not have reached.
    pub(super) async fn complete_within(
        &self,
        system: &str,
        user: &str,
        budget: u32,
    ) -> Result<String, String> {
        self.complete_within_as(system, user, budget, false, &[])
            .await
    }

    /// One conversation turn with the model's words shown while it writes them: an
    /// OpenAI-compatible call streams, and the prose before its first tool fence is written as
    /// `partial` events (ADR-N-032 §4, API/04 §4). Anthropic's messages keep the whole call.
    /// `tools` go as native tool definitions; each call the model makes comes back as the tool
    /// block the turn's dispatch reads (JC1-10, [`fence`]).
    pub(super) async fn complete_streamed(
        &self,
        system: &str,
        user: &str,
        tools: &[ToolSpec],
    ) -> Result<String, String> {
        self.complete_within_as(
            system,
            user,
            OUTPUT_BUDGET,
            self.provider != "anthropic",
            tools,
        )
        .await
    }

    async fn complete_within_as(
        &self,
        system: &str,
        user: &str,
        budget: u32,
        stream: bool,
        tools: &[ToolSpec],
    ) -> Result<String, String> {
        let first = match self
            .complete_once(system, user, budget, stream, tools)
            .await
        {
            Err(CallError::Empty) => {
                self.complete_once(system, user, budget, stream, tools)
                    .await
            }
            Err(CallError::Credit {
                affordable: Some(afford),
            }) if afford >= MIN_CREDIT_BUDGET && afford < budget => {
                self.complete_once(system, user, afford - afford / 20, stream, tools)
                    .await
            }
            other => other,
        };
        first.map_err(|err| err.said(budget))
    }

    /// One POST to the proxy, asked once more when a gateway failed, answering the response
    /// only when it succeeded: a 402 is the key's credit, any other refusal says its status.
    async fn send_llm(&self, path: &str, body: &Value) -> Result<reqwest::Response, CallError> {
        let first_version = body["max_tokens"] == json!(FIRST_VERSION_BUDGET);
        let effort = self.reasoning_effort.as_deref().map(|effort| {
            if first_version {
                FIRST_VERSION_EFFORT
            } else {
                effort
            }
        });
        let body = with_effort(path, body, effort, self.routing.as_ref());
        // A provider that was busy or failed for a moment is asked once more before the person
        // is told: on dev one 502 ended an answer and had the person send it again, and a 429 or
        // a dropped connection reads the same (T-2772). A call that timed out is not asked
        // again: the person already waited the whole turn, and Try again is theirs to press.
        let mut tries = 0;
        let response = loop {
            tries += 1;
            let sent = self
                .http
                .post(format!("{}{path}", self.proxy_base))
                .bearer_auth(&self.bearer)
                .json(&body)
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(err) if tries == 1 && !err.is_timeout() => {
                    tracing::warn!(error = %err, "the model call did not go through; asking once more");
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_AFTER_MS)).await;
                    continue;
                }
                Err(err) => {
                    tracing::warn!(error = %err, "the model call did not go through the proxy");
                    return Err(CallError::Failed(if err.is_timeout() {
                        "the model service did not answer in time. Your question was not \
                         answered and nothing was changed; send the message again in a moment."
                            .to_owned()
                    } else {
                        "the model service could not be reached. Your question was not answered \
                         and nothing was changed; send the message again in a moment."
                            .to_owned()
                    }));
                }
            };
            let status = response.status();
            // The proxy's 429 for a run that spent a limit its profile sets is no busy provider:
            // asked again it is refused again, and the run says which limit it reached (T-2997).
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait = backoff(response.headers());
                let text = response.text().await.unwrap_or_default();
                if let Some(reached) = run_limit_reached(&text) {
                    tracing::info!(%status, "the run reached a limit of its profile");
                    return Err(CallError::Failed(reached));
                }
                if tries == 1 {
                    tracing::warn!(%status, "the model provider could not answer; asking once more");
                    tokio::time::sleep(wait).await;
                    continue;
                }
                tracing::warn!(%status, provider = %provider_said(&text), "the model provider refused the call");
                return Err(CallError::Failed(refusal(status)));
            }
            if tries == 1 && retryable(status) {
                tracing::warn!(%status, "the model provider could not answer; asking once more");
                tokio::time::sleep(backoff(response.headers())).await;
                continue;
            }
            break response;
        };
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::PAYMENT_REQUIRED {
            return Err(CallError::Credit {
                affordable: affordable_tokens(&text),
            });
        }
        let said = provider_said(&text);
        // The proxy's audit line carries the status alone, so a refusal is otherwise only
        // visible in the person's own conversation and never in the logs (T-2247).
        tracing::warn!(%status, provider = %said, "the model provider refused the call");
        Err(CallError::Failed(refusal(status)))
    }

    /// Common HTTP execution for model calls through the proxy, handling 402 credits and budget cuts.
    async fn post_llm(&self, path: &str, body: &Value, budget: u32) -> Result<Value, CallError> {
        let text = self
            .send_llm(path, body)
            .await?
            .text()
            .await
            .unwrap_or_default();
        whole_answer(&text, budget)
    }

    /// A streamed chat completion: its text, with the prose so far written as `partial` events
    /// at most every [`PARTIAL_EVERY`]. An upstream that answers whole instead is read whole.
    async fn post_llm_streamed(&self, body: &Value, budget: u32) -> Result<String, CallError> {
        let mut response = self.send_llm("/v1/llm/chat/completions", body).await?;
        let sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !sse {
            let text = response.text().await.unwrap_or_default();
            return chat_content(&whole_answer(&text, budget)?);
        }
        let mut stream = Streamed::default();
        let mut said = String::new();
        let mut said_at: Option<std::time::Instant> = None;
        loop {
            let chunk = response.chunk().await.map_err(|err| {
                CallError::Failed(format!(
                    "the model's answer stopped part way ({err}); send the message again"
                ))
            })?;
            let Some(chunk) = chunk else { break };
            stream.push(&chunk);
            let prose = partial_prose(&stream.text);
            if !prose.is_empty()
                && prose != said
                && said_at.is_none_or(|at| at.elapsed() >= PARTIAL_EVERY)
            {
                // A partial that could not be written costs the person only the preview; the
                // answer itself still arrives as the thought.
                let _ = self
                    .event(
                        "partial",
                        json!({ "text": prose, "elapsedMs": self.elapsed_ms() }),
                    )
                    .await;
                said = prose.to_owned();
                said_at = Some(std::time::Instant::now());
            }
        }
        stream.finish();
        if let Some(error) = stream.error {
            tracing::warn!(provider = %error, "the model provider stopped a streamed answer");
            return Err(CallError::Failed(
                "the model provider stopped its answer part way; send the message again".to_owned(),
            ));
        }
        if stream.finish_reason.as_deref() == Some("length") {
            return Err(cut_at(budget));
        }
        // No delta at all is a stream that carried no answer; asked once more like an empty one.
        if stream.text.is_empty() && stream.calls.is_empty() && stream.finish_reason.is_none() {
            return Err(CallError::Empty);
        }
        let mut text = stream.text;
        for (name, arguments) in &stream.calls {
            text.push_str(&fence(
                name,
                serde_json::from_str(arguments).unwrap_or(Value::Null),
            ));
        }
        Ok(text)
    }

    /// One call through the proxy, in the body the profile's provider reads (AG-53); `stream`
    /// asks an OpenAI-compatible provider for its answer as it is written.
    pub(super) async fn complete_once(
        &self,
        system: &str,
        user: &str,
        budget: u32,
        stream: bool,
        tools: &[ToolSpec],
    ) -> Result<String, CallError> {
        let anthropic = self.provider == "anthropic";
        let with_tools = |mut body: Value| {
            if !tools.is_empty() {
                body["tools"] = json!(tools_json(anthropic, tools));
                if !anthropic {
                    body["parallel_tool_calls"] = json!(true);
                }
            }
            body
        };
        if stream && !anthropic {
            let body = with_tools(json!({
                "model": self.model,
                "max_tokens": budget,
                "stream": true,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user_content(user) },
                ],
            }));
            return self.post_llm_streamed(&body, budget).await;
        }
        let (path, body) = if self.provider == "anthropic" {
            (
                "/v1/llm/messages",
                with_tools(json!({
                    "model": self.model,
                    "max_tokens": budget,
                    "system": system,
                    "messages": [{ "role": "user", "content": user_content(user) }],
                })),
            )
        } else {
            (
                "/v1/llm/chat/completions",
                with_tools(json!({
                    "model": self.model,
                    "max_tokens": budget,
                    "messages": [
                        { "role": "system", "content": system },
                        { "role": "user", "content": user_content(user) },
                    ],
                })),
            )
        };
        let answer = self.post_llm(path, &body, budget).await?;
        // OpenAI-compatible: choices[0].message.content. Anthropic: content[].text, joined.
        if answer.pointer("/choices/0/message/content").is_some() {
            return chat_content(&answer);
        }
        messages_text(&answer)
    }

    /// One tool completion through the proxy with 402 retry and empty-answer retry (SDK-20).
    pub(super) async fn complete_tools(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[ToolSpec],
        budget: u32,
    ) -> Result<ToolAnswer, CallError> {
        let first = match self
            .complete_tools_once(system, messages, tools, budget)
            .await
        {
            Err(CallError::Empty) => {
                self.complete_tools_once(system, messages, tools, budget)
                    .await
            }
            Err(CallError::Credit {
                affordable: Some(afford),
            }) if afford >= MIN_CREDIT_BUDGET && afford < budget => {
                self.complete_tools_once(system, messages, tools, afford - afford / 20)
                    .await
            }
            other => other,
        };
        first
    }

    async fn complete_tools_once(
        &self,
        system: &str,
        messages: &[Value],
        tools: &[ToolSpec],
        budget: u32,
    ) -> Result<ToolAnswer, CallError> {
        let messages = &cache_marked(messages);
        if self.provider == "anthropic" {
            let tools_json = tools_json(true, tools);
            let merged = merge_anthropic_messages(messages);
            let body = json!({
                "model": self.model,
                "max_tokens": budget,
                "system": system,
                "messages": merged,
                "tools": tools_json,
            });
            let answer = self.post_llm("/v1/llm/messages", &body, budget).await?;
            let mut text_parts = Vec::new();
            let mut calls = Vec::new();
            if let Some(content) = answer.get("content").and_then(Value::as_array) {
                for part in content {
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
                    if part_type == "text" {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            text_parts.push(t);
                        }
                    } else if part_type == "tool_use" {
                        let id = part
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let name = part
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let input = part.get("input").cloned().unwrap_or(Value::Null);
                        calls.push(ToolCall { id, name, input });
                    }
                }
            }
            let text = if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            };
            let input_tokens = usage_at(&answer, "/usage/input_tokens");
            let output_tokens = usage_at(&answer, "/usage/output_tokens");
            if text.as_deref().unwrap_or("").trim().is_empty() && calls.is_empty() {
                return Err(CallError::Empty);
            }
            Ok(ToolAnswer {
                text,
                calls,
                usage_tokens: input_tokens + output_tokens,
                input_tokens,
                output_tokens,
            })
        } else {
            let tools_json = tools_json(false, tools);
            let mut full_messages = vec![json!({ "role": "system", "content": system })];
            full_messages.extend_from_slice(messages);
            // Several calls per answer (T-3075): the loop runs all of them and answers them in
            // one next call, so independent reads cost one round trip.
            let body = json!({
                "model": self.model,
                "max_tokens": budget,
                "messages": full_messages,
                "tools": tools_json,
                "parallel_tool_calls": true,
            });
            let answer = self
                .post_llm("/v1/llm/chat/completions", &body, budget)
                .await?;
            let choice_msg = answer.pointer("/choices/0/message");
            let text = choice_msg
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let mut calls = Vec::new();
            if let Some(tool_calls) = choice_msg
                .and_then(|m| m.get("tool_calls"))
                .and_then(Value::as_array)
            {
                for tc in tool_calls {
                    let id = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let name = tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let input = if let Some(s) =
                        tc.pointer("/function/arguments").and_then(Value::as_str)
                    {
                        serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
                    } else {
                        tc.pointer("/function/arguments")
                            .cloned()
                            .unwrap_or(Value::Null)
                    };
                    calls.push(ToolCall { id, name, input });
                }
            }
            let input_tokens = usage_at(&answer, "/usage/prompt_tokens");
            let output_tokens = usage_at(&answer, "/usage/completion_tokens");
            let usage_tokens = answer
                .pointer("/usage/total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(input_tokens + output_tokens);
            if text.as_deref().unwrap_or("").trim().is_empty() && calls.is_empty() {
                return Err(CallError::Empty);
            }
            Ok(ToolAnswer {
                text,
                calls,
                usage_tokens,
                input_tokens,
                output_tokens,
            })
        }
    }

    /// The types the data needs name that the endpoint serves, in the order they were named.
    pub(super) fn types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        for need in self.data_needs.as_array().into_iter().flatten() {
            for entity_type in need
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !types.iter().any(|known| known == entity_type) {
                    types.push(entity_type.to_owned());
                }
            }
        }
        served_only(types, self.schema_index.get())
    }

    /// The data needs as the model reads them: every type the endpoint does not serve left out,
    /// and a need left with no type dropped.
    pub(super) fn served_needs(&self) -> Value {
        needs_served(&self.data_needs, self.schema_index.get())
    }

    /// The endpoint's schema index, read through the proxy with the run's ticket (EP-46). A run
    /// over several endpoints gets one index: every endpoint's models, each marked with the
    /// index of its endpoint; an endpoint whose index cannot be read keeps the types its needs
    /// name, so a proxy that does not serve it yet narrows nothing away.
    pub(super) async fn schema_index(&self) -> Result<Value, String> {
        let mut index = self
            .read_entities(&format!("{}/schema/index.json", self.data_base(0)))
            .await?;
        if self.endpoints.len() < 2 {
            return Ok(index);
        }
        let by_endpoint = endpoints::types_by_endpoint(&self.endpoints, &self.data_needs);
        let mut models: Vec<Value> = Vec::new();
        for at in 0..self.endpoints.len() {
            let read = if at == 0 {
                Ok(index.clone())
            } else {
                self.read_entities(&format!("{}/schema/index.json", self.data_base(at)))
                    .await
            };
            match read {
                Ok(read) => models.extend(
                    read["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .cloned()
                        .map(|mut model| {
                            model["endpoint"] = json!(at);
                            model
                        }),
                ),
                Err(_) => models.push(json!({
                    "endpoint": at,
                    "unread": true,
                    "types": by_endpoint.get(at).cloned().unwrap_or_default(),
                })),
            }
        }
        index["models"] = Value::Array(models);
        Ok(index)
    }

    /// Where the data of the run's endpoint at `at` is read through the proxy.
    pub(super) fn data_base(&self, at: usize) -> String {
        endpoints::data_base(&self.proxy_base, &self.endpoints, at)
    }

    /// One GET through the proxy with the run's ticket, as JSON.
    pub(super) async fn read_entities(&self, url: &str) -> Result<Value, String> {
        let body = self.read_text(url).await?;
        serde_json::from_str::<Value>(&body).map_err(|err| err.to_string())
    }

    /// One GET through the proxy with the run's ticket, as text.
    pub(super) async fn read_text(&self, url: &str) -> Result<String, String> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status();
        let body = crate::capped::text(response, crate::capped::ANSWER_BYTES).await?;
        if !status.is_success() {
            return Err(format!(
                "{status}: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        Ok(body)
    }

    /// A few entities per type, read through the proxy like the application will (AP-57). A
    /// type several endpoints serve is read through each and joined by id, the first endpoint's
    /// entities leading. A type that cannot be read is an empty list with the reason in the chat:
    /// the model still gets the data needs, and the chat says what was missing.
    pub(super) async fn samples(&self, types: &[String]) -> Result<Value, String> {
        let mut samples = serde_json::Map::new();
        let mut joined = String::new();
        for entity_type in types {
            let serving = endpoints::serving(&self.endpoints, &self.data_needs, entity_type);
            let names: Vec<&str> = serving
                .iter()
                .filter_map(|&at| self.endpoints.get(at))
                .map(|endpoint| endpoint.name.as_str())
                .collect();
            self.thought(&format!(
                "Reading {SAMPLES_PER_TYPE} entities of {entity_type} through {}.",
                if self.endpoints.len() < 2 || names.is_empty() {
                    "the endpoint".to_owned()
                } else {
                    names.join(" and ")
                }
            ))
            .await?;
            let mut rows: Vec<Value> = Vec::new();
            let mut carried: Vec<String> = Vec::new();
            for &at in &serving {
                let base = format!(
                    "{}/ngsi-ld/v1/entities?type={}&limit={SAMPLES_PER_TYPE}&options=keyValues",
                    self.data_base(at),
                    urlencoding(entity_type)
                );
                let ids: Vec<&str> = rows.iter().filter_map(|row| row["id"].as_str()).collect();
                // A later endpoint is asked for the entities already read, so their parts meet.
                let mut read = if ids.is_empty() {
                    self.read_entities(&base).await
                } else {
                    self.read_entities(&format!("{base}&id={}", urlencoding(&ids.join(","))))
                        .await
                };
                if !ids.is_empty() && matches!(&read, Ok(Value::Array(found)) if found.is_empty()) {
                    read = self.read_entities(&base).await;
                }
                let name = self
                    .endpoints
                    .get(at)
                    .map_or("the endpoint", |endpoint| endpoint.name.as_str());
                match read {
                    Ok(Value::Array(entities)) => {
                        carried.push(format!(
                            "`{name}` carries {}",
                            endpoints::attributes_of(&entities).join(", ")
                        ));
                        endpoints::join_by_id(&mut rows, entities);
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        self.thought(&format!(
                            "No sample of {entity_type} could be read through {name}: {reason}"
                        ))
                        .await?;
                    }
                }
            }
            if carried.len() > 1 {
                joined.push_str(&format!(
                    "- {entity_type}: {}. The samples are these rows joined by `id`; read the \
                     type from each endpoint with `{{ endpoint }}` and join the rows the same way.\n",
                    carried.join("; ")
                ));
            }
            samples.insert(entity_type.clone(), Value::Array(rows));
        }
        let _ = self.joined.set(joined);
        Ok(Value::Object(samples))
    }

    pub(super) async fn status(&self, status: AgentRunStatus) -> Result<(), String> {
        self.state
            .agents
            .set_status(&self.run_id, status, None)
            .await
            .map_err(|err| err.to_string())?;
        if status == AgentRunStatus::AwaitingApproval {
            crate::api::agent_runs::lease_for_approval(&self.state, &self.run_id).await;
        }
        self.event(
            "status",
            json!({ "status": status.as_str(), "timestamp": now_rfc3339() }),
        )
        .await
    }
}

/// A tool the model may call in an editing turn (SDK-20) or a connector turn (T-5958), in the
/// provider's own body.
pub(super) struct ToolSpec {
    pub name: std::borrow::Cow<'static, str>,
    pub description: std::borrow::Cow<'static, str>,
    pub input_schema: Value,
}

/// One call the model asked for: the id the provider echoes, the tool and its arguments.
#[derive(Debug, Clone)]
pub(super) struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What a tool completion answered: prose, calls, and what it cost.
#[derive(Debug, Clone, Default)]
pub(super) struct ToolAnswer {
    pub text: Option<String>,
    pub calls: Vec<ToolCall>,
    pub usage_tokens: u64,
    /// The two halves of `usage_tokens`, 0 when the provider does not report them.
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A whole answer read as JSON; one cut at the output budget applied nothing and says so.
/// `body` with the profile's reasoning effort (AG-72, JC1-26) in the OpenAI-compatible
/// `reasoning` member OpenRouter reads, when there is one to add.
/// ponytail: the Anthropic messages body gets none; add `thinking` when a profile runs on it.
/// The profile's routing goes the same way, as OpenRouter's `provider` member; the proxy removes it
/// before any other upstream (AG-72).
fn with_effort<'a>(
    path: &str,
    body: &'a Value,
    effort: Option<&str>,
    provider: Option<&'a Value>,
) -> Effortful<'a> {
    let chat = path.ends_with("/chat/completions");
    Effortful {
        body,
        reasoning: effort
            .filter(|_| chat)
            .map(|effort| json!({ "effort": effort })),
        provider: provider.filter(|_| chat),
    }
}

/// A request body with its `reasoning`, written out over the borrowed body: a turn's prompt is
/// not copied for one more member.
#[derive(serde::Serialize)]
struct Effortful<'a> {
    #[serde(flatten)]
    body: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a Value>,
}

fn whole_answer(text: &str, budget: u32) -> Result<Value, CallError> {
    let answer: Value = serde_json::from_str(text)
        .map_err(|err| CallError::Failed(format!("the model's answer is not JSON: {err}")))?;
    let cut = answer
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| reason == "length")
        || answer
            .get("stop_reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason == "max_tokens");
    if cut {
        return Err(cut_at(budget));
    }
    Ok(answer)
}

fn cut_at(budget: u32) -> CallError {
    CallError::Failed(format!(
        "the answer was cut at the output budget of {budget} tokens and nothing was applied; \
         ask for less at once"
    ))
}

/// An OpenAI-compatible answer's text, `choices[0].message.content`.
fn chat_content(answer: &Value) -> Result<String, CallError> {
    let content = answer
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str);
    let calls = answer
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if content.is_none() && calls.is_empty() {
        return Err(CallError::Empty);
    }
    let mut text = content.unwrap_or_default().to_owned();
    for call in &calls {
        let name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|a| serde_json::from_str(a).ok())
            .unwrap_or(Value::Null);
        text.push_str(&fence(name, arguments));
    }
    Ok(text)
}

/// An Anthropic messages answer as text: its text parts joined, each `tool_use` part as the tool
/// block the conversation's dispatch reads ([`fence`]).
fn messages_text(answer: &Value) -> Result<String, CallError> {
    let parts = answer
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut joined: String = parts
        .iter()
        .filter(|part| part.get("type").is_none_or(|t| t == "text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    for part in parts.iter().filter(|part| part["type"] == "tool_use") {
        let name = part.get("name").and_then(Value::as_str).unwrap_or_default();
        joined.push_str(&fence(
            name,
            part.get("input").cloned().unwrap_or(Value::Null),
        ));
    }
    if joined.is_empty() {
        return Err(CallError::Empty);
    }
    Ok(joined)
}

/// A native tool call written as the tool block the conversation's dispatch reads (JC1-10): a
/// `jc_` tool's input under `arguments`, any other tool's fields beside its name. The name the
/// provider says wins over a `tool` field inside the input.
pub(super) fn fence(name: &str, input: Value) -> String {
    let block = if name.starts_with("jc_") {
        json!({ "tool": name, "arguments": input })
    } else {
        let mut fields = match input {
            Value::Object(fields) => fields,
            _ => serde_json::Map::new(),
        };
        fields.insert("tool".into(), json!(name));
        Value::Object(fields)
    };
    format!("\n\n```json\n{block}\n```\n")
}

/// The tool definitions in the body the provider reads.
fn tools_json(anthropic: bool, tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            if anthropic {
                json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema })
            } else {
                json!({
                    "type": "function",
                    "function": { "name": t.name, "description": t.description, "parameters": t.input_schema },
                })
            }
        })
        .collect()
}

/// The most native tool calls one streamed answer may carry.
const MAX_STREAM_CALLS: usize = 32;

/// What a `partial` shows of the answer so far: the prose before the first tool fence, without a
/// fence's first backticks still arriving, so a tool call's JSON never reaches the screen.
fn partial_prose(text: &str) -> &str {
    let before = text.split("```").next().unwrap_or_default();
    before.trim_end_matches('`').trim()
}

/// A chat completion read from its server-sent events as they arrive: the `data:` lines' deltas
/// joined, the finish reason, and an error a provider sends inside the stream.
#[derive(Default)]
struct Streamed {
    line: Vec<u8>,
    text: String,
    /// The native tool calls so far, by their index in the stream: name and arguments text.
    calls: Vec<(String, String)>,
    finish_reason: Option<String>,
    error: Option<String>,
}

impl Streamed {
    /// A chunk of the stream; a line split across chunks waits for its end.
    fn push(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.line_done(&String::from_utf8_lossy(&line));
            } else {
                self.line.push(byte);
            }
        }
    }

    /// The stream ended: a last line without its newline still counts.
    fn finish(&mut self) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.line_done(&String::from_utf8_lossy(&line));
        }
    }

    /// One `delta.tool_calls` entry: its name and arguments text joined onto the call at its
    /// index.
    fn tool_delta(&mut self, call: &Value) {
        let at = call
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(self.calls.len().saturating_sub(1));
        // ponytail: a provider index past MAX_STREAM_CALLS is dropped, never allocated.
        if at >= MAX_STREAM_CALLS {
            return;
        }
        if self.calls.len() <= at {
            self.calls.resize(at + 1, (String::new(), String::new()));
        }
        let Some((name, arguments)) = self.calls.get_mut(at) else {
            return;
        };
        if let Some(part) = call.pointer("/function/name").and_then(Value::as_str) {
            name.push_str(part);
        }
        if let Some(part) = call.pointer("/function/arguments").and_then(Value::as_str) {
            arguments.push_str(part);
        }
    }

    fn line_done(&mut self, line: &str) {
        let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(frame) = serde_json::from_str::<Value>(data) else {
            return;
        };
        if let Some(error) = frame.get("error") {
            self.error = Some(provider_said(&error.to_string()));
            return;
        }
        if let Some(delta) = frame
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            self.text.push_str(delta);
        }
        for call in frame
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.tool_delta(call);
        }
        if let Some(reason) = frame
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
        {
            self.finish_reason = Some(reason.to_owned());
        }
    }
}

fn usage_at(answer: &Value, pointer: &str) -> u64 {
    answer.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

/// Anthropic takes alternating roles: two turns of one role in a row are one turn with the
/// content blocks joined (a string is one text block).
/// Where a one-message prompt's cached part ends (T-3079): the text before it is the same from
/// one call to the next, the text after it is the call's own. Never sent: [`user_content`]
/// turns it into two content blocks.
pub(super) const CACHE_BREAK: &str = "\u{1}jc-cache-break\u{1}";

/// A user message as sent: a prompt with a [`CACHE_BREAK`] becomes two text blocks, the first
/// marked for the provider's prompt cache; one without it stays a plain string.
pub(super) fn user_content(user: &str) -> Value {
    match user.split_once(CACHE_BREAK) {
        Some((stable, own)) => json!([
            { "type": "text", "text": stable, "cache_control": { "type": "ephemeral" } },
            { "type": "text", "text": own }
        ]),
        None => Value::String(user.to_owned()),
    }
}

/// The messages with the first user message, a loop's opening, marked for the provider's prompt
/// cache (T-3073): one `cache_control` breakpoint on a text block, the shape Anthropic and
/// OpenRouter (Gemini as well, which uses the last breakpoint) read. Every call of a turn sends
/// the tools, the rules and that opening byte for byte, so each later call reads them from the
/// cache instead of paying for them again; what a step adds always follows the breakpoint.
pub(super) fn cache_marked(messages: &[Value]) -> Vec<Value> {
    let mut marked = messages.to_vec();
    if let Some(opening) = marked
        .iter_mut()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    {
        if let Some(text) = opening
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            opening["content"] = json!([{
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" }
            }]);
        }
    }
    marked
}

pub(super) fn merge_anthropic_messages(messages: &[Value]) -> Vec<Value> {
    let blocks = |content: &Value| -> Vec<Value> {
        match content {
            Value::String(text) => vec![json!({ "type": "text", "text": text })],
            Value::Array(parts) => parts.clone(),
            other => vec![json!({ "type": "text", "text": other.to_string() })],
        }
    };
    let mut merged: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = blocks(message.get("content").unwrap_or(&Value::Null));
        match merged.last_mut() {
            Some(last) if last.get("role").and_then(Value::as_str) == Some(role) => {
                if let Some(parts) = last.get_mut("content").and_then(Value::as_array_mut) {
                    parts.extend(content);
                }
            }
            _ => merged.push(json!({ "role": role, "content": content })),
        }
    }
    merged
}

/// The model's own turn, appended so the next call sees what it asked for.
pub(super) fn assistant_turn_message(provider: &str, answer: &ToolAnswer) -> Value {
    if provider == "anthropic" {
        let mut content = Vec::new();
        if let Some(text) = answer
            .text
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            content.push(json!({ "type": "text", "text": text }));
        }
        for call in &answer.calls {
            content.push(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.input,
            }));
        }
        json!({ "role": "assistant", "content": content })
    } else {
        let calls: Vec<Value> = answer
            .calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.input.to_string() },
                })
            })
            .collect();
        let mut message = json!({ "role": "assistant", "content": answer.text });
        if !calls.is_empty() {
            message["tool_calls"] = Value::Array(calls);
        }
        message
    }
}

/// A tool's result, in the shape the provider reads it back.
pub(super) fn tool_result_message(provider: &str, call_id: &str, content: &str) -> Value {
    if provider == "anthropic" {
        json!({
            "role": "user",
            "content": [{ "type": "tool_result", "tool_use_id": call_id, "content": content }],
        })
    } else {
        json!({ "role": "tool", "tool_call_id": call_id, "content": content })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-3065, AG-97: the proxy's daily-budget refusal is shown as the proxy said it and is not
    /// asked again; a 429 that is neither a profile limit nor a budget is left to the retry.
    #[test]
    fn a_daily_budget_is_said_as_the_proxy_said_it() {
        let budget = r#"{"type":"https://joinedcontext.com/errors/daily-budget","title":"Daily Budget Spent","status":429,"detail":"Today's model budget for the assistant is spent. It starts again at 00:00 UTC; an administrator can raise it."}"#;
        assert_eq!(
            run_limit_reached(budget).as_deref(),
            Some("Today's model budget for the assistant is spent. It starts again at 00:00 UTC; an administrator can raise it.")
        );
        let steps = r#"{"type":"https://joinedcontext.com/errors/too-many-requests","status":429,"detail":"step limit exceeded (stepsPerRun)"}"#;
        assert!(run_limit_reached(steps).is_some_and(|said| said.contains("stepsPerRun")));
        let busy = r#"{"error":{"message":"Rate limit exceeded","code":429}}"#;
        assert_eq!(run_limit_reached(busy), None);
    }
}

#[cfg(test)]
mod stream_tests {
    //! A streamed answer (T-2821, ADR-N-032 §4): the deltas joined however the chunks split them,
    //! and what a `partial` may show of it.

    use super::*;

    fn frame(content: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({ "choices": [{ "index": 0, "delta": { "content": content } }] })
        )
    }

    #[test]
    fn deltas_join_however_the_chunks_split_the_lines() {
        let whole = format!(
            "{}{}: keep-alive\n\ndata: {}\n\ndata: [DONE]\n\n",
            frame("Two stations "),
            frame("are empty: Töölö."),
            json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }], "usage": { "total_tokens": 9 } })
        );
        let bytes = whole.as_bytes();
        // Split everywhere, the middle of the two-byte ö included.
        for at in 1..bytes.len() {
            let mut stream = Streamed::default();
            stream.push(&bytes[..at]);
            stream.push(&bytes[at..]);
            stream.finish();
            assert_eq!(
                stream.text, "Two stations are empty: Töölö.",
                "split at {at}"
            );
            assert_eq!(stream.finish_reason.as_deref(), Some("stop"));
            assert!(stream.error.is_none());
        }
    }

    #[test]
    fn a_last_line_without_its_newline_and_crlf_lines_still_count() {
        let mut stream = Streamed::default();
        stream.push(frame("a").replace('\n', "\r\n").as_bytes());
        stream.push(frame("b").trim_end().as_bytes());
        stream.finish();
        assert_eq!(stream.text, "ab");
    }

    #[test]
    fn an_error_inside_the_stream_is_kept_and_a_length_cut_is_seen() {
        let mut stream = Streamed::default();
        stream.push(b"data: {\"error\":{\"message\":\"Provider overloaded\"}}\n");
        stream.finish();
        assert_eq!(stream.error.as_deref(), Some("Provider overloaded"));

        let mut cut = Streamed::default();
        cut.push(
            format!(
                "{}data: {}\n",
                frame("half"),
                json!({ "choices": [{ "delta": {}, "finish_reason": "length" }] })
            )
            .as_bytes(),
        );
        assert_eq!(cut.finish_reason.as_deref(), Some("length"));
    }

    /// JC1-10: a native tool call streamed in pieces is joined by its index, and written as the
    /// block the dispatch reads; a `jc_` tool's input goes under `arguments`.
    #[test]
    fn a_streamed_tool_call_is_joined_and_written_as_its_block() {
        let delta = |call: Value| {
            format!(
                "data: {}\n\n",
                json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [call] } }] })
            )
        };
        let mut stream = Streamed::default();
        stream.push(
            delta(json!({ "index": 0, "function": { "name": "search_", "arguments": "{\"q\":" } }))
                .as_bytes(),
        );
        stream.push(
            delta(
                json!({ "index": 0, "function": { "name": "catalog", "arguments": "\"bikes\"}" } }),
            )
            .as_bytes(),
        );
        stream.push(delta(json!({ "index": 999, "function": { "name": "flood" } })).as_bytes());
        assert_eq!(
            stream.calls,
            [("search_catalog".to_owned(), "{\"q\":\"bikes\"}".to_owned())]
        );
        let block = fence("search_catalog", json!({ "q": "bikes", "tool": "other" }));
        assert_eq!(data_query::search_calls(&block), ["bikes"]);
        let registry = fence("jc_resource_list", json!({ "kind": "Endpoint" }));
        let parsed = tools_registry::calls(&registry);
        assert_eq!(parsed[0].name, "jc_resource_list");
        assert_eq!(parsed[0].arguments, json!({ "kind": "Endpoint" }));
    }

    #[test]
    fn a_line_that_is_not_json_is_skipped_not_fatal() {
        let mut stream = Streamed::default();
        stream.push(b"data: {not json\nevent: ping\n");
        stream.push(frame("ok").as_bytes());
        assert_eq!(stream.text, "ok");
    }

    #[test]
    fn a_partial_never_shows_a_tool_fence_even_half_arrived() {
        assert_eq!(
            partial_prose("Reading the events.\n\n```json\n{\"tool\":\"query_endpoint\""),
            "Reading the events."
        );
        assert_eq!(
            partial_prose("Reading the events.\n\n`"),
            "Reading the events."
        );
        assert_eq!(
            partial_prose("Reading the events.\n``"),
            "Reading the events."
        );
        assert_eq!(partial_prose("```json\n{\"tool\":\"x\"}\n```\nAfter"), "");
        assert_eq!(partial_prose(""), "");
        assert_eq!(partial_prose("Use `q` to filter."), "Use `q` to filter.");
    }
}

#[cfg(test)]
mod effort_tests {
    use super::*;

    /// AG-72 (T-6403): the first version a person waits for goes at low, the rest of the run at the
    /// profile's effort, and a profile without one sends none.
    #[tokio::test]
    async fn the_first_version_goes_at_low_effort_and_the_rest_at_the_profiles() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": "Done." } }],
            })))
            .mount(&server)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.provider = "openrouter".into();
        driver.reasoning_effort = Some("medium".into());
        for budget in [FIRST_VERSION_BUDGET, CODE_OUTPUT_BUDGET] {
            driver
                .complete_once("rules", "hello", budget, false, &[])
                .await
                .expect("an answer");
        }
        driver.reasoning_effort = None;
        driver
            .complete_once("rules", "hello", FIRST_VERSION_BUDGET, false, &[])
            .await
            .expect("an answer");
        let efforts: Vec<Value> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|request| serde_json::from_slice::<Value>(&request.body).expect("a JSON body"))
            .map(|body| body["reasoning"]["effort"].clone())
            .collect();
        assert_eq!(efforts, [json!("low"), json!("medium"), Value::Null]);
    }

    /// AG-72: the profile's routing reaches a chat call, streamed or whole, and never a messages
    /// body.
    #[tokio::test]
    async fn the_profiles_routing_rides_a_chat_call_and_no_messages_call() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "ok" }, "finish_reason": "stop" }],
                "content": [{ "type": "text", "text": "ok" }],
            })))
            .mount(&server)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.provider = "openrouter".into();
        driver.routing = Some(json!({ "sort": "latency", "allow_fallbacks": true }));
        for stream in [false, true] {
            driver
                .complete_once("rules", "hello", CODE_OUTPUT_BUDGET, stream, &[])
                .await
                .expect("an answer");
        }
        driver.provider = "anthropic".into();
        driver
            .complete_once("rules", "hello", CODE_OUTPUT_BUDGET, false, &[])
            .await
            .expect("an answer");
        let provider: Vec<Value> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|request| serde_json::from_slice::<Value>(&request.body).expect("a JSON body"))
            .map(|body| body["provider"].clone())
            .collect();
        let routing = json!({ "sort": "latency", "allow_fallbacks": true });
        assert_eq!(provider, [routing.clone(), routing, Value::Null]);
    }

    /// JC1-26: the profile's reasoning effort reaches every chat call the run makes.
    #[tokio::test]
    async fn every_chat_call_carries_the_profiles_reasoning_effort() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "role": "assistant", "content": "Done." } }],
            })))
            .mount(&server)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.provider = "openrouter".into();
        driver.reasoning_effort = Some("medium".into());
        driver
            .complete_once("rules", "hello", 100, false, &[])
            .await
            .expect("an answer");
        driver
            .complete_tools_once(
                "rules",
                &[json!({ "role": "user", "content": "hi" })],
                &[],
                100,
            )
            .await
            .expect("an answer");
        let bodies: Vec<Value> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
            .collect();
        assert_eq!(bodies.len(), 2);
        for body in &bodies {
            assert_eq!(body["reasoning"], json!({ "effort": "medium" }), "{body}");
        }
        assert_eq!(
            serde_json::to_value(with_effort(
                "/v1/llm/chat/completions",
                &json!({}),
                None,
                None
            ))
            .expect("a JSON body"),
            json!({}),
            "no effort named, none sent"
        );
    }
}
