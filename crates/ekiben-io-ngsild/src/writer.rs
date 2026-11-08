//! The writer core of the `ngsi_ld` processor and output (E5).
//!
//! The entities of a batch's messages (a message is one entity or an array of entities) are
//! packed into batch requests that pass neither cap: `max_batch_count` entities (at most 1000,
//! Antares' item limit) and `max_batch_bytes` (at most Antares' 4 MiB body limit). A request
//! never carries one entity id twice, so a 207's `errors[]` maps back to its messages by
//! `entityId`. A request of several messages refused whole (`drop_on`: 400, 413, 422) is sent
//! again one message at a time, so a single bad message never costs the others their write.
//! Values pass through untouched: no `@context` is added, nothing is rewritten.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_bloblang::Value;
use ekiben_core::{CoreError, OrgContext, Part};
use futures::StreamExt;
use reqwest::Method;
use serde_json::Value as JsonValue;

use crate::client::{Answer, CallEnv, Failure, NgsiClient};
use crate::config::{
    self, BrokerUrl, Fields, CLIENT_FIELDS, DEFAULT_BATCH_BYTES, MAX_BATCH_BYTES, MAX_BATCH_COUNT,
    MAX_IN_FLIGHT,
};
use crate::metrics;

/// The writer's own fields, beside [`CLIENT_FIELDS`].
pub const WRITER_FIELDS: &[&str] = &[
    "operation",
    "options",
    "max_batch_count",
    "max_batch_bytes",
    "max_in_flight",
];

/// The metadata key the Portal's outcome sink reads a whole refusal from.
pub const FAILED_KEY: &str = "jc_failed";
/// The start of the metadata key the Portal's outcome sink reads member refusals from.
pub const REFUSED_PREFIX: &str = "jc_refused_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Upsert,
    Update,
    Create,
}

/// One member refusal as the Portal's `result_map` writes it: `{id, message}`, the id the
/// broker named (or `""`) and its problem's `detail`, else `title`, else `""`.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub id: Value,
    pub message: Value,
}

/// What happened to one message of a batch.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    /// Its entities the broker refused one by one (a 207's `errors`), or that could not be sent.
    pub refused: Vec<Refusal>,
    /// Every request carrying its entities was refused whole (`drop_on`), or the message is no
    /// entity at all: the broker's words for it (the problem's `detail`, else `title`, else the
    /// error).
    pub whole: Option<String>,
    /// A request carrying its entities still failed after its retries: the error.
    pub failed: Option<String>,
    /// How many of its entities were written.
    pub written: usize,
}

struct Entity {
    msg: usize,
    id: Option<String>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Tally {
    entities: usize,
    whole_refused: usize,
    whole_text: Option<String>,
    whole_members: Vec<Refusal>,
}

pub struct Writer {
    url: String,
    operation: Operation,
    max_count: usize,
    max_bytes: usize,
    max_in_flight: usize,
    client: NgsiClient,
}

impl Writer {
    pub fn from_config(
        conf: &JsonValue,
        component: &'static str,
        org: &Arc<OrgContext>,
    ) -> Result<Self, CoreError> {
        let known: Vec<&str> = CLIENT_FIELDS.iter().chain(WRITER_FIELDS).copied().collect();
        let f = Fields::new(conf, component, &known)?;
        let raw_url = f
            .str("url")?
            .ok_or_else(|| f.err("field url is required"))?;
        let base = BrokerUrl::parse(component, raw_url)?;
        let operation = match f.string_or("operation", "upsert")?.as_str() {
            "upsert" => Operation::Upsert,
            "update" => Operation::Update,
            "create" => Operation::Create,
            other => {
                return Err(f.err(format!(
                    "operation must be upsert, update or create, got {other:?}"
                )))
            }
        };
        let replace = match f.string_or("options", "update")?.as_str() {
            "update" => false,
            "replace" => true,
            other => return Err(f.err(format!("options must be update or replace, got {other:?}"))),
        };
        // The Portal's write: `upsert?options=update`, and replace drops the query (PL-79).
        let url = match (operation, replace) {
            (Operation::Upsert, false) => base.join("entityOperations/upsert?options=update"),
            (Operation::Upsert, true) => base.join("entityOperations/upsert"),
            (Operation::Update, false) => base.join("entityOperations/update"),
            (Operation::Create, false) => base.join("entityOperations/create"),
            (_, true) => return Err(f.err("options: replace is only defined for operation upsert")),
        };
        let max_count = f.u64_or("max_batch_count", MAX_BATCH_COUNT as u64)?;
        if max_count == 0 || max_count > MAX_BATCH_COUNT as u64 {
            return Err(f.err(format!(
                "max_batch_count must be between 1 and {MAX_BATCH_COUNT}: Antares refuses a larger batch"
            )));
        }
        let max_bytes = f.bytes_or("max_batch_bytes", DEFAULT_BATCH_BYTES)?;
        if !(2..=MAX_BATCH_BYTES).contains(&max_bytes) {
            return Err(f.err(format!(
                "max_batch_bytes must be at most 4MiB ({MAX_BATCH_BYTES} bytes): Antares refuses a larger body"
            )));
        }
        let max_in_flight = f.u64_or("max_in_flight", 1)?;
        if max_in_flight == 0 || max_in_flight > MAX_IN_FLIGHT as u64 {
            return Err(f.err(format!(
                "max_in_flight must be between 1 and {MAX_IN_FLIGHT}"
            )));
        }
        let settings = config::parse_client(&f, &base, Duration::from_secs(30), &[400, 413, 422])?;
        Ok(Self {
            url,
            operation,
            max_count: usize::try_from(max_count).unwrap_or(MAX_BATCH_COUNT),
            max_bytes,
            max_in_flight: usize::try_from(max_in_flight).unwrap_or(1),
            client: NgsiClient::new(org, settings)?,
        })
    }

    /// The URL every batch request of this writer posts to.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn operation(&self) -> Operation {
        self.operation
    }

    /// Writes the entities of `parts` and answers one [`Verdict`] per message, in order.
    pub async fn write(&self, env: &CallEnv<'_>, parts: &[Part]) -> Vec<Verdict> {
        let mut verdicts: Vec<Verdict> = vec![Verdict::default(); parts.len()];
        let mut tallies: Vec<Tally> = (0..parts.len()).map(|_| Tally::default()).collect();
        let mut entities: Vec<Entity> = Vec::new();
        let mut local_refusals = 0u64;
        for ((msg, part), verdict) in parts.iter().enumerate().zip(verdicts.iter_mut()) {
            match decode(part) {
                Err(why) => verdict.whole = Some(why),
                Ok(list) => {
                    for (id, bytes) in list {
                        if bytes.len().saturating_add(2) > self.max_bytes {
                            local_refusals += 1;
                            verdict.refused.push(Refusal {
                                id: Value::String(id.clone().unwrap_or_default()),
                                message: Value::String(format!(
                                    "the entity is {} bytes, more than max_batch_bytes ({})",
                                    bytes.len(),
                                    self.max_bytes
                                )),
                            });
                            continue;
                        }
                        entities.push(Entity { msg, id, bytes });
                    }
                }
            }
        }
        for e in &entities {
            if let Some(t) = tallies.get_mut(e.msg) {
                t.entities += 1;
            }
        }

        let first = self.plan(&entities, 0..entities.len());
        let mut isolate: Vec<usize> = Vec::new();
        for (members, result) in self.run(env, &entities, first).await {
            let msgs = messages_of(&entities, &members);
            match result {
                Err(Failure::Refused(_)) if msgs.len() > 1 => isolate.extend(members),
                other => self.apply(env, &entities, &members, other, &mut verdicts, &mut tallies),
            }
        }
        if !isolate.is_empty() {
            // One message at a time, so the refusal names the message it belongs to.
            let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
            for idx in isolate {
                let Some(e) = entities.get(idx) else { continue };
                match groups.iter_mut().find(|(m, _)| *m == e.msg) {
                    Some((_, list)) => list.push(idx),
                    None => groups.push((e.msg, vec![idx])),
                }
            }
            let mut again = Vec::new();
            for (_, group) in groups {
                again.extend(self.plan(&entities, group.into_iter()));
            }
            for (members, result) in self.run(env, &entities, again).await {
                self.apply(
                    env,
                    &entities,
                    &members,
                    result,
                    &mut verdicts,
                    &mut tallies,
                );
            }
        }

        for (verdict, tally) in verdicts.iter_mut().zip(tallies) {
            if tally.entities > 0
                && tally.whole_refused == tally.entities
                && verdict.whole.is_none()
            {
                verdict.whole = tally.whole_text;
            } else {
                verdict.refused.extend(tally.whole_members);
            }
        }
        if local_refusals > 0 {
            env.metrics
                .incr_counter(&metrics::refused_key(env.label), local_refusals);
        }
        verdicts
    }

    /// Packs `subset` (entity indexes, in order) into requests under both caps, starting a new
    /// request rather than carrying one entity id twice.
    fn plan(&self, entities: &[Entity], subset: impl Iterator<Item = usize>) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        let mut current_len = 2usize;
        let mut ids: HashSet<&str> = HashSet::new();
        for idx in subset {
            let Some(e) = entities.get(idx) else { continue };
            let sep = usize::from(!current.is_empty());
            let dup = e.id.as_deref().is_some_and(|id| ids.contains(id));
            if !current.is_empty()
                && (current.len() >= self.max_count
                    || current_len.saturating_add(e.bytes.len() + sep) > self.max_bytes
                    || dup)
            {
                out.push(std::mem::take(&mut current));
                current_len = 2;
                ids.clear();
            }
            let sep = usize::from(!current.is_empty());
            current_len = current_len.saturating_add(e.bytes.len() + sep);
            if let Some(id) = e.id.as_deref() {
                ids.insert(id);
            }
            current.push(idx);
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }

    async fn run(
        &self,
        env: &CallEnv<'_>,
        entities: &[Entity],
        requests: Vec<Vec<usize>>,
    ) -> Vec<(Vec<usize>, Result<Answer, Failure>)> {
        futures::stream::iter(requests.into_iter().map(|members| {
            let body = request_body(entities, &members);
            async move {
                let result = self
                    .client
                    .call(env, Method::POST, &self.url, Some(body))
                    .await;
                (members, result)
            }
        }))
        .buffered(self.max_in_flight)
        .collect()
        .await
    }

    fn apply(
        &self,
        env: &CallEnv<'_>,
        entities: &[Entity],
        members: &[usize],
        result: Result<Answer, Failure>,
        verdicts: &mut [Verdict],
        tallies: &mut [Tally],
    ) {
        match result {
            Ok(answer) if answer.status == 207 => {
                apply_multi_status(env, &answer, entities, members, verdicts)
            }
            Ok(_) => {
                for e in members.iter().filter_map(|i| entities.get(*i)) {
                    if let Some(v) = verdicts.get_mut(e.msg) {
                        v.written += 1;
                    }
                }
                env.metrics
                    .incr_counter(&metrics::written_key(env.label), members.len() as u64);
            }
            Err(Failure::Refused(answer)) => {
                let text = problem_text(&answer);
                for e in members.iter().filter_map(|i| entities.get(*i)) {
                    if let Some(t) = tallies.get_mut(e.msg) {
                        t.whole_refused += 1;
                        t.whole_text.get_or_insert_with(|| text.clone());
                        t.whole_members.push(Refusal {
                            id: Value::String(e.id.clone().unwrap_or_default()),
                            message: Value::String(text.clone()),
                        });
                    }
                }
                env.metrics
                    .incr_counter(&metrics::refused_key(env.label), members.len() as u64);
            }
            Err(failure) => {
                let text = failure.text();
                for m in messages_of(entities, members) {
                    if let Some(v) = verdicts.get_mut(m) {
                        v.failed.get_or_insert_with(|| text.clone());
                    }
                }
            }
        }
    }
}

/// The messages `members` belong to, each once, in order.
fn messages_of(entities: &[Entity], members: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for e in members.iter().filter_map(|i| entities.get(*i)) {
        if !out.contains(&e.msg) {
            out.push(e.msg);
        }
    }
    out
}

/// A 207 `BatchOperationResult`: each of `errors[]` goes to the message whose entity it names;
/// one naming no entity of the request goes to the messages with an id-less entity, else to
/// every message of the request, so a refusal is never lost.
fn apply_multi_status(
    env: &CallEnv<'_>,
    answer: &Answer,
    entities: &[Entity],
    members: &[usize],
    verdicts: &mut [Verdict],
) {
    let errors = match serde_json::from_slice::<JsonValue>(&answer.body) {
        Ok(JsonValue::Object(m)) => match m.get("errors") {
            Some(JsonValue::Array(items)) => items.clone(),
            _ => Vec::new(),
        },
        _ => {
            let text = format!(
                "the broker answered 207 with a body that is no BatchOperationResult: {}",
                answer.error_text
            );
            for m in messages_of(entities, members) {
                if let Some(v) = verdicts.get_mut(m) {
                    v.failed.get_or_insert_with(|| text.clone());
                }
            }
            return;
        }
    };
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for idx in members {
        if let Some(id) = entities.get(*idx).and_then(|e| e.id.as_deref()) {
            by_id.insert(id, *idx);
        }
    }
    let mut refused: HashSet<usize> = HashSet::new();
    for err in &errors {
        let refusal = to_refusal(err);
        let named = err
            .get("entityId")
            .and_then(JsonValue::as_str)
            .and_then(|id| by_id.get(id))
            .copied();
        match named {
            Some(idx) => {
                refused.insert(idx);
                if let Some(v) = entities.get(idx).and_then(|e| verdicts.get_mut(e.msg)) {
                    v.refused.push(refusal);
                }
            }
            None => {
                let idless: Vec<usize> = members
                    .iter()
                    .copied()
                    .filter(|i| entities.get(*i).is_some_and(|e| e.id.is_none()))
                    .collect();
                refused.extend(idless.iter().copied());
                let targets = if idless.is_empty() {
                    messages_of(entities, members)
                } else {
                    messages_of(entities, &idless)
                };
                for m in targets {
                    if let Some(v) = verdicts.get_mut(m) {
                        v.refused.push(refusal.clone());
                    }
                }
            }
        }
    }
    let mut written = 0u64;
    for idx in members {
        if refused.contains(idx) {
            continue;
        }
        if let Some(v) = entities.get(*idx).and_then(|e| verdicts.get_mut(e.msg)) {
            v.written += 1;
            written += 1;
        }
    }
    env.metrics
        .incr_counter(&metrics::written_key(env.label), written);
    env.metrics
        .incr_counter(&metrics::refused_key(env.label), errors.len() as u64);
}

/// `{"id": e.entityId.or(""), "message": e.error.detail.or(e.error.title).or("")}`.
fn to_refusal(e: &JsonValue) -> Refusal {
    let present = |v: Option<&JsonValue>| v.filter(|v| !v.is_null()).cloned();
    let error = e.get("error");
    let id = present(e.get("entityId"))
        .map(Value::from_json_value)
        .unwrap_or_else(|| Value::String(String::new()));
    let message = present(error.and_then(|x| x.get("detail")))
        .or_else(|| present(error.and_then(|x| x.get("title"))))
        .map(Value::from_json_value)
        .unwrap_or_else(|| Value::String(String::new()));
    Refusal { id, message }
}

/// A whole refusal in the broker's words, as the Portal's `refusals` step reads it: the
/// problem's `detail`, else its `title`, else the error text.
fn problem_text(answer: &Answer) -> String {
    let parsed: Option<JsonValue> = serde_json::from_slice(&answer.body).ok();
    let pick = |k: &str| {
        parsed
            .as_ref()
            .and_then(|p| p.get(k))
            .filter(|v| !v.is_null())
            .map(|v| Value::from_json_value(v.clone()).to_string_coerced())
    };
    pick("detail")
        .or_else(|| pick("title"))
        .unwrap_or_else(|| answer.error_text.clone())
}

fn request_body(entities: &[Entity], members: &[usize]) -> Bytes {
    let len = members
        .iter()
        .filter_map(|i| entities.get(*i))
        .map(|e| e.bytes.len() + 1)
        .sum::<usize>()
        + 1;
    let mut body = Vec::with_capacity(len);
    body.push(b'[');
    let mut first = true;
    for e in members.iter().filter_map(|i| entities.get(*i)) {
        if !first {
            body.push(b',');
        }
        first = false;
        body.extend_from_slice(&e.bytes);
    }
    body.push(b']');
    Bytes::from(body)
}

/// The entities of one message: its id (when it has a string one) and its JSON text.
/// Decoded entities of one message: the entity id when it has one, and its JSON bytes.
type Decoded = Vec<(Option<String>, Vec<u8>)>;

fn decode(part: &Part) -> Result<Decoded, String> {
    let value = part
        .as_structured_ref()
        .map_err(|e| format!("the message is not JSON: {e}"))?;
    let items: Vec<Value> = match value {
        Value::Object(_) => vec![value],
        Value::Array(items) => items.iter().cloned().collect(),
        other => {
            return Err(format!(
                "the message is a {} where an NGSI-LD entity or an array of entities is expected",
                other.type_name()
            ))
        }
    };
    items
        .into_iter()
        .enumerate()
        .map(|(n, item)| match &item {
            Value::Object(m) => {
                let id = match m.get("id") {
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                };
                let text = item.to_go_json_with(false, None).ok_or_else(|| {
                    format!("entity {n} holds a number JSON cannot carry (NaN or infinity)")
                })?;
                Ok((id, text.into_bytes()))
            }
            other => Err(format!(
                "item {n} of the message is a {} where an NGSI-LD entity is expected",
                other.type_name()
            )),
        })
        .collect()
}

/// The value of `jc_refused_<label>`: the refusals as the Portal's `result_map` formats them
/// (`format_json(no_indent: true)`: compact, keys sorted, HTML escaped).
pub fn refused_json(refused: &[Refusal]) -> String {
    let items: Vec<Value> = refused
        .iter()
        .map(|r| {
            let mut m = BTreeMap::new();
            m.insert("id".to_string(), r.id.clone());
            m.insert("message".to_string(), r.message.clone());
            Value::Object(m.into())
        })
        .collect();
    Value::Array(items.into()).to_go_json()
}

/// The error a message of the output carries, or `None` when it was written whole.
pub fn error_text(v: &Verdict) -> Option<String> {
    if let Some(f) = &v.failed {
        return Some(f.clone());
    }
    if let Some(w) = &v.whole {
        return Some(format!("the broker refused the write: {w}"));
    }
    if v.refused.is_empty() {
        return None;
    }
    let shown: Vec<String> = v
        .refused
        .iter()
        .take(10)
        .map(|r| {
            format!(
                "{}: {}",
                r.id.to_string_coerced(),
                r.message.to_string_coerced()
            )
        })
        .collect();
    let more = if v.refused.len() > 10 { "; ..." } else { "" };
    Some(format!(
        "the broker refused {} of the message's entities: {}{more}",
        v.refused.len(),
        shown.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_read_the_problem_as_the_portal_does() {
        let r = to_refusal(
            &serde_json::json!({"entityId": "urn:a", "error": {"title": "T", "detail": "D"}}),
        );
        assert_eq!(r.id, Value::String("urn:a".into()));
        assert_eq!(r.message, Value::String("D".into()));
        let r = to_refusal(
            &serde_json::json!({"entityId": null, "error": {"title": "T", "detail": null}}),
        );
        assert_eq!(r.id, Value::String(String::new()));
        assert_eq!(r.message, Value::String("T".into()));
        let r = to_refusal(&serde_json::json!("not an object"));
        assert_eq!(r.message, Value::String(String::new()));
        assert_eq!(
            refused_json(&[Refusal {
                id: Value::String("urn:a".into()),
                message: Value::String("<b>&".into())
            }]),
            // `format_json` escapes HTML like Go's encoding/json: `<`, `>` and `&` as \u escapes.
            r#"[{"id":"urn:a","message":"\u003cb\u003e\u0026"}]"#
        );
        assert_eq!(refused_json(&[]), "[]");
    }

    #[test]
    fn decode_keeps_values_and_refuses_non_entities() {
        let part = Part::from_str(
            r#"[{"id":"urn:a","type":"T","v":{"type":"Property","value":1.50,"unitCode":"CEL","observedAt":"2026-01-01T00:00:00Z"}}]"#,
        );
        let list = decode(&part).expect("decodes");
        assert_eq!(list.len(), 1);
        let text = String::from_utf8(list[0].1.clone()).expect("utf8");
        assert!(text.contains("1.50"), "{text}");
        assert!(text.contains("\"unitCode\":\"CEL\""), "{text}");
        assert!(!text.contains("@context"), "{text}");
        assert_eq!(list[0].0.as_deref(), Some("urn:a"));
        assert!(decode(&Part::from_str("\"x\"")).is_err());
        assert!(decode(&Part::from_str("[1]")).is_err());
        assert!(decode(&Part::from_str("not json")).is_err());
        assert!(decode(&Part::from_str("[]")).expect("empty").is_empty());
    }
}
