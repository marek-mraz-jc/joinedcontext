//! The knowledge assistant's sources through the registry (T-6284, API/14 §4.8).
//!
//! A KnowledgeSource is planned like any other kind; what the assistant holds of it (the crawl
//! state, the counts, the last failed reads) and its passage search had browser routes and no
//! operations, so a connector could add a source and never learn whether it was crawled or what it
//! answers. Each operation calls the route's own function: the grants and the answers are the
//! route's, and nothing here fetches a page.

use axum::extract::{Path, Query, State};
use axum::Json;
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME};
use super::runs::as_user;
use super::{annotations, parse_input, OpError, Operation};
use crate::api::knowledge::Tried;
use crate::change::Lane;
use crate::error::ApiError;

/// The longest search text and the most hits, as the assistant's own trial search takes them.
const MAX_Q: u64 = 300;
const MAX_HITS: u8 = 10;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StatusInput {
    /// One source; absent lists every source of the project.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchInput {
    pub source: String,
    pub q: String,
    pub limit: Option<u8>,
}

fn status_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source": text("e.g. city-web; absent: every source", NAME)
        },
        "additionalProperties": false
    })
}

fn search_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source": text("e.g. city-web", NAME),
            "q": { "type": "string", "minLength": 1, "maxLength": MAX_Q, "description": "e.g. parking permit" },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_HITS, "description": "default 5" }
        },
        "required": ["source", "q"],
        "additionalProperties": false
    })
}

fn status_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "description": "Each source with its crawl state; state not-crawled before the first crawl",
                "items": { "type": "object" }
            },
            "errors": {
                "type": "array",
                "description": "The named source's last failed reads, for a caller who may propose a KnowledgeSource",
                "items": { "type": "object" }
            }
        },
        "required": ["items"]
    })
}

fn search_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "hits": {
                "type": "array",
                "description": "Passages best first, each with its page url and title",
                "items": { "type": "object" }
            }
        },
        "required": ["hits"]
    })
}

fn validate_status(val: &Value) -> Result<(), OpError> {
    parse_input::<StatusInput>(val.clone()).map(|_| ())
}

fn validate_search(val: &Value) -> Result<(), OpError> {
    let input: SearchInput = parse_input(val.clone())?;
    let q = input.q.trim();
    if q.is_empty() || q.chars().count() > MAX_Q as usize {
        return Err(OpError::InvalidInput {
            path: "/q".into(),
            message: format!("q must be 1 to {MAX_Q} characters, e.g. parking permit"),
        });
    }
    if input.limit.is_some_and(|l| l == 0 || l > MAX_HITS) {
        return Err(OpError::InvalidInput {
            path: "/limit".into(),
            message: format!("limit must be 1 to {MAX_HITS}"),
        });
    }
    Ok(())
}

/// The route's list, narrowed to `source` when the call names one; with it, the source's last
/// failed reads when the caller may propose a KnowledgeSource, as the errors route decides.
async fn status(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: StatusInput,
) -> Result<Value, OpError> {
    let Json(mut listed) = crate::api::knowledge::list_sources(
        as_user(caller),
        State(state.clone()),
        Path(project.to_owned()),
    )
    .await?;
    let Some(source) = input.source else {
        return Ok(listed);
    };
    let items = listed["items"]
        .as_array_mut()
        .map(std::mem::take)
        .unwrap_or_default();
    let Some(item) = items.into_iter().find(|i| i["source"] == source.as_str()) else {
        return Err(OpError::Api(ApiError::NotFound(format!(
            "KnowledgeSource '{source}' not found in project '{project}'; jc_knowledge_status without source lists them"
        ))));
    };
    let mut answer = json!({ "items": [item] });
    let proposes = crate::permissions::for_request(state, &caller.identity, project)
        .check("KnowledgeSource", Verb::Propose, None)
        .is_ok();
    if proposes && answer["items"][0]["state"] != "not-crawled" {
        let Json(errors) = crate::api::knowledge::list_errors(
            as_user(caller),
            State(state.clone()),
            Path((project.to_owned(), source)),
        )
        .await?;
        answer["errors"] = errors.get("items").cloned().unwrap_or_else(|| json!([]));
    }
    Ok(answer)
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_knowledge_status",
            title: "Read Knowledge State",
            description:
                "Each KnowledgeSource's crawl state and counts; with source, its last failed reads",
            input: status_input_schema,
            output: status_output_schema,
            annotations: annotations(true, false, true),
            kind: "KnowledgeSource",
            verb: None,
            lane: Lane::Green,
            validate: validate_status,
            run: |caller, state, project, val| {
                Box::pin(async move { status(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_knowledge_search",
            title: "Search Knowledge",
            description:
                "Passages of one KnowledgeSource, best first, each with its page url and title",
            input: search_input_schema,
            output: search_output_schema,
            annotations: annotations(true, false, true),
            kind: "KnowledgeSource",
            verb: None,
            lane: Lane::Green,
            validate: validate_search,
            run: |caller, state, project, val| {
                Box::pin(async move {
                    validate_search(&val)?;
                    let input: SearchInput = parse_input(val)?;
                    let Json(hits) = crate::api::knowledge::try_search(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.source)),
                        Query(Tried {
                            q: input.q.trim().to_owned(),
                            limit: input.limit,
                        }),
                    )
                    .await?;
                    Ok(hits)
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_names_its_source_and_a_bounded_q() {
        assert!(validate_search(&json!({ "source": "web", "q": "parking" })).is_ok());
        assert!(validate_search(&json!({ "source": "web", "q": "x", "limit": 10 })).is_ok());
        for (input, path) in [
            (json!({ "source": "web", "q": "  " }), "/q"),
            (json!({ "source": "web", "q": "x".repeat(301) }), "/q"),
            (json!({ "source": "web", "q": "x", "limit": 0 }), "/limit"),
            (json!({ "source": "web", "q": "x", "limit": 11 }), "/limit"),
        ] {
            match validate_search(&input) {
                Err(OpError::InvalidInput { path: at, .. }) => assert_eq!(at, path, "{input}"),
                other => panic!("{input}: {other:?}"),
            }
        }
        assert!(validate_search(&json!({ "q": "x" })).is_err());
        assert!(validate_search(&json!({ "source": "web", "q": "x", "url": "http://a" })).is_err());
    }

    #[test]
    fn a_status_takes_at_most_one_source() {
        assert!(validate_status(&json!({})).is_ok());
        assert!(validate_status(&json!({ "source": "web" })).is_ok());
        assert!(validate_status(&json!({ "startUrls": ["http://a"] })).is_err());
    }
}
