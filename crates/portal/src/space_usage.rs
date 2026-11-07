//! How much a space holds, as the broker that holds it counts it (API/01 §29, T-2889, T-2996).
//!
//! The broker keeps a space's entities in its tenant, named by the space's URN segment (PF-84).
//! The Portal reads the count from the broker's admin route `GET /q/tenants/{tenant}` with the
//! broker's admin token, the same bearer its view writer sends (T-3640). The NGSI-LD count query
//! (`GET /ngsi-ld/v1/entities?local=true&count=true&limit=0`) is not usable here: the
//! `joinedcontext` policy engine denies a caller without subject headers on every query clause,
//! so on dev it answered 403 for every space (T-3983). A count is a scan on the broker's side,
//! so each answer is kept: these are dashboard numbers, not per-request work. A small space is
//! measured again on a read after `FRESH`; a large one once an hour (`KEPT`), by the health
//! probes' sweep over every space (T-6208), or when a reader asks for a recalculation.
//! The storage size in bytes comes in the same answer, measured by the broker (`bytes`, T-2890).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The problem type of a tenant the broker has never written to (`GET /q/tenants/{tenant}`).
const NO_TENANT: &str = "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound";
use utoipa::ToSchema;

/// How long one broker answer for a small space is shown before a read asks again.
const FRESH: chrono::Duration = chrono::Duration::minutes(5);
/// How long the answer for a large space is kept: it is measured once an hour (T-6208).
const KEPT: chrono::Duration = chrono::Duration::hours(1);
/// A space whose current state and history hold fewer rows than this is small: measuring it
/// costs the broker little, so a read past [`FRESH`] measures it again.
const SMALL_ROWS: u64 = 10_000;
/// A recalculation takes an answer this young as its own: concurrent and repeated clicks share
/// one broker read (T-6208).
const RECALCULATED: chrono::Duration = chrono::Duration::seconds(30);
/// How long a kept answer still stands in for a broker that did not answer: a count a few
/// minutes old, with its `observedAt`, says more than a 503 (T-4825). It outlasts [`KEPT`], so a
/// failed hourly measurement keeps the last one on the page.
const STALE: chrono::Duration = chrono::Duration::hours(3);
/// How long the Portal waits for the broker. Its first answer after a five-minute gap counts the
/// tenant's history and measures its size under two 5 s statement bounds (Antares
/// `SIZE_SCAN_TIMEOUT`), so 5 s here cut every Helsinki count off on dev (T-4825).
const TIMEOUT: Duration = Duration::from_secs(20);

/// What one space holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceUsage {
    /// The entities of the space's tenant, as the broker counts them.
    pub entities: u64,
    /// The `ContextBroker` the space lives on, which counted it (PF-111).
    pub broker: String,
    /// When the broker answered.
    #[schema(value_type = String, format = DateTime)]
    pub observed_at: DateTime<Utc>,
    /// When the broker measured the size: its own `measuredAt` (it keeps a size five minutes),
    /// else when it answered (T-6208).
    #[schema(value_type = String, format = DateTime)]
    pub measured_at: DateTime<Utc>,
    /// The storage the space takes, measured by the broker; absent when the broker cannot
    /// measure it or holds no tenant for the space yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<SpaceBytes>,
    /// The space's history: its temporal attribute instances, as the broker counts them; absent
    /// when the broker could neither count nor estimate them (T-6085).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr_instances: Option<u64>,
    /// True when the broker took the history count and size from its database's statistics,
    /// because the history was too long to count within its bound (T-6085).
    pub estimated: bool,
}

/// A space's storage in bytes, as the broker measures it in its store (T-2890).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceBytes {
    /// The current state: every Entity of the space.
    pub entities: u64,
    /// The history: the space's temporal attribute instances.
    pub attr_instances: u64,
    /// The current state by each Entity's first type (expanded IRI); sums to `entities`.
    pub by_type: BTreeMap<String, u64>,
}

/// Asks the broker for a space's counts and keeps each answer: five minutes for a small space,
/// an hour for a large one.
pub struct Counter {
    broker: Option<String>,
    template: Option<String>,
    token_file: Option<PathBuf>,
    http: reqwest::Client,
    cache: RwLock<HashMap<String, SpaceUsage>>,
    /// One lock per broker tenant: a second ask waits for the measurement in flight and takes its
    /// answer, so the broker counts a space once however many readers ask.
    measuring: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    surfaces: RwLock<Option<(DateTime<Utc>, BTreeSet<String>)>>,
}

impl Counter {
    /// `broker` is `JC_PORTAL_BROKER_URL` and `token_file` `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE`;
    /// without either every read answers why.
    pub fn new(broker: Option<String>, token_file: Option<PathBuf>) -> Self {
        Self {
            broker: broker.map(|url| url.trim_end_matches('/').to_owned()),
            template: None,
            token_file,
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            cache: RwLock::default(),
            measuring: Mutex::default(),
            surfaces: RwLock::default(),
        }
    }

    /// Counts a space on a broker other than `main` at `template` (`JC_PORTAL_BROKER_URL_TEMPLATE`).
    pub fn with_broker_template(mut self, template: Option<String>) -> Self {
        self.template = template;
        self
    }

    /// The usage of the space whose broker tenant is `tenant` (its URN segment, PF-84) on the
    /// broker named `broker` (PF-111, empty when its `brokerRef` resolves to none). `tenant`
    /// must already be a DNS-1123 name: it becomes a path segment.
    pub async fn usage(&self, broker_name: &str, tenant: &str) -> Result<SpaceUsage, String> {
        self.measured(broker_name, tenant, |kept| current(kept, Utc::now()), true)
            .await
    }

    /// One broker read now, for a decision that must not stand on a kept number (a broker move,
    /// PF-115): no kept answer is taken, neither before the read nor when the broker is away.
    pub async fn measure_now(&self, broker_name: &str, tenant: &str) -> Result<SpaceUsage, String> {
        self.measured(broker_name, tenant, |_| false, false).await
    }

    /// Measures the space again for a reader who asked (T-6208). An answer younger than
    /// `RECALCULATED` stands: concurrent clicks share one broker read, and repeated ones within
    /// half a minute ask nothing.
    pub async fn recalculate(&self, broker_name: &str, tenant: &str) -> Result<SpaceUsage, String> {
        self.measured(
            broker_name,
            tenant,
            |kept| Utc::now() - kept.observed_at < RECALCULATED,
            true,
        )
        .await
    }

    /// The kept answer while `good` holds for it, else one broker read under the tenant's lock;
    /// with `fallback` a failed read answers the last kept row younger than [`STALE`].
    async fn measured(
        &self,
        broker_name: &str,
        tenant: &str,
        good: impl Fn(&SpaceUsage) -> bool,
        fallback: bool,
    ) -> Result<SpaceUsage, String> {
        let broker = self.reach(broker_name)?;
        let key = format!("{broker}/{tenant}");
        let held = |key: &str| {
            self.cache
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(key)
                .filter(|kept| good(kept))
                .cloned()
        };
        if let Some(kept) = held(&key) {
            return Ok(kept);
        }
        let lock = Arc::clone(
            self.measuring
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(key.clone())
                .or_default(),
        );
        let _measuring = lock.lock().await;
        // the reader before this one may have measured it while this one waited
        if let Some(kept) = held(&key) {
            return Ok(kept);
        }
        self.ask(broker_name, &broker, tenant, key, fallback).await
    }

    /// One broker read of the tenant's counts, kept under `key`.
    async fn ask(
        &self,
        broker_name: &str,
        broker: &str,
        tenant: &str,
        key: String,
        fallback: bool,
    ) -> Result<SpaceUsage, String> {
        let token = self.token()?;
        let response = match self
            .http
            .get(format!("{broker}/q/tenants/{tenant}"))
            .bearer_auth(token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                let reason = unanswered(&err);
                tracing::warn!(tenant, error = ?err.without_url(), "the broker did not answer a space's counts");
                return self
                    .kept(&key, Utc::now())
                    .filter(|_| fallback)
                    .ok_or(reason);
            }
        };
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        // the answer's time, not the ask's: a reader who asked while this read was in flight
        // takes it as a fresh one
        let now = Utc::now();
        let (entities, bytes, attr_instances) = match status {
            status if status.is_success() => (
                count_of(&body)?,
                bytes_of(&body),
                body["counts"]["attrInstances"].as_u64(),
            ),
            // The broker creates a tenant on its first write: no tenant is an empty space. Any
            // other 404 is an address that reaches no broker, never an empty space. An empty
            // space stores nothing: its size is zero, never unknown (T-6204).
            StatusCode::NOT_FOUND if body["type"] == NO_TENANT => (
                0,
                Some(SpaceBytes {
                    entities: 0,
                    attr_instances: 0,
                    by_type: BTreeMap::new(),
                }),
                Some(0),
            ),
            status => {
                tracing::warn!(tenant, %status, "the broker refused a space's counts");
                return self
                    .kept(&key, now)
                    .filter(|_| fallback)
                    .ok_or_else(|| format!("the broker answered {status} for the space's counts"));
            }
        };
        let measured_at = body["measuredAt"]
            .as_str()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map_or(now, |at| at.with_timezone(&Utc).min(now));
        let usage = SpaceUsage {
            entities,
            broker: broker_name.to_owned(),
            observed_at: now,
            measured_at,
            bytes,
            attr_instances,
            estimated: body["estimated"].as_bool().unwrap_or(false),
        };
        self.cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, usage.clone());
        Ok(usage)
    }

    /// The address of broker `broker_name`, or why the Portal has none.
    fn reach(&self, broker_name: &str) -> Result<String, String> {
        self.address(broker_name).ok_or_else(|| match broker_name {
            "" => {
                "the space's brokerRef names no ContextBroker; fix the space's manifest".to_owned()
            }
            jc_core::kinds::IMPLICIT_BROKER => {
                "no broker address is configured (JC_PORTAL_BROKER_URL)".to_owned()
            }
            name => {
                format!("the broker '{name}' has no address; set JC_PORTAL_BROKER_URL_TEMPLATE")
            }
        })
    }

    /// The Attribute names the Entities of `entity_type` hold in `tenant` on broker
    /// `broker_name`, names only (`GET /q/tenants/{tenant}/types/{type}/attributes`, T-6001).
    /// Asked on every call: a plan judges what is stored now. The second member is true when
    /// the broker stopped at its scan ceiling, so the list may miss a name.
    pub async fn stored_attributes(
        &self,
        broker_name: &str,
        tenant: &str,
        entity_type: &str,
    ) -> Result<(Vec<String>, bool), String> {
        let broker = self.reach(broker_name)?;
        let token = self.token()?;
        let mut url = reqwest::Url::parse(&broker)
            .map_err(|_| "the broker address is not a URL".to_owned())?;
        url.path_segments_mut()
            .map_err(|()| "the broker address is not a URL".to_owned())?
            .pop_if_empty()
            .extend(["q", "tenants", tenant, "types", entity_type, "attributes"]);
        let response = self.http.get(url).bearer_auth(token).send().await.map_err(|err| {
            let reason = unanswered(&err);
            tracing::warn!(tenant, error = ?err.without_url(), "the broker did not answer a type's attribute names");
            reason
        })?;
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(tenant, %status, "the broker refused a type's attribute names");
            return Err(format!(
                "the broker answered {status} for the stored attribute names"
            ));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| "the broker's answer was not JSON".to_owned())?;
        let names = body["attributes"]
            .as_array()
            .ok_or_else(|| "the broker's answer names no attributes".to_owned())?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        Ok((names, body["partial"] == true))
    }

    /// The last answer for `key` while it is younger than [`STALE`].
    fn kept(&self, key: &str, now: DateTime<Utc>) -> Option<SpaceUsage> {
        self.cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .filter(|kept| now - kept.observed_at < STALE)
            .cloned()
    }

    /// Whether the Portal has an address for broker `broker_name`.
    pub fn reaches(&self, broker_name: &str) -> bool {
        self.address(broker_name).is_some()
    }

    /// The address of broker `broker_name` (PF-111), `None` when the Portal has none.
    pub fn address(&self, broker_name: &str) -> Option<String> {
        crate::spaces::broker_address(
            broker_name,
            self.broker.as_deref(),
            self.template.as_deref(),
        )
    }

    /// The client every admin call to a broker goes through, with the brokers' timeout.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The brokers' admin token, read on every ask: a rotated Secret needs no restart.
    pub fn token(&self) -> Result<String, String> {
        self.token_file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                "the broker's admin token is not readable (JC_PORTAL_BROKER_ADMIN_TOKEN_FILE)"
                    .to_owned()
            })
    }

    /// The version broker `broker_name` reports on `GET /q/health`, or why it is not up
    /// (API/01 §43). Asked on every call: a health answer kept for minutes would hide an outage.
    pub async fn health(&self, broker_name: &str) -> Result<String, String> {
        let body = self.health_of(broker_name).await?;
        Ok(body["version"].as_str().unwrap_or_default().to_owned())
    }

    /// The surfaces the platform broker mounts, by the names its `GET /q/health` lists under
    /// `surfaces` (T-4484), kept five minutes like a count. `None` when it did not answer, which
    /// is not kept, so the next ask tries again.
    pub async fn surfaces(&self) -> Option<BTreeSet<String>> {
        let now = Utc::now();
        if let Some((_, kept)) = self
            .surfaces
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(at, _)| now - *at < FRESH)
        {
            return Some(kept.clone());
        }
        let body = self.health_of(jc_core::kinds::IMPLICIT_BROKER).await.ok()?;
        let names: BTreeSet<String> = body["surfaces"].as_object()?.keys().cloned().collect();
        *self.surfaces.write().unwrap_or_else(|e| e.into_inner()) = Some((now, names.clone()));
        Some(names)
    }

    async fn health_of(&self, broker_name: &str) -> Result<Value, String> {
        let broker = crate::spaces::broker_address(
            broker_name,
            self.broker.as_deref(),
            self.template.as_deref(),
        )
        .ok_or_else(|| {
            "the Portal has no address for this broker (JC_PORTAL_BROKER_URL, \
             JC_PORTAL_BROKER_URL_TEMPLATE)"
                .to_owned()
        })?;
        let response = self
            .http
            .get(format!("{broker}/q/health"))
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(broker_name, error = %err.without_url(), "a broker did not answer its health");
                "the broker did not answer; try again in a minute".to_owned()
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the broker answered {status} for its health"));
        }
        Ok(response.json().await.unwrap_or(Value::Null))
    }
}

/// Whether a kept answer still stands for a read at `now`: five minutes for a small space, an
/// hour for a large one, whose next measurement the hourly sweep makes (T-6208).
fn current(kept: &SpaceUsage, now: DateTime<Utc>) -> bool {
    let rows = kept
        .entities
        .saturating_add(kept.attr_instances.unwrap_or(u64::MAX));
    let age = now - kept.observed_at;
    age < FRESH || (rows >= SMALL_ROWS && age < KEPT)
}

/// Why a broker sent no answer, in words that say where to look: a timeout is a slow count, a
/// refused connection an address or a NetworkPolicy (T-4825).
fn unanswered(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        format!(
            "the broker took longer than {} s to count the space; try again in a minute",
            TIMEOUT.as_secs()
        )
    } else if err.is_connect() {
        "the Portal cannot reach the broker (its address or a NetworkPolicy); try again in a minute"
            .to_owned()
    } else {
        "the broker did not answer; try again in a minute".to_owned()
    }
}

/// The entity count of a `GET /q/tenants/{tenant}` answer.
fn count_of(body: &Value) -> Result<u64, String> {
    body["counts"]["entities"]
        .as_u64()
        .ok_or_else(|| "the broker's answer holds no readable entity count".to_owned())
}

/// The `bytes` of a `GET /q/tenants/{tenant}` answer; `None` from a broker that does not measure
/// (or answers a shape this Portal does not know), so the page shows no size rather than a zero.
fn bytes_of(body: &Value) -> Option<SpaceBytes> {
    serde_json::from_value(body.get("bytes")?.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn the_count_is_read_from_the_tenants_entity_count() {
        assert_eq!(count_of(&json!({"counts": {"entities": 14232}})), Ok(14232));
        assert_eq!(count_of(&json!({"counts": {"entities": 0}})), Ok(0));
    }

    #[test]
    fn an_answer_without_a_count_is_an_error_and_never_a_zero() {
        assert!(count_of(&Value::Null).is_err());
        assert!(count_of(&json!({"counts": {"entities": -1}})).is_err());
        assert!(count_of(&json!({"counts": {"entities": "many"}})).is_err());
    }

    #[test]
    fn the_size_is_read_from_the_tenants_bytes_and_never_made_up() {
        let body = json!({"counts": {"entities": 2}, "bytes": {
            "entities": 900, "attrInstances": 40, "byType": {"https://x/Room": 600, "https://x/Bridge": 300}}});
        let bytes = bytes_of(&body).expect("bytes");
        assert_eq!((bytes.entities, bytes.attr_instances), (900, 40));
        assert_eq!(bytes.by_type["https://x/Room"], 600);
        // an older broker, a negative or a partial answer: no size, never a zero
        assert_eq!(bytes_of(&json!({"counts": {"entities": 2}})), None);
        assert_eq!(
            bytes_of(&json!({"bytes": {"entities": -1, "attrInstances": 0, "byType": {}}})),
            None
        );
        assert_eq!(bytes_of(&json!({"bytes": {"entities": 9}})), None);
    }

    #[tokio::test]
    async fn without_a_broker_address_the_reason_is_answered() {
        let err = Counter::new(None, None)
            .usage("main", "air")
            .await
            .unwrap_err();
        assert!(err.contains("JC_PORTAL_BROKER_URL"), "{err}");
    }

    /// PF-111: each space is counted on its own broker, with the same tenant name on both.
    #[tokio::test]
    async fn each_space_is_counted_on_its_own_broker() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let main = MockServer::start().await;
        let canary = MockServer::start().await;
        for (server, count) in [(&main, 3), (&canary, 7)] {
            Mock::given(method("GET"))
                .and(path("/q/tenants/air"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"counts": {"entities": count}})),
                )
                .expect(1)
                .mount(server)
                .await;
        }
        let file = std::env::temp_dir().join(format!("usage-token-{}", std::process::id()));
        std::fs::write(&file, "t").expect("write the token");
        let counter = Counter::new(Some(main.uri()), Some(file.clone()))
            .with_broker_template(Some(canary.uri().replace("127.0.0.1", "{broker}")));
        let on_main = counter.usage("main", "air").await;
        let on_canary = counter.usage("127.0.0.1", "air").await;
        let _ = std::fs::remove_file(&file);
        assert_eq!(on_main.map(|u| u.entities), Ok(3));
        assert_eq!(on_canary.map(|u| u.entities), Ok(7));
    }

    /// T-6001: a type's stored names come from the admin route with the admin token; a broker
    /// that does not answer, or answers no list, is an error and never an empty list.
    #[tokio::test]
    async fn stored_attribute_names_are_read_with_the_admin_token_and_never_made_up() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air/types/Counter/attributes"))
            .and(header("authorization", "Bearer t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"tenant": "air", "type": "Counter", "attributes": ["count", "legacy"], "partial": false}),
            ))
            .mount(&broker)
            .await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air/types/Odd/attributes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tenant": "air"})))
            .mount(&broker)
            .await;
        let file = token_file("attrs");
        let counter = Counter::new(Some(broker.uri()), Some(file.clone()));
        let got = counter.stored_attributes("main", "air", "Counter").await;
        let odd = counter.stored_attributes("main", "air", "Odd").await;
        let refused = counter.stored_attributes("main", "air", "Gone").await;
        let down = Counter::new(Some("http://127.0.0.1:1".to_owned()), Some(file.clone()))
            .stored_attributes("main", "air", "Counter")
            .await;
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            got,
            Ok((vec!["count".to_owned(), "legacy".to_owned()], false))
        );
        assert!(odd.is_err(), "{odd:?}");
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("404")),
            "{refused:?}"
        );
        assert!(
            down.as_ref().is_err_and(|e| e.contains("cannot reach")),
            "{down:?}"
        );
    }

    fn token_file(name: &str) -> PathBuf {
        let file = std::env::temp_dir().join(format!("usage-{name}-{}", std::process::id()));
        std::fs::write(&file, "t").expect("write the token");
        file
    }

    /// T-4825: a count slower than the Portal waits says so, and a refused connection names
    /// the address and the NetworkPolicy, never one "did not answer" for both.
    #[tokio::test]
    async fn a_slow_count_and_an_unreachable_broker_say_where_to_look() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let slow = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"counts": {"entities": 1}}))
                    .set_delay(Duration::from_millis(500)),
            )
            .mount(&slow)
            .await;
        let file = token_file("slow");
        let mut counter = Counter::new(Some(slow.uri()), Some(file.clone()));
        counter.http = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("client");
        let late = counter.usage("main", "air").await.unwrap_err();
        let mut closed = Counter::new(Some("http://127.0.0.1:9".into()), Some(file.clone()));
        closed.http = counter.http.clone();
        let refused = closed.usage("main", "air").await.unwrap_err();
        let _ = std::fs::remove_file(&file);
        assert!(late.contains("took longer than"), "{late}");
        assert!(refused.contains("NetworkPolicy"), "{refused}");
    }

    /// T-4825: the last count stands in, with its own `observedAt`, while the broker is away.
    #[tokio::test]
    async fn the_history_count_and_its_estimate_flag_are_carried_from_the_broker() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(path("/q/tenants/air"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "counts": {"entities": 3, "attrInstances": 240000},
                "bytes": {"entities": 900, "attrInstances": 96000000, "byType": {}},
                "estimated": true,
            })))
            .mount(&broker)
            .await;
        Mock::given(path("/q/tenants/old"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"counts": {"entities": 1}})),
            )
            .mount(&broker)
            .await;
        let counter = Counter::new(Some(broker.uri()), Some(token_file("history")));
        let air = counter.usage("main", "air").await.expect("air");
        assert_eq!((air.attr_instances, air.estimated), (Some(240_000), true));
        assert_eq!(air.bytes.map(|b| b.attr_instances), Some(96_000_000));
        // a broker before T-6085 answers neither: no history count, never a made-up 0
        let old = counter.usage("main", "old").await.expect("old");
        assert_eq!((old.attr_instances, old.estimated), (None, false));
    }

    /// T-6204: a space whose tenant the broker never created is empty, size included.
    #[tokio::test]
    async fn a_space_without_a_tenant_answers_zero_bytes() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(path("/q/tenants/fresh"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"type": NO_TENANT})))
            .mount(&broker)
            .await;
        let counter = Counter::new(Some(broker.uri()), Some(token_file("fresh")));
        let fresh = counter.usage("main", "fresh").await.expect("fresh");
        assert_eq!((fresh.entities, fresh.attr_instances), (0, Some(0)));
        assert_eq!(
            fresh.bytes,
            Some(SpaceBytes {
                entities: 0,
                attr_instances: 0,
                by_type: BTreeMap::new(),
            })
        );
    }

    #[tokio::test]
    async fn a_kept_count_answers_while_the_broker_is_away() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&broker)
            .await;
        let file = token_file("kept");
        let counter = Counter::new(Some(broker.uri()), Some(file.clone()));
        let key = format!("{}/air", broker.uri());
        let old = |minutes| SpaceUsage {
            entities: 42,
            broker: "main".into(),
            observed_at: Utc::now() - chrono::Duration::minutes(minutes),
            measured_at: Utc::now() - chrono::Duration::minutes(minutes),
            bytes: None,
            attr_instances: None,
            estimated: false,
        };
        counter
            .cache
            .write()
            .expect("cache")
            .insert(key.clone(), old(10));
        let kept = counter.usage("main", "air").await;
        counter.cache.write().expect("cache").insert(key, old(181));
        let gone = counter.usage("main", "air").await;
        let _ = std::fs::remove_file(&file);
        assert_eq!(kept.map(|u| u.entities), Ok(42));
        assert!(gone.unwrap_err().contains("503"));
    }

    /// T-6208: a decision read takes no kept row, a fresh one included, and fails while the
    /// broker is away instead of answering the last count.
    #[tokio::test]
    async fn a_measurement_now_never_answers_a_kept_count() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"counts": {"entities": 0}})),
            )
            .up_to_n_times(1)
            .mount(&broker)
            .await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&broker)
            .await;
        let file = token_file("now");
        let counter = Counter::new(Some(broker.uri()), Some(file.clone()));
        let first = counter.measure_now("main", "air").await;
        let kept = counter.usage("main", "air").await;
        let now = counter.measure_now("main", "air").await;
        let _ = std::fs::remove_file(&file);
        assert_eq!(first.map(|u| u.entities), Ok(0));
        assert_eq!(kept.map(|u| u.entities), Ok(0));
        assert!(now.unwrap_err().contains("503"));
        assert_eq!(broker.received_requests().await.map(|r| r.len()), Some(2));
    }

    /// T-6208: a small space is measured again after five minutes, a large one is kept for the
    /// hour until the sweep measures it.
    #[test]
    fn a_small_space_is_kept_five_minutes_and_a_large_one_an_hour() {
        let now = Utc::now();
        let row = |entities, history: Option<u64>, minutes| SpaceUsage {
            entities,
            broker: "main".into(),
            observed_at: now - chrono::Duration::minutes(minutes),
            measured_at: now,
            bytes: None,
            attr_instances: history,
            estimated: false,
        };
        assert!(current(&row(10, Some(0), 4), now));
        assert!(!current(&row(10, Some(0), 6), now));
        assert!(current(&row(9_000, Some(5_000), 59), now));
        assert!(!current(&row(9_000, Some(5_000), 61), now));
        // a history nobody counted is no proof of a small space
        assert!(current(&row(10, None, 30), now));
    }

    /// T-6208: concurrent recalculations share one broker read, a click within half a minute
    /// asks nothing, and the size's time is the broker's `measuredAt`.
    #[tokio::test]
    async fn concurrent_recalculations_share_one_broker_read() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q/tenants/air"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({
                        "counts": {"entities": 5, "attrInstances": 0},
                        "measuredAt": "2026-10-31T17:00:00.000Z",
                    }))
                    .set_delay(Duration::from_millis(200)),
            )
            .expect(1)
            .mount(&broker)
            .await;
        let file = token_file("recalc");
        let counter = Counter::new(Some(broker.uri()), Some(file.clone()));
        let (a, b, c) = tokio::join!(
            counter.recalculate("main", "air"),
            counter.recalculate("main", "air"),
            counter.recalculate("main", "air"),
        );
        let again = counter.recalculate("main", "air").await;
        let _ = std::fs::remove_file(&file);
        let a = a.expect("a");
        assert_eq!(Ok(&a), b.as_ref());
        assert_eq!(Ok(&a), c.as_ref());
        assert_eq!(Ok(&a), again.as_ref());
        assert_eq!(a.measured_at.to_rfc3339(), "2026-10-31T17:00:00+00:00");
    }

    /// T-6208: a recalculation measures a kept answer older than half a minute again, which a
    /// read of a large space would still take.
    #[tokio::test]
    async fn a_recalculation_measures_again_what_a_read_keeps() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let broker = MockServer::start().await;
        Mock::given(path("/q/tenants/air"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"counts": {"entities": 50_000, "attrInstances": 7}})),
            )
            .expect(1)
            .mount(&broker)
            .await;
        let file = token_file("remeasure");
        let counter = Counter::new(Some(broker.uri()), Some(file.clone()));
        let key = format!("{}/air", broker.uri());
        let at = Utc::now() - chrono::Duration::minutes(20);
        counter.cache.write().expect("cache").insert(
            key,
            SpaceUsage {
                entities: 40_000,
                broker: "main".into(),
                observed_at: at,
                measured_at: at,
                bytes: None,
                attr_instances: Some(7),
                estimated: false,
            },
        );
        let read = counter.usage("main", "air").await.expect("read");
        let recalculated = counter
            .recalculate("main", "air")
            .await
            .expect("recalculated");
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            read.entities, 40_000,
            "a read keeps a large space for the hour"
        );
        assert_eq!(recalculated.entities, 50_000);
        assert!(recalculated.measured_at > at);
    }

    #[tokio::test]
    async fn a_space_on_an_unreachable_broker_is_never_counted_on_main() {
        let counter = Counter::new(Some("http://127.0.0.1:9".into()), None);
        let err = counter.usage("canary", "air").await.unwrap_err();
        assert!(err.contains("JC_PORTAL_BROKER_URL_TEMPLATE"), "{err}");
        let err = counter.usage("", "air").await.unwrap_err();
        assert!(err.contains("brokerRef"), "{err}");
    }
}
