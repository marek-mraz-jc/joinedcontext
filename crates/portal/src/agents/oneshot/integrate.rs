//! The steps of "Integrate a pipeline" the Portal takes itself (T-2695, ADR-N-032, AG-91): a
//! file or a feed's address handed over at the first step is profiled, the person picks where it
//! lands, and a new space is drafted whole by `space_complete` and opened ready to propose. A file
//! is a sample of a feed: its feed's address is asked for, the pipeline reads that address and is
//! tested on the file. What the Portal cannot decide alone (an existing space's model, a data
//! source, words) goes to the model, on the path, with what was handed over said plainly.

use super::*;

/// The steps a question of this path is, as its `question` event carries them in `step`.
pub(super) const SOURCE_STEP: &str = "integrate-source";
const TARGET_STEP: &str = "integrate-target";
const FEED_STEP: &str = "integrate-feed";

/// The option of the target question that drafts a new space rather than picking one.
const NEW_SPACE: &str = "new";
/// The option of the feed question for a file that is data once, with no feed behind it.
const NO_FEED: &str = "none";

/// What the Portal did with an answer.
pub(super) enum Taken {
    /// It took the step itself; the prose is the turn's, for the conversation's history.
    Done(String),
    /// The model takes it, with these words as the person's turn.
    Model(String),
}

/// A handed-over sample's shape: what the person reads before choosing where it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Profile {
    pub rows: usize,
    pub columns: Vec<String>,
}

/// The rows and columns of a CSV, TSV or JSON sample. JSON is read as the records the feed
/// shape finds (an array, a FeatureCollection, an envelope around one).
// ponytail: a quoted newline inside a CSV field counts as a row; the model's inference reads the
// file properly, this is the one line a person reads.
pub(super) fn profile(format: &str, text: &str) -> Profile {
    if format == "json" {
        let records = serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .and_then(crate::ops::feed_shape::records);
        return records.map_or(
            Profile {
                rows: 0,
                columns: Vec::new(),
            },
            |records| Profile {
                rows: records.rows.len(),
                columns: records.fields(),
            },
        );
    }
    let separator = if format == "tsv" { '\t' } else { ',' };
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let columns = lines
        .next()
        .map(|header| fields(header, separator))
        .unwrap_or_default();
    Profile {
        rows: lines.count(),
        columns,
    }
}

/// The fields of one CSV line: split on the separator outside double quotes, quotes undone.
fn fields(line: &str, separator: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == separator && !quoted => out.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    out.push(field);
    out.into_iter().map(|f| f.trim().to_owned()).collect()
}

/// A TSV sample as the CSV `space_complete` reads: each field quoted when it needs to be.
pub(super) fn tsv_as_csv(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.split('\t')
                .map(|field| {
                    if field.contains([',', '"']) {
                        format!("\"{}\"", field.replace('"', "\"\""))
                    } else {
                        field.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The file `space_complete` takes: its stem kept, its extension the format's, in lower case,
/// because the operation knows a sample by `.csv` and `.json` (a `Stations.CSV` was no sample).
fn completion_file(name: &str, format: &str, text: &str) -> Value {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let (extension, content) = match format {
        "tsv" => ("csv", tsv_as_csv(text)),
        other => (other, text.to_owned()),
    };
    json!({ "name": format!("{stem}.{extension}"), "content": content })
}

impl Driver {
    /// Takes an answer to one of this path's own questions, or leaves it to the model (`None`).
    pub(super) async fn integrate_answer(
        &self,
        answer: &crate::agents::run::AgentRunEvent,
    ) -> Result<Option<Taken>, String> {
        let question_id = answer
            .payload
            .get("questionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let events = self
            .state
            .agents
            .events_since(&self.run_id, 0)
            .await
            .map_err(|err| err.to_string())?;
        let step = events
            .iter()
            .rev()
            .find(|event| {
                event.kind == "question"
                    && event.payload.get("questionId").and_then(Value::as_str) == Some(question_id)
            })
            .and_then(|event| event.payload.get("step").and_then(Value::as_str));
        let answers = answer
            .payload
            .get("answers")
            .cloned()
            .unwrap_or(Value::Null);
        match step {
            Some(SOURCE_STEP) => self.source_taken(&answers).await,
            Some(TARGET_STEP) => self.target_taken(&answers, &events).await,
            Some(FEED_STEP) => self.feed_taken(&answers, &events).await,
            Some(
                step @ (build::ENDPOINTS_STEP
                | build::ACCESS_STEP
                | build::AUDIENCE_STEP
                | build::DESCRIBE_STEP),
            ) => self.build_answer(step, &answers, &events).await,
            Some(run_now::STEP) => self.run_answer(&answers).await,
            _ => Ok(None),
        }
    }

    /// A file is profiled and an address kept for the runner; either way the next question is
    /// where it lands. An option or words go to the model.
    async fn source_taken(&self, answers: &Value) -> Result<Option<Taken>, String> {
        let prose = if let Some(file) = answers.get("file") {
            let started = std::time::Instant::now();
            let field = |key: &str| file.get(key).and_then(Value::as_str).unwrap_or_default();
            let shape = profile(field("format"), field("text"));
            self.event(
                "tool",
                json!({
                    "tool": "profile_sample",
                    "status": "ok",
                    "durationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "elapsedMs": self.elapsed_ms(),
                    "input": { "name": field("name"), "format": field("format") },
                    "output": { "rows": shape.rows, "columns": shape.columns },
                }),
            )
            .await?;
            format!(
                "{} holds {} rows with the columns {}.",
                field("name"),
                shape.rows,
                shape.columns.join(", ")
            )
        } else if let Some(url) = answers.get("url").and_then(Value::as_str) {
            // Read once, by the runner, when the space is drafted: never twice (T-2697).
            format!("The data is read from {url}.")
        } else {
            return Ok(None);
        };
        self.thought(&prose).await?;
        self.ask_target().await?;
        Ok(Some(Taken::Done(prose)))
    }

    /// "Which space should it land in?": the spaces the person may read, and a new one.
    async fn ask_target(&self) -> Result<(), String> {
        let call = tools_registry::AskCall {
            question: "Which space should it land in?".to_owned(),
            options: Vec::new(),
            default: None,
            pick: Some("spaces"),
            multiple: false,
            min: None,
            max: None,
            input: tools_registry::AskInput::default(),
            step: Some(TARGET_STEP),
        };
        let mut call = match self.fill_options(call.clone()).await {
            Ok(call) => call,
            // No space to pick is no refusal here: a new one is still an answer.
            Err(_) => call,
        };
        call.options.push(tools_registry::AskOption {
            value: NEW_SPACE.to_owned(),
            title: "A new context space".to_owned(),
            description: Some("Drafted with its data model, endpoint and pipeline".to_owned()),
            disabled: None,
        });
        call.default = Some(json!(NEW_SPACE));
        self.ask_person(&call, Some(self.elapsed_ms())).await?;
        Ok(())
    }

    /// A new space is drafted whole from what was handed over and opened ready to propose; an
    /// existing one is the model's, told where the data lands.
    async fn target_taken(
        &self,
        answers: &Value,
        events: &[crate::agents::run::AgentRunEvent],
    ) -> Result<Option<Taken>, String> {
        let Some(target) = answers.get("answer").and_then(Value::as_str) else {
            return Ok(None);
        };
        if target != NEW_SPACE {
            return Ok(Some(Taken::Model(format!(
                "Land the data I handed over in the existing context space '{target}'."
            ))));
        }
        let source = handed_over(events).ok_or("nothing was handed over to draft a space from")?;
        if source.get("files").is_some() {
            let question = "What is the address of the feed this file is a sample of?";
            self.ask_feed(question).await?;
            return Ok(Some(Taken::Done(question.to_owned())));
        }
        Ok(Some(Taken::Done(self.space_complete(source, "").await?)))
    }

    /// "What is the address of the feed this file is a sample of?": an address, or no feed.
    async fn ask_feed(&self, question: &str) -> Result<(), String> {
        let call = tools_registry::AskCall {
            question: question.to_owned(),
            options: vec![tools_registry::AskOption {
                value: NO_FEED.to_owned(),
                title: "There is no feed".to_owned(),
                description: Some(
                    "Draft the space and its data model from the file; its rows load through Import"
                        .to_owned(),
                ),
                disabled: None,
            }],
            default: None,
            pick: None,
            multiple: false,
            min: None,
            max: None,
            input: tools_registry::AskInput {
                file: false,
                url: true,
            },
            step: Some(FEED_STEP),
        };
        self.ask_person(&call, Some(self.elapsed_ms())).await?;
        Ok(())
    }

    /// The new space drafted from the file: with its feed's address, a data source reading that
    /// address and a pipeline tested on the file; with none, the space and its model. Words go to
    /// the model.
    async fn feed_taken(
        &self,
        answers: &Value,
        events: &[crate::agents::run::AgentRunEvent],
    ) -> Result<Option<Taken>, String> {
        let feed = if let Some(url) = answers.get("url").and_then(Value::as_str) {
            // The answer was checked as an absolute http(s) address when it was given; written
            // back as parsed, it is one word the operation reads as the source's address.
            Some(
                url::Url::parse(url)
                    .map_err(|err| format!("the feed's address is not a URL: {err}"))?
                    .to_string(),
            )
        } else if answers.get("answer").and_then(Value::as_str) == Some(NO_FEED) {
            None
        } else {
            return Ok(None);
        };
        let mut source =
            handed_over(events).ok_or("nothing was handed over to draft a space from")?;
        let Some(files) = source.get_mut("files").and_then(Value::as_array_mut) else {
            return Err("the feed question follows a file, and no file was handed over".to_owned());
        };
        let Some(feed) = feed else {
            // The person loads the file's rows themselves, and every write goes through an
            // Endpoint and its Policy: the space gets both drafted with it (DM-81, JC1-17).
            source["upload"] = json!(true);
            let prose = self.space_complete(source, "").await?;
            // A pipeline reads a feed; a file with none is data once (T-2695). Said, not guessed
            // around.
            let once = "A file is data once, not a feed: its space and data model are drafted, \
                        and its rows load through Import once the space is approved. Give a \
                        feed's address to have a pipeline read it on a schedule.";
            self.thought(once).await?;
            return Ok(Some(Taken::Done(format!("{prose} {once}"))));
        };
        // `space_complete` reads a source's address from a README beside the sample and tests
        // the pipeline on the sample: the file is the feed's sample, the address its source.
        files.push(json!({
            "name": "README.md",
            "content": format!("The feed this file is a sample of: {feed}\n"),
        }));
        Ok(Some(Taken::Done(self.space_complete(source, "").await?)))
    }
}

/// The input `space_complete` drafts from: the file or the address answered at the first step.
fn handed_over(events: &[crate::agents::run::AgentRunEvent]) -> Option<Value> {
    let source_questions: Vec<&str> = events
        .iter()
        .filter(|event| {
            event.kind == "question"
                && event.payload.get("step").and_then(Value::as_str) == Some(SOURCE_STEP)
        })
        .filter_map(|event| event.payload.get("questionId").and_then(Value::as_str))
        .collect();
    let answers = events
        .iter()
        .rev()
        .filter(|event| event.kind == "answer")
        .find(|event| {
            event
                .payload
                .get("questionId")
                .and_then(Value::as_str)
                .is_some_and(|id| source_questions.contains(&id))
        })?
        .payload
        .get("answers")?;
    if let Some(file) = answers.get("file") {
        let field = |key: &str| file.get(key).and_then(Value::as_str);
        return Some(json!({
            "files": [completion_file(field("name")?, field("format")?, field("text")?)]
        }));
    }
    let url = answers.get("url").and_then(Value::as_str)?;
    Some(json!({ "url": url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A doubled quote inside a quoted field is one quote, and the separator inside quotes is
    /// text (T-7818).
    #[test]
    fn a_doubled_quote_in_a_quoted_field_is_one_quote() {
        assert_eq!(
            fields(r#""say ""hi""";b,c;d"#, ';'),
            vec![r#"say "hi""#.to_owned(), "b,c".to_owned(), "d".to_owned()]
        );
    }

    #[test]
    fn the_first_step_of_the_path_is_the_source_step() {
        assert_eq!(
            crate::agents::paths::Path::IntegratePipeline
                .first_step()
                .step,
            Some(SOURCE_STEP)
        );
    }

    #[test]
    fn a_csv_is_its_header_and_its_rows() {
        let text = "station,\"name, long\",bikes\n1,\"Kamppi, \"\"A\"\"\",4\n\n2,Kallio,0\n";
        assert_eq!(
            profile("csv", text),
            Profile {
                rows: 2,
                columns: vec!["station".into(), "name, long".into(), "bikes".into()],
            }
        );
        assert_eq!(profile("tsv", "a\tb\n1\t2\n").columns, vec!["a", "b"]);
        assert_eq!(
            profile("csv", ""),
            Profile {
                rows: 0,
                columns: Vec::new()
            }
        );
    }

    #[test]
    fn json_is_the_records_it_holds() {
        let text = r#"{"data": {"stations": [{"id": 1, "bikes": 3}, {"id": 2, "name": "x"}]}}"#;
        let shape = profile("json", text);
        assert_eq!(shape.rows, 2);
        assert!(
            shape.columns.contains(&"bikes".to_owned())
                && shape.columns.contains(&"name".to_owned())
        );
        assert_eq!(profile("json", "not json").rows, 0);
    }

    #[test]
    fn the_file_space_complete_takes_has_a_lower_case_extension_and_is_csv_for_tsv() {
        assert_eq!(
            completion_file("Stations.CSV", "csv", "a,b\n1,2"),
            json!({ "name": "Stations.csv", "content": "a,b\n1,2" })
        );
        assert_eq!(
            completion_file("bikes.tsv", "tsv", "a\tb\n1,5\t\"x\"")["content"],
            "a,b\n\"1,5\",\"\"\"x\"\"\""
        );
        assert_eq!(completion_file("noext", "json", "[]")["name"], "noext.json");
    }

    fn event(kind: &str, payload: Value) -> crate::agents::run::AgentRunEvent {
        crate::agents::run::AgentRunEvent {
            run_id: "r".into(),
            seq: 0,
            kind: kind.into(),
            payload,
            created_at: String::new(),
        }
    }

    fn driver() -> Driver {
        Driver::for_tests(
            crate::state::AppState::new(crate::config::Config::for_tests(), None),
            "helsinki",
        )
    }

    /// T-7924: an answer this path cannot read without the run's events is a failure the turn
    /// reports, never an answer taken from nothing: a store that is down says so.
    #[tokio::test]
    async fn an_answer_on_a_store_that_is_down_is_a_failure() {
        let mut dead = driver();
        dead.state.agents = std::sync::Arc::new(crate::agents::store::AgentStore::new(
            Some(crate::test_db::dead_pool()),
            crate::test_db::placed_mirror("helsinki"),
        ));
        let answer = event("answer", json!({ "questionId": "q-1", "answers": {} }));
        assert!(dead.integrate_answer(&answer).await.is_err());
    }

    /// T-7924: an answer to a question this path never asked, or to none, is the model's.
    #[tokio::test]
    async fn an_answer_to_no_question_of_this_path_is_the_models() {
        let driver = driver();
        for payload in [
            json!({}),
            json!({ "questionId": "q-unknown", "answers": {} }),
        ] {
            let taken = driver
                .integrate_answer(&event("answer", payload.clone()))
                .await
                .expect("no store failure");
            assert!(taken.is_none(), "{payload}");
        }
    }

    /// T-7924: a source answer with neither a file nor an address is words for the model, and
    /// nothing is asked next.
    #[tokio::test]
    async fn a_source_answer_without_a_file_or_an_address_is_the_models() {
        let driver = driver();
        for answers in [json!({}), json!({ "answer": "bikes" }), json!({ "url": 7 })] {
            let taken = driver.source_taken(&answers).await.expect("no failure");
            assert!(taken.is_none(), "{answers}");
        }
        let all = driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events");
        assert!(all.iter().all(|event| event.kind != "question"));
    }

    /// T-7924: in a project with no space to pick, the address still leads on to the target
    /// question, which offers the new space alone and chooses it by default.
    #[tokio::test]
    async fn with_no_space_to_pick_the_target_question_offers_a_new_one() {
        let driver = driver();
        let taken = driver
            .source_taken(&json!({ "url": "https://feed.example/a.json" }))
            .await
            .expect("no failure");
        assert!(
            matches!(&taken, Some(Taken::Done(prose)) if prose == "The data is read from https://feed.example/a.json."),
        );
        let questions: Vec<Value> = driver
            .state
            .agents
            .events_since(&driver.run_id, 0)
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == "question")
            .map(|event| event.payload)
            .collect();
        assert_eq!(questions.len(), 1, "{questions:?}");
        assert_eq!(questions[0]["step"], TARGET_STEP);
        let options = questions[0]["options"].as_array().expect("options");
        assert_eq!(options.len(), 1, "{options:?}");
        assert_eq!(options[0]["value"], NEW_SPACE);
        assert_eq!(questions[0]["default"], NEW_SPACE);
    }

    /// T-7924: a target answer without a chosen option is the model's.
    #[tokio::test]
    async fn a_target_answer_without_an_option_is_the_models() {
        let driver = driver();
        for answers in [json!({}), json!({ "answer": 3 })] {
            let taken = driver
                .target_taken(&answers, &[])
                .await
                .expect("no failure");
            assert!(taken.is_none(), "{answers}");
        }
    }

    /// T-7924: the feed question takes an address or "no feed": anything else is the model's,
    /// an address that is no URL is refused, and a feed for a source that was no file is
    /// refused, never drafted from nothing.
    #[tokio::test]
    async fn the_feed_answer_takes_an_address_or_no_feed() {
        let driver = driver();
        for answers in [json!({}), json!({ "answer": "later" }), json!({ "url": 1 })] {
            let taken = driver.feed_taken(&answers, &[]).await.expect("no failure");
            assert!(taken.is_none(), "{answers}");
        }
        let not_a_url = driver
            .feed_taken(&json!({ "url": "no address" }), &[])
            .await
            .err()
            .expect("refused");
        assert!(
            not_a_url.starts_with("the feed's address is not a URL"),
            "{not_a_url}"
        );
        let nothing = driver
            .feed_taken(&json!({ "answer": NO_FEED }), &[])
            .await
            .err()
            .expect("refused");
        assert_eq!(nothing, "nothing was handed over to draft a space from");
        let address_only = [
            event(
                "question",
                json!({ "questionId": "q-1", "step": SOURCE_STEP }),
            ),
            event(
                "answer",
                json!({ "questionId": "q-1", "answers": { "url": "https://feed.example/a.json" } }),
            ),
        ];
        let no_file = driver
            .feed_taken(
                &json!({ "url": "https://feed.example/b.json" }),
                &address_only,
            )
            .await
            .err()
            .expect("refused");
        assert_eq!(
            no_file,
            "the feed question follows a file, and no file was handed over"
        );
    }

    #[test]
    fn what_was_handed_over_is_the_answer_to_the_source_question() {
        let event = |kind: &str, payload: Value| crate::agents::run::AgentRunEvent {
            run_id: "r".into(),
            seq: 0,
            kind: kind.into(),
            payload,
            created_at: String::new(),
        };
        let events = [
            event(
                "question",
                json!({ "questionId": "q-1", "step": SOURCE_STEP }),
            ),
            event("question", json!({ "questionId": "q-other" })),
            event(
                "answer",
                json!({ "questionId": "q-other", "answers": { "url": "https://no.example" } }),
            ),
            event(
                "answer",
                json!({ "questionId": "q-1", "answers": { "url": "https://feed.example/a.json" } }),
            ),
        ];
        assert_eq!(
            handed_over(&events),
            Some(json!({ "url": "https://feed.example/a.json" }))
        );
        assert_eq!(
            handed_over(&events[..3]),
            None,
            "an answer to another question is not the source"
        );
    }
}
