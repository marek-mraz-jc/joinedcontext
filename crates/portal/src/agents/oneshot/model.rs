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

    /// One GET through the proxy with the run's ticket, as text, without the attributes the
    /// read projects' models mark personal (AG-158, JC1-03): every page a run reads reaches the
    /// model provider, as a sample, a test input or a preview.
    pub(super) async fn read_text(&self, url: &str) -> Result<String, String> {
        let body = self.read_raw(url).await?;
        match serde_json::from_str::<Value>(&body) {
            Ok(mut page) if crate::personal_data::carries_entities(&page) => {
                self.withhold_personal_in(&mut page).await?;
                Ok(page.to_string())
            }
            _ => Ok(body),
        }
    }

    /// Removes from `value` the attributes marked personal by the models of this run's project
    /// and of each project whose endpoint it reads. A model that cannot be read is an `Err`:
    /// nothing could be withheld, so the caller refuses the page.
    // ponytail: reads the models per page; keep them on the Driver when runs read many pages.
    pub(super) async fn withhold_personal_in(&self, value: &mut Value) -> Result<(), String> {
        let mut projects = std::collections::BTreeSet::from([self.project.clone()]);
        projects.extend(self.endpoints.iter().filter_map(|endpoint| {
            super::tools_data::endpoint_project(&self.state, &endpoint.slug)
        }));
        for project in projects {
            let marks = crate::personal_data::of_project(&self.state, &project).await?;
            crate::personal_data::withhold(value, &marks);
        }
        Ok(())
    }

    async fn read_raw(&self, url: &str) -> Result<String, String> {
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
        // In bounds: the calls were just resized past `at`.
        let (name, arguments) = &mut self.calls[at];
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
    let mut merged: Vec<(&str, Vec<Value>)> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = blocks(message.get("content").unwrap_or(&Value::Null));
        match merged.last_mut() {
            Some((last, parts)) if *last == role => parts.extend(content),
            _ => merged.push((role, content)),
        }
    }
    merged
        .into_iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect()
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

    /// The provider's message shapes at their edges: an opening already in blocks keeps them,
    /// content of no known shape becomes text, an Anthropic turn carries its prose, and an
    /// OpenAI turn without calls has no `tool_calls` (T-7818).
    #[test]
    fn provider_messages_keep_their_shape_at_the_edges() {
        let blocks = serde_json::json!([{ "type": "text", "text": "hello" }]);
        let marked = cache_marked(&[serde_json::json!({ "role": "user", "content": blocks })]);
        assert_eq!(marked[0]["content"], blocks);
        let merged = merge_anthropic_messages(&[
            serde_json::json!({ "role": "user", "content": 7 }),
            serde_json::json!({ "role": "user", "content": "more" }),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0]["content"],
            serde_json::json!([{ "type": "text", "text": "7" }, { "type": "text", "text": "more" }])
        );
        let answer = ToolAnswer {
            text: Some("Here it is.".to_owned()),
            ..ToolAnswer::default()
        };
        assert_eq!(
            assistant_turn_message("anthropic", &answer)["content"],
            serde_json::json!([{ "type": "text", "text": "Here it is." }])
        );
        let openai = assistant_turn_message("openai", &answer);
        assert!(openai.get("tool_calls").is_none(), "{openai}");
        assert_eq!(openai["content"], "Here it is.");
    }

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

    /// T-7923: a provider that sends a call's name in one delta and its arguments in the next
    /// gets one call; a delta naming no index joins the last call.
    #[test]
    fn a_name_and_its_arguments_in_separate_deltas_are_one_call() {
        let delta = |call: Value| {
            format!(
                "data: {}\n\n",
                json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [call] } }] })
            )
        };
        let mut stream = Streamed::default();
        stream.push(
            delta(json!({ "index": 0, "function": { "name": "search_catalog" } })).as_bytes(),
        );
        stream.push(delta(json!({ "function": { "arguments": "{\"q\":\"bikes\"}" } })).as_bytes());
        assert_eq!(
            stream.calls,
            [("search_catalog".to_owned(), "{\"q\":\"bikes\"}".to_owned())]
        );
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

#[cfg(test)]
mod transport_tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn chat(content: &str) -> Value {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
    }

    async fn driver_at(server: &MockServer, provider: &str) -> Driver {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.provider = provider.into();
        driver
    }

    /// T-7916: an Anthropic answer's text part without text and a part of another type (a
    /// thinking block) are skipped; the text and the tool call stand.
    #[tokio::test]
    async fn parts_the_tool_answer_does_not_read_are_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    { "type": "thinking", "thinking": "hm" },
                    { "type": "text" },
                    { "type": "text", "text": "Reading." },
                    { "type": "tool_use", "id": "c1", "name": "read_file", "input": { "path": "a" } }
                ],
                "usage": { "input_tokens": 3, "output_tokens": 2 }
            })))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "anthropic").await;
        let answer = driver
            .complete_tools(
                "rules",
                &[json!({ "role": "user", "content": "hi" })],
                &[],
                100,
            )
            .await
            .expect("an answer");
        assert_eq!(answer.text.as_deref(), Some("Reading."));
        assert_eq!(answer.calls.len(), 1);
        assert_eq!(answer.calls[0].name, "read_file");
    }

    /// T-7916: no user message, or one already in blocks, is left as it is; turns of one role
    /// in a row are one Anthropic message, whatever shape their content has.
    #[test]
    fn the_cache_mark_and_the_merge_keep_what_they_do_not_change() {
        let system_only = vec![json!({ "role": "system", "content": "rules" })];
        assert_eq!(cache_marked(&system_only), system_only);
        let blocks = vec![json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] })];
        assert_eq!(cache_marked(&blocks), blocks);
        let merged = merge_anthropic_messages(&[
            json!({ "role": "user", "content": "a" }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "b" }] }),
            json!({ "content": 7 }),
            json!({ "role": "assistant", "content": "c" }),
        ]);
        assert_eq!(
            merged,
            vec![
                json!({ "role": "user", "content": [
                    { "type": "text", "text": "a" },
                    { "type": "text", "text": "b" },
                    { "type": "text", "text": "7" }
                ] }),
                json!({ "role": "assistant", "content": [{ "type": "text", "text": "c" }] }),
            ]
        );
    }

    async fn calls(server: &MockServer) -> Vec<Value> {
        server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
            .collect()
    }

    /// T-2997, AG-97: a 429 that names a profile limit or a daily budget is said at once and never
    /// asked again; a plain busy 429 is asked once more and then said as busy.
    #[tokio::test]
    async fn a_429_naming_a_limit_stops_at_once_and_a_busy_one_is_asked_twice() {
        for (detail, kind, said) in [
            ("stepsPerRun of 30 reached", "about:blank", "its step limit"),
            ("token budget spent", "about:blank", "its token budget"),
            (
                "Jana's daily budget is spent until 00:00 UTC",
                "https://jc/problems/daily-budget",
                "Jana's daily budget is spent until 00:00 UTC",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(429)
                        .set_body_json(json!({ "type": kind, "detail": detail })),
                )
                .mount(&server)
                .await;
            let driver = driver_at(&server, "openrouter").await;
            let err = driver
                .complete_once("rules", "hi", 100, false, &[])
                .await
                .expect_err("a limit");
            assert!(err.said(100).contains(said), "{detail}");
            assert_eq!(calls(&server).await.len(), 1, "{detail}: never asked again");
        }
        assert_eq!(run_limit_reached(r#"{"detail":"slow down"}"#), None);
        assert_eq!(run_limit_reached("not json"), None);
        assert_eq!(run_limit_reached(r#"{"title":"no detail"}"#), None);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let err = driver
            .complete_once("rules", "hi", 100, false, &[])
            .await
            .expect_err("busy");
        assert_eq!(
            err.said(100),
            refusal(reqwest::StatusCode::TOO_MANY_REQUESTS)
        );
        assert_eq!(
            calls(&server).await.len(),
            2,
            "a busy provider is asked once more"
        );
    }

    /// T-2772: a gateway failure is asked once more and the second answer stands; a refusal that
    /// is not worth a retry is said after one call.
    #[tokio::test]
    async fn a_failing_gateway_is_asked_once_more_and_a_refusal_is_not() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat("Second time.")))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let text = driver
            .complete_once("rules", "hi", 100, false, &[])
            .await
            .expect("the retry answers");
        assert_eq!(text, "Second time.");
        assert_eq!(calls(&server).await.len(), 2);

        for (status, calls_made) in [(503, 2), (401, 1), (400, 1)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("retry-after", "0")
                        .set_body_json(json!({ "error": { "message": "see https://keys" } })),
                )
                .mount(&server)
                .await;
            let driver = driver_at(&server, "anthropic").await;
            let err = driver
                .complete_once("rules", "hi", 100, false, &[])
                .await
                .expect_err("refused");
            let said = err.said(100);
            assert_eq!(
                said,
                refusal(reqwest::StatusCode::from_u16(status).expect("a status"))
            );
            assert!(!said.contains("https://"), "never the provider's body");
            assert_eq!(calls(&server).await.len(), calls_made, "{status}");
        }
    }

    #[test]
    fn retry_after_is_the_providers_capped_and_else_a_second() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(backoff(&headers).as_millis(), u128::from(RETRY_AFTER_MS));
        headers.insert(
            reqwest::header::RETRY_AFTER,
            " 2 ".parse().expect("a header"),
        );
        assert_eq!(backoff(&headers).as_millis(), 2000);
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "3600".parse().expect("a header"),
        );
        assert_eq!(
            backoff(&headers).as_millis(),
            u128::from(RETRY_AFTER_MAX_MS)
        );
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "soon".parse().expect("a header"),
        );
        assert_eq!(backoff(&headers).as_millis(), u128::from(RETRY_AFTER_MS));
        for status in [408, 425, 429, 500, 502, 503, 504] {
            assert!(retryable(
                reqwest::StatusCode::from_u16(status).expect("a status")
            ));
        }
        for status in [400, 401, 402, 404, 501] {
            assert!(!retryable(
                reqwest::StatusCode::from_u16(status).expect("a status")
            ));
        }
    }

    /// A proxy nothing listens on is asked once more and then said as unreachable; a call that
    /// timed out is said at once, never asked again.
    #[tokio::test]
    async fn an_unreachable_proxy_and_a_timed_out_call_say_what_happened() {
        let closed = tokio::net::TcpSocket::new_v4().expect("a socket");
        closed
            .bind("127.0.0.1:0".parse().expect("an address"))
            .expect("bound");
        let port = closed.local_addr().expect("an address").port();
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = format!("http://127.0.0.1:{port}");
        let err = driver
            .complete_once("rules", "hi", 100, false, &[])
            .await
            .expect_err("unreachable");
        assert!(err.said(100).contains("could not be reached"));

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(chat("late"))
                    .set_delay(std::time::Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let mut driver = driver_at(&server, "openrouter").await;
        driver.http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .expect("a client");
        let err = driver
            .complete_once("rules", "hi", 100, false, &[])
            .await
            .expect_err("timed out");
        assert!(err.said(100).contains("did not answer in time"));
        assert_eq!(calls(&server).await.len(), 1);
    }

    /// A 402 that names what the key can still pay for is asked again within it; one too small
    /// to be worth a call, or with no figure, is said as the credit running out.
    #[tokio::test]
    async fn short_credit_is_asked_again_within_it_and_else_said() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(402).set_body_json(
                json!({ "error": { "message": "This request can only afford 10000 tokens" } }),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat("Within credit.")))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let text = driver
            .complete_within_as("rules", "hi", 20_000, false, &[])
            .await
            .expect("an answer within the credit");
        assert_eq!(text, "Within credit.");
        let budgets: Vec<Value> = calls(&server)
            .await
            .iter()
            .map(|body| body["max_tokens"].clone())
            .collect();
        assert_eq!(budgets, [json!(20_000), json!(9_500)]);

        for (body, said) in [
            (
                json!({ "error": { "message": "can only afford 100 tokens" } }),
                "about 100 more output tokens",
            ),
            (
                json!({ "error": { "message": "no credit" } }),
                "has run out;",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(402).set_body_json(body))
                .mount(&server)
                .await;
            let driver = driver_at(&server, "openrouter").await;
            let err = driver
                .complete_within_as("rules", "hi", 20_000, false, &[])
                .await
                .expect_err("no credit");
            assert!(err.contains(said), "{err}");
            assert_eq!(calls(&server).await.len(), 1);
        }
    }

    /// An empty answer is asked once more, by the whole call and by the tool call alike.
    #[tokio::test]
    async fn an_empty_answer_is_asked_once_more() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "content": [] })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "content": [{ "type": "text", "text": "Now." }] })),
            )
            .mount(&server)
            .await;
        let driver = driver_at(&server, "anthropic").await;
        assert_eq!(
            driver
                .complete_within_as("rules", "hi", 100, false, &[])
                .await
                .expect("an answer"),
            "Now."
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(chat("  ")))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(402).set_body_json(
                    json!({ "error": { "message": "can only afford 5000 tokens" } }),
                ),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let messages = [json!({ "role": "user", "content": "hi" })];
        let err = driver
            .complete_tools("rules", &messages, &[], 20_000)
            .await
            .expect_err("credit after the empty answer");
        assert!(
            matches!(
                err,
                CallError::Credit {
                    affordable: Some(5000)
                }
            ),
            "{err:?}"
        );
        assert_eq!(calls(&server).await.len(), 2);
    }

    /// A tool call short of credit is asked again within it.
    #[tokio::test]
    async fn a_tool_call_short_of_credit_is_asked_again_within_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(402).set_body_json(
                    json!({ "error": { "message": "can only afford 8000 tokens" } }),
                ),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "tool_calls": [
                    { "id": "c1", "function": { "name": "jc_list", "arguments": "{\"kind\":\"View\"}" } },
                    { "id": "c2", "function": { "name": "jc_raw", "arguments": { "a": 1 } } },
                    { "id": "c3", "function": { "name": "jc_broken", "arguments": "{" } }
                ] } }],
                "usage": { "prompt_tokens": 7, "completion_tokens": 3 },
            })))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let messages = [json!({ "role": "user", "content": "hi" })];
        let answer = driver
            .complete_tools("rules", &messages, &[], 20_000)
            .await
            .expect("an answer");
        assert_eq!(answer.text, None);
        let inputs: Vec<(&str, &Value)> = answer
            .calls
            .iter()
            .map(|call| (call.name.as_str(), &call.input))
            .collect();
        assert_eq!(
            inputs,
            [
                ("jc_list", &json!({ "kind": "View" })),
                ("jc_raw", &json!({ "a": 1 })),
                ("jc_broken", &Value::Null)
            ]
        );
        assert_eq!(
            (
                answer.input_tokens,
                answer.output_tokens,
                answer.usage_tokens
            ),
            (7, 3, 10)
        );
        assert_eq!(calls(&server).await[1]["max_tokens"], json!(7_600));
    }

    /// An Anthropic tool answer keeps its text parts and its tool uses; an answer with neither is
    /// empty.
    #[tokio::test]
    async fn an_anthropic_tool_answer_keeps_its_text_and_its_tool_uses() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [
                    { "type": "text", "text": "Reading." },
                    { "type": "tool_use", "id": "t1", "name": "jc_get", "input": { "n": 1 } },
                    { "type": "tool_use" },
                    { "type": "image" }
                ],
                "usage": { "input_tokens": 4, "output_tokens": 2 },
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "content": [] })))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "anthropic").await;
        let messages = [json!({ "role": "user", "content": "hi" })];
        let answer = driver
            .complete_tools_once("rules", &messages, &[], 100)
            .await
            .expect("an answer");
        assert_eq!(answer.text.as_deref(), Some("Reading."));
        assert_eq!(answer.calls.len(), 2);
        assert_eq!(
            (answer.calls[0].id.as_str(), answer.calls[0].name.as_str()),
            ("t1", "jc_get")
        );
        assert_eq!(
            (answer.calls[1].name.as_str(), &answer.calls[1].input),
            ("", &Value::Null)
        );
        assert_eq!(answer.usage_tokens, 6);
        assert!(matches!(
            driver
                .complete_tools_once("rules", &messages, &[], 100)
                .await,
            Err(CallError::Empty)
        ));
    }

    fn sse(frames: &[Value]) -> String {
        frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .chain(std::iter::once("data: [DONE]\n\n".to_owned()))
            .collect()
    }

    async fn streamed(body: String) -> Result<String, CallError> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let tools = [ToolSpec {
            name: "jc_list".into(),
            description: "lists".into(),
            input_schema: json!({ "type": "object" }),
        }];
        let answer = driver.complete_once("rules", "hi", 100, true, &tools).await;
        let body = &calls(&server).await[0];
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert_eq!(body["tools"][0]["function"]["name"], json!("jc_list"));
        answer
    }

    /// ADR-N-032: a streamed answer is its deltas joined with its tool calls as blocks, writing
    /// the prose as a partial; an error inside it, a cut and an empty stream each say so.
    #[tokio::test]
    async fn a_streamed_answer_is_joined_and_its_failures_are_said() {
        let delta = |content: &str| json!({ "choices": [{ "delta": { "content": content } }] });
        let text = streamed(sse(&[
            delta("Reading "),
            delta("the list."),
            json!({ "choices": [{ "delta": { "tool_calls": [
                { "index": 0, "function": { "name": "jc_list", "arguments": "{\"kind\":\"View\"}" } }
            ] } }] }),
        ]))
        .await
        .expect("an answer");
        assert!(text.starts_with("Reading the list."), "{text}");
        assert_eq!(
            tools_registry::calls(&text)[0].arguments,
            json!({ "kind": "View" })
        );

        let err = streamed(sse(&[json!({ "error": { "message": "overloaded" } })]))
            .await
            .expect_err("stopped");
        assert!(err.said(100).contains("stopped its answer part way"));

        let err = streamed(sse(&[
            json!({ "choices": [{ "delta": { "content": "half" }, "finish_reason": "length" }] }),
        ]))
        .await
        .expect_err("cut");
        assert!(err.said(100).contains("output budget of 100 tokens"));

        assert!(matches!(streamed(sse(&[])).await, Err(CallError::Empty)));
    }

    /// An upstream asked to stream that answers whole is read whole.
    #[tokio::test]
    async fn a_stream_answered_whole_is_read_whole() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "tool_calls": [
                    { "function": { "name": "search_catalog", "arguments": "{\"q\":\"bikes\"}" } }
                ] } }],
            })))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let text = driver
            .complete_once("rules", "hi", 100, true, &[])
            .await
            .expect("an answer");
        assert_eq!(data_query::search_calls(&text), ["bikes"]);
    }

    /// An answer that is not JSON, or that the provider cut, is said as such.
    #[test]
    fn a_broken_or_cut_whole_answer_is_said() {
        let err = whole_answer("<html>", 10).expect_err("not JSON");
        assert!(err.said(10).contains("not JSON"));
        let err = whole_answer(r#"{"stop_reason":"max_tokens"}"#, 10).expect_err("cut");
        assert!(err.said(10).contains("budget of 10 tokens"));
        assert!(whole_answer(r#"{"stop_reason":"end_turn"}"#, 10).is_ok());
        assert!(matches!(
            chat_content(&json!({ "choices": [] })),
            Err(CallError::Empty)
        ));
        assert!(matches!(messages_text(&json!({})), Err(CallError::Empty)));
        let text = messages_text(&json!({ "content": [
            { "text": "untyped " },
            { "type": "tool_use", "name": "search_catalog", "input": { "q": "x" } }
        ] }))
        .expect("an answer");
        assert!(text.starts_with("untyped "), "{text}");
        assert_eq!(data_query::search_calls(&text), ["x"]);
        assert_eq!(
            fence("search_catalog", json!("bare")),
            fence("search_catalog", json!({}))
        );
    }

    /// T-7923: a stream that breaks off after its first words fails with "stopped part way",
    /// having shown those words once as a partial; words arriving within the partial interval
    /// are not shown again.
    #[tokio::test]
    async fn a_stream_cut_off_mid_answer_says_it_stopped_part_way() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        let upstream = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a call");
            // The whole request first, so closing the socket is a short body and not a reset.
            let mut seen = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.expect("read");
                if read == 0 {
                    break;
                }
                seen.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&seen).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if seen.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let frame = |content: &str| {
                format!(
                    "data: {}\n\n",
                    json!({ "choices": [{ "delta": { "content": content } }] })
                )
            };
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 100000\r\n\r\n",
                )
                .await
                .expect("headers");
            for content in ["Reading ", "the list."] {
                socket
                    .write_all(frame(content).as_bytes())
                    .await
                    .expect("a frame");
                socket.flush().await.expect("flushed");
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
        });
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = format!("http://{address}");
        driver.provider = "openrouter".into();
        let err = driver
            .complete_once("rules", "hi", 100, true, &[])
            .await
            .expect_err("cut off");
        let said = err.said(100);
        assert!(said.contains("stopped part way"), "{said}");
        upstream.await.expect("the upstream ran");
        let partials = driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "partial")
            .count();
        assert_eq!(partials, 1);
    }

    /// T-7923: an Anthropic tool answer without content is empty, asked once more, and still
    /// no answer.
    #[tokio::test]
    async fn a_tool_answer_without_content_is_asked_again_and_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "usage": { "input_tokens": 3, "output_tokens": 0 }
            })))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "anthropic").await;
        let answer = driver
            .complete_tools(
                "rules",
                &[json!({ "role": "user", "content": "hi" })],
                &[],
                100,
            )
            .await;
        assert!(matches!(answer, Err(CallError::Empty)), "{answer:?}");
        assert_eq!(calls(&server).await.len(), 2);
    }

    /// T-7923: a type two needs name is read once; a second endpoint that holds none of the
    /// entities already read is read again whole, and a page that is no list adds nothing.
    #[tokio::test]
    async fn a_second_endpoint_without_the_read_ids_is_read_whole_and_a_non_list_adds_nothing() {
        use wiremock::matchers::{path, query_param};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/data/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": "urn:ngsi-ld:Bike:1", "type": "Bike", "n": 1 }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/data/endpoints/dockslug/ngsi-ld/v1/entities"))
            .and(query_param("id", "urn:ngsi-ld:Bike:1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/data/endpoints/dockslug/ngsi-ld/v1/entities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "not": "a list" })))
            .mount(&server)
            .await;
        let mut driver = driver_at(&server, "anthropic").await;
        driver.endpoints = vec![
            endpoints::RunEndpoint {
                name: "bikes".into(),
                slug: "bikeslug".into(),
                space: "bikes".into(),
            },
            endpoints::RunEndpoint {
                name: "docks".into(),
                slug: "dockslug".into(),
                space: "docks".into(),
            },
        ];
        driver.data_needs = json!([
            { "contextSpaceRef": "bikes", "types": ["Bike", "Bike"] },
            { "contextSpaceRef": "docks", "types": ["Bike"] }
        ]);
        assert_eq!(driver.types(), ["Bike"]);
        let samples = driver.samples(&["Bike".to_owned()]).await.expect("samples");
        assert_eq!(
            samples,
            json!({ "Bike": [{ "id": "urn:ngsi-ld:Bike:1", "type": "Bike", "n": 1 }] })
        );
        let docks = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path().contains("dockslug"))
            .count();
        assert_eq!(docks, 2, "asked by id, then whole");
    }

    /// T-7923: a page that is not JSON is no entities, and a status the store cannot set is
    /// the store's refusal.
    #[tokio::test]
    async fn a_page_that_is_not_json_and_a_status_of_no_run_are_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        assert!(driver
            .read_entities(&format!("{}/page", server.uri()))
            .await
            .is_err());
        assert!(driver.status(AgentRunStatus::Starting).await.is_err());
    }

    /// A page the proxy refuses is its status and the start of its body.
    #[tokio::test]
    async fn a_refused_read_is_its_status_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string("x".repeat(500)))
            .mount(&server)
            .await;
        let driver = driver_at(&server, "openrouter").await;
        let err = driver
            .read_entities(&format!("{}/page", server.uri()))
            .await
            .expect_err("refused");
        assert_eq!(err, format!("403 Forbidden: {}", "x".repeat(200)));
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        assert!(driver.read_entities("http://127.0.0.1:1/").await.is_err());
    }
}

#[cfg(test)]
mod personal_tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    /// JC1-03, AG-158: the samples a code pass sends the model provider leave out what the
    /// project's models mark personal; when those models cannot be read, no sample is sent.
    #[tokio::test]
    async fn samples_whose_personal_marks_cannot_be_read_never_reach_the_model() {
        let proxy = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!([{
                "id": "urn:ngsi-ld:PotholeReport:1",
                "type": "PotholeReport",
                "contactEmail": "jana@example.org"
            }])))
            .mount(&proxy)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.proxy_base = proxy.uri();
        let types = ["PotholeReport".to_owned()];

        // A project without models marks nothing: the sample passes whole.
        let whole = driver.samples(&types).await.expect("samples");
        assert!(whole.to_string().contains("jana@example.org"), "{whole}");

        // A model the Portal cannot read (no forge here): nothing could be withheld.
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "DataModel".into(),
            metadata: ObjectMeta::new("reports", "helsinki"),
            spec: json!({ "linkml": "model.linkml.yaml" }),
            status: None,
        });
        let refused = driver.samples(&types).await.expect("samples");
        assert!(
            !refused.to_string().contains("jana@example.org"),
            "{refused}"
        );
    }
}
