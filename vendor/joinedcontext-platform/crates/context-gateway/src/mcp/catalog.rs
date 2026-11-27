//! `endpoint: catalog` on the hub and on every named server: the caller's own catalogue (T-4503,
//! ADR-N-025).
//!
//! An agent's first call names the catalogue and finds a dataset with the ordinary NGSI-LD query;
//! its second names the Endpoint the dataset names. The public catalogue is broker data the
//! Portal writes (T-4500, T-4746); this one is built per caller from the Endpoints the request
//! reaches, by the same function from the same DCAT-AP records ([`jc_core::ngsi_catalog`]), and
//! takes the CKAN-named `Dataset` and `Organization` entities from the public one, which only the
//! Portal can build because only it reads CKAN. [`jc_core::ngsi_catalog::link`] keeps those whose
//! Endpoint the caller reaches, so the anonymous caller's catalogue is the public one entity for
//! entity, and a caller with grants sees, beside it, the Endpoints its token opens and why. That
//! read of the public catalogue, at most once a minute for every caller, is the only broker read
//! here (EP-87, EP-140): the records are built without the schema index, which the catalogue
//! does not list.
//!
//! An Endpoint outside the request's reach gives no entity, no word of `searchText` and no
//! count, because the catalogue is computed from `reach` and nothing else; a named server's
//! reach is already its members, so a connector can only narrow it.

use super::jsonrpc::{answered, error, refused, result};
use crate::app::Gateway;
use crate::auth::subject::Subject;
use crate::handlers::{endpoint_surface, schema};
use crate::resolver::Endpoint;
use antares_jsonld::{expand_entity, expand_types, Context, ExpandOpts};
use jc_core::kinds::{Audience, QualityKind};
use jc_core::ngsi_catalog::{self, Place};
use serde_json::{json, Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The name a tool call gives the catalogue as its `endpoint`.
pub(crate) const NAME: &str = "catalog";

/// The tools the catalogue answers; every other one is the dataset's own.
pub(crate) const TOOLS: &[&str] = &["query_entities", "get_entity", "list_types"];

/// The arguments `query_entities` takes on the catalogue.
const ARGUMENTS: &[&str] = &[
    "type",
    "id",
    "idPattern",
    "q",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "attrs",
    "pick",
    "format",
    "count",
    "limit",
    "cursor",
];

/// A page when the caller names no `limit`, as the broker's own default.
const DEFAULT_LIMIT: usize = 20;

/// Catalogues kept between requests; past it the cache starts again empty.
const CACHED: usize = 64;

/// How long the public catalogue's Datasets and Organizations are reused before they are read
/// again (EP-87).
const PUBLIC_FOR: Duration = Duration::from_secs(60);

/// Entities per page of that read, and the pages it reads at most.
const PUBLIC_PAGE: usize = 1000;
const PUBLIC_PAGES: usize = 20;

/// Whether a tool call's `endpoint` names the catalogue.
pub(crate) fn names_catalog(named: &str) -> bool {
    named.trim().eq_ignore_ascii_case(NAME)
}

/// One caller's catalogue: the entities, each beside its expanded form the matcher reads.
struct Catalog {
    /// The Endpoints it was built from, held so that a reload, which builds new ones, is a miss
    /// and no address of them is reused while the entry lives.
    built_from: Vec<Arc<Endpoint>>,
    /// The subject each of them was read as, compared on a hit: the key is a hash, and a
    /// collision never serves one caller another's catalogue.
    built_for: Vec<Subject>,
    /// The public read it took its Datasets and Organizations from.
    public: Arc<Vec<Value>>,
    entities: Vec<(Value, Value)>,
    /// The Datasets ranked by words, for the hub's `search` (T-4493).
    index: super::search::Index,
}

impl Catalog {
    /// Whether this entry answers a caller reaching `listed` while the public read is `public`:
    /// the same Endpoints, read as the same subjects.
    fn serves(&self, public: &Arc<Vec<Value>>, listed: &[(&Arc<Endpoint>, &Subject)]) -> bool {
        Arc::ptr_eq(&self.public, public)
            && self.built_from.len() == listed.len()
            && self.built_for.len() == listed.len()
            && self.built_from.iter().zip(&self.built_for).zip(listed).all(
                |((held, read_as), (endpoint, subject))| {
                    Arc::ptr_eq(held, endpoint) && read_as == *subject
                },
            )
    }
}

fn cache() -> &'static Mutex<HashMap<u64, Arc<Catalog>>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Catalog>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn core() -> &'static Context {
    static CORE: OnceLock<Context> = OnceLock::new();
    CORE.get_or_init(antares_jsonld::core_context)
}

/// The space of the broker-backed public catalogue itself, which is not a dataset of its own.
fn is_catalog_space(endpoint: &Endpoint) -> bool {
    endpoint.space == NAME || endpoint.space == format!("{}-{NAME}", endpoint.project)
}

/// The installation's public catalogue Endpoint in reach, under either space name the catalogue
/// takes (`catalog` or the rendered `{project}-catalog`).
fn public_catalog(reach: &[(Arc<Endpoint>, Subject)]) -> Option<&Arc<Endpoint>> {
    reach
        .iter()
        .map(|(endpoint, _)| endpoint)
        .find(|endpoint| is_catalog_space(endpoint) && endpoint.audience == Audience::Public)
}

/// The public catalogue's CKAN-named `Dataset` and `Organization` entities, read through the
/// installation's `catalog` Endpoint as the anonymous caller, at most once a minute for every
/// caller: the one list every caller shares is what anyone may read, never what the first caller
/// to reach it happened to hold. Without that Endpoint in
/// reach the list is empty; a read that fails keeps the last one it had.
async fn public_entities(gateway: &Gateway, reach: &[(Arc<Endpoint>, Subject)]) -> Arc<Vec<Value>> {
    // By the catalogue Endpoint's slug: a reload that changed it reads afresh.
    type Held = HashMap<String, (Instant, Arc<Vec<Value>>)>;
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    // ponytail: one read at a time for every catalogue; per-slug locks if an installation ever
    // serves more than one.
    static READING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let Some(endpoint) = public_catalog(reach) else {
        return Arc::new(Vec::new());
    };
    let held = HELD.get_or_init(Mutex::default);
    let last = || {
        held.lock()
            .ok()
            .and_then(|held| held.get(&endpoint.slug).cloned())
    };
    let fresh = |(read, _): &(Instant, Arc<Vec<Value>>)| read.elapsed() < PUBLIC_FOR;
    let before = last();
    if let Some((_, entities)) = before.as_ref().filter(|kept| fresh(kept)) {
        return Arc::clone(entities);
    }
    // Single flight: while one caller reads, the others answer from the expired list, or wait
    // for the first read when there is none, so an expiry costs the broker one read.
    let _reading = match (READING.try_lock(), &before) {
        (Ok(guard), _) => guard,
        (Err(_), Some((_, entities))) => return Arc::clone(entities),
        (Err(_), None) => READING.lock().await,
    };
    let before = last();
    if let Some((_, entities)) = before.as_ref().filter(|kept| fresh(kept)) {
        return Arc::clone(entities);
    }
    let anonymous = crate::auth::subject::anonymous_on(endpoint);
    let read = match read_public(gateway, endpoint, &anonymous).await {
        Ok(entities) => Arc::new(entities),
        Err(why) => {
            tracing::warn!(slug = %endpoint.slug, %why, "the public catalogue did not read");
            match before {
                Some((_, entities)) => entities,
                None => Arc::new(Vec::new()),
            }
        }
    };
    held.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(endpoint.slug.clone(), (Instant::now(), Arc::clone(&read)));
    read
}

/// The `query_entities` arguments of one page of the public catalogue: the types a list, as the
/// broker's tool takes them.
fn public_arguments(cursor: Option<Value>) -> Value {
    let mut arguments = json!({
        "type": ["Dataset", "Organization"],
        "format": "normalized",
        "limit": PUBLIC_PAGE,
    });
    if let Some(cursor) = cursor {
        arguments["cursor"] = cursor;
    }
    arguments
}

/// Every `Dataset` and `Organization` the `catalog` Endpoint holds, a page at a time.
async fn read_public(
    gateway: &Gateway,
    endpoint: &Endpoint,
    subject: &Subject,
) -> Result<Vec<Value>, String> {
    let mut entities = Vec::new();
    let mut cursor: Option<Value> = None;
    for _ in 0..PUBLIC_PAGES {
        let arguments = public_arguments(cursor.take());
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "query_entities", "arguments": arguments },
        });
        // A broker that does not answer holds this caller and every one waiting on READING for
        // one member call's deadline per page, never its own 120 s; a large catalogue that
        // answers each page in time is read whole.
        let answer = tokio::time::timeout(
            super::server::MEMBER_DEADLINE,
            super::endpoint::member_call(gateway, endpoint, subject, message),
        )
        .await
        .map_err(|_| "no answer within ten seconds".to_owned())?;
        let (page, next) = public_page(&answer)?;
        entities.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(entities),
        }
    }
    Err(format!(
        "more than {} entities; the catalogue reads no further",
        PUBLIC_PAGE * PUBLIC_PAGES
    ))
}

/// The Datasets and Organizations of one page of `query_entities`, and its `nextCursor`.
fn public_page(answer: &Value) -> Result<(Vec<Value>, Option<Value>), String> {
    if let Some(message) = answer.pointer("/error/message").and_then(Value::as_str) {
        return Err(message.to_owned());
    }
    let result = answer.get("result").unwrap_or(&Value::Null);
    if result.get("isError") == Some(&Value::Bool(true)) {
        let why = result.pointer("/content/0/text").and_then(Value::as_str);
        return Err(why
            .unwrap_or("the catalog Endpoint refused the read")
            .to_owned());
    }
    let structured = result.get("structuredContent").unwrap_or(&Value::Null);
    let entities = structured
        .get("entities")
        .and_then(Value::as_array)
        .ok_or("the catalog Endpoint answered no entities")?
        .iter()
        .filter(|entity| matches!(entity["type"].as_str(), Some("Dataset" | "Organization")))
        .map(|entity| {
            let mut entity = entity.clone();
            // The filter above kept objects only (a `type` member is read off one).
            entity.as_object_mut().into_iter().for_each(|members| {
                members.remove("@context");
            });
            entity
        })
        .collect();
    Ok((entities, structured.get("nextCursor").cloned()))
}

/// Why the caller sees an Endpoint it reaches.
pub(crate) fn because(endpoint: &Endpoint, subject: &Subject) -> &'static str {
    match endpoint.audience {
        Audience::Public => "public",
        _ if subject.service_account.is_some() => "grant",
        Audience::Organization => "organization",
        Audience::ProjectList => "project",
    }
}

/// The space's `area` rule, which is where `georel=near` finds its Dataset (DM-87).
pub(crate) fn bbox(endpoint: &Endpoint) -> Option<[f64; 4]> {
    let rules = endpoint.declared_types.as_ref()?;
    rules.quality.iter().find_map(|rule| {
        let corners = rule
            .bbox
            .as_ref()
            .filter(|_| rule.kind == QualityKind::Area)?;
        let mut out = [0.0; 4];
        for (slot, corner) in out.iter_mut().zip(corners) {
            *slot = corner.as_f64()?;
        }
        Some(out)
    })
}

/// The caller's catalogue, from the cache while the same Endpoints reach the same way and the
/// public catalogue was not read again.
async fn of(gateway: &Gateway, reach: &[(Arc<Endpoint>, Subject)]) -> Arc<Catalog> {
    let public = public_entities(gateway, reach).await;
    let listed: Vec<(&Arc<Endpoint>, &Subject)> = reach
        .iter()
        .filter(|(endpoint, _)| !is_catalog_space(endpoint))
        .map(|(endpoint, subject)| (endpoint, subject))
        .collect();
    // The subject is in the key because the types each Endpoint lists are the caller's own
    // (T-4567). ponytail: a catalogue outlives a Policy's time window until the next reload.
    let mut hasher = DefaultHasher::new();
    for (endpoint, subject) in &listed {
        endpoint.slug.hash(&mut hasher);
        subject.hash(&mut hasher);
    }
    let key = hasher.finish();
    let cached = cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned());
    if let Some(catalog) = cached.filter(|catalog| catalog.serves(&public, &listed)) {
        return catalog;
    }

    let base = gateway.base_url();

    let base: &str = &base;
    let mut records = Vec::with_capacity(listed.len());
    let mut places = BTreeMap::new();
    let mut facts = HashMap::new();
    let now = crate::pdp::now();
    for (endpoint, subject) in &listed {
        // The types list_endpoints names and a query answers, not the space's declared ones
        // (T-4567).
        let types = schema::visible_types(endpoint, &schema::visible(subject, endpoint, now));
        let classified = jc_core::classification::classify(types.iter().map(String::as_str));
        facts.insert(
            endpoint.slug.clone(),
            super::search::Facts {
                types,
                domains: classified.domains.iter().map(|d| (*d).to_owned()).collect(),
                themes: endpoint_surface::classified(endpoint)
                    .themes
                    .iter()
                    .map(|theme| theme.code().to_owned())
                    .collect(),
                words: classified.keywords.into_values().flatten().collect(),
                page: endpoint.page.clone(),
            },
        );
        let index = json!({ "endpoint": endpoint.slug, "models": [] });
        let space = gateway.resolver.resolve_space(&endpoint.space);
        records.push(endpoint_surface::dataset(
            endpoint,
            space.as_deref(),
            &index,
            base,
        ));
        places.insert(
            endpoint.slug.clone(),
            Place {
                space: endpoint.space.clone(),
                bbox: bbox(endpoint),
                access_level: endpoint.audience.as_str().to_owned(),
                visible_because: because(endpoint, subject).to_owned(),
            },
        );
    }
    let feed = endpoint_surface::feed(records, base, &gateway.org_domain);
    let nodes = feed
        .get("@included")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut built = ngsi_catalog::entities(nodes, &places, None);
    built.extend(
        public
            .iter()
            .filter_map(|entity| Some((entity["id"].as_str()?.to_owned(), entity.clone()))),
    );
    ngsi_catalog::link(&mut built);
    let entities = built
        .into_values()
        .map(|entity| {
            let expanded = entity
                .as_object()
                .and_then(|members| expand_entity(members, core(), ExpandOpts::default()).ok())
                .unwrap_or(Value::Null);
            (entity, expanded)
        })
        .collect::<Vec<(Value, Value)>>();
    let index = super::search::Index::build(entities.iter().map(|(entity, _)| entity), &facts);
    let catalog = Arc::new(Catalog {
        built_from: listed
            .iter()
            .map(|(endpoint, _)| Arc::clone(endpoint))
            .collect(),
        built_for: listed
            .iter()
            .map(|(_, subject)| (*subject).clone())
            .collect(),
        public,
        entities,
        index,
    });
    let mut cache = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= CACHED {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&catalog));
    drop(cache);
    catalog
}

/// The hub's `search` over the caller's catalogue (T-4493).
pub(crate) async fn search(
    gateway: &Gateway,
    reach: &[(Arc<Endpoint>, Subject)],
    id: Value,
    arguments: &Map<String, Value>,
) -> Value {
    tracing::info!(endpoints = reach.len(), "hub search");
    let catalog = of(gateway, reach).await;
    result(
        id,
        catalog
            .index
            .search(arguments)
            .unwrap_or_else(|why| refused(&why, &Value::Null)),
    )
}

/// One tool call on the catalogue.
pub(crate) async fn call(
    gateway: &Gateway,
    reach: &[(Arc<Endpoint>, Subject)],
    id: Value,
    name: &str,
    arguments: &Map<String, Value>,
) -> Value {
    if !TOOLS.contains(&name) {
        return error(
            id,
            -32602,
            &format!(
                "the catalog answers {}; call `{name}` on the Endpoint a Dataset lists in dataService",
                TOOLS.join(", ")
            ),
        );
    }
    tracing::info!(tool = %name, endpoints = reach.len(), "hub catalog call");
    let catalog = of(gateway, reach).await;
    let answer = match name {
        "list_types" => Ok(answered(
            &json!(["DataService", "Dataset", "Organization"]),
            "entityTypes",
        )),
        "get_entity" => get(&catalog, arguments),
        _ => query(&catalog, arguments),
    };
    result(id, answer.unwrap_or_else(|why| refused(&why, &Value::Null)))
}

fn get(catalog: &Catalog, arguments: &Map<String, Value>) -> Result<Value, String> {
    let wanted = arguments.get("id").and_then(Value::as_str).ok_or(
        "get_entity on the catalog takes the `id` of a Dataset, a DataService or an Organization",
    )?;
    let key_values = key_values_asked(arguments)?;
    let entity = catalog
        .entities
        .iter()
        .find(|(entity, _)| entity["id"] == wanted)
        .map(|(entity, _)| shaped(entity, key_values, None))
        .ok_or_else(|| format!("the catalog holds no entity `{wanted}`"))?;
    Ok(answered(&entity, "entity"))
}

fn query(catalog: &Catalog, arguments: &Map<String, Value>) -> Result<Value, String> {
    if let Some(other) = arguments
        .keys()
        .find(|key| !ARGUMENTS.contains(&key.as_str()))
    {
        return Err(format!(
            "the catalog does not take `{other}`; it takes {}",
            ARGUMENTS.join(", ")
        ));
    }
    let text = |key: &str| arguments.get(key).and_then(Value::as_str);
    // `type`, `attrs` and `pick` are a name or a list of names, as the tool schema publishes
    // them; a list is the comma-separated form (JC2-12).
    let names = |key: &str| -> Result<Option<String>, String> {
        let wrong = || format!("`{key}` is a name or a list of names");
        match arguments.get(key) {
            None => Ok(None),
            Some(Value::String(one)) => Ok(Some(one.clone())),
            Some(Value::Array(many)) => many
                .iter()
                .map(|name| name.as_str().ok_or_else(wrong))
                .collect::<Result<Vec<_>, _>>()
                .map(|many| Some(many.join(","))),
            Some(_) => Err(wrong()),
        }
    };
    let mut sub = json!({ "type": "Subscription" });
    if let Some(types) = names("type")? {
        let types = types.as_str();
        let types = if types.contains(['|', ',', ';', '(']) || types == "*" {
            Value::String(types.to_owned())
        } else {
            expand_types(&Value::String(types.to_owned()), core())
                .ok()
                .and_then(|iris| iris.into_iter().next())
                .ok_or_else(|| format!("`type` {types} is not a type name"))?
        };
        sub["entities"] = json!([{ "type": types }]);
    }
    if let Some(pattern) = text("idPattern") {
        antares_ql::regex::compile(pattern).map_err(|e| format!("idPattern: {e}"))?;
        let mut selectors = sub["entities"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![json!({})]);
        for selector in &mut selectors {
            selector["idPattern"] = json!(pattern);
        }
        sub["entities"] = Value::Array(selectors);
    }
    if let Some(q) = text("q") {
        let q = folded_search(&antares_ql::percent_decode(q.as_bytes()));
        antares_ql::parse_q(&q).map_err(|e| format!("q: {e}"))?;
        sub["q"] = json!(q);
    }
    if let Some(georel) = text("georel") {
        let mut geo = json!({ "georel": georel });
        for key in ["geometry", "coordinates", "geoproperty"] {
            if let Some(value) = arguments.get(key) {
                geo[key] = value.clone();
            }
        }
        let params = geo
            .as_object()
            .map(antares_matcher::geo_params)
            .unwrap_or_default();
        antares_ql::geo::GeoQuery::from_params(&params).map_err(|e| format!("georel: {e}"))?;
        sub["geoQ"] = geo;
    }
    let ids: Option<Vec<&str>> = match arguments.get("id") {
        None => None,
        Some(Value::String(ids)) => Some(ids.split(',').map(str::trim).collect()),
        Some(Value::Array(ids)) => Some(ids.iter().filter_map(Value::as_str).collect()),
        Some(_) => return Err("`id` is one id or a comma-separated list".into()),
    };
    let limit = match arguments.get("limit") {
        None => DEFAULT_LIMIT,
        Some(limit) => limit
            .as_u64()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| (1..=1000).contains(limit))
            .ok_or("`limit` is a number from 1 to 1000")?,
    };
    let offset = match arguments.get("cursor") {
        None => 0,
        Some(cursor) => cursor
            .as_str()
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .or_else(|| cursor.as_u64().and_then(|n| usize::try_from(n).ok()))
            .ok_or("`cursor` is the nextCursor of the previous page, passed back as given")?,
    };
    let key_values = key_values_asked(arguments)?;
    let attrs_text = match names("pick")? {
        Some(pick) => Some(pick),
        None => names("attrs")?,
    };
    let attrs: Option<Vec<&str>> = attrs_text
        .as_deref()
        .map(|names| names.split(',').map(str::trim).collect());

    let matching: Vec<&Value> = catalog
        .entities
        .iter()
        .filter(|(entity, _)| {
            ids.as_ref()
                .is_none_or(|ids| entity["id"].as_str().is_some_and(|id| ids.contains(&id)))
        })
        .filter(|(_, expanded)| antares_matcher::would_notify(&sub, expanded, core(), &|_| None))
        .map(|(entity, _)| entity)
        .collect();
    let total = matching.len();
    let page: Vec<Value> = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|entity| shaped(entity, key_values, attrs.as_deref()))
        .collect();
    let mut answer = answered(&Value::Array(page.clone()), "entities");
    // `answered` always carries `structuredContent` as an object.
    answer
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
        .into_iter()
        .for_each(|structured| {
            if offset + page.len() < total {
                structured.insert(
                    "nextCursor".into(),
                    json!((offset + page.len()).to_string()),
                );
            }
            if arguments.get("count").and_then(Value::as_bool) == Some(true) {
                structured.insert("total".into(), json!(total));
            }
        });
    Ok(answer)
}

/// `q` with each `searchText` pattern folded as `searchText` is, so `"Banská"` finds `banska`:
/// an agent types the place as it is written.
fn folded_search(q: &str) -> String {
    static TERM: OnceLock<regex::Regex> = OnceLock::new();
    let term = TERM.get_or_init(|| {
        regex::Regex::new(r#"(searchText!?~=)"([^"]*)""#).expect("a valid pattern")
    });
    term.replace_all(q, |found: &regex::Captures<'_>| {
        // A regex escape (`\D`, `\S`) keeps its case: lowered it would mean its opposite.
        let mut pattern = String::new();
        let mut escaped = false;
        for c in found[2].chars() {
            match escaped {
                true => pattern.push(c),
                false => pattern.push_str(&ngsi_catalog::fold(c.encode_utf8(&mut [0; 4]))),
            }
            escaped = !escaped && c == '\\';
        }
        format!("{}\"{pattern}\"", &found[1])
    })
    .into_owned()
}

/// Whether `format` asks for keyValues; normalized is the default, as on the broker.
fn key_values_asked(arguments: &Map<String, Value>) -> Result<bool, String> {
    match arguments.get("format").and_then(Value::as_str) {
        None | Some("normalized") => Ok(false),
        Some("keyValues" | "simplified") => Ok(true),
        Some(other) => Err(format!(
            "the catalog answers `format` normalized or keyValues, not {other}"
        )),
    }
}

/// The entity as asked: only the attributes named, and keyValues as the broker writes it.
fn shaped(entity: &Value, key_values: bool, attrs: Option<&[&str]>) -> Value {
    let Some(members) = entity.as_object() else {
        return entity.clone();
    };
    let mut out = Map::new();
    for (name, member) in members {
        let core_member = name == "id" || name == "type";
        if !core_member && attrs.is_some_and(|attrs| !attrs.contains(&name.as_str())) {
            continue;
        }
        let value = if key_values && !core_member {
            ["value", "object", "languageMap"]
                .iter()
                .find_map(|key| member.get(*key).cloned())
                .unwrap_or_else(|| member.clone())
        } else {
            member.clone()
        };
        out.insert(name.clone(), value);
    }
    Value::Object(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// T-5699, F-PLT2-021: the public catalogue is found under the rendered
    /// `{project}-catalog` space too, and a non-public one in that space is not it.
    #[test]
    fn public_catalog_takes_the_rendered_space_name() {
        let mut rendered = fixture();
        rendered.space = format!("{}-{NAME}", rendered.project);
        let rendered = Arc::new(rendered);
        let other = Arc::new(fixture());
        let reach = vec![
            (Arc::clone(&other), Subject::anonymous()),
            (Arc::clone(&rendered), Subject::anonymous()),
        ];
        let found = public_catalog(&reach).map(|endpoint| Arc::ptr_eq(endpoint, &rendered));
        assert_eq!(found, Some(true));
        let mut private = (*rendered).clone();
        private.audience = Audience::Organization;
        let reach = vec![(Arc::new(private), Subject::anonymous())];
        assert!(public_catalog(&reach).is_none());
    }

    /// T-5699, F-PLT2-014: an entry built for one subject never serves another reaching the
    /// same Endpoints, whatever the key hashed to.
    #[test]
    fn a_catalog_serves_only_the_subjects_it_was_read_as() {
        let endpoint = Arc::new(fixture());
        let public = Arc::new(Vec::new());
        let first = Subject::anonymous();
        let second = Subject {
            id: Some("someone".to_owned()),
            ..Subject::default()
        };
        let catalog = Catalog {
            built_from: vec![Arc::clone(&endpoint)],
            built_for: vec![first.clone()],
            public: Arc::clone(&public),
            entities: Vec::new(),
            index: super::super::search::Index::build(std::iter::empty(), &HashMap::new()),
        };
        assert!(catalog.serves(&public, &[(&endpoint, &first)]));
        assert!(!catalog.serves(&public, &[(&endpoint, &second)]));
    }

    /// An Endpoint with nothing declared, for the unit tests of this crate.
    pub(crate) fn fixture() -> Endpoint {
        Endpoint {
            declared_types: None,
            roles: Default::default(),
            slug: "aaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            title: BTreeMap::new(),
            description: BTreeMap::new(),
            space: "mesto-ovzdusie".to_owned(),
            tenant: crate::middleware::tenancy::Tenant::segment("mesto-ovzdusie").ok(),
            project: "mesto".to_owned(),
            audience: Audience::Public,
            allowed_projects: Vec::new(),
            representations: Vec::new(),
            rate_limit: None,
            creates: None,
            file_limits: None,
            hidden_attributes: Default::default(),
            projection: None,
            view_mapping: None,
            catalog: None,
            deprecation: None,
            feeds: Vec::new(),
            calendars: Vec::new(),
            map_defaults: Default::default(),
            page: None,
            open_data: false,
            base_path: String::new(),
            models: Vec::new(),
            policy_names: Vec::new(),
            manifest_name: String::new(),
            policies: Vec::new(),
        }
    }

    #[test]
    fn a_workload_sees_an_internal_endpoint_by_its_grant_and_a_person_by_the_audience() {
        let mut endpoint = fixture();
        let person = Subject {
            user: Some("jana".to_owned()),
            ..Subject::anonymous()
        };
        let workload = Subject {
            service_account: Some("conformance".to_owned()),
            ..Subject::anonymous()
        };
        endpoint.audience = Audience::Public;
        assert_eq!(because(&endpoint, &workload), "public");
        endpoint.audience = Audience::Organization;
        assert_eq!(because(&endpoint, &person), "organization");
        assert_eq!(because(&endpoint, &workload), "grant");
        endpoint.audience = Audience::ProjectList;
        assert_eq!(because(&endpoint, &person), "project");
    }

    /// JC2-12: `type` and `pick` as the lists the tool schema publishes narrow the answer, as
    /// their comma-separated forms do.
    #[test]
    fn a_type_and_pick_list_narrow_the_catalog_answer() {
        let entities = [
            json!({ "id": "urn:ngsi-ld:Dataset:a", "type": "Dataset", "title": "Air", "description": "d" }),
            json!({ "id": "urn:ngsi-ld:Organization:b", "type": "Organization", "name": "City" }),
        ]
        .into_iter()
        .map(|entity| {
            let expanded = entity
                .as_object()
                .and_then(|members| expand_entity(members, core(), ExpandOpts::default()).ok())
                .unwrap_or(Value::Null);
            (entity, expanded)
        })
        .collect();
        let catalog = Catalog {
            built_from: Vec::new(),
            built_for: Vec::new(),
            public: Arc::new(Vec::new()),
            entities,
            index: super::super::search::Index::build(std::iter::empty(), &HashMap::new()),
        };
        let arguments = json!({ "type": ["Dataset"], "pick": ["id", "title"] });
        let answer = query(&catalog, arguments.as_object().expect("object")).expect("answer");
        let found = &answer["structuredContent"]["entities"];
        assert_eq!(found.as_array().map(Vec::len), Some(1), "{answer}");
        assert_eq!(found[0]["id"], "urn:ngsi-ld:Dataset:a");
        assert!(found[0].get("description").is_none(), "{answer}");
        let wrong = json!({ "type": [1] });
        assert!(query(&catalog, wrong.as_object().expect("object")).is_err());
    }

    fn catalog_of(entities: Vec<Value>) -> Catalog {
        let entities = entities
            .into_iter()
            .map(|entity| {
                let expanded = entity
                    .as_object()
                    .and_then(|members| expand_entity(members, core(), ExpandOpts::default()).ok())
                    .unwrap_or(Value::Null);
                (entity, expanded)
            })
            .collect();
        Catalog {
            built_from: Vec::new(),
            built_for: Vec::new(),
            public: Arc::new(Vec::new()),
            entities,
            index: super::super::search::Index::build(std::iter::empty(), &HashMap::new()),
        }
    }

    fn asked(catalog: &Catalog, arguments: Value) -> Result<Value, String> {
        query(catalog, arguments.as_object().expect("object"))
    }

    fn ids_of(answer: &Value) -> Vec<&str> {
        answer["structuredContent"]["entities"]
            .as_array()
            .map(|found| found.iter().filter_map(|e| e["id"].as_str()).collect())
            .unwrap_or_default()
    }

    /// `get_entity` on the catalogue: by id, keyValues when asked, and an error naming what is
    /// wrong for a missing id, an unknown one and a format the catalogue does not answer.
    #[test]
    fn get_on_the_catalog_answers_one_entity_or_says_why_not() {
        let catalog = catalog_of(vec![
            json!({ "id": "urn:ngsi-ld:Dataset:a", "type": "Dataset",
                "title": { "type": "Property", "value": "Air" } }),
        ]);
        let got = |arguments: Value| get(&catalog, arguments.as_object().expect("object"));
        let normalized = got(json!({ "id": "urn:ngsi-ld:Dataset:a" })).expect("the entity");
        assert_eq!(
            normalized["structuredContent"]["entity"]["title"]["value"],
            "Air"
        );
        let plain = got(json!({ "id": "urn:ngsi-ld:Dataset:a", "format": "keyValues" }))
            .expect("the entity");
        assert_eq!(plain["structuredContent"]["entity"]["title"], "Air");
        assert!(got(json!({})).is_err_and(|e| e.contains("takes the `id`")));
        assert!(got(json!({ "id": "urn:ngsi-ld:Dataset:zz" }))
            .is_err_and(|e| e.contains("holds no entity `urn:ngsi-ld:Dataset:zz`")));
        assert!(
            got(json!({ "id": "urn:ngsi-ld:Dataset:a", "format": "geojson" }))
                .is_err_and(|e| e.contains("not geojson"))
        );
    }

    /// The catalogue's query arguments, each edge: an unknown argument, a type that is no name,
    /// a type expression and `*`, an idPattern alone, with a type and malformed, an id as a
    /// string, a list and neither, and the limit and cursor bounds with nextCursor and total.
    #[test]
    fn the_catalog_query_checks_each_argument_and_pages() {
        let catalog = catalog_of(
            (0..3)
                .map(|n| json!({ "id": format!("urn:ngsi-ld:Dataset:d{n}"), "type": "Dataset" }))
                .chain([json!({ "id": "urn:ngsi-ld:Organization:o", "type": "Organization" })])
                .collect(),
        );
        assert!(asked(&catalog, json!({ "local": true }))
            .is_err_and(|e| e.contains("does not take `local`")));
        assert!(asked(&catalog, json!({ "type": 5 }))
            .is_err_and(|e| e.contains("`type` is a name or a list of names")));
        assert!(asked(&catalog, json!({ "type": "not a type" }))
            .is_err_and(|e| e.contains("is not a type name")));
        let either = asked(&catalog, json!({ "type": "Dataset,Organization" })).expect("answer");
        assert_eq!(ids_of(&either).len(), 4, "{either}");
        let all = asked(&catalog, json!({ "type": "*" })).expect("answer");
        assert_eq!(ids_of(&all).len(), 4, "{all}");

        let patterned = asked(&catalog, json!({ "idPattern": ".*:d[12]$" })).expect("answer");
        assert_eq!(
            ids_of(&patterned),
            ["urn:ngsi-ld:Dataset:d1", "urn:ngsi-ld:Dataset:d2"]
        );
        let typed = asked(
            &catalog,
            json!({ "type": "Organization", "idPattern": ".*:d1$" }),
        )
        .expect("answer");
        assert!(ids_of(&typed).is_empty(), "{typed}");
        assert!(asked(&catalog, json!({ "idPattern": "(" }))
            .is_err_and(|e| e.starts_with("idPattern: ")));

        let listed = asked(
            &catalog,
            json!({ "id": "urn:ngsi-ld:Dataset:d0, urn:ngsi-ld:Organization:o" }),
        )
        .expect("answer");
        assert_eq!(ids_of(&listed).len(), 2, "{listed}");
        let array =
            asked(&catalog, json!({ "id": ["urn:ngsi-ld:Dataset:d2", 7] })).expect("answer");
        assert_eq!(ids_of(&array), ["urn:ngsi-ld:Dataset:d2"]);
        assert!(asked(&catalog, json!({ "id": 7 }))
            .is_err_and(|e| e.contains("one id or a comma-separated list")));

        for limit in [json!(0), json!(1001), json!("2"), json!(-1)] {
            assert!(
                asked(&catalog, json!({ "limit": limit })).is_err_and(|e| e.contains("1 to 1000")),
                "{limit}"
            );
        }
        let first = asked(&catalog, json!({ "limit": 2, "count": true })).expect("answer");
        assert_eq!(first["structuredContent"]["nextCursor"], "2", "{first}");
        assert_eq!(first["structuredContent"]["total"], 4, "{first}");
        for cursor in [json!("2"), json!(2)] {
            let next = asked(&catalog, json!({ "limit": 2, "cursor": cursor })).expect("answer");
            assert_eq!(ids_of(&next).len(), 2, "{next}");
            assert!(
                next["structuredContent"].get("nextCursor").is_none(),
                "{next}"
            );
            assert!(next["structuredContent"].get("total").is_none(), "{next}");
        }
        for cursor in [json!("x"), json!(-3), json!(true)] {
            assert!(
                asked(&catalog, json!({ "cursor": cursor }))
                    .is_err_and(|e| e.contains("nextCursor")),
                "{cursor}"
            );
        }
    }

    #[test]
    fn a_search_pattern_is_folded_as_search_text_is_but_its_escapes_are_not() {
        assert_eq!(
            folded_search(r#"searchText~="Banská \D+";name=="Žilina""#),
            r#"searchText~="banska \D+";name=="Žilina""#
        );
        assert_eq!(folded_search("pm10>40"), "pm10>40");
    }

    /// A catalogue read that the broker never answers ends at one member call's deadline, and
    /// the caller is answered the list it had (none here) instead of waiting on the broker.
    #[tokio::test(start_paused = true)]
    async fn a_public_catalogue_read_ends_at_the_member_deadline() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = silent.local_addr().expect("addr");
        // Holds every connection open for a minute of the paused clock, then lets go.
        let holder = tokio::spawn(async move {
            let mut held = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(60), async {
                while let Ok((socket, _)) = silent.accept().await {
                    held.push(socket);
                }
            })
            .await;
        });
        let gateway = Gateway::new(
            crate::proxy::Broker::new(format!("http://{address}")),
            Box::new(crate::pdp::PolicyPdp),
            "example.org",
        );
        let mut endpoint = fixture();
        endpoint.slug = "silentcatalogueaaaaaaaaaaa".to_owned();
        endpoint.space = NAME.to_owned();
        endpoint.audience = Audience::Public;
        let reach = vec![(Arc::new(endpoint), Subject::anonymous())];
        let asked = tokio::time::Instant::now();
        let read = public_entities(&gateway, &reach).await;
        assert!(read.is_empty());
        assert!(
            asked.elapsed() <= super::super::server::MEMBER_DEADLINE + Duration::from_secs(1),
            "{:?}",
            asked.elapsed()
        );
        holder.await.expect("the holder ends with its minute");
    }

    /// A catalogue whose pages each answer within the member deadline is read whole, however
    /// long all its pages take together.
    #[tokio::test(start_paused = true)]
    async fn a_slow_public_catalogue_is_read_page_by_page() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let pages = Arc::new(AtomicUsize::new(0));
        let served = Arc::clone(&pages);
        let broker = axum::Router::new().fallback(move || {
            let served = Arc::clone(&served);
            async move {
                tokio::time::sleep(Duration::from_secs(6)).await;
                let n = served.fetch_add(1, Ordering::SeqCst);
                let mut content = json!({
                    "entities": [{ "id": format!("urn:ngsi-ld:Dataset:{n}"), "type": "Dataset" }],
                });
                if n < 2 {
                    content["nextCursor"] = json!(format!("c{n}"));
                }
                axum::Json(json!({ "jsonrpc": "2.0", "id": 1, "result": { "structuredContent": content } }))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, broker).await });
        let gateway = Gateway::new(
            crate::proxy::Broker::new(format!("http://{address}")),
            Box::new(crate::pdp::PolicyPdp),
            "example.org",
        );
        let mut endpoint = fixture();
        endpoint.slug = "slowcatalogueaaaaaaaaaaaaa".to_owned();
        endpoint.space = NAME.to_owned();
        endpoint.audience = Audience::Public;
        let reach = vec![(Arc::new(endpoint), Subject::anonymous())];
        let read = public_entities(&gateway, &reach).await;
        assert_eq!(read.len(), 3, "{read:?}");
        assert_eq!(pages.load(Ordering::SeqCst), 3);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What the broker of [`scripted`] does with each read: a page of one Dataset, an error
    /// message, a refused tool call, a page that always names a next one.
    const PAGE: usize = 0;
    const ERROR: usize = 1;
    const REFUSED: usize = 2;
    const ENDLESS: usize = 3;

    /// A broker whose answer to every catalogue read follows `mode` and waits `delay` seconds.
    async fn scripted(mode: Arc<AtomicUsize>, delay: Arc<AtomicUsize>) -> Gateway {
        let broker = axum::Router::new().fallback(move || {
            let (mode, delay) = (Arc::clone(&mode), Arc::clone(&delay));
            async move {
                let wait = delay.load(Ordering::SeqCst) as u64;
                tokio::time::sleep(Duration::from_secs(wait)).await;
                match mode.load(Ordering::SeqCst) {
                    ERROR => axum::Json(json!({ "jsonrpc": "2.0", "id": 1,
                        "error": { "message": "the broker said no" } })),
                    REFUSED => axum::Json(json!({ "jsonrpc": "2.0", "id": 1,
                        "result": { "isError": true } })),
                    kind => {
                        let mut content = json!({ "entities": [
                            { "id": "urn:ngsi-ld:Dataset:one", "type": "Dataset", "@context": "x" },
                            { "id": "urn:ngsi-ld:Vehicle:skipped", "type": "Vehicle" },
                        ] });
                        if kind == ENDLESS {
                            content["nextCursor"] = json!("more");
                        }
                        axum::Json(json!({ "jsonrpc": "2.0", "id": 1,
                            "result": { "structuredContent": content } }))
                    }
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, broker).await });
        Gateway::new(
            crate::proxy::Broker::new(format!("http://{address}")),
            Box::new(crate::pdp::PolicyPdp),
            "example.org",
        )
    }

    fn public_reach(slug: &str) -> Vec<(Arc<Endpoint>, Subject)> {
        let mut endpoint = fixture();
        endpoint.slug = slug.to_owned();
        endpoint.space = NAME.to_owned();
        endpoint.audience = Audience::Public;
        vec![(Arc::new(endpoint), Subject::anonymous())]
    }

    /// A page the broker refuses, errors on or never ends is an error naming why, and a read
    /// that holds a Dataset keeps it and drops what is no Dataset, with its `@context`.
    #[tokio::test]
    async fn a_public_read_names_why_it_failed_and_stops_at_its_page_limit() {
        let (mode, delay) = (
            Arc::new(AtomicUsize::new(PAGE)),
            Arc::new(AtomicUsize::new(0)),
        );
        let gateway = scripted(Arc::clone(&mode), delay).await;
        let reach = public_reach("namedfailurecatalogueaaaaaa");
        let (endpoint, anonymous) = (&reach[0].0, &reach[0].1);
        let read = read_public(&gateway, endpoint, anonymous)
            .await
            .expect("a page");
        assert_eq!(read.len(), 1, "{read:?}");
        assert!(read[0].get("@context").is_none(), "{read:?}");
        mode.store(ERROR, Ordering::SeqCst);
        let why = read_public(&gateway, endpoint, anonymous)
            .await
            .expect_err("an error");
        assert_eq!(why, "the broker said no");
        mode.store(REFUSED, Ordering::SeqCst);
        let why = read_public(&gateway, endpoint, anonymous)
            .await
            .expect_err("refused");
        assert_eq!(why, "the catalog Endpoint refused the read");
        mode.store(ENDLESS, Ordering::SeqCst);
        let why = read_public(&gateway, endpoint, anonymous)
            .await
            .expect_err("endless");
        assert!(
            why.contains(&format!("more than {}", PUBLIC_PAGE * PUBLIC_PAGES)),
            "{why}"
        );
    }

    #[test]
    fn a_public_page_reads_each_shape_the_tool_answers() {
        let refused = json!({ "result": { "isError": true,
            "content": [{ "type": "text", "text": "no grant" }] } });
        assert_eq!(public_page(&refused).expect_err("refused"), "no grant");
        let none = json!({ "result": { "structuredContent": {} } });
        assert_eq!(
            public_page(&none).expect_err("no entities"),
            "the catalog Endpoint answered no entities"
        );
        assert!(public_page(&json!({})).is_err());
    }

    /// A read that fails after one that worked answers the list it had, and a caller arriving
    /// while another reads is answered that list at once rather than waiting for the read.
    #[tokio::test(start_paused = true)]
    async fn a_failed_or_running_public_read_answers_the_list_it_had() {
        let (mode, delay) = (
            Arc::new(AtomicUsize::new(PAGE)),
            Arc::new(AtomicUsize::new(0)),
        );
        let gateway = scripted(Arc::clone(&mode), Arc::clone(&delay)).await;
        let reach = public_reach("stalecatalogueaaaaaaaaaaaaaa");
        assert_eq!(public_entities(&gateway, &reach).await.len(), 1);
        tokio::time::advance(PUBLIC_FOR + Duration::from_secs(1)).await;
        mode.store(ERROR, Ordering::SeqCst);
        let kept = public_entities(&gateway, &reach).await;
        assert_eq!(kept.len(), 1, "the failed read keeps the last list");
        tokio::time::advance(PUBLIC_FOR + Duration::from_secs(1)).await;
        mode.store(PAGE, Ordering::SeqCst);
        delay.store(5, Ordering::SeqCst);
        let (first, second) = tokio::join!(
            public_entities(&gateway, &reach),
            public_entities(&gateway, &reach)
        );
        assert_eq!((first.len(), second.len()), (1, 1));
    }

    /// A catalogue read that cannot be had and was never had is the empty list.
    #[tokio::test(start_paused = true)]
    async fn a_public_read_that_never_worked_is_the_empty_list() {
        let (mode, delay) = (
            Arc::new(AtomicUsize::new(ERROR)),
            Arc::new(AtomicUsize::new(0)),
        );
        let gateway = scripted(mode, delay).await;
        let reach = public_reach("neverreadcatalogueaaaaaaaaaa");
        assert!(public_entities(&gateway, &reach).await.is_empty());
    }

    /// Past `CACHED` distinct catalogues the cache starts again, never grows without bound.
    #[tokio::test]
    async fn the_catalog_cache_starts_again_past_its_limit() {
        let gateway = Gateway::new(
            crate::proxy::Broker::new("http://127.0.0.1:9"),
            Box::new(crate::pdp::PolicyPdp),
            "example.org",
        );
        for n in 0..=CACHED + 1 {
            let mut endpoint = fixture();
            endpoint.slug = format!("cachedcatalogue{n:011}");
            let reach = vec![(Arc::new(endpoint), Subject::anonymous())];
            let built = of(&gateway, &reach).await;
            assert_eq!(built.built_from.len(), 1);
        }
        let held = cache().lock().expect("the cache").len();
        assert!(held <= CACHED, "{held}");
    }

    /// An invalid `q` or geo-query is refused naming the argument, and an entity that is no
    /// object is returned as it is.
    #[test]
    fn a_bad_q_or_geo_query_names_its_argument_and_a_non_object_is_kept() {
        let catalog = catalog_of(vec![
            json!({ "id": "urn:ngsi-ld:Dataset:a", "type": "Dataset" }),
        ]);
        assert!(asked(&catalog, json!({ "q": "((" })).is_err_and(|e| e.starts_with("q: ")));
        assert!(asked(&catalog, json!({ "georel": "nonsense" }))
            .is_err_and(|e| e.starts_with("georel: ")));
        assert_eq!(shaped(&json!("plain"), true, None), json!("plain"));
    }

    #[test]
    fn the_public_catalogue_asks_its_types_as_a_list() {
        let first = public_arguments(None);
        assert_eq!(first["type"], json!(["Dataset", "Organization"]));
        assert!(first.get("cursor").is_none());
        assert_eq!(public_arguments(Some(json!("c1")))["cursor"], json!("c1"));
    }

    #[test]
    fn the_public_catalogue_is_not_a_dataset_of_itself() {
        let mut endpoint = fixture();
        endpoint.project = "mesto".to_owned();
        endpoint.space = "mesto-catalog".to_owned();
        assert!(is_catalog_space(&endpoint));
        endpoint.space = "mesto-catalogue".to_owned();
        assert!(!is_catalog_space(&endpoint));
    }
}
