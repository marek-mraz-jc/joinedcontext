//! The `ngsi_ld` input (E6), read only, in one of three modes:
//! - `query` (default): `GET /entities` page by page, the page sequence of the Portal's
//!   `endpoint_input` (first page `count=true`, then `offset`), one message per entity and one
//!   batch per page; with `changes_only` an entity is emitted again only when it changed. No
//!   `interval`: one pass, then the input ends. With `interval`: a pass every interval.
//! - `temporal`: `GET /temporal/entities` with `timerel`/`timeAt`, following the broker's paging
//!   link while it stays on the same Endpoint; with `interval` the watermark (`timeAt`) moves to
//!   the newest `observedAt` of the pages the output acked, in order.
//! - `notification`: receives the Notifications of a subscription the Portal reconciler created
//!   (see [`crate::notification`]); this input never creates a subscription.
//!
//! An input acks a page only after the output acked it: the change gate and the watermark move
//! on ack, never on read.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use ekiben_bloblang::Value;
use ekiben_core::{
    AckResponder, Batch, Cache, ComponentConfig, CoreError, Input, Part, Registry, StreamContext,
};
use reqwest::header::LINK;
use reqwest::Method;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use url::Url;

use crate::client::{sleep_or_stop, CallEnv, NgsiClient};
use crate::config::{self, BrokerUrl, Fields, CLIENT_FIELDS, MAX_BATCH_COUNT};
use crate::metrics;
use crate::notification::{NotificationReader, NOTIFICATION_FIELDS};

/// The fields of the `query` and `temporal` modes, beside [`CLIENT_FIELDS`].
pub const READ_FIELDS: &[&str] = &[
    "mode",
    "type",
    "id",
    "id_pattern",
    "attrs",
    "q",
    "scope_q",
    "geo_q",
    "format",
    "limit",
    "max_entities",
    "interval",
    "changes_only",
    "changes_cap",
    "changes_cache",
    "timerel",
    "time_at",
    "end_time_at",
    "last_n",
    "max_pages",
];

/// One pass reads at most this many entities by default (the Portal's `endpoint_input` cap).
pub const DEFAULT_MAX_ENTITIES: u64 = 50_000;
/// The ceiling of `max_entities`.
pub const MAX_ENTITIES: u64 = 1_000_000;
/// The default and the ceiling of the change gate's memory (entities remembered).
pub const DEFAULT_CHANGES_CAP: u64 = 100_000;
pub const MAX_CHANGES_CAP: u64 = 1_000_000;
/// The default and the ceiling of the paging links one temporal pass follows.
pub const DEFAULT_MAX_PAGES: u64 = 1000;
pub const MAX_PAGES: u64 = 100_000;

pub fn register(registry: &Registry) {
    registry.register_input("ngsi_ld", |conf, ctx| {
        Ok(Box::new(NgsiLdInput::from_config(conf, ctx)?))
    });
}

pub struct NgsiLdInput {
    label: String,
    mode: Mode,
}

enum Mode {
    Query(Box<QueryReader>),
    Temporal(Box<TemporalReader>),
    Notification(Box<NotificationReader>),
}

impl NgsiLdInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let label = if conf.label.is_empty() {
            "ngsi_ld".to_string()
        } else {
            conf.label.clone()
        };
        let mode_name = conf
            .config
            .get("mode")
            .and_then(JsonValue::as_str)
            .unwrap_or("query");
        let mode = match mode_name {
            "notification" => {
                let f = Fields::new(&conf.config, "ngsi_ld", NOTIFICATION_FIELDS)?;
                Mode::Notification(Box::new(NotificationReader::from_fields(&f, ctx)?))
            }
            "query" | "temporal" => {
                let known: Vec<&str> = CLIENT_FIELDS.iter().chain(READ_FIELDS).copied().collect();
                let f = Fields::new(&conf.config, "ngsi_ld", &known)?;
                if mode_name == "query" {
                    Mode::Query(Box::new(QueryReader::from_fields(&f, ctx)?))
                } else {
                    Mode::Temporal(Box::new(TemporalReader::from_fields(&f, ctx)?))
                }
            }
            other => {
                return Err(CoreError::config(format!(
                    "ngsi_ld: mode must be query, temporal or notification, got {other:?}"
                )))
            }
        };
        Ok(Self { label, mode })
    }
}

#[async_trait]
impl Input for NgsiLdInput {
    fn input_type(&self) -> &'static str {
        "ngsi_ld"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if let Mode::Notification(r) = &mut self.mode {
            r.connect(ctx)?;
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let label = self.label.clone();
        match &mut self.mode {
            Mode::Query(r) => r.read(ctx, &label).await,
            Mode::Temporal(r) => r.read(ctx, &label).await,
            Mode::Notification(r) => r.read(ctx).await,
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Mode::Notification(r) = &mut self.mode {
            r.close();
        }
        Ok(())
    }
}

/// `s` percent-encoded but for RFC 3986's unreserved characters.
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The selection both read modes share: `type`/`id`/`idPattern`, `attrs`, `q`, `scopeQ`, the
/// geo-query and the representation, as query parameters in the Portal's order.
struct Selection {
    head: String,
    tail: String,
}

fn selection(f: &Fields<'_>, temporal: bool) -> Result<Selection, CoreError> {
    let mut head: Vec<String> = Vec::new();
    let types = f.strings("type")?;
    if !types.is_empty() {
        head.push(format!(
            "type={}",
            types.iter().map(|t| enc(t)).collect::<Vec<_>>().join(",")
        ));
    }
    let ids = f.strings("id")?;
    if !ids.is_empty() {
        head.push(format!(
            "id={}",
            ids.iter().map(|t| enc(t)).collect::<Vec<_>>().join(",")
        ));
    }
    if let Some(p) = f.str("id_pattern")?.filter(|p| !p.is_empty()) {
        head.push(format!("idPattern={}", enc(p)));
    }
    let attrs = f.strings("attrs")?;
    if !attrs.is_empty() {
        head.push(format!(
            "attrs={}",
            attrs.iter().map(|t| enc(t)).collect::<Vec<_>>().join(",")
        ));
    }
    let mut tail: Vec<String> = Vec::new();
    let q = f.str("q")?.filter(|s| !s.is_empty());
    if let Some(q) = q {
        tail.push(format!("q={}", enc(q)));
    }
    if let Some(s) = f.str("scope_q")?.filter(|s| !s.is_empty()) {
        tail.push(format!("scopeQ={}", enc(s)));
    }
    let geo = match f.object("geo_q")? {
        None => false,
        Some(g) => {
            let wrapped = JsonValue::Object(g.clone());
            let gf = Fields::new(
                &wrapped,
                f.component(),
                &["georel", "geometry", "coordinates", "geoproperty"],
            )?;
            let georel = gf
                .str("georel")?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| f.err("geo_q.georel is required"))?;
            let geometry = gf
                .str("geometry")?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| f.err("geo_q.geometry is required"))?;
            let coordinates = match gf.raw("coordinates") {
                Some(JsonValue::String(s)) if !s.is_empty() => s.clone(),
                Some(v @ JsonValue::Array(_)) => v.to_string(),
                _ => return Err(f.err("geo_q.coordinates is required (a string or an array)")),
            };
            tail.push(format!("georel={}", enc(georel)));
            tail.push(format!("geometry={}", enc(geometry)));
            tail.push(format!("coordinates={}", enc(&coordinates)));
            if let Some(p) = gf.str("geoproperty")?.filter(|s| !s.is_empty()) {
                tail.push(format!("geoproperty={}", enc(p)));
            }
            true
        }
    };
    if !temporal && types.is_empty() && ids.is_empty() && attrs.is_empty() && q.is_none() && !geo {
        return Err(f.err(
            "a query needs at least one of type, id, attrs, q or geo_q (ETSI GS CIM 009 5.7.2)",
        ));
    }
    if f.raw("id_pattern").is_some() && types.is_empty() && ids.is_empty() {
        // idPattern alone selects nothing on most brokers; refuse it rather than read nothing.
        if !temporal && attrs.is_empty() && q.is_none() && !geo {
            return Err(f.err("id_pattern needs type (or another selector) beside it"));
        }
    }
    Ok(Selection {
        head: head.join("&"),
        tail: tail.join("&"),
    })
}

/// The `options` parameter for a representation (`format`): `normalized` (default),
/// `concise`, `simplified` or `keyValues`.
fn representation(f: &Fields<'_>, sys_attrs: bool) -> Result<Option<String>, CoreError> {
    let mut opts: Vec<&str> = Vec::new();
    match f.string_or("format", "normalized")?.as_str() {
        "normalized" => {}
        "concise" => opts.push("concise"),
        "simplified" | "keyValues" | "keyvalues" => opts.push("keyValues"),
        other => {
            return Err(f.err(format!(
                "format must be normalized, concise, simplified or keyValues, got {other:?}"
            )))
        }
    }
    if sys_attrs {
        opts.push("sysAttrs");
    }
    Ok((!opts.is_empty()).then(|| format!("options={}", opts.join(","))))
}

fn join_params(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("&")
}

/// The entities of an answer: a JSON array of objects (numbers kept as the broker wrote them).
fn entities_of(body: &Bytes) -> Result<Vec<Value>, String> {
    let value = ekiben_bloblang::go_json::decode_with_numbers(body)
        .ok_or_else(|| "the broker answered a body that is not JSON".to_string())?;
    match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(n, v)| match v {
                Value::Object(_) => Ok(v.clone()),
                other => Err(format!(
                    "item {n} of the answer is a {} where an entity is expected",
                    other.type_name()
                )),
            })
            .collect(),
        other => Err(format!(
            "the broker answered a {} where an array of entities is expected",
            other.type_name()
        )),
    }
}

fn entity_id(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => match m.get("id") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A map that forgets its oldest key past `cap` entries.
pub(crate) struct BoundedMap {
    cap: usize,
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl BoundedMap {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn get(&self, k: &str) -> Option<&String> {
        self.map.get(k)
    }

    pub(crate) fn contains(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }

    pub(crate) fn insert(&mut self, k: String, v: String) {
        if self.map.insert(k.clone(), v).is_none() {
            self.order.push_back(k);
        }
        while self.map.len() > self.cap {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// The version of an entity the change gate compares: its `modifiedAt` when the broker gives
/// one, else a digest of its JSON text.
fn version_of(v: &Value) -> String {
    if let Value::Object(m) = v {
        if let Some(Value::String(s)) = m.get("modifiedAt") {
            return format!("m:{s}");
        }
    }
    let text = v.to_go_json_unescaped();
    format!("h:{}", hex::encode(&Sha256::digest(text.as_bytes())[..]))
}

/// Where the change gate remembers what it emitted: in memory (bounded), or a cache resource
/// (`changes_cache`) so a restart does not emit everything again.
enum Gate {
    Memory(Arc<Mutex<BoundedMap>>),
    Cache(Arc<dyn Cache>),
}

impl Gate {
    async fn seen(&self, key: &str) -> Option<String> {
        match self {
            Gate::Memory(m) => m
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(key)
                .cloned(),
            Gate::Cache(c) => c
                .get(key)
                .await
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        }
    }

    fn commit(&self, updates: Vec<(String, String)>) {
        match self {
            Gate::Memory(m) => {
                let mut g = m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                for (k, v) in updates {
                    g.insert(k, v);
                }
            }
            Gate::Cache(c) => {
                let c = c.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        for (k, v) in updates {
                            if c.set(&k, Bytes::from(v), None).await.is_err() {
                                tracing::warn!("ngsi_ld changes_cache refused an entry");
                            }
                        }
                    });
                }
            }
        }
    }
}

struct Pass {
    offset: u64,
    total: Option<u64>,
}

/// The `query` mode.
pub struct QueryReader {
    client: NgsiClient,
    query: String,
    limit: u64,
    max_entities: u64,
    interval: Option<Duration>,
    gate: Option<Arc<Gate>>,
    pass: Option<Pass>,
    next_pass: Option<Instant>,
    passes: u64,
}

impl QueryReader {
    fn from_fields(f: &Fields<'_>, ctx: &StreamContext) -> Result<Self, CoreError> {
        let raw_url = f
            .str("url")?
            .ok_or_else(|| f.err("field url is required"))?;
        let base = BrokerUrl::parse(f.component(), raw_url)?;
        for k in ["timerel", "time_at", "end_time_at", "last_n", "max_pages"] {
            if f.raw(k).is_some() {
                return Err(f.err(format!("field {k} belongs to mode temporal")));
            }
        }
        let sel = selection(f, false)?;
        let limit = f.u64_or("limit", MAX_BATCH_COUNT as u64)?;
        if limit == 0 || limit > MAX_BATCH_COUNT as u64 {
            return Err(f.err(format!("limit must be between 1 and {MAX_BATCH_COUNT}")));
        }
        let max_entities = f.u64_or("max_entities", DEFAULT_MAX_ENTITIES)?;
        if max_entities == 0 || max_entities > MAX_ENTITIES {
            return Err(f.err(format!("max_entities must be between 1 and {MAX_ENTITIES}")));
        }
        let interval = match f.str("interval")?.filter(|s| !s.is_empty()) {
            None => None,
            Some(_) => Some(f.duration_or("interval", Duration::ZERO)?).filter(|d| !d.is_zero()),
        };
        let changes_only = f.bool_or("changes_only", false)?;
        let gate = if changes_only {
            match f.str("changes_cache")?.filter(|s| !s.is_empty()) {
                Some(name) => Some(Arc::new(Gate::Cache(
                    ctx.resources.get_cache(&ctx.org.org_id, name)?,
                ))),
                None => {
                    let cap = f.u64_or("changes_cap", DEFAULT_CHANGES_CAP)?;
                    if cap == 0 || cap > MAX_CHANGES_CAP {
                        return Err(f.err(format!(
                            "changes_cap must be between 1 and {MAX_CHANGES_CAP}"
                        )));
                    }
                    Some(Arc::new(Gate::Memory(Arc::new(Mutex::new(
                        BoundedMap::new(usize::try_from(cap).unwrap_or(usize::MAX)),
                    )))))
                }
            }
        } else {
            if f.raw("changes_cap").is_some() || f.raw("changes_cache").is_some() {
                return Err(f.err("changes_cap and changes_cache need changes_only: true"));
            }
            None
        };
        let options = representation(f, changes_only)?;
        let limit_param = format!("limit={limit}");
        let query = base.join(&format!(
            "entities?{}",
            join_params(&[
                &sel.head,
                options.as_deref().unwrap_or(""),
                &limit_param,
                &sel.tail
            ])
        ));
        let settings = config::parse_client(f, &base, Duration::from_secs(30), &[])?;
        Ok(Self {
            client: NgsiClient::new(&ctx.org, settings)?,
            query,
            limit,
            max_entities,
            interval,
            gate,
            pass: None,
            next_pass: None,
            passes: 0,
        })
    }

    /// The URL of the next page of the current pass.
    pub fn page_url(&self, offset: u64) -> String {
        if offset == 0 {
            format!("{}&count=true", self.query)
        } else {
            format!("{}&offset={offset}", self.query)
        }
    }

    async fn read(
        &mut self,
        ctx: &StreamContext,
        label: &str,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if ctx.is_shutting_down() {
                return Err(CoreError::EndOfInput);
            }
            if self.pass.is_none() {
                match (self.interval, self.next_pass) {
                    (_, None) => {}
                    (None, Some(_)) => return Err(CoreError::EndOfInput),
                    (Some(_), Some(at)) => {
                        let now = Instant::now();
                        if at > now && !sleep_or_stop(&ctx.shutdown_rx, at - now).await {
                            return Err(CoreError::EndOfInput);
                        }
                    }
                }
                self.passes += 1;
                self.pass = Some(Pass {
                    offset: 0,
                    total: None,
                });
            }
            match self.page(ctx, label).await {
                Ok(Some(out)) => return Ok(out),
                Ok(None) => {}
                Err(e) => {
                    self.end_pass();
                    if self.interval.is_none() {
                        return Err(e);
                    }
                    tracing::warn!(label, error = %e, "ngsi_ld query pass failed; the next pass runs after the interval");
                }
            }
        }
    }

    fn end_pass(&mut self) {
        self.pass = None;
        self.next_pass = Some(Instant::now() + self.interval.unwrap_or(Duration::ZERO));
    }

    /// One page: a batch to emit, or `None` when it emitted nothing (all unchanged, or the pass
    /// ended).
    async fn page(
        &mut self,
        ctx: &StreamContext,
        label: &str,
    ) -> Result<Option<(Batch, AckResponder)>, CoreError> {
        let Some(pass) = self.pass.as_ref() else {
            return Ok(None);
        };
        let offset = pass.offset;
        let env = CallEnv {
            org: &ctx.org,
            resources: &ctx.resources,
            metrics: ctx.metrics.as_ref(),
            label,
            shutdown: &ctx.shutdown_rx,
        };
        let answer = self
            .client
            .call(&env, Method::GET, &self.page_url(offset), None)
            .await
            .map_err(|f| CoreError::other(format!("ngsi_ld: {}", f.text())))?;
        let entities =
            entities_of(&answer.body).map_err(|e| CoreError::other(format!("ngsi_ld: {e}")))?;
        let page_len = entities.len() as u64;
        let mut total = pass.total;
        if offset == 0 {
            total = answer
                .headers
                .get("ngsild-results-count")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            match total {
                None if page_len >= self.limit => {
                    return Err(CoreError::other(format!(
                        "ngsi_ld: the source answered a full page of {} and did not say how many entities it holds, so a read of one page would cut the rest off",
                        self.limit
                    )))
                }
                Some(n) if n > self.max_entities => {
                    return Err(CoreError::other(format!(
                        "ngsi_ld: the source holds {n} entities, more than the {} one run reads",
                        self.max_entities
                    )))
                }
                _ => {}
            }
        }
        let next = offset.saturating_add(page_len);
        let done = page_len < self.limit
            || page_len == 0
            || total.is_some_and(|t| next >= t)
            || next >= self.max_entities;
        if done {
            self.end_pass();
        } else {
            self.pass = Some(Pass {
                offset: next,
                total,
            });
        }
        ctx.metrics
            .incr_counter(&metrics::read_key(label), page_len);

        let mut batch: Batch = Vec::with_capacity(entities.len());
        let mut updates: Vec<(String, String)> = Vec::new();
        for entity in entities {
            if let Some(gate) = &self.gate {
                if let Some(id) = entity_id(&entity) {
                    let key = format!("{}:{id}", ctx.stream_id);
                    let version = version_of(&entity);
                    if gate.seen(&key).await.as_deref() == Some(version.as_str()) {
                        continue;
                    }
                    updates.push((key, version));
                }
            }
            batch.push(Part::from_structured(entity));
        }
        if batch.is_empty() {
            return Ok(None);
        }
        let ack = match self.gate.clone() {
            None => AckResponder::noop(),
            Some(gate) => AckResponder::from_fn(move |res| {
                if res.is_none() {
                    gate.commit(updates);
                }
            }),
        };
        Ok(Some((batch, ack)))
    }
}

/// The watermark of the temporal mode: pages commit in the order they were read, so a page the
/// output nacked holds every later page's `observedAt` back and the next pass reads it again.
#[derive(Default)]
struct Watermark {
    committed: Option<DateTime<Utc>>,
    pages: BTreeMap<u64, (Option<DateTime<Utc>>, Option<bool>)>,
    next_commit: u64,
    next_seq: u64,
}

impl Watermark {
    fn open(&mut self, newest: Option<DateTime<Utc>>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.pages.insert(seq, (newest, None));
        seq
    }

    fn settle(&mut self, seq: u64, ok: bool) {
        if let Some(p) = self.pages.get_mut(&seq) {
            p.1 = Some(ok);
        }
        while let Some((newest, Some(true))) = self.pages.get(&self.next_commit).cloned() {
            self.pages.remove(&self.next_commit);
            self.next_commit += 1;
            if let Some(n) = newest {
                if self.committed.is_none_or(|c| n > c) {
                    self.committed = Some(n);
                }
            }
        }
    }

    /// A new pass forgets the pages still open (their entities are read again).
    fn restart(&mut self) {
        self.pages.clear();
        self.next_commit = self.next_seq;
    }
}

/// The newest `observedAt` in a temporal answer: instances (`[{"value":..,"observedAt":..}]`)
/// and the aggregated `values` (`[[value, time], ..]`) form.
fn newest_observed(entities: &[Value]) -> Option<DateTime<Utc>> {
    fn parse(s: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    }
    fn walk(v: &Value, depth: usize, best: &mut Option<DateTime<Utc>>) {
        if depth > 6 {
            return;
        }
        let mut see = |d: Option<DateTime<Utc>>| {
            if let Some(d) = d {
                if best.is_none_or(|b| d > b) {
                    *best = Some(d);
                }
            }
        };
        match v {
            Value::Object(m) => {
                if let Some(Value::String(s)) = m.get("observedAt") {
                    see(parse(s));
                }
                if let Some(Value::Array(vals)) = m.get("values") {
                    for pair in vals.iter() {
                        if let Value::Array(p) = pair {
                            if let Some(Value::String(s)) = p.get(1) {
                                see(parse(s));
                            }
                        }
                    }
                }
                for (k, child) in m.iter() {
                    if k != "values" {
                        walk(child, depth + 1, best);
                    }
                }
            }
            Value::Array(items) => {
                for item in items.iter() {
                    walk(item, depth + 1, best);
                }
            }
            _ => {}
        }
    }
    let mut best = None;
    for e in entities {
        walk(e, 0, &mut best);
    }
    best
}

/// The target of a `Link` header entry whose `rel` is one of `rels`.
pub(crate) fn link_target(header: &str, rels: &[&str]) -> Option<String> {
    for entry in header.split(',') {
        let mut pieces = entry.split(';');
        let target = pieces.next()?.trim();
        let target = target.strip_prefix('<')?.strip_suffix('>')?;
        let matched = pieces.any(|p| {
            let p = p.trim();
            p.strip_prefix("rel=")
                .map(|r| r.trim_matches('"'))
                .is_some_and(|r| r.split_whitespace().any(|one| rels.contains(&one)))
        });
        if matched {
            return Some(target.to_string());
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimeRel {
    After,
    Before,
    Between,
}

/// The `temporal` mode.
pub struct TemporalReader {
    client: NgsiClient,
    base: BrokerUrl,
    head: String,
    timerel: TimeRel,
    time_at: Option<DateTime<Utc>>,
    end_time_at: Option<DateTime<Utc>>,
    max_pages: u64,
    interval: Option<Duration>,
    watermark: Arc<Mutex<Watermark>>,
    next: Option<String>,
    pages_this_pass: u64,
    next_pass: Option<Instant>,
    in_pass: bool,
}

impl TemporalReader {
    fn from_fields(f: &Fields<'_>, ctx: &StreamContext) -> Result<Self, CoreError> {
        let raw_url = f
            .str("url")?
            .ok_or_else(|| f.err("field url is required"))?;
        let base = BrokerUrl::parse(f.component(), raw_url)?;
        for k in [
            "changes_only",
            "changes_cap",
            "changes_cache",
            "max_entities",
        ] {
            if f.raw(k).is_some() {
                return Err(f.err(format!("field {k} belongs to mode query")));
            }
        }
        let sel = selection(f, true)?;
        if sel.head.is_empty() && sel.tail.is_empty() {
            return Err(f.err("a temporal query needs type, id, attrs, q or geo_q"));
        }
        let time = |k: &str| -> Result<Option<DateTime<Utc>>, CoreError> {
            match f.str(k)?.filter(|s| !s.is_empty()) {
                None => Ok(None),
                Some(s) => DateTime::parse_from_rfc3339(s)
                    .map(|d| Some(d.with_timezone(&Utc)))
                    .map_err(|_| f.err(format!("{k} must be an RFC 3339 date-time"))),
            }
        };
        let time_at = time("time_at")?;
        let end_time_at = time("end_time_at")?;
        let timerel = match f.string_or("timerel", "after")?.as_str() {
            "after" => TimeRel::After,
            "before" => TimeRel::Before,
            "between" => TimeRel::Between,
            other => {
                return Err(f.err(format!(
                    "timerel must be after, before or between, got {other:?}"
                )))
            }
        };
        if time_at.is_none() {
            return Err(f.err("time_at is required in mode temporal"));
        }
        if timerel == TimeRel::Between && end_time_at.is_none() {
            return Err(f.err("timerel between needs end_time_at"));
        }
        let interval = match f.str("interval")?.filter(|s| !s.is_empty()) {
            None => None,
            Some(_) => Some(f.duration_or("interval", Duration::ZERO)?).filter(|d| !d.is_zero()),
        };
        if interval.is_some() && timerel != TimeRel::After {
            return Err(f.err("interval needs timerel after: the watermark moves timeAt forward"));
        }
        let limit = f.u64_or("limit", 100)?;
        if limit == 0 || limit > MAX_BATCH_COUNT as u64 {
            return Err(f.err(format!("limit must be between 1 and {MAX_BATCH_COUNT}")));
        }
        let last_n = match f.raw("last_n") {
            None => None,
            Some(_) => {
                let n = f.u64_or("last_n", 0)?;
                if n == 0 {
                    return Err(f.err("last_n must be at least 1"));
                }
                Some(n)
            }
        };
        let max_pages = f.u64_or("max_pages", DEFAULT_MAX_PAGES)?;
        if max_pages == 0 || max_pages > MAX_PAGES {
            return Err(f.err(format!("max_pages must be between 1 and {MAX_PAGES}")));
        }
        let options = representation(f, false)?;
        let limit_param = format!("limit={limit}");
        let last_n_param = last_n.map(|n| format!("lastN={n}")).unwrap_or_default();
        let head = join_params(&[
            &sel.head,
            options.as_deref().unwrap_or(""),
            &limit_param,
            &last_n_param,
            &sel.tail,
        ]);
        let settings = config::parse_client(f, &base, Duration::from_secs(30), &[])?;
        Ok(Self {
            client: NgsiClient::new(&ctx.org, settings)?,
            base,
            head,
            timerel,
            time_at,
            end_time_at,
            max_pages,
            interval,
            watermark: Arc::new(Mutex::new(Watermark::default())),
            next: None,
            pages_this_pass: 0,
            next_pass: None,
            in_pass: false,
        })
    }

    fn fmt_time(t: DateTime<Utc>) -> String {
        t.to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    /// The first URL of a pass: `timeAt` is the configured one, or the committed watermark.
    pub fn first_url(&self) -> String {
        let committed = self
            .watermark
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .committed;
        let at = committed.or(self.time_at);
        let rel = match self.timerel {
            TimeRel::After => "after",
            TimeRel::Before => "before",
            TimeRel::Between => "between",
        };
        let mut time = format!("timerel={rel}");
        if let Some(at) = at {
            time.push_str(&format!("&timeAt={}", enc(&Self::fmt_time(at))));
        }
        if self.timerel == TimeRel::Between {
            if let Some(end) = self.end_time_at {
                time.push_str(&format!("&endTimeAt={}", enc(&Self::fmt_time(end))));
            }
        }
        self.base.join(&format!(
            "temporal/entities?{}",
            join_params(&[&self.head, &time])
        ))
    }

    async fn read(
        &mut self,
        ctx: &StreamContext,
        label: &str,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if ctx.is_shutting_down() {
                return Err(CoreError::EndOfInput);
            }
            if !self.in_pass {
                match (self.interval, self.next_pass) {
                    (_, None) => {}
                    (None, Some(_)) => return Err(CoreError::EndOfInput),
                    (Some(_), Some(at)) => {
                        let now = Instant::now();
                        if at > now && !sleep_or_stop(&ctx.shutdown_rx, at - now).await {
                            return Err(CoreError::EndOfInput);
                        }
                    }
                }
                self.watermark
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .restart();
                self.in_pass = true;
                self.pages_this_pass = 0;
                self.next = Some(self.first_url());
            }
            match self.page(ctx, label).await {
                Ok(Some(out)) => return Ok(out),
                Ok(None) => {}
                Err(e) => {
                    self.end_pass();
                    if self.interval.is_none() {
                        return Err(e);
                    }
                    tracing::warn!(label, error = %e, "ngsi_ld temporal pass failed; the next pass runs after the interval");
                }
            }
        }
    }

    fn end_pass(&mut self) {
        self.in_pass = false;
        self.next = None;
        self.next_pass = Some(Instant::now() + self.interval.unwrap_or(Duration::ZERO));
    }

    async fn page(
        &mut self,
        ctx: &StreamContext,
        label: &str,
    ) -> Result<Option<(Batch, AckResponder)>, CoreError> {
        let Some(url) = self.next.take() else {
            self.end_pass();
            return Ok(None);
        };
        self.pages_this_pass += 1;
        let env = CallEnv {
            org: &ctx.org,
            resources: &ctx.resources,
            metrics: ctx.metrics.as_ref(),
            label,
            shutdown: &ctx.shutdown_rx,
        };
        let answer = self
            .client
            .call(&env, Method::GET, &url, None)
            .await
            .map_err(|f| CoreError::other(format!("ngsi_ld: {}", f.text())))?;
        let entities =
            entities_of(&answer.body).map_err(|e| CoreError::other(format!("ngsi_ld: {e}")))?;
        let following = answer
            .headers
            .get_all(LINK)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|h| link_target(h, &["intervalafter", "next"]));
        match following {
            Some(target) if !entities.is_empty() => {
                let resolved = Url::parse(&url)
                    .and_then(|u| u.join(&target))
                    .map_err(|_| CoreError::other("ngsi_ld: the broker's paging link is no URL"))?;
                if !self.base.contains(&resolved) {
                    return Err(CoreError::AccessDenied {
                        org: ctx.org.org_id.0.clone(),
                        reason: "the broker's paging link leaves the Endpoint; it is not followed"
                            .to_string(),
                    });
                }
                if self.pages_this_pass >= self.max_pages {
                    return Err(CoreError::other(format!(
                        "ngsi_ld: the temporal answer runs past max_pages ({})",
                        self.max_pages
                    )));
                }
                self.next = Some(resolved.to_string());
            }
            _ => self.end_pass(),
        }
        ctx.metrics
            .incr_counter(&metrics::read_key(label), entities.len() as u64);
        if entities.is_empty() {
            return Ok(None);
        }
        let newest = newest_observed(&entities);
        let seq = self
            .watermark
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open(newest);
        let wm = self.watermark.clone();
        let ack = AckResponder::from_fn(move |res| {
            wm.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .settle(seq, res.is_none());
        });
        let batch: Batch = entities.into_iter().map(Part::from_structured).collect();
        Ok(Some((batch, ack)))
    }

    /// The `timeAt` the next pass starts from, once the output acked the pages before it.
    pub fn committed_watermark(&self) -> Option<String> {
        self.watermark
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .committed
            .map(Self::fmt_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_keeps_unreserved_only() {
        assert_eq!(enc("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(enc("speed>50;x==\"y\""), "speed%3E50%3Bx%3D%3D%22y%22");
        assert_eq!(enc("é"), "%C3%A9");
        assert_eq!(enc("&offset=0"), "%26offset%3D0");
    }

    #[test]
    fn link_targets() {
        let h = "<http://b/ngsi-ld/v1/temporal/entities?x=1>; rel=\"intervalafter\", <http://c/>; rel=\"self\"";
        assert_eq!(
            link_target(h, &["intervalafter"]).as_deref(),
            Some("http://b/ngsi-ld/v1/temporal/entities?x=1")
        );
        assert_eq!(link_target(h, &["next"]), None);
        assert_eq!(link_target("garbage", &["next"]), None);
    }

    #[test]
    fn watermark_commits_in_order() {
        let t = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .expect("t")
                .with_timezone(&Utc)
        };
        let mut w = Watermark::default();
        let a = w.open(Some(t("2026-01-01T00:00:00Z")));
        let b = w.open(Some(t("2026-01-02T00:00:00Z")));
        w.settle(b, true);
        assert_eq!(
            w.committed, None,
            "a later page never commits past an open one"
        );
        w.settle(a, true);
        assert_eq!(w.committed, Some(t("2026-01-02T00:00:00Z")));
        let c = w.open(Some(t("2026-01-03T00:00:00Z")));
        let d = w.open(Some(t("2026-01-04T00:00:00Z")));
        w.settle(c, false);
        w.settle(d, true);
        assert_eq!(w.committed, Some(t("2026-01-02T00:00:00Z")));
        w.restart();
        let e = w.open(None);
        w.settle(e, true);
        assert_eq!(w.committed, Some(t("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn newest_observed_reads_instances_and_values() {
        let v = Value::from_json_value(serde_json::json!([
            {"id": "urn:a", "speed": [
                {"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"},
                {"type": "Property", "value": 2, "observedAt": "2026-01-01T00:05:00+01:00"}
            ]},
            {"id": "urn:b", "speed": {"type": "Property", "values": [[3, "2026-01-01T00:10:00Z"]]}}
        ]));
        let Value::Array(items) = v else {
            panic!("array")
        };
        let items: Vec<Value> = items.iter().cloned().collect();
        let n = newest_observed(&items).expect("found");
        assert_eq!(
            n.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-01-01T00:10:00Z"
        );
    }

    #[test]
    fn bounded_map_forgets_the_oldest() {
        let mut m = BoundedMap::new(2);
        m.insert("a".into(), "1".into());
        m.insert("b".into(), "1".into());
        m.insert("a".into(), "2".into());
        m.insert("c".into(), "1".into());
        assert_eq!(m.len(), 2);
        assert!(!m.contains("a"));
        assert_eq!(m.get("c").map(String::as_str), Some("1"));
    }
}
