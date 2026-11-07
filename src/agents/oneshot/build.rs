//! The steps of "Build an app" the Portal takes itself (T-2696, T-2721, API/04 §Paths): which
//! endpoints the app reads, what it may do with the data, who opens it and what it should do, each
//! asked in the conversation with no model call. The answers end as one `app-build` event, the
//! card whose Build the person presses to start the run with their own session: the conversation
//! starts no run (AG-11), and the run is checked against the person's grants like any other
//! (AP-132, PF-70).

use super::integrate::Taken;
use super::*;

/// The first question of the path, as its `question` event carries it in `step`.
pub(super) const ENDPOINTS_STEP: &str = "build-app-endpoints";
pub(super) const ACCESS_STEP: &str = "build-app-access";
pub(super) const AUDIENCE_STEP: &str = "build-app-audience";
pub(super) const DESCRIBE_STEP: &str = "build-app-describe";

/// The presets of AP-132, as the builder offers them.
const ACCESS: [(&str, &str); 3] = [
    ("read", "Only read it"),
    ("update", "Read and update records"),
    ("full", "Read, add, update and delete records"),
];

/// Who may open the app, as `App.spec.visibility` names it; `public` is never a run's (AP-42).
const AUDIENCE: [(&str, &str); 3] = [
    ("project", "Everyone in the project"),
    ("organization", "Everyone in the organization"),
    ("private", "Only me"),
];
const PUBLIC_REFUSED: &str = "A run never builds a public app (AP-42): build it for the project, \
     then make it public on its page, which a publisher approves";

/// The longest description a run takes: `POST …/agent-runs` refuses a longer prompt.
const MAX_DESCRIPTION_CHARS: usize = crate::api::agent_runs::MAX_PROMPT_CHARS;

fn option(value: &str, title: &str) -> tools_registry::AskOption {
    tools_registry::AskOption {
        value: value.to_owned(),
        title: title.to_owned(),
        description: None,
        disabled: None,
    }
}

fn question(
    text: &str,
    options: Vec<tools_registry::AskOption>,
    step: &'static str,
) -> tools_registry::AskCall {
    tools_registry::AskCall {
        question: text.to_owned(),
        options,
        default: None,
        pick: None,
        multiple: false,
        min: None,
        max: None,
        input: tools_registry::AskInput::default(),
        step: Some(step),
    }
}

/// The answer the person gave to the newest question of `step`, if they answered it.
fn answer_of(events: &[AgentRunEvent], step: &str) -> Option<Value> {
    let asked: Vec<&str> = events
        .iter()
        .filter(|event| {
            event.kind == "question"
                && event.payload.get("step").and_then(Value::as_str) == Some(step)
        })
        .filter_map(|event| event.payload.get("questionId").and_then(Value::as_str))
        .collect();
    events
        .iter()
        .rev()
        .find(|event| {
            event.kind == "answer"
                && event
                    .payload
                    .get("questionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| asked.contains(&id))
        })
        .and_then(|event| event.payload.get("answers")?.get("answer").cloned())
}

/// The endpoints chosen, in the order chosen; empty for words instead of a choice.
fn endpoints_of(answer: Option<&Value>) -> Vec<String> {
    match answer {
        Some(Value::Array(chosen)) => chosen
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => Vec::new(),
    }
}

/// One of `known`'s values, or `None` for anything else (words, an option that was never offered).
fn one_of(answer: &Value, known: &[(&str, &str)]) -> Option<String> {
    let value = answer.get("answer").and_then(Value::as_str)?;
    known
        .iter()
        .any(|(offered, _)| *offered == value)
        .then(|| value.to_owned())
}

/// A description the run takes: trimmed, not empty, not longer than a run's prompt.
fn description_of(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("say what the app should do".to_owned());
    }
    if text.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "the description is longer than {MAX_DESCRIPTION_CHARS} characters"
        ));
    }
    Ok(text.to_owned())
}

impl Driver {
    /// Takes an answer to one of this path's own questions; `None` leaves it to the model.
    pub(super) async fn build_answer(
        &self,
        step: &str,
        answers: &Value,
        events: &[AgentRunEvent],
    ) -> Result<Option<Taken>, String> {
        match step {
            ENDPOINTS_STEP => {
                if self
                    .chosen_endpoints(answers.get("answer"))
                    .await
                    .is_empty()
                {
                    return Ok(None);
                }
                self.ask_build(question(
                    "What may the app do with the data?",
                    ACCESS
                        .iter()
                        .map(|(value, title)| option(value, title))
                        .collect(),
                    ACCESS_STEP,
                ))
                .await
            }
            ACCESS_STEP => {
                if one_of(answers, &ACCESS).is_none() {
                    return Ok(None);
                }
                let mut options: Vec<_> = AUDIENCE
                    .iter()
                    .map(|(value, title)| option(value, title))
                    .collect();
                options.push(tools_registry::AskOption {
                    disabled: Some(PUBLIC_REFUSED.to_owned()),
                    ..option("public", "Anyone, without signing in")
                });
                let mut ask = question("Who opens it?", options, AUDIENCE_STEP);
                ask.default = Some(json!("project"));
                self.ask_build(ask).await
            }
            AUDIENCE_STEP => {
                if one_of(answers, &AUDIENCE).is_none() {
                    return Ok(None);
                }
                self.ask_build(question(
                    "What should the app do?",
                    Vec::new(),
                    DESCRIBE_STEP,
                ))
                .await
            }
            DESCRIBE_STEP => match answers.get("answer").and_then(Value::as_str) {
                Some(text) => self.ready_to_build(text, events).await.map(Some),
                None => Ok(None),
            },
            _ => Ok(None),
        }
    }

    /// Words typed while "What should the app do?" stands answer it; `None` when it does not stand.
    pub(super) async fn build_words(&self, text: &str) -> Result<Option<Taken>, String> {
        if self.current_path() != Some(crate::agents::paths::Path::BuildApp) {
            return Ok(None);
        }
        let events = self
            .state
            .agents
            .events_since(&self.run_id, 0)
            .await
            .map_err(|err| err.to_string())?;
        let standing = events.iter().rev().find(|event| {
            event.kind == "question" || event.kind == "answer" || event.kind == "app-build"
        });
        let describing = standing.is_some_and(|event| {
            event.kind == "question"
                && event.payload.get("step").and_then(Value::as_str) == Some(DESCRIBE_STEP)
        });
        if !describing {
            return Ok(None);
        }
        self.ready_to_build(text, &events).await.map(Some)
    }

    /// The chosen endpoints the person may read in the project, in the order chosen (JC1-14):
    /// words typed where nothing was there to pick name no endpoint, and leave the turn to the
    /// model. A listing that fails reads as nothing readable.
    async fn chosen_endpoints(&self, answer: Option<&Value>) -> Vec<String> {
        let chosen = endpoints_of(answer);
        if chosen.is_empty() {
            return chosen;
        }
        let readable = self.readable("endpoints").await.unwrap_or_default();
        chosen
            .into_iter()
            .filter(|name| {
                readable
                    .iter()
                    .any(|item| item.get("name").and_then(Value::as_str) == Some(name.as_str()))
            })
            .collect()
    }

    async fn ask_build(&self, call: tools_registry::AskCall) -> Result<Option<Taken>, String> {
        let asked = call.question.clone();
        self.ask_person(&call, Some(self.elapsed_ms())).await?;
        Ok(Some(Taken::Done(asked)))
    }

    /// Every answer of the path in one `app-build` event, the card the person starts the run from.
    async fn ready_to_build(&self, text: &str, events: &[AgentRunEvent]) -> Result<Taken, String> {
        let prompt = match description_of(text) {
            Ok(prompt) => prompt,
            Err(reason) => {
                // Asked again rather than built from nothing.
                self.ask_build(question(
                    "What should the app do?",
                    Vec::new(),
                    DESCRIBE_STEP,
                ))
                .await?;
                return Ok(Taken::Done(reason));
            }
        };
        let endpoints = self
            .chosen_endpoints(answer_of(events, ENDPOINTS_STEP).as_ref())
            .await;
        let chosen = |step: &str, known: &[(&str, &str)]| {
            answer_of(events, step).and_then(|answer| one_of(&json!({ "answer": answer }), known))
        };
        let (Some(access), Some(visibility)) = (
            chosen(ACCESS_STEP, &ACCESS),
            chosen(AUDIENCE_STEP, &AUDIENCE),
        ) else {
            return Err("the endpoints, the access and the audience are answered before the app is described".to_owned());
        };
        if endpoints.is_empty() {
            return Err("no endpoint was chosen for the app to read".to_owned());
        }
        let audience = AUDIENCE
            .iter()
            .find(|(value, _)| *value == visibility)
            .map_or(visibility.as_str(), |(_, title)| title);
        let prose = format!(
            "Ready to build: it reads {}, may {}, and {} opens it. Press Build to start it here; \
             its preview shows in this conversation.",
            endpoints
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", "),
            ACCESS
                .iter()
                .find(|(value, _)| *value == access)
                .map_or("read it", |(_, title)| title)
                .to_lowercase(),
            audience.to_lowercase(),
        );
        self.thought(&prose).await?;
        self.event(
            "app-build",
            json!({
                "endpoints": endpoints,
                "access": access,
                "visibility": visibility,
                "prompt": prompt,
                "elapsedMs": self.elapsed_ms(),
            }),
        )
        .await?;
        Ok(Taken::Done(prose))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_one_of_the_options_offered_and_words_are_none() {
        assert_eq!(
            one_of(&json!({ "answer": "update" }), &ACCESS).as_deref(),
            Some("update")
        );
        assert_eq!(one_of(&json!({ "answer": "public" }), &AUDIENCE), None);
        assert_eq!(
            one_of(&json!({ "answer": "write everything" }), &ACCESS),
            None
        );
        assert_eq!(one_of(&json!({ "answer": ["read"] }), &ACCESS), None);
    }

    /// JC1-14: words typed where the project had no Endpoint to pick name none, so the path
    /// asks nothing further and the model takes the words.
    #[tokio::test]
    async fn words_for_an_endpoint_the_project_lacks_choose_none() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let driver = Driver::for_tests(state, "helsinki");
        let taken = driver
            .build_answer(
                ENDPOINTS_STEP,
                &json!({ "answer": "bike counters please" }),
                &[],
            )
            .await
            .expect("no failure");
        assert!(taken.is_none(), "the words went on as an endpoint name");
        assert!(driver
            .chosen_endpoints(Some(&json!(["bike counters please"])))
            .await
            .is_empty());
    }

    #[test]
    fn the_endpoints_are_the_order_chosen_and_words_choose_none() {
        assert_eq!(
            endpoints_of(Some(&json!(["bikes", "air"]))),
            ["bikes", "air"]
        );
        assert_eq!(endpoints_of(Some(&json!("bikes"))), ["bikes"]);
        assert!(endpoints_of(Some(&json!({ "text": "bikes" }))).is_empty());
        assert!(endpoints_of(None).is_empty());
    }

    #[test]
    fn a_description_is_words_within_a_runs_prompt() {
        assert_eq!(
            description_of("  a desk for alerts  ").as_deref(),
            Ok("a desk for alerts")
        );
        assert!(description_of("   ").is_err());
        assert!(description_of(&"x".repeat(MAX_DESCRIPTION_CHARS + 1)).is_err());
        assert!(description_of(&"é".repeat(MAX_DESCRIPTION_CHARS)).is_ok());
    }
}
