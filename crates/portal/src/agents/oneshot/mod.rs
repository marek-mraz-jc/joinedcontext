//! The runs the Portal drives itself (Architecture/19 §1.2, AP-56…AP-60, AG-53, AG-54): the
//! assistant's conversation, and every run that builds, which writes code on the App SDK (an
//! application, a dashboard and an analysis, ADR-N-022). A `static` run has no workspace: the
//! Portal reads a few entities per type through the proxy and drives the model itself. The proxy
//! is still the one holding the model key and the endpoint: the driver speaks to it with the
//! run's ticket, like a workspace would. The spec.json kit is retired (T-0681).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, OnceLock, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::agents::access::Access;
use crate::agents::code;
use crate::agents::endpoints;
use crate::agents::entity_write;
use crate::agents::patch;
use crate::agents::preview;
use crate::agents::profile::Profile;
use crate::agents::repository;
use crate::agents::run::{AgentRun, AgentRunEvent, AgentRunStatus};
use crate::agents::store::now_rfc3339;
use crate::agents::{
    change, data_query, fields, grant, kpi, kpi_pipeline, model_change, share, verification,
};
use crate::auth::session::Identity;
use crate::git::gitea::{Author, GitError};
use crate::state::AppState;
use jc_core::kinds::Verb;

mod build;
mod code_pass;
mod connector_turn;
mod conversation;
mod edit_loop;
mod integrate;
mod model;
mod run_now;
mod tools_change;
mod tools_data;
mod tools_registry;
mod tools_space;
pub(crate) mod ui_actions;

/// Entities read per type as the model's sample of the data (AP-57).
const SAMPLES_PER_TYPE: u32 = 5;
/// Output tokens one conversation turn may spend: prose or one tool call, well under this.
const OUTPUT_BUDGET: u32 = 24000;
/// Output tokens one call of a code run may spend: a whole application with its tests is tens
/// of thousands, and a cut answer is refused whole all the same.
const CODE_OUTPUT_BUDGET: u32 = 64000;

/// What the conversation records as the instruction of the completing call.
const COMPLETE_TURN: &str = "Complete the application.";

/// The output ceiling of a code run's first version (SDK-13): the answer is asked to stay near
/// 6,000 tokens, which a model writes in about 35 seconds; the ceiling leaves room for its
/// reasoning so a slightly longer answer is not cut.
const FIRST_VERSION_BUDGET: u32 = 20000;
/// The reasoning effort of that call (AG-72): a person watches it, and medium effort took 42 to 90 s
/// over the bike prompt against 20 to 29 s at low, with the same components chosen (T-6403). The
/// second call, which completes the application, keeps the profile's.
const FIRST_VERSION_EFFORT: &str = "low";
/// One model call, wall clock: the budget above at a hundred tokens a second, with room.
const CALL_TIMEOUT: Duration = Duration::from_secs(480);
/// One model call of a conversation, wall clock. A person waits in front of it: the bikes
/// question sat 55 s after its catalog search with nothing on screen, because a chat turn shared
/// the builder's eight minutes per call (T-2462). Past this the turn says it failed, at once,
/// and the person asks again.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(90);
/// The name the driver signs its own chat lines with; a message by anyone else is a pass.
pub const AGENT: &str = "agent";
/// The smallest output budget worth a second call when the key's credit runs short.
const MIN_CREDIT_BUDGET: u32 = 4000;

/// Why one model call gave no answer.
#[derive(Debug)]
enum CallError {
    /// The provider answered with no text.
    Empty,
    /// The key's credit does not cover the output budget; the provider's own figure when it
    /// gave one.
    Credit {
        affordable: Option<u32>,
    },
    Failed(String),
}

impl CallError {
    /// What the chat says. Never the provider's body: it carries links to the key's settings.
    fn said(self, budget: u32) -> String {
        match self {
            Self::Empty => "the model's answer carried no text".to_owned(),
            Self::Credit {
                affordable: Some(afford),
            } => format!(
                "the model provider's credit for this Portal has run out (it covers about \
                 {afford} more output tokens, this step asks for up to {budget}); an \
                 administrator tops up the key, then send the message again"
            ),
            Self::Credit { affordable: None } => "the model provider's credit for this Portal \
                 has run out; an administrator tops up the key, then send the message again"
                .to_owned(),
            Self::Failed(reason) => reason,
        }
    }
}

/// The output tokens a 402 says the key can still pay for: OpenRouter writes "can only afford
/// 58556".
fn affordable_tokens(body: &str) -> Option<u32> {
    let said = provider_said(body);
    let (_, rest) = said.split_once("can only afford ")?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

static LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid regex"));

/// The provider's error message, without links, at most 400 characters.
///
/// A gateway says "Provider returned error" and puts the upstream's own words in
/// `error.metadata.raw` (OpenRouter, T-2247): without them a refused call tells the person and the
/// logs nothing they can act on, so both halves are carried, the upstream's first.
fn provider_said(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let field = |pointer: &str| {
        parsed
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|said| !said.is_empty())
            .map(str::to_owned)
    };
    let message = field("/error/message").or_else(|| field("/message"));
    let raw = field("/error/metadata/raw").or_else(|| field("/error/metadata/reason"));
    let said = match (message, raw) {
        (Some(message), Some(raw)) if !raw.contains(&message) => format!("{message}: {raw}"),
        (Some(message), None) => message,
        (_, Some(raw)) => raw,
        (None, None) => body.to_owned(),
    };
    LINK.replace_all(&said, "(link removed)")
        .chars()
        .take(400)
        .collect()
}

/// What a person is told when the model call is refused (T-2419).
///
/// The status and the provider's own words belong in the log line beside this one, where an
/// operator reads them. The conversation gets a sentence about the question they asked. It used to
/// get the problem document instead — `the proxy answered 401 Unauthorized to the model call:
/// {"type":…,"status":401,"detail":"invalid run credentials"}` — which told a person their
/// credentials were wrong for a failure they had no part in, named a component they cannot reach,
/// and said nothing about what to do next (T-2420).
fn refusal(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        // The proxy would not take the run's own credentials, or the provider would not take the
        // Portal's key. Both are ours to fix, and neither is worth a retry by the person.
        401 | 403 => "the assistant could not reach the model service: it refused this Portal's \
                     credentials. Your question was not answered and nothing was changed. Tell an \
                     administrator, then send the message again."
            .to_owned(),
        // Busy or briefly down: the same message, sent again, usually goes through.
        408 | 425 | 429 | 500..=599 => {
            "the model service did not answer in time. Your question was \
                                        not answered and nothing was changed; send the message \
                                        again in a moment."
                .to_owned()
        }
        _ => "the model service refused this call. Your question was not answered and nothing was \
              changed; send the message again, and tell an administrator if it keeps happening."
            .to_owned(),
    }
}

/// The system prompt of a conversation turn: prose or one tool call, never a file (AG-67), in
/// the Portal's own plain voice (UI-45).
// The last sentence is T-2756: the audit found the assistant answering with a table of jc_* tools.
const CONVERSATION_SYSTEM: &str =
    "You are the joinedcontext Portal assistant. Answer the person in \
     plain prose, or with exactly one tool call when the user message describes it. Never write \
     files or SEARCH/REPLACE blocks. Write like a tool, not a chatbot: at most two short \
     sentences before a card or a tool call, saying what was found, drafted or is still needed \
     and naming it; no greeting, no apology, no \"I'll\", \"I've\", \"Let me\", \
     \"successfully\", \"seamless\" or \"powerful\", and no exclamation marks. Say what you can \
     do in the person's words: never a tool name such as jc_resource_propose, never a \
     requirement id such as (AP-44).";

/// What a drafting tool of the conversation left: the answer for the person, or what the model
/// reads back to look at the data again and try once more (AG-76).
enum Worked {
    Done(String),
    Again(String),
}

/// Whether a message asks for an indicator kept up to date rather than one number: a pipeline, a
/// period, a schedule or recomputing on change.
fn asks_for_pipeline(text: &str) -> bool {
    static WORDS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)\b(pipelines?|periodic(ally)?|schedul\w*|every\s+(\d+\s*)?(second|minute|min|hour|day|week)s?|hourly|daily|keep\w*\b.{0,40}\bupdated|on\s+(every\s+)?change)\b",
        )
        .expect("a literal pattern")
    });
    WORDS.is_match(text)
}

/// A draft or a read the model asked for, as the results section names it.
fn drafted(tool: &str, input: Option<Value>) -> data_query::QueryCall {
    let input = input.unwrap_or(Value::Null);
    let endpoint = ["sourceEndpoint", "endpoint"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .to_owned();
    data_query::QueryCall {
        endpoint,
        name: tool.to_owned(),
        arguments: input,
    }
}

/// What one run needs to be driven; everything is copied out of the run and the settings so the
/// task owns what it reads.
/// `text` without the run's ticket, the secret half of its bearer.
fn redact(bearer: &str, text: &str) -> String {
    match bearer.split_once('.') {
        Some((_, ticket)) if !ticket.is_empty() => text.replace(ticket, "[redacted]"),
        _ => text.to_owned(),
    }
}

struct Driver {
    state: AppState,
    http: reqwest::Client,
    run_id: String,
    project: String,
    prompt: String,
    data_needs: Value,
    /// The endpoint the application reads and, with a write need, writes through (AP-62).
    endpoint_slug: String,
    /// Every endpoint the run reads, the primary (`endpoint_slug`) first (AP-44).
    endpoints: Vec<endpoints::RunEndpoint>,
    /// Whether the data needs carry a write operation: what makes a `form` view allowed.
    allows_write: bool,
    bearer: String,
    proxy_base: String,
    model: String,
    provider: String,
    /// The profile's `model.reasoningEffort`, sent on every chat call (AG-72, JC1-26).
    reasoning_effort: Option<String>,
    /// The profile's `model.routing`, sent on every chat call (AG-72).
    routing: Option<Value>,
    ttl: Duration,
    /// How long one model call of a conversation may take before the turn fails (T-2462).
    answer_timeout: Duration,
    /// Passes that produced a preview; the `v` of the preview URL.
    passes: AtomicU32,
    /// The endpoint's `schema/index.json`, read once before the first pass: which types it
    /// serves, so a data need naming anything else never reaches the chat or the model.
    schema_index: OnceLock<Value>,
    /// Which endpoint carried which attributes of a type several endpoints serve, for the pack:
    /// the samples are joined by id, and the application has to join the same way.
    joined: OnceLock<String>,
    /// Where every pass is committed when the Portal has a forge: the run's branch, the
    /// application's folder.
    branch: String,
    path_prefix: String,
    /// The application's own repository in the forge's organization, when the run commits
    /// there rather than into the configuration repository (AP-75), and the application it holds.
    repository: Option<String>,
    app_name: String,
    created_by: String,
    /// The person who started the run: every tool call runs as them, never wider (AG-70).
    identity: Identity,
    /// The profile's access block, checked with `identity` before each tool call (AG-70).
    access: Access,
    kind: String,
    /// A `wasm` App (T-3575): the run starts from the template with its server half.
    wasm: bool,
    unattended: bool,
    continues: Option<String>,
    steps_per_run: u32,
    /// How many characters of a continued conversation's transcript this run may carry
    /// (AG-68), a share of the profile's own token budget.
    transcript_budget: usize,
    /// The profile's `model.maxTokensPerRun`, checked before every call of the editing loop
    /// (AG-25, T-3077).
    max_tokens_per_run: u64,
    /// The form the person asked from, when they asked from one (T-1611).
    form: FormContext,
    /// The path the conversation is on (AG-89): the one the person picked, the one `choose_path`
    /// or a hand-over took, or none. It changes during a turn, so it is behind a lock.
    path: std::sync::Mutex<Option<crate::agents::paths::Path>>,
    /// The kinds whose schema went back to the model this turn: each once (T-6488).
    schema_read: std::sync::Mutex<std::collections::BTreeSet<String>>,
    /// The newest page the person was on as they spoke (T-2763).
    page: std::sync::Mutex<Option<crate::agents::page::PageContext>>,
    /// The capabilities the person chose (AG-92): the opening's, then each message's.
    capabilities: std::sync::Mutex<Option<crate::agents::capabilities::Capabilities>>,
    /// When the request that started this run arrived, for the `elapsedMs` of its steps (AG-91).
    started: std::time::Instant,
}

/// The share of a run's token budget the prior transcript may spend, and four characters to
/// the token: a fifth for what was said before, the rest for what this run has to do (AG-68,
/// AG-41).
fn transcript_budget(max_tokens_per_run: u64) -> usize {
    usize::try_from(max_tokens_per_run / 5 * 4).unwrap_or(usize::MAX)
}

/// Starts the pass in the background. Returns at once; the run's stream is where the outcome
/// goes (AP-60).
/// Where the person is standing when they ask (UI-61, AG-77, T-1611): the form that is open, the
/// draft it edits and the field they were last in.
///
/// It carries no values. What the form holds is the person's own draft, which the assistant reads
/// with `jc_draft_get` and changes with `jc_draft_put` under the caller's grants — so a secret typed
/// into a form never travels in a prompt, and a field the caller may not read stays unread.
#[derive(Debug, Clone, Default)]
pub struct FormContext {
    pub kind: Option<String>,
    pub name: Option<String>,
    pub field: Option<String>,
}

/// What a conversation opens with besides its words: the form it was asked from and the path
/// the person picked (AG-87).
#[derive(Debug, Clone, Default)]
pub struct Opening {
    pub form: FormContext,
    pub path: Option<crate::agents::paths::Path>,
    /// The page the person was on (T-2763).
    pub page: Option<crate::agents::page::PageContext>,
    /// The capabilities the person chose (AG-92, T-2718).
    pub access: Option<crate::agents::capabilities::Capabilities>,
}

impl FormContext {
    /// Nothing to say when no form named itself.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none() && self.name.is_none() && self.field.is_none()
    }
}

pub fn spawn(
    state: AppState,
    run: &AgentRun,
    identity: &Identity,
    ticket: &str,
    profile: &Profile,
    settings: &crate::config::AgentSettings,
    opening: Opening,
) {
    let Opening {
        form,
        path,
        page,
        access,
    } = opening;
    let http = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .unwrap_or_default();
    let driver = Driver {
        state,
        http,
        run_id: run.id.clone(),
        project: run.project.clone(),
        prompt: run.prompt.clone(),
        data_needs: run.data_needs.clone(),
        endpoint_slug: run.endpoint_slug.clone(),
        endpoints: endpoints::of_run(run),
        allows_write: run.allows_write,
        bearer: format!("jcr_{}.{ticket}", run.id),
        proxy_base: settings.proxy_base.trim_end_matches('/').to_owned(),
        model: profile.model_name.clone(),
        provider: profile.model_provider.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        routing: profile.routing.clone(),
        ttl: Duration::from_secs(settings.run_ttl_secs.max(1) as u64),
        answer_timeout: ANSWER_TIMEOUT,
        passes: AtomicU32::new(0),
        schema_index: OnceLock::new(),
        joined: OnceLock::new(),
        branch: run.branch.clone(),
        path_prefix: run.path_prefix.clone(),
        repository: run
            .in_own_repository()
            .then(|| crate::agents::repository::name(&run.project, &run.app_name)),
        app_name: run.app_name.clone(),
        created_by: run.created_by.clone(),
        identity: identity.clone(),
        access: profile.access.clone(),
        kind: run.kind.clone(),
        wasm: jc_core::kinds::AppClass::parse(&run.app_class) == Ok(jc_core::kinds::AppClass::Wasm),
        unattended: run.unattended,
        continues: run.continues.clone(),
        steps_per_run: profile.steps_per_run,
        transcript_budget: transcript_budget(profile.max_tokens_per_run),
        max_tokens_per_run: profile.max_tokens_per_run,
        form,
        path: std::sync::Mutex::new(path),
        schema_read: std::sync::Mutex::default(),
        page: std::sync::Mutex::new(page),
        capabilities: std::sync::Mutex::new(access),
        started: std::time::Instant::now(),
    };
    tokio::spawn(async move {
        let run_id = driver.run_id.clone();
        if let Err(message) = driver.drive().await {
            tracing::warn!(run_id = %run_id, %message, "the run failed");
            driver.fail(&message).await;
        }
    });
}

#[cfg(test)]
impl Driver {
    /// A driver for the unit tests of the conversation's tools: the person `test-user`.
    pub(super) fn for_tests(state: AppState, project: &str) -> Self {
        Driver {
            state,
            http: reqwest::Client::new(),
            run_id: "test-run".into(),
            project: project.into(),
            prompt: String::new(),
            data_needs: json!([]),
            endpoint_slug: "test-slug".into(),
            endpoints: Vec::new(),
            allows_write: false,
            bearer: String::new(),
            proxy_base: "http://localhost:8080".into(),
            model: "test-model".into(),
            provider: "anthropic".into(),
            reasoning_effort: None,
            routing: None,
            ttl: Duration::from_secs(60),
            answer_timeout: ANSWER_TIMEOUT,
            passes: AtomicU32::new(0),
            schema_index: OnceLock::new(),
            joined: OnceLock::new(),
            branch: "agent/test".into(),
            path_prefix: "apps/test/".into(),
            repository: None,
            app_name: "test".into(),
            created_by: "test-user@hel.fi".into(),
            identity: Identity {
                client: None,
                subject: "sub-test-user".into(),
                username: "test-user".into(),
                email: Some("test-user@hel.fi".into()),
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            access: Access::default(),
            kind: "conversation".into(),
            wasm: false,
            unattended: false,
            continues: None,
            steps_per_run: 30,
            transcript_budget: transcript_budget(400_000),
            max_tokens_per_run: 2_000_000,
            form: FormContext::default(),
            path: std::sync::Mutex::new(None),
            schema_read: std::sync::Mutex::default(),
            page: std::sync::Mutex::new(None),
            capabilities: std::sync::Mutex::new(None),
            started: std::time::Instant::now(),
        }
    }
}

impl Driver {
    /// The capabilities the person chose, as the newest message left them (AG-92).
    pub(super) fn capabilities(&self) -> Option<crate::agents::capabilities::Capabilities> {
        self.capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the person is looking at, when they asked from a form (UI-61, AG-77, T-1611).
    ///
    /// The form is the person's own draft, so the assistant works on that draft and nothing else:
    /// it reads it with `jc_draft_get`, and when asked to fill it in it writes the same draft with
    /// `jc_draft_put`. The open form is watching that draft and shows what the assistant wrote, so
    /// the person sees the fields change and decides. Nothing here is proposed.
    fn looking_at(&self, pack: &mut String) {
        if let Some(page) = self
            .page
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            pack.push_str(&page.section());
        }
        if self.form.is_empty() {
            return;
        }
        pack.push_str("\n## THE FORM THE PERSON IS LOOKING AT\n\n");
        if let Some(kind) = &self.form.kind {
            pack.push_str(&format!(
                "They have the {kind} form open in front of them, in this project.\n"
            ));
        }
        if let Some(name) = &self.form.name {
            pack.push_str(&format!(
                "It is editing the draft `{name}`. Read it with `jc_draft_get` before you say \
                 anything about what it holds, and never guess its values.\n"
            ));
        }
        if let Some(field) = &self.form.field {
            pack.push_str(&format!(
                "The field they were last in is `{field}`. A question like \"what goes here?\" or \
                 \"what is this?\" is about that field: answer in two sentences a person filling \
                 the form can use — what it is for, what happens if it is left empty, and one value \
                 that would work — and never in the words of the specification.\n"
            ));
        }
        if self.form.name.is_some() {
            pack.push_str(
                "\nAsked to fill the form in, or to change what it holds, from a sentence: write \
                 that same draft with `jc_draft_put`, carrying the whole manifest with your values \
                 merged into what it already has. The form is watching the draft and shows what you \
                 wrote, the person reads it and proposes it themselves. Never propose it, never open \
                 a Change, and never write a different draft than the one named here.\n",
            );
        }
    }

    async fn drive(&self) -> Result<(), String> {
        // The stream is subscribed before anything moves, so a message sent while the first
        // pass runs is not lost between the preview and the loop.
        // What the run stored before the subscription is no event to answer: a lagging inbox
        // reads the store after it (JC1-31).
        let stored = self
            .state
            .agents
            .events_since(&self.run_id, 0)
            .await
            .ok()
            .and_then(|events| events.last().map(|event| event.seq))
            .unwrap_or(0);
        let mut inbox = self.state.agent_events.subscribe(&self.run_id).await;
        let deadline = tokio::time::Instant::now() + self.ttl;

        if self.kind == "conversation" {
            let mut inbox = crate::agents::events::RunInbox::new(inbox, &self.run_id, stored);
            return self.drive_conversation(&mut inbox, deadline).await;
        }
        // Unreadable, the data needs pass as given, less the abstract base class (see `types`).
        if let Ok(index) = self.schema_index().await {
            let _ = self.schema_index.set(index);
        }
        // Every run that builds is code on the App SDK (ADR-N-022, AP-56): an application, a
        // dashboard (T-3159) and an analysis (T-3160). The spec.json kit is gone (T-0681).
        self.drive_code(&mut inbox, deadline).await
    }
    /// `text` without the run's ticket: an upstream that echoes the request back must not put
    /// the bearer into an event or in front of the model (CC-06).
    fn redacted(&self, text: &str) -> String {
        redact(&self.bearer, text)
    }

    async fn thought(&self, text: &str) -> Result<(), String> {
        self.event("thought", json!({ "text": text })).await
    }

    async fn event(&self, kind: &str, payload: Value) -> Result<(), String> {
        self.append(kind, payload).await.map(|_| ())
    }

    /// [`Self::event`], answering the event as stored, its sequence number included.
    async fn append(&self, kind: &str, mut payload: Value) -> Result<AgentRunEvent, String> {
        // Every step and every page opened says when it happened in the run (T-2697, API/04 §4):
        // a journey reads where the time went from the events alone.
        if matches!(kind, "tool" | "navigate") {
            if let Value::Object(fields) = &mut payload {
                fields
                    .entry("elapsedMs")
                    .or_insert_with(|| json!(self.elapsed_ms()));
            }
        }
        // A tool's arguments and answer and the model's own words can quote what a source or
        // the person typed: credential-shaped text is masked before it is stored (JC1-25).
        if matches!(kind, "tool" | "thought") {
            payload = crate::secret_scan::masked(&payload);
        }
        let text = payload.to_string();
        let payload = if self.redacted(&text) == text {
            payload
        } else {
            serde_json::from_str(&self.redacted(&text)).unwrap_or(Value::Null)
        };
        let event = self
            .state
            .agents
            .append_event(&self.run_id, kind, payload)
            .await
            .map_err(|err| err.to_string())?;
        self.state.agent_events.broadcast(&event).await;
        Ok(event)
    }

    async fn fail(&self, message: &str) {
        let _ = self
            .state
            .agents
            .set_status(&self.run_id, AgentRunStatus::Failed, Some(message))
            .await;
        let _ = self
            .event(
                "status",
                json!({ "status": AgentRunStatus::Failed.as_str(), "error": message, "timestamp": now_rfc3339() }),
            )
            .await;
    }

    /// The wall clock ran out while the run was previewing: the run has ended, the preview
    /// stays as built (AP-60). Not a failure, so no `error` travels with the status and the
    /// page shows the state's own words (T-0669).
    async fn expire(&self) {
        // A run that proposed is on the approval's lease, which the reaper keeps (T-2772): the
        // driver stops here and leaves it waiting.
        if let Ok(Some(run)) = self.state.agents.get_run(&self.run_id).await {
            if run.status == AgentRunStatus::AwaitingApproval.as_str() {
                return;
            }
        }
        let _ = self
            .state
            .agents
            .set_status(&self.run_id, AgentRunStatus::Expired, None)
            .await;
        let _ = self
            .event(
                "status",
                json!({ "status": AgentRunStatus::Expired.as_str(), "timestamp": now_rfc3339() }),
            )
            .await;
    }
}

/// What one pass of a code run came to.
/// What a code call is asked to fix, when it is a repair.
#[derive(Clone, Copy)]
enum Fix<'a> {
    /// The project does not build: transpile errors and refused imports.
    Build(&'a [String]),
    /// The project builds and its preview's check found these (SDK-28).
    Preview(&'a [String]),
    /// The first version is on screen; the rest of the application follows (SDK-13).
    Complete,
    /// The version builds and its tests, run in the sandbox, fail (SDK-38).
    Tests(&'a [String]),
}

/// A generated version in the frame: its `v`, the sequence number of its `preview` event, so
/// what happened after it is its own, and whether its observation has arrived.
#[derive(Clone)]
struct Shown {
    version: u32,
    seq: i64,
    observed: bool,
}

/// What a verification pass works over.
#[derive(Clone, Copy)]
struct Check<'a> {
    samples: &'a Value,
    conversation: &'a [(String, String)],
    instruction: &'a str,
    /// The version on screen came from an instruction the editing agent handled: its repair is
    /// the editing agent's too, never a pass over the whole project (SDK-20, SDK-28).
    edited: bool,
}

/// Verification passes after one instruction (SDK-28).
const MAX_VERIFICATIONS: u32 = 3;
/// How long a version that reported an error waits for its observation before it is checked on
/// the errors alone.
const OBSERVATION_WAIT: Duration = Duration::from_secs(15);

/// Sleeps until `at`, or forever when there is nothing to wait for.
async fn sleep_until_some(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

enum CodePass {
    /// The files changed and build.
    Built { prose: String },
    /// The answer changed no file: a turn of the conversation, not a version.
    Unchanged(String),
    /// Still not building after the repair: the files are as they were and the chat says why.
    Failed,
}

/// The application's name the first answer begins with in bold (`**Helsinki Traffic Alerts
/// Map**`), and the prose with the markers removed; no name when the answer does not begin
/// with one of 3 to 60 characters.
pub(crate) fn title_of(prose: &str) -> (Option<String>, String) {
    let trimmed = prose.trim_start();
    if let Some(rest) = trimmed.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            let name = rest[..end].trim();
            let length = name.chars().count();
            if (3..=60).contains(&length) && !name.contains('\n') {
                return (Some(name.to_owned()), format!("{name}{}", &rest[end + 2..]));
            }
        }
    }
    (None, prose.to_owned())
}

/// The refusals of an answer with its unreadable blocks added, as the step's log shows them.
fn with_unread(refused: &[patch::Refused], unread: usize) -> Vec<patch::Refused> {
    let mut all = refused.to_vec();
    all.extend((0..unread).map(|_| patch::Refused {
        path: String::new(),
        reason: "a block without its closing marker; asked for again".to_owned(),
    }));
    all
}

/// Whether a build problem is patch-protocol talk for the model rather than a build error a
/// person reads (T-0785).
fn is_protocol(problem: &str) -> bool {
    problem.contains("<<<<<<< SEARCH") || problem.starts_with(NO_BLOCKS)
}

/// What a repair call is told when the answer changed nothing: the model's business, never the
/// person's (T-0785).
const NO_BLOCKS: &str = "the answer carried no SEARCH/REPLACE block";

/// What a repair call is told about blocks it could not read.
fn unread_problem(unread: usize) -> String {
    format!(
        "{unread} block(s) of the answer could not be read and were not applied: every block \
         needs its file path on the line right before <<<<<<< SEARCH, then =======, then \
         >>>>>>> REPLACE. Send those changes again as complete blocks"
    )
}

/// The first line of a commit: the first sentence of what the model said, short enough for a
/// log line. The whole of it is the commit body.
pub(crate) fn commit_subject(prose: &str) -> String {
    const MAX: usize = 100;
    let first = prose
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let sentence = first
        .char_indices()
        .find(|(i, c)| {
            matches!(c, '.' | '!' | '?')
                && first[i + c.len_utf8()..].starts_with(char::is_whitespace)
        })
        .map_or(first, |(i, c)| &first[..i + c.len_utf8()]);
    if sentence.chars().count() <= MAX {
        return sentence.to_owned();
    }
    let cut: String = sentence.chars().take(MAX - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':']))
}

/// The subject, then the whole prose when it says more.
fn commit_message(prose: &str) -> String {
    let subject = commit_subject(prose);
    let prose = prose.trim();
    if prose.is_empty() || prose == subject {
        return if subject.is_empty() {
            "Update the application".to_owned()
        } else {
            subject
        };
    }
    format!("{subject}\n\n{prose}")
}

/// `prose` trimmed, or `fallback` when the model wrote none.
fn or_else(prose: String, fallback: &str) -> String {
    match prose.trim() {
        "" => fallback.to_owned(),
        text => text.to_owned(),
    }
}

fn is_terminal(event: &AgentRunEvent) -> bool {
    event
        .payload
        .get("status")
        .and_then(Value::as_str)
        .and_then(AgentRunStatus::parse)
        .is_some_and(|status| status.is_terminal())
}

/// The abstract base class of every NGSI-LD model (`ngsi-ld-core`): a JSON Schema carries it as a
/// definition, but no entity is ever of this type.
const ABSTRACT_TYPES: [&str; 1] = ["Entity"];

/// `types` less the abstract base class and, when the endpoint's schema index is known and
/// lists any type, less every type the endpoint does not serve (EP-46, AP-44).
/// The `jc-types.ts` of several models as one file: each exported name once, the first model's
/// declaration winning, and the first file's preamble.
fn merge_declarations(files: &[String]) -> String {
    static EXPORT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^export\s+(?:declare\s+)?(?:interface|type|enum|const|class|function)\s+([A-Za-z_$][\w$]*)")
            .expect("valid regex")
    });
    let Some((first, rest)) = files.split_first() else {
        return String::new();
    };
    if rest.is_empty() {
        return first.clone();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut merged = String::new();
    for (n, file) in files.iter().enumerate() {
        // A chunk is a top-level `export` and the lines up to the next one.
        let mut chunks: Vec<String> = vec![String::new()];
        for line in file.lines() {
            if line.starts_with("export ") {
                chunks.push(String::new());
            }
            let chunk = chunks.last_mut().expect("one chunk");
            chunk.push_str(line);
            chunk.push('\n');
        }
        for (i, chunk) in chunks.into_iter().enumerate() {
            if i == 0 {
                if n == 0 {
                    merged.push_str(&chunk);
                }
                continue;
            }
            let name = EXPORT
                .captures(&chunk)
                .map(|c| c[1].to_owned())
                .unwrap_or_else(|| chunk.clone());
            if !seen.contains(&name) {
                seen.push(name);
                merged.push_str(&chunk);
            }
        }
    }
    merged
}

fn served_only(types: Vec<String>, index: Option<&Value>) -> Vec<String> {
    let served: Vec<&str> = index
        .and_then(|index| index.get("models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("types").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    types
        .into_iter()
        .filter(|name| !ABSTRACT_TYPES.contains(&name.as_str()))
        .filter(|name| served.is_empty() || served.contains(&name.as_str()))
        .collect()
}

/// `data_needs` with each need's `types` passed through [`served_only`]; a need whose types
/// all go is dropped.
fn needs_served(data_needs: &Value, index: Option<&Value>) -> Value {
    let needs = data_needs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|need| {
            let types: Vec<String> = need
                .get("types")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let kept = served_only(types, index);
            if kept.is_empty() {
                return None;
            }
            let mut need = need.clone();
            need["types"] = json!(kept);
            Some(need)
        });
    Value::Array(needs.collect())
}

/// The most distinct findings [`findings_of`] names; the rest are counted.
const FINDINGS_NAMED: usize = 8;

/// A verdict's findings on one line, for the model, the step and the person: `path message; …`.
/// A finding every record of the page repeats is named once with its count (`records[*].id …
/// (20 records)`), so a red test over a page never reads as the same line per row (T-6532).
fn findings_of(verdict: &Value) -> String {
    static INDEX: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[\d+\]").expect("valid regex"));
    // (the line with its indices as `[*]`, the first line as written, how often)
    let mut grouped: Vec<(String, String, usize)> = Vec::new();
    for f in verdict
        .get("findings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let line = format!(
            "{} {}",
            f.get("path").and_then(Value::as_str).unwrap_or(""),
            f.get("message").and_then(Value::as_str).unwrap_or("")
        );
        let key = INDEX.replace_all(&line, "[*]").into_owned();
        match grouped.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, count)) => *count += 1,
            None => grouped.push((key, line, 1)),
        }
    }
    let mut parts: Vec<String> = grouped
        .iter()
        .take(FINDINGS_NAMED)
        .map(|(key, line, count)| match count {
            1 => line.clone(),
            n => format!("{key} ({n} records)"),
        })
        .collect();
    if grouped.len() > FINDINGS_NAMED {
        parts.push(format!("and {} more", grouped.len() - FINDINGS_NAMED));
    }
    parts.join("; ")
}

/// What a changed pipeline's or data source's test showed: the card beside the chat, and the
/// verdict the draft carries when the test gave one.
#[derive(Default)]
struct Tested {
    card: Option<Value>,
    verdict: Option<crate::ops::verdict::Verdict>,
}

fn failed_step(tool: &str, started: std::time::Instant, input: &Value, reason: &str) -> Value {
    json!({
        "tool": tool,
        "status": "failed",
        "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "input": input,
        "error": reason,
    })
}

/// The prompt section that teaches each tool, by heading, the operation behind the tool, and the
/// kind it proposes when that operation is the one `jc_resource_propose`.
const TOOL_SECTIONS: [(&str, &str, Option<&str>); 4] = [
    (
        "## WHEN THE PERSON ASKS TO SHARE OR PUBLISH DATA",
        "jc_endpoint_propose",
        None,
    ),
    (
        "## WHEN THE PERSON ASKS FOR AN INDICATOR",
        "jc_kpi_compute",
        None,
    ),
    (
        "## WHEN THE PERSON ASKS TO KEEP AN INDICATOR UPDATED",
        "jc_resource_propose",
        Some("Pipeline"),
    ),
    (
        "## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE",
        "jc_space_complete",
        None,
    ),
];

/// The text without the section that starts at `heading`, up to the next `## ` heading.
fn without_section(text: &str, heading: &str) -> String {
    let Some(start) = text.find(heading) else {
        return text.to_owned();
    };
    let body = start + heading.len();
    let end = text[body..]
        .find("\n## ")
        .map_or(text.len(), |offset| body + offset + 1);
    format!("{}{}", &text[..start], &text[end..])
}

/// Whether a registered operation changes something, for the guard that refuses a call the data
/// wrote (AG-20). An operation nobody registered is treated as acting: unknown is not safe.
pub(crate) fn acting_operation(name: &str) -> bool {
    tools_registry::acts(name)
}

pub(crate) fn space_complete_tool_call(answer: &str) -> Option<Value> {
    for fence in share::TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) == Some("space_complete") {
            return Some(value);
        }
    }
    None
}

pub(crate) fn navigate_tool_call(answer: &str) -> Option<String> {
    for fence in share::TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) == Some("navigate") {
            return value
                .get("route")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
    None
}

fn sent_by_person(event: &AgentRunEvent) -> bool {
    event
        .payload
        .get("sentBy")
        .and_then(Value::as_str)
        .is_some_and(|by| by != AGENT)
}

/// Percent-encodes one query value; entity types are URIs or short names, both fit.
fn urlencoding(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-6532 (live walk T-6490, S-20): a test red on every row of a page was pasted to the person
    /// as two lines per row; each finding is named once with how many records repeat it.
    #[test]
    fn findings_repeated_per_record_are_named_once_with_their_count() {
        let mut findings = Vec::new();
        for i in 0..20 {
            findings.push(json!({ "path": format!("validation[{i}]"), "message": "no 'type'" }));
            findings
                .push(json!({ "path": format!("records[{i}].id"), "message": "id: not a URN" }));
        }
        findings.push(json!({ "path": "records[3].count", "message": "count: not a number" }));
        let line = findings_of(&json!({ "ok": false, "findings": findings }));
        assert_eq!(
            line,
            "validation[*] no 'type' (20 records); records[*].id id: not a URN (20 records); \
             records[3].count count: not a number"
        );
        let many: Vec<Value> = (0..12)
            .map(|i| json!({ "path": format!("spec.f{i}"), "message": "wrong" }))
            .collect();
        assert!(findings_of(&json!({ "findings": many })).ends_with("spec.f7 wrong; and 4 more"));
    }

    /// JC1-25: a tool event quoting a credential (a source URL's key, a header a log line
    /// repeats) is stored masked, not only the run's own ticket.
    #[tokio::test]
    async fn a_tool_event_quoting_a_credential_is_stored_masked() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state.clone(), "helsinki");
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJlc2lnbmF0dXJl";
        driver
            .event(
                "tool",
                json!({ "name": "jc_data_query", "output": format!("failed: Authorization: Bearer {jwt}") }),
            )
            .await
            .expect("stored");
        let stored = state
            .agents
            .events_since("test-run", 0)
            .await
            .expect("events");
        let text = serde_json::to_string(&stored[0].payload).expect("json");
        assert!(!text.contains(jwt), "{text}");
        assert!(text.contains("jc_data_query"), "{text}");
    }

    /// T-2419: a refused model call tells the person about their question, not about a proxy.
    #[test]
    fn a_refused_model_call_is_a_sentence_a_person_can_act_on() {
        use reqwest::StatusCode;
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::IM_A_TEAPOT,
        ] {
            let said = refusal(status);
            // No problem document, no internal component, no bare status code.
            for leak in ["{", "\"type\"", "proxy", "401", "invalid run credentials"] {
                assert!(
                    !said.contains(leak),
                    "{status} says {leak:?} to a person: {said}"
                );
            }
            // What happened to their question, and what they do now.
            assert!(said.contains("nothing was changed"), "{status}: {said}");
            assert!(said.contains("send the message again"), "{status}: {said}");
        }
        // A refusal of this Portal's credentials is not something a person retries their way out
        // of alone: it names the administrator.
        assert!(refusal(reqwest::StatusCode::UNAUTHORIZED).contains("administrator"));
        // A busy service is: it does not.
        assert!(!refusal(reqwest::StatusCode::SERVICE_UNAVAILABLE).contains("administrator"));
    }

    #[test]
    fn a_period_a_schedule_or_a_pipeline_asks_for_a_pipeline_and_a_question_does_not() {
        for yes in [
            "Create a pipeline from helsinki into helsinki-kpi",
            "compute the total of available bikes every 15 minutes",
            "recompute it on every change",
            "keep the average free slots updated",
            "an hourly count of alerts",
        ] {
            assert!(asks_for_pipeline(yes), "{yes}");
        }
        for no in [
            "how many stations are closed",
            "what is the average PM10 now",
        ] {
            assert!(!asks_for_pipeline(no), "{no}");
        }
    }

    #[test]
    fn the_conversation_prompt_asks_for_a_short_plain_reply() {
        for rule in [
            "at most two short sentences",
            "no greeting, no apology",
            "\"I'll\", \"I've\", \"Let me\"",
            "\"successfully\", \"seamless\" or \"powerful\"",
            "no exclamation marks",
            "never a tool name such as jc_resource_propose",
            "never a requirement id such as (AP-44)",
        ] {
            assert!(CONVERSATION_SYSTEM.contains(rule), "{rule}");
        }
    }

    #[test]
    fn the_types_of_several_models_are_one_file_with_each_name_once() {
        let bikes = "// generated\nimport type { Row } from \"@joinedcontext/sdk\";\nexport interface Station extends Row {\n  type: \"Station\";\n}\nexport type Status = \"open\" | \"closed\";\n";
        let kpis = "// generated\nimport type { Row } from \"@joinedcontext/sdk\";\nexport interface KeyPerformanceIndicator extends Row {\n  type: \"KeyPerformanceIndicator\";\n}\nexport type Status = \"draft\";\n";
        let merged = merge_declarations(&[bikes.to_owned(), kpis.to_owned()]);
        assert_eq!(merged.matches("import type").count(), 1, "{merged}");
        assert_eq!(merged.matches("export type Status").count(), 1, "{merged}");
        assert!(merged.contains("\"open\" | \"closed\""), "{merged}");
        assert!(
            merged.contains("export interface KeyPerformanceIndicator"),
            "{merged}"
        );
        assert_eq!(merge_declarations(&[bikes.to_owned()]), bikes);
    }

    #[test]
    fn the_first_answer_names_the_application_in_bold() {
        let (title, prose) =
            title_of("**Helsinki Traffic Alerts Map** shows every alert on a map.");
        assert_eq!(title.as_deref(), Some("Helsinki Traffic Alerts Map"));
        assert_eq!(
            prose,
            "Helsinki Traffic Alerts Map shows every alert on a map."
        );
        assert_eq!(title_of("A map of alerts.").0, None);
        assert_eq!(title_of("**ab** too short").0, None);
    }

    #[test]
    fn a_commit_subject_is_the_first_sentence_and_short() {
        assert_eq!(
            commit_subject("The Overview has more KPIs. It also shows alerts.\n\nMore."),
            "The Overview has more KPIs."
        );
        assert_eq!(
            commit_subject("Version 2.5 of the map"),
            "Version 2.5 of the map"
        );
        let long = "This application provides an interactive map and dashboard for Helsinki city bike docking stations, showing real-time bike availability and free docking slots";
        let subject = commit_subject(long);
        assert!(subject.chars().count() <= 100, "{subject}");
        assert!(subject.ends_with('…'), "{subject}");
        assert_eq!(commit_message("Done."), "Done.");
        assert_eq!(commit_message(""), "Update the application");
        assert_eq!(commit_message("One. Two."), "One.\n\nOne. Two.");
    }

    #[test]
    fn a_credit_refusal_gives_its_figure_and_never_its_links() {
        let body = r#"{"error":{"message":"This request requires more credits, or fewer max_tokens. You requested up to 64000 tokens, but can only afford 58556. To increase, visit https://openrouter.ai/workspaces/default/keys/e0cd and adjust the key's total limit","code":402}}"#;
        assert_eq!(affordable_tokens(body), Some(58556));
        assert!(!provider_said(body).contains("openrouter.ai"));
        let said = CallError::Credit {
            affordable: affordable_tokens(body),
        }
        .said(64000);
        assert!(said.contains("58556") && said.contains("64000"), "{said}");
        assert!(!said.contains("http"), "{said}");
        assert_eq!(affordable_tokens("Payment Required"), None);
        assert_eq!(
            provider_said("plain <b>body</b> at http://x.test/a"),
            "plain <b>body</b> at (link removed)"
        );
    }

    /// T-2247: a refused call must say what the upstream refused. The gateway's own sentence is
    /// the same for every cause ("Provider returned error"), and what a person or a log can act on
    /// sits in `error.metadata.raw`.
    #[test]
    fn a_refused_call_carries_the_upstreams_own_words_and_not_only_the_gateways() {
        let body = r#"{"error":{"message":"Provider returned error","code":400,"metadata":{"provider_name":"Google","raw":"{\"error\":{\"code\":400,\"message\":\"contents.parts must not be empty\",\"status\":\"INVALID_ARGUMENT\"}}"}}}"#;
        let said = provider_said(body);
        assert!(said.starts_with("Provider returned error: "), "{said}");
        assert!(said.contains("contents.parts must not be empty"), "{said}");

        // A gateway that repeats itself inside `raw` is said once.
        let doubled =
            r#"{"error":{"message":"rate limited","metadata":{"raw":"rate limited by Google"}}}"#;
        assert_eq!(provider_said(doubled), "rate limited by Google");

        // An empty `raw` is no message at all, and the gateway's sentence still arrives.
        let empty = r#"{"error":{"message":"Provider returned error","metadata":{"raw":"  "}}}"#;
        assert_eq!(provider_said(empty), "Provider returned error");
        assert!(provider_said(r#"{"error":{"code":400}}"#).contains("400"));
    }

    #[test]
    fn a_type_the_endpoint_does_not_serve_and_the_abstract_base_class_are_left_out() {
        let index = json!({ "models": [
            { "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"] },
            { "name": "weather", "version": 2, "types": ["WeatherObserved"] }
        ] });
        let named = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();

        assert_eq!(
            served_only(
                named(&[
                    "BikeHireDockingStation",
                    "Entity",
                    "Alert",
                    "WeatherObserved"
                ]),
                Some(&index)
            ),
            named(&["BikeHireDockingStation", "WeatherObserved"])
        );
        // Without an index, only the abstract base class goes.
        assert_eq!(
            served_only(named(&["BikeHireDockingStation", "Entity"]), None),
            named(&["BikeHireDockingStation"])
        );
        // An index that lists no type narrows nothing more.
        assert_eq!(
            served_only(named(&["Alert", "Entity"]), Some(&json!({ "models": [] }))),
            named(&["Alert"])
        );

        let needs = json!([
            { "types": ["BikeHireDockingStation", "Entity"], "attrs": ["name"] },
            { "types": ["Entity"], "attrs": ["id"] }
        ]);
        assert_eq!(
            needs_served(&needs, Some(&index)),
            json!([{ "types": ["BikeHireDockingStation"], "attrs": ["name"] }])
        );
    }

    #[test]
    fn without_section_drops_one_heading_up_to_the_next() {
        let text = "## A\none\n## B\ntwo\n```json\n{}\n```\n## C\nthree\n";
        assert_eq!(without_section(text, "## B"), "## A\none\n## C\nthree\n");
        assert_eq!(
            without_section(text, "## C"),
            "## A\none\n## B\ntwo\n```json\n{}\n```\n"
        );
        assert_eq!(without_section(text, "## D"), text);
    }

    #[test]
    fn navigate_tool_call_extracts_route() {
        let text = "I will open the Assistant.\n\n```json\n{\"tool\":\"navigate\",\"route\":\"/projects/helsinki/assistant\"}\n```\n";
        assert_eq!(
            navigate_tool_call(text),
            Some("/projects/helsinki/assistant".to_owned())
        );
        assert_eq!(navigate_tool_call("plain response"), None);
    }

    #[test]
    fn types_are_listed_once_in_order_and_urls_are_encoded() {
        let driver_types = |needs: Value| {
            let mut types = Vec::new();
            for need in needs.as_array().into_iter().flatten() {
                for t in need["types"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !types.iter().any(|k: &String| k == t) {
                        types.push(t.to_owned());
                    }
                }
            }
            types
        };
        assert_eq!(
            driver_types(json!([{ "types": ["A", "B"] }, { "types": ["B", "C"] }])),
            vec!["A", "B", "C"]
        );
        assert_eq!(
            urlencoding("https://uri.fiware.org/ns/data-models#Bike Station"),
            "https%3A%2F%2Furi.fiware.org%2Fns%2Fdata-models%23Bike%20Station"
        );
    }

    #[test]
    fn a_message_by_the_agent_itself_starts_no_pass() {
        let event = |by: &str| AgentRunEvent {
            run_id: "r".into(),
            seq: 1,
            kind: "message".into(),
            payload: json!({ "text": "x", "sentBy": by }),
            created_at: String::new(),
        };
        assert!(sent_by_person(&event("demo.steward")));
        assert!(!sent_by_person(&event(AGENT)));
        let status = AgentRunEvent {
            kind: "status".into(),
            payload: json!({ "status": "failed" }),
            ..event("x")
        };
        assert!(is_terminal(&status));
    }
}
