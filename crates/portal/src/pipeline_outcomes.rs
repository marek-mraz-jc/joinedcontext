//! What the pipeline runner did not write, per pipeline (PL-61, ADR-N-034).
//!
//! The runner's validation stage refuses a record that breaks its space's model and posts it to
//! the Portal's internal rejected route; this is where it is kept, with the rule it broke and the
//! time, so a steward sees it on the pipeline's page and replays it after a fix. Durable with a
//! database and in memory without one, like the drafts. Bounded per pipeline: the newest
//! [`KEPT`] stay, so a feed that breaks the model on every message cannot fill the database.
//!
//! A record is masked before it is stored: a value under a secret's key name (the list a write is
//! refused by, MF-24) and a value shaped like a credential are replaced. A pipeline's source can
//! carry anything, and the rejected list is read by whoever may read the pipeline.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::RwLock;

/// How many rejected records one pipeline keeps (PL-61).
pub const KEPT: usize = 1000;

/// What a masked value reads as.
pub const MASK: &str = "[MASKED]";

/// One record the runner did not write.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    /// Its place in the list, which "Retry after fix" names.
    pub id: i64,
    /// When the runner refused it, RFC 3339.
    pub at: String,
    /// The record as the mapping produced it, secrets masked.
    pub record: Value,
    /// The constraint it broke (a SHACL component, `type` or `id`, PL-59).
    pub rule: String,
    /// The attribute the constraint is about.
    pub path: String,
    /// What is wrong, in words.
    pub message: String,
    /// The step of `spec.steps` the record failed at, when it failed in a step and not in the
    /// validation stage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<i32>,
    /// The run whose log holds the record's line (PL-62); none for a record a failed replay put
    /// back, or one kept before runs were named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

type Key = (String, String);

/// One row of `pipeline_rejected` as Postgres answers it.
type Row = (
    i64,
    time::OffsetDateTime,
    Value,
    String,
    String,
    String,
    Option<i32>,
    Option<String>,
);

/// Why the stage refused a record: the constraint, where, in words, and the step it failed at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reason {
    pub rule: String,
    pub path: String,
    pub message: String,
    pub step: Option<i32>,
    pub run: Option<String>,
}

/// The rejected records of every pipeline.
pub struct RejectedStore {
    inner: Inner,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<HashMap<Key, (i64, Vec<Rejected>)>>),
}

impl RejectedStore {
    /// Durable with a database, in memory without one.
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new(HashMap::new())),
            },
        }
    }

    /// Keeps one refused record, masked, and drops the oldest beyond [`KEPT`].
    pub async fn reject(
        &self,
        project: &str,
        pipeline: &str,
        record: &Value,
        reason: &Reason,
    ) -> Result<(), sqlx::Error> {
        let Reason {
            rule,
            path,
            message,
            step,
            run,
        } = reason;
        let step = *step;
        let record = mask(record);
        match &self.inner {
            Inner::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO pipeline_rejected (project, pipeline, record, rule, path, message, step, run) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(project)
                .bind(pipeline)
                .bind(&record)
                .bind(rule)
                .bind(path)
                .bind(message)
                .bind(step)
                .bind(run)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "DELETE FROM pipeline_rejected WHERE project = $1 AND pipeline = $2 AND id NOT IN \
                     (SELECT id FROM pipeline_rejected WHERE project = $1 AND pipeline = $2 \
                      ORDER BY id DESC LIMIT $3)",
                )
                .bind(project)
                .bind(pipeline)
                .bind(KEPT as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await
            }
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let (next, list) = map
                    .entry((project.to_owned(), pipeline.to_owned()))
                    .or_default();
                *next += 1;
                list.push(Rejected {
                    id: *next,
                    at: now(),
                    record,
                    rule: rule.clone(),
                    path: path.clone(),
                    message: message.clone(),
                    step,
                    run: run.clone(),
                });
                let over = list.len().saturating_sub(KEPT);
                list.drain(..over);
                Ok(())
            }
        }
    }

    /// The pipeline's rejected records, newest first, at most `limit`, older than `before` when
    /// it is given (paging by id).
    pub async fn list(
        &self,
        project: &str,
        pipeline: &str,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<Rejected>, sqlx::Error> {
        let limit = limit.clamp(1, KEPT);
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<Row> = sqlx::query_as(
                    "SELECT id, at, record, rule, path, message, step, run FROM pipeline_rejected \
                         WHERE project = $1 AND pipeline = $2 AND ($3::bigint IS NULL OR id < $3) \
                         ORDER BY id DESC LIMIT $4",
                )
                .bind(project)
                .bind(pipeline)
                .bind(before)
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(id, at, record, rule, path, message, step, run)| Rejected {
                            id,
                            at: at
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                            record,
                            rule,
                            path,
                            message,
                            step,
                            run,
                        },
                    )
                    .collect())
            }
            Inner::Memory(map) => {
                let map = map.read().unwrap_or_else(|e| e.into_inner());
                Ok(map
                    .get(&(project.to_owned(), pipeline.to_owned()))
                    .map(|(_, list)| {
                        list.iter()
                            .rev()
                            .filter(|r| before.is_none_or(|before| r.id < before))
                            .take(limit)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default())
            }
        }
    }

    /// How many records the pipeline holds refused.
    pub async fn count(&self, project: &str, pipeline: &str) -> Result<u64, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let (count,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM pipeline_rejected WHERE project = $1 AND pipeline = $2",
                )
                .bind(project)
                .bind(pipeline)
                .fetch_one(pool)
                .await?;
                Ok(u64::try_from(count).unwrap_or_default())
            }
            Inner::Memory(map) => Ok(map
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(project.to_owned(), pipeline.to_owned()))
                .map_or(0, |(_, list)| list.len() as u64)),
        }
    }

    /// Drops the list of every pipeline a project `loaded` no longer holds: a deleted pipeline's
    /// refused records go with it. `loaded` maps each project a completed sync read to the
    /// pipelines it holds, so a paused pipeline keeps its list, and a project this sync did not
    /// read (its repository did not stage) is not touched at all.
    pub async fn forget_gone(
        &self,
        loaded: &BTreeMap<String, HashSet<String>>,
    ) -> Result<(), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let scope: Vec<&str> = loaded.keys().map(String::as_str).collect();
                let (projects, pipelines): (Vec<&str>, Vec<&str>) = loaded
                    .iter()
                    .flat_map(|(project, names)| {
                        names.iter().map(move |n| (project.as_str(), n.as_str()))
                    })
                    .unzip();
                sqlx::query(
                    "DELETE FROM pipeline_rejected r WHERE r.project = ANY($1::text[]) AND NOT EXISTS (SELECT 1 FROM \
                     UNNEST($2::text[], $3::text[]) AS k(project, pipeline) \
                     WHERE k.project = r.project AND k.pipeline = r.pipeline)",
                )
                .bind(&scope)
                .bind(&projects)
                .bind(&pipelines)
                .execute(pool)
                .await?;
                Ok(())
            }
            Inner::Memory(map) => {
                map.write()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|(project, pipeline), _| {
                        loaded
                            .get(project)
                            .is_none_or(|names| names.contains(pipeline))
                    });
                Ok(())
            }
        }
    }

    /// Takes the named records out of the list, for a replay: a record that still breaks the
    /// model comes back through the runner with its rule.
    pub async fn take(
        &self,
        project: &str,
        pipeline: &str,
        ids: &[i64],
    ) -> Result<Vec<Rejected>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<(i64, Value)> = sqlx::query_as(
                    "DELETE FROM pipeline_rejected WHERE project = $1 AND pipeline = $2 AND id = ANY($3) \
                     RETURNING id, record",
                )
                .bind(project)
                .bind(pipeline)
                .bind(ids)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, record)| Rejected {
                        id,
                        at: String::new(),
                        record,
                        rule: String::new(),
                        path: String::new(),
                        message: String::new(),
                        step: None,
                        run: None,
                    })
                    .collect())
            }
            Inner::Memory(map) => {
                let mut map = map.write().unwrap_or_else(|e| e.into_inner());
                let Some((_, list)) = map.get_mut(&(project.to_owned(), pipeline.to_owned()))
                else {
                    return Ok(Vec::new());
                };
                let (taken, kept): (Vec<Rejected>, Vec<Rejected>) =
                    list.drain(..).partition(|r| ids.contains(&r.id));
                *list = kept;
                Ok(taken)
            }
        }
    }
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The record with every secret masked: a value under a secret's key name, at any depth, and a
/// string shaped like a credential wherever it sits.
pub fn mask(record: &Value) -> Value {
    match record {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let secret_key = crate::secret_scan::secret_name(key)
                        && !value.is_object()
                        && !value.is_array()
                        && !value.is_null()
                        // `${VAR}` names a secret the runner injects and holds none (PL-16).
                        && !value
                            .as_str()
                            .is_some_and(crate::api::mutate::is_variable_reference);
                    let masked = if secret_key {
                        Value::String(MASK.to_owned())
                    } else {
                        mask(value)
                    };
                    (key.clone(), masked)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(mask).collect()),
        Value::String(text) if credential_shaped(text) => Value::String(MASK.to_owned()),
        other => other.clone(),
    }
}

/// Free text with every JSON object in it (an HTTP answer's body a runner quotes, such as the
/// gateway's Problem Details after `unexpected response code (403): 403 Forbidden, Error:`) told
/// in its own words: its `detail`, `title`, `message` or `error`, else left out. A body the log
/// cut short is left out from its `{` on (T-5734).
pub fn json_in_words(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let (before, body) = rest.split_at(at);
        let mut values = serde_json::Deserializer::from_str(body).into_iter::<Value>();
        let words = match values.next() {
            Some(Ok(Value::Object(object))) => ["detail", "title", "message", "error"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str))
                .map(str::to_owned),
            Some(Err(error)) if error.is_eof() => {
                out.push_str(before.trim_end_matches([' ', ',', ':']));
                return out;
            }
            _ => {
                out.push_str(before);
                out.push('{');
                rest = &body[1..];
                continue;
            }
        };
        match words {
            Some(words) => {
                out.push_str(before);
                out.push_str(&words);
            }
            None => out.push_str(before.trim_end_matches([' ', ',', ':'])),
        }
        rest = &body[values.byte_offset()..];
    }
    out.push_str(rest);
    out
}

/// Free text with every word shaped like a credential masked, and the word after a `Bearer` or
/// `Basic` scheme too: a runner's error or a record id can quote whatever the source held.
pub fn mask_text(text: &str) -> String {
    let mut previous = "";
    text.split(' ')
        .map(|word| {
            let scheme =
                previous.eq_ignore_ascii_case("bearer") || previous.eq_ignore_ascii_case("basic");
            previous = word;
            if scheme || credential_shaped(word) {
                MASK
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a string reads like a credential: a bearer or basic header, a JWT, a PEM block, a
/// URL with a password in it, or a token with a well-known issuer prefix.
pub fn credential_shaped(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("bearer ") || lower.starts_with("basic ") {
        return true;
    }
    if trimmed.contains("-----BEGIN") {
        return true;
    }
    // A JWT: three base64url segments, the first a JSON header (`eyJ`).
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() == 3
        && parts[0].starts_with("eyJ")
        && parts.iter().all(|part| {
            part.len() >= 4
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '=')
        })
    {
        return true;
    }
    // `scheme://user:password@host`; a whole `${VAR}` as the password names the secret the
    // runner injects and holds none.
    if let Some((_, rest)) = trimmed.split_once("://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority
            .rsplit_once('@')
            .and_then(|(userinfo, _)| userinfo.split_once(':'))
            .is_some_and(|(_, password)| !crate::api::mutate::is_variable_reference(password))
        {
            return true;
        }
    }
    // The platform's own API key: `jc_`, the key id in 16 hex digits, `_`, its URL-safe secret
    // (API/01 §9). An operation name (`jc_resource_propose`) has no hex id and stays readable.
    if let Some((id, secret)) = trimmed
        .strip_prefix("jc_")
        .and_then(|rest| rest.split_once('_'))
    {
        if id.len() == 16
            && id.bytes().all(|b| b.is_ascii_hexdigit())
            && secret.len() >= 16
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return true;
        }
    }
    const PREFIXES: [&str; 8] = [
        "ghp_",
        "gho_",
        "github_pat_",
        "glpat-",
        "sk-",
        "xoxb-",
        "xoxp-",
        "AKIA",
    ];
    PREFIXES.iter().any(|prefix| {
        trimmed.starts_with(prefix)
            && trimmed.len() >= prefix.len() + 16
            && !trimmed.contains(char::is_whitespace)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-5734: a runner's error quoting the gateway's Problem Details reads as its words.
    #[test]
    fn a_json_body_in_a_message_reads_as_its_words() {
        let refused = r#"the write failed: unexpected response code (403): 403 Forbidden, Error: {"type":"about:blank","title":"Forbidden","status":403,"detail":"Access Denied by Policy"}"#;
        assert_eq!(
            json_in_words(refused),
            "the write failed: unexpected response code (403): 403 Forbidden, Error: Access Denied by Policy"
        );
        assert_eq!(
            json_in_words(r#"refused: {"status":403}; next try 22:30"#),
            "refused; next try 22:30"
        );
        assert_eq!(
            json_in_words(r#"403 Forbidden, Error: {"type":"about:blank","detai"#),
            "403 Forbidden, Error"
        );
        assert_eq!(json_in_words("a {brace} stays"), "a {brace} stays");
        assert_eq!(json_in_words("{"), "");
    }

    /// F-R2POR4-006: a source spells a secret's key in any case and with `_` or `-`; each one
    /// is masked in a rejected record and in a sample, the way the operations door refuses it.
    #[test]
    fn a_secret_key_in_any_spelling_is_masked() {
        let record =
            json!({ "Password": "x1", "API_KEY": "y2", "client-secret": "z3", "name": "n" });
        let masked = mask(&record);
        for key in ["Password", "API_KEY", "client-secret"] {
            assert_eq!(masked[key], MASK, "{key}");
        }
        assert_eq!(masked["name"], "n");
        let sampled = crate::pipeline_samples::redacted(record, &[]);
        for key in ["Password", "API_KEY", "client-secret"] {
            assert_eq!(sampled[key], "•••", "{key}");
        }
        assert_eq!(sampled["name"], "n");
    }

    /// Built from parts, so no literal in this file reads as a credential to a scanner.
    fn jwt() -> String {
        [
            "eyJhbGciOiJIUzI1NiJ9",
            "eyJzdWIiOiJ4In0",
            "c2lnbmF0dXJlLXBhcnQ",
        ]
        .join(".")
    }

    #[test]
    fn a_secret_key_and_a_credential_shaped_value_are_masked_and_nothing_else() {
        let token = format!("{}{}", "ghp_", "a".repeat(36));
        let record = json!({
            "id": "urn:ngsi-ld:Station:hel.fi:helsinki:s-1",
            "password": "hunter2hunter2",
            "note": { "type": "Property", "value": format!("Bearer {}", jwt()) },
            "feed": { "type": "Property", "value": format!("https://user:{}@feed.example/x", "pw") },
            "list": [token, "ordinary text"],
            "token": { "secretRef": { "name": "feed" } },
            "pm10": { "type": "Property", "value": 18.4 }
        });
        let masked = mask(&record);
        assert_eq!(masked["password"], MASK);
        assert_eq!(masked["note"]["value"], MASK);
        assert_eq!(masked["feed"]["value"], MASK);
        assert_eq!(masked["list"][0], MASK);
        assert_eq!(masked["list"][1], "ordinary text");
        assert_eq!(
            masked["token"],
            json!({ "secretRef": { "name": "feed" } }),
            "a reference is no secret"
        );
        assert_eq!(masked["pm10"]["value"], 18.4);
        assert_eq!(masked["id"], record["id"]);
        assert!(!masked.to_string().contains("hunter2"));
        // T-5971: a manifest's `${VAR}` names the secret and holds none.
        let named = json!({ "password": "${FEED_PASSWORD}" });
        assert_eq!(mask(&named), named);
        // T-6130: so does a `${VAR}` in a URL's password place; a written-out one is masked.
        assert!(!credential_shaped("nats://demo:${FEED_PASSWORD}@nats:4222"));
        assert!(credential_shaped("nats://demo:s3cret@nats:4222"));
        assert!(credential_shaped("nats://demo:${FEED_PASSWORD}x@nats:4222"));
        assert!(credential_shaped(
            "nats://demo:${FEED_PASSWORD}@u:s3cret@nats:4222"
        ));
    }

    /// T-3272: the platform's own API key is a credential; an operation name is not.
    #[test]
    fn the_platforms_own_api_key_is_masked_and_an_operation_name_is_not() {
        let key = format!("{}{}_{}", "jc_", "3f9c2a7b1d4e8f06", "Zm9vYmFyYmF6cXV4MTIz");
        assert!(credential_shaped(&key));
        assert_eq!(
            mask_text(&format!("my key {key} fails")),
            format!("my key {MASK} fails")
        );
        for readable in [
            "jc_resource_propose",
            "jc_pipeline_test",
            "jc_3f9c2a7b1d4e8f06_short",
            "jc_nothex0000000000_Zm9vYmFyYmF6cXV4MTIz",
        ] {
            assert!(!credential_shaped(readable), "{readable}");
        }
    }

    #[test]
    fn ordinary_values_are_not_credentials() {
        for text in [
            "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:1",
            "https://hel.fi/api/x?y=1",
            "sk",
            "a.b.c",
            "2026-09-01T06:00:00Z",
            "Mannerheimintie 1",
        ] {
            assert!(!credential_shaped(text), "{text}");
        }
    }

    fn reason(rule: &str, path: &str, step: Option<i32>) -> Reason {
        Reason {
            rule: rule.into(),
            path: path.into(),
            message: format!("{path} breaks {rule}"),
            step,
            run: Some("2026-10-07T08:00:00Z".into()),
        }
    }

    #[tokio::test]
    async fn the_list_is_per_pipeline_newest_first_bounded_and_paged() {
        let store = RejectedStore::new(None);
        for n in 0..(KEPT + 5) {
            store
                .reject(
                    "p",
                    "a",
                    &json!({ "n": n }),
                    &reason("sh:datatype", "pm10", None),
                )
                .await
                .expect("kept");
        }
        store
            .reject("p", "b", &json!({ "n": 0 }), &reason("id", "id", Some(2)))
            .await
            .expect("kept");
        assert_eq!(store.count("p", "a").await.expect("count"), KEPT as u64);
        assert_eq!(store.count("p", "b").await.expect("count"), 1);
        assert_eq!(store.count("q", "a").await.expect("count"), 0);

        let page = store.list("p", "a", 10, None).await.expect("a page");
        assert_eq!(page.len(), 10);
        assert_eq!(page[0].record["n"], json!(KEPT + 4), "newest first");
        let next = store
            .list("p", "a", 10, Some(page[9].id))
            .await
            .expect("the next page");
        assert!(next[0].id < page[9].id);
        let oldest = store.list("p", "a", KEPT, None).await.expect("all");
        assert_eq!(
            oldest.last().map(|r| r.record["n"].clone()),
            Some(json!(5)),
            "the oldest five went"
        );

        let taken = store.take("p", "a", &[page[0].id]).await.expect("taken");
        assert_eq!(taken.len(), 1);
        assert_eq!(store.count("p", "a").await.expect("count"), KEPT as u64 - 1);
        assert!(
            store
                .take("p", "b", &[page[0].id])
                .await
                .expect("none")
                .is_empty(),
            "ids are per pipeline"
        );
    }
}
