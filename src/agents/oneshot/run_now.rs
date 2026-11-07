//! A pipeline's run outside its schedule from the conversation (T-6482, GOAL-connectors §6). The
//! assistant reaches `jc_pipeline_run` as every MCP client does, and as there it runs on the
//! person's word, never the model's (AG-63): the call leaves the turn with a question the Portal
//! asks, and the person's answer runs it with no model in between. A conversation run as a
//! connector segment has no writes, so it is never offered the run.

use super::integrate::Taken;
use super::tools_registry::{AskCall, AskInput, AskOption, RegistryCall};
use super::*;

use crate::connectors::RUN;

/// The question's name on the run's log.
pub(super) const STEP: &str = "run-now";
/// What the option that runs answers, before the pipeline's name. The answers route takes only
/// what the question offered (`offered_answer`), so the name is the one the person read.
const RUN_ANSWER: &str = "run:";
/// What the option that runs nothing answers.
const KEEP_ANSWER: &str = "keep";

fn confirmation(name: &str) -> AskCall {
    let option = |value: String, title: &str| AskOption {
        value,
        title: title.to_owned(),
        description: None,
        disabled: None,
    };
    AskCall {
        question: format!("Run pipeline {name} now, outside its schedule?"),
        options: vec![
            option(format!("{RUN_ANSWER}{name}"), "Run it now"),
            option(KEEP_ANSWER.to_owned(), "Do not run it"),
        ],
        default: None,
        pick: None,
        multiple: false,
        min: None,
        max: None,
        input: AskInput::default(),
        step: Some(STEP),
    }
}

impl Driver {
    /// The model's `jc_pipeline_run`: asks the person and answers the question's words, which
    /// end the turn, or the refusal that goes back to the model, before anybody is asked to
    /// confirm what would then be refused (AG-63). Nothing runs here.
    pub(super) async fn ask_to_run(&self, input: &Value) -> Result<Result<String, String>, String> {
        let started = std::time::Instant::now();
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let checked = self
            .access
            .check(RUN, &self.identity, &self.state, &self.project)
            .and_then(|()| name.ok_or_else(|| format!("{RUN} names the pipeline: name it")))
            .and_then(|name| {
                // Registered: `check` found it.
                let op = crate::ops::find(RUN).ok_or_else(|| RUN.to_owned())?;
                let (identity, state) = (&self.identity, &self.state);
                crate::ops::may_try(op, identity, state, &self.project, Some(input))
                    .map(|()| name)
                    .map_err(|refused| refused.to_string())
            });
        match checked {
            Ok(name) => Ok(Ok(self.ask_person(&confirmation(name), None).await?)),
            Err(reason) => {
                self.event("tool", failed_step(RUN, started, input, &reason))
                    .await?;
                Ok(Err(reason))
            }
        }
    }

    /// The person's answer to [`confirmation`]: the run, or nothing. The call goes through the
    /// door every call of this conversation takes, so each check runs again; words instead of
    /// an option go to the model.
    pub(super) async fn run_answer(&self, answers: &Value) -> Result<Option<Taken>, String> {
        let Some(chosen) = answers.get("answer").and_then(Value::as_str) else {
            return Ok(None);
        };
        let prose = match chosen.strip_prefix(RUN_ANSWER) {
            Some(name) => self.run_confirmed(name).await?,
            None if chosen == KEEP_ANSWER => {
                "Nothing ran: the pipeline stays on its schedule.".to_owned()
            }
            None => return Ok(None),
        };
        self.thought(&prose).await?;
        Ok(Some(Taken::Done(prose)))
    }

    /// The confirmed run, in a sentence for the person, with the pipeline's page open once it is
    /// queued: the run ends later, and that page shows how. The conversation's capabilities may
    /// have narrowed since the question.
    async fn run_confirmed(&self, name: &str) -> Result<String, String> {
        if self
            .capabilities()
            .is_some_and(|chosen| !chosen.allows_tool(RUN, false))
        {
            return Ok(format!(
                "Pipeline {name} did not start: this conversation may only read now."
            ));
        }
        let call = RegistryCall {
            name: RUN.to_owned(),
            arguments: json!({ "name": name }),
        };
        let said = self.registry_call(&call).await?;
        let answer: Option<Value> = serde_json::from_str(&said).ok();
        let Some(run) = answer
            .as_ref()
            .and_then(|answer| answer.get("run"))
            .and_then(Value::as_str)
        else {
            let reason = said.trim_start_matches("error: ");
            return Ok(format!("Pipeline {name} did not start: {reason}"));
        };
        let page = tools_registry::navigate_args(
            &json!({ "page": "resource", "name": name, "plural": "Pipeline" }),
        )?;
        self.open_page(&page).await?;
        Ok(format!(
            "Pipeline {name} is queued as run {run}: it waits its turn in its lane, and its page \
             shows how it ended."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agents::capabilities::{Capabilities, Preset};
    use crate::agents::paths::Path;
    use crate::state::AppState;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A steward whose profile grants the run, in a project with one pipeline.
    fn steward() -> Driver {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Pipeline".into(),
            metadata: crate::resource::ObjectMeta::new("counters-load", "helsinki"),
            spec: json!({}),
            status: None,
        });
        let mut driver = Driver::for_tests(state.clone(), "helsinki");
        driver.identity.roles = vec![state.config.bootstrap_admins.clone()];
        driver.access = crate::agents::access::Access::from_spec(&json!({ "access": {
            "operations": [RUN],
            "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }],
        } }));
        driver
    }

    async fn events(driver: &Driver, kind: &str) -> Vec<Value> {
        let all = driver.state.agents.events_since(&driver.run_id, 0).await;
        all.expect("events")
            .into_iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.payload)
            .collect()
    }

    /// The model's call ends the turn with the Portal's question: no tool ran, and the person
    /// reads the pipeline's name and both ways out (S-24 of the live walk T-5962).
    #[tokio::test]
    async fn the_models_call_asks_the_person_and_runs_nothing() {
        let model = MockServer::start().await;
        let fence = "```json\n{ \"tool\": \"jc_pipeline_run\", \"arguments\": { \"name\": \"counters-load\" } }\n```";
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": fence }],
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            })))
            .mount(&model)
            .await;
        let mut driver = steward();
        driver.proxy_base = model.uri();
        let asked = driver
            .converse(&[], "Run pipeline counters-load now")
            .await
            .expect("a turn");
        assert_eq!(
            asked,
            "Run pipeline counters-load now, outside its schedule?"
        );
        assert!(events(&driver, "tool").await.is_empty());
        let questions = events(&driver, "question").await;
        assert_eq!(questions[0]["step"], STEP);
        assert_eq!(questions[0]["options"][0]["value"], "run:counters-load");
        assert_eq!(questions[0]["options"][1]["value"], KEEP_ANSWER);
        assert_eq!(model.received_requests().await.expect("recorded").len(), 1);
    }

    /// What would be refused after the answer is refused before the question (AG-63, T-6280).
    #[tokio::test]
    async fn a_refused_run_asks_nobody() {
        for (input, why) in [
            (json!({}), "name it"),
            (json!({ "name": " " }), "name it"),
            (json!({ "name": "only-in-a-change" }), "jc_change_list"),
        ] {
            let driver = steward();
            let refused = driver.ask_to_run(&input).await.expect("no store failure");
            assert!(refused.is_err_and(|reason| reason.contains(why)), "{input}");
            assert!(events(&driver, "question").await.is_empty(), "{input}");
        }

        let mut outsider = steward();
        outsider.identity.roles.clear();
        let refused = outsider
            .ask_to_run(&json!({ "name": "counters-load" }))
            .await
            .expect("no store failure");
        assert!(refused.is_err());
        assert!(events(&outsider, "question").await.is_empty());

        let mut unnamed = steward();
        unnamed.access = crate::agents::access::Access::from_spec(&json!({}));
        let refused = unnamed
            .ask_to_run(&json!({ "name": "counters-load" }))
            .await
            .expect("no store failure");
        assert!(refused.is_err_and(|reason| reason.contains("AG-70")));
    }

    /// "Do not run it" and words run nothing; words go on to the model.
    #[tokio::test]
    async fn only_the_run_option_runs() {
        let driver = steward();
        let kept = driver.run_answer(&json!({ "answer": KEEP_ANSWER })).await;
        assert!(matches!(kept, Ok(Some(Taken::Done(prose))) if prose.contains("Nothing ran")));
        let words = driver.run_answer(&json!({ "answer": "later" })).await;
        assert!(matches!(words, Ok(None)));
        let none = driver.run_answer(&json!({})).await;
        assert!(matches!(none, Ok(None)));
        assert!(events(&driver, "tool").await.is_empty());
    }

    /// The confirmed run reaches the operation (this test world has no runner, so the door's own
    /// 503 is the proof), and the person reads why it did not start.
    #[tokio::test]
    async fn the_answer_runs_the_operation_and_says_a_refusal() {
        let driver = steward();
        let taken = driver
            .run_answer(&json!({ "answer": "run:counters-load" }))
            .await;
        let Ok(Some(Taken::Done(prose))) = taken else {
            panic!("the Portal takes the answer");
        };
        assert!(prose.contains("did not start"), "{prose}");
        assert!(prose.contains("no pipeline runner"), "{prose}");
        let tools = events(&driver, "tool").await;
        assert_eq!(tools.len(), 1, "{tools:?}");
        assert_eq!(tools[0]["tool"], RUN);
        assert_eq!(tools[0]["input"]["name"], "counters-load");
    }

    /// A conversation switched to reading after the question runs nothing.
    #[tokio::test]
    async fn a_read_only_conversation_runs_nothing() {
        let driver = steward();
        *driver
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Capabilities {
            preset: Preset::Read,
            endpoints: Default::default(),
            segment: None,
        });
        let taken = driver
            .run_answer(&json!({ "answer": "run:counters-load" }))
            .await;
        assert!(matches!(taken, Ok(Some(Taken::Done(prose))) if prose.contains("may only read")));
        assert!(events(&driver, "tool").await.is_empty());
    }

    /// The free turn and the pipeline path offer the run; the other paths keep their budget.
    #[test]
    fn the_free_turn_and_the_pipeline_path_offer_the_run() {
        assert!(Path::offers(None, RUN));
        assert!(Path::offers(Some(Path::IntegratePipeline), RUN));
        assert!(!Path::offers(Some(Path::FindData), RUN));
    }

    /// No connector segment of a conversation reaches the run: it has no writes (AG-142).
    #[test]
    fn a_segment_conversation_is_never_offered_the_run() {
        let driver = steward();
        for segment in crate::connectors::SEGMENTS.iter().map(|segment| segment.0) {
            let (_, offered) = driver.connector_offered(segment);
            assert!(!offered.iter().any(|op| op.name == RUN), "{segment}");
        }
    }

    /// The page the assistant opens for a connector is the person's own (S-46, S-50).
    #[test]
    fn a_connector_leads_to_my_connectors() {
        let call = tools_registry::navigate_args(&json!({ "page": "connectors" })).expect("a page");
        assert_eq!(
            tools_registry::route_of("helsinki", &call),
            Ok("/ai-connectors".to_owned())
        );
    }

    /// A profile that names the run offers it, and the prompt says what the assistant does
    /// about a run and a connector.
    #[test]
    fn the_prompt_names_the_run_and_the_connectors_page() {
        let driver = steward();
        let offered = tools_registry::offered(
            &driver.access,
            &driver.identity,
            &driver.state,
            &driver.project,
        );
        assert!(offered.iter().any(|op| op.name == RUN));
        let section = tools_registry::section(&offered);
        assert!(section.contains("page `connectors`"), "{section}");
        assert!(section.contains("Call `jc_pipeline_run`"), "{section}");
    }
}
