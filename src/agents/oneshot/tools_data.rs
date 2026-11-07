//! The data tools the model calls on the endpoints a run opened: open, query, read, write (AG-70).

use super::*;

impl Driver {
    /// Each endpoint's read tools, from its own `tools/list` through the proxy, so the model
    /// sees exactly what the gateway offers this person there (AG-75). An endpoint that does
    /// not answer offers nothing.
    pub(super) async fn data_tools(&self, chosen: &[endpoints::RunEndpoint]) -> Vec<Vec<Value>> {
        let lists = (0..chosen.len()).map(|index| self.tools_of(chosen, index));
        futures_util::future::join_all(lists).await
    }

    /// The query cards of the endpoints the conversation reads and `cards` has no answer for yet,
    /// read at once and kept for the turn (T-4760). One that does not answer has none for now.
    pub(super) async fn read_cards(
        &self,
        chosen: &[endpoints::RunEndpoint],
        cards: &mut BTreeMap<String, Option<String>>,
    ) {
        let missing: Vec<usize> = (0..chosen.len())
            .filter(|&index| !cards.contains_key(&chosen[index].name))
            .collect();
        let read = futures_util::future::join_all(
            missing.iter().map(|&index| self.card_of(chosen, index)),
        )
        .await;
        for (index, card) in missing.into_iter().zip(read) {
            cards.insert(chosen[index].name.clone(), card);
        }
    }

    /// One endpoint's `card://{slug}` through the proxy with the person's own token: the card the
    /// gateway serves them, so it names only what their grants read (T-4759, T-4760).
    async fn card_of(&self, chosen: &[endpoints::RunEndpoint], index: usize) -> Option<String> {
        let endpoint = chosen.get(index)?;
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index)
        );
        let read = json!({
            "jsonrpc": "2.0", "id": 1, "method": "resources/read",
            "params": { "uri": format!("card://{}", endpoint.slug) },
        });
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(&read)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            tracing::info!(run = %self.run_id, endpoint = %endpoint.name, status = %response.status(), "an endpoint served no query card");
            return None;
        }
        let body: Value = crate::capped::json(response, crate::capped::ANSWER_BYTES)
            .await
            .ok()?;
        body["result"]["contents"][0]["text"]
            .as_str()
            .map(str::to_owned)
    }

    /// The read tools one endpoint of the conversation offers, from its own `tools/list`. One
    /// that does not answer offers nothing for now, and says why in the log (T-3044).
    pub(super) async fn tools_of(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
    ) -> Vec<Value> {
        self.listed_tools(chosen, index)
            .await
            .unwrap_or_else(|reason| {
                tracing::warn!(
                    run = %self.run_id,
                    endpoint = chosen.get(index).map_or("", |e| e.name.as_str()),
                    %reason,
                    "an endpoint of the conversation did not list its tools"
                );
                Vec::new()
            })
    }

    /// One endpoint's `tools/list`, or why it did not answer: the status and the problem's own
    /// detail, never a header or a token.
    async fn listed_tools(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
    ) -> Result<Vec<Value>, String> {
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index)
        );
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .send()
            .await
            .map_err(|err| format!("the platform could not be reached ({err})"))?;
        let status = response.status();
        let body = crate::capped::json::<Value>(response, crate::capped::ANSWER_BYTES)
            .await
            .unwrap_or(Value::Null);
        if !status.is_success() {
            let detail = body["detail"].as_str().unwrap_or_default();
            return Err(format!("the platform answered {status} {detail}")
                .trim_end()
                .to_owned());
        }
        if let Some(error) = body.get("error") {
            return Err(format!("the endpoint answered {error}"));
        }
        Ok(data_query::read_only_tools(&body))
    }

    /// The project's endpoints the conversation does not read yet and the person may open: an
    /// audience that admits the project and a profile that grants reading it (AG-58, AG-70).
    pub(super) fn openable_endpoints(
        &self,
        chosen: &[endpoints::RunEndpoint],
    ) -> Vec<data_query::Openable> {
        // Shared Endpoints come back from `shared_endpoints` already admitted to the project.
        let shared = crate::api::assistant::shared_endpoints(&self.state.mirror, &self.project)
            .into_iter()
            .map(|(_, endpoint)| endpoint);
        self.state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .into_iter()
            .chain(shared)
            .filter(|env| !chosen.iter().any(|c| c.name == env.metadata.name))
            .filter(|env| {
                env.spec["slug"]
                    .as_str()
                    .is_some_and(|slug| !slug.is_empty())
            })
            .filter(|env| {
                crate::api::assistant::endpoint_access(&env.spec, &self.project).is_allowed()
            })
            .filter(|env| self.access.grants_endpoint(&env.metadata.name, false))
            .map(|env| data_query::Openable {
                title: crate::api::assistant::title_of(&env),
                space: crate::api::assistant::ref_name(&env.spec["contextSpaceRef"])
                    .unwrap_or_default(),
                name: env.metadata.name,
            })
            .collect()
    }

    /// The index of `name` among the conversation's endpoints, opening it first when the person
    /// may read it: stored on the run and shown in the data bar, as if the person had used it
    /// (AG-76). `Err` is what the model reads back.
    pub(super) async fn open_endpoint(
        &self,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        name: &str,
    ) -> Result<usize, String> {
        if let Some(index) = chosen.iter().position(|e| e.name == name) {
            return Ok(index);
        }
        let openable = self.openable_endpoints(chosen);
        if !openable.iter().any(|o| o.name == name) {
            if let Some(why) = self.not_live(name).await {
                return Err(why);
            }
            return Err(format!(
                "'{name}' is not an endpoint the person may read in this project; they may open: {}",
                openable
                    .iter()
                    .map(|o| o.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if chosen.len() >= endpoints::MAX_ENDPOINTS {
            return Err(format!(
                "a conversation reads at most {} endpoints: {}",
                endpoints::MAX_ENDPOINTS,
                chosen
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let opened =
            endpoints::resolve_readable(&self.state.mirror, &self.project, &[name.to_owned()])
                .map_err(|err| err.to_string())?;
        chosen.extend(opened);
        self.state
            .agents
            .set_endpoints(&self.run_id, chosen)
            .await
            .map_err(|err| err.to_string())?;
        self.event(
            "endpoints",
            json!({ "names": chosen.iter().map(|e| e.name.clone()).collect::<Vec<_>>() }),
        )
        .await?;
        let index = chosen.len() - 1;
        let offered = self.tools_of(chosen, index).await;
        tools.resize(chosen.len() - 1, Vec::new());
        tools.push(offered);
        Ok(index)
    }

    /// One `tools/call` on an endpoint of the conversation, on the log as a `query_endpoint`
    /// step; what the model reads back, errors included, so it can correct itself.
    pub(super) async fn query_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        call: &data_query::QueryCall,
        tools: &[Vec<Value>],
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let input =
            json!({ "endpoint": call.endpoint, "name": call.name, "arguments": call.arguments });
        let index = chosen.iter().position(|e| e.name == call.endpoint);
        let refusal = match index {
            None => Some(format!(
                "'{}' is not an endpoint of this conversation; the endpoints are: {}",
                call.endpoint,
                chosen
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Some(i) => {
                let offered = tools.get(i).map_or(&[][..], Vec::as_slice);
                if data_query::offers(offered, &call.name) {
                    None
                } else if offered.is_empty() {
                    // An empty list may be one that failed: ask again before telling the model
                    // the endpoint offers nothing, which it then told the person (T-3044).
                    match self.listed_tools(chosen, i).await {
                        Ok(fresh) if data_query::offers(&fresh, &call.name) => None,
                        Ok(fresh) => {
                            Some(data_query::not_offered(&fresh, &call.name, &call.endpoint))
                        }
                        Err(reason) => Some(format!(
                            "endpoint '{}' could not be read just now: {reason}. Tell the person \
                             it could not be read just now; do not say it offers no data",
                            call.endpoint
                        )),
                    }
                } else {
                    Some(data_query::not_offered(offered, &call.name, &call.endpoint))
                }
            }
        };
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reason) = refusal {
            self.event(
                "tool",
                json!({ "tool": "query_endpoint", "status": "failed", "durationMs": millis(), "input": input, "error": reason }),
            )
            .await?;
            return Ok(format!("error: {reason}"));
        }
        let index = index.unwrap_or(0);
        let mut answer = data_query::compact_geometry(
            &self
                .call_endpoint(chosen, index, &data_query::rpc(call))
                .await,
        );
        let withheld = match self.withhold_personal(chosen, index, &mut answer).await {
            Ok(withheld) => withheld,
            Err(reason) => {
                self.event(
                    "tool",
                    json!({ "tool": "query_endpoint", "status": "failed", "durationMs": millis(), "input": input, "error": reason }),
                )
                .await?;
                return Ok(format!("error: {reason}"));
            }
        };
        let text = self.redacted(&format!(
            "{}{}{withheld}",
            data_query::result_text(&answer),
            data_query::reading_notes(call, &answer)
        ));
        let failed = answer.get("error").is_some()
            || answer.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
        let mut payload = json!({
            "tool": "query_endpoint",
            "status": if failed { "failed" } else { "ok" },
            "durationMs": millis(),
            "input": input,
            "output": answer.get("result").cloned().unwrap_or(Value::Null),
        });
        if failed {
            payload["error"] = Value::String(text.clone());
        }
        self.event("tool", payload).await?;
        Ok(text)
    }

    /// Removes from an endpoint's answer the attributes its project's models mark personal
    /// before the model provider reads it (AG-158, JC1-03): from `structuredContent`, and the
    /// text beside it is rewritten from what is left. `Ok` is the note naming what left (empty
    /// when nothing did); `Err` refuses the answer, whose models could not be read.
    // ponytail: an answer with entities in its text alone is not parsed; every gateway and
    // Antares tool declares an outputSchema, so its answer carries structuredContent (AG-12).
    async fn withhold_personal(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
        answer: &mut Value,
    ) -> Result<String, String> {
        let Some(result) = answer.get_mut("result") else {
            return Ok(String::new());
        };
        let carries = result
            .get("structuredContent")
            .is_some_and(crate::personal_data::carries_entities);
        if !carries {
            return Ok(String::new());
        }
        let project = chosen
            .get(index)
            .and_then(|endpoint| endpoint_project(&self.state, &endpoint.slug))
            .unwrap_or_else(|| self.project.clone());
        let marks = crate::personal_data::of_project(&self.state, &project).await?;
        let removed = crate::personal_data::withhold(&mut result["structuredContent"], &marks);
        if removed.is_empty() {
            return Ok(String::new());
        }
        let text = result["structuredContent"].to_string();
        result["content"] = json!([{ "type": "text", "text": text }]);
        Ok(format!(
            "\nwithheld personal attributes (the model marks them personal; the person reads them on the Portal): {}",
            removed.join(", ")
        ))
    }

    /// Why `name` answers nothing yet, when it is an endpoint on its way (T-2763): its Change
    /// waits for approval, or it is only a draft. A test of what was just drafted is then an
    /// honest "not live", never a query that failed for no reason the person can act on.
    async fn not_live(&self, name: &str) -> Option<String> {
        // A forge that cannot be read leaves the draft to say what it can.
        let open =
            crate::api::changes::list_changes_readable(&self.state, &self.identity, &self.project)
                .await
                .map(|list| list.items)
                .unwrap_or_default();
        if let Some(why) = pending_endpoint(&open, name) {
            return Some(why);
        }
        let effective = crate::permissions::for_request(&self.state, &self.identity, &self.project);
        let drafted = self
            .state
            .drafts
            .list(&self.project)
            .await
            .ok()?
            .into_iter()
            .any(|draft| {
                draft.workspace.is_none()
                    && draft.kind == "Endpoint"
                    && draft.name == name
                    && effective.may_read_manifest(&draft.kind, &draft.manifest)
            });
        drafted.then(|| {
            format!(
                "the endpoint '{name}' is a draft, not a live endpoint: nothing reads through it \
                 until the person proposes it from the endpoint form and its change is approved. \
                 Tell the person so, and query it once it is live; nothing was tested"
            )
        })
    }

    /// One JSON-RPC request to an endpoint of the conversation through the proxy, with the
    /// person's grants: the endpoint's answer, or `{error}` saying why there is none.
    pub(super) async fn call_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
        rpc: &Value,
    ) -> Value {
        let url = format!(
            "{}/mcp",
            endpoints::data_base(&self.proxy_base, chosen, index)
        );
        match self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .header("accept", "application/json")
            .json(rpc)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let body = crate::capped::text(response, crate::capped::ANSWER_BYTES)
                    .await
                    .unwrap_or_default();
                if status.is_success() {
                    serde_json::from_str::<Value>(&body).unwrap_or_else(|err| {
                        unreachable_endpoint(format!("the endpoint's answer is not JSON: {err}"))
                    })
                } else {
                    // The gateway's own problem document goes through as it is.
                    match serde_json::from_str::<Value>(&body) {
                        Ok(problem) if problem.get("title").is_some() => {
                            json!({ "error": problem })
                        }
                        _ => unreachable_endpoint(format!(
                            "the endpoint answered {status}: {}",
                            body.chars().take(300).collect::<String>()
                        )),
                    }
                }
            }
            Err(err) => {
                unreachable_endpoint(format!("the call did not go through the proxy: {err}"))
            }
        }
    }

    /// What one read tool of an endpoint answered, structured; `Err` is the refusal in words.
    pub(super) async fn read_endpoint(
        &self,
        chosen: &[endpoints::RunEndpoint],
        index: usize,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let call = data_query::QueryCall {
            endpoint: String::new(),
            name: name.to_owned(),
            arguments,
        };
        let answer = self
            .call_endpoint(chosen, index, &data_query::rpc(&call))
            .await;
        let refused = answer.get("error").is_some()
            || answer.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
        if refused {
            return Err(self.redacted(&data_query::result_text(&answer)));
        }
        Ok(answer
            .pointer("/result/structuredContent")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// A change to entities prepared for the person (AG-78): their grants on the endpoint and
    /// each entity are read through the proxy, and the preview is published for the card to
    /// apply. Nothing is written; what the grants refuse goes back to the model with the reason.
    pub(super) async fn write_entities(
        &self,
        call: Result<entity_write::WriteEntities, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "write_entities";
        let started = std::time::Instant::now();
        let input = call
            .as_ref()
            .ok()
            .and_then(|c| serde_json::to_value(c).ok())
            .unwrap_or(Value::Null);
        let prepared = match call {
            Ok(call) => self.prepared_write(&call, chosen, tools).await,
            Err(reason) => Err(reason),
        };
        let output = match prepared {
            Ok(output) => output,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The change could not be prepared: {reason}"),
                    )
                    .await;
            }
        };
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "output": output,
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = "Review the values and apply the change.".to_owned();
        }
        self.thought(&prose).await?;
        Ok(Worked::Done(prose))
    }

    /// The preview of a `write_entities` call: every entity the person's grants let them update,
    /// as it is and as it would be.
    pub(super) async fn prepared_write(
        &self,
        call: &entity_write::WriteEntities,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
    ) -> Result<Value, String> {
        entity_write::checked(call)?;
        let index = self.open_endpoint(chosen, tools, &call.endpoint).await?;
        let access = self
            .read_endpoint(chosen, index, "describe_access", json!({}))
            .await
            .map_err(|reason| {
                format!(
                    "the grants on '{}' could not be read: {reason}",
                    call.endpoint
                )
            })?;
        let mut entities = Vec::new();
        for change in &call.entities {
            let entity_type = entity_write::type_of(&change.id).unwrap_or_default();
            let attributes: Vec<&str> = change.attrs.keys().map(String::as_str).collect();
            if let Some(reason) = entity_write::refusal(&access, entity_type, &attributes) {
                return Err(format!("{}: {reason}", change.id));
            }
            let current = self
                .read_endpoint(chosen, index, "get_entity", json!({ "id": change.id }))
                .await
                .map_err(|reason| format!("{} could not be read: {reason}", change.id))?;
            entities.push(entity_write::previewed(change, entity_type, &current));
        }
        Ok(json!({
            "endpoint": call.endpoint,
            "slug": chosen[index].slug,
            "entities": entities,
        }))
    }
}

/// The Change an endpoint of `name` waits on, as the sentence the model reads (T-2763).
fn pending_endpoint(open: &[crate::api::changes::ChangeProposal], name: &str) -> Option<String> {
    use crate::change::ChangePhase;
    let change = open.iter().find(|change| {
        matches!(
            change.status.phase,
            ChangePhase::PendingApproval | ChangePhase::Deploying
        ) && change.summary.params.get("kind").and_then(Value::as_str) == Some("Endpoint")
            && change.summary.params.get("name").and_then(Value::as_str) == Some(name)
    })?;
    let waits = if change.status.phase == ChangePhase::Deploying {
        "is approved and still deploying"
    } else {
        "waits for approval"
    };
    Some(format!(
        "the endpoint '{name}' is not live yet: its change {} {waits}, and nothing reads through \
         it before it is deployed. Tell the person so, name the change, and query it once it is \
         live; nothing was tested",
        change.metadata.name
    ))
}

/// The project of the Endpoint with `slug`: the conversation's own, or the one a shared space
/// reference reaches.
fn endpoint_project(state: &AppState, slug: &str) -> Option<String> {
    state
        .mirror
        .matching(|env| {
            env.kind == "Endpoint" && env.spec.get("slug").and_then(Value::as_str) == Some(slug)
        })
        .into_iter()
        .find_map(|env| env.metadata.namespace)
}

/// A call that reached no answer, as the JSON-RPC `error` member holding the Portal's problem
/// document: the model reads one refusal shape from every door (JC1-57).
fn unreachable_endpoint(detail: String) -> Value {
    json!({ "error": crate::error::ApiError::Unavailable(detail).into_document().1 })
}

#[cfg(test)]
mod tests {
    //! `open_endpoint` and `openable_endpoints` (T-2515; AG-58, AG-70, AG-76): an endpoint a
    //! conversation opens mid-run.

    use super::*;
    use crate::agents::access::Access;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::state::AppState;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn endpoint(state: &AppState, name: &str, spec: Value) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        });
    }

    fn open(slug: &str, audience: &str) -> Value {
        json!({ "slug": slug, "audience": audience, "contextSpaceRef": "bikes" })
    }

    /// A project of seven endpoints: five open to it, one shut to it by its audience, one that
    /// has no slug yet. The proxy answers every `tools/list` with one read tool named by slug.
    async fn world() -> (Driver, AppState, MockServer) {
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(
                "^/v1/data(/endpoints/[a-z0-9-]+)?/mcp$",
            ))
            .respond_with(|request: &wiremock::Request| {
                let slug = request
                    .url
                    .path()
                    .split('/')
                    .nth(4)
                    .unwrap_or("primary")
                    .to_owned();
                ResponseTemplate::new(200).set_body_json(
                    json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
                    { "name": format!("read-{slug}"), "annotations": { "readOnlyHint": true } },
                    { "name": "write", "annotations": { "readOnlyHint": false } },
                ] } }),
                )
            })
            .mount(&proxy)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        for (name, spec) in [
            ("a", open("slug-a", "public")),
            ("b", open("slug-b", "organization")),
            ("c", open("slug-c", "public")),
            ("d", open("slug-d", "public")),
            ("e", open("slug-e", "public")),
            ("f", open("slug-f", "public")),
            (
                "shut",
                json!({ "slug": "slug-shut", "audience": "project-list", "allowedProjects": ["espoo"] }),
            ),
            ("odd", open("slug-odd", "friends")),
            ("unslugged", json!({ "slug": "", "audience": "public" })),
            ("slugless", json!({ "audience": "public" })),
        ] {
            endpoint(&state, name, spec);
        }
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.proxy_base = proxy.uri();
        (driver, state, proxy)
    }

    fn names(openable: &[data_query::Openable]) -> Vec<&str> {
        openable.iter().map(|o| o.name.as_str()).collect()
    }

    fn run_endpoint(name: &str) -> endpoints::RunEndpoint {
        endpoints::RunEndpoint {
            name: name.into(),
            slug: format!("slug-{name}"),
            space: "bikes".into(),
        }
    }

    async fn logged(state: &AppState, kind: &str) -> Vec<Value> {
        state
            .agents
            .events_since("test-run", 0)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.payload)
            .collect()
    }

    /// T-2515, AG-58: an endpoint whose audience does not admit the project is refused, and the
    /// refusal lists only what may be opened.
    #[tokio::test]
    async fn an_endpoint_the_person_may_not_read_is_refused_and_lists_only_what_they_may_open() {
        let (driver, state, proxy) = world().await;
        for name in ["shut", "odd", "unslugged"] {
            let (mut chosen, mut tools) = (Vec::new(), Vec::new());
            let refused = driver
                .open_endpoint(&mut chosen, &mut tools, name)
                .await
                .expect_err(name);
            assert_eq!(
                refused,
                format!("'{name}' is not an endpoint the person may read in this project; they may open: a, b, c, d, e, f")
            );
            assert!(chosen.is_empty() && tools.is_empty(), "{name}");
        }
        assert!(logged(&state, "endpoints").await.is_empty());
        assert!(proxy
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    /// T-2515, AG-70: a profile that lists its endpoints narrows the person's: an endpoint it
    /// leaves out is neither offered nor opened, and a profile granting only `write` grants none.
    #[tokio::test]
    async fn an_endpoint_the_profile_narrows_out_is_absent_from_openable_even_if_the_person_may_read_it(
    ) {
        let (mut driver, _state, _proxy) = world().await;
        driver.access = Access::from_spec(&json!({ "access": { "operations": [], "endpoints": [
            { "name": "a", "verbs": ["read"] },
            { "name": "b", "verbs": ["write"] },
        ] } }));
        assert_eq!(names(&driver.openable_endpoints(&[])), vec!["a"]);
        for name in ["b", "c"] {
            let refused = driver
                .open_endpoint(&mut Vec::new(), &mut Vec::new(), name)
                .await
                .expect_err(name);
            assert!(refused.ends_with("they may open: a"), "{refused}");
        }
    }

    /// T-2515: a name nobody published answers exactly like one the person may not read, so the
    /// answer never tells the two apart.
    #[tokio::test]
    async fn a_name_that_does_not_exist_is_refused_the_same_as_one_the_person_cannot_read() {
        let (driver, _state, _proxy) = world().await;
        let mut answers = Vec::new();
        for name in ["shut", "no-such-endpoint", "", "A", "a ", "../a"] {
            let refused = driver
                .open_endpoint(&mut Vec::new(), &mut Vec::new(), name)
                .await
                .expect_err(name);
            answers.push(refused.replacen(&format!("'{name}'"), "'…'", 1));
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "{answers:#?}"
        );
    }

    /// T-2515, AG-76: at most `MAX_ENDPOINTS`; the refusal names the set the conversation reads.
    #[tokio::test]
    async fn the_open_past_max_endpoints_is_refused_naming_the_current_set() {
        let (driver, state, _proxy) = world().await;
        let (mut chosen, mut tools) = (Vec::new(), Vec::new());
        for name in ["a", "b", "c", "d", "e"] {
            driver
                .open_endpoint(&mut chosen, &mut tools, name)
                .await
                .expect(name);
        }
        assert_eq!(chosen.len(), endpoints::MAX_ENDPOINTS);
        let refused = driver
            .open_endpoint(&mut chosen, &mut tools, "f")
            .await
            .expect_err("a sixth");
        assert_eq!(
            refused,
            "a conversation reads at most 5 endpoints: a, b, c, d, e"
        );
        assert_eq!(chosen.len(), 5);
        assert_eq!(tools.len(), 5);
        assert_eq!(logged(&state, "endpoints").await.len(), 5);
    }

    /// T-2515: an endpoint the conversation already reads is its index again: nothing stored,
    /// logged or asked of the proxy. Two opens at once cannot race: `chosen` is `&mut`, so the
    /// borrow checker serialises them (the `two_concurrent_opens…` case is struck).
    #[tokio::test]
    async fn an_endpoint_already_chosen_is_a_no_op_returning_its_existing_index() {
        let (driver, state, proxy) = world().await;
        // Already chosen when the run started, so even a name the audience now shuts is its index.
        let mut chosen = vec![run_endpoint("a"), run_endpoint("shut")];
        let mut tools = vec![vec![json!({ "name": "kept" })], Vec::new()];
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "shut").await,
            Ok(1)
        );
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "a").await,
            Ok(0)
        );
        assert_eq!(chosen.len(), 2);
        assert_eq!(tools, vec![vec![json!({ "name": "kept" })], Vec::new()]);
        assert!(logged(&state, "endpoints").await.is_empty());
        assert!(proxy
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    /// T-2515, EP-02: the gateway addresses an endpoint by its slug; one without a slug is never
    /// offered, and neither is one already chosen.
    #[tokio::test]
    async fn an_endpoint_with_an_empty_slug_is_never_offered() {
        let (driver, _state, _proxy) = world().await;
        let offered = driver.openable_endpoints(&[run_endpoint("c")]);
        assert_eq!(names(&offered), vec!["a", "b", "d", "e", "f"]);
        assert!(offered.iter().all(|o| o.space == "bikes"));
    }

    /// T-2515, AG-76: each open stores the run's endpoints and logs every name the conversation
    /// now reads, in order, as the data bar shows them.
    #[tokio::test]
    async fn opening_records_an_endpoints_event_with_every_chosen_name() {
        let (driver, state, _proxy) = world().await;
        let mut chosen = vec![run_endpoint("a")];
        let mut tools = vec![Vec::new()];
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "c").await,
            Ok(1)
        );
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "b").await,
            Ok(2)
        );
        assert_eq!(
            logged(&state, "endpoints").await,
            vec![
                json!({ "names": ["a", "c"] }),
                json!({ "names": ["a", "c", "b"] })
            ]
        );
        assert_eq!(chosen[2], run_endpoint("b"));
    }

    /// T-2515, AG-75: the new endpoint's read tools land in its own slot, after the earlier
    /// endpoints' tools, which stay as they were; a list shorter than the endpoints is padded.
    #[tokio::test]
    async fn tools_of_the_newly_opened_endpoint_are_appended_not_replacing_earlier_slots() {
        let (driver, _state, _proxy) = world().await;
        let earlier = vec![json!({ "name": "read-first" })];
        let mut chosen = vec![run_endpoint("a"), run_endpoint("b")];
        let mut tools = vec![earlier.clone()];
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "d").await,
            Ok(2)
        );
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0], earlier);
        assert_eq!(tools[1], Vec::<Value>::new());
        let offered: Vec<&str> = tools[2].iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            offered,
            vec!["read-slug-d"],
            "only the read tools, of that endpoint"
        );
    }

    /// T-2515: `resolve` reads the same mirror, with no await in between, for the two things
    /// `openable_endpoints` already required (the Endpoint exists, its slug is not empty), so a
    /// resolve failure after the check is struck. What can fail after the check is the store or
    /// the log, and that is an error, never an index.
    #[tokio::test]
    async fn resolve_failing_after_the_grant_check_passed_returns_an_error_not_a_partial_open() {
        let (mut driver, _state, _proxy) = world().await;
        let mut chosen = Vec::new();
        let mut tools = Vec::new();
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "a").await,
            Ok(0)
        );
        assert_eq!(chosen, vec![run_endpoint("a")]);
        assert_eq!(tools.len(), chosen.len());
        driver.proxy_base = "http://127.0.0.1:9".into();
        // A proxy that does not answer tools/list opens the endpoint with no tools, not an error.
        assert_eq!(
            driver.open_endpoint(&mut chosen, &mut tools, "b").await,
            Ok(1)
        );
        assert_eq!(tools[1], Vec::<Value>::new());
    }

    fn proposal(
        kind: &str,
        name: &str,
        phase: crate::change::ChangePhase,
    ) -> crate::api::changes::ChangeProposal {
        let mut params = serde_json::Map::new();
        params.insert("kind".into(), json!(kind));
        params.insert("name".into(), json!(name));
        crate::api::changes::ChangeProposal {
            api_version: API_VERSION.to_owned(),
            kind: "Change".to_owned(),
            metadata: crate::change::ChangeMeta::from_merge_request(0x22e, "helsinki"),
            status: crate::change::ChangeStatus::new(
                crate::change::Lane::Yellow,
                phase,
                crate::change::PlanSummary::new(1, 0, 0),
            ),
            summary: crate::api::changes::ChangeSummary {
                key: "change.summary.create".to_owned(),
                params,
            },
            author: crate::api::changes::ChangeAuthor {
                name: "Piper".to_owned(),
                email: None,
            },
            created_at: "2026-09-25T02:00:00Z".to_owned(),
            plan_fields: None,
            files: None,
            file_count: None,
            workspace: None,
            waits_on: Vec::new(),
            decision: None,
            ci: None,
            space: None,
        }
    }

    /// T-2763: an endpoint on its way names the change it waits on; a merged one, another kind
    /// or another name is not what the person asked to test.
    #[test]
    fn an_endpoint_on_its_way_names_the_change_it_waits_on() {
        use crate::change::ChangePhase;
        let open = [
            proposal("Endpoint", "sample-endpoint", ChangePhase::PendingApproval),
            proposal("Endpoint", "bikes", ChangePhase::Deploying),
            proposal("Endpoint", "air", ChangePhase::Merged),
            proposal("ContextSpace", "trams", ChangePhase::PendingApproval),
        ];
        let waits = pending_endpoint(&open, "sample-endpoint").expect("waits");
        assert!(
            waits.contains("its change chg-0000022e waits for approval")
                && waits.contains("nothing was tested"),
            "{waits}"
        );
        let deploying = pending_endpoint(&open, "bikes").expect("deploying");
        assert!(
            deploying.contains("is approved and still deploying"),
            "{deploying}"
        );
        for name in ["air", "trams", "unknown"] {
            assert_eq!(pending_endpoint(&open, name), None, "{name}");
        }
    }

    /// T-4760: each endpoint's card is read once through the proxy as `card://{slug}`; one the
    /// gateway refuses has none, and its types are never put before the model.
    #[tokio::test]
    async fn the_cards_are_read_once_and_a_refused_one_is_none() {
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::path("/v1/data/mcp"))
            .and(wiremock::matchers::body_string_contains("card://slug-a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "jsonrpc": "2.0", "id": 1,
                "result": { "contents": [{ "uri": "card://slug-a", "mimeType": "text/plain", "text": "NGSI-LD Endpoint helsinki/a" }] } })))
            .expect(1)
            .mount(&proxy)
            .await;
        Mock::given(method("POST"))
            .and(wiremock::matchers::path("/v1/data/endpoints/slug-b/mcp"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&proxy)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = proxy.uri();
        let chosen = [run_endpoint("a"), run_endpoint("b")];
        let mut cards = BTreeMap::new();
        driver.read_cards(&chosen, &mut cards).await;
        driver.read_cards(&chosen, &mut cards).await;
        assert_eq!(
            cards.get("a"),
            Some(&Some("NGSI-LD Endpoint helsinki/a".to_owned()))
        );
        assert_eq!(cards.get("b"), Some(&None));
        let section = data_query::cards_section(&chosen, &cards);
        assert!(
            section.contains("`a`")
                && section.contains("```text\nNGSI-LD Endpoint helsinki/a\n```"),
            "{section}"
        );
        assert!(!section.contains("`b`"), "{section}");
    }

    /// JC1-03, AG-158: an endpoint's answer reaches the model provider only after the
    /// attributes its project's models mark personal leave it; when those models cannot be
    /// read, the answer is refused whole and its values reach neither the model nor the log.
    #[tokio::test]
    async fn an_endpoint_answer_whose_personal_marks_cannot_be_read_never_reaches_the_model() {
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": 1, "result": {
                    "content": [{ "type": "text", "text": "jana@example.org" }],
                    "structuredContent": { "entities": [{
                        "id": "urn:ngsi-ld:PotholeReport:1",
                        "type": "PotholeReport",
                        "contactEmail": { "type": "Property", "value": "jana@example.org" }
                    }] }
                }
            })))
            .mount(&proxy)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        endpoint(&state, "reports", open("slug-reports", "public"));
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.proxy_base = proxy.uri();
        let chosen = vec![run_endpoint("reports")];
        let tools = vec![vec![json!({ "name": "query_entities" })]];
        let call = data_query::QueryCall {
            endpoint: "reports".into(),
            name: "query_entities".into(),
            arguments: json!({ "type": "PotholeReport" }),
        };

        // A project without models marks nothing: the answer passes whole.
        let read = driver
            .query_endpoint(&chosen, &call, &tools)
            .await
            .expect("read");
        assert!(read.contains("jana@example.org"), "{read}");

        // A model the Portal cannot read (no forge here): nothing could be withheld.
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "DataModel".into(),
            metadata: ObjectMeta::new("reports", "helsinki"),
            spec: json!({ "linkml": "model.linkml.yaml" }),
            status: None,
        });
        let refused = driver
            .query_endpoint(&chosen, &call, &tools)
            .await
            .expect("refused");
        assert!(refused.starts_with("error: "), "{refused}");
        assert!(!refused.contains("jana@example.org"), "{refused}");
        let last = logged(&state, "tool").await.pop().expect("logged");
        assert_eq!("failed", last["status"], "{last}");
        assert!(!last.to_string().contains("jana@example.org"), "{last}");
    }

    /// JC1-57: an endpoint call that reached no answer comes back as the problem document every
    /// other door answers, and the gateway's own problem passes through untouched.
    #[tokio::test]
    async fn an_endpoint_call_without_an_answer_is_a_problem_document() {
        let proxy = MockServer::start().await;
        Mock::given(wiremock::matchers::path("/v1/data/mcp"))
            .respond_with(ResponseTemplate::new(502).set_body_string("upstream down"))
            .mount(&proxy)
            .await;
        Mock::given(wiremock::matchers::path("/v1/data/endpoints/shut/mcp"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                json!({ "type": "about:blank", "title": "Forbidden", "status": 403, "detail": "no grant" }),
            ))
            .mount(&proxy)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = proxy.uri();
        let run = |slug: &str| endpoints::RunEndpoint {
            name: slug.into(),
            slug: slug.into(),
            space: "bikes".into(),
        };
        let chosen = [run("primary"), run("shut")];
        let down = driver.call_endpoint(&chosen, 0, &json!({})).await;
        assert_eq!(down["error"]["status"], 503, "{down}");
        let detail = down["error"]["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("502") && detail.contains("upstream down"),
            "{detail}"
        );
        let refused = driver.call_endpoint(&chosen, 1, &json!({})).await;
        assert_eq!(refused["error"]["detail"], "no grant", "{refused}");
        assert!(data_query::result_text(&refused).starts_with("error: "));
    }
}
