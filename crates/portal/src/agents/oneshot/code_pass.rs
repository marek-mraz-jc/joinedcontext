//! The code pass of a workspace run: the model writes files, the workspace builds and tests them, the first version is published (AG-53, AG-54).

use super::*;
use crate::agents::sandbox;

impl Driver {
    /// The chat line and the instruction lines when another App of the project already has this
    /// run's archetype and layout (AP-137). Each other App's design is read back from the request
    /// of its first run; an App with no run, or a store that does not answer, is left out.
    async fn design_advice(&self, others: &[String]) -> Option<(String, String)> {
        use crate::agents::samples;
        let runs = match self.state.agents.list_runs(&self.project, 500).await {
            Ok(runs) => runs,
            Err(err) => {
                tracing::warn!(run = %self.run_id, error = %err, "the project's runs were not read; no design is compared");
                return None;
            }
        };
        // Newest first: the last run seen of an App that continues nothing is its first.
        let mut first: BTreeMap<&str, &str> = BTreeMap::new();
        for run in &runs {
            if run.app_name != self.app_name
                && run.continues.is_none()
                && others.contains(&run.app_name)
            {
                first.insert(run.app_name.as_str(), run.prompt.as_str());
            }
        }
        let designed: Vec<(String, samples::Design)> = first
            .into_iter()
            .map(|(app, prompt)| (app.to_owned(), samples::design(prompt)))
            .collect();
        let design = samples::design(&self.prompt);
        let advice = samples::advice(&design, &designed)?;
        Some((
            format!(
                "The App {} is already a {} laid out as {}; this one gets its own look and is \
                 laid out as {} so the two do not look alike. Ask for the same layout if you \
                 want it.",
                advice.twin, design.archetype, design.layout, advice.layout
            ),
            samples::advice_section(&design, &advice),
        ))
    }

    /// A code run (Architecture/20 §4.1): one call writes the application over the template,
    /// what does not build goes back once, every generated version is checked against what the
    /// frame observed (SDK-28), and every message after the first run is one more pass over the
    /// same files. The template is the model's context, never the preview (SDK-14).
    pub(super) async fn drive_code(
        &self,
        inbox: &mut broadcast::Receiver<AgentRunEvent>,
        deadline: tokio::time::Instant,
    ) -> Result<(), String> {
        self.status(AgentRunStatus::Starting).await?;
        let mut files = if self.wasm {
            let mut files = preview::wasm_template_files();
            for (path, text) in files.iter_mut() {
                if path.starts_with("server/src/") {
                    *text = code::service_examples(text, |service| self.service_on(service));
                }
            }
            files
        } else {
            preview::template_files()
        };
        // A look of its own, inside the branding and unlike the project's other Apps (AP-123).
        let others: Vec<String> = self
            .state
            .mirror
            .matching(|env| {
                env.kind == "App"
                    && env.metadata.namespace.as_deref() == Some(self.project.as_str())
            })
            .into_iter()
            .map(|env| env.metadata.name)
            .collect();
        let look = crate::agents::theme::tokens(&self.state.branding(), &self.app_name, &others);
        files.insert(
            crate::agents::theme::PATH.to_owned(),
            serde_json::to_string_pretty(&look).unwrap_or_else(|_| look.to_string()),
        );
        match self.jc_types().await {
            Ok(types) => {
                files.insert(code::TYPES.to_owned(), types);
            }
            Err(reason) => {
                self.thought(&format!(
                    "The row types of the endpoint were not rendered, so the template's \
                     placeholder types stand: {reason}"
                ))
                .await?
            }
        }
        // What the run branch holds, so every commit carries what changed since the last one.
        let mut committed = BTreeMap::new();
        self.thought("Writing the application for your request.")
            .await?;

        let samples = self.samples(&self.types()).await?;
        self.status(AgentRunStatus::Building).await?;

        let mut conversation: Vec<(String, String)> = Vec::new();
        let mut instruction = self.prompt.clone();
        // A dashboard is an application that only reads (T-3159), and an analysis a page that
        // answers a question from what it reads (T-3160): the request is framed so.
        if self.kind == "dashboard" {
            instruction = format!("{}\n\n{instruction}", code::DASHBOARD);
        } else if self.kind == "analysis" {
            instruction = format!("{}\n\n{instruction}", code::ANALYSIS);
        }
        // No two Apps of the project alike (AP-137): the look differs by construction, the
        // archetype and the layout are compared here, and a twin is advice, not a refusal.
        if let Some(note) = self.design_advice(&others).await {
            self.thought(&note.0).await?;
            instruction.push_str(&note.1);
        }
        // The generated version in the frame, none until one transpiles.
        let mut shown: Option<Shown> = None;
        // Verification passes since the last instruction (SDK-28).
        let mut verified = 0;
        // The failures the last test-fix pass was made for (SDK-38, T-2997).
        let mut fixing: Option<Vec<String>> = None;
        // Whether the last instruction was handled by the editing agent (see `Check::edited`).
        let mut edited = false;
        // A frame that reports an error but never an observation (an older page, a crash
        // before the application settles) is still checked, on the errors alone.
        let mut fallback: Option<tokio::time::Instant> = None;
        match self
            .code_pass(
                &samples,
                &mut files,
                &conversation,
                &instruction,
                None,
                false,
            )
            .await?
        {
            CodePass::Built { prose, .. } => {
                let (title, prose) = title_of(&prose);
                if let Some(title) = title {
                    if let Err(err) = self.state.agents.set_title(&self.run_id, &title).await {
                        tracing::warn!(run = %self.run_id, error = %err, "title not recorded");
                    }
                    self.event("title", json!({ "title": title })).await?;
                }
                shown = Some(
                    self.publish_code(&files, &prose, Some(&mut committed), true)
                        .await?,
                );
                conversation.push((instruction.clone(), prose));
                // The first version is small so it is on screen fast; the rest follows while the
                // person looks at it (SDK-13).
                self.thought("Completing the application: the other pages, functions and tests.")
                    .await?;
                match self
                    .code_pass(
                        &samples,
                        &mut files,
                        &conversation,
                        &instruction,
                        Some(Fix::Complete),
                        true,
                    )
                    .await
                {
                    Ok(CodePass::Built { prose }) => {
                        shown = Some(
                            self.publish_code(&files, &prose, Some(&mut committed), false)
                                .await?,
                        );
                        conversation.push((COMPLETE_TURN.to_owned(), prose));
                    }
                    Ok(CodePass::Unchanged(prose)) => self.thought(&prose).await?,
                    Ok(CodePass::Failed) => {}
                    Err(reason) => {
                        self.thought(&format!(
                            "The first version stays on screen; completing it failed: {reason}"
                        ))
                        .await?
                    }
                }
            }
            CodePass::Unchanged(prose) => {
                self.thought(&prose).await?;
                conversation.push((instruction.clone(), prose));
            }
            // The run keeps the files it starts from, so the next message and the function
            // route read them, though the frame shows none of them.
            CodePass::Failed => {
                let stored: serde_json::Map<String, Value> = files
                    .iter()
                    .map(|(path, content)| (path.clone(), Value::String(content.clone())))
                    .collect();
                self.state
                    .agents
                    .set_files(&self.run_id, Value::Object(stored))
                    .await
                    .map_err(|err| err.to_string())?;
            }
        }
        self.status(AgentRunStatus::Testing).await?;
        self.status(AgentRunStatus::Previewing).await?;
        if self.unattended {
            self.status(AgentRunStatus::AwaitingApproval).await?;
        }

        let mut queued: std::collections::VecDeque<AgentRunEvent> =
            std::collections::VecDeque::new();
        loop {
            let event = match queued.pop_front() {
                Some(event) => Some(event),
                None => tokio::select! {
                    event = inbox.recv() => match event {
                        Ok(event) => Some(event),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    },
                    () = tokio::time::sleep_until(deadline) => {
                        self.expire().await;
                        return Ok(());
                    }
                    () = sleep_until_some(fallback) => None,
                },
            };
            let Some(event) = event else {
                // No observation came after the error: check the version on the errors alone.
                fallback = None;
                let unobserved = shown.as_mut().filter(|shown| !shown.observed);
                if let Some(on_screen) = unobserved.map(|shown| {
                    shown.observed = true;
                    shown.clone()
                }) {
                    let check = Check {
                        samples: &samples,
                        conversation: &conversation,
                        instruction: &instruction,
                        edited,
                    };
                    if let Some(next) = self
                        .verify(
                            check,
                            &mut files,
                            &mut committed,
                            &on_screen,
                            None,
                            &mut verified,
                        )
                        .await?
                    {
                        shown = Some(next);
                    }
                }
                continue;
            };
            match event.kind.as_str() {
                "status" if is_terminal(&event) => return Ok(()),
                "preview_error" => {
                    if shown.as_ref().is_some_and(|shown| !shown.observed) && fallback.is_none() {
                        fallback = Some(tokio::time::Instant::now() + OBSERVATION_WAIT);
                    }
                }
                "preview_observation" => {
                    let version = event
                        .payload
                        .get("version")
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok());
                    let Some(on_screen) = shown
                        .as_mut()
                        .filter(|shown| Some(shown.version) == version && !shown.observed)
                    else {
                        continue;
                    };
                    on_screen.observed = true;
                    fallback = None;
                    let on_screen = on_screen.clone();
                    // A person's message waiting behind the observation comes first; the
                    // check of a version the message is about to replace is dropped.
                    while let Ok(next) = inbox.try_recv() {
                        queued.push_back(next);
                    }
                    if queued
                        .iter()
                        .any(|next| next.kind == "message" && sent_by_person(next))
                    {
                        continue;
                    }
                    let check = Check {
                        samples: &samples,
                        conversation: &conversation,
                        instruction: &instruction,
                        edited,
                    };
                    if let Some(next) = self
                        .verify(
                            check,
                            &mut files,
                            &mut committed,
                            &on_screen,
                            Some(&event.payload),
                            &mut verified,
                        )
                        .await?
                    {
                        shown = Some(next);
                    }
                }
                "tests" => {
                    let Some(on_screen) = shown.clone() else {
                        continue;
                    };
                    // A person's message waiting behind the result comes first, as with a check.
                    while let Ok(next) = inbox.try_recv() {
                        queued.push_back(next);
                    }
                    if queued
                        .iter()
                        .any(|next| next.kind == "message" && sent_by_person(next))
                    {
                        continue;
                    }
                    let check = Check {
                        samples: &samples,
                        conversation: &conversation,
                        instruction: &instruction,
                        edited,
                    };
                    if let Some(next) = self
                        .tests_failed(
                            check,
                            &mut files,
                            &mut committed,
                            &on_screen,
                            &event.payload,
                            &mut fixing,
                        )
                        .await?
                    {
                        shown = Some(next);
                    }
                }
                "message" if sent_by_person(&event) => {
                    fallback = None;
                    verified = 0;
                    fixing = None;
                    let text = event
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    // The element the person pointed at in the preview, when they did (SDK-46).
                    let scope = event
                        .payload
                        .get("scope")
                        .and_then(Value::as_str)
                        .and_then(code::scope);
                    let text = match scope {
                        Some((file, line)) => format!(
                            "{text}\n\nThe person pointed at {file}:{line} in the preview. Change \
                             {file} only: every other file is refused in this turn."
                        ),
                        None => text,
                    };
                    instruction = text.clone();
                    self.thought("Working on your message…").await?;
                    // A version on screen is edited in place, tool by tool (SDK-20); before one
                    // exists the message is one more whole pass.
                    edited = shown.is_some();
                    if edited {
                        match self
                            .scoped_edit_turn(
                                &mut files,
                                &mut committed,
                                &mut conversation,
                                &text,
                                shown.as_ref(),
                                scope.map(|(file, _)| file),
                            )
                            .await
                        {
                            Ok(Some(next)) => shown = Some(next),
                            Ok(None) => {}
                            Err(message) => {
                                self.thought(&format!("The turn failed: {message}")).await?
                            }
                        }
                        continue;
                    }
                    match self
                        .code_pass(
                            &samples,
                            &mut files,
                            &conversation,
                            &text,
                            None,
                            shown.is_some(),
                        )
                        .await
                    {
                        Ok(CodePass::Built { prose, .. }) => {
                            shown = Some(
                                self.publish_code(
                                    &files,
                                    &prose,
                                    Some(&mut committed),
                                    shown.is_none(),
                                )
                                .await?,
                            );
                            conversation.push((text, prose));
                        }
                        Ok(CodePass::Unchanged(prose)) => {
                            self.thought(&prose).await?;
                            conversation.push((text, prose));
                        }
                        Ok(CodePass::Failed) => {}
                        Err(message) => {
                            self.thought(&format!("The pass failed: {message}")).await?
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// SDK-28: the version on screen checked against the run with no model call, and a
    /// verification pass when the check finds something, at most [`MAX_VERIFICATIONS`] after
    /// each instruction. The version a pass publishes is returned; it is checked in its turn
    /// when its own observation arrives.
    pub(super) async fn verify(
        &self,
        check: Check<'_>,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        on_screen: &Shown,
        observation: Option<&Value>,
        verified: &mut u32,
    ) -> Result<Option<Shown>, String> {
        let since = self
            .state
            .agents
            .events_since(&self.run_id, on_screen.seq)
            .await
            .map_err(|err| err.to_string())?;
        let found = verification::check(check.samples, &since, observation);
        if found.problems.is_empty() {
            if observation.is_some() {
                self.thought(&found.summary()).await?;
            }
            return Ok(None);
        }
        let list = found
            .problems
            .iter()
            .map(|problem| format!("- {problem}"))
            .collect::<Vec<_>>()
            .join("\n");
        if *verified >= MAX_VERIFICATIONS {
            self.thought(&format!(
                "The preview still shows problems after {MAX_VERIFICATIONS} verification \
                 passes; say what to change:\n{list}"
            ))
            .await?;
            return Ok(None);
        }
        *verified += 1;
        self.thought(&format!("Checking the preview found:\n{list}"))
            .await?;
        let mut asked = found.problems.clone();
        if let Some(observation) = observation {
            asked.push(verification::rendered(observation));
        }
        if check.edited {
            return self
                .repair_by_edit(check, files, committed, on_screen, &asked)
                .await;
        }
        match self
            .code_pass(
                check.samples,
                files,
                check.conversation,
                check.instruction,
                Some(Fix::Preview(&asked)),
                true,
            )
            .await
        {
            Ok(CodePass::Built { prose, .. }) => Ok(Some(
                self.publish_code(files, &prose, Some(committed), false)
                    .await?,
            )),
            Ok(CodePass::Unchanged(prose)) => {
                self.thought(&prose).await?;
                Ok(None)
            }
            Ok(CodePass::Failed) => Ok(None),
            // The version on screen stands; a failed call is said, not a failed run.
            Err(message) => {
                self.thought(&format!("The verification pass failed: {message}"))
                    .await?;
                Ok(None)
            }
        }
    }

    /// A verification of a version an instruction produced, handed to the editing agent with
    /// what the check found as its instruction: small targeted calls on the files that exist,
    /// never the whole project written again (SDK-20, SDK-28). The person's conversation keeps
    /// their own turns; the repair is on the stream as its thoughts and tool calls.
    async fn repair_by_edit(
        &self,
        check: Check<'_>,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        on_screen: &Shown,
        found: &[String],
    ) -> Result<Option<Shown>, String> {
        let mut conversation = check.conversation.to_vec();
        match self
            .edit_turn(
                files,
                committed,
                &mut conversation,
                &repair_instruction(check.instruction, found),
                Some(on_screen),
            )
            .await
        {
            Ok(next) => Ok(next),
            // The version on screen stands; a failed call is said, not a failed run.
            Err(message) => {
                self.thought(&format!("The verification pass failed: {message}"))
                    .await?;
                Ok(None)
            }
        }
    }

    /// One pass of a code run: a call, its blocks applied, the project checked, and one repair
    /// call when it does not build (SDK-13, SDK-14). A pass that still does not build leaves the
    /// files as they were and says why.
    ///
    /// `found` is what a verification of the version on screen found, when the pass is one;
    /// `on_screen` says whether a generated version is in the frame, which is what a pass that
    /// still does not build leaves there.
    pub(super) async fn code_pass(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        ask: Option<Fix<'_>>,
        on_screen: bool,
    ) -> Result<CodePass, String> {
        let before = files.clone();
        let (mut prose, mut errors) = self
            .code_step(samples, files, conversation, instruction, ask)
            .await?;
        if !errors.is_empty() {
            // The errors go to the model (`Fix::Build`); the person reads one plain sentence,
            // never the patch protocol (T-0785).
            self.thought("The application does not build; fixing it.")
                .await?;
            (prose, errors) = self
                .code_step(
                    samples,
                    files,
                    conversation,
                    instruction,
                    Some(Fix::Build(&errors)),
                )
                .await?;
        }
        if !errors.is_empty() {
            *files = before;
            let errors: Vec<String> = errors.into_iter().filter(|e| !is_protocol(e)).collect();
            // Only the patch protocol went wrong: the person reads what happened, not the
            // protocol (T-0785, T-1662).
            let reasons = if errors.is_empty() {
                "the model answered without a change to the files".to_owned()
            } else {
                errors.join("\n")
            };
            let said = if on_screen {
                format!(
                    "The application still does not build, so the preview keeps the version \
                     before this request:\n{reasons}"
                )
            } else {
                format!(
                    "The application could not be built:\n{reasons}\nSend a message to try again."
                )
            };
            self.thought(&said).await?;
            return Ok(CodePass::Failed);
        }
        if *files == before {
            return Ok(CodePass::Unchanged(or_else(
                prose,
                "The application is unchanged.",
            )));
        }
        Ok(CodePass::Built {
            prose: or_else(prose, "The application is ready."),
        })
    }

    /// Whether the App's organization, project and App layers all have `service` on (AP-164): an
    /// App the mirror does not hold yet uses none beyond identity and data.
    fn service_on(&self, service: &str) -> bool {
        let Ok(service) = serde_json::from_value::<jc_core::kinds::AppService>(json!(service))
        else {
            return false;
        };
        let Some(spec) = self
            .state
            .mirror
            .find(|env| {
                env.kind == "App"
                    && env.metadata.name == self.app_name
                    && env.metadata.namespace.as_deref() == Some(self.project.as_str())
            })
            .and_then(|env| serde_json::from_value::<jc_core::kinds::AppSpec>(env.spec).ok())
        else {
            return false;
        };
        crate::apps::services::off_at(&self.state.mirror, &self.project, &spec, service).is_none()
    }

    /// Whether this is the run's opening instruction, which `drive_code` may have framed around the
    /// person's words (a dashboard, an analysis, a note on the look): it is the first version all
    /// the same, small and at low effort (SDK-13, AG-72, T-6457).
    fn opens(&self, conversation: &[(String, String)], instruction: &str) -> bool {
        conversation.is_empty() && instruction.contains(self.prompt.as_str())
    }

    /// One call over the project: the blocks applied where SDK-11 allows, then what keeps the
    /// files from building. A refused block goes back only beside a problem, so a stray path
    /// never costs a second call.
    pub(super) async fn code_step(
        &self,
        samples: &Value,
        files: &mut BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        fix: Option<Fix<'_>>,
    ) -> Result<(String, Vec<String>), String> {
        let first = fix.is_none() && self.opens(conversation, instruction);
        let user = self
            .code_pack(samples, files, conversation, instruction, fix)
            .await;
        let budget = if first {
            FIRST_VERSION_BUDGET
        } else {
            CODE_OUTPUT_BUDGET
        };
        let answer = self.complete_within(&code::SYSTEM, &user, budget).await?;
        let (blocks, prose, unread) = patch::parse(&answer);
        let (applied, refused) = patch::apply_where(files, &blocks, code::writable, code::REFUSAL);
        self.event(
            "tool",
            json!({
                "tool": "apply_patch",
                "command": format!("{} block(s)", blocks.len() + unread),
                // A block the model must send again is a repair, not a refusal: the step
                // stays a plain step (T-0785).
                "exitCode": if refused.is_empty() { 0 } else { 1 },
                "applied": applied,
                "refused": with_unread(&refused, unread),
            }),
        )
        .await?;
        let mut problems = code::problems(files);
        problems.extend(code::off_service_calls(
            crate::ops::runs::SERVICES_CATALOG,
            files,
            |service| self.service_on(service),
        ));
        // The prose already says what those blocks did, so a lost one is a repair, not a note.
        if unread > 0 {
            problems.push(unread_problem(unread));
        }
        if blocks.is_empty() && conversation.is_empty() {
            problems.push(format!("{NO_BLOCKS}; write the application as blocks"));
        }
        if !problems.is_empty() {
            problems.extend(
                refused
                    .iter()
                    .map(|refused| format!("{}: {}", refused.path, refused.reason)),
            );
        }
        Ok((prose, problems))
    }

    /// The user message of one code call (SDK-13): what every run of this Portal shares comes
    /// first, so the provider's prompt cache pays for it, then the endpoint's types and data,
    /// then the request.
    pub(super) async fn code_pack(
        &self,
        samples: &Value,
        files: &BTreeMap<String, String>,
        conversation: &[(String, String)],
        instruction: &str,
        fix: Option<Fix<'_>>,
    ) -> String {
        let mut pack = String::from("## THE SDK\n\n");
        pack.push_str(code::SDK_API.trim());
        pack.push_str("\n\nWhat `@joinedcontext/sdk` exports:\n```ts\n");
        pack.push_str(code::SDK_EXPORTS.trim());
        pack.push_str("\n```\n\n## THE FILES OF THE PROJECT\n\n");
        // The template's components the application has not changed go by their exports and
        // its unchanged tests by name (T-3076): they are used, not rewritten.
        let template = crate::agents::preview::template_files();
        let types = files.get_key_value(code::TYPES);
        let mut named = Vec::new();
        for (path, content) in files
            .iter()
            .filter(|(path, _)| path.as_str() != code::TYPES)
            .chain(types)
        {
            // The lock is the template's and never the model's: its name is enough.
            if path.ends_with("Cargo.lock") {
                named.push(path.as_str());
                continue;
            }
            let fence = path.rsplit('.').next().unwrap_or("text");
            match code::shown(path, content, &template) {
                code::Shown::Whole => {
                    pack.push_str(&format!("### {path}\n```{fence}\n{content}\n```\n"))
                }
                code::Shown::Outline(exports) => pack.push_str(&format!(
                    "### {path} (an unchanged template component, its exports: use it as it is)\n```{fence}\n{exports}\n```\n"
                )),
                code::Shown::Named => named.push(path.as_str()),
            }
        }
        if !named.is_empty() {
            pack.push_str(&format!(
                "Unchanged template tests and styles, not shown and left as they are: {}\n",
                named.join(", ")
            ));
        }
        pack.push_str(&code::services_section(
            crate::ops::runs::SERVICES_CATALOG,
            |service| self.service_on(service),
        ));
        if files.contains_key(code::SERVER_MANIFEST) {
            pack.push_str(code::SERVER_SECTION);
        }
        // The example of the request's kind, for writing and completing; a repair stays on its problems.
        if matches!(fix, None | Some(Fix::Complete)) {
            if let Some(sample) = crate::agents::samples::section(&self.prompt) {
                pack.push_str(&sample);
            }
        }
        pack.push_str(
            "\n## THE DATA\n\nData needs (types and attributes the person asked for):\n```json\n",
        );
        pack.push_str(&serde_json::to_string_pretty(&self.served_needs()).unwrap_or_default());
        pack.push_str("\n```\n\n");
        pack.push_str(&endpoints::pack_section(&self.endpoints, &self.data_needs));
        if self.allows_write {
            let schema = fields::for_endpoint(
                &self.state,
                &self.project,
                &self.endpoint_slug,
                &self.types(),
            )
            .await;
            pack.push_str(
                "The application MAY write: its data needs carry a write operation, so a form \
                 saves through the endpoint with the person's own access. The attributes per \
                 type as the space's DataModel declares them (JSON Schema properties):\n```json\n",
            );
            match schema {
                Some(schema) => {
                    pack.push_str(&serde_json::to_string_pretty(&schema).unwrap_or_default())
                }
                None => pack.push_str("{}"),
            }
            pack.push_str("\n```\n");
        } else {
            pack.push_str(
                "The application may NOT write: its data needs carry no write operation. Add no \
                 form and no save; when the request asks to edit, say that editing needs write \
                 access in the data needs.\n",
            );
        }
        pack.push_str(
            "\nFive entities per type, as the SDK's rows (`options=keyValues`):\n```json\n",
        );
        pack.push_str(&serde_json::to_string_pretty(samples).unwrap_or_default());
        pack.push_str("\n```\n");
        if let Some(joined) = self.joined.get().filter(|joined| !joined.is_empty()) {
            pack.push_str("\nTypes read from several endpoints:\n");
            pack.push_str(joined);
        }
        if !conversation.is_empty() {
            pack.push_str("\n## THE CONVERSATION SO FAR\n\n");
            for (asked, answered) in conversation {
                pack.push_str(&format!("Person: {asked}\nYou: {answered}\n\n"));
            }
        }
        let opening = fix.is_none() && self.opens(conversation, instruction);
        pack.push_str(&format!(
            "\n## THE REQUEST\n\n{}\n\n## THIS CALL\n\n",
            if opening { instruction } else { &self.prompt }
        ));
        match fix {
            Some(Fix::Build(errors)) => {
                pack.push_str(
                    "The last answer was applied and the project does not build. Fix every \
                     problem below, each named with its file and line, and answer with the blocks \
                     that fix them:\n",
                );
                for error in errors {
                    pack.push_str(&format!("- {error}\n"));
                }
                pack.push_str(&format!("\nThe request being fulfilled: {instruction}\n"));
            }
            Some(Fix::Preview(found)) => {
                pack.push_str(
                    "The application builds and is on screen. Checking its preview against the \
                     data found the problems below; the last item is what the preview rendered, \
                     page by page. Fix every problem at its cause, change nothing else, and \
                     answer with the blocks that fix them:\n",
                );
                for problem in found {
                    pack.push_str(&format!("- {problem}\n"));
                }
                pack.push_str(&format!("\nThe request being fulfilled: {instruction}\n"));
            }
            Some(Fix::Tests(failures)) => {
                pack.push_str(
                    "The application builds and is on screen. Its tests were run the way the \
                     build lane runs them before anything is published, and the ones below fail, \
                     each with its file, its name and its message. Fix each at its cause: the \
                     application where it does the wrong thing, the test where it asks for what \
                     the request never did or reads the page before its data has arrived (wait \
                     with `findBy…`, not `getBy…`). Change nothing else, keep every test that \
                     passes, and answer with the blocks that fix them:\n",
                );
                for failure in failures {
                    pack.push_str(&format!("- {failure}\n"));
                }
                pack.push_str(&format!("\nThe request being fulfilled: {instruction}\n"));
            }
            Some(Fix::Complete) => pack.push_str(
                "The first version of the application is on screen: the files above are it. \
                 Complete the application for the request: the other pages, filters, charts, \
                 maps, exports and functions the request and the data call for, and a test \
                 beside every page, component and function the application has. Keep the design \
                 and the page of the first version; change them only where completing needs it. \
                 Every list of entities you add is a `TypeGrid`, a narrowed one with its `q` prop, \
                 never a `<table>`. Answer with SEARCH/REPLACE blocks over the files above.\n",
            ),
            None if opening => pack.push_str(
                "Write the FIRST VERSION of the application for the request above, which goes on \
                 screen at once: `src/App.tsx`, the layout (`src/app.css`; \
                 `src/design-tokens.json` is already this application's look) and the one page the request is most about, complete and working \
                 with the real data, its entities in a `TypeGrid` under the map or charts with any \
                 worked-out value as a `formula` column. No tests, no other page and no function unless that page \
                 needs it: a second call adds them while the person already looks at this \
                 version. Keep the whole answer under 6,000 tokens. Begin your sentences with \
                 the application's name in bold, two to five words that say what it shows, for \
                 example **Helsinki Traffic Alerts Map**; never a file name or an id.\n",
            ),
            None => pack.push_str(&format!(
                "The person says: {instruction}\n\nChange the application accordingly, with tests \
                 for what you change.\n"
            )),
        }
        pack
    }

    /// `src/jc-types.ts` of the run (SDK-10): the LinkML the endpoint projects, read through the
    /// proxy like a sample, rendered by Model Tools.
    pub(super) async fn jc_types(&self) -> Result<String, String> {
        let index = match self.schema_index.get() {
            Some(index) => index.clone(),
            None => self.schema_index().await?,
        };
        let needed = self.types();
        let models = index
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let serves_needed = |model: &Value| {
            model["types"].as_array().is_some_and(|types| {
                types
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|t| needed.iter().any(|n| n == t))
            })
        };
        // ponytail: per endpoint, the first model publishing a needed type; an endpoint over two
        // models whose types the application both needs gets the other model's types as the
        // placeholder's.
        let mut chosen: Vec<&Value> = Vec::new();
        for at in 0..self.endpoints.len().max(1) {
            let of_endpoint =
                |model: &&Value| model["endpoint"].as_u64().unwrap_or(0) as usize == at;
            if let Some(model) = models
                .iter()
                .filter(of_endpoint)
                .filter(|model| model["unread"].as_bool() != Some(true))
                .find(|model| serves_needed(model))
            {
                chosen.push(model);
            }
        }
        if chosen.is_empty() {
            chosen.extend(models.first());
        }
        if chosen.is_empty() {
            return Err("the endpoint publishes no model".to_owned());
        }
        let mut rendered = Vec::new();
        for model in chosen {
            let major = model["version"]
                .as_u64()
                .ok_or("the endpoint's model has no version")?;
            let at = model["endpoint"].as_u64().unwrap_or(0) as usize;
            let types = async {
                let source = self
                    .read_text(&format!(
                        "{}/schema/v{major}/model.linkml.yaml",
                        self.data_base(at)
                    ))
                    .await?;
                crate::tools::model_tools::typescript(&self.state, &source).await
            }
            .await;
            match types {
                Ok(types) => rendered.push(types),
                // The primary's types are the file; another endpoint's that cannot be rendered
                // leaves its rows untyped rather than every row.
                Err(reason) if at == 0 => return Err(reason),
                Err(_) => {}
            }
        }
        if rendered.is_empty() {
            return Err("no model of the run's endpoints could be rendered".to_owned());
        }
        Ok(merge_declarations(&rendered))
    }

    /// Stores the files, says `prose`, commits what changed since `committed` when given, and
    /// points the frame at the new version (SDK-15, SDK-17).
    pub(super) async fn publish_code(
        &self,
        files: &BTreeMap<String, String>,
        prose: &str,
        committed: Option<&mut BTreeMap<String, String>>,
        first_version: bool,
    ) -> Result<Shown, String> {
        let stored: serde_json::Map<String, Value> = files
            .iter()
            .map(|(path, content)| (path.clone(), Value::String(content.clone())))
            .collect();
        self.state
            .agents
            .set_files(&self.run_id, Value::Object(stored))
            .await
            .map_err(|err| err.to_string())?;
        self.thought(prose).await?;
        if let Some(committed) = committed {
            self.commit_files(files, committed, prose).await?;
        }
        let pass = self.passes.fetch_add(1, Ordering::SeqCst) + 1;
        let url = format!(
            "/api/v1/projects/{}/agent-runs/{}/preview?v={pass}",
            self.project, self.run_id
        );
        self.state
            .agents
            .set_preview_url(&self.run_id, &url)
            .await
            .map_err(|err| err.to_string())?;
        if first_version {
            self.first_version().await;
        }
        let event = self.append("preview", json!({ "previewUrl": url })).await?;
        // The first version is the quick one, written without tests (SDK-13); every later one is
        // tested before the run offers publication (SDK-38).
        if !first_version {
            self.start_tests(files, pass).await?;
        }
        Ok(Shown {
            version: pass,
            seq: event.seq,
            observed: false,
        })
    }

    /// Starts one version's tests in the sandbox (SDK-38) and returns at once: the result comes
    /// back as a `tests` event, which the run's loop reads like any other. A version with no tests,
    /// or an installation with no sandbox, is a `skipped` event saying why.
    pub(super) async fn start_tests(
        &self,
        files: &BTreeMap<String, String>,
        version: u32,
    ) -> Result<(), String> {
        let skipped = match (self.state.app_tests.clone(), sandbox::has_tests(files)) {
            (_, false) => Some("this version has no tests"),
            (None, true) => Some(
                "this installation has no test sandbox; the build lane runs them at publication",
            ),
            (Some(sandbox), true) => {
                self.event("tests", json!({ "version": version, "outcome": "running" }))
                    .await?;
                let state = self.state.clone();
                let run_id = self.run_id.clone();
                let files = files.clone();
                let bearer = self.bearer.clone();
                tokio::spawn(async move {
                    let report = sandbox.run(&run_id, version, &files).await;
                    let mut payload = serde_json::to_value(&report).unwrap_or_else(|_| json!({}));
                    payload["version"] = json!(version);
                    // A test may print what the application was given; the ticket never lands.
                    let text = redact(&bearer, &payload.to_string());
                    let payload = serde_json::from_str(&text).unwrap_or(Value::Null);
                    match state.agents.append_event(&run_id, "tests", payload).await {
                        Ok(event) => state.agent_events.broadcast(&event).await,
                        Err(err) => {
                            tracing::warn!(run = %run_id, error = %err, "a test result was not recorded")
                        }
                    }
                });
                None
            }
        };
        if let Some(reason) = skipped {
            let mut payload = serde_json::to_value(sandbox::Report::skipped(reason))
                .map_err(|err| err.to_string())?;
            payload["version"] = json!(version);
            self.event("tests", payload).await?;
        }
        Ok(())
    }

    /// SDK-38: a `tests` event of the version on screen that failed goes back to the model; a
    /// result of an older version is past.
    ///
    /// Each red version gets its fix pass while the fixes move the failures (T-2997). The passes
    /// shared the three verification passes once, so a run stopped at its fifth version with its
    /// own tests red and steps left. What bounds them now is what the profile sets: the proxy
    /// refuses a call past `stepsPerRun` or `maxTokensPerRun`, and the run says which. The same
    /// failures after a fix are the one stop of its own: another pass would spend the budget on a
    /// fix the model has already shown it cannot make.
    pub(super) async fn tests_failed(
        &self,
        check: Check<'_>,
        files: &mut BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        on_screen: &Shown,
        payload: &Value,
        fixing: &mut Option<Vec<String>>,
    ) -> Result<Option<Shown>, String> {
        let version = payload.get("version").and_then(Value::as_u64);
        if version != Some(u64::from(on_screen.version)) || payload["outcome"] != "failed" {
            return Ok(None);
        }
        let failures: Vec<sandbox::Failure> =
            serde_json::from_value(payload["failures"].clone()).unwrap_or_default();
        let report = sandbox::Report {
            outcome: sandbox::Outcome::Failed,
            passed: 0,
            failed: u32::try_from(failures.len()).unwrap_or(u32::MAX),
            failures,
            duration_ms: None,
            reason: None,
        };
        let found = report.for_the_model();
        if found.is_empty() {
            return Ok(None);
        }
        let list = found
            .iter()
            .map(|failure| format!("- {failure}"))
            .collect::<Vec<_>>()
            .join("\n");
        if fixing.as_ref() == Some(&found) {
            self.thought(&format!(
                "The same tests fail after a fix pass, so the run does not offer publication and \
                 stops fixing them rather than spend its budget on the same fix; say what to \
                 change:\n{list}"
            ))
            .await?;
            return Ok(None);
        }
        *fixing = Some(found.clone());
        self.thought(&format!("The tests found:\n{list}")).await?;
        if check.edited {
            let mut conversation = check.conversation.to_vec();
            return match self
                .edit_turn(
                    files,
                    committed,
                    &mut conversation,
                    &tests_instruction(check.instruction, &found),
                    Some(on_screen),
                )
                .await
            {
                Ok(next) => Ok(next),
                Err(message) => {
                    self.thought(&format!("Fixing the tests failed: {message}"))
                        .await?;
                    Ok(None)
                }
            };
        }
        match self
            .code_pass(
                check.samples,
                files,
                check.conversation,
                check.instruction,
                Some(Fix::Tests(&found)),
                true,
            )
            .await
        {
            Ok(CodePass::Built { prose, .. }) => Ok(Some(
                self.publish_code(files, &prose, Some(committed), false)
                    .await?,
            )),
            Ok(CodePass::Unchanged(prose)) => {
                self.thought(&prose).await?;
                Ok(None)
            }
            Ok(CodePass::Failed) => Ok(None),
            Err(message) => {
                self.thought(&format!("Fixing the tests failed: {message}"))
                    .await?;
                Ok(None)
            }
        }
    }

    /// What changed since `committed`, as one commit on the run's branch (SDK-17, AP-24). A
    /// Portal without a forge keeps the run in its store alone; a forge that refuses is said in
    /// the chat and the version stands.
    ///
    /// A static application commits to its own repository (AP-75, AP-76): the first commit of a
    /// run creates the repository when it is not there yet, and makes the run's branch hold
    /// exactly the run's files and the README, so a branch cut from an earlier version keeps
    /// nothing the run no longer has.
    pub(super) async fn commit_files(
        &self,
        files: &BTreeMap<String, String>,
        committed: &mut BTreeMap<String, String>,
        message: &str,
    ) -> Result<(), String> {
        // A workspace run's folder is in the project's own repository in layout 2 (CC-87).
        let Some(gitea) = self.state.forge_for(&self.project) else {
            return Ok(());
        };
        if self.branch.is_empty() {
            return Ok(());
        }
        let repo = match &self.repository {
            Some(name) => gitea.for_application(name.clone()),
            None => (*gitea).clone(),
        };
        let first = committed.is_empty();
        let mut uploads: Vec<(String, String)> = files
            .iter()
            .filter(|(path, content)| committed.get(*path) != Some(*content))
            .map(|(path, content)| (format!("{}{path}", self.path_prefix), content.clone()))
            .collect();
        let gone: Vec<String> = committed
            .keys()
            .filter(|path| !files.contains_key(*path))
            .map(|path| format!("{}{path}", self.path_prefix))
            .collect();
        if uploads.is_empty() && gone.is_empty() {
            return Ok(());
        }
        let own = self.repository.is_some();
        if own {
            if let Some(refusal) = repository::build_refusal(files, committed) {
                return self
                    .thought(&format!("The commit did not land in the forge: {refusal}"))
                    .await;
            }
        }
        let readme = if own && first && !files.contains_key(repository::README) {
            let title = match self.state.agents.get_run(&self.run_id).await {
                Ok(Some(run)) => run.title,
                _ => None,
            };
            Some(repository::readme(
                &self.project,
                &self.app_name,
                title.as_deref(),
                &repo.clone_url(),
            ))
        } else {
            None
        };
        let outcome: Result<String, GitError> = async {
            if own {
                repo.ensure_repository(&format!(
                    "Application {} of project {}, generated in the joinedcontext Portal",
                    self.app_name, self.project
                ))
                .await?;
            }
            if repo.branch_head(&self.branch).await.is_err() {
                let base = repo.default_branch().await?;
                repo.create_branch(&self.branch, &base).await?;
            }
            let mut deletes = Vec::new();
            if own && first {
                for (path, sha) in repo.list_tree_blobs(&self.branch).await? {
                    let kept = files.contains_key(&path)
                        || (readme.is_some() && path == repository::README);
                    if !kept {
                        deletes.push((path, sha));
                    }
                }
            } else {
                for path in gone {
                    if let Some(file) = repo.get_file(&path, &self.branch).await? {
                        deletes.push((path, file.sha));
                    }
                }
            }
            if let Some(readme) = readme {
                uploads.push((repository::README.to_owned(), readme));
            }
            repo.change_files(
                &self.branch,
                &commit_message(message),
                Author {
                    name: &self.created_by,
                    email: "agent@joinedcontext.local",
                },
                &uploads,
                &deletes,
            )
            .await
        }
        .await;
        match outcome {
            Ok(sha) => {
                *committed = files.clone();
                self.event(
                    "commit",
                    json!({ "sha": sha, "message": commit_subject(message) }),
                )
                .await?;
                if own {
                    self.mirror_to_github(&repo).await;
                }
                Ok(())
            }
            Err(err) => {
                self.thought(&format!("The commit did not land in the forge: {err}"))
                    .await
            }
        }
    }

    /// Keeps the application's GitHub copy in place, when the installation keeps one (AP-79).
    ///
    /// After the commit, so the forge repository exists and the first push carries this pass. A
    /// copy that cannot be set up is said on the run and costs the pass nothing: the forge holds
    /// the commit either way, and the next pass tries again.
    async fn mirror_to_github(&self, repo: &crate::git::GiteaClient) {
        let Some(mirror) = self.state.github_mirror.as_deref() else {
            return;
        };
        let description = format!(
            "Application {} of project {}, generated in the joinedcontext Portal",
            self.app_name, self.project
        );
        let said = match mirror.mirror(repo, &description).await {
            Ok(crate::git::github_mirror::Mirrored::Current) => return,
            Ok(crate::git::github_mirror::Mirrored::Added) => {
                "The application's repository is now copied to GitHub on every commit.".to_owned()
            }
            Ok(crate::git::github_mirror::Mirrored::Repaired(failed)) => format!(
                "The copy on GitHub had stopped following the forge ({failed}); it was set up again."
            ),
            Err(err) => {
                tracing::warn!(run = %self.run_id, repository = %repo.repo, error = %err, "GitHub copy not set up");
                format!(
                    "The copy on GitHub could not be set up: {err}. The forge holds this version; \
                     the next change tries again."
                )
            }
        };
        if let Err(err) = self.thought(&said).await {
            tracing::warn!(run = %self.run_id, error = %err, "the GitHub copy's sentence was not recorded");
        }
    }

    /// The run's first version and the timings it closes (AG-66).
    pub(super) async fn first_version(&self) {
        if let Err(err) = self.state.agents.record_first_version(&self.run_id).await {
            tracing::warn!(run = %self.run_id, error = %err, "first version not recorded");
        }
        if let Ok(Some(r)) = self.state.agents.get_run(&self.run_id).await {
            if let Some(ms) = r.first_frame_ms {
                crate::telemetry::record_run_timing("first_frame", &r.profile, ms);
            }
            if let Some(ms) = r.first_version_ms {
                crate::telemetry::record_run_timing("first_version", &r.profile, ms);
            }
        }
    }
}

/// The editing agent's instruction when the version's tests fail (SDK-38).
fn tests_instruction(request: &str, failures: &[String]) -> String {
    let list = failures
        .iter()
        .map(|failure| format!("- {failure}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "The last change was: {request}\nThe application's tests, run the way the build lane \
         runs them, fail as below. Fix each at its cause, the application or the test (a test \
         that reads the page before its data arrived waits with findBy…), and change nothing \
         else:\n{list}"
    )
}

/// The editing agent's instruction for a verification: the person's request and what the check
/// of its version found.
fn repair_instruction(request: &str, found: &[String]) -> String {
    let list = found
        .iter()
        .map(|problem| format!("- {problem}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "The last change was: {request}\nChecking the preview after it found these problems. \
         Fix them in the files that cause them and change nothing else:\n{list}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_completing_call_keeps_a_narrowed_list_in_a_type_grid() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state, "helsinki");
        let pack = driver
            .code_pack(
                &json!({}),
                &BTreeMap::new(),
                &[],
                "A list",
                Some(Fix::Complete),
            )
            .await;
        assert!(pack.contains("a narrowed one with its `q` prop"), "{pack}");
    }

    /// T-3076: the first code call over the template carries the SDK, the components by their
    /// exports and the application's own files: within 26.5k tokens (~25.8k measured since the
    /// SDK's platform services, T-3583), where the whole template alone was ~50k. It is one call
    /// a run; the per-step base is the editing opening (edit_loop, within 15k).
    #[tokio::test]
    async fn the_first_code_call_over_the_template_stays_within_26_5k_tokens() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.prompt = "A map and a table of the city bike stations".into();
        let template = crate::agents::preview::template_files();
        let samples = json!({ "BikeHireDockingStation": [
            { "id": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:001", "name": "Kaivopuisto",
              "availableBikeNumber": 7, "location": { "type": "Point", "coordinates": [24.95, 60.16] } }
        ] });
        let prompt = driver.prompt.clone();
        let pack = driver
            .code_pack(&samples, &template, &[], &prompt, None)
            .await;
        let tokens = (code::SYSTEM.len() + pack.len()) / 4;
        assert!(tokens <= 26_500, "{tokens} tokens");
        assert!(
            pack.contains("export function EntityMap"),
            "the components are named by their exports"
        );
        assert!(
            !pack.contains("### src/components/EntityForm.test.tsx"),
            "no template test is carried"
        );
        assert!(pack.contains("Unchanged template tests and styles"));
    }

    /// T-3623: the builder offers the services the App's layers have on and never one a layer
    /// keeps off: `ai` is on the App and off at the organization, so the pack names no call of it.
    #[tokio::test]
    async fn the_pack_offers_no_service_a_layer_keeps_off() {
        let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| {
            crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            }
        };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(envelope(
            "App",
            "test",
            "helsinki",
            json!({ "kind": "ui", "source": { "path": "." }, "build": {}, "visibility": "project",
                    "lifecycle": "draft", "dataNeeds": [], "services": ["email", "ai"] }),
        ));
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.prompt = "Mail me the worst defects, rated".into();
        let template = crate::agents::preview::template_files();
        let prompt = driver.prompt.clone();
        let org = |services: Value| {
            envelope(
                "Organization",
                "helsinki",
                "org",
                json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                        "policies": { "apps": { "services": services } } }),
            )
        };

        driver
            .state
            .mirror
            .upsert(org(json!(["email", "files", "jobs"])));
        let pack = driver
            .code_pack(&json!({}), &template, &[], &prompt, None)
            .await;
        assert!(
            pack.contains("- email: calls jc().email.send"),
            "on at every layer"
        );
        // The SDK's API.md, also in the pack, names every call; the services section offers them.
        assert!(!pack.contains("- ai: calls"), "off at the organization");
        assert!(!pack.contains("- files: calls"), "off at the App");

        driver
            .state
            .mirror
            .upsert(org(json!(["email", "files", "jobs", "ai"])));
        let pack = driver
            .code_pack(&json!({}), &template, &[], &prompt, None)
            .await;
        assert!(
            pack.contains("- ai: calls jc().ai.complete"),
            "on once the organization allows it"
        );
    }

    /// T-3623, recorded answers: the model writes a call of `ai`, which the organization keeps
    /// off; the call goes back to it as a problem of its file, and the version that is built has
    /// no call of it.
    #[tokio::test]
    async fn generated_code_that_calls_a_service_a_layer_keeps_off_is_sent_back() {
        let server = wiremock::MockServer::start().await;
        let answer = |text: &str| {
            wiremock::ResponseTemplate::new(200)
                .set_body_json(json!({ "content": [{ "type": "text", "text": text }] }))
        };
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(answer(
                "A rating.\nsrc/Rate.ts\n<<<<<<< SEARCH\n=======\nexport const rate = (c: { ai: { complete(x: object): unknown } }) => c.ai.complete({});\n>>>>>>> REPLACE\n",
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(answer(
                "Without the model.\nsrc/Rate.ts\n<<<<<<< SEARCH\nexport const rate = (c: { ai: { complete(x: object): unknown } }) => c.ai.complete({});\n=======\nexport const rate = (severity: number) => Math.min(5, Math.max(1, severity));\n>>>>>>> REPLACE\n",
            ))
            .mount(&server)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| {
            crate::resource::ResourceEnvelope {
                api_version: crate::resource::API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: crate::resource::ObjectMeta::new(name, namespace),
                spec,
                status: None,
            }
        };
        state.mirror.upsert(envelope(
            "App",
            "test",
            "helsinki",
            json!({ "kind": "ui", "source": { "path": "." }, "build": {}, "visibility": "project",
                    "lifecycle": "draft", "dataNeeds": [], "services": ["ai"] }),
        ));
        state.mirror.upsert(envelope(
            "Organization",
            "helsinki",
            "org",
            json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                    "policies": { "apps": { "services": ["email"] } } }),
        ));
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.prompt = "Rate each defect".into();
        let mut files = crate::agents::preview::template_files();
        let prompt = driver.prompt.clone();
        let pass = driver
            .code_pass(&json!({}), &mut files, &[], &prompt, None, false)
            .await
            .expect("the pass ends");
        assert!(matches!(pass, CodePass::Built { .. }));
        assert!(
            files
                .values()
                .all(|content| !content.contains(".ai.complete(")),
            "the built version calls no ai"
        );
        let asked = server.received_requests().await.unwrap_or_default();
        assert_eq!(asked.len(), 2, "one call and its repair");
        let repair = String::from_utf8_lossy(&asked[1].body);
        assert!(
            repair.contains("src/Rate.ts: calls the ai service"),
            "{repair}"
        );
    }

    /// T-6457: a run framed by a note on the look (or as a dashboard) still opens with the first
    /// version: 20,000 tokens at low effort, its frame kept in the request. It went at the
    /// profile's effort as one 90 to 140 s call once the project held a twin.
    #[tokio::test]
    async fn a_framed_opening_is_still_the_first_version() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "Nothing to change." }, "finish_reason": "stop" }],
            })))
            .mount(&server)
            .await;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.provider = "openrouter".into();
        driver.reasoning_effort = Some("medium".into());
        driver.prompt = "Map the bikes".into();
        let framed = "Map the bikes\n\nNOTE ON THE LOOK: lay this App out as `dashboard-grid`.";
        let mut files = crate::agents::preview::template_files();
        driver
            .code_step(&json!({}), &mut files, &[], framed, None)
            .await
            .expect("the step ends");
        let asked = server.received_requests().await.unwrap_or_default();
        let body: Value = serde_json::from_slice(&asked[0].body).expect("a JSON body");
        assert_eq!(body["max_tokens"], json!(FIRST_VERSION_BUDGET));
        assert_eq!(body["reasoning"]["effort"], "low");
        let text = body.to_string();
        assert!(text.contains("FIRST VERSION"), "{text}");
        assert!(text.contains("NOTE ON THE LOOK"), "{text}");
    }

    /// What the one model call a verification makes looks like, for a version of the first run
    /// or of an instruction.
    async fn first_call_of_a_verification(edited: bool) -> Value {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": "Fixed." }],
            })))
            .mount(&server)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.kind = "application".into();
        driver.prompt = "A table of the stations".into();
        driver
            .state
            .agents
            .append_event(
                &driver.run_id,
                "preview_error",
                json!({ "message": "x is undefined" }),
            )
            .await
            .expect("event");
        let conversation = vec![("A table of the stations".to_owned(), "Ready.".to_owned())];
        let check = Check {
            samples: &json!({}),
            conversation: &conversation,
            instruction: "add a form for a new station",
            edited,
        };
        let on_screen = Shown {
            version: 2,
            seq: 0,
            observed: true,
        };
        let mut files = preview::template_files();
        driver
            .verify(
                check,
                &mut files,
                &mut BTreeMap::new(),
                &on_screen,
                None,
                &mut 0,
            )
            .await
            .expect("the verification ends");
        let requests = server.received_requests().await.expect("recorded");
        serde_json::from_slice(&requests[0].body).expect("a JSON body")
    }

    /// T-2466: once an instruction produced the version on screen, what the check finds is fixed
    /// by the editing agent's small tool calls, never by writing the whole project again.
    #[tokio::test]
    async fn a_verification_after_an_instruction_goes_to_the_editing_agent() {
        let body = first_call_of_a_verification(true).await;
        assert!(body["tools"].is_array(), "{body}");
        let system = body["system"].as_str().unwrap_or_default();
        assert!(
            system.starts_with("You edit a small web application"),
            "{system}"
        );
        let asked = body["messages"].to_string();
        assert!(asked.contains("x is undefined"), "{asked}");
        assert!(asked.contains("add a form for a new station"), "{asked}");
    }

    /// T-2466: the first run's own version is still repaired by a pass over the project.
    #[tokio::test]
    async fn a_verification_of_the_first_run_stays_a_pass_over_the_project() {
        let body = first_call_of_a_verification(false).await;
        assert!(body.get("tools").is_none(), "{body}");
    }
}

#[cfg(test)]
mod repository_tests {
    use std::sync::Arc;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::git::GiteaClient;

    const REPO: &str = "/api/v1/repos/joinedcontext/helsinki_city-bikes";
    const BRANCH: &str = "agent/app-city-bikes/run-1";

    fn driver(server: &MockServer) -> (AppState, Driver) {
        let gitea = GiteaClient::new(
            server.uri().parse().expect("a url"),
            "joinedcontext",
            "configuration",
            "t",
        )
        .expect("a client");
        let state =
            AppState::new(crate::config::Config::for_tests(), None).with_gitea(Arc::new(gitea));
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.repository = Some("helsinki_city-bikes".into());
        driver.app_name = "city-bikes".into();
        driver.path_prefix = String::new();
        driver.branch = BRANCH.into();
        (state, driver)
    }

    fn files() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("package.json".to_owned(), "{}".to_owned()),
            ("src/App.tsx".to_owned(), "export default 1".to_owned()),
        ])
    }

    /// The one `POST /contents` commit the forge received: `(operation, path, sha)` per file.
    async fn commit_of(server: &MockServer) -> Vec<(String, String, Option<String>)> {
        let commits: Vec<Value> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| {
                request.method.as_str() == "POST"
                    && request.url.path() == format!("{REPO}/contents")
            })
            .map(|request| serde_json::from_slice(&request.body).expect("json"))
            .collect();
        assert_eq!(commits.len(), 1, "one commit per pass");
        let mut files: Vec<_> = commits[0]["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|file| {
                (
                    file["operation"].as_str().unwrap_or_default().to_owned(),
                    file["path"].as_str().unwrap_or_default().to_owned(),
                    file["sha"].as_str().map(str::to_owned),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// AP-75, AP-76: the first commit of a run creates the application's repository, cuts the
    /// run's branch from `main`, and leaves the branch holding exactly the run's files and the
    /// README: a file of an earlier version is deleted, nothing lands in the configuration
    /// repository.
    #[tokio::test]
    async fn the_first_commit_creates_the_repository_and_the_branch_holds_the_whole_application() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/orgs/joinedcontext/repos"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/{BRANCH}")))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/{BRANCH}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "head1" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/branches")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/git/trees/head1")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "truncated": false,
                "tree": [
                    { "path": "README.md", "type": "blob", "sha": "r1" },
                    { "path": "src/App.tsx", "type": "blob", "sha": "a1" },
                    { "path": "src/Old.tsx", "type": "blob", "sha": "o1" },
                    { "path": "src", "type": "tree", "sha": "t1" }
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/contents")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
            )
            .mount(&server)
            .await;

        let (state, driver) = driver(&server);
        let mut committed = BTreeMap::new();
        driver
            .commit_files(&files(), &mut committed, "First version")
            .await
            .expect("committed");

        assert_eq!(
            commit_of(&server).await,
            vec![
                ("delete".into(), "src/Old.tsx".into(), Some("o1".into())),
                ("upload".into(), "README.md".into(), None),
                ("upload".into(), "package.json".into(), None),
                ("upload".into(), "src/App.tsx".into(), None),
            ]
        );
        assert_eq!(
            committed,
            files(),
            "the next pass commits what changed since this one"
        );
        let requests = server.received_requests().await.unwrap_or_default();
        assert!(
            requests
                .iter()
                .all(|request| !request.url.path().contains("/configuration")),
            "the configuration repository was written to"
        );
        let events = state
            .agents
            .events_since("test-run", 0)
            .await
            .expect("events");
        assert!(
            events
                .iter()
                .any(|event| event.kind == "commit" && event.payload["sha"] == "c1"),
            "the run links its commit: {events:?}"
        );
    }

    /// AP-75, AP-76: a later pass in a repository that exists creates nothing and changes only
    /// what changed; a model that wrote its own README keeps it.
    #[tokio::test]
    async fn a_later_pass_commits_only_what_changed_and_never_recreates_the_repository() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/orgs/joinedcontext/repos"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/{BRANCH}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "head2" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/src/Gone.tsx")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "sha": "g1", "content": "" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/contents")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c2" } })),
            )
            .mount(&server)
            .await;

        let (_, driver) = driver(&server);
        let mut committed = files();
        committed.insert("src/Gone.tsx".into(), "x".into());
        let mut next = files();
        next.insert("src/App.tsx".into(), "export default 2".into());
        next.insert("README.md".into(), "# mine".into());
        driver
            .commit_files(&next, &mut committed, "Second version")
            .await
            .expect("committed");

        assert_eq!(
            commit_of(&server).await,
            vec![
                ("delete".into(), "src/Gone.tsx".into(), Some("g1".into())),
                ("upload".into(), "README.md".into(), None),
                ("upload".into(), "src/App.tsx".into(), None),
            ]
        );
        let requests = server.received_requests().await.unwrap_or_default();
        assert!(
            requests
                .iter()
                .all(|request| !request.url.path().contains("/git/trees")),
            "a later pass read the whole tree"
        );
    }

    /// AP-100: a pass that would add, change or delete anything under `.gitea/` reaches the
    /// forge not at all, and the run is told why, so only a workflow a reviewer merged runs.
    #[tokio::test]
    async fn a_pass_that_would_change_the_build_commits_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/contents")))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;

        let (state, driver) = driver(&server);
        let mut committed = files();
        committed.insert(
            repository::WORKFLOW.into(),
            repository::WORKFLOW_TEXT.into(),
        );
        let mut next = committed.clone();
        next.insert(
            repository::WORKFLOW.into(),
            "on: [push]\njobs: { x: { runs-on: node-22, steps: [ { run: env } ] } }\n".into(),
        );
        next.insert("src/App.tsx".into(), "export default 2".into());
        let before = committed.clone();
        driver
            .commit_files(&next, &mut committed, "Second version")
            .await
            .expect("the pass goes on");

        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "the forge was asked for something"
        );
        assert_eq!(committed, before, "nothing counts as committed");
        let events = state
            .agents
            .events_since("test-run", 0)
            .await
            .expect("events");
        assert!(
            events
                .iter()
                .any(|event| event.payload.to_string().contains("AP-100")),
            "the run says why: {events:?}"
        );
    }

    /// The forge half of a later pass, for the cases that only differ in what GitHub answers.
    async fn a_forge_for_a_later_pass() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(REPO))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/{BRANCH}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "head2" } })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/contents")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c2" } })),
            )
            .mount(&server)
            .await;
        server
    }

    async fn thoughts(state: &AppState) -> Vec<String> {
        state
            .agents
            .events_since("test-run", 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "thought")
            .map(|event| {
                event.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    /// AP-79: after the commit, the application's repository gets its GitHub copy, pushed at once,
    /// and the run says so.
    #[tokio::test]
    async fn a_pass_sets_up_the_github_copy_after_its_commit_and_says_so() {
        let forge = a_forge_for_a_later_pass().await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/push_mirrors")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&forge)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/push_mirrors")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&forge)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("{REPO}/push_mirrors-sync")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&forge)
            .await;
        let github = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_city-bikes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&github)
            .await;

        let (state, mut driver) = driver(&forge);
        let state = state.with_github_mirror(Arc::new(
            crate::git::github_mirror::GithubMirror::at(&github.uri(), "hel-apps", "ghp_secret"),
        ));
        driver.state = state.clone();
        let mut committed = files();
        let mut next = files();
        next.insert("src/App.tsx".into(), "export default 2".into());
        driver
            .commit_files(&next, &mut committed, "Second version")
            .await
            .expect("committed");

        let said = thoughts(&state).await;
        assert!(
            said.iter().any(|text| text.contains("copied to GitHub")),
            "{said:?}"
        );
    }

    /// AP-79: GitHub refusing costs the pass nothing. The commit stands, the run says why in words
    /// a person can act on, and the token appears nowhere in what it says.
    #[tokio::test]
    async fn a_github_refusal_is_said_on_the_run_and_the_commit_stands() {
        let forge = a_forge_for_a_later_pass().await;
        let github = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/hel-apps/helsinki_city-bikes"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({ "message": "Bad credentials" })),
            )
            .mount(&github)
            .await;

        let (state, mut driver) = driver(&forge);
        let state = state.with_github_mirror(Arc::new(
            crate::git::github_mirror::GithubMirror::at(&github.uri(), "hel-apps", "ghp_secret"),
        ));
        driver.state = state.clone();
        let mut committed = files();
        let mut next = files();
        next.insert("src/App.tsx".into(), "export default 2".into());
        driver
            .commit_files(&next, &mut committed, "Second version")
            .await
            .expect("the pass does not fail on GitHub");

        assert_eq!(committed, next, "the commit stands");
        let said = thoughts(&state).await;
        let refusal = said
            .iter()
            .find(|text| text.contains("could not be set up"))
            .unwrap_or_else(|| panic!("the run says nothing about GitHub: {said:?}"));
        assert!(refusal.contains("Bad credentials"), "{refusal}");
        assert!(
            said.iter().all(|text| !text.contains("ghp_secret")),
            "{said:?}"
        );
    }
}

#[cfg(test)]
mod sandbox_tests {
    use std::sync::Arc;

    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::agents::sandbox::{Sandbox, SandboxSettings};
    use crate::apps::kube::KubeClient;

    fn tested() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("src/App.tsx".to_owned(), "export default 1".to_owned()),
            (
                "src/App.test.tsx".to_owned(),
                "test('x', () => {})".to_owned(),
            ),
        ])
    }

    async fn tests_events(state: &AppState) -> Vec<Value> {
        state
            .agents
            .events_since("test-run", 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "tests")
            .map(|event| event.payload)
            .collect()
    }

    /// SDK-38: without a sandbox, or without a test, the version says why its tests did not run.
    #[tokio::test]
    async fn a_version_is_skipped_with_its_reason_when_nothing_can_run_its_tests() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state.clone(), "helsinki");
        driver.start_tests(&tested(), 2).await.expect("started");
        let untested = BTreeMap::from([("src/App.tsx".to_owned(), "1".to_owned())]);
        driver.start_tests(&untested, 3).await.expect("started");

        let events = tests_events(&state).await;
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0]["version"], 2);
        assert_eq!(events[0]["outcome"], "skipped");
        assert!(events[0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("no test sandbox")));
        assert_eq!(events[1]["version"], 3);
        assert!(events[1]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("no tests")));
    }

    /// SDK-38: the run says the tests are running at once, and the result follows as its own
    /// event of the same version, without the ticket in it.
    #[tokio::test]
    async fn the_result_follows_the_running_event_and_never_carries_the_ticket() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("/jobs/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": { "conditions": [{ "type": "Failed", "status": "True" }] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("/pods$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "items": [{ "metadata": { "name": "p" } }] })),
            )
            .mount(&server)
            .await;
        let log = format!(
            "JC-TESTS {}",
            json!({"outcome": "failed", "passed": 0, "failed": 1, "failures": [
                {"file": "src/App.test.tsx", "name": "reads", "message": "token s3cr3t-ticket leaked"}
            ]})
        );
        Mock::given(method("GET"))
            .and(path_regex("/pods/p/log$"))
            .respond_with(ResponseTemplate::new(200).set_body_string(log))
            .mount(&server)
            .await;

        let settings = SandboxSettings {
            namespace: "jc-app-tests".into(),
            image: format!("builder@sha256:{}", "a".repeat(64)),
            rust_image: None,
        };
        let kube = KubeClient::with_token(&server.uri(), "t").expect("a client");
        let state = AppState::new(crate::config::Config::for_tests(), None)
            .with_app_tests(Arc::new(Sandbox::new(kube, settings)));
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.bearer = "run.s3cr3t-ticket".into();
        driver.start_tests(&tested(), 4).await.expect("started");

        let mut events = tests_events(&state).await;
        assert_eq!(events[0], json!({ "version": 4, "outcome": "running" }));
        for _ in 0..100 {
            if events.len() > 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            events = tests_events(&state).await;
        }
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[1]["version"], 4);
        assert_eq!(events[1]["outcome"], "failed");
        assert!(
            !events[1].to_string().contains("s3cr3t-ticket"),
            "{}",
            events[1]
        );
    }
}

#[cfg(test)]
mod tests_repair {
    use super::*;

    /// A driver whose model answers every call with `Fixed.` and the run's recorded thoughts.
    async fn driver() -> (wiremock::MockServer, Driver) {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/llm/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": "Fixed." }],
            })))
            .mount(&server)
            .await;
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut driver = Driver::for_tests(state, "helsinki");
        driver.proxy_base = server.uri();
        driver.kind = "application".into();
        driver.prompt = "A desk of today's alerts".into();
        (server, driver)
    }

    fn failed(version: u32) -> Value {
        json!({
            "version": version,
            "outcome": "failed",
            "passed": 2,
            "failed": 1,
            "failures": [{
                "file": "src/pages/AlertDesk.test.tsx",
                "name": "AlertDesk lists the alerts",
                "message": "Unable to find role row: the query ran before the alerts loaded"
            }]
        })
    }

    async fn run_tests_failed(
        driver: &Driver,
        edited: bool,
        payload: &Value,
        fixing: &mut Option<Vec<String>>,
    ) {
        let conversation = vec![("A desk of today's alerts".to_owned(), "Ready.".to_owned())];
        let check = Check {
            samples: &json!({}),
            conversation: &conversation,
            instruction: "add a filter by district",
            edited,
        };
        let on_screen = Shown {
            version: 3,
            seq: 0,
            observed: true,
        };
        let mut files = preview::template_files();
        driver
            .tests_failed(
                check,
                &mut files,
                &mut BTreeMap::new(),
                &on_screen,
                payload,
                fixing,
            )
            .await
            .expect("the repair ends");
    }

    async fn asked(server: &wiremock::MockServer) -> Vec<Value> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
            .collect()
    }

    async fn thoughts(driver: &Driver) -> Vec<String> {
        driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "thought")
            .filter_map(|event| event.payload["text"].as_str().map(str::to_owned))
            .collect()
    }

    /// T-2704, SDK-38: a failing test goes back to the model with its file, name and message, as a
    /// pass over the project for a version the first run wrote.
    #[tokio::test]
    async fn a_failing_test_goes_to_the_model_with_its_log() {
        let (server, driver) = driver().await;
        let mut fixing = None;
        run_tests_failed(&driver, false, &failed(3), &mut fixing).await;
        assert!(
            fixing.is_some(),
            "the failures the pass was made for are kept"
        );
        let calls = asked(&server).await;
        assert_eq!(calls.len(), 1, "one repair pass");
        let sent = calls[0].to_string();
        assert!(sent.contains("AlertDesk lists the alerts"), "{sent}");
        assert!(
            sent.contains("the query ran before the alerts loaded"),
            "{sent}"
        );
        assert!(sent.contains("findBy"), "the likely cause is named: {sent}");
        assert!(thoughts(&driver)
            .await
            .iter()
            .any(|t| t.starts_with("The tests found:")));
    }

    /// After an instruction the editing agent repairs it, told what the tests found.
    #[tokio::test]
    async fn after_an_instruction_the_editing_agent_gets_the_failures() {
        let (server, driver) = driver().await;
        run_tests_failed(&driver, true, &failed(3), &mut None).await;
        let calls = asked(&server).await;
        assert!(
            calls[0]["tools"].is_array(),
            "the editing agent: {}",
            calls[0]
        );
        let sent = calls[0]["messages"].to_string();
        assert!(sent.contains("add a filter by district"), "{sent}");
        assert!(sent.contains("AlertDesk lists the alerts"), "{sent}");
    }

    /// T-2997: a red version gets its fix pass however many came before, as long as the last fix
    /// moved the failures. Runs on dev stopped at their fifth version with its tests red: the
    /// fixes spent the three verification passes and no pass was left.
    #[tokio::test]
    async fn a_red_version_gets_its_fix_pass_while_the_failures_move() {
        let (server, driver) = driver().await;
        // Passes came before, and the last one was made for another failure than this one.
        let mut fixing = Some(vec!["an earlier failure the last pass fixed".to_owned()]);
        run_tests_failed(&driver, false, &failed(3), &mut fixing).await;
        assert_eq!(
            asked(&server).await.len(),
            1,
            "one fix pass for the red version"
        );
        assert!(thoughts(&driver)
            .await
            .iter()
            .any(|t| t.starts_with("The tests found:")));
    }

    /// The same failures after a fix pass end with no model call and a reason the person can act
    /// on; the publish gate refuses the version while its last result failed (`tests_hold`).
    #[tokio::test]
    async fn the_same_failures_after_a_fix_stop_and_say_why() {
        let (server, driver) = driver().await;
        let mut fixing = None;
        run_tests_failed(&driver, false, &failed(3), &mut fixing).await;
        run_tests_failed(&driver, false, &failed(3), &mut fixing).await;
        assert_eq!(
            asked(&server).await.len(),
            1,
            "no second pass for the same failures"
        );
        let said = thoughts(&driver).await;
        assert!(
            said.iter()
                .any(|t| t.contains("The same tests fail after a fix pass")
                    && t.contains("does not offer publication")
                    && t.contains("AlertDesk lists the alerts")),
            "{said:?}"
        );
    }

    /// A result of an older version, a passing one or a sandbox error repairs nothing.
    #[tokio::test]
    async fn only_a_failure_of_the_version_on_screen_is_repaired() {
        let (server, driver) = driver().await;
        let mut fixing = None;
        for payload in [
            failed(2),
            json!({ "version": 3, "outcome": "passed", "passed": 3, "failed": 0 }),
            json!({ "version": 3, "outcome": "error", "reason": "the tests did not finish" }),
            json!({ "version": 3, "outcome": "running" }),
        ] {
            run_tests_failed(&driver, false, &payload, &mut fixing).await;
        }
        assert!(fixing.is_none());
        assert!(asked(&server).await.is_empty());
    }
}
