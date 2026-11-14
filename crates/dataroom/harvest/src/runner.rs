#![forbid(unsafe_code)]
//! Harvest runner executing sources with SSRF protection, provenance tracking, and withdrawal
//! of datasets no longer listed by a complete source read (C9).

use crate::guard::{validate_and_pin_url, ValidatedTarget, MAX_HARVEST_BYTES, MAX_REDIRECT_HOPS};
use crate::sources::{
    parse_arcgis_hub_items, parse_ckan_search_page, parse_dcat_page, parse_nkod_sparql_bindings,
};
use chrono::Utc;
use dataroom_core::ops::HarvestRunner;
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    ActivityRow, DataroomError, Dataset, HarvestLogLine, HarvestRun, HarvestRunState,
    HarvestSkippedItem, HarvestSource, HarvestSourceKind, RecordState, RequestCtx, Result, Store,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Response from a single HTTP hop before redirect handling.
#[derive(Debug, Clone)]
pub enum FetchResponse {
    Ok {
        content_type: Option<String>,
        body: String,
    },
    Redirect {
        status: u16,
        location: String,
    },
}

/// Outbound HTTP fetcher trait so both real HTTPS and deterministic test doubles go through
/// the exact same SSRF, redirect-hop, DNS-pinning, byte-cap and host-allowlist checks (A2).
pub trait OutboundFetcher: Send + Sync {
    fn fetch_hop<'a>(
        &'a self,
        target: &'a ValidatedTarget,
        accept: &'a str,
        max_bytes: usize,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>>;
}

/// Production HTTPS fetcher backed by `reqwest` with DNS pinning (`resolve_to_addrs`),
/// `Policy::none()` (so `HarvestEngine` validates every redirect hop), per-source byte cap,
/// and timeout (A2).
#[derive(Default)]
pub struct HttpFetcher;

impl HttpFetcher {
    pub fn new() -> Self {
        Self
    }
}

impl OutboundFetcher for HttpFetcher {
    fn fetch_hop<'a>(
        &'a self,
        target: &'a ValidatedTarget,
        accept: &'a str,
        max_bytes: usize,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        Box::pin(async move {
            let mut builder = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10).min(timeout))
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("dataroom-harvest/1.0");

            // Only a target whose DNS answer passed the guard is fetched, on exactly that answer
            // (A2): without pinned addresses reqwest would resolve the name again by itself.
            if target.pinned_addrs.is_empty() {
                return Err(DataroomError::OutboundRefused(format!(
                    "{} has no checked address; refusing an unpinned fetch",
                    target.host
                )));
            }
            builder = builder.resolve_to_addrs(&target.host, &target.pinned_addrs);

            let client = builder
                .build()
                .map_err(|e| DataroomError::OutboundRefused(format!("build HTTP client: {e}")))?;

            let url_str = target.url.as_str();
            let mut resp = client
                .get(url_str)
                .header("Accept", accept)
                .send()
                .await
                .map_err(|e| DataroomError::OutboundRefused(format!("fetch {url_str}: {e}")))?;

            let status = resp.status();
            if status.is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        DataroomError::OutboundRefused(format!(
                            "{url_str} answered redirect {} without Location header",
                            status.as_u16()
                        ))
                    })?;
                return Ok(FetchResponse::Redirect {
                    status: status.as_u16(),
                    location: loc.to_owned(),
                });
            }

            if !status.is_success() {
                return Err(DataroomError::OutboundRefused(format!(
                    "{url_str} answered HTTP {}",
                    status.as_u16()
                )));
            }

            if let Some(cl) = resp.content_length() {
                if cl > max_bytes as u64 {
                    return Err(DataroomError::OutboundRefused(format!(
                        "{url_str} Content-Length {cl} exceeds cap of {max_bytes} bytes"
                    )));
                }
            }

            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);

            let mut buf = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| DataroomError::OutboundRefused(format!("read {url_str}: {e}")))?
            {
                if buf.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(DataroomError::OutboundRefused(format!(
                        "{url_str} body exceeded cap of {max_bytes} bytes"
                    )));
                }
                buf.extend_from_slice(&chunk);
            }

            let text = String::from_utf8_lossy(&buf).into_owned();
            Ok(FetchResponse::Ok {
                content_type,
                body: text,
            })
        })
    }
}

/// Hosts and the addresses a [`MapFetcher`] was asked to connect to.
type PinnedSeen = Arc<std::sync::RwLock<Vec<(String, Vec<SocketAddr>)>>>;

/// Deterministic in-memory map fetcher for testing harvest runs, redirects, DNS pinning, byte caps,
/// pagination and withdrawal (A2).
#[derive(Clone, Default)]
pub struct MapFetcher {
    responses: Arc<std::sync::RwLock<BTreeMap<String, FetchResponse>>>,
    statuses: Arc<std::sync::RwLock<BTreeMap<String, Vec<u16>>>>,
    pinned_seen: PinnedSeen,
}

impl MapFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &self,
        url: impl Into<String>,
        content_type: Option<&str>,
        body: impl Into<String>,
    ) {
        self.responses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                url.into(),
                FetchResponse::Ok {
                    content_type: content_type.map(str::to_owned),
                    body: body.into(),
                },
            );
    }

    pub fn insert_redirect(
        &self,
        url: impl Into<String>,
        status: u16,
        location: impl Into<String>,
    ) {
        self.responses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                url.into(),
                FetchResponse::Redirect {
                    status,
                    location: location.into(),
                },
            );
    }

    /// `url` answers HTTP `status` once, before its other answers, as [`HttpFetcher`] reports
    /// a status it does not follow.
    pub fn insert_status(&self, url: impl Into<String>, status: u16) {
        self.statuses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .entry(url.into())
            .or_default()
            .push(status);
    }

    pub fn pinned_seen(&self) -> Vec<(String, Vec<SocketAddr>)> {
        self.pinned_seen
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl OutboundFetcher for MapFetcher {
    fn fetch_hop<'a>(
        &'a self,
        target: &'a ValidatedTarget,
        _accept: &'a str,
        max_bytes: usize,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<FetchResponse>> {
        self.pinned_seen
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push((target.host.clone(), target.pinned_addrs.clone()));

        let url = target.url.as_str();
        let mut statuses = self.statuses.write().unwrap_or_else(|e| e.into_inner());
        if let Some(status) = statuses.get_mut(url).filter(|q| !q.is_empty()) {
            let status = status.remove(0);
            return Box::pin(std::future::ready(Err(DataroomError::OutboundRefused(
                format!("{url} answered HTTP {status}"),
            ))));
        }
        drop(statuses);
        let found = self
            .responses
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(url)
            .cloned()
            .ok_or_else(|| DataroomError::OutboundRefused(format!("no response for {url}")));

        let checked = found.and_then(|resp| match resp {
            FetchResponse::Ok { content_type, body } => {
                if body.len() > max_bytes {
                    Err(DataroomError::OutboundRefused(format!(
                        "{url} body size {} exceeded cap of {max_bytes} bytes",
                        body.len()
                    )))
                } else {
                    Ok(FetchResponse::Ok { content_type, body })
                }
            }
            other => Ok(other),
        });
        Box::pin(std::future::ready(checked))
    }
}

type CustomDnsResolver = Arc<dyn Fn(&str, u16) -> Result<Vec<SocketAddr>> + Send + Sync>;

/// Most pages read from a paged Hub or DCAT feed in one run.
const MAX_PAGES: usize = 50;

/// Most `package_search` pages (1000 records each) read from a CKAN source in one run.
const CKAN_MAX_PAGES: usize = 20;

/// A source read that cannot be shown complete: nothing of it is written or withdrawn.
fn incomplete(what: String) -> DataroomError {
    DataroomError::OutboundRefused(format!("incomplete read: {what}"))
}

/// The publisher's datasets with their distributions and licence terms: prod's
/// `harvest-nkod.py` QUERY, `%s` the publisher IRI.
const NKOD_QUERY: &str = "PREFIX dcat: <http://www.w3.org/ns/dcat#>
PREFIX dct: <http://purl.org/dc/terms/>
PREFIX l: <https://data.gov.sk/def/ontology/legislation/>
SELECT ?ds ?title ?description ?modified ?issued ?landing ?keyword
       ?dist ?distTitle ?url ?format ?work ?database ?special WHERE {
  ?ds a dcat:Dataset ; dct:publisher <%s> ; dct:title ?title .
  OPTIONAL { ?ds dct:description ?description }
  OPTIONAL { ?ds dct:modified ?modified }
  OPTIONAL { ?ds dct:issued ?issued }
  OPTIONAL { ?ds dcat:landingPage ?landing }
  OPTIONAL { ?ds dcat:keyword ?keyword }
  OPTIONAL {
    ?ds dcat:distribution ?dist .
    OPTIONAL { ?dist dct:title ?distTitle }
    OPTIONAL { ?dist dcat:downloadURL ?url }
    OPTIONAL { ?dist dct:format ?format }
    OPTIONAL { ?dist l:termsOfUse ?terms .
               OPTIONAL { ?terms l:authorsWorkType ?work }
               OPTIONAL { ?terms l:originalDatabaseType ?database }
               OPTIONAL { ?terms l:databaseProtectedBySpecialRightsType ?special } }
  }
}
";

/// The SPARQL GET of an NKOD source: its `url` as given when it carries a `query`, else the
/// endpoint asked [`NKOD_QUERY`] for `target_publisher_iri`. The IRI goes into the query between
/// `<` and `>`, so one that is not an absolute http(s) IRI free of the characters SPARQL forbids
/// in an IRI is refused.
pub fn nkod_query_url(source: &HarvestSource) -> Result<String> {
    let mut url = url::Url::parse(&source.url).map_err(|e| {
        DataroomError::field_error("url", format!("'{}' is not a URL: {e}", source.url))
    })?;
    if url.query_pairs().any(|(k, _)| k == "query") {
        return Ok(source.url.clone());
    }
    let iri = source.target_publisher_iri.as_deref().ok_or_else(|| {
        DataroomError::field_error(
            "targetPublisherIri",
            "an NKOD source without a query in its url needs the publisher IRI",
        )
    })?;
    let forbidden = |c: char| c.is_whitespace() || c.is_control() || "<>\"{}|^`\\".contains(c);
    if !(iri.starts_with("https://") || iri.starts_with("http://")) || iri.contains(forbidden) {
        return Err(DataroomError::field_error(
            "targetPublisherIri",
            format!("'{iri}' is not an IRI the query can hold"),
        ));
    }
    url.query_pairs_mut()
        .append_pair("query", &NKOD_QUERY.replace("%s", iri));
    Ok(url.into())
}

/// Harvest runner engine implementing [`HarvestRunner`] (C9, A2).
pub struct HarvestEngine {
    fetcher: Arc<dyn OutboundFetcher>,
    resolve_dns: bool,
    custom_dns: Option<CustomDnsResolver>,
    max_bytes: usize,
    timeout: Duration,
    deadline: Duration,
    own_gateway: Option<(String, SocketAddr)>,
    retry_waits: Vec<Duration>,
}

/// How long one run may take before it is stopped and recorded failed (prod's CronJob
/// `activeDeadlineSeconds`).
pub const HARVEST_DEADLINE: Duration = Duration::from_secs(3600);

/// The waits, in seconds, before each new read of a sheet whose read failed by itself (prod's
/// `OD_RETRY_WAITS`, T-4385).
const SHEET_RETRY_WAITS: [u64; 3] = [5, 30, 120];

/// Whether a failed read passes by itself (prod's `_od_transient`): a 429, a 5xx or a broken
/// connection. Any other answer (a 4xx, a body over the cap, a refused address) is the answer.
pub fn transient(e: &DataroomError) -> bool {
    let DataroomError::OutboundRefused(m) = e else {
        return false;
    };
    match m.rsplit_once(" answered HTTP ") {
        Some((_, code)) => code.parse::<u16>().is_ok_and(|c| c == 429 || c >= 500),
        None => m.starts_with("fetch ") || m.starts_with("read "),
    }
}

impl HarvestEngine {
    pub fn new(fetcher: Arc<dyn OutboundFetcher>) -> Self {
        Self {
            fetcher,
            resolve_dns: true,
            custom_dns: None,
            max_bytes: MAX_HARVEST_BYTES,
            timeout: Duration::from_secs(60),
            deadline: HARVEST_DEADLINE,
            own_gateway: None,
            retry_waits: SHEET_RETRY_WAITS.map(Duration::from_secs).to_vec(),
        }
    }

    /// The instance's own gateway: a URL under `public` (an origin, no trailing slash) is read
    /// from `internal`, the loopback listener that serves it, as prod's harvest read the
    /// Endpoints in-cluster. Only that one origin goes there, with no redirect followed off it;
    /// every other URL passes the guard as before.
    pub fn with_own_gateway(mut self, public: &str, internal: SocketAddr) -> Self {
        self.own_gateway = Some((public.trim_end_matches('/').to_owned(), internal));
        self
    }

    /// The pinned target of `url` when it lies under the own gateway's origin.
    fn own_target(&self, url: &str) -> Result<Option<ValidatedTarget>> {
        let Some((public, internal)) = &self.own_gateway else {
            return Ok(None);
        };
        // Only an Endpoint read: the loopback listener serves every module, and a URL here can
        // come from a remote page (a Hub's `next` link) or a source an editor wrote. Anything
        // else under the origin, or a path the listener could decode into another mount, goes
        // through the guard, which refuses loopback.
        let Some(rest) = url
            .strip_prefix(public.as_str())
            .filter(|r| r.starts_with("/api/endpoint/"))
            .filter(|r| {
                // The path alone: a query or fragment reaches no mount (a filtered dataset's
                // sheet probe carries its encoded `isPartOf` IRI there).
                let path = r.split(['?', '#']).next().unwrap_or(r);
                let lower = path.to_ascii_lowercase();
                !["%2e", "%2f", "%5c", "\\"]
                    .iter()
                    .any(|bad| lower.contains(bad))
            })
        else {
            return Ok(None);
        };
        let url = url::Url::parse(&format!("http://{internal}{rest}")).map_err(|e| {
            DataroomError::OutboundRefused(format!("own gateway address of {url}: {e}"))
        })?;
        // The parser drops dot segments: what it kept must still be an Endpoint read.
        if !url.path().starts_with("/api/endpoint/") {
            return Ok(None);
        }
        Ok(Some(ValidatedTarget {
            url,
            host: internal.ip().to_string(),
            pinned_addrs: vec![*internal],
        }))
    }

    /// The waits before each new read of a sheet that failed by itself ([`transient`]).
    pub fn with_retry_waits(mut self, waits: Vec<Duration>) -> Self {
        self.retry_waits = waits;
        self
    }

    /// Whether the sheet at `url` answers, read as prod's `od_read` reads it: a read that failed
    /// by itself is asked again after each of the retry waits (the gateway answers 503 while
    /// its download slots are full, as when jc starts and the Portal probes every Endpoint).
    async fn sheet_answers(&self, url: &str, source: &HarvestSource, hosts: &[String]) -> bool {
        let mut waits = self.retry_waits.iter();
        loop {
            match self.checked_fetch(url, source, hosts, "text/csv").await {
                Ok(_) => return true,
                Err(e) if transient(&e) => match waits.next() {
                    Some(wait) => tokio::time::sleep(*wait).await,
                    None => return false,
                },
                Err(_) => return false,
            }
        }
    }

    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn with_dns_resolution(mut self, resolve_dns: bool) -> Self {
        self.resolve_dns = resolve_dns;
        self
    }

    pub fn with_custom_dns<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&str, u16) -> Result<Vec<SocketAddr>> + Send + Sync + 'static,
    {
        self.custom_dns = Some(Arc::new(resolver));
        self
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fetches `url` while enforcing the SSRF guard, host allow-list, and DNS pinning on the
    /// initial URL and on every redirect hop (max [`MAX_REDIRECT_HOPS`] = 5 hops, A2).
    pub async fn checked_fetch(
        &self,
        url: &str,
        source: &HarvestSource,
        cat_allowed_hosts: &[String],
        accept: &str,
    ) -> Result<(Option<String>, String)> {
        let mut current_url = url.to_owned();
        let mut hops = 0;
        loop {
            let resolver_ref = self
                .custom_dns
                .as_ref()
                .map(|r| r.as_ref() as &crate::guard::DnsResolver);
            let own = if hops == 0 {
                self.own_target(&current_url)?
            } else {
                None
            };
            let target = match own {
                Some(t) => t,
                None => validate_and_pin_url(
                    &current_url,
                    &source.allowed_hosts,
                    cat_allowed_hosts,
                    self.resolve_dns,
                    resolver_ref,
                )?,
            };

            match self
                .fetcher
                .fetch_hop(&target, accept, self.max_bytes, self.timeout)
                .await?
            {
                FetchResponse::Ok { content_type, body } => {
                    if body.len() > self.max_bytes {
                        return Err(DataroomError::OutboundRefused(format!(
                            "{current_url} body size {} exceeded cap of {} bytes",
                            body.len(),
                            self.max_bytes
                        )));
                    }
                    return Ok((content_type, body));
                }
                FetchResponse::Redirect { location, .. } => {
                    if hops == MAX_REDIRECT_HOPS {
                        return Err(DataroomError::OutboundRefused(format!(
                            "redirect limit of {MAX_REDIRECT_HOPS} hops exceeded from {url}"
                        )));
                    }
                    hops += 1;
                    let next_url = target
                        .url
                        .join(&location)
                        .map_err(|e| {
                            DataroomError::OutboundRefused(format!(
                                "invalid redirect Location '{location}': {e}"
                            ))
                        })?
                        .into();
                    current_url = next_url;
                }
            }
        }
    }

    async fn gather_items(
        &self,
        ctx: &RequestCtx,
        store: &dyn Store,
        source: &HarvestSource,
        cat_allowed_hosts: &[String],
        default_lang: &str,
    ) -> Result<(Vec<(String, Dataset)>, Vec<HarvestSkippedItem>)> {
        // Check cross-organization harvest policy if the source URL points to another catalogue
        // on this installation (C9).
        if let Ok(parsed) = url::Url::parse(&source.url) {
            if let Some(host) = parsed.host_str() {
                if let Some(target_cat) = store.resolve_host(host).await? {
                    if target_cat.org_id != ctx.org_id && !target_cat.allow_cross_org_harvest {
                        return Err(DataroomError::Forbidden(format!(
                            "catalogue '{}' does not allow cross-organization harvesting",
                            target_cat.host
                        )));
                    }
                }
            }
        }

        match source.kind {
            HarvestSourceKind::ArcGisHub => {
                let mut features = Vec::new();
                let hub = source.url.trim_end_matches('/');
                // Sorted: unsorted, the Hub's pages overlap at random (opendata.bbsk.sk
                // 2026-11-10: 3 of 4 reads held 115 items, 100 of them distinct), and the
                // items a repeat stands in for go missing from the read.
                let mut next_url = Some(format!(
                    "{hub}/api/search/v1/collections/dataset/items?limit=100&sortBy=properties.created"
                ));
                let mut expected_total = None;

                for _ in 0..MAX_PAGES {
                    let Some(url) = next_url.take() else {
                        break;
                    };
                    let (_, body) = self
                        .checked_fetch(&url, source, cat_allowed_hosts, "application/json")
                        .await?;
                    let page: Value = serde_json::from_str(&body).map_err(|e| {
                        DataroomError::OutboundRefused(format!("invalid Hub JSON: {e}"))
                    })?;
                    if let Some(m) = page.get("numberMatched").and_then(Value::as_u64) {
                        expected_total = Some(m as usize);
                    }
                    if let Some(arr) = page.get("features").and_then(Value::as_array) {
                        features.extend(arr.iter().cloned());
                    }
                    next_url = page
                        .get("links")
                        .and_then(Value::as_array)
                        .and_then(|links| {
                            links.iter().find_map(|l| {
                                (l.get("rel").and_then(Value::as_str) == Some("next"))
                                    .then(|| {
                                        l.get("href").and_then(Value::as_str).map(str::to_owned)
                                    })
                                    .flatten()
                            })
                        });
                }

                // Only a complete read may withdraw (harvest-hub.py `hub_items`): the Hub must
                // say how many items it holds, stop paging, and hand over exactly that many.
                if next_url.is_some() {
                    return Err(incomplete(format!(
                        "the Hub still pages after {MAX_PAGES} pages"
                    )));
                }
                // An item read twice hides one never read: the read is not complete.
                let distinct = features
                    .iter()
                    .filter_map(|f| f.pointer("/properties/id").and_then(Value::as_str))
                    .collect::<BTreeSet<_>>()
                    .len();
                if distinct != features.len() {
                    return Err(incomplete(format!(
                        "the Hub listed {} items, {distinct} of them distinct",
                        features.len()
                    )));
                }
                match expected_total {
                    Some(expected) if features.len() == expected => {}
                    Some(expected) => {
                        return Err(incomplete(format!(
                            "got {} of {expected} Hub items",
                            features.len()
                        )));
                    }
                    None => {
                        return Err(incomplete(format!(
                            "the Hub listed {} items without numberMatched",
                            features.len()
                        )));
                    }
                }

                Ok(parse_arcgis_hub_items(
                    ctx.org_id,
                    ctx.catalogue_id,
                    source,
                    cat_allowed_hosts,
                    &features,
                    default_lang,
                ))
            }
            HarvestSourceKind::NkodSparql => {
                let url = nkod_query_url(source)?;
                let (_, body) = self
                    .checked_fetch(
                        &url,
                        source,
                        cat_allowed_hosts,
                        "application/sparql-results+json",
                    )
                    .await?;
                let val: Value = serde_json::from_str(&body).map_err(|e| {
                    DataroomError::OutboundRefused(format!("invalid SPARQL JSON: {e}"))
                })?;
                Ok(parse_nkod_sparql_bindings(
                    ctx.org_id,
                    ctx.catalogue_id,
                    source,
                    cat_allowed_hosts,
                    &val,
                    default_lang,
                ))
            }
            HarvestSourceKind::DcatAp => {
                let mut all_datasets = Vec::new();
                let mut next_url = Some(source.url.clone());
                let mut seen_pages = BTreeSet::new();

                for _ in 0..MAX_PAGES {
                    let Some(url) = next_url.take() else {
                        break;
                    };
                    if !seen_pages.insert(url.clone()) {
                        return Err(incomplete(format!("the feed's pages loop back to {url}")));
                    }
                    let (ct, body) = self
                        .checked_fetch(
                            &url,
                            source,
                            cat_allowed_hosts,
                            "text/turtle, application/ld+json, application/rdf+xml",
                        )
                        .await?;
                    let (page_ds, next) = parse_dcat_page(
                        ctx.org_id,
                        ctx.catalogue_id,
                        source,
                        &body,
                        ct.as_deref(),
                        default_lang,
                    )?;
                    all_datasets.extend(page_ds);
                    next_url = next;
                }
                if next_url.is_some() {
                    return Err(incomplete(format!(
                        "the feed still pages after {MAX_PAGES} pages"
                    )));
                }

                Ok((all_datasets, Vec::new()))
            }
            HarvestSourceKind::CkanAction => {
                let mut all_datasets = Vec::new();
                let mut start = 0usize;
                let base = source.url.trim_end_matches('/');
                let mut total = None;
                for _ in 0..CKAN_MAX_PAGES {
                    let url = if base.contains("/api/3/action/package_search") {
                        // A source URL may carry its own filter (`?fq=...`): keep it.
                        let sep = if base.contains('?') { '&' } else { '?' };
                        format!("{base}{sep}rows=1000&start={start}")
                    } else {
                        format!("{base}/api/3/action/package_search?rows=1000&start={start}")
                    };
                    let (_, body) = self
                        .checked_fetch(&url, source, cat_allowed_hosts, "application/json")
                        .await?;
                    let (page_ds, count) = parse_ckan_search_page(
                        ctx.org_id,
                        ctx.catalogue_id,
                        source,
                        &body,
                        default_lang,
                    )?;
                    total = Some(count);
                    let fetched_len = page_ds.len();
                    all_datasets.extend(page_ds);
                    start += fetched_len;
                    if fetched_len == 0 || start >= count {
                        break;
                    }
                }
                // Only a complete read may withdraw: every record the count promised was read.
                let total = total.unwrap_or(0);
                if start < total {
                    return Err(incomplete(format!("read {start} of {total} CKAN packages")));
                }
                Ok((all_datasets, Vec::new()))
            }
        }
    }
}

type Gathered = (Vec<(String, Dataset)>, Vec<HarvestSkippedItem>);

impl HarvestEngine {
    /// Keeps the items the platform converted (`source.converted`), each with its Endpoint's
    /// distributions in place of the source's links (prod's `od_standards`, EP-118); the others
    /// are skipped and, when an earlier run listed them, withdrawn. An Endpoint's model index
    /// that does not read fails the run: an outage, not a catalogue to empty.
    async fn convert(
        &self,
        source: &HarvestSource,
        cat_allowed_hosts: &[String],
        lang: &str,
        hidden: &BTreeMap<String, String>,
        kept: Vec<(String, Dataset)>,
        mut skipped: Vec<HarvestSkippedItem>,
    ) -> Result<Gathered> {
        let gateway = source
            .gateway
            .as_deref()
            .map(|g| g.trim_end_matches('/'))
            .filter(|g| !g.is_empty())
            .ok_or_else(|| {
                DataroomError::field_error(
                    "gateway",
                    "a source with converted datasets needs the gateway their Endpoints answer under",
                )
            })?;
        let total = kept.len();
        let mut indexes: BTreeMap<String, Value> = BTreeMap::new();
        let mut out = Vec::new();
        for (item_id, mut ds) in kept {
            let title = dataroom_core::text_in_lang(&ds.title, None)
                .unwrap_or(&ds.name)
                .to_owned();
            let Some(entry) = source.converted.get(&ds.name) else {
                skipped.push(HarvestSkippedItem {
                    item_id,
                    title,
                    reason: "not converted to NGSI-LD, so not listed (EP-118)".into(),
                });
                continue;
            };
            let root = format!("{gateway}/api/endpoint/{}/", entry.endpoint);
            if !indexes.contains_key(&entry.endpoint) {
                let (_, body) = self
                    .checked_fetch(
                        &format!("{root}schema/index.json"),
                        source,
                        cat_allowed_hosts,
                        "application/json",
                    )
                    .await?;
                let index: Value = serde_json::from_str(&body).map_err(|e| {
                    DataroomError::OutboundRefused(format!(
                        "the schema index of Endpoint {} is not JSON: {e}",
                        entry.endpoint
                    ))
                })?;
                indexes.insert(entry.endpoint.clone(), index);
            }
            let dists = crate::converted::distributions(
                &root,
                entry,
                &indexes[&entry.endpoint],
                &ds.id,
                lang,
            );
            if dists.is_empty() {
                skipped.push(HarvestSkippedItem {
                    item_id,
                    title,
                    reason: format!(
                        "the model of {} on its Endpoint lacks an artefact, so not listed (EP-118)",
                        entry.entity_type
                    ),
                });
                continue;
            }
            ds.distributions = dists;
            // prod's sheet (`od_sheet`): for a dataset the publish gate leaves visible whose
            // Endpoint answers the file. A refusal (a 4xx, a body over the cap) means none, as on
            // prod; a 429, 5xx or broken read is read again as prod reads it, and where prod then
            // fails the whole run, means none this run and is asked again on the next (C9: the
            // other datasets go on).
            // ponytail: the probe reads the whole file under MAX_HARVEST_BYTES; prod's cap is 50 MB.
            if !hidden.contains_key(&ds.name) {
                let url = crate::converted::sheet_address(&root, entry);
                if self.sheet_answers(&url, source, cat_allowed_hosts).await {
                    let n = ds.distributions.len();
                    ds.distributions
                        .push(crate::converted::sheet(&root, entry, &ds.id, lang, n));
                }
            }
            // The platform's own dataset, not a copy of the source's: its IRI is the catalogue's
            // (prod's DCAT names it by its CKAN address), the source stays its landing page.
            ds.uri = None;
            ds.endpoint_url = Some(root.clone());
            ds.extras.insert("endpoint".into(), root);
            let hash = crate::sources::compute_harvest_hash(&ds);
            ds.harvest_hash = Some(hash.clone());
            ds.extras.insert("harvest_hash".into(), hash);
            out.push((item_id, ds));
        }
        if out.is_empty() {
            return Err(DataroomError::OutboundRefused(format!(
                "none of {total} datasets lists the platform's distributions; nothing is written or withdrawn"
            )));
        }
        Ok((out, skipped))
    }
}

impl HarvestRunner for HarvestEngine {
    /// One run of `source`: refused with a conflict while another run of it holds the source's
    /// Running row, stopped and recorded failed after the deadline, a failure always leaves a
    /// Failed row behind.
    fn run_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        store: &'a dyn Store,
        source: &'a HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestRun>> {
        Box::pin(async move {
            let started_at = Utc::now();
            let run_id = format!("run-{}", Uuid::now_v7());
            let mut run = HarvestRun {
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                id: run_id.clone(),
                source_id: source.id.clone(),
                state: HarvestRunState::Running,
                started_at,
                finished_at: None,
                created_count: 0,
                updated_count: 0,
                unchanged_count: 0,
                withdrawn_count: 0,
                skipped: Vec::new(),
                error: None,
                logs: Vec::new(),
            };
            let deadline = chrono::Duration::from_std(self.deadline)
                .unwrap_or_else(|_| chrono::Duration::hours(1));
            if !store
                .claim_harvest_run(ctx, run.clone(), started_at - deadline)
                .await?
            {
                return Err(DataroomError::Conflict(format!(
                    "harvest source '{}' has a run in progress",
                    source.name
                )));
            }
            let (err, fingerprint) = match tokio::time::timeout(
                self.deadline,
                self.execute(ctx, store, source, run_id, started_at),
            )
            .await
            {
                Ok(Ok(done)) => return Ok(done),
                Ok(Err(err)) => (err, "harvest.error"),
                Err(_) => (
                    DataroomError::OutboundRefused(format!(
                        "the run passed its deadline of {} s",
                        self.deadline.as_secs()
                    )),
                    "harvest.deadline",
                ),
            };
            // A fetch failure wrote its own Failed row; anything else still holds the lock.
            let still_running = store
                .get_harvest_run(ctx, &run.id)
                .await
                .ok()
                .flatten()
                .is_none_or(|r| r.state == HarvestRunState::Running);
            if still_running {
                run.state = HarvestRunState::Failed;
                run.finished_at = Some(Utc::now());
                run.error = Some(err.to_string());
                run.logs.push(HarvestLogLine {
                    at: Utc::now(),
                    level: "error".into(),
                    fingerprint: fingerprint.into(),
                    message: err.to_string(),
                });
                store.upsert_harvest_run(ctx, run).await?;
            }
            Err(err)
        })
    }
}

impl HarvestEngine {
    async fn execute(
        &self,
        ctx: &RequestCtx,
        store: &dyn Store,
        source: &HarvestSource,
        run_id: String,
        started_at: chrono::DateTime<Utc>,
    ) -> Result<HarvestRun> {
        {
            let mut logs = Vec::new();

            let cat = store
                .get_catalogue(ctx.org_id, ctx.catalogue_id)
                .await?
                .ok_or_else(|| DataroomError::NotFound("catalogue not found".into()))?;

            // Ensure target publisher exists (`harvest-hub.py` line 213).
            if store
                .get_publisher(ctx, &source.publisher_id)
                .await?
                .is_none()
            {
                return Err(DataroomError::Validation {
                    message: format!("publisher '{}' does not exist", source.publisher_id),
                    fields: BTreeMap::new(),
                });
            }

            let gather_res = self
                .gather_items(
                    ctx,
                    store,
                    source,
                    &cat.allowed_outbound_hosts,
                    &cat.default_language,
                )
                .await;

            let gather_res = match gather_res {
                Ok((kept, skipped)) if !source.converted.is_empty() => {
                    self.convert(
                        source,
                        &cat.allowed_outbound_hosts,
                        &cat.default_language,
                        &cat.hidden_datasets,
                        kept,
                        skipped,
                    )
                    .await
                }
                other => other,
            };
            let gather_res = gather_res.and_then(|(kept, skipped)| {
                if kept.is_empty() {
                    // An answer with nothing to keep is an outage, not a catalogue to empty.
                    Err(DataroomError::OutboundRefused(
                        "source listed 0 items to keep; nothing is written or withdrawn".into(),
                    ))
                } else {
                    Ok((kept, skipped))
                }
            });
            let (kept_items, mut skipped) = match gather_res {
                Ok(pair) => pair,
                Err(err) => {
                    logs.push(HarvestLogLine {
                        at: Utc::now(),
                        level: "error".into(),
                        fingerprint: format!("harvest.{}.fetch_error", source.kind.as_str()),
                        message: err.to_string(),
                    });
                    let failed_run = HarvestRun {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        id: run_id,
                        source_id: source.id.clone(),
                        state: HarvestRunState::Failed,
                        started_at,
                        finished_at: Some(Utc::now()),
                        created_count: 0,
                        updated_count: 0,
                        unchanged_count: 0,
                        withdrawn_count: 0,
                        skipped: Vec::new(),
                        error: Some(err.to_string()),
                        logs,
                    };
                    store.upsert_harvest_run(ctx, failed_run).await?;
                    return Err(err);
                }
            };

            // Index existing datasets from this harvest source by `harvest_source_item_id`.
            let existing_all = store.list_datasets(ctx).await?;
            let mut existing_by_item: BTreeMap<String, Dataset> = BTreeMap::new();
            // Ids and names of every record this source does not hold: a local dataset or another
            // source's. A harvested item never takes one of them over (C9, C16).
            let mut foreign: BTreeMap<String, String> = BTreeMap::new();
            for ds in existing_all {
                let hs = ds
                    .harvest_source_id
                    .as_deref()
                    .or_else(|| ds.extras.get("harvest_source").map(String::as_str));
                if hs == Some(source.name.as_str()) || hs == Some(source.id.as_str()) {
                    let item_id = ds
                        .harvest_source_item_id
                        .clone()
                        .or_else(|| ds.extras.get("source_id").cloned())
                        .or_else(|| ds.extras.get("source_uri").cloned())
                        .unwrap_or_else(|| ds.name.clone());
                    existing_by_item.insert(item_id, ds);
                } else {
                    foreign.insert(ds.id.clone(), ds.name.clone());
                    foreign.insert(ds.name.clone(), ds.name.clone());
                }
            }

            let mut created_count = 0usize;
            let mut updated_count = 0usize;
            let mut unchanged_count = 0usize;
            let mut withdrawn_count = 0usize;

            for (item_id, mut incoming) in kept_items {
                incoming.harvest_source_id = Some(source.name.clone());
                incoming.harvest_source_item_id = Some(item_id.clone());
                if let Some(old) = existing_by_item.remove(&item_id) {
                    incoming.id = old.id.clone();
                    incoming.name = old.name.clone();
                    if old.state == RecordState::Active
                        && old.harvest_hash.is_some()
                        && old.harvest_hash == incoming.harvest_hash
                    {
                        unchanged_count += 1;
                    } else {
                        incoming.state = RecordState::Active;
                        store.upsert_dataset(ctx, incoming).await?;
                        updated_count += 1;
                    }
                } else if let Some(held) = foreign
                    .get(&incoming.id)
                    .or_else(|| foreign.get(&incoming.name))
                {
                    skipped.push(HarvestSkippedItem {
                        item_id,
                        title: dataroom_core::text_in_lang(&incoming.title, None)
                            .unwrap_or(&incoming.name)
                            .to_owned(),
                        reason: format!(
                            "dataset '{held}' already exists and does not belong to this source"
                        ),
                    });
                } else {
                    let (id, name) = (incoming.id.clone(), incoming.name.clone());
                    store.upsert_dataset(ctx, incoming).await?;
                    foreign.insert(id, name.clone());
                    foreign.insert(name.clone(), name);
                    created_count += 1;
                }
            }

            // Withdraw any previously harvested dataset from this source that was not in the complete read.
            for (gone_item_id, gone_ds) in existing_by_item {
                if gone_ds.state == RecordState::Active {
                    store.withdraw_dataset(ctx, &gone_ds.id).await?;
                    withdrawn_count += 1;
                    logs.push(HarvestLogLine {
                        at: Utc::now(),
                        level: "info".into(),
                        fingerprint: "harvest.withdrawn".into(),
                        message: format!(
                            "Withdrew dataset '{}' (source item {})",
                            gone_ds.name, gone_item_id
                        ),
                    });
                }
            }

            for sk in &skipped {
                logs.push(HarvestLogLine {
                    at: Utc::now(),
                    level: "info".into(),
                    fingerprint: "harvest.skipped".into(),
                    message: format!("Skipped {} {:?}: {}", sk.item_id, sk.title, sk.reason),
                });
            }

            let run = HarvestRun {
                org_id: ctx.org_id,
                catalogue_id: ctx.catalogue_id,
                id: run_id,
                source_id: source.id.clone(),
                state: HarvestRunState::Succeeded,
                started_at,
                finished_at: Some(Utc::now()),
                created_count,
                updated_count,
                unchanged_count,
                withdrawn_count,
                skipped,
                error: None,
                logs,
            };
            let saved_run = store.upsert_harvest_run(ctx, run).await?;
            store
                .record_activity(
                    ctx,
                    ActivityRow {
                        org_id: ctx.org_id,
                        catalogue_id: ctx.catalogue_id,
                        id: format!("act-{}", Uuid::now_v7()),
                        at: Utc::now(),
                        actor: ctx.actor_name().to_owned(),
                        operation: "harvest.run".into(),
                        object_type: "HarvestSource".into(),
                        object_id: source.id.clone(),
                        summary: format!(
                            "Harvest {}: created {}, updated {}, unchanged {}, withdrawn {}",
                            source.name,
                            created_count,
                            updated_count,
                            unchanged_count,
                            withdrawn_count
                        ),
                        details: json!({
                            "run_id": saved_run.id,
                            "created": created_count,
                            "updated": updated_count,
                            "unchanged": unchanged_count,
                            "withdrawn": withdrawn_count,
                        }),
                    },
                )
                .await?;
            Ok(saved_run)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::validate_outbound_url;
    use dataroom_core::{Catalogue, CatalogueId, OrgId, Publisher, Subject, ThemeTokens};
    use dataroom_store_memory::MemoryStore;

    fn test_setup() -> (OrgId, CatalogueId, RequestCtx, MemoryStore) {
        let org_id = OrgId::from_uuid(Uuid::from_u128(1));
        let catalogue_id = CatalogueId::from_uuid(Uuid::from_u128(10));
        let ctx = RequestCtx::new(
            org_id,
            catalogue_id,
            Subject::authenticated("harvester", vec![org_id], Vec::new()),
        );
        let store = MemoryStore::new();
        (org_id, catalogue_id, ctx, store)
    }

    #[test]
    fn ssrf_guard_refuses_private_ips_plain_http_and_unlisted_hosts() {
        let allowed = vec!["opendata.bbsk.sk".to_owned()];
        // 1. Plain http refused
        assert!(
            validate_outbound_url("http://opendata.bbsk.sk/api/items", &allowed, &[], false)
                .is_err()
        );

        // 2. Private IPv4 ranges, loopback, and cloud metadata refused even if added to allowlist
        for internal in [
            "https://127.0.0.1/api/3/action/package_search",
            "https://10.0.0.1/catalog.ttl",
            "https://172.16.5.4/catalog.ttl",
            "https://192.168.1.1/catalog.ttl",
            "https://169.254.169.254/latest/meta-data/",
            "https://100.64.0.1/catalog.ttl",
            "https://[::1]/catalog.ttl",
            "https://localhost/catalog.ttl",
            "https://ckan.default.svc.cluster.local/api",
        ] {
            let host = url::Url::parse(internal)
                .unwrap()
                .host_str()
                .unwrap()
                .to_owned();
            assert!(
                validate_outbound_url(internal, &[host], &[], false).is_err(),
                "{internal} must be refused by SSRF guard"
            );
        }

        // 3. Host outside allowlist refused
        assert!(validate_outbound_url(
            "https://evil.example.org/catalog.ttl",
            &allowed,
            &[],
            false
        )
        .is_err());

        // 4. URL with embedded credentials refused
        assert!(validate_outbound_url(
            "https://user:secret@opendata.bbsk.sk/api",
            &allowed,
            &[],
            false
        )
        .is_err());

        // 5. Allowed public https URL passes
        assert!(validate_outbound_url(
            "https://opendata.bbsk.sk/api/search/v1/collections/dataset/items?limit=100",
            &allowed,
            &[],
            false
        )
        .is_ok());
    }

    #[tokio::test]
    async fn harvest_creates_deduplicates_keeps_unchanged_and_withdraws_dropped_items() {
        let (org_id, catalogue_id, ctx, store) = test_setup();
        store
            .upsert_catalogue(Catalogue {
                org_id,
                id: catalogue_id,
                name: "bbsk".into(),
                host: "opendata.bbsk.sk".into(),
                title: BTreeMap::from([("sk".into(), "BBSK".into())]),
                description: BTreeMap::from([("sk".into(), "Katalóg".into())]),
                homepage: None,
                default_language: "sk".into(),
                languages: vec!["sk".into(), "en".into()],
                main_profile: "dcat-ap-sk-2.0".into(),
                active_profiles: Vec::new(),
                default_licence: Some("cc-by".into()),
                publisher_uri: Some("https://data.gov.sk/id/legal-subject/37828100".into()),
                publisher_name: None,
                contact_point: None,
                theme: ThemeTokens::default(),
                branding: None,
                hidden_datasets: Default::default(),
                allow_cross_org_harvest: true,
                allowed_outbound_hosts: vec!["hub.bbsk.sk".into()],
                applicable_legislation: Vec::new(),
                modified: Utc::now(),
                deleted: None,
                public_base: None,
            })
            .await
            .unwrap();

        store
            .upsert_publisher(
                &ctx,
                Publisher {
                    org_id,
                    catalogue_id,
                    id: "pub-bbsk".into(),
                    name: "bbsk".into(),
                    title: BTreeMap::from([("sk".into(), "BBSK".into())]),
                    description: BTreeMap::new(),
                    uri: "https://data.gov.sk/id/legal-subject/37828100".into(),
                    publisher_type: None,
                    identifier: None,
                    email: None,
                    homepage: None,
                    logo_url: None,
                    contact_point: None,
                    state: RecordState::Active,
                    merged_into: None,
                    editor_grants: Vec::new(),
                    badge_initials: None,
                    badge_colour: None,
                    declared_by_instance: None,
                    created: Utc::now(),
                    modified: Utc::now(),
                },
            )
            .await
            .unwrap();

        let source = HarvestSource {
            org_id,
            catalogue_id,
            id: "hs-hub".into(),
            name: "hub-bbsk".into(),
            kind: HarvestSourceKind::ArcGisHub,
            url: "https://hub.bbsk.sk".into(),
            publisher_id: "bbsk".into(),
            target_publisher_iri: Some("https://data.gov.sk/id/legal-subject/37828100".into()),
            spatial_uri: Some("http://data.europa.eu/nuts/code/SK032".into()),
            name_prefix: String::new(),
            allowed_hosts: vec!["hub.bbsk.sk".into()],
            rewrite_hosts: BTreeMap::new(),
            skip_items: BTreeMap::new(),
            title_overrides: BTreeMap::new(),
            spelling_fixes: BTreeMap::new(),
            require_column: BTreeMap::new(),
            converted: BTreeMap::new(),
            gateway: None,
            schedule: "23 3 * * *".into(),
            enabled: true,
            modified: Utc::now(),
        };
        store
            .upsert_harvest_source(&ctx, source.clone())
            .await
            .unwrap();

        let fetcher = MapFetcher::new();
        let hub_url = "https://hub.bbsk.sk/api/search/v1/collections/dataset/items?limit=100&sortBy=properties.created";

        // Run 1: 2 valid open datasets + 1 closed-licence item (skipped)
        fetcher.insert(
            hub_url,
            Some("application/json"),
            json!({
                "numberMatched": 3,
                "features": [
                    {
                        "properties": {
                            "id": "item111111",
                            "title": "Cesty II. a III. triedy",
                            "type": "Feature Service",
                            "license": "CC-BY-4.0",
                            "url": "https://hub.bbsk.sk/arcgis/rest/services/cesty/FeatureServer",
                            "description": "<p>Správa ciest</p>",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["doprava", "cesty"]
                        }
                    },
                    {
                        "properties": {
                            "id": "item222222",
                            "title": "Stredné školy",
                            "type": "Shapefile",
                            "license": "CC0-1.0",
                            "description": "Zoznam škôl",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["skoly"]
                        }
                    },
                    {
                        "properties": {
                            "id": "item333333",
                            "title": "Interná mapa",
                            "type": "Shapefile",
                            "license": "Proprietary"
                        }
                    }
                ],
                "links": []
            })
            .to_string(),
        );

        let engine = HarvestEngine::new(Arc::new(fetcher.clone())).with_dns_resolution(false);
        let run1 = engine
            .run_source(&ctx, &store, &source)
            .await
            .expect("run 1");
        assert_eq!(run1.created_count, 2);
        assert_eq!(run1.updated_count, 0);
        assert_eq!(run1.unchanged_count, 0);
        assert_eq!(run1.withdrawn_count, 0);
        assert_eq!(run1.skipped.len(), 1);

        // Run 2: item222222 is dropped by the source; item111111 is unchanged -> item222222 is withdrawn!
        fetcher.insert(
            hub_url,
            Some("application/json"),
            json!({
                "numberMatched": 1,
                "features": [
                    {
                        "properties": {
                            "id": "item111111",
                            "title": "Cesty II. a III. triedy",
                            "type": "Feature Service",
                            "license": "CC-BY-4.0",
                            "url": "https://hub.bbsk.sk/arcgis/rest/services/cesty/FeatureServer",
                            "description": "<p>Správa ciest</p>",
                            "created": 1700000000000i64,
                            "modified": 1700000000000i64,
                            "tags": ["doprava", "cesty"]
                        }
                    }
                ],
                "links": []
            })
            .to_string(),
        );

        let run2 = engine
            .run_source(&ctx, &store, &source)
            .await
            .expect("run 2");
        assert_eq!(run2.created_count, 0);
        assert_eq!(run2.unchanged_count, 1);
        assert_eq!(run2.withdrawn_count, 1);

        let all_ds = store.list_datasets(&ctx).await.unwrap();
        let active: Vec<&str> = all_ds
            .iter()
            .filter(|d| d.state == RecordState::Active)
            .map(|d| d.name.as_str())
            .collect();
        let withdrawn: Vec<&str> = all_ds
            .iter()
            .filter(|d| d.state == RecordState::Withdrawn)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(withdrawn.len(), 1);
    }

    /// The own gateway shortcut reaches the loopback listener for an Endpoint read alone: any
    /// other path under the origin (another module of the listener, an address a remote page or
    /// an editor wrote) stays with the guard, which refuses loopback.
    #[test]
    fn the_own_gateway_reaches_only_endpoint_reads_on_the_loopback_listener() {
        let internal: SocketAddr = "127.0.0.1:42102".parse().unwrap();
        let engine = HarvestEngine::new(Arc::new(MapFetcher::new()))
            .with_own_gateway("http://localhost:42102/", internal);
        let own = |u: &str| engine.own_target(u).expect("no parse error");
        let read = own("http://localhost:42102/api/endpoint/abc/ngsi-ld/v1/entities?type=T")
            .expect("shortcut");
        assert_eq!(
            read.url.as_str(),
            "http://127.0.0.1:42102/api/endpoint/abc/ngsi-ld/v1/entities?type=T"
        );
        assert_eq!(read.pinned_addrs, vec![internal]);
        for other in [
            "http://localhost:42102/portal/api/v1/projects",
            "http://localhost:42102/streams/streams",
            "http://localhost:42102/",
            "http://localhost:42102/api/endpoint/../portal/api",
            "http://localhost:42102/api/endpoint/a/../../portal",
            "http://localhost:42102/api/endpoint/%2e%2e/portal",
            "http://localhost:42102/api/endpoint/a%2F..%2F..%2Fportal",
            "http://localhost:42102/api/endpoint\\..\\portal",
            "http://localhost:421020/api/endpoint/a",
            "http://localhost:42102.evil.example/api/endpoint/a",
        ] {
            assert!(own(other).is_none(), "{other}");
        }
    }

    /// A filtered dataset's sheet probe carries its `isPartOf` IRI percent-encoded in the query
    /// (`%2F`): the query reaches no mount, so the shortcut takes it; the path check stays.
    #[test]
    fn the_own_gateway_takes_an_endpoint_read_whose_query_is_encoded() {
        let internal: SocketAddr = "127.0.0.1:42102".parse().unwrap();
        let engine = HarvestEngine::new(Arc::new(MapFetcher::new()))
            .with_own_gateway("http://localhost:42102", internal);
        let own = |u: &str| engine.own_target(u).expect("no parse error");
        let sheet = "/api/endpoint/bbsk-poi/file.csv?type=PointOfInterest\
                     &q=isPartOf%3D%3D%22https%3A%2F%2Fdata.example%2Fds%2Fa%5Cb%2e%22";
        let read = own(&format!("http://localhost:42102{sheet}")).expect("shortcut");
        assert_eq!(read.url.as_str(), format!("http://127.0.0.1:42102{sheet}"));
        assert!(own("http://localhost:42102/api/endpoint/a/file.csv#x%2F").is_some());
        for other in [
            "http://localhost:42102/api/endpoint/a%2F..%2F..%2Fportal?q=1",
            "http://localhost:42102/api/endpoint/%2e%2e/portal?q=%2F",
            "http://localhost:42102/api/endpoint\\..\\portal?q=1",
            "http://localhost:42102/api/endpoint/a/../../portal?q=%2F",
            "http://localhost:42102/portal/api?x=/api/endpoint/",
        ] {
            assert!(own(other).is_none(), "{other}");
        }
    }

    /// prod's `_od_transient`: a 429, a 5xx and a broken connection are read again; a 4xx, a
    /// body over the cap and a refused or unscripted address are the answer.
    #[test]
    fn a_read_is_asked_again_only_when_it_failed_by_itself() {
        let refused = |m: &str| DataroomError::OutboundRefused(m.to_owned());
        for m in [
            "http://127.0.0.1:1/api/endpoint/a/file.csv answered HTTP 503",
            "https://x.example/f answered HTTP 429",
            "https://x.example/f answered HTTP 500",
            "fetch https://x.example/f: connection reset",
            "read https://x.example/f: unexpected EOF",
        ] {
            assert!(transient(&refused(m)), "{m}");
        }
        for m in [
            "https://x.example/f answered HTTP 413",
            "https://x.example/f answered HTTP 404",
            "https://x.example/f body exceeded cap of 10 bytes",
            "no response for https://x.example/f",
            "scheme 'http' is refused",
        ] {
            assert!(!transient(&refused(m)), "{m}");
        }
        assert!(!transient(&DataroomError::Forbidden("read x".into())));
    }

    #[tokio::test]
    async fn a2_http_fetcher_refuses_a_target_without_checked_addresses() {
        let target = ValidatedTarget {
            url: url::Url::parse("https://allowed.example.org/catalog.ttl").unwrap(),
            host: "allowed.example.org".into(),
            pinned_addrs: Vec::new(),
        };
        let res = HttpFetcher::new()
            .fetch_hop(&target, "text/turtle", 1024, Duration::from_secs(1))
            .await;
        assert!(
            matches!(res, Err(DataroomError::OutboundRefused(ref m)) if m.contains("unpinned")),
            "got {:?}",
            res.map(|_| ())
        );
    }

    #[tokio::test]
    async fn a2_redirect_ssrf_guards_every_hop_pins_dns_and_enforces_caps() {
        let (org_id, catalogue_id, _ctx, _store) = test_setup();
        let source = HarvestSource {
            org_id,
            catalogue_id,
            id: "hs-ssrf".into(),
            name: "ssrf-test".into(),
            kind: HarvestSourceKind::DcatAp,
            url: "https://allowed.example.org/start.ttl".into(),
            publisher_id: "bbsk".into(),
            target_publisher_iri: None,
            spatial_uri: None,
            name_prefix: String::new(),
            allowed_hosts: vec![
                "allowed.example.org".into(),
                "rebind.example.org".into(),
                "169.254.169.254".into(),
            ],
            rewrite_hosts: BTreeMap::new(),
            skip_items: BTreeMap::new(),
            title_overrides: BTreeMap::new(),
            spelling_fixes: BTreeMap::new(),
            require_column: BTreeMap::new(),
            converted: BTreeMap::new(),
            gateway: None,
            schedule: "23 3 * * *".into(),
            enabled: true,
            modified: Utc::now(),
        };

        let fetcher = MapFetcher::new();
        // 1. Redirect to cloud metadata 169.254.169.254 -> refused!
        fetcher.insert_redirect(
            "https://allowed.example.org/to-metadata",
            302,
            "https://169.254.169.254/latest/meta-data/",
        );
        // 2. Redirect to a DNS name that resolves to 127.0.0.1 -> refused!
        fetcher.insert_redirect(
            "https://allowed.example.org/to-loopback-dns",
            302,
            "https://rebind.example.org/secret.ttl",
        );
        // 3. Redirect to an unlisted host -> refused!
        fetcher.insert_redirect(
            "https://allowed.example.org/to-unlisted",
            302,
            "https://unlisted.example.org/catalog.ttl",
        );
        // 4. Redirect loop (> 5 hops) -> refused!
        fetcher.insert_redirect(
            "https://allowed.example.org/loop-a",
            302,
            "https://allowed.example.org/loop-b",
        );
        fetcher.insert_redirect(
            "https://allowed.example.org/loop-b",
            302,
            "https://allowed.example.org/loop-a",
        );
        // 5. Body over the per-source byte cap -> refused!
        fetcher.insert(
            "https://allowed.example.org/huge.ttl",
            Some("text/turtle"),
            "x".repeat(1024),
        );
        // 6. Valid redirect hop pins DNS answer for the connection
        fetcher.insert_redirect(
            "https://allowed.example.org/valid-hop",
            301,
            "https://allowed.example.org/final.ttl",
        );
        fetcher.insert(
            "https://allowed.example.org/final.ttl",
            Some("text/turtle"),
            "@prefix dcat: <http://www.w3.org/ns/dcat#> .",
        );

        let engine = HarvestEngine::new(Arc::new(fetcher.clone()))
            .with_max_bytes(512)
            .with_custom_dns(|host, port| match host {
                "allowed.example.org" => Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))]),
                "rebind.example.org" => Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]),
                other => Err(DataroomError::OutboundRefused(format!(
                    "unknown host {other}"
                ))),
            });

        // Test 1: Redirect to 169.254.169.254 refused
        let err_meta = engine
            .checked_fetch(
                "https://allowed.example.org/to-metadata",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .unwrap_err();
        assert!(
            err_meta.to_string().contains("169.254.169.254"),
            "expected metadata IP refusal, got {err_meta}"
        );

        // Test 2: Redirect to 127.0.0.1 by DNS name refused
        let err_dns = engine
            .checked_fetch(
                "https://allowed.example.org/to-loopback-dns",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .unwrap_err();
        assert!(
            err_dns.to_string().contains("127.0.0.1"),
            "expected loopback DNS refusal, got {err_dns}"
        );

        // Test 3: Redirect to unlisted host refused
        let err_unlisted = engine
            .checked_fetch(
                "https://allowed.example.org/to-unlisted",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .unwrap_err();
        assert!(
            err_unlisted.to_string().contains("unlisted.example.org"),
            "expected unlisted host refusal, got {err_unlisted}"
        );

        // Test 4: Redirect loop refused after 5 hops
        let err_loop = engine
            .checked_fetch(
                "https://allowed.example.org/loop-a",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .unwrap_err();
        assert!(
            err_loop.to_string().contains("redirect limit of 5 hops"),
            "expected redirect loop refusal, got {err_loop}"
        );

        // Test 5: Body over the cap refused
        let err_cap = engine
            .checked_fetch(
                "https://allowed.example.org/huge.ttl",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .unwrap_err();
        assert!(
            err_cap.to_string().contains("exceeded cap of 512 bytes"),
            "expected byte cap refusal, got {err_cap}"
        );

        // Test 6: Valid redirect hop succeeds and pins the resolved public IP for each hop
        let (_, ok_body) = engine
            .checked_fetch(
                "https://allowed.example.org/valid-hop",
                &source,
                &[],
                "text/turtle",
            )
            .await
            .expect("valid hop should succeed");
        assert!(ok_body.contains("dcat:"));
        let pinned = fetcher.pinned_seen();
        assert!(pinned
            .iter()
            .all(|(_, addrs)| addrs == &vec![SocketAddr::from(([93, 184, 216, 34], 443))]));
    }
}
