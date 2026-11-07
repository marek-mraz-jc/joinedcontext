//! The weekly link check (T-5133): once every [`EVERY`] the leader asks each stored `url` and
//! `organizer.url` of the entities a pipeline wrote whether it still opens, and the source health
//! view (T-5048) shows per pipeline how many links answer and the first dead ones.
//!
//! The entities are read through the space's public Endpoint, as an anonymous visitor would read
//! them, so the check never holds a right a visitor lacks. A link is asked only on an
//! inventoried host: the host of a DataSource of the project, the sources a steward approved. A
//! host gets one request a second ([`PER_HOST`]), its robots.txt is read first and honoured
//! (RFC 9309), each request waits at most [`TIMEOUT`], and a redirect is followed up to
//! [`REDIRECTS`] times on the same or another inventoried host. HEAD first; a host refusing HEAD
//! (405, 501) gets a GET of the first byte. A 4xx, a 5xx or no answer counts once; a link that
//! failed [`DEAD_AFTER`] checks in a row is dead.
//!
//! A pipeline is matched to an entity by the host of the entity's `source` (the events page its
//! source publishes, T-4896), else of its `url`: pipelines of one space sharing one host share
//! their links.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;
use utoipa::ToSchema;

use crate::store::Mirror;

/// How often the links are asked.
pub const EVERY: Duration = Duration::days(7);
/// The pause between two requests to one host.
pub const PER_HOST: StdDuration = StdDuration::from_secs(1);
/// How long one request waits for its answer.
pub const TIMEOUT: StdDuration = StdDuration::from_secs(10);
/// How long a link's host has to accept the connection.
const CONNECT_TIMEOUT: StdDuration = StdDuration::from_secs(5);
/// Redirects followed per link.
pub const REDIRECTS: usize = 5;
/// Failed checks in a row that make a link dead.
pub const DEAD_AFTER: u32 = 2;
/// Dead links the view lists per pipeline.
pub const SHOWN: usize = 5;
/// ponytail: links asked per pipeline and run; a source with more is asked on its first ones.
const PER_PIPELINE: usize = 500;
/// Entities read per space and run, in pages of [`PAGE`].
const PER_SPACE: usize = 20_000;
const PAGE: usize = 1_000;
/// Hosts asked at once.
const HOSTS_AT_ONCE: usize = 8;
/// The robots.txt read per origin.
const ROBOTS_BYTES: usize = 512 * 1024;
/// The product token robots.txt groups name.
const AGENT: &str = "joinedcontext-platform";
const USER_AGENT: &str = "joinedcontext-platform/1.0 (link check; public events)";

/// A link that failed [`DEAD_AFTER`] checks in a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = DeadLink)]
pub struct DeadLink {
    pub url: String,
    /// The status it answered, none when it did not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why it did not answer (a timeout, a refused connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The newest link check of one pipeline, as the source health view shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PipelineLinks)]
pub struct Links {
    /// When the check ran, RFC 3339.
    pub checked_at: String,
    /// Links asked.
    pub checked: usize,
    /// Links that answered 2xx or 3xx.
    pub answered: usize,
    /// Links not asked: their robots.txt refuses the reader, or their host is no source of the project.
    pub skipped: usize,
    /// Links that failed [`DEAD_AFTER`] checks in a row.
    pub dead_count: usize,
    /// The first [`SHOWN`] of them.
    pub dead: Vec<DeadLink>,
    /// Every link that failed its newest check, with its failures in a row; kept, never shown.
    #[serde(default, skip_serializing)]
    #[schema(ignore)]
    pub failing: BTreeMap<String, DeadLinkState>,
}

/// One failing link between two checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadLinkState {
    pub failures: u32,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
}

/// What one link answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    Failed {
        status: Option<u16>,
        error: Option<String>,
    },
    Skipped,
}

/// A pipeline's links before they are asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub project: String,
    pub pipeline: String,
    pub links: Vec<Url>,
    /// Links on no inventoried host: counted as skipped, never asked.
    pub elsewhere: usize,
}

/// The links of `previous` and the outcomes of this run folded into what the view shows.
pub fn fold(
    previous: Option<&Links>,
    outcomes: &[(String, Outcome)],
    elsewhere: usize,
    now: DateTime<Utc>,
) -> Links {
    let mut links = Links {
        checked_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        skipped: elsewhere,
        ..Links::default()
    };
    for (url, outcome) in outcomes {
        match outcome {
            Outcome::Answered => {
                links.checked += 1;
                links.answered += 1;
            }
            Outcome::Skipped => links.skipped += 1,
            Outcome::Failed { status, error } => {
                links.checked += 1;
                let before = previous
                    .and_then(|p| p.failing.get(url))
                    .map_or(0, |f| f.failures);
                links.failing.insert(
                    url.clone(),
                    DeadLinkState {
                        failures: before.saturating_add(1),
                        status: *status,
                        error: error.clone(),
                    },
                );
            }
        }
    }
    let dead = links
        .failing
        .iter()
        .filter(|(_, state)| state.failures >= DEAD_AFTER)
        .map(|(url, state)| DeadLink {
            url: url.clone(),
            status: state.status,
            error: state.error.clone(),
        })
        .collect::<Vec<_>>();
    links.dead_count = dead.len();
    links.dead = dead.into_iter().take(SHOWN).collect();
    links
}

/// The user agents of one robots.txt group and its rules, `true` an Allow.
type RobotsGroup = (Vec<String>, Vec<(bool, String)>);

/// Whether robots.txt `body` lets `AGENT` read `path` (RFC 9309): the group naming the agent,
/// else the `*` group; the longest matching rule wins, Allow on a tie.
pub fn robots_allow(body: &str, path: &str) -> bool {
    let mut groups: Vec<RobotsGroup> = Vec::new();
    let mut in_agents = false;
    for line in body.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match key.as_str() {
            "user-agent" => {
                if !in_agents {
                    groups.push((Vec::new(), Vec::new()));
                }
                in_agents = true;
                if let Some(group) = groups.last_mut() {
                    group.0.push(value.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                in_agents = false;
                if let Some(group) = groups.last_mut() {
                    if !value.is_empty() {
                        group.1.push((key == "allow", value.to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    let ours = groups
        .iter()
        .filter(|(agents, _)| agents.iter().any(|a| a == AGENT))
        .collect::<Vec<_>>();
    let chosen = if ours.is_empty() {
        groups
            .iter()
            .filter(|(agents, _)| agents.iter().any(|a| a == "*"))
            .collect()
    } else {
        ours
    };
    let best = chosen
        .iter()
        .flat_map(|(_, rules)| rules)
        .filter(|(_, pattern)| matches_rule(pattern, path))
        .max_by_key(|(allow, pattern)| (pattern.len(), *allow));
    best.is_none_or(|(allow, _)| *allow)
}

/// A robots.txt path pattern: a prefix, `*` any run of characters, a trailing `$` the end.
fn matches_rule(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let parts = parts.collect::<Vec<_>>();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if last && anchored {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    !anchored || rest.is_empty()
}

/// A host the check may ask: a public DNS name, never an address or a cluster-internal name.
pub fn askable_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && !host.starts_with('[')
        && ![
            ".local",
            ".internal",
            ".svc",
            ".cluster.local",
            ".localhost",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

/// An http(s) link on an askable host, without its fragment.
pub fn link(text: &str) -> Option<Url> {
    let mut url = Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.host_str().is_some_and(askable_host) {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// The links of one entity in keyValues form: its `url` and its organizer's `url`.
fn entity_links(entity: &Value) -> Vec<String> {
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    let organizer = entity.get("organizer").and_then(|o| match o {
        Value::Object(_) => text(o.get("url")),
        _ => None,
    });
    [text(entity.get("url")), organizer]
        .into_iter()
        .flatten()
        .collect()
}

/// The host an entity is matched to a pipeline by: its `source`'s, else its `url`'s.
fn entity_host(entity: &Value) -> Option<String> {
    ["source", "url"].iter().find_map(|key| {
        entity
            .get(*key)
            .and_then(Value::as_str)
            .and_then(|s| Url::parse(s).ok())
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
    })
}

/// A space whose entities pipelines write, readable through a public Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub project: String,
    /// The public Endpoint's slug.
    pub slug: String,
    pub types: BTreeSet<String>,
    /// The pipelines writing it, with their sources' hosts.
    pub pipelines: Vec<(String, BTreeSet<String>)>,
    /// Every DataSource host of the project.
    pub inventoried: BTreeSet<String>,
}

fn name_of(reference: Option<&Value>) -> Option<String> {
    reference
        .and_then(|r| r.as_str().or_else(|| r.get("name").and_then(Value::as_str)))
        .map(str::to_owned)
}

fn datasource_host(mirror: &Mirror, project: &str, name: &str) -> Option<String> {
    mirror
        .get(project, "DataSource", name)
        .and_then(|d| {
            d.spec
                .pointer("/http/url")
                .or_else(|| d.spec.get("url"))
                .and_then(Value::as_str)
                .and_then(link)
        })
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
}

/// Every space a pipeline writes that a public Endpoint serves, with its pipelines and hosts.
pub fn spaces(mirror: &Mirror) -> Vec<Space> {
    let mut found: BTreeMap<(String, String), Space> = BTreeMap::new();
    let mut pipelines = mirror.matching(|env| env.kind == "Pipeline");
    pipelines.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    for pipeline in pipelines {
        let project = pipeline.metadata.namespace.clone().unwrap_or_default();
        let sources = match pipeline.spec.get("sources").and_then(Value::as_array) {
            Some(list) => list.iter().collect::<Vec<_>>(),
            None => pipeline.spec.get("source").into_iter().collect(),
        };
        let hosts = sources
            .iter()
            .filter_map(|s| name_of(s.get("dataSourceRef")))
            .filter_map(|name| datasource_host(mirror, &project, &name))
            .collect::<BTreeSet<_>>();
        if hosts.is_empty() {
            continue;
        }
        for (endpoint, kind) in crate::alerts::outputs_of(&pipeline.spec) {
            let Some(space) = endpoint
                .and_then(|e| mirror.get(&project, "Endpoint", &e))
                .and_then(|e| name_of(e.spec.get("contextSpaceRef")))
            else {
                continue;
            };
            let Some(slug) = public_slug(mirror, &project, &space) else {
                continue;
            };
            let entry = found
                .entry((project.clone(), space))
                .or_insert_with(|| Space {
                    project: project.clone(),
                    slug,
                    types: BTreeSet::new(),
                    pipelines: Vec::new(),
                    inventoried: BTreeSet::new(),
                });
            entry.types.extend(kind);
            if !entry
                .pipelines
                .iter()
                .any(|(n, _)| *n == pipeline.metadata.name)
            {
                entry
                    .pipelines
                    .push((pipeline.metadata.name.clone(), hosts.clone()));
            }
        }
    }
    let mut spaces = found.into_values().collect::<Vec<_>>();
    for space in &mut spaces {
        space.inventoried = mirror
            .matching(|env| {
                env.kind == "DataSource"
                    && env.metadata.namespace.as_deref() == Some(&space.project)
            })
            .iter()
            .filter_map(|d| datasource_host(mirror, &space.project, &d.metadata.name))
            .collect();
    }
    spaces
}

/// The slug of a public Endpoint of `space`, the first by name.
fn public_slug(mirror: &Mirror, project: &str, space: &str) -> Option<String> {
    let mut endpoints = mirror.matching(|env| {
        env.kind == "Endpoint"
            && env.metadata.namespace.as_deref() == Some(project)
            && env.spec.get("audience").and_then(Value::as_str) == Some("public")
            && name_of(env.spec.get("contextSpaceRef")).as_deref() == Some(space)
    });
    endpoints.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    endpoints.iter().find_map(|e| {
        e.spec
            .get("slug")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

/// The links per pipeline of a space's entities (keyValues), each pipeline's own first.
pub fn plan(space: &Space, entities: &[Value]) -> Vec<Plan> {
    space
        .pipelines
        .iter()
        .map(|(pipeline, hosts)| {
            let mut seen = BTreeSet::new();
            let mut plan = Plan {
                project: space.project.clone(),
                pipeline: pipeline.clone(),
                links: Vec::new(),
                elsewhere: 0,
            };
            let theirs = entities
                .iter()
                .filter(|e| entity_host(e).is_some_and(|h| hosts.contains(&h)));
            for text in theirs.flat_map(entity_links) {
                if !seen.insert(text.clone()) {
                    continue;
                }
                match link(&text) {
                    Some(url)
                        if url.host_str().is_some_and(|h| {
                            space.inventoried.contains(&h.to_ascii_lowercase())
                        }) && plan.links.len() < PER_PIPELINE =>
                    {
                        plan.links.push(url)
                    }
                    _ => plan.elsewhere += 1,
                }
            }
            plan
        })
        .collect()
}

/// The newest check of each pipeline, durable with a database.
pub struct LinkStore {
    db: Option<sqlx::PgPool>,
    memory: RwLock<HashMap<(String, String), Links>>,
}

impl LinkStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            db,
            memory: RwLock::default(),
        }
    }

    pub async fn get(&self, project: &str, pipeline: &str) -> Result<Option<Links>, sqlx::Error> {
        let Some(pool) = &self.db else {
            let memory = self.memory.read().unwrap_or_else(|p| p.into_inner());
            return Ok(memory
                .get(&(project.to_owned(), pipeline.to_owned()))
                .cloned());
        };
        let row: Option<(sqlx::types::Json<Links>,)> =
            sqlx::query_as("SELECT result FROM link_check WHERE project = $1 AND pipeline = $2")
                .bind(project)
                .bind(pipeline)
                .fetch_optional(pool)
                .await?;
        Ok(row.map(|(json,)| json.0))
    }

    pub async fn put(
        &self,
        project: &str,
        pipeline: &str,
        links: &Links,
    ) -> Result<(), sqlx::Error> {
        let Some(pool) = &self.db else {
            let mut memory = self.memory.write().unwrap_or_else(|p| p.into_inner());
            memory.insert((project.to_owned(), pipeline.to_owned()), links.clone());
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO link_check (project, pipeline, checked_at, result) VALUES ($1, $2, now(), $3) \
             ON CONFLICT (project, pipeline) DO UPDATE SET checked_at = now(), result = EXCLUDED.result",
        )
        .bind(project)
        .bind(pipeline)
        .bind(sqlx::types::Json(links))
        .execute(pool)
        .await?;
        Ok(())
    }

    /// When the newest check ran, over every pipeline.
    pub async fn newest(&self) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        let Some(pool) = &self.db else {
            let memory = self.memory.read().unwrap_or_else(|p| p.into_inner());
            return Ok(memory
                .values()
                .filter_map(|l| DateTime::parse_from_rfc3339(&l.checked_at).ok())
                .map(|t| t.with_timezone(&Utc))
                .max());
        };
        let row: (Option<time::OffsetDateTime>,) =
            sqlx::query_as("SELECT max(checked_at) FROM link_check")
                .fetch_one(pool)
                .await?;
        Ok(row
            .0
            .and_then(|t| DateTime::from_timestamp(t.unix_timestamp(), 0)))
    }
}

/// The client that asks event links: no proxy, [`jc_core::outbound::PublicOnly`] judges every
/// name once it resolves (each redirect hop included), so a public name answering 10.x or
/// 127.x is never asked. Redirects are followed by hand in [`HostQueue`].
fn link_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(jc_core::outbound::PublicOnly))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_default()
}

/// The weekly run: reads the spaces through the gateway, asks the links, keeps the result.
pub struct Checker {
    mirror: Arc<Mirror>,
    store: Arc<LinkStore>,
    gateway: Url,
    /// Reads the installation's own gateway, an internal Service.
    http: reqwest::Client,
    /// Asks the event links: a name that resolves to an internal address is refused.
    links: reqwest::Client,
    running: AtomicBool,
}

impl Checker {
    /// `gateway` is the address the Portal reads public Endpoints at (`JC_PORTAL_GATEWAY_URL`).
    pub fn new(mirror: Arc<Mirror>, store: Arc<LinkStore>, gateway: Url) -> Self {
        Self {
            mirror,
            store,
            gateway,
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(USER_AGENT)
                .build()
                .unwrap_or_default(),
            links: link_client(),
            running: AtomicBool::new(false),
        }
    }

    /// Starts a run in the background when the newest is [`EVERY`] old and none is under way.
    pub fn start_due(self: &Arc<Self>, now: DateTime<Utc>) {
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let checker = Arc::clone(self);
        tokio::spawn(async move {
            match checker.store.newest().await {
                Ok(newest) if newest.is_none_or(|at| now - at >= EVERY) => checker.run(now).await,
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "the link check could not read its last run"),
            }
            checker.running.store(false, Ordering::Release);
        });
    }

    /// One pass over every space.
    pub async fn run(&self, now: DateTime<Utc>) {
        let mut plans = Vec::new();
        let mut inventoried = BTreeSet::new();
        for space in spaces(&self.mirror) {
            inventoried.extend(space.inventoried.iter().cloned());
            match self.entities(&space).await {
                Ok(entities) => plans.extend(plan(&space, &entities)),
                Err(reason) => {
                    tracing::warn!(project = %space.project, slug = %space.slug, %reason,
                    "the link check could not read a space")
                }
            }
        }
        let outcomes = self.ask(&plans, &inventoried).await;
        for plan in &plans {
            let mine = plan
                .links
                .iter()
                .filter_map(|u| outcomes.get(u.as_str()).map(|o| (u.to_string(), o.clone())))
                .collect::<Vec<_>>();
            let result = async {
                let previous = self.store.get(&plan.project, &plan.pipeline).await?;
                let links = fold(previous.as_ref(), &mine, plan.elsewhere, now);
                self.store.put(&plan.project, &plan.pipeline, &links).await
            };
            if let Err(error) = result.await {
                tracing::warn!(project = %plan.project, pipeline = %plan.pipeline, %error,
                    "the link check was not kept");
            }
        }
    }

    /// The entities of `space` with a `url`, keyValues, at most [`PER_SPACE`].
    async fn entities(&self, space: &Space) -> Result<Vec<Value>, String> {
        let base = crate::api::catalogue::endpoint_url(&self.gateway, &space.slug);
        let types = space.types.iter().cloned().collect::<Vec<_>>().join(",");
        let mut all = Vec::new();
        while all.len() < PER_SPACE {
            let offset = all.len().to_string();
            let limit = PAGE.to_string();
            let mut query = vec![
                ("q", "url"),
                ("attrs", "url,organizer,source"),
                ("options", "keyValues"),
                ("limit", limit.as_str()),
                ("offset", offset.as_str()),
            ];
            if !types.is_empty() {
                query.push(("type", types.as_str()));
            }
            let response = self
                .http
                .get(format!("{base}ngsi-ld/v1/entities"))
                .header(reqwest::header::ACCEPT, "application/json")
                .query(&query)
                .send()
                .await
                .map_err(|e| format!("{e:?}"))?;
            if !response.status().is_success() {
                return Err(format!("the Endpoint answered {}", response.status()));
            }
            let page: Vec<Value> =
                crate::capped::json(response, crate::capped::ANSWER_BYTES).await?;
            let done = page.len() < PAGE;
            all.extend(page);
            if done {
                break;
            }
        }
        Ok(all)
    }

    /// Asks every link once, each host on its own at [`PER_HOST`].
    async fn ask(
        &self,
        plans: &[Plan],
        inventoried: &BTreeSet<String>,
    ) -> HashMap<String, Outcome> {
        let mut by_host: BTreeMap<String, BTreeSet<Url>> = BTreeMap::new();
        for url in plans.iter().flat_map(|p| &p.links) {
            if let Some(host) = url.host_str() {
                by_host
                    .entry(host.to_ascii_lowercase())
                    .or_default()
                    .insert(url.clone());
            }
        }
        let asked = stream::iter(by_host)
            .map(|(host, urls)| HostQueue::new(&self.links, host, inventoried).ask_all(urls))
            .buffer_unordered(HOSTS_AT_ONCE)
            .collect::<Vec<_>>()
            .await;
        asked.into_iter().flatten().collect()
    }
}

/// The requests to one host, one a second, its robots.txt read once; a redirect to another host
/// is asked on this host's beat.
struct HostQueue<'a> {
    http: &'a reqwest::Client,
    host: String,
    inventoried: &'a BTreeSet<String>,
    robots: HashMap<String, Option<String>>,
    asked_once: bool,
    pause: StdDuration,
}

impl<'a> HostQueue<'a> {
    fn new(http: &'a reqwest::Client, host: String, inventoried: &'a BTreeSet<String>) -> Self {
        Self {
            http,
            host,
            inventoried,
            robots: HashMap::new(),
            asked_once: false,
            pause: PER_HOST,
        }
    }

    async fn ask_all(mut self, urls: BTreeSet<Url>) -> Vec<(String, Outcome)> {
        let mut out = Vec::with_capacity(urls.len());
        for url in urls {
            let outcome = self.ask(&url).await;
            out.push((url.to_string(), outcome));
        }
        tracing::debug!(host = %self.host, links = out.len(), "link check asked a host");
        out
    }

    async fn beat(&mut self) {
        if self.asked_once {
            tokio::time::sleep(self.pause).await;
        }
        self.asked_once = true;
    }

    /// Whether robots.txt of `url`'s origin allows its path; read once per origin.
    async fn allowed(&mut self, url: &Url) -> bool {
        let origin = url.origin().ascii_serialization();
        if !self.robots.contains_key(&origin) {
            self.beat().await;
            let body = match self.http.get(format!("{origin}/robots.txt")).send().await {
                // RFC 9309 2.5: a crawler reads at least 500 KiB of it.
                Ok(r) if r.status().is_success() => crate::capped::text(r, ROBOTS_BYTES).await.ok(),
                // RFC 9309 2.3.1.3: a 4xx robots.txt allows everything.
                Ok(r) if r.status().is_client_error() => Some(String::new()),
                // 5xx, a redirect or no answer: the site is taken as closed for this run.
                _ => None,
            };
            self.robots.insert(origin.clone(), body);
        }
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        self.robots
            .get(&origin)
            .and_then(Option::as_ref)
            .is_some_and(|body| robots_allow(body, &path))
    }

    async fn ask(&mut self, url: &Url) -> Outcome {
        let mut current = url.clone();
        for _ in 0..=REDIRECTS {
            if !self.allowed(&current).await {
                return Outcome::Skipped;
            }
            self.beat().await;
            let mut answer = self.http.head(current.clone()).send().await;
            if matches!(&answer, Ok(r) if matches!(r.status().as_u16(), 405 | 501)) {
                self.beat().await;
                answer = self
                    .http
                    .get(current.clone())
                    .header(reqwest::header::RANGE, "bytes=0-0")
                    .send()
                    .await;
            }
            let response = match answer {
                Ok(r) => r,
                Err(e) => {
                    let error = if e.is_timeout() {
                        "no answer in 10 s".to_owned()
                    } else {
                        "no answer".to_owned()
                    };
                    return Outcome::Failed {
                        status: None,
                        error: Some(error),
                    };
                }
            };
            let status = response.status();
            if !status.is_redirection() {
                return if status.is_success() {
                    Outcome::Answered
                } else {
                    Outcome::Failed {
                        status: Some(status.as_u16()),
                        error: None,
                    }
                };
            }
            let next = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| current.join(l).ok());
            match next {
                Some(next)
                    if matches!(next.scheme(), "http" | "https")
                        && next.host_str().is_some_and(|h| {
                            self.inventoried.contains(&h.to_ascii_lowercase())
                        }) =>
                {
                    current = next
                }
                // A redirect off the inventory is an answer: the page moved somewhere it is not ours to ask.
                _ => return Outcome::Answered,
            }
        }
        Outcome::Failed {
            status: None,
            error: Some(format!("more than {REDIRECTS} redirects")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_rules_pick_our_group_and_the_longest_match() {
        let body = "User-agent: *\nDisallow: /\n\nUser-agent: joinedcontext-platform\nDisallow: /private/\nAllow: /private/open$\n";
        assert!(robots_allow(body, "/podujatia/1"));
        assert!(!robots_allow(body, "/private/x"));
        assert!(robots_allow(body, "/private/open"));
        let star = "User-agent: *\nDisallow: /*.pdf$\nDisallow: /admin\n";
        assert!(!robots_allow(star, "/files/a.pdf"));
        assert!(robots_allow(star, "/files/a.pdf?x=1"));
        assert!(!robots_allow(star, "/admin/x"));
        assert!(robots_allow(star, "/podujatia/"));
        assert!(robots_allow("", "/anything"));
    }

    #[test]
    fn only_public_dns_names_are_asked() {
        assert!(link("https://www.banskabystrica.sk/podujatia/#x")
            .is_some_and(|u| u.fragment().is_none()));
        for refused in [
            "http://10.0.0.1/x",
            "http://[::1]/x",
            "http://antares/x",
            "http://antares.jc.svc/x",
            "http://a.svc.cluster.local/x",
            "ftp://example.sk/x",
            "javascript:alert(1)",
        ] {
            assert!(link(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_link_failing_twice_in_a_row_is_dead_and_an_answer_clears_it() {
        let now = Utc::now();
        let bad = |status| Outcome::Failed {
            status: Some(status),
            error: None,
        };
        let first = fold(
            None,
            &[
                ("https://a.sk/1".into(), bad(404)),
                ("https://a.sk/2".into(), Outcome::Answered),
            ],
            1,
            now,
        );
        assert_eq!(
            (
                first.checked,
                first.answered,
                first.skipped,
                first.dead_count
            ),
            (2, 1, 1, 0)
        );
        let second = fold(Some(&first), &[("https://a.sk/1".into(), bad(404))], 0, now);
        assert_eq!(second.dead_count, 1);
        assert_eq!(
            second.dead,
            vec![DeadLink {
                url: "https://a.sk/1".into(),
                status: Some(404),
                error: None
            }]
        );
        let third = fold(
            Some(&second),
            &[("https://a.sk/1".into(), Outcome::Answered)],
            0,
            now,
        );
        assert_eq!(third.dead_count, 0);
        assert!(third.failing.is_empty());
    }

    #[test]
    fn the_view_lists_the_first_five_dead_links_and_counts_them_all() {
        let now = Utc::now();
        let outcomes = (0..7)
            .map(|i| {
                (
                    format!("https://a.sk/{i}"),
                    Outcome::Failed {
                        status: None,
                        error: Some("no answer in 10 s".into()),
                    },
                )
            })
            .collect::<Vec<_>>();
        let first = fold(None, &outcomes, 0, now);
        let second = fold(Some(&first), &outcomes, 0, now);
        assert_eq!(second.dead_count, 7);
        assert_eq!(second.dead.len(), SHOWN);
    }

    #[test]
    fn a_plan_asks_only_inventoried_hosts_of_its_own_source() {
        let space = Space {
            project: "bbsk".into(),
            slug: "s".into(),
            types: BTreeSet::from(["Event".to_owned()]),
            pipelines: vec![
                ("podujatia-a".into(), BTreeSet::from(["a.sk".to_owned()])),
                ("podujatia-b".into(), BTreeSet::from(["b.sk".to_owned()])),
            ],
            inventoried: BTreeSet::from(["a.sk".to_owned(), "b.sk".to_owned()]),
        };
        let entities = vec![
            serde_json::json!({"id": "1", "source": "https://a.sk/podujatia/", "url": "https://a.sk/e/1",
                "organizer": {"name": "Kino", "url": "https://kino.example.org/"}}),
            serde_json::json!({"id": "2", "source": "https://b.sk/", "url": "https://b.sk/e/2"}),
            serde_json::json!({"id": "3", "source": "https://a.sk/podujatia/", "url": "http://10.1.2.3/admin"}),
        ];
        let plans = plan(&space, &entities);
        assert_eq!(
            plans[0].links.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec!["https://a.sk/e/1"]
        );
        assert_eq!(plans[0].elsewhere, 2);
        assert_eq!(
            plans[1].links.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec!["https://b.sk/e/2"]
        );
    }

    /// A site of fixed pages that records what it was asked, and when.
    async fn site(
        asked: Arc<std::sync::Mutex<Vec<(String, std::time::Instant)>>>,
    ) -> std::net::SocketAddr {
        use axum::http::{Method, StatusCode};
        use axum::response::IntoResponse;
        let app = axum::Router::new().fallback(move |method: Method, uri: axum::http::Uri| {
            let asked = Arc::clone(&asked);
            async move {
                let path = uri.path().to_owned();
                if let Ok(mut asked) = asked.lock() {
                    asked.push((format!("{method} {path}"), std::time::Instant::now()));
                }
                match path.as_str() {
                    "/robots.txt" => {
                        (StatusCode::OK, "User-agent: *\nDisallow: /private/\n").into_response()
                    }
                    "/ok" => StatusCode::OK.into_response(),
                    "/gone" => StatusCode::NOT_FOUND.into_response(),
                    "/broken" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    "/slow" => {
                        tokio::time::sleep(StdDuration::from_secs(3)).await;
                        StatusCode::OK.into_response()
                    }
                    "/no-head" if method == Method::HEAD => {
                        StatusCode::METHOD_NOT_ALLOWED.into_response()
                    }
                    "/no-head" => StatusCode::PARTIAL_CONTENT.into_response(),
                    "/moved" => (StatusCode::FOUND, [("location", "/ok")]).into_response(),
                    "/away" => {
                        (StatusCode::FOUND, [("location", "http://localhost:9/x")]).into_response()
                    }
                    "/loop" => (StatusCode::FOUND, [("location", "/loop")]).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        at
    }

    #[tokio::test]
    async fn each_link_is_asked_on_its_host_at_its_pace_and_judged() {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let at = site(Arc::clone(&asked)).await;
        let http = reqwest::Client::builder()
            .timeout(StdDuration::from_millis(500))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let inventoried = BTreeSet::from(["127.0.0.1".to_owned()]);
        let mut queue = HostQueue::new(&http, "127.0.0.1".into(), &inventoried);
        queue.pause = StdDuration::from_millis(150);
        let url = |path: &str| Url::parse(&format!("http://{at}{path}")).expect("url");
        let paths = [
            "/ok",
            "/gone",
            "/broken",
            "/slow",
            "/no-head",
            "/moved",
            "/away",
            "/loop",
            "/private/x",
        ];
        let outcomes = queue.ask_all(paths.iter().map(|p| url(p)).collect()).await;
        let outcome = |path: &str| {
            outcomes
                .iter()
                .find(|(u, _)| *u == url(path).to_string())
                .map(|(_, o)| o.clone())
                .expect(path)
        };
        assert_eq!(outcome("/ok"), Outcome::Answered);
        assert_eq!(
            outcome("/gone"),
            Outcome::Failed {
                status: Some(404),
                error: None
            }
        );
        assert_eq!(
            outcome("/broken"),
            Outcome::Failed {
                status: Some(500),
                error: None
            }
        );
        assert_eq!(
            outcome("/slow"),
            Outcome::Failed {
                status: None,
                error: Some("no answer in 10 s".into())
            }
        );
        assert_eq!(outcome("/no-head"), Outcome::Answered);
        assert_eq!(outcome("/moved"), Outcome::Answered);
        // A redirect off the inventory is an answer and is never followed.
        assert_eq!(outcome("/away"), Outcome::Answered);
        assert!(matches!(
            outcome("/loop"),
            Outcome::Failed { status: None, .. }
        ));
        // robots.txt refuses the path: skipped, never asked.
        assert_eq!(outcome("/private/x"), Outcome::Skipped);
        let asked = asked.lock().expect("asked").clone();
        let lines = asked.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>();
        assert_eq!(lines.iter().filter(|l| **l == "GET /robots.txt").count(), 1);
        assert!(!lines.iter().any(|l| l.contains("/private/")), "{lines:?}");
        assert!(lines.contains(&"GET /no-head"));
        for pair in asked.windows(2) {
            assert!(
                pair[1].1.duration_since(pair[0].1) >= StdDuration::from_millis(140),
                "{lines:?}"
            );
        }
    }

    #[test]
    fn a_failure_of_a_slow_host_twice_in_a_row_flags_the_source() {
        let now = Utc::now();
        let slow = [(
            "https://a.sk/slow".to_owned(),
            Outcome::Failed {
                status: None,
                error: Some("no answer in 10 s".into()),
            },
        )];
        let first = fold(None, &slow, 0, now);
        assert_eq!(first.dead_count, 0, "one timeout alone flags nothing");
        let second = fold(Some(&first), &slow, 0, now);
        assert_eq!(second.dead[0].error.as_deref(), Some("no answer in 10 s"));
    }

    #[tokio::test]
    async fn a_closed_robots_txt_skips_every_link_of_the_site() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        let app =
            axum::Router::new().fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE });
        tokio::spawn(async move { axum::serve(listener, app).await });
        let http = reqwest::Client::new();
        let inventoried = BTreeSet::new();
        let mut queue = HostQueue::new(&http, "127.0.0.1".into(), &inventoried);
        queue.pause = StdDuration::from_millis(1);
        let url = Url::parse(&format!("http://{at}/podujatia/1")).expect("url");
        assert_eq!(queue.ask(&url).await, Outcome::Skipped);
    }

    fn manifest(kind: &str, name: &str, spec: Value) -> crate::resource::ResourceEnvelope {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": kind,
            "metadata": { "name": name, "namespace": "bbsk" },
            "spec": spec,
        }))
        .expect("manifest")
    }

    /// T-5133 review: a link whose name resolves to an internal address is never asked, robots.txt
    /// included; the run's own link client refuses it at resolution.
    #[tokio::test]
    async fn a_name_resolving_to_an_internal_address_is_never_asked() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&hits);
        let app = axum::Router::new().fallback(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { axum::http::StatusCode::OK }
        });
        tokio::spawn(async move { axum::serve(listener, app).await });
        let http = link_client();
        let url = Url::parse(&format!("http://localhost:{port}/podujatia/1")).expect("url");
        let refused = http.head(url.clone()).send().await.expect_err("refused");
        assert!(
            jc_core::outbound::refused(&refused).is_some(),
            "{refused:?}"
        );
        let inventoried = BTreeSet::from(["localhost".to_owned()]);
        let mut queue = HostQueue::new(&http, "localhost".into(), &inventoried);
        queue.pause = StdDuration::from_millis(1);
        assert_eq!(queue.ask(&url).await, Outcome::Skipped);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_run_reads_the_space_through_its_public_endpoint_and_asks_no_host_outside_the_inventory(
    ) {
        let mirror = Arc::new(Mirror::new());
        mirror.upsert(manifest(
            "DataSource",
            "podujatia-a",
            serde_json::json!({ "type": "http", "http": { "url": "https://a.example/feed" } }),
        ));
        mirror.upsert(manifest(
            "Pipeline",
            "podujatia-a",
            serde_json::json!({
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "podujatia-a" } },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:bbsk.sk:podujatia:zapis",
                "output": { "type": "Event" },
            }),
        ));
        mirror.upsert(manifest(
            "Endpoint",
            "zapis",
            serde_json::json!({ "contextSpaceRef": "podujatia" }),
        ));
        mirror.upsert(manifest("Endpoint", "app", serde_json::json!({
            "audience": "public", "contextSpaceRef": { "kind": "ContextSpace", "name": "podujatia" }, "slug": "s1",
        })));
        let found = spaces(&mirror);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "s1");
        assert_eq!(
            found[0].inventoried,
            BTreeSet::from(["a.example".to_owned()])
        );

        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let gateway = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = Arc::clone(&seen);
            async move {
                if let Ok(mut seen) = seen.lock() {
                    seen.push(uri.to_string());
                }
                axum::Json(serde_json::json!([
                    { "id": "urn:ngsi-ld:Event:1", "source": "https://a.example/podujatia/",
                      "url": "https://elsewhere.example/e/1",
                      "organizer": { "name": "Kino", "url": "http://127.0.0.1:9/" } },
                ]))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let at = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, gateway).await });
        let store = Arc::new(LinkStore::new(None));
        let checker = Checker::new(
            Arc::clone(&mirror),
            Arc::clone(&store),
            Url::parse(&format!("http://{at}/")).expect("gateway"),
        );
        let now = Utc::now();
        checker.run(now).await;
        let links = store
            .get("bbsk", "podujatia-a")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!((links.checked, links.skipped, links.dead_count), (0, 2, 0));
        assert_eq!(
            store.newest().await.expect("newest").map(|t| t.timestamp()),
            Some(now.timestamp())
        );
        let asked = asked.lock().expect("asked").clone();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(
            asked[0].starts_with("/api/endpoint/s1/ngsi-ld/v1/entities?"),
            "{asked:?}"
        );
        assert!(asked[0].contains("type=Event"), "{asked:?}");
    }
}
