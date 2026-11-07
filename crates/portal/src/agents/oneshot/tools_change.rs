//! The change tools of the conversation: resources, dashboards, models, grants, the catalog search (AG-77, PF-52).

use super::*;

/// jc-core's words for a manifest its kind cannot read: an unknown field, a wrong type.
const NOT_PARSED: &str = "manifest does not parse";

/// The operation that does what a model tried with a field the kind does not have: a SyncSource
/// pauses through its loop's switch, never its manifest (T-6533, S-25).
fn not_a_field(kind: &str, findings: &str) -> Option<&'static str> {
    let switch = ["paused", "suspended", "enabled", "disabled", "active"]
        .iter()
        .any(|field| findings.contains(&format!("unknown field `{field}`")));
    (kind == "SyncSource" && findings.contains(NOT_PARSED) && switch).then_some(
        "a SyncSource has no such field: pause it, or resume it, with jc_syncsource_pause \
         {\"name\": \"<the SyncSource>\", \"paused\": true or false}, which changes no manifest",
    )
}

/// What the kind's page opens on: the resource a change touches (`?edit=`), or the draft of a new
/// one, which the form loads by its name (`?draft=`, AG-45).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Opening {
    Change,
    Create,
}

impl Driver {
    /// Whether this run may open a change of `kind` for the person (AG-70): the profile names
    /// `jc_resource_propose` with `propose` on the kind, or shares endpoints and the kind is
    /// Endpoint; and the person holds the verb, `delete` for a removal.
    pub(super) fn may_change(&self, kind: &str, verb: Verb) -> Result<(), String> {
        let shares = kind == "Endpoint" && self.granted("jc_endpoint_propose").is_ok();
        if !shares && !self.access.proposes("jc_resource_propose", kind) {
            return Err(format!(
                "the agent profile does not grant propose on {kind} through jc_resource_propose (AG-70)"
            ));
        }
        crate::permissions::for_request(&self.state, &self.identity, &self.home(kind))
            .check(kind, verb, None)
            .map_err(|err| err.to_string())
    }

    /// Where a kind's resources live: the organization's namespace for a RoleBinding, this
    /// project for everything else, and this project for a `Role` the run writes, which is a
    /// role of the project it works in (PF-68).
    pub(super) fn home(&self, kind: &str) -> String {
        self.asker().home(kind)
    }

    /// The project's resources this run may change, by kind, so the model names one that exists.
    pub(super) fn changeable(&self) -> BTreeMap<&'static str, Vec<String>> {
        crate::resource::kinds()
            .filter(|info| info.scope.allows_project() || info.scope.allows_organization())
            .filter(|info| self.may_change(info.kind, Verb::Propose).is_ok())
            .filter_map(|info| {
                let names = self.names_of(info.kind);
                (!names.is_empty()).then_some((info.kind, names))
            })
            .collect()
    }

    /// The names of this kind the run may name, from every place the kind lives: for a `Role`
    /// that is the project's own roles and the organization's, because both are in force here
    /// (PF-68, T-0872).
    pub(super) fn names_of(&self, kind: &str) -> Vec<String> {
        self.asker().names_of(kind)
    }

    /// The copy the person is working in: their newest live workspace of this project (CC-76).
    /// Its drafts and pages are the copy's; without one, drafts are the project's own.
    pub(super) async fn active_workspace(&self) -> Option<String> {
        let now = chrono::Utc::now();
        self.state
            .workspaces
            .list(&self.project)
            .await
            .ok()?
            .into_iter()
            .filter(|w| !w.expired(now) && crate::ops::workspaces::owns(w, &self.identity))
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .map(|w| w.name)
    }

    /// Why a second resource may not be changed outside a copy (AG-82): the assistant already
    /// holds an open draft of another resource of this person in the project. A change of more
    /// than one resource is made in a workspace and brought back as one Change.
    ///
    /// Only drafts this run wrote count (JC1-13): an assistant draft the person left in an
    /// earlier conversation is theirs to propose or drop, and no reason to refuse this one.
    /// ponytail: a continued conversation starts its count again; carry the first run's start
    /// when continuations should share it.
    pub(super) async fn second_resource(&self, kind: &str, name: &str) -> Option<String> {
        let drafts = self.state.drafts.list(&self.project).await.ok()?;
        let since = chrono::Utc::now() - self.started.elapsed();
        let other = drafts.into_iter().find(|draft| {
            draft.workspace.is_none()
                && draft.touched_kind == "assistant"
                && draft.touched_by == self.created_by
                && draft.updated_at >= since
                && !(draft.kind == kind && draft.name == name)
        })?;
        Some(format!(
            "this conversation already changes {} '{}'; a change of more than one resource is \
             made in a copy of the project: call jc_workspace_open first, then change each \
             resource, then jc_workspace_compare and show the person what changes (AG-82)",
            other.kind, other.name
        ))
    }

    /// A change or a removal of a resource the conversation names (AG-77, AG-73). What the model
    /// got wrong goes back to it while drafts are left: an unknown kind or name with the real
    /// ones, a patch the check refuses with the reason and the manifest as it is. Nothing is
    /// proposed; the kind's page opens on the change or the removal for the person.
    pub(super) async fn change_resource(
        &self,
        call: Result<change::ChangeResource, String>,
        answer: &str,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let started = std::time::Instant::now();
        let failed = |input: &Value, reason: &str| failed_step(TOOL, started, input, reason);
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The change could not be read: {reason}"),
                    )
                    .await;
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        let changeable = self.changeable();
        let Some(info) = crate::resource::by_kind(params.kind.trim())
            .filter(|info| info.scope.allows_project() || info.scope.allows_organization())
        else {
            let kinds: Vec<&str> = changeable.keys().copied().collect();
            let reason = format!(
                "'{}' is not a kind of this project; the kinds you may change are {}",
                params.kind,
                kinds.join(", ")
            );
            self.event("tool", failed(&input, &reason)).await?;
            return self
                .again(
                    last,
                    format!("error: {reason}"),
                    format!("That could not be changed: {reason}"),
                )
                .await;
        };
        let verb = if params.delete {
            Verb::Delete
        } else {
            Verb::Propose
        };
        if let Err(reason) = self.may_change(info.kind, verb) {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        if params.create {
            // A dashboard is created with the layers its pages draw; the other kinds are one
            // manifest the person finishes in the kind's own form (AG-45).
            return if info.kind == "Dashboard" {
                self.create_dashboard(info.kind, &params, answer, input, started, last)
                    .await
            } else {
                self.create_resource(info, &params, answer, input, started, last)
                    .await
            };
        }
        let name = params.name.trim();
        let Some(current) = self
            .state
            .mirror
            .get(&self.home(info.kind), info.kind, name)
        else {
            let names = self.names_of(info.kind);
            let reason = if names.is_empty() {
                format!(
                    "{} '{name}' does not exist; the project has no {}",
                    info.kind, info.kind
                )
            } else {
                format!(
                    "{} '{name}' does not exist; the project's are {}",
                    info.kind,
                    names.join(", ")
                )
            };
            self.event("tool", failed(&input, &reason)).await?;
            return self
                .again(
                    last,
                    format!("error: {reason}"),
                    format!("That could not be changed: {reason}"),
                )
                .await;
        };
        let workspace = self.active_workspace().await;
        if workspace.is_none() {
            if let Some(reason) = self.second_resource(info.kind, name).await {
                self.event("tool", failed(&input, &reason)).await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("That needs a copy of the project first: {reason}"),
                    )
                    .await;
            }
        }
        let route = within(
            change::route(&self.project, info.kind, info.plural, name, params.delete),
            workspace.as_deref(),
        );
        // A model's classes and attributes live in its LinkML source, not in the manifest.
        if info.kind == "DataModel" && !params.delete {
            let operations = params.operations.as_deref().unwrap_or_default();
            return self
                .change_model(name, operations, answer, input, started, last, route)
                .await;
        }
        let draft = json!({ "kind": info.kind, "name": name });

        if params.delete {
            self.event(
                "tool",
                json!({
                    "tool": TOOL,
                    "status": "ok",
                    "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "input": input,
                    "output": { "kind": info.kind, "name": name, "delete": true },
                }),
            )
            .await?;
            let mut prose = share::prose_of(answer);
            if prose.is_empty() {
                prose =
                    format!("Opened the removal of '{name}'; type its name there to propose it.");
            }
            self.thought(&prose).await?;
            self.event("navigate", json!({ "route": route })).await?;
            return Ok(Worked::Done(prose));
        }

        let mut current = serde_json::to_value(&current).map_err(|err| err.to_string())?;
        if let Value::Object(fields) = &mut current {
            fields.remove("status");
        }
        let Some(patch) = params.patch.as_ref() else {
            // A call without a patch reads the manifest first, so the next one names real fields.
            return self
                .again(
                    last,
                    format!("the manifest of {} '{name}' as it is: {current}; answer with change_resource and a patch of the fields that change", info.kind),
                    format!("Which field of '{name}' should change, and to what value?"),
                )
                .await;
        };
        let manifest = match change::patched(&current, patch) {
            Ok(manifest) => manifest,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}; the manifest as it is: {current}"),
                        format!("The change to '{name}' could not be made: {reason}"),
                    )
                    .await;
            }
        };
        // A pipeline change the test does not exercise opens untested: a pause is not a mapping.
        let untested = info.kind == "Pipeline" && !change::reaches_the_test(&current, &manifest);
        self.open_change(
            info, name, manifest, answer, TOOL, input, started, last, route, draft, untested,
        )
        .await
    }

    /// `change_resource` for the person's message `asked`: a Role that grants writes the person
    /// said they do not want goes back to the model with each write it grants, and on the last
    /// draft the person reads why nothing was drafted (T-6534, walk S-28).
    pub(super) async fn change_asked(
        &self,
        call: Result<change::ChangeResource, String>,
        asked: &str,
        answer: &str,
        last: bool,
    ) -> Result<Worked, String> {
        let Some(reason) = call
            .as_ref()
            .ok()
            .and_then(|params| change::wider_than_asked(params, asked))
        else {
            return self.change_resource(call, answer, last).await;
        };
        let input = call
            .ok()
            .and_then(|params| serde_json::to_value(params).ok())
            .unwrap_or(Value::Null);
        self.event(
            "tool",
            failed_step(
                "change_resource",
                std::time::Instant::now(),
                &input,
                &reason,
            ),
        )
        .await?;
        self.again(
            last,
            format!("error: {reason}"),
            "Nothing was drafted: you asked for no writes, and what the role should do needs a \
             write verb (running a pipeline needs propose and approve on Pipeline). Say whether \
             the role may hold it, or keep the role to reading."
                .to_owned(),
        )
        .await
    }

    /// A new resource of a kind whose form the person finishes (AG-45, UI-45): the manifest the
    /// sentence described, put through the dry run every channel uses and through the same test a
    /// change of that kind runs (a data source fetches its URL, a pipeline runs on a page of its
    /// source), kept as the person's draft, and the kind's page opens on that draft with the form
    /// filled. Nothing is proposed: the person reads the form and proposes it (PF-58). A name that
    /// is taken, a missing spec and a refused check go back to the model with the reason.
    pub(super) async fn create_resource(
        &self,
        info: &crate::resource::KindInfo,
        params: &change::ChangeResource,
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        // A model is drawn from the editor's operations, never from a manifest patch (DM-13).
        if info.kind == "DataModel" {
            return self
                .create_model(params, answer, input, started, last)
                .await;
        }
        let name = params.name.trim();
        let home = self.home(info.kind);
        let drafted = if self.state.mirror.get(&home, info.kind, name).is_some() {
            Err(format!(
                "{} '{name}' already exists; change it with a patch instead of creating it",
                info.kind
            ))
        } else {
            change::new_manifest(info.kind, &home, name, params.patch.as_ref())
        };
        let manifest = match drafted {
            Ok(manifest) => manifest,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The new {} could not be drafted: {reason}", info.kind),
                    )
                    .await;
            }
        };
        let route = within(
            change::create_route(&self.project, info.kind, info.plural),
            self.active_workspace().await.as_deref(),
        );
        let draft = json!({ "kind": info.kind, "name": name });
        self.open_change_as(
            info,
            name,
            manifest,
            answer,
            TOOL,
            input,
            started,
            last,
            route,
            draft,
            false,
            Opening::Create,
        )
        .await
    }

    /// Every manifest `new_dashboard` drafted through the platform's dry run, in order; the first
    /// refusal comes back with the manifest it refused.
    ///
    /// The dashboard is dry-run against a copy of the mirror that already holds the layers
    /// drafted beside it, since they land in its one proposal (T-2553): every check, UI-19's
    /// included, sees the dashboard as it will be, so a new layer on a non-public endpoint is
    /// refused exactly like an existing one. The mirror itself is not touched.
    pub(super) async fn dry_run_drafted(&self, drafted: &[Value]) -> Result<(), (String, Value)> {
        let refuse = |manifest: &Value, findings: String| {
            (
                format!(
                    "the platform's check refuses {} '{}': {findings}",
                    manifest["kind"].as_str().unwrap_or_default(),
                    manifest["metadata"]["name"].as_str().unwrap_or_default()
                ),
                manifest.clone(),
            )
        };
        let mut beside = self.state.clone();
        let view = self.state.mirror.snapshot();
        for layer in drafted
            .iter()
            .filter(|manifest| manifest["kind"] == "Layer")
        {
            let envelope =
                serde_json::from_value::<crate::resource::ResourceEnvelope>(layer.clone())
                    .map_err(|err| refuse(layer, err.to_string()))?;
            view.upsert(envelope);
        }
        beside.mirror = std::sync::Arc::new(view);
        for manifest in drafted {
            let state = if manifest["kind"] == "Dashboard" {
                &beside
            } else {
                &self.state
            };
            match crate::api::dry_run::execute_dry_run(
                &self.identity,
                state,
                &self.project,
                manifest.clone(),
            )
            .await
            {
                Ok(result) if result.valid => {}
                Ok(result) => {
                    return Err(refuse(
                        manifest,
                        serde_json::to_string(&result.plan).unwrap_or_default(),
                    ))
                }
                Err(err) => return Err(refuse(manifest, err.to_string())),
            }
        }
        Ok(())
    }

    /// A new dashboard with the new layers its pages draw (AG-77, UI-17, UI-18). Every manifest
    /// passes the dry run and is kept as the person's draft, and the dashboard editor opens on
    /// the dashboard; its one proposal carries the layers with it. What the model got wrong goes
    /// back to it with the project's real names and the endpoint's attributes.
    pub(super) async fn create_dashboard(
        &self,
        kind: &str,
        params: &change::ChangeResource,
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let name = params.name.trim();
        if kind == "Dashboard" {
            if let Err(reason) = self.may_change("Layer", Verb::Propose) {
                return self
                    .refused(TOOL, started, input, reason)
                    .await
                    .map(Worked::Done);
            }
        }
        let mut drafted = match self.new_dashboard(kind, name, params).await {
            Ok(drafted) => drafted,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The dashboard could not be drafted: {reason}"),
                    )
                    .await;
            }
        };
        if let Err((reason, manifest)) = self.dry_run_drafted(&drafted).await {
            self.event("tool", failed_step(TOOL, started, &input, &reason))
                .await?;
            return self
                .again(
                    last,
                    format!("error: {reason}; the manifest was {manifest}"),
                    format!("The dashboard does not pass the platform's check: {reason}"),
                )
                .await;
        }
        for manifest in &drafted {
            let kind = manifest["kind"].as_str().unwrap_or_default();
            let named = manifest["metadata"]["name"].as_str().unwrap_or_default();
            let kept = self
                .state
                .drafts
                .put(
                    &self.project,
                    kind,
                    named,
                    manifest.clone(),
                    None,
                    &self.created_by,
                    "assistant",
                )
                .await;
            if let Err(err) = kept {
                return self
                    .draft_not_kept(TOOL, started, &input, kind, named, &err)
                    .await;
            }
            let verdict = crate::ops::verdict::Verdict::green(manifest, None);
            if let Err(err) = self
                .state
                .drafts
                .set_verdict(&self.project, kind, named, verdict)
                .await
            {
                tracing::warn!(run = %self.run_id, kind, name = named, error = %err, "dashboard verdict not kept");
            }
        }
        // The dashboard is drafted last, so it is the one the editor opens on.
        let dashboard = drafted.pop().unwrap_or(Value::Null);
        let layers: Vec<&str> = drafted
            .iter()
            .filter_map(|layer| layer["metadata"]["name"].as_str())
            .collect();
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": millis(),
                "input": input,
                "output": { "kind": "Dashboard", "name": name, "checked": true, "create": true, "layers": layers },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!("Drafted the dashboard '{name}'; review it and propose it.");
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({
                "route": format!("/projects/{}/dashboards?edit={name}", self.project),
                "prefill": dashboard,
                "draft": { "kind": "Dashboard", "name": name },
            }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// The new layers and then the dashboard, as manifests, or what is wrong with the call in
    /// words the model can act on.
    pub(super) async fn new_dashboard(
        &self,
        kind: &str,
        name: &str,
        params: &change::ChangeResource,
    ) -> Result<Vec<Value>, String> {
        if kind != "Dashboard" {
            return Err(format!(
                "only a Dashboard is created here; a new {kind} is drafted on its own page"
            ));
        }
        if self
            .state
            .mirror
            .get(&self.project, "Dashboard", name)
            .is_some()
        {
            return Err(format!(
                "Dashboard '{name}' already exists; change it with a patch instead"
            ));
        }
        let spec = params.patch.as_ref().and_then(change::spec_of).ok_or(
            "a new dashboard carries its spec as the patch: {\"spec\": {\"title\", \"visibility\", \"pages\"}}",
        )?;
        let public = spec["visibility"].as_str() == Some("public");
        let endpoints: BTreeMap<String, Value> = self
            .state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .into_iter()
            .map(|envelope| (envelope.metadata.name, envelope.spec))
            .collect();
        let existing = self.names_of("Layer");
        let mut manifests = Vec::new();
        for layer in &params.layers {
            let layer_name = layer.name.trim();
            if existing.iter().any(|known| known == layer_name) {
                return Err(format!(
                    "Layer '{layer_name}' already exists; draw it by its name and leave it out of layers"
                ));
            }
            let source = layer.spec["sourceEndpointRef"].as_str().unwrap_or_default();
            let Some(endpoint) = endpoints.get(source) else {
                return Err(format!(
                    "layer '{layer_name}' reads '{source}', which is not an endpoint of the project; its endpoints are {}",
                    endpoints.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            };
            if public && endpoint["audience"].as_str() != Some("public") {
                return Err(format!(
                    "a public dashboard reads only public endpoints, and '{source}' is not public (UI-19)"
                ));
            }
            let entity_type = layer.spec["entityType"].as_str().unwrap_or_default();
            let named = change::named_attributes(&layer.spec);
            let schema = if named.is_empty() {
                None
            } else {
                fields::of_endpoint(
                    &self.state,
                    &self.project,
                    endpoint,
                    &[entity_type.to_owned()],
                )
                .await
            };
            if let Some(schema) = schema {
                let Some(properties) = schema[entity_type]["properties"].as_object() else {
                    return Err(format!(
                        "the data model behind '{source}' has no type '{entity_type}'"
                    ));
                };
                let unknown: Vec<&str> = named
                    .iter()
                    .map(String::as_str)
                    .filter(|attribute| !properties.contains_key(*attribute))
                    .collect();
                if !unknown.is_empty() {
                    return Err(format!(
                        "layer '{layer_name}' names {}, which {entity_type} on '{source}' does not have; its attributes are {}",
                        unknown.join(", "),
                        properties.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            manifests.push(change::manifest(
                "Layer",
                &self.project,
                layer_name,
                layer.spec.clone(),
            ));
        }
        for drawn in crate::dashboards::layer_names(&spec) {
            let new = params.layers.iter().any(|layer| layer.name.trim() == drawn);
            if !new && !existing.contains(&drawn) {
                return Err(format!(
                    "a page draws '{drawn}', which is neither a layer of the project nor one of the new layers; the project's layers are {}",
                    if existing.is_empty() { "none".to_owned() } else { existing.join(", ") }
                ));
            }
        }
        manifests.push(change::manifest("Dashboard", &self.project, name, spec));
        Ok(manifests)
    }

    /// A data model changed by the editor's operations (AG-77, DM-13): applied to the LinkML
    /// source the repository holds and put through the source's dry run, which compiles it and
    /// classifies the change, a breaking one included; the Models page opens on the same
    /// operations and applies them to the text. What the model got wrong goes back to it with the
    /// model's classes and their slots.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn change_model(
        &self,
        name: &str,
        operations: &[model_change::Operation],
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
        route: String,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let home = &self.home("DataModel");
        let spec = self
            .state
            .mirror
            .get(home, "DataModel", name)
            .map(|envelope| envelope.spec)
            .unwrap_or(Value::Null);
        let model = match crate::api::datamodels::read_source(&self.state, home, name)
            .await
            .map_err(|err| err.to_string())
            .and_then(|source| {
                serde_norway::from_str::<Value>(&source).map_err(|err| err.to_string())
            }) {
            Ok(model) => model,
            Err(reason) => {
                let reason =
                    format!("the source of data model '{name}' could not be read: {reason}");
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The data model '{name}' could not be changed: {reason}"),
                    )
                    .await;
            }
        };
        let outline = model_change::outline(&model);
        if operations.is_empty() {
            return self
                .again(
                    last,
                    format!("the classes of data model '{name}' and their slots: {outline}; answer with change_resource and the operations that change it"),
                    format!("What should change in the data model '{name}'?"),
                )
                .await;
        }
        let checked = match model_change::apply(&model, operations) {
            Ok(changed) => match serde_norway::to_string(&changed) {
                Ok(source) => crate::api::datamodels::check_source(
                    &self.state,
                    &self.identity,
                    home,
                    name,
                    &spec,
                    &source,
                    None,
                )
                .await
                .map(|(checked, _, _)| checked)
                .map_err(|err| format!("the platform's check refuses the change: {err}")),
                Err(err) => Err(err.to_string()),
            },
            Err(reason) => Err(reason),
        };
        let checked = match checked {
            Ok(checked) => checked,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}; the classes of data model '{name}' and their slots: {outline}"),
                        format!("The data model '{name}' could not be changed: {reason}"),
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
                "output": {
                    "kind": "DataModel",
                    "name": name,
                    "checked": true,
                    "severity": checked.severity,
                    "changes": checked.changes,
                    "version": checked.version,
                },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose =
                format!("Opened the data model '{name}' with the change; review it and save it.");
        }
        self.thought(&prose).await?;
        self.event(
            "navigate",
            json!({ "route": route, "prefill": { "operations": operations } }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// A new data model (AG-45, DM-13): the editor's operations applied to the model the editor
    /// starts with, put through the source's check, which compiles it; the draft is kept in the
    /// shape the Models page keeps its own (`spec.linkml` holds the source) and the page opens on
    /// it, so the person reads and saves it in the editor. What does not check goes back.
    pub(super) async fn create_model(
        &self,
        params: &change::ChangeResource,
        answer: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "change_resource";
        let name = params.name.trim();
        let home = self.home("DataModel");
        let operations = params.operations.as_deref().unwrap_or_default();
        let drafted = self.drafted_model(&home, name, operations).await;
        let source = match drafted {
            Ok(source) => source,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The new data model could not be drafted: {reason}"),
                    )
                    .await;
            }
        };
        let workspace = self.active_workspace().await;
        let manifest = json!({
            "kind": "DataModel",
            "metadata": { "name": name },
            "spec": { "linkml": source },
        });
        if let Err(err) = self
            .state
            .drafts
            .put_in(
                workspace.as_deref(),
                &home,
                "DataModel",
                name,
                manifest,
                None,
                &self.created_by,
                "assistant",
            )
            .await
        {
            let reason = format!("the draft could not be kept: {err}");
            self.event("tool", failed_step(TOOL, started, &input, &reason))
                .await?;
            return Ok(Worked::Done(format!(
                "The new data model '{name}' could not be kept: {err}"
            )));
        }
        self.event(
            "tool",
            json!({
                "tool": TOOL,
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "output": { "kind": "DataModel", "name": name, "checked": true, "create": true },
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose =
                format!("Opened the new data model '{name}' in the editor; review it and save it.");
        }
        self.thought(&prose).await?;
        let route = within(
            change::create_route(&self.project, "DataModel", "datamodels"),
            workspace.as_deref(),
        );
        self.event(
            "navigate",
            json!({ "route": route, "draft": { "kind": "DataModel", "name": name } }),
        )
        .await?;
        Ok(Worked::Done(prose))
    }

    /// The LinkML source of a new model `name` built from `operations`, checked as a save checks
    /// it; `Err` says what the model must change.
    async fn drafted_model(
        &self,
        home: &str,
        name: &str,
        operations: &[model_change::Operation],
    ) -> Result<String, String> {
        if !crate::resource::is_dns1123(name) {
            return Err(format!(
                "'{name}' is no data model name: lower-case letters, digits and '-', such as 'bike-lanes'"
            ));
        }
        if self.state.mirror.get(home, "DataModel", name).is_some() {
            return Err(format!(
                "DataModel '{name}' already exists; change it with operations instead of creating it"
            ));
        }
        if operations.is_empty() {
            return Err(
                "a new data model carries its classes and attributes as operations: addClass \
                 (is_a Entity) per entity type, addSlot with its class and range per attribute"
                    .to_owned(),
            );
        }
        let domain = crate::api::assistant::org_domain(&self.state, &self.project);
        let model = model_change::apply(&model_change::blank(&domain, name), operations)?;
        let source = serde_norway::to_string(&model).map_err(|err| err.to_string())?;
        crate::api::datamodels::check_source(
            &self.state,
            &self.identity,
            home,
            name,
            &Value::Null,
            &source,
            None,
        )
        .await
        .map_err(|err| format!("the platform's check refuses the model: {err}"))?;
        Ok(source)
    }

    /// A role for people or a group (AG-77, PF-52): the binding named and checked like any
    /// proposal, which holds it to what the person holds on its scope, then the Access page's
    /// grant form opens on it. An unknown role goes back with the organization's roles.
    pub(super) async fn grant_role(
        &self,
        call: Result<grant::GrantRole, String>,
        answer: &str,
        last: bool,
    ) -> Result<Worked, String> {
        const TOOL: &str = "grant_role";
        let started = std::time::Instant::now();
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &Value::Null, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The grant could not be read: {reason}"),
                    )
                    .await;
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.may_change("RoleBinding", Verb::Propose) {
            return self
                .refused(TOOL, started, input, reason)
                .await
                .map(Worked::Done);
        }
        let roles = self.names_of("Role");
        let checked = if roles.iter().any(|role| role == params.role.trim()) {
            grant::manifest(
                &params,
                &crate::api::assistant::org_domain(&self.state, &self.project),
            )
        } else {
            Err(format!(
                "the organization has no role '{}'; its roles are {}",
                params.role,
                roles.join(", ")
            ))
        };
        let manifest = match checked {
            Ok(manifest) => manifest,
            Err(reason) => {
                self.event("tool", failed_step(TOOL, started, &input, &reason))
                    .await?;
                return self
                    .again(
                        last,
                        format!("error: {reason}"),
                        format!("The role could not be granted: {reason}"),
                    )
                    .await;
            }
        };
        let name = manifest["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let info = crate::resource::by_kind("RoleBinding")
            .ok_or_else(|| "RoleBinding is not a kind of this Portal".to_owned())?;
        // Project settings → Members holds the grant form since T-2606 (Architecture/09 §14.3);
        // the page keys the hand-off by the address it opens on, so it names that one.
        let route = format!("/projects/{}/settings/members?grant={name}", self.project);
        let draft = json!({ "kind": "RoleBinding", "name": name });
        self.open_change(
            info, &name, manifest, answer, TOOL, input, started, last, route, draft, false,
        )
        .await
    }

    /// A change of a resource that exists (AG-77): the kind's page opens on it with the manifest
    /// as the form's values.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn open_change(
        &self,
        info: &crate::resource::KindInfo,
        name: &str,
        manifest: Value,
        answer: &str,
        tool: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
        route: String,
        draft: Value,
        untested: bool,
    ) -> Result<Worked, String> {
        self.open_change_as(
            info,
            name,
            manifest,
            answer,
            tool,
            input,
            started,
            last,
            route,
            draft,
            untested,
            Opening::Change,
        )
        .await
    }

    /// The patched manifest checked by the dry run every channel uses (AG-77), kept as the
    /// person's draft and opened on the kind's page. A refused check goes back to the model.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn open_change_as(
        &self,
        info: &crate::resource::KindInfo,
        name: &str,
        manifest: Value,
        answer: &str,
        tool: &str,
        input: Value,
        started: std::time::Instant,
        last: bool,
        route: String,
        draft: Value,
        untested: bool,
        opening: Opening,
    ) -> Result<Worked, String> {
        let millis = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let home = &self.home(info.kind);
        let (refused, probe) = match crate::api::dry_run::execute_dry_run(
            &self.identity,
            &self.state,
            home,
            manifest.clone(),
        )
        .await
        {
            Ok(result) if result.valid => (None, result.probe),
            Ok(result) => (
                Some(format!(
                    "the plan does not validate: {}",
                    serde_json::to_string(&result.plan).unwrap_or_default()
                )),
                None,
            ),
            Err(err) => (Some(err.to_string()), None),
        };
        if let Some(findings) = refused {
            self.event(
                "tool",
                json!({
                    "tool": tool,
                    "status": "failed",
                    "durationMs": millis(),
                    "input": input,
                    "error": findings,
                }),
            )
            .await?;
            if let Some(instead) = not_a_field(info.kind, &findings) {
                return Ok(Worked::Again(format!(
                    "error: the platform's check refuses the change: {findings}; {instead}"
                )));
            }
            if let Some(reason) = self.schema_reply(info.kind, &findings, &manifest).await? {
                return Ok(Worked::Again(reason));
            }
            let prose =
                format!("The change to '{name}' does not pass the platform's check: {findings}");
            if last && opening == Opening::Create {
                return self.open_the_form(info.kind, &prose, route).await;
            }
            return self
                .again(
                    last,
                    format!("error: the platform's check refuses the change: {findings}; the changed manifest was {manifest}"),
                    prose,
                )
                .await;
        }
        // A changed pipeline runs on a page of its source and a changed data source fetches its
        // URL before the form opens (AG-77, PL-45, MF-39); a red test goes back to the model.
        let tested = if untested {
            Ok(Tested {
                card: Some(
                    json!({ "untested": "the change does not touch what the pipeline reads, maps or writes" }),
                ),
                verdict: None,
            })
        } else {
            self.tested(info.kind, &manifest, probe).await
        };
        let mut half_written = None;
        let tested = match tested {
            Ok(tested) => tested,
            Err((findings, card)) => {
                self.event(
                    "tool",
                    json!({
                        "tool": tool,
                        "status": "failed",
                        "durationMs": millis(),
                        "input": input,
                        "error": format!("the test is not green: {findings}"),
                        "output": { "kind": info.kind, "name": name, "test": card },
                    }),
                )
                .await?;
                // A red test goes back to the model while rounds are left, a new resource's too:
                // a new pipeline drafted as `root = this` opened its form red with every row's
                // finding pasted, and nobody wrote the mapping from the source's columns
                // (T-6532, live walk T-6490 S-20).
                if opening == Opening::Change || !last {
                    let hint = if info.kind == "Pipeline" {
                        " Write spec.compute.bloblang from the columns of the rows it ran on: \
                         `id` as urn:ngsi-ld:<Type>:<the row's key column>, `type`, and one \
                         attribute per column the target's model has; then send the call again."
                    } else {
                        ""
                    };
                    return self
                        .again(
                            last,
                            format!("error: the change's test is not green: {findings}; what it ran on: {card}; the changed manifest was {manifest}.{hint}"),
                            format!("The change to '{name}' does not pass its test: {findings}"),
                        )
                        .await;
                }
                // On the last round a new resource whose test is not green still opens its form:
                // the mapping is finished in the editor, with the editor's own test beside it
                // (AG-45). The draft is kept without a verdict, so the strict gate refuses a
                // proposal until the person has checked it there (PF-57), and the finding is
                // what they read first.
                half_written = Some(findings);
                Tested {
                    card: Some(card),
                    verdict: None,
                }
            }
        };
        let workspace = self.active_workspace().await;
        if let Err(err) = self
            .state
            .drafts
            .put_in(
                workspace.as_deref(),
                home,
                info.kind,
                name,
                manifest.clone(),
                None,
                &self.created_by,
                "assistant",
            )
            .await
        {
            return self
                .draft_not_kept(tool, started, &input, info.kind, name, &err)
                .await;
        }
        // The draft carries the check it passed, so the form proposes it without a second one
        // while the person leaves it as it is (AG-77). A new resource whose test is not green
        // carries no verdict at all: the strict gate then refuses a proposal until the person has
        // finished it in the form and checked it there (PF-57), which is the whole point of
        // opening the form rather than sending the model round again.
        if half_written.is_none() {
            let verdict = tested
                .verdict
                .unwrap_or_else(|| crate::ops::verdict::Verdict::green(&manifest, None));
            if let Err(err) = self
                .state
                .drafts
                .set_verdict_in(workspace.as_deref(), home, info.kind, name, verdict)
                .await
            {
                tracing::warn!(run = %self.run_id, kind = %info.kind, error = %err, "change verdict not kept");
            }
        }
        let mut output =
            json!({ "kind": info.kind, "name": name, "checked": half_written.is_none() });
        if let Some(card) = tested.card {
            output["test"] = card;
        }
        if let Some(findings) = &half_written {
            output["unfinished"] = json!(findings);
        }
        self.event(
            "tool",
            json!({
                "tool": tool,
                "status": "ok",
                "durationMs": millis(),
                "input": input,
                "output": output,
            }),
        )
        .await?;
        let mut prose = share::prose_of(answer);
        if prose.is_empty() {
            prose = format!("Opened '{name}' with the change; review it and propose it.");
        }
        if let Some(findings) = &half_written {
            // The person reads what is still wrong with it before they read the form (AG-45).
            prose = format!(
                "{prose}\n\nIts test is not green yet: {findings}. The form is open on it; finish \
                 it there and check it before you propose it."
            );
        }
        // The rest of a request for several resources, each with its page (T-6531).
        let steps: Vec<change::NextStep> = input
            .get("next")
            .and_then(|next| serde_json::from_value(next.clone()).ok())
            .unwrap_or_default();
        prose.push_str(&change::next_steps(
            &self.project,
            &steps,
            workspace.as_deref(),
            |kind| self.may_change(kind, Verb::Propose).is_ok(),
        ));
        self.thought(&prose).await?;
        // A new resource opens the page on its own draft, and the form loads that draft itself:
        // handing the manifest over as the form's values as well would fill the form twice, the
        // first time from a shape no form reads. The endpoint form reads its own values; every
        // other page opens a change on the manifest.
        let mut payload = json!({ "route": route, "draft": draft });
        if opening == Opening::Change {
            payload["prefill"] = if info.kind == "Endpoint" {
                share::form_values(&manifest)
            } else {
                manifest
            };
        }
        self.event("navigate", payload).await?;
        Ok(Worked::Done(prose))
    }

    /// The test of a changed Pipeline or DataSource; every other kind has none. `Err` carries the
    /// findings and the card of what the test ran on; a test that could not run (no runner, a
    /// credential, a source without a page) is a card that says why, never a refusal.
    pub(super) async fn tested(
        &self,
        kind: &str,
        manifest: &Value,
        probe: Option<crate::api::dry_run::Probe>,
    ) -> Result<Tested, (String, Value)> {
        match kind {
            "DataSource" => {
                let Some(probe) = probe else {
                    return Ok(Tested::default());
                };
                let failed = crate::api::pipeline_test::feed_failed(&probe).map(str::to_owned);
                let card =
                    json!({ "source": { "url": manifest["spec"]["http"]["url"] }, "probe": probe });
                match failed {
                    Some(reason) => Err((reason, card)),
                    None => Ok(Tested {
                        card: Some(card),
                        verdict: None,
                    }),
                }
            }
            "Pipeline" => {
                let (sample, source) = match self.page_of(manifest).await {
                    Ok(page) => page,
                    Err(untested) => {
                        return Ok(Tested {
                            card: Some(json!({ "untested": untested })),
                            verdict: None,
                        })
                    }
                };
                let Some(op) = crate::ops::find("jc_pipeline_test") else {
                    return Ok(Tested::default());
                };
                let caller = crate::ops::Caller {
                    bearer: None,
                    identity: self.identity.clone(),
                    via: crate::ops::Via::Agent,
                    access: None,
                };
                let test = json!({ "pipeline": manifest, "sample": sample });
                let out = match crate::ops::call(op, &caller, &self.state, &self.project, test)
                    .await
                {
                    Ok(out) => out,
                    Err(err) => {
                        return Ok(Tested {
                            card: Some(json!({ "source": source, "untested": err.to_string() })),
                            verdict: None,
                        })
                    }
                };
                let verdict = out.get("verdict").cloned().unwrap_or(Value::Null);
                let card = json!({
                    "source": source,
                    "verdict": { "ok": verdict["ok"], "findings": verdict["findings"] },
                    "records": out.pointer("/input/events"),
                    "sample": out.get("mapping").and_then(Value::as_array).map(|m| m.iter().take(3).cloned().collect::<Vec<_>>()),
                });
                if verdict["ok"].as_bool() == Some(true) {
                    Ok(Tested {
                        card: Some(card),
                        verdict: serde_json::from_value(verdict).ok(),
                    })
                } else {
                    Err((findings_of(&verdict), card))
                }
            }
            _ => Ok(Tested::default()),
        }
    }

    /// What a pipeline is tested on (PL-43): one fetch of its `http` data source, read as the runner
    /// reads it (`sampling`), or a page of the endpoint it reads when this conversation reads that
    /// endpoint, with the source it came from. `Err` says why there is none.
    pub(super) async fn page_of(&self, manifest: &Value) -> Result<(Value, Value), String> {
        let source = &manifest["spec"]["source"];
        if let Some(name) = source["dataSourceRef"]["name"].as_str() {
            let Some(data_source) = self.state.mirror.get(&self.project, "DataSource", name) else {
                return Err(format!("its data source '{name}' does not exist"));
            };
            return match crate::api::pipeline_test::probe_plan(&data_source.spec) {
                Some(Ok(url)) => crate::api::pipeline_test::sampling(&data_source.spec, url)
                    .map(|(sample, _)| (json!(sample), json!({ "dataSource": name })))
                    .map_err(|e| format!("its data source '{name}' cannot be sampled: {e}")),
                Some(Err(skipped)) => Err(skipped.skipped.unwrap_or_default()),
                None => Err(format!(
                    "its data source '{name}' is not fetched over http, so it has no page to test on"
                )),
            };
        }
        let Some(endpoint) = source["endpointRef"]["name"].as_str() else {
            return Err("it names no source to test on".to_owned());
        };
        let chosen = match self.state.agents.get_run(&self.run_id).await {
            Ok(Some(run)) => endpoints::of_run(&run),
            _ => self.endpoints.clone(),
        };
        let Some(index) = chosen.iter().position(|e| e.name == endpoint) else {
            return Err(format!(
                "this conversation does not read its endpoint '{endpoint}'"
            ));
        };
        let query = &source["query"];
        let base = endpoints::data_base(&self.proxy_base, &chosen, index);
        // A read names a type (GW33): a query with no selector is 400 BadRequestData, so a
        // source that names none is tested on the first type the endpoint lists.
        let named = query["type"].as_str().map(str::to_owned);
        let chosen_type = match named {
            Some(one) => Some(one),
            None => first_served_type(&self.read_text(&format!("{base}/ngsi-ld/v1/types")).await?),
        };
        let Some(chosen_type) = chosen_type else {
            return Err(format!(
                "its endpoint '{endpoint}' names no type to read, so there is no page to test on"
            ));
        };
        let mut url = format!("{base}/ngsi-ld/v1/entities?limit=100");
        for (parameter, value) in [
            ("type", Some(chosen_type)),
            ("q", query["q"].as_str().map(str::to_owned)),
            (
                "attrs",
                query["attrs"].as_array().map(|attrs| {
                    attrs
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                }),
            ),
        ] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                url.push_str(&format!("&{parameter}={}", urlencoding(&value)));
            }
        }
        let page = self.read_text(&url).await?;
        Ok((
            // The runner reads an endpoint's page as one message, as the KPI pipeline test does.
            json!({ "text": page, "format": "text" }),
            json!({ "endpoint": endpoint }),
        ))
    }

    /// A manifest the kind's own parser refused goes back with that kind's schema, once a turn and
    /// on the last draft too: the model guessed the fields, and another guess costs a draft. The
    /// schema reaches this prompt only here (T-5948); `None` for any other refusal (T-6488).
    async fn schema_reply(
        &self,
        kind: &str,
        findings: &str,
        manifest: &Value,
    ) -> Result<Option<String>, String> {
        if !findings.contains(NOT_PARSED) {
            return Ok(None);
        }
        let first = self
            .schema_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(kind.to_owned());
        if !first {
            return Ok(None);
        }
        let started = std::time::Instant::now();
        let Ok(schema) = crate::ops::schemas::schema_of(kind, None) else {
            return Ok(None);
        };
        self.event(
            "tool",
            json!({
                "tool": crate::ops::schemas::GET,
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": { "kind": kind },
                "output": { "kind": kind },
            }),
        )
        .await?;
        Ok(Some(format!(
            "error: the platform's check refuses the change: {findings}; the manifest was \
             {manifest}; the schema of {kind}, read for you: {schema}; call change_resource again \
             with only the fields this schema names"
        )))
    }

    /// A new resource the model could not draft ends on its kind's page, where the person finishes
    /// it in the form; nothing was kept (AG-45, T-6488).
    async fn open_the_form(
        &self,
        kind: &str,
        prose: &str,
        route: String,
    ) -> Result<Worked, String> {
        let prose = format!(
            "{prose}\n\nNothing was saved. Create it in the form on [the {kind} page]({route}); \
             a key or password goes in the form's secret field, never in this chat."
        );
        self.thought(&prose).await?;
        self.event("navigate", json!({ "route": route })).await?;
        Ok(Worked::Done(prose))
    }

    /// What goes back to the model while drafts are left, or the person's answer on the last one.
    pub(super) async fn again(
        &self,
        last: bool,
        reason: String,
        prose: String,
    ) -> Result<Worked, String> {
        if last {
            self.thought(&prose).await?;
            return Ok(Worked::Done(prose));
        }
        Ok(Worked::Again(reason))
    }

    /// A draft the store refused or could not keep ends the tool, since the card and the form
    /// would open on a draft that is not there (JC1-30). The person reads what to do; a database
    /// error's own text stays in the log.
    pub(super) async fn draft_not_kept(
        &self,
        tool: &str,
        started: std::time::Instant,
        input: &Value,
        kind: &str,
        name: &str,
        err: &crate::ops::drafts::DraftError,
    ) -> Result<Worked, String> {
        use crate::ops::drafts::DraftError;
        tracing::warn!(run = %self.run_id, kind, name, error = %err, "draft not kept");
        let reason = match err {
            DraftError::Secret(field) => format!(
                "the {kind} '{name}' holds a literal secret in '{field}'; name it by secretRef"
            ),
            DraftError::Conflict { .. } => {
                format!("the draft of {kind} '{name}' changed meanwhile; open it again")
            }
            DraftError::Db(_) | DraftError::NotFound { .. } => {
                format!("the draft of {kind} '{name}' could not be saved; try again in a moment")
            }
        };
        self.event("tool", failed_step(tool, started, input, &reason))
            .await?;
        let prose = format!("Nothing was opened: {reason}.");
        self.thought(&prose).await?;
        Ok(Worked::Done(prose))
    }

    /// The project's endpoints as manifests, from the Portal's mirror of the repository.
    pub(super) fn project_endpoints(&self) -> Vec<Value> {
        self.state
            .mirror
            .list(
                &self.project,
                "Endpoint",
                &crate::store::ListOptions::default(),
            )
            .items
            .iter()
            .filter_map(|envelope| serde_json::to_value(envelope).ok())
            .collect()
    }

    /// The entity types `endpoint` serves, from its space's compiled model.
    fn served_types(&self, endpoint: &Value) -> Vec<String> {
        crate::resource::reference_name(&endpoint["spec"]["contextSpaceRef"])
            .map(|space| {
                crate::api::access_requests::served_types(
                    &self.state,
                    &self.project,
                    space,
                    endpoint,
                )
            })
            .unwrap_or_default()
    }

    /// The `edit_endpoint` call of the earlier prompt, still read for one release (EP-72, AG-77):
    /// the edit becomes the endpoint's changed manifest and opens like a `change_resource` change.
    pub(super) async fn edit_endpoint(
        &self,
        call: Result<share::EditEndpoint, String>,
        answer: &str,
    ) -> Result<String, String> {
        const TOOL: &str = "edit_endpoint";
        let started = std::time::Instant::now();
        let failed = |input: &Value, reason: &str| {
            json!({
                "tool": TOOL,
                "status": "failed",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": input,
                "error": reason,
            })
        };
        let params = match call {
            Ok(params) => params,
            Err(reason) => {
                self.event("tool", failed(&Value::Null, &reason)).await?;
                let prose = format!("The change could not be read: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let input = serde_json::to_value(&params).unwrap_or(Value::Null);
        if let Err(reason) = self.may_change("Endpoint", Verb::Propose) {
            return self.refused(TOOL, started, input, reason).await;
        }
        let edit = match share::edit(&self.project_endpoints(), &params, |endpoint| {
            self.served_types(endpoint)
        }) {
            Ok(edit) => edit,
            Err(reason) => {
                self.event("tool", failed(&input, &reason)).await?;
                let prose = format!("The endpoint could not be changed: {reason}");
                self.thought(&prose).await?;
                return Ok(prose);
            }
        };
        let name = params.name.trim();
        let info = crate::resource::by_kind("Endpoint").ok_or("Endpoint is not a kind")?;
        let route = change::route(&self.project, info.kind, info.plural, name, false);
        let draft = json!({ "kind": info.kind, "name": name });
        match self
            .open_change(
                info,
                name,
                edit.endpoint,
                answer,
                TOOL,
                input,
                started,
                true,
                route,
                draft,
                false,
            )
            .await?
        {
            Worked::Done(prose) | Worked::Again(prose) => Ok(prose),
        }
    }

    /// The model's catalog search (AG-70): the platform runs it and records the tool step;
    /// `None` when nothing matches.
    async fn catalog(&self, q: &str) -> Result<Option<Value>, String> {
        let started = std::time::Instant::now();
        let effective = crate::permissions::for_request(&self.state, &self.identity, &self.project);
        let catalog =
            crate::api::assistant::search(&self.state, &self.project, &effective, q, None).await;
        let mut output = serde_json::to_value(&catalog).unwrap_or(Value::Null);
        // The search also reads the open-data catalogue, which is public (EP-67): a question
        // about another city finds its published datasets (T-6104).
        let (datasets, _) =
            crate::api::catalogue::datasets_matching(&self.state, q, CATALOGUE_DATASETS).await;
        output["datasets"] = json!(datasets);
        self.event(
            "tool",
            json!({
                "tool": "search_catalog",
                "status": "ok",
                "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "input": { "q": q },
                "output": output.clone(),
            }),
        )
        .await?;
        Ok((!catalog.items.is_empty() || !datasets.is_empty()).then_some(output))
    }

    /// A `search_catalog` call of the model, with its own words, answered as text the next
    /// model call reads.
    pub(super) async fn search(&self, q: &str) -> Result<String, String> {
        if let Err(reason) = self.granted("jc_catalog_search") {
            return Ok(format!("error: {reason}"));
        }
        Ok(match self.catalog(q).await? {
            Some(output) => output.to_string(),
            None => format!(
                "nothing in the project, the Endpoints shared with it or the open-data catalogue \
                 matches the words '{q}'"
            ),
        })
    }

    /// The profile and the person who started the run both allow the operation behind a tool
    /// (AG-70).
    pub(super) fn granted(&self, operation: &str) -> Result<(), String> {
        self.access
            .check(operation, &self.identity, &self.state, &self.project)
    }

    /// A tool call outside the run's access: a failed `tool` event before anything runs, and the
    /// reason in the chat (AG-56, AG-70).
    pub(super) async fn refused(
        &self,
        tool: &str,
        started: std::time::Instant,
        input: Value,
        reason: String,
    ) -> Result<String, String> {
        self.event("tool", failed_step(tool, started, &input, &reason))
            .await?;
        let prose = format!("That is outside what this assistant may do: {reason}");
        self.thought(&prose).await?;
        Ok(prose)
    }

    /// A prompt without the sections of the tools this run may not call, so the model is shown
    /// only the effective tool set (AG-70).
    pub(super) fn narrowed(&self, prompt: &str) -> String {
        TOOL_SECTIONS
            .iter()
            .filter(|(_, operation, kind)| {
                match kind {
                    Some(kind) => self.may_change(kind, Verb::Propose),
                    None => self.granted(operation),
                }
                .is_err()
            })
            .fold(prompt.to_owned(), |text, (heading, _, _)| {
                without_section(&text, heading)
            })
    }
}

/// The open datasets one model search reads back: titles and links, a few hundred tokens.
const CATALOGUE_DATASETS: usize = 5;

/// The first type an endpoint's `/ngsi-ld/v1/types` answer lists, for a source that names none.
///
/// The answer is an `EntityTypeList`; anything else (an error page, an empty list) yields
/// `None`, and the caller says so rather than sending a query the gateway must refuse.
/// `route` inside the copy `workspace`, so the page opens on the copy's resource (CC-76).
fn within(route: String, workspace: Option<&str>) -> String {
    match workspace {
        Some(name) if route.contains('?') => format!("{route}&workspace={name}"),
        Some(name) => format!("{route}?workspace={name}"),
        None => route,
    }
}

fn first_served_type(page: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(page).ok()?;
    parsed["typeList"]
        .as_array()?
        .iter()
        .find_map(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod not_a_field_tests {
    use super::not_a_field;

    /// T-6533, S-25: the walk's invented `spec.paused` is answered with the pause operation, as
    /// jc-core words the refusal; other kinds and other refusals keep the schema reply.
    #[test]
    fn a_paused_syncsource_field_names_the_pause_operation() {
        let findings = "bad request: spec is not a valid SyncSource: manifest does not parse: \
                        spec: unknown field `paused`, expected one of `source`, `schedule`";
        let instead = not_a_field("SyncSource", findings).expect("the pause operation");
        assert!(instead.contains("jc_syncsource_pause"), "{instead}");
        assert_eq!(not_a_field("Pipeline", findings), None);
        let other = "manifest does not parse: spec: unknown field `interval`";
        assert_eq!(not_a_field("SyncSource", other), None);
        let blank = "invalid schedule.interval ``: interval must be a positive number";
        assert_eq!(not_a_field("SyncSource", blank), None);
    }
}

#[cfg(test)]
mod type_list_tests {
    use super::first_served_type;

    /// GW33: a source that names no type is tested on the first type the endpoint lists, so the
    /// Portal never sends the selector-less query the gateway must refuse (T-0780).
    #[test]
    fn reads_the_first_type_an_endpoint_serves() {
        let page = r#"{"id":"urn:ngsi-ld:EntityTypeList:1","type":"EntityTypeList",
            "typeList":["AirQualityObserved","Device"]}"#;
        assert_eq!(
            first_served_type(page).as_deref(),
            Some("AirQualityObserved")
        );
    }

    #[test]
    fn an_endpoint_that_serves_nothing_names_no_type() {
        for page in [
            r#"{"id":"urn:ngsi-ld:EntityTypeList:1","type":"EntityTypeList","typeList":[]}"#,
            r#"{"type":"EntityTypeList"}"#,
            r#"{"title":"Forbidden","status":403}"#,
            "not json at all",
            "",
        ] {
            assert_eq!(first_served_type(page), None, "{page}");
        }
    }

    /// A list whose first entry is not a string is skipped rather than read as one.
    #[test]
    fn a_malformed_entry_does_not_become_a_type() {
        let page = r#"{"typeList":[{"id":"nope"},"Device"]}"#;
        assert_eq!(first_served_type(page).as_deref(), Some("Device"));
    }
}

#[cfg(test)]
mod workspace_guard_tests {
    use super::*;

    #[test]
    fn a_route_opens_inside_the_copy() {
        assert_eq!(
            within("/p/x/pipelines?edit=a".into(), Some("air")),
            "/p/x/pipelines?edit=a&workspace=air"
        );
        assert_eq!(
            within("/p/x/endpoints".into(), Some("air")),
            "/p/x/endpoints?workspace=air"
        );
        assert_eq!(within("/p/x/endpoints".into(), None), "/p/x/endpoints");
    }

    async fn opened(state: &AppState, name: &str, owner: &str, ttl_hours: i64) {
        state
            .workspaces
            .create(crate::ops::workspaces::Opening {
                name,
                title: None,
                project: "helsinki",
                owner,
                base_revision: "b",
                scope: crate::ops::workspaces::Scope::Project {},
                ttl_hours,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_copy_in_use_is_the_persons_newest_live_one() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state.clone(), "helsinki");
        assert_eq!(driver.active_workspace().await, None);
        opened(&state, "someone-elses", "petra@hel.fi", 24).await;
        assert_eq!(
            driver.active_workspace().await,
            None,
            "another person's copy is not theirs"
        );
        opened(&state, "first", "test-user@hel.fi", 24).await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        opened(&state, "second", "test-user", 24).await;
        assert_eq!(driver.active_workspace().await.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn a_second_resource_outside_a_copy_is_refused_with_the_way_forward() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state.clone(), "helsinki");
        assert_eq!(
            driver.second_resource("Pipeline", "bikes").await,
            None,
            "the first change"
        );
        state
            .drafts
            .put(
                "helsinki",
                "Pipeline",
                "bikes",
                json!({}),
                None,
                "test-user@hel.fi",
                "assistant",
            )
            .await
            .unwrap();
        assert_eq!(
            driver.second_resource("Pipeline", "bikes").await,
            None,
            "the same resource again"
        );
        let reason = driver
            .second_resource("DataSource", "feed")
            .await
            .expect("refused");
        assert!(
            reason.contains("jc_workspace_open") && reason.contains("Pipeline 'bikes'"),
            "{reason}"
        );
        // A draft the person typed, or one inside a copy, is not this conversation's change.
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state.clone(), "helsinki");
        state
            .drafts
            .put(
                "helsinki",
                "Pipeline",
                "bikes",
                json!({}),
                None,
                "test-user@hel.fi",
                "form",
            )
            .await
            .unwrap();
        state
            .drafts
            .put_in(
                Some("air"),
                "helsinki",
                "Pipeline",
                "air",
                json!({}),
                None,
                "test-user@hel.fi",
                "assistant",
            )
            .await
            .unwrap();
        assert_eq!(driver.second_resource("DataSource", "feed").await, None);
    }

    /// JC1-13: an assistant draft of an earlier conversation is not this run's change.
    #[tokio::test]
    async fn an_earlier_conversations_draft_does_not_refuse_this_ones_change() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state
            .drafts
            .put(
                "helsinki",
                "Pipeline",
                "bikes",
                json!({}),
                None,
                "test-user@hel.fi",
                "assistant",
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let driver = Driver::for_tests(state.clone(), "helsinki");
        assert_eq!(driver.second_resource("DataSource", "feed").await, None);
    }
}

#[cfg(test)]
mod new_dashboard_tests {
    //! `new_dashboard` (T-2514; UI-19, AG-77): a `change_resource` Dashboard create turned into
    //! Layer and Dashboard manifests, refused before any draft when it would not stand.

    use super::*;
    use crate::agents::change::{ChangeResource, NewLayer};
    use crate::git::GiteaClient;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::state::AppState;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LINKML: &str = "id: https://hel.fi/models/bikes\nname: bikes\nclasses:\n  BikeHireDockingStation:\n    slots: [id, name, availableBikeNumber]\nslots:\n  id: {}\n  name: { range: string }\n  availableBikeNumber: { range: integer }\n";

    fn put(state: &AppState, kind: &str, name: &str, spec: Value) {
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        });
    }

    /// The forge serving the `bikes` model's source, and the project around it: a public and a
    /// private endpoint over the space, one existing layer on each, one existing dashboard.
    async fn world() -> (Driver, MockServer) {
        let forge = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/org/manifests/contents/projects/helsinki/spaces/helsinki/datamodels/bikes.linkml.yaml"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "s",
                "content": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, LINKML),
            })))
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/org/manifests"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&forge)
            .await;
        let client = GiteaClient::new(forge.uri().parse().expect("url"), "org", "manifests", "t")
            .expect("a forge client");
        let state =
            AppState::new(crate::config::Config::for_tests(), None).with_gitea(Arc::new(client));
        put(
            &state,
            "ContextSpace",
            "helsinki",
            json!({ "dataModelRef": "bikes" }),
        );
        put(
            &state,
            "DataModel",
            "bikes",
            json!({ "contextSpaceRef": "helsinki", "linkml": "bikes.linkml.yaml" }),
        );
        put(
            &state,
            "Endpoint",
            "bikes-open",
            json!({ "contextSpaceRef": "helsinki", "audience": "public" }),
        );
        put(
            &state,
            "Endpoint",
            "bikes-staff",
            json!({ "contextSpaceRef": "helsinki", "audience": "project" }),
        );
        put(
            &state,
            "Endpoint",
            "bikes-unsaid",
            json!({ "contextSpaceRef": "helsinki" }),
        );
        put(
            &state,
            "Layer",
            "open-stations",
            json!({ "sourceEndpointRef": "bikes-open", "entityType": "BikeHireDockingStation" }),
        );
        put(
            &state,
            "Layer",
            "staff-stations",
            json!({ "sourceEndpointRef": "bikes-staff", "entityType": "BikeHireDockingStation" }),
        );
        put(
            &state,
            "Dashboard",
            "bikes",
            json!({ "title": "Bikes", "visibility": "project", "pages": [] }),
        );
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.identity.roles = vec![state.config.bootstrap_admins.clone()];
        (driver, forge)
    }

    /// T-6481: the draft test sent every DataSource as `format: json`, so a CSV feed failed its
    /// mapping on "parse as json" while `jc_pipeline_test` passed on the same source. The page is
    /// read as the runner reads it: rows only for `format: csv`, an XML or `.csv` body whole.
    #[tokio::test]
    async fn a_draft_is_tested_on_its_data_source_as_the_runner_reads_it() {
        let (driver, _forge) = world().await;
        for (http, format) in [
            (
                json!({ "url": "https://x.example/counters", "format": "csv" }),
                "csv",
            ),
            (json!({ "url": "https://x.example/counters.csv" }), "text"),
            (json!({ "url": "https://x.example/feed.xml" }), "text"),
            (json!({ "url": "https://x.example/bikes.json" }), "json"),
        ] {
            put(
                &driver.state,
                "DataSource",
                "counters",
                json!({ "type": "http", "http": http }),
            );
            let draft = json!({ "spec": { "source": {
                "dataSourceRef": { "kind": "DataSource", "name": "counters" }
            }}});
            let (sample, source) = driver.page_of(&draft).await.expect("a page");
            assert_eq!(sample["format"], format, "{http}");
            assert_eq!(sample["url"], http["url"]);
            assert_eq!(source, json!({ "dataSource": "counters" }));
        }
    }

    /// What the caller does with every drafted manifest before it is stored: the platform's dry
    /// run, `mutate::propose` with its UI-19 check and the kind's schema (`change_resource`).
    async fn dry_run_refuses(driver: &Driver, params: &ChangeResource) -> bool {
        let manifests = driver
            .new_dashboard("Dashboard", &params.name, params)
            .await
            .expect("drafted");
        driver.dry_run_drafted(&manifests).await.is_err()
    }

    fn layer(name: &str, endpoint: &str, extra: Value) -> NewLayer {
        let mut spec = json!({ "sourceEndpointRef": endpoint, "entityType": "BikeHireDockingStation", "style": "circle" });
        if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
            spec.extend(extra.clone());
        }
        NewLayer {
            name: name.to_owned(),
            spec,
        }
    }

    fn create(visibility: &str, draws: &[&str], layers: Vec<NewLayer>) -> ChangeResource {
        ChangeResource {
            kind: "Dashboard".into(),
            name: "stations".into(),
            patch: Some(json!({ "spec": {
                "title": "Stations",
                "visibility": visibility,
                "pages": [{ "title": "Map", "layout": "full-map", "layers": draws }],
            } })),
            create: true,
            layers,
            ..Default::default()
        }
    }

    async fn drafted(driver: &Driver, params: &ChangeResource) -> Result<Vec<String>, String> {
        driver
            .new_dashboard("Dashboard", &params.name, params)
            .await
            .map(|manifests| {
                manifests
                    .iter()
                    .map(|m| {
                        format!(
                            "{}/{}",
                            m["kind"].as_str().unwrap_or(""),
                            m["metadata"]["name"].as_str().unwrap_or("")
                        )
                    })
                    .collect()
            })
    }

    /// What `share::edit` draws a calendar over: the space's model, narrowed by the projection.
    #[tokio::test]
    async fn an_endpoint_serves_the_classes_of_its_space_model() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state
            .model_schemas
            .replace(std::collections::HashMap::from([(
                ("helsinki".to_owned(), "bikes".to_owned()),
                Arc::new(crate::pipeline_validation::ModelSchema::compile(
                    "bikes",
                    "1.0.0",
                    &json!({ "definitions": { "BikeCounter": { "properties": {} } } }),
                    &["BikeCounter".to_owned()],
                    false,
                )),
            )]));
        let driver = Driver::for_tests(state, "helsinki");
        let served =
            |space: &str| driver.served_types(&json!({ "spec": { "contextSpaceRef": space } }));
        assert_eq!(served("bikes"), ["BikeCounter"]);
        assert!(served("no-model").is_empty(), "no model, no type");
        assert!(driver.served_types(&json!({ "spec": {} })).is_empty());
    }

    /// T-2553, UI-18, UI-19: a public dashboard over new public layers passes the platform's check.
    #[tokio::test]
    async fn a_public_dashboard_over_new_public_layers_is_drafted() {
        let (driver, _forge) = world().await;
        let params = create(
            "public",
            &["new"],
            vec![layer("new", "bikes-open", json!({}))],
        );
        let manifests = driver
            .new_dashboard("Dashboard", &params.name, &params)
            .await
            .expect("drafted");
        let checked = driver.dry_run_drafted(&manifests).await;
        assert!(checked.is_ok(), "{checked:?}");
    }

    /// T-2514, UI-19: a public dashboard reads only public endpoints, whether its layer is new
    /// or one the project has, and an endpoint that says no audience is not public.
    #[tokio::test]
    async fn a_public_dashboard_layer_on_a_private_endpoint_is_refused() {
        let (driver, _forge) = world().await;
        for (case, params) in [
            (
                "a new layer on a project endpoint",
                create(
                    "public",
                    &["new"],
                    vec![layer("new", "bikes-staff", json!({}))],
                ),
            ),
            (
                "a new layer on an endpoint with no audience",
                create(
                    "public",
                    &["new"],
                    vec![layer("new", "bikes-unsaid", json!({}))],
                ),
            ),
            (
                "a public and a private new layer",
                create(
                    "public",
                    &["a", "b"],
                    vec![
                        layer("a", "bikes-open", json!({})),
                        layer("b", "bikes-staff", json!({})),
                    ],
                ),
            ),
        ] {
            let refused = drafted(&driver, &params).await.expect_err(case);
            assert!(refused.contains("UI-19"), "{case}: {refused}");
        }
        // A layer the project has is not looked at here; the caller's dry run refuses it before
        // anything is drafted (mutate.rs 4b', dashboards::check), and passes a public one.
        let existing = create("public", &["staff-stations"], Vec::new());
        assert!(dry_run_refuses(&driver, &existing).await);
        assert!(!dry_run_refuses(&driver, &create("public", &["open-stations"], Vec::new())).await);
        let drawn = create(
            "public",
            &["open-stations", "new"],
            vec![layer("new", "bikes-open", json!({}))],
        );
        assert_eq!(
            drafted(&driver, &drawn).await,
            Ok(vec![
                "Layer/new".to_owned(),
                "Dashboard/stations".to_owned()
            ])
        );
    }

    /// T-2514, UI-19: a project dashboard may read any endpoint of the project.
    #[tokio::test]
    async fn a_private_dashboard_may_read_a_private_endpoint() {
        let (driver, _forge) = world().await;
        for visibility in ["project", "organization"] {
            let params = create(
                visibility,
                &["staff", "staff-stations"],
                vec![layer("staff", "bikes-staff", json!({}))],
            );
            assert_eq!(
                drafted(&driver, &params).await,
                Ok(vec![
                    "Layer/staff".to_owned(),
                    "Dashboard/stations".to_owned()
                ]),
                "{visibility}"
            );
        }
    }

    /// T-2514, AG-77: a create never overwrites what exists; nor is any other kind created here.
    #[tokio::test]
    async fn an_existing_dashboard_name_is_refused_as_a_patch_not_a_create() {
        let (driver, _forge) = world().await;
        let mut params = create("project", &["open-stations"], Vec::new());
        params.name = "bikes".into();
        let refused = drafted(&driver, &params)
            .await
            .expect_err("an existing dashboard");
        assert!(
            refused.contains("already exists; change it with a patch"),
            "{refused}"
        );
        let other = driver
            .new_dashboard("Endpoint", "x", &params)
            .await
            .expect_err("an endpoint");
        assert!(
            other.contains("only a Dashboard is created here"),
            "{other}"
        );
    }

    /// T-2514: a layer reading something that is not an endpoint of the project is refused with
    /// the project's endpoints, so the model can name one that exists.
    #[tokio::test]
    async fn a_layer_naming_an_unknown_endpoint_lists_the_projects_endpoints() {
        let (driver, _forge) = world().await;
        for source in ["bikes-gone", "", "../bikes-open"] {
            let params = create("project", &["new"], vec![layer("new", source, json!({}))]);
            let refused = drafted(&driver, &params).await.expect_err(source);
            assert!(
                refused.ends_with("its endpoints are bikes-open, bikes-staff, bikes-unsaid"),
                "{source}: {refused}"
            );
        }
    }

    /// T-2514: a new layer never replaces one the project has, spaces around its name included.
    #[tokio::test]
    async fn a_layer_reusing_an_existing_layer_name_is_refused() {
        let (driver, _forge) = world().await;
        for name in ["open-stations", " open-stations "] {
            let params = create(
                "project",
                &["open-stations"],
                vec![layer(name, "bikes-open", json!({}))],
            );
            let refused = drafted(&driver, &params).await.expect_err(name);
            assert!(
                refused.contains("Layer 'open-stations' already exists"),
                "{refused}"
            );
        }
    }

    /// T-2514: an attribute the type does not have, named anywhere in the layer, is refused with
    /// the type's real attributes.
    #[tokio::test]
    async fn an_unknown_attribute_named_by_a_layer_lists_the_types_real_attributes() {
        let (driver, _forge) = world().await;
        for extra in [
            json!({ "colorBy": { "property": "freeBikes" } }),
            json!({ "sizeBy": { "property": "freeBikes" } }),
            json!({ "popupProperties": ["name", "freeBikes"] }),
        ] {
            let params = create(
                "project",
                &["new"],
                vec![layer("new", "bikes-open", extra.clone())],
            );
            let refused = drafted(&driver, &params)
                .await
                .expect_err("an unknown attribute");
            assert!(
                refused.contains("names freeBikes, which BikeHireDockingStation"),
                "{extra}: {refused}"
            );
            assert!(refused.contains("availableBikeNumber"), "{refused}");
            assert!(refused.contains("name"), "{refused}");
        }
        let known = create(
            "project",
            &["new"],
            vec![layer(
                "new",
                "bikes-open",
                json!({ "colorBy": { "property": "availableBikeNumber" }, "popupProperties": ["name"] }),
            )],
        );
        assert!(drafted(&driver, &known).await.is_ok());
    }

    /// T-2514: a type the endpoint's model does not have is refused when the layer names an
    /// attribute of it.
    #[tokio::test]
    async fn an_entity_type_the_endpoints_schema_does_not_have_is_refused() {
        let (driver, _forge) = world().await;
        let mut unknown = layer("new", "bikes-open", json!({ "popupProperties": ["name"] }));
        unknown.spec["entityType"] = json!("WeatherObserved");
        let refused = drafted(&driver, &create("project", &["new"], vec![unknown]))
            .await
            .expect_err("a type");
        assert_eq!(
            refused,
            "the data model behind 'bikes-open' has no type 'WeatherObserved'"
        );
    }

    /// T-2514: every layer a page draws is new or the project's own.
    #[tokio::test]
    async fn a_page_drawing_a_layer_that_is_neither_new_nor_existing_is_refused() {
        let (driver, _forge) = world().await;
        let params = create("project", &["open-stations", "ghost"], Vec::new());
        let refused = drafted(&driver, &params).await.expect_err("a ghost layer");
        assert!(refused.contains("draws 'ghost'"), "{refused}");
        assert!(
            refused.ends_with("the project's layers are open-stations, staff-stations"),
            "{refused}"
        );
    }

    /// T-2514: a create without the dashboard's spec is refused with the shape it needs.
    #[tokio::test]
    async fn a_dashboard_with_no_spec_patch_is_refused() {
        let (driver, _forge) = world().await;
        for patch in [
            None,
            Some(json!("spec")),
            Some(json!({ "spec": "x" })),
            Some(json!({ "spec": [] })),
        ] {
            let mut params = create("project", &[], Vec::new());
            params.patch = patch.clone();
            let refused = drafted(&driver, &params).await.expect_err("no spec");
            assert!(
                refused.contains("a new dashboard carries its spec as the patch"),
                "{patch:?}: {refused}"
            );
        }
        // A patch that is an object is the spec itself (`change::spec_of`); an empty one is
        // drafted here and refused by the caller's dry run, so it never becomes a draft.
        for patch in [json!({}), json!({ "spec": {} }), json!({ "metadata": {} })] {
            let mut params = create("project", &[], Vec::new());
            params.patch = Some(patch.clone());
            assert!(dry_run_refuses(&driver, &params).await, "{patch}");
        }
    }

    /// T-2514: a layer that names no attribute needs no schema: the forge is never read.
    #[tokio::test]
    async fn a_layer_naming_no_attributes_skips_the_schema_fetch_entirely() {
        let (driver, forge) = world().await;
        let mut plain = layer("new", "bikes-open", json!({}));
        plain.spec["entityType"] = json!("AnyType");
        assert!(drafted(&driver, &create("project", &["new"], vec![plain]))
            .await
            .is_ok());
        assert!(forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }
}

#[cfg(test)]
mod model_create_tests {
    //! JC1-05: the Create a data model path drafts a new DataModel and opens its editor.

    use super::*;
    use crate::agents::change::ChangeResource;
    use crate::agents::paths::Path;
    use crate::state::AppState;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A steward of `helsinki` and a Model Tools service that compiles every source it is sent.
    async fn steward() -> (Driver, AppState, MockServer) {
        let tools = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "errors": [] })))
            .mount(&tools)
            .await;
        let mut config = crate::config::Config::for_tests();
        config.model_tools_url = Some(tools.uri());
        let state = AppState::new(config, None);
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.identity.roles = vec![state.config.bootstrap_admins.clone()];
        // A profile granting the proposal, so the person's own rights decide (AG-70).
        let operations: Vec<&str> = crate::ops::registry().iter().map(|op| op.name).collect();
        driver.access = crate::agents::access::Access::from_spec(&json!({ "access": {
            "operations": operations,
            "kinds": [{ "kind": "DataModel", "verbs": ["read", "propose"] }],
        } }));
        *driver.path.lock().unwrap_or_else(PoisonError::into_inner) = Some(Path::CreateDataModel);
        (driver, state, tools)
    }

    fn lanes(operations: Value) -> ChangeResource {
        ChangeResource {
            kind: "DataModel".into(),
            name: "bike-lanes".into(),
            create: true,
            operations: serde_json::from_value(operations).expect("operations"),
            ..Default::default()
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

    #[tokio::test]
    async fn a_new_data_model_is_drafted_and_its_editor_opens_on_the_draft() {
        let (driver, state, tools) = steward().await;
        let worked = driver
            .change_resource(
                Ok(lanes(json!([
                    { "op": "addClass", "name": "BikeLane", "is_a": "Entity" },
                    { "op": "addSlot", "name": "widthMeters", "class": "BikeLane", "range": "float" },
                ]))),
                "Drafting the bike lanes model.",
                true,
            )
            .await
            .expect("worked");
        assert!(matches!(worked, Worked::Done(_)));
        let draft = state
            .drafts
            .get("helsinki", "DataModel", "bike-lanes")
            .await
            .expect("read")
            .expect("a draft is kept");
        let source = draft.manifest["spec"]["linkml"]
            .as_str()
            .expect("the source");
        let model: Value = serde_norway::from_str(source).expect("yaml");
        assert_eq!(model["name"], "bike-lanes");
        assert_eq!(model["classes"]["BikeLane"]["is_a"], "Entity");
        assert!(source.contains("widthMeters"), "{source}");
        let opened = logged(&state, "navigate").await;
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert_eq!(opened[0]["route"], "/projects/helsinki/models");
        assert_eq!(
            opened[0]["draft"],
            json!({ "kind": "DataModel", "name": "bike-lanes" })
        );
        let sent = tools.received_requests().await.unwrap_or_default();
        assert_eq!(
            sent.len(),
            1,
            "the source is compiled once before the editor opens"
        );
    }

    #[tokio::test]
    async fn a_model_without_operations_or_with_a_bad_name_goes_back_and_keeps_nothing() {
        let (driver, state, _tools) = steward().await;
        for (params, says) in [
            (lanes(json!([])), "operations"),
            (
                ChangeResource {
                    name: "Bike Lanes".into(),
                    ..lanes(json!([{ "op": "addClass", "name": "BikeLane" }]))
                },
                "no data model name",
            ),
        ] {
            driver
                .change_resource(Ok(params), "", false)
                .await
                .expect("worked");
            let failed = logged(&state, "tool").await;
            let error = failed
                .last()
                .and_then(|e| e["error"].as_str())
                .unwrap_or_default();
            assert!(error.contains(says), "{error}");
        }
        assert!(logged(&state, "navigate").await.is_empty());
        assert!(state
            .drafts
            .get("helsinki", "DataModel", "bike-lanes")
            .await
            .expect("read")
            .is_none());
    }

    #[tokio::test]
    async fn a_person_who_may_not_propose_a_model_is_refused_before_anything_is_drafted() {
        let (mut driver, state, tools) = steward().await;
        driver.identity.roles.clear();
        driver
            .change_resource(
                Ok(lanes(json!([{ "op": "addClass", "name": "BikeLane" }]))),
                "",
                true,
            )
            .await
            .expect("worked");
        assert!(logged(&state, "navigate").await.is_empty());
        assert!(tools
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }
}

#[cfg(test)]
mod unparsed_tests {
    //! T-6488 (S-14, S-51): a manifest the kind's parser refuses brings the kind's schema back once
    //! and a create the model cannot draft ends on the form, the pasted key kept out of it.

    use super::*;
    use crate::agents::change::ChangeResource;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::state::AppState;

    const KEY: &str = "abc123pastedkey456";

    fn steward() -> (Driver, AppState) {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.identity.roles = vec![state.config.bootstrap_admins.clone()];
        let operations: Vec<&str> = crate::ops::registry().iter().map(|op| op.name).collect();
        driver.access = crate::agents::access::Access::from_spec(&json!({ "access": {
            "operations": operations,
            "kinds": [
                { "kind": "DataSource", "verbs": ["read", "propose"] },
                { "kind": "Endpoint", "verbs": ["read", "propose"] },
            ],
        } }));
        (driver, state)
    }

    fn source(spec: Value) -> ChangeResource {
        ChangeResource {
            kind: "DataSource".into(),
            name: "counters-api".into(),
            create: true,
            patch: Some(json!({ "spec": spec })),
            ..Default::default()
        }
    }

    fn with_http(extra: Value) -> Value {
        let mut http = json!({ "url": "https://counters.invalid/data.csv" });
        crate::plan::merge_patch(&mut http, &extra);
        json!({ "type": "http", "http": http })
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

    async fn call(driver: &Driver, params: ChangeResource, last: bool) -> Worked {
        driver
            .change_resource(Ok(params), "", last)
            .await
            .expect("no event failed")
    }

    #[tokio::test]
    async fn the_first_parse_refusal_brings_the_schema_even_on_the_last_draft() {
        let (driver, state) = steward();
        let guess = || source(with_http(json!({})));
        let wrong = || {
            let mut params = guess();
            params.patch = Some(json!({ "spec": {
                "type": "http",
                "http": { "url": "https://counters.invalid/data.csv" },
                "secrets": [{ "header": "X-Api-Key", "name": "counters-api-key" }],
            } }));
            params
        };

        let Worked::Again(reason) = call(&driver, wrong(), true).await else {
            panic!("the schema goes back instead of ending the turn");
        };
        assert!(reason.contains("manifest does not parse"), "{reason}");
        assert!(reason.contains("authorization"), "the schema: {reason}");
        assert!(reason.len() < 16_000, "a turn's worth: {}", reason.len());
        let tools = logged(&state, "tool").await;
        assert_eq!(tools.len(), 2, "the refusal and the read: {tools:?}");
        assert_eq!(tools[1]["tool"], crate::ops::schemas::GET);
        assert_eq!(tools[1]["input"]["kind"], "DataSource");

        // Once: the second refusal on the last draft ends on the form with its link.
        let Worked::Done(prose) = call(&driver, wrong(), true).await else {
            panic!("the schema goes back once");
        };
        let route = "/projects/helsinki/datasources";
        assert!(prose.contains(&format!("]({route}")), "{prose}");
        let opened = logged(&state, "navigate").await;
        assert_eq!(opened.len(), 1);
        assert!(opened[0]["route"]
            .as_str()
            .is_some_and(|r| r.contains(route)));
        assert!(logged(&state, "tool").await.len() == 3, "no second read");

        // A later turn reads it again, and another kind reads its own at once.
        driver.schema_read.lock().expect("lock").clear();
        assert!(matches!(
            call(&driver, wrong(), true).await,
            Worked::Again(_)
        ));
    }

    #[tokio::test]
    async fn another_refusal_brings_no_schema() {
        let (driver, state) = steward();
        let Worked::Again(reason) = call(&driver, source(json!({ "type": "http" })), false).await
        else {
            panic!("the refusal goes back");
        };
        assert!(!reason.contains("read for you"), "{reason}");
        let read = logged(&state, "tool").await;
        assert!(read.iter().all(|e| e["tool"] != crate::ops::schemas::GET));

        // That refusal spent nothing: the first parse refusal still reads the schema.
        let guess = source(json!({ "type": "http", "guess": true }));
        let Worked::Again(reason) = call(&driver, guess, true).await else {
            panic!("the schema goes back");
        };
        assert!(reason.contains("read for you"), "{reason}");
    }

    #[tokio::test]
    async fn the_named_header_opens_the_form_and_a_pasted_key_opens_nothing() {
        let (driver, state) = steward();
        let named = source(with_http(json!({ "authorization": {
            "header": "X-Api-Key",
            "headerRef": { "name": "counters-api-key" },
        } })));
        assert!(matches!(call(&driver, named, false).await, Worked::Done(_)));
        assert_eq!(logged(&state, "navigate").await.len(), 1, "the form opened");

        let (driver, state) = steward();
        let pasted = source(with_http(json!({ "headers": { "X-Api-Key": KEY } })));
        let worked = call(&driver, pasted, true).await;
        let (Worked::Done(answer) | Worked::Again(answer)) = worked;
        assert!(!answer.contains(KEY), "{answer}");
        let draft = state.drafts.get("helsinki", "DataSource", "counters-api");
        assert!(
            draft.await.ok().flatten().is_none(),
            "no draft holds the key"
        );
    }

    #[tokio::test]
    async fn a_hidden_attribute_reaches_the_change_after_a_guess() {
        let (driver, state) = steward();
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: "counters".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: json!({ "contextSpaceRef": "air", "slug": "abcdefghijklmnopqrstuvwxyz", "audience": "public" }),
            status: None,
        });
        let patch = |projection: Value| ChangeResource {
            kind: "Endpoint".into(),
            name: "counters".into(),
            patch: Some(json!({ "spec": { "projection": projection } })),
            ..Default::default()
        };
        let guess = patch(json!({ "exclude": ["ownerEmail"] }));
        let Worked::Again(reason) = call(&driver, guess, false).await else {
            panic!("the guess goes back");
        };
        assert!(reason.contains("hiddenAttributes"), "{reason}");
        let named = patch(json!({ "hiddenAttributes": ["ownerEmail"] }));
        let _ = call(&driver, named, true).await;
        let tools = logged(&state, "tool").await;
        let parsed = !tools.last().is_some_and(|t| {
            t["error"]
                .as_str()
                .is_some_and(|error| error.contains(NOT_PARSED))
        });
        assert!(parsed, "{tools:?}");
    }

    /// T-6534, walk S-28: the Role drafted for "run pipelines, no writes" granted Pipeline
    /// propose and approve. It goes back to the model naming each write, keeps no draft and opens
    /// no page; on the last draft the person reads why and what to decide.
    #[tokio::test]
    async fn a_role_with_writes_the_person_said_no_to_is_never_drafted() {
        let (driver, state) = steward();
        let asked = "Create a role Analyst: read all Endpoints, run pipelines, no writes.";
        let analyst = || ChangeResource {
            kind: "Role".into(),
            name: "analyst".into(),
            create: true,
            patch: Some(json!({ "spec": { "rules": [
                { "kinds": ["Endpoint"], "verbs": ["read"] },
                { "kinds": ["Pipeline"], "verbs": ["propose", "approve"] },
            ] } })),
            ..Default::default()
        };
        let again = driver
            .change_asked(Ok(analyst()), asked, "", false)
            .await
            .expect("no event failed");
        let Worked::Again(reason) = again else {
            panic!("the Role was drafted");
        };
        assert!(reason.contains("approve on Pipeline"), "{reason}");
        let done = driver
            .change_asked(Ok(analyst()), asked, "", true)
            .await
            .expect("no event failed");
        let Worked::Done(prose) = done else {
            panic!("the last draft went back to the model");
        };
        assert!(prose.contains("you asked for no writes"), "{prose}");
        assert!(logged(&state, "navigate").await.is_empty());
        let failed = logged(&state, "tool").await;
        assert_eq!(failed.len(), 2, "{failed:?}");
        assert!(failed.iter().all(|step| step["status"] == "failed"));
    }
}
