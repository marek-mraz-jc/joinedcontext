//! The drafting tools of the conversation: Complete this space, a KPI, a KPI pipeline, a share (AG-76, PL-45).

use super::*;

impl Driver {
    /// The `propose_endpoint` tool: the manifests rendered and published as a step, then a
    /// `navigate` that opens the endpoint form with them (EP-72, UI-45). A request that cannot
    /// be rendered is a failed step the person reads in the chat; nothing is written either way.
    /// The KPI step (T-0583, PF-54, PF-55): the entities read through the proxy like a
    /// sample, the aggregate computed, the indicator rendered and handed over as a `tool`
    /// event; the card the person sees carries the write, with their own session.
    pub(super) async fn space_complete(
        &self,
        input: Value,
        answer: &str,
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let Some(op) = crate::ops::find("jc_space_complete") else {
            return Err("jc_space_complete not found".into());
        };
        if let Err(reason) = self.granted("jc_space_complete") {
            return self.refused("space_complete", started, input, reason).await;
        }
        let caller = crate::ops::Caller {
            bearer: None,
            identity: self.identity.clone(),
            via: crate::ops::Via::Agent,
            access: None,
        };
        // The operation reads its own fields only: the call's `tool` key goes, and the
        // assistant never proposes, the person does on the page it opens (AG-73).
        let mut input = input;
        if let Some(fields) = input.as_object_mut() {
            fields.remove("tool");
            fields.remove("propose");
        }
        match crate::ops::call(op, &caller, &self.state, &self.project, input.clone()).await {
            Ok(mut output) => {
                // The verdicts' traces (the feed's records, megabytes) stay with the operation:
                // the chat and the hand-off carry the drafts and each verdict's answer, which is
                // what the page shows; a hand-off that large is refused by the browser's session
                // storage and the page opens empty (T-0891).
                without_traces(&mut output);
                let space_name = output
                    .get("space")
                    .and_then(Value::as_str)
                    .unwrap_or("space");
                self.event(
                    "tool",
                    json!({
                        "tool": "space_complete",
                        "status": "ok",
                        "durationMs": millis(started),
                        "input": input,
                        "output": output,
                    }),
                )
                .await?;
                let mut prose = share::prose_of(answer);
                if prose.is_empty() {
                    prose = format!("Completed drafts for context space '{space_name}'.");
                }
                self.thought(&prose).await?;
                // The drafts travel with the navigation: the page shows them ready to propose
                // instead of an empty form (AG-73).
                self.event(
                    "navigate",
                    json!({
                        "route": format!("/projects/{}/spaces/complete?space={}", self.project, space_name),
                        "prefill": { "result": output, "url": input.get("url") },
                    }),
                )
                .await?;
                Ok(prose)
            }
            Err(e) => {
                let reason = e.to_string();
                self.event(
                    "tool",
                    json!({
                        "tool": "space_complete",
                        "status": "failed",
                        "durationMs": millis(started),
                        "input": input,
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("Completing space failed: {reason}");
                self.thought(&prose).await?;
                Ok(prose)
            }
        }
    }

    pub(super) async fn kpi(
        &self,
        call: Result<kpi::ComputeKpi, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let failed = |reason: String| {
            json!({
                "tool": "compute_kpi",
                "status": "failed",
                "durationMs": millis(started),
                "error": reason,
            })
        };
        // A failure the model can correct goes back to it while drafts are left (AG-76).
        let again = |reason: String, prose: String| {
            if last {
                Worked::Done(prose)
            } else {
                Worked::Again(format!("error: {reason}"))
            }
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator request could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_kpi_compute") {
            return self
                .refused("compute_kpi", started, input, reason)
                .await
                .map(Worked::Done);
        }
        let named = params
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let index = match named {
            Some(name) => self.open_endpoint(chosen, tools, name).await,
            None if chosen.is_empty() => Err(
                "the conversation reads no endpoint: name the one the indicator reads (endpoint)"
                    .to_owned(),
            ),
            None => Ok(endpoints::of_type(
                chosen,
                &self.data_needs,
                &params.entity_type,
            )),
        };
        let index = match index {
            Ok(index) => index,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator has no endpoint to read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let mut url = format!(
            "{}/ngsi-ld/v1/entities?type={}&options=keyValues&limit=1000",
            endpoints::data_base(&self.proxy_base, chosen, index),
            urlencoding(&params.entity_type)
        );
        if !params.attribute.is_empty() {
            url.push_str(&format!("&attrs={}", urlencoding(&params.attribute)));
        }
        if let Some(q) = params.q.as_deref().filter(|q| !q.trim().is_empty()) {
            url.push_str(&format!("&q={}", urlencoding(q)));
        }
        let rows = match self.read_entities(&url).await {
            Ok(Value::Array(rows)) => rows,
            Ok(_) => Vec::new(),
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The entities could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(reason, prose));
            }
        };
        let (value, count) = kpi::compute(&rows, &params.attribute, params.agg);
        let Some(value) = value else {
            let reason = format!(
                "no {} carries a number in '{}' ({} read)",
                params.entity_type,
                params.attribute,
                rows.len()
            );
            self.event("tool", failed(reason.clone())).await?;
            let prose = format!("The indicator has no value: {reason}.");
            if last {
                self.thought(&prose).await?;
            }
            return Ok(again(reason, prose));
        };
        // The endpoint read, for the provenance; the indicator space's endpoint, when the
        // project has one, for the card's write.
        let (endpoint_name, endpoint_space) = match chosen.get(index) {
            Some(read) => (
                read.name.clone(),
                if read.space.is_empty() {
                    self.project.clone()
                } else {
                    read.space.clone()
                },
            ),
            None => (self.project.clone(), self.project.clone()),
        };
        let now = now_rfc3339();
        let provenance = kpi::Provenance {
            org_domain: &crate::api::assistant::org_domain(&self.state, &self.project),
            project: &self.project,
            endpoint_space: &endpoint_space,
            endpoint_name: &endpoint_name,
            run_id: &self.run_id,
            now: &now,
        };
        let indicator = match kpi::entity(&params, value, &provenance) {
            Ok(indicator) => indicator,
            Err(reason) => {
                self.event("tool", failed(reason.clone())).await?;
                let prose = format!("The indicator could not be rendered: {reason}");
                self.thought(&prose).await?;
                return Ok(Worked::Done(prose));
            }
        };
        let target = kpi::kpi_endpoint(&self.state, &self.project);
        self.event(
            "tool",
            json!({
                "tool": "compute_kpi",
                "status": "ok",
                "durationMs": millis(started),
                "input": input,
                "output": {
                    "name": params.name,
                    "title": params.title,
                    "value": value,
                    "unit": params.unit,
                    "formula": kpi::formula(&params),
                    "count": count,
                    "space": jc_core::kpi::kpi_space(&self.project),
                    "endpointSlug": target.as_ref().map(|(slug, _)| slug.clone()),
                    "endpointName": target.as_ref().map(|(_, name)| name.clone()),
                    "entity": indicator.to_json(),
                },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!(
                "{} = {value} over {count} entities; write it into the indicator space from the card.",
                kpi::formula(&params)
            );
        }
        self.thought(&prose).await?;
        Ok(Worked::Done(prose))
    }

    /// An indicator kept up to date (AG-74, PL-45, PL-51): the pipeline and, for an indicator
    /// space the project does not have, its space, endpoint and policies, kept as the person's
    /// drafts; the pipeline tested on a page of the source and its form opened. Nothing is
    /// proposed here.
    pub(super) async fn kpi_pipeline(
        &self,
        call: Result<kpi_pipeline::DraftKpiPipeline, String>,
        answer: &str,
        chosen: &mut Vec<endpoints::RunEndpoint>,
        tools: &mut Vec<Vec<Value>>,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "draft_kpi_pipeline";
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let failed = |input: &Value, reason: &str| {
            json!({
                "tool": TOOL,
                "status": "failed",
                "durationMs": millis(started),
                "input": input,
                "error": reason,
            })
        };
        // A refused plan or a red test goes back to the model while drafts are left (AG-76).
        let again = |reason: String, prose: String| {
            if last {
                Worked::Done(prose)
            } else {
                Worked::Again(reason)
            }
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                let prose = format!("The pipeline request could not be read: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.may_change("Pipeline", Verb::Propose) {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        // The source is read like every other endpoint of the conversation: opened first when
        // the person may read it, so the test runs on a real page (AG-76).
        if let Some(named) = params
            .source_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if let Err(reason) = self.open_endpoint(chosen, tools, named).await {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The pipeline could not be drafted: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        }

        let endpoints = self.project_endpoints();
        let listed = |kind: &str| {
            self.state
                .mirror
                .list(&self.project, kind, &crate::store::ListOptions::default())
                .items
        };
        let spaces: Vec<String> = listed("ContextSpace")
            .into_iter()
            .map(|env| env.metadata.name)
            .collect();
        let policies: Vec<Value> = listed("Policy")
            .iter()
            .filter_map(|env| serde_json::to_value(env).ok())
            .collect();
        let run_endpoint = chosen.first().map(|e| e.name.clone());
        let org_domain = crate::api::assistant::org_domain(&self.state, &self.project);
        let slug = share::slug();
        let world = kpi_pipeline::World {
            project: &self.project,
            org_domain: &org_domain,
            endpoints: &endpoints,
            spaces: &spaces,
            policies: &policies,
            run_endpoint: run_endpoint.as_deref(),
            new_slug: &slug,
        };
        let plan = match kpi_pipeline::plan(&params, &world) {
            Ok(plan) => plan,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The pipeline could not be drafted: {reason}");
                if last {
                    self.thought(&prose).await?;
                }
                return Ok(again(format!("error: {reason}"), prose));
            }
        };
        let name = params.name.trim().to_owned();

        for drafted in &plan.space_drafts {
            if let Err(err) = self
                .state
                .drafts
                .put(
                    &self.project,
                    &drafted.kind,
                    &drafted.name,
                    drafted.manifest.clone(),
                    None,
                    &self.created_by,
                    "assistant",
                )
                .await
            {
                return self
                    .draft_not_kept(TOOL, started, &input, &drafted.kind, &drafted.name, &err)
                    .await;
            }
        }
        if let Err(err) = self
            .state
            .drafts
            .put(
                &self.project,
                "Pipeline",
                &name,
                plan.pipeline.clone(),
                None,
                &self.created_by,
                "assistant",
            )
            .await
        {
            return self
                .draft_not_kept(TOOL, started, &input, "Pipeline", &name, &err)
                .await;
        }

        // The test runs on a page of the source when this run reads that endpoint, and on an
        // empty page otherwise, so the mapping and the indicator's admission are checked either way.
        let source_index = chosen.iter().position(|e| e.name == plan.source_endpoint);
        let sampled = source_index.is_some();
        let page = if let Some(index) = source_index {
            let mut url = format!(
                "{}/ngsi-ld/v1/entities?type={}&limit=100",
                endpoints::data_base(&self.proxy_base, chosen, index),
                urlencoding(params.entity_type.trim())
            );
            if !params.attribute.trim().is_empty() {
                url.push_str(&format!("&attrs={}", urlencoding(params.attribute.trim())));
            }
            if let Some(q) = params.q.as_deref().filter(|q| !q.trim().is_empty()) {
                url.push_str(&format!("&q={}", urlencoding(q)));
            }
            match self.read_text(&url).await {
                Ok(page) => page,
                // A failed read is no empty page: the test would pass on nothing (JC1-29).
                Err(reason) => {
                    let reason = format!("the source page could not be read: {reason}");
                    self.event("tool", failed(&input, &reason)).await?;
                    return self
                        .again(
                            last,
                            format!("error: {reason}"),
                            format!("The pipeline is drafted but not tested: {reason}"),
                        )
                        .await;
                }
            }
        } else {
            "[]".to_owned()
        };
        let verdict = match crate::ops::find("jc_pipeline_test") {
            Some(op) => {
                let caller = crate::ops::Caller {
                    bearer: None,
                    identity: self.identity.clone(),
                    via: crate::ops::Via::Agent,
                    access: None,
                };
                let test = json!({
                    "pipeline": plan.pipeline,
                    "sample": { "text": page, "format": "text" },
                    "draft": { "kind": "Pipeline", "name": name },
                });
                match crate::ops::call(op, &caller, &self.state, &self.project, test).await {
                    Ok(out) => out.get("verdict").cloned().unwrap_or(Value::Null),
                    Err(err) => json!({ "ok": false, "untested": err.to_string() }),
                }
            }
            None => json!({ "ok": false, "untested": "jc_pipeline_test is not registered" }),
        };

        let (verdict, awaits) = without_awaited(verdict, &plan);
        // A red test the model can act on goes back to it; a test that could not run at all (no
        // runner) is shown on the card, as before.
        let red = verdict.get("ok").and_then(Value::as_bool) == Some(false)
            && verdict.get("untested").is_none()
            && !awaits;
        if red {
            let findings = findings_of(&verdict);
            let reason = format!(
                "the pipeline's test on {} is not green: {findings}. The page it ran on ({} characters): {}",
                if sampled { "a page of the source" } else { "an empty page" },
                page.len(),
                page.chars().take(2_000).collect::<String>()
            );
            self.event(
                "tool",
                json!({
                    "tool": TOOL,
                    "status": "failed",
                    "durationMs": millis(started),
                    "input": input,
                    "error": format!("the test is not green: {findings}"),
                    "output": { "verdict": verdict, "pipeline": plan.pipeline },
                }),
            )
            .await?;
            if !last {
                return Ok(Worked::Again(reason));
            }
            let prose = format!(
                "The pipeline '{name}' is drafted but its test is still not green: {findings}. The draft is kept on the Pipelines page."
            );
            self.thought(&prose).await?;
            return Ok(Worked::Done(prose));
        }

        let drafts: Vec<Value> = plan
            .space_drafts
            .iter()
            .map(|d| json!({ "kind": d.kind, "name": d.name, "plural": d.plural, "manifest": d.manifest }))
            .collect();
        // The runner's token names the slugs it may write through; a new endpoint's slug is not
        // among them until the deployment's runner client says so (Architecture/08, KPI pipelines).
        let audience = plan
            .space_drafts
            .iter()
            .any(|d| d.kind == "Endpoint")
            .then(|| plan.target_slug.clone())
            .flatten();
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": millis(started),
                "input": input,
                "output": {
                    "name": name,
                    "title": params.title,
                    "formula": plan.formula,
                    "trigger": plan.trigger.describe(),
                    "sourceEndpoint": plan.source_endpoint,
                    "sourceSpace": plan.source_space,
                    "targetSpace": plan.target_space,
                    "targetEndpoint": plan.target_endpoint,
                    "targetSlug": plan.target_slug,
                    "sampled": sampled,
                    "verdict": verdict,
                    "pipeline": plan.pipeline,
                    "drafts": drafts,
                    "runnerAudience": audience,
                },
            }),
        )
        .await?;

        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!(
                "Drafted the pipeline '{name}': {} into {}, {}; review it in the form and propose it.",
                plan.formula,
                plan.target_space,
                plan.trigger.describe()
            );
        }
        if !plan.space_drafts.is_empty() {
            prose.push_str(&format!(
                " The indicator space {} does not exist yet: its drafts are on the card; propose them with the pipeline.",
                plan.target_space
            ));
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({
                "route": format!("/projects/{}/pipelines", self.project),
                "prefill": plan.prefill,
                "draft": { "kind": "Pipeline", "name": name },
            }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    pub(super) async fn share(
        &self,
        call: Result<share::ProposeEndpoint, String>,
        answer: &str,
    ) -> Result<String, String> {
        let started = std::time::Instant::now();
        let millis = |started: std::time::Instant| {
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The share request could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.granted("jc_endpoint_propose") {
            return self
                .refused("propose_endpoint", started, input, reason)
                .await;
        }
        let known = crate::api::assistant::declared_groups(&self.state, &self.project);
        let classes = crate::ops::endpoint_reach::space_classes(
            &self.state,
            &self.project,
            params.context_space.trim(),
        );
        match share::check_types(&params, &classes)
            .and_then(|()| share::render(&self.project, &params, &known))
        {
            Ok(proposal) => {
                let output = serde_json::to_value(&proposal).unwrap_or(Value::Null);
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "ok",
                        "durationMs": millis(started),
                        "input": input,
                        "output": output,
                    }),
                )
                .await?;
                let manifest = serde_json::to_value(&proposal.endpoint).unwrap_or_else(|_| {
                    serde_json::to_value(&proposal.prefill).unwrap_or(Value::Null)
                });
                // The form opens on this draft: one that was not kept would open an empty form
                // while the chat says it is filled (T-2763), so the person reads why instead.
                if let Err(err) = self
                    .state
                    .drafts
                    .put(
                        &self.project,
                        "Endpoint",
                        &params.name,
                        manifest,
                        None,
                        &self.created_by,
                        "assistant",
                    )
                    .await
                {
                    let why = match err {
                        crate::ops::drafts::DraftError::Db(detail) => {
                            tracing::warn!(project = %self.project, %detail, "endpoint draft not kept");
                            "the drafts cannot be written now".to_owned()
                        }
                        other => other.to_string(),
                    };
                    let prose = format!(
                        "The endpoint '{}' was rendered but not kept as a draft, so no form opens: {why}",
                        params.name
                    );
                    self.thought(&prose).await?;
                    return Ok(prose);
                }
                let mut prose = share::prose_of(answer);
                if prose.is_empty() {
                    prose = format!(
                        "Drafted the endpoint '{}'; review it in the form and propose it.",
                        params.name
                    );
                }
                self.thought(&prose).await?;
                self.event(
                    "navigate",
                    json!({
                        "route": format!("/projects/{}/endpoints", self.project),
                        "prefill": proposal.prefill,
                        "draft": {
                            "kind": "Endpoint",
                            "name": params.name,
                        },
                    }),
                )
                .await?;
                Ok(prose)
            }
            Err(reason) => {
                self.event(
                    "tool",
                    json!({
                        "tool": "propose_endpoint",
                        "status": "failed",
                        "durationMs": millis(started),
                        "input": input,
                        "error": reason,
                    }),
                )
                .await?;
                let prose = format!("The endpoint could not be drafted: {reason}");
                self.thought(&prose).await?;
                Ok(prose)
            }
        }
    }
}

/// The verdict without what it says of the indicator space's Endpoint drafted beside the
/// pipeline, which exists once both are approved, and whether that was all it found: every test
/// said so of a new space (T-6532, live walk T-6490 S-36). What else it finds stays red.
fn without_awaited(mut verdict: Value, plan: &kpi_pipeline::Plan) -> (Value, bool) {
    let awaited = plan
        .space_drafts
        .iter()
        .any(|d| d.kind == "Endpoint" && d.name == plan.target_endpoint);
    let Some(found) = verdict
        .get_mut("findings")
        .and_then(Value::as_array_mut)
        .filter(|_| awaited)
    else {
        return (verdict, false);
    };
    let before = found.len();
    found.retain(|f| {
        !f.get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| crate::record_check::awaits_endpoint(m, &plan.target_endpoint))
    });
    let awaits = found.len() < before && found.is_empty();
    if awaits {
        verdict["awaits"] = json!(format!(
            "Endpoint {} is drafted with the pipeline: its write is checked once both are approved",
            plan.target_endpoint
        ));
    }
    (verdict, awaits)
}

/// Drops `trace` from every draft's verdict of a `space_complete` output (T-0891).
fn without_traces(output: &mut Value) {
    if let Some(drafts) = output.get_mut("drafts").and_then(Value::as_array_mut) {
        for draft in drafts {
            if let Some(verdict) = draft.get_mut("verdict").and_then(Value::as_object_mut) {
                verdict.remove("trace");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::without_traces;
    use serde_json::json;

    #[test]
    fn a_hand_off_keeps_each_verdict_but_not_its_trace() {
        let mut output = json!({
            "space": "city-bikes",
            "drafts": [
                { "kind": "DataSource", "verdict": { "ok": true, "findings": [], "trace": { "records": [1, 2, 3] } } },
                { "kind": "Endpoint", "verdict": null },
                { "kind": "Pipeline" }
            ]
        });
        without_traces(&mut output);
        assert_eq!(
            output["drafts"][0]["verdict"],
            json!({ "ok": true, "findings": [] })
        );
        assert_eq!(output["drafts"][1]["verdict"], json!(null));
        assert!(output["drafts"][2].get("verdict").is_none());
        let mut bare = json!({ "space": "x" });
        without_traces(&mut bare);
        assert_eq!(bare, json!({ "space": "x" }));
    }

    /// A steward's run in `helsinki` whose source Endpoint answers `answer`, with an indicator
    /// space already there, so the pipeline plan holds and the test reads the source page.
    async fn kpi_run(answer: u16) -> (super::Driver, crate::state::AppState, wiremock::MockServer) {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let source = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(answer).set_body_string("[]"))
            .mount(&source)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let put = |kind: &str, name: &str, spec: serde_json::Value| {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta {
                    name: name.to_owned(),
                    namespace: Some("helsinki".to_owned()),
                    ..Default::default()
                },
                spec,
                status: None,
            })
        };
        put("ContextSpace", "helsinki", json!({}));
        put("ContextSpace", "helsinki-kpi", json!({}));
        put(
            "Endpoint",
            "helsinki-all",
            json!({ "contextSpaceRef": "helsinki", "slug": "allslug" }),
        );
        put(
            "Endpoint",
            "helsinki-kpi",
            json!({ "contextSpaceRef": "helsinki-kpi", "slug": "kpislug" }),
        );
        put(
            "Policy",
            "kpi-pipelines-write",
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
                "assignee": { "kind": "serviceAccount", "id": "pipelines" },
                "operations": ["upsertBatch"]
            }),
        );
        let mut driver = super::Driver::for_tests(state.clone(), "helsinki");
        driver.proxy_base = source.uri();
        driver.identity.roles = vec![state.config.bootstrap_admins.clone()];
        let operations: Vec<&str> = crate::ops::registry().iter().map(|op| op.name).collect();
        driver.access = crate::agents::access::Access::from_spec(&json!({ "access": {
            "operations": operations,
            "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }],
        } }));
        (driver, state, source)
    }

    async fn tool_events(state: &crate::state::AppState) -> Vec<serde_json::Value> {
        state
            .agents
            .events_since("test-run", 0)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|event| event.kind == "tool")
            .map(|event| event.payload)
            .collect()
    }

    fn bikes() -> crate::agents::kpi_pipeline::DraftKpiPipeline {
        serde_json::from_value(json!({
            "name": "bikes-available-avg", "type": "BikeHireDockingStation",
            "attribute": "availableBikeNumber", "agg": "avg", "every": "15m"
        }))
        .expect("a call")
    }

    /// T-6532 (live walk T-6490, S-36): an indicator space drafted with the pipeline has no
    /// Endpoint yet, and the test saying so kept every such draft red. Those findings go, for that
    /// Endpoint only, and the verdict says its write is checked once both are approved; any other
    /// finding keeps the draft red.
    #[test]
    fn an_endpoint_drafted_with_the_pipeline_does_not_turn_its_test_red() {
        let endpoints = [json!({
            "metadata": { "name": "helsinki-all" },
            "spec": { "contextSpaceRef": "helsinki", "slug": "allslug" }
        })];
        let spaces = ["helsinki".to_owned()];
        let world = crate::agents::kpi_pipeline::World {
            project: "helsinki",
            org_domain: "hel.fi",
            endpoints: &endpoints,
            spaces: &spaces,
            policies: &[],
            run_endpoint: Some("helsinki-all"),
            new_slug: "newslug",
        };
        let mut call = bikes();
        call.target_space = Some("helsinki-bikes-kpi".into());
        let plan = crate::agents::kpi_pipeline::plan(&call, &world).expect("a plan");
        assert_eq!(plan.target_endpoint, "helsinki-bikes-kpi");
        let not_there = |endpoint: &str| {
            vec![
                json!({ "path": "records[0]", "message": format!("endpoint: Endpoint {endpoint} is not in this project") }),
                json!({ "path": "records[0]", "message": format!("endpoint: Endpoint {endpoint} has no view on the broker yet: the reconciler has not read it; test again once it has") }),
            ]
        };
        let (verdict, awaits) = super::without_awaited(
            json!({ "ok": false, "findings": not_there("helsinki-bikes-kpi") }),
            &plan,
        );
        assert!(awaits, "{verdict}");
        assert_eq!(verdict["findings"], json!([]));
        assert!(
            verdict["awaits"]
                .as_str()
                .is_some_and(|a| a.contains("helsinki-bikes-kpi")),
            "{verdict}"
        );
        let mut more = not_there("helsinki-bikes-kpi");
        more.push(json!({ "path": "records[0].kpiValue", "message": "kpiValue: not a number" }));
        let (verdict, awaits) =
            super::without_awaited(json!({ "ok": false, "findings": more }), &plan);
        assert!(!awaits);
        assert_eq!(verdict["findings"].as_array().map(Vec::len), Some(1));
        let (verdict, awaits) = super::without_awaited(
            json!({ "ok": false, "findings": not_there("someone-else") }),
            &plan,
        );
        assert!(!awaits && verdict.get("awaits").is_none(), "{verdict}");
    }

    /// JC1-29: a source page that cannot be read fails the step; the pipeline is never tested
    /// on an empty page standing in for it.
    #[tokio::test]
    async fn a_source_page_that_cannot_be_read_fails_the_step_and_tests_nothing() {
        let (driver, state, _source) = kpi_run(503).await;
        let mut chosen = vec![crate::agents::endpoints::RunEndpoint {
            name: "helsinki-all".into(),
            slug: "allslug".into(),
            space: "helsinki".into(),
        }];
        let worked = driver
            .kpi_pipeline(Ok(bikes()), "", &mut chosen, &mut Vec::new(), true)
            .await
            .expect("the run goes on");
        let super::Worked::Done(prose) = worked else {
            panic!("the last draft ends the tool");
        };
        assert!(
            prose.contains("not tested") && prose.contains("503"),
            "{prose}"
        );
        let events = tool_events(&state).await;
        let last = events.last().expect("a tool event");
        assert_eq!(last["status"], "failed", "{last}");
        assert!(
            last.get("output").is_none(),
            "no verdict on a page never read: {last}"
        );
    }

    /// JC1-30: a draft the store cannot keep ends the tool with what to do, and a database
    /// error's own text never reaches the person.
    #[tokio::test]
    async fn a_draft_the_store_cannot_keep_ends_the_tool_without_its_database_text() {
        use crate::ops::drafts::DraftError;
        let (driver, state, _source) = kpi_run(200).await;
        let worked = driver
            .draft_not_kept(
                "draft_kpi_pipeline",
                std::time::Instant::now(),
                &json!({}),
                "Pipeline",
                "bikes",
                &DraftError::Db("connection to 10.0.0.7:5432 refused".into()),
            )
            .await
            .expect("the run goes on");
        let super::Worked::Done(prose) = worked else {
            panic!("a lost draft ends the tool");
        };
        assert!(
            prose.contains("could not be saved") && !prose.contains("10.0.0.7"),
            "{prose}"
        );
        let events = tool_events(&state).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["status"], "failed");
        assert!(!events[0].to_string().contains("10.0.0.7"), "{}", events[0]);
        let secret = driver
            .draft_not_kept(
                "t",
                std::time::Instant::now(),
                &json!({}),
                "DataSource",
                "feed",
                &DraftError::Secret("spec.auth.password".into()),
            )
            .await
            .expect("the run goes on");
        let super::Worked::Done(prose) = secret else {
            panic!("a refused draft ends the tool");
        };
        assert!(
            prose.contains("spec.auth.password") && prose.contains("secretRef"),
            "{prose}"
        );
    }
}
