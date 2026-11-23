#![forbid(unsafe_code)]
//! JSON-LD parser (backed by `oxjsonld`) and serializer supporting:
//! - Standard DCAT-AP JSON-LD, the graph as it is
//! - Compact JSON-LD under the key dictionary of a profile's manifest (the Czech LKOD and the
//!   Slovak catalogue interface are two), read back with the profile's own context file
//!
//! Security (A1):
//! - Remote `@context` URLs not in the built-in offline context map are refused without any network fetch.
//! - Billion-laughs / recursive `@context` term expansion or excessive context size is refused.
//! - Byte, triple, nesting-depth and literal-length caps are enforced while parsing.

use crate::dcat::{dataset_publisher, terms_of_use};
use crate::graph::{
    base_iri_error, iri_text, ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple,
    BASE_IRI,
};
use crate::profile::{
    CatalogueMember, Compact, Contact, Context, DatasetMember, DcatProfile, DistributionMember,
    Profile, TermsMember,
};
use dataroom_core::{
    Catalogue, ContactKind, ContactPoint, DataroomError, Dataset, Distribution, Publisher, Result,
};
use oxjsonld::JsonLdParser;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Maximum number of terms allowed in a JSON-LD `@context` to prevent billion-laughs context bombs (A1).
const MAX_CONTEXT_TERMS: usize = 256;

/// Resolves a known built-in `@context` URL to its local inline `@context` Value, or refuses any
/// unknown remote `@context` URL without fetching (A1).
fn inline_and_check_jsonld(val: &mut Value, limits: &RdfLimits, depth: usize) -> Result<()> {
    if depth > limits.max_depth {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "nesting-depth".into(),
            message: format!(
                "JSON-LD nesting depth {depth} exceeds maximum of {}",
                limits.max_depth
            ),
        });
    }
    match val {
        Value::Array(items) => {
            for item in items {
                inline_and_check_jsonld(item, limits, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(obj) => {
            if let Some(ctx_val) = obj.get_mut("@context") {
                resolve_and_validate_context(ctx_val, 0)?;
            }
            // In SK JSON-LD examples (`dataset.jsonld`), `kontaktný_bod.jméno` is used alongside `meno`
            // and `autor_databáze` alongside `autor_databázy`, and `začátek` alongside `začiatok`.
            // Also normalize `@included` to `@graph` for JSON-LD 1.0 processors.
            if let Some(included) = obj.remove("@included") {
                obj.entry("@graph".to_owned()).or_insert(included);
            }
            for (_, v) in obj.iter_mut() {
                inline_and_check_jsonld(v, limits, depth + 1)?;
            }
            Ok(())
        }
        Value::String(s) => {
            if s.len() > limits.max_literal_length {
                return Err(DataroomError::RdfLimitExceeded {
                    kind: "literal-length".into(),
                    message: format!(
                        "JSON-LD string length {} exceeds maximum of {} bytes",
                        s.len(),
                        limits.max_literal_length
                    ),
                });
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The installed profile that carries the context at `iri`, so it is read and never fetched.
fn offline_context(iri: &str) -> Option<(&'static Profile, &'static Context)> {
    DcatProfile::installed().find_map(|installed| {
        let profile = installed.manifest();
        let context = profile.contexts.iter().find(|c| {
            c.prefixes.iter().any(|p| iri.starts_with(p.as_str()))
                || c.suffixes.iter().any(|s| iri.ends_with(s.as_str()))
        })?;
        Some((profile, context))
    })
}

fn resolve_and_validate_context(ctx_val: &mut Value, depth: usize) -> Result<()> {
    if depth > 8 {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "context-bomb".into(),
            message: "JSON-LD @context nesting depth exceeded (possible billion-laughs context)"
                .into(),
        });
    }
    match ctx_val {
        Value::String(url) => {
            let trimmed = url.trim();
            if let Some((profile, context)) = offline_context(trimmed) {
                let mut parsed: Value =
                    serde_json::from_str(profile.file(&context.file)).map_err(|e| {
                        DataroomError::Rdf(format!("built-in {} context: {e}", profile.id))
                    })?;
                if let Some(inner) = parsed.get_mut("@context") {
                    if let Some(map) = inner.as_object_mut() {
                        for (term, meaning) in &context.add {
                            map.insert(term.clone(), meaning.clone());
                        }
                    }
                    *ctx_val = inner.take();
                }
                Ok(())
            } else if trimmed == "https://schema.org/" || trimmed == "http://schema.org/" {
                *ctx_val = json!({ "@vocab": "http://schema.org/" });
                Ok(())
            } else {
                Err(DataroomError::RdfLimitExceeded {
                    kind: "remote-context".into(),
                    message: format!(
                        "remote JSON-LD @context URL '{trimmed}' is refused (no network fetch allowed)"
                    ),
                })
            }
        }
        Value::Array(items) => {
            if items.len() > 16 {
                return Err(DataroomError::RdfLimitExceeded {
                    kind: "context-bomb".into(),
                    message: "JSON-LD @context array has too many entries".into(),
                });
            }
            for item in items {
                resolve_and_validate_context(item, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if map.len() > MAX_CONTEXT_TERMS {
                return Err(DataroomError::RdfLimitExceeded {
                    kind: "context-bomb".into(),
                    message: format!(
                        "JSON-LD @context defines {} terms, exceeding maximum of {MAX_CONTEXT_TERMS}",
                        map.len()
                    ),
                });
            }
            // Detect billion-laughs style prefix chains where term A expands to B:..., B expands to C:..., etc.
            let mut prefix_targets: BTreeMap<&str, &str> = BTreeMap::new();
            for (k, v) in map.iter() {
                let target_id = match v {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(o) => o.get("@id").and_then(Value::as_str),
                    _ => None,
                };
                if let Some(tid) = target_id {
                    if let Some((prefix, _)) = tid.split_once(':') {
                        if prefix != "http" && prefix != "https" && prefix != "urn" {
                            prefix_targets.insert(k.as_str(), prefix);
                        }
                    }
                }
                if let Value::Object(o) = v {
                    if let Some(nested_ctx) = o.get("@context") {
                        let mut cloned = nested_ctx.clone();
                        resolve_and_validate_context(&mut cloned, depth + 1)?;
                    }
                }
            }
            // Check chain length in `prefix_targets`: any chain > 3 hops or cycle is a context bomb!
            for start in prefix_targets.keys() {
                let mut cur = *start;
                for hop in 0..=4 {
                    if hop == 4 {
                        return Err(DataroomError::RdfLimitExceeded {
                            kind: "context-bomb".into(),
                            message: "JSON-LD @context recursive/chained prefix expansion refused (billion-laughs guard)".into(),
                        });
                    }
                    match prefix_targets.get(cur) {
                        Some(next) => cur = *next,
                        None => break,
                    }
                }
            }
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Err(DataroomError::Rdf("invalid JSON-LD @context value".into())),
    }
}

/// Terms the document's contexts define as aliases of `@type` and `@id` (e.g. CZ `typ`, `iri`).
#[derive(Default)]
struct KeywordAliases {
    type_terms: std::collections::BTreeSet<String>,
    id_terms: std::collections::BTreeSet<String>,
}

impl KeywordAliases {
    fn collect(&mut self, val: &Value) {
        match val {
            Value::Array(items) => items.iter().for_each(|item| self.collect(item)),
            Value::Object(obj) => {
                if let Some(ctx) = obj.get("@context") {
                    self.collect_context(ctx);
                }
                obj.values().for_each(|v| self.collect(v));
            }
            _ => {}
        }
    }

    fn collect_context(&mut self, ctx: &Value) {
        match ctx {
            Value::Array(items) => items.iter().for_each(|item| self.collect_context(item)),
            Value::Object(defs) => {
                for (term, def) in defs {
                    let target = match def {
                        Value::String(s) => Some(s.as_str()),
                        Value::Object(o) => o.get("@id").and_then(Value::as_str),
                        _ => None,
                    };
                    match target {
                        Some("@type") => {
                            self.type_terms.insert(term.clone());
                        }
                        Some("@id") => {
                            self.id_terms.insert(term.clone());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn rank(&self, key: &str) -> u8 {
        if key == "@context" {
            0
        } else if key == "@type" || self.type_terms.contains(key) {
            1
        } else if key == "@id" || self.id_terms.contains(key) {
            2
        } else {
            3
        }
    }
}

/// Writes `val` as JSON with each object's keys ordered by [`KeywordAliases::rank`].
fn write_ordered_json(val: &Value, aliases: &KeywordAliases, out: &mut String) -> Result<()> {
    match val {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_ordered_json(item, aliases, out)?;
            }
            out.push(']');
        }
        Value::Object(obj) => {
            let mut entries: Vec<(&String, &Value)> = obj.iter().collect();
            entries.sort_by_key(|(k, _)| aliases.rank(k));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_text(k)?);
                out.push(':');
                write_ordered_json(v, aliases, out)?;
            }
            out.push('}');
        }
        scalar => out.push_str(&json_text(scalar)?),
    }
    Ok(())
}

/// A JSON scalar or key written by `serde_json` (string escaping, number format).
fn json_text<T: serde::Serialize + ?Sized>(v: &T) -> Result<String> {
    serde_json::to_string(v)
        .map_err(|e| DataroomError::Rdf(format!("serialize normalized JSON-LD: {e}")))
}

/// Parses a JSON-LD byte slice using `oxjsonld::JsonLdParser`, enforcing [`RdfLimits`] (A1).
pub fn parse_jsonld_bytes(bytes: &[u8], limits: &RdfLimits) -> Result<RdfGraph> {
    if bytes.len() > limits.max_bytes {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "bytes".into(),
            message: format!(
                "JSON-LD input size {} exceeds limit of {} bytes",
                bytes.len(),
                limits.max_bytes
            ),
        });
    }
    let mut val: Value = serde_json::from_slice(bytes)
        .map_err(|e| DataroomError::Rdf(format!("invalid JSON-LD: {e}")))?;

    inline_and_check_jsonld(&mut val, limits, 0)?;

    // `serde_json::Map` sorts keys, but `oxjsonld` reads objects in one pass and wants
    // `@context`, then `@type`, then `@id` (or their aliases) ahead of every other key.
    let mut aliases = KeywordAliases::default();
    aliases.collect(&val);
    let mut normalized = String::with_capacity(bytes.len());
    write_ordered_json(&val, &aliases, &mut normalized)?;
    let normalized_bytes = normalized.into_bytes();

    let parser = JsonLdParser::new()
        .with_base_iri(BASE_IRI)
        .map_err(base_iri_error)?;

    let mut graph = RdfGraph::new();
    for item in parser.for_slice(&normalized_bytes) {
        let quad = item.map_err(|e| DataroomError::Rdf(format!("JSON-LD parse error: {e}")))?;
        let subject = match quad.subject {
            oxrdf::Subject::NamedNode(n) => SubjectNode::Named(n.into_string()),
            oxrdf::Subject::BlankNode(b) => SubjectNode::Blank(b.into_string()),
        };
        let predicate = quad.predicate.into_string();
        let object = match quad.object {
            oxrdf::Term::NamedNode(n) => Term::Iri(n.into_string()),
            oxrdf::Term::BlankNode(b) => Term::Blank(b.into_string()),
            oxrdf::Term::Literal(lit) => {
                let (value, datatype, language) = lit.destruct();
                if let Some(lang) = language {
                    Term::Literal(LiteralValue::lang(value, lang))
                } else if let Some(dt) = datatype {
                    Term::Literal(LiteralValue::typed(value, dt.into_string()))
                } else {
                    Term::Literal(LiteralValue::plain(value))
                }
            }
        };
        graph.insert_checked(
            Triple {
                subject,
                predicate,
                object,
            },
            limits,
        )?;
    }
    Ok(graph)
}

/// Parses a JSON-LD string into an [`RdfGraph`].
pub fn parse_jsonld(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    parse_jsonld_bytes(input.as_bytes(), limits)
}

/// Serializes an [`RdfGraph`] into standard expanded/compacted JSON-LD (`@graph`).
pub fn serialize_graph_jsonld(graph: &RdfGraph) -> String {
    let mut by_subj: BTreeMap<&SubjectNode, BTreeMap<&str, Vec<&Term>>> = BTreeMap::new();
    for t in &graph.triples {
        by_subj
            .entry(&t.subject)
            .or_default()
            .entry(t.predicate.as_str())
            .or_default()
            .push(&t.object);
    }

    let mut nodes = Vec::new();
    let mut field_prefixes: BTreeMap<String, String> = BTreeMap::new();
    for (subj, preds) in by_subj {
        let mut obj = Map::new();
        match subj {
            SubjectNode::Named(iri) => {
                obj.insert("@id".into(), json!(iri_text(iri)));
            }
            SubjectNode::Blank(id) => {
                obj.insert("@id".into(), json!(format!("_:{id}")));
            }
        }
        for (pred, terms) in preds {
            if pred == ns::RDF_TYPE {
                let types: Vec<Value> = terms
                    .iter()
                    .map(|t| json!(iri_text(t.string_value())))
                    .collect();
                obj.insert(
                    "@type".into(),
                    if types.len() == 1 {
                        types[0].clone()
                    } else {
                        Value::Array(types)
                    },
                );
                continue;
            }
            let values: Vec<Value> = terms
                .iter()
                .map(|t| match t {
                    Term::Iri(iri) => json!({ "@id": iri_text(iri) }),
                    Term::Blank(id) => json!({ "@id": format!("_:{id}") }),
                    Term::Literal(lit) => {
                        if let Some(lang) = &lit.language {
                            json!({ "@value": lit.value, "@language": lang })
                        } else if lit.datatype == ns::XSD_STRING {
                            json!(lit.value)
                        } else if let (true, Ok(n)) =
                            (lit.datatype == ns::XSD_INTEGER, lit.value.parse::<i64>())
                        {
                            // a number, as prod's ckanext-dcat writes hydra:totalItems
                            json!({ "@value": n, "@type": iri_text(&lit.datatype) })
                        } else {
                            json!({ "@value": lit.value, "@type": iri_text(&lit.datatype) })
                        }
                    }
                })
                .collect();
            let key = match field_term(pred) {
                Some((prefix, namespace, name)) => {
                    field_prefixes.insert(prefix.clone(), namespace);
                    format!("{prefix}:{name}")
                }
                None => iri_text(pred).into_owned(),
            };
            obj.insert(
                key,
                if values.len() == 1 {
                    values[0].clone()
                } else {
                    Value::Array(values)
                },
            );
        }
        nodes.push(Value::Object(obj));
    }

    let mut doc = json!({
        "@context": {
            "dcat": ns::DCAT,
            "dct": ns::DCT,
            "dcatap": ns::DCATAP,
            "foaf": ns::FOAF,
            "vcard": ns::VCARD,
            "skos": ns::SKOS,
            "spdx": ns::SPDX,
            "adms": ns::ADMS,
            "hydra": ns::HYDRA,
            "xsd": ns::XSD
        },
        "@graph": nodes,
        "@included": nodes
    });
    // T-7746: each Organization's dataset-field namespace, declared beside the vocabularies.
    for (prefix, namespace) in field_prefixes {
        doc["@context"][prefix] = json!(namespace);
    }
    format!("{:#}", doc)
}

/// An Organization's dataset-field property `https://<host>/ns/dataset-fields/<org>/<name>` as
/// its JSON-LD prefix `fields-<org>`, the namespace and the name (T-7746); `None` for any other
/// IRI or one whose segments are not DNS labels.
fn field_term(iri: &str) -> Option<(String, String, &str)> {
    let rest = iri.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let (org, name) = path.strip_prefix("ns/dataset-fields/")?.split_once('/')?;
    let label = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    (label(org) && label(name) && !host.is_empty())
        .then(|| (format!("fields-{org}"), format!("https://{host}/ns/dataset-fields/{org}/"), name))
}

fn compact_contact(keys: &Contact, kind: &str, contact: &ContactPoint) -> Value {
    let email = if contact.email.starts_with("mailto:") {
        contact.email.clone()
    } else {
        format!("mailto:{}", contact.email)
    };
    let mut node = Map::new();
    let class = match contact.kind {
        ContactKind::Organization => &keys.organization,
        ContactKind::Individual => &keys.individual,
    };
    node.insert(kind.to_owned(), json!(class));
    for key in &keys.name {
        node.insert(key.clone(), json!(contact.name));
    }
    node.insert(keys.email.clone(), json!(iri_text(&email)));
    Value::Object(node)
}

/// A catalogue and its datasets' IRIs in a profile's compact JSON-LD: the LKOD and the Slovak
/// catalogue interface are two key dictionaries of this one writer.
pub fn serialize_compact_catalogue(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    keys: &Compact,
) -> String {
    let base = catalogue.base_url();
    let cat_iri = format!("{base}/catalog");
    let node = &keys.catalogue;

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(node.context));
    doc.insert(keys.iri.clone(), json!(cat_iri));
    doc.insert(keys.kind.clone(), json!(node.class));
    for (member, key) in &node.members {
        let value = match member {
            CatalogueMember::Title => Some(json!(catalogue.title)),
            CatalogueMember::Description => Some(json!(catalogue.description)),
            CatalogueMember::Contact => catalogue
                .contact_point
                .as_ref()
                .map(|cp| compact_contact(&node.contact, &keys.kind, cp)),
            CatalogueMember::Homepage => {
                Some(json!(catalogue.homepage.as_deref().unwrap_or(&base)))
            }
            CatalogueMember::Publisher => catalogue.declared_publisher_uri().map(|iri| json!(iri)),
            CatalogueMember::AccessPoint => Some(json!(cat_iri)),
            CatalogueMember::Datasets => Some(json!(datasets
                .iter()
                .map(|d| d.canonical_uri(&base))
                .collect::<Vec<_>>())),
        };
        if let Some(value) = value {
            doc.insert(key.clone(), value);
        }
    }
    format!("{:#}", Value::Object(doc))
}

/// A dataset in a profile's compact JSON-LD.
pub fn serialize_compact_dataset(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
    profile: DcatProfile,
    keys: &Compact,
) -> String {
    let base = catalogue.base_url();
    let ds_iri = dataset.canonical_uri(&base);
    let node = &keys.dataset;

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(node.context));
    doc.insert(keys.iri.clone(), json!(ds_iri));
    doc.insert(keys.kind.clone(), json!(node.class));
    for (member, key) in &node.members {
        let value = match member {
            DatasetMember::Title => Some(json!(dataset.title)),
            DatasetMember::Description => Some(json!(dataset.description)),
            DatasetMember::Publisher => dataset_publisher(dataset, catalogue, publisher)
                .and_then(|(iri, _)| iri)
                .map(|iri| json!(iri_text(&iri))),
            DatasetMember::Themes => (!dataset.themes.is_empty()).then(|| {
                json!(dataset
                    .themes
                    .iter()
                    .map(|t| dataroom_core::vocab::theme_to_iri(t))
                    .collect::<Vec<_>>())
            }),
            DatasetMember::Keywords => {
                (!dataset.keywords.is_empty()).then(|| json!(dataset.keywords))
            }
            DatasetMember::KeywordsOrTags => {
                let mut words = dataset.keywords.clone();
                if words.is_empty() && !dataset.tags.is_empty() {
                    words.insert(catalogue.default_language.clone(), dataset.tags.clone());
                }
                Some(json!(words))
            }
            DatasetMember::Frequency => dataset
                .frequency
                .as_ref()
                .map(|f| json!(dataroom_core::vocab::frequency_to_eu_iri(f))),
            DatasetMember::Spatial => (!dataset.spatial.is_empty()).then(|| {
                json!(dataset
                    .spatial
                    .iter()
                    .map(|s| iri_text(s))
                    .collect::<Vec<_>>())
            }),
            DatasetMember::LandingPage => dataset.landing_page.as_ref().map(|p| json!(iri_text(p))),
            DatasetMember::Contact => dataset
                .contact_point
                .as_ref()
                .map(|cp| compact_contact(&node.contact, &keys.kind, cp)),
            DatasetMember::Distributions => Some(Value::Array(
                dataset
                    .distributions
                    .iter()
                    .map(|d| compact_distribution(dataset, d, &ds_iri, profile, keys))
                    .collect(),
            )),
        };
        if let Some(value) = value {
            doc.insert(key.clone(), value);
        }
    }
    format!("{:#}", Value::Object(doc))
}

fn compact_distribution(
    dataset: &Dataset,
    dist: &Distribution,
    ds_iri: &str,
    profile: DcatProfile,
    keys: &Compact,
) -> Value {
    let iri = dist
        .uri
        .clone()
        .unwrap_or_else(|| format!("{ds_iri}/resource/{}", dist.id));
    let mut out = Map::new();
    out.insert(keys.iri.clone(), json!(iri_text(&iri)));
    out.insert(keys.kind.clone(), json!(keys.distribution.class));
    for (member, key) in &keys.distribution.members {
        let value = match member {
            DistributionMember::Terms => terms_of_use(dataset, dist, profile).map(|stated| {
                let mut node = Map::new();
                node.insert(keys.kind.clone(), json!(keys.terms.class));
                for (member, key) in &keys.terms.members {
                    let held = match member {
                        TermsMember::AuthorsWork => &stated.authors_work,
                        TermsMember::DatabaseAsWork => &stated.database_as_work,
                        TermsMember::SuiGenerisDatabase => &stated.sui_generis_database,
                        TermsMember::PersonalData => &stated.personal_data,
                    };
                    if let Some(held) = held {
                        node.insert(key.clone(), json!(iri_text(held)));
                    }
                }
                Value::Object(node)
            }),
            DistributionMember::AccessUrl => Some(json!(iri_text(&dist.access_url))),
            DistributionMember::DownloadUrl => {
                dist.download_url.as_ref().map(|u| json!(iri_text(u)))
            }
            DistributionMember::Format => dist
                .format
                .as_ref()
                .map(|f| json!(dataroom_core::vocab::format_to_eu_iri(f))),
            DistributionMember::MediaType => dist
                .media_type
                .as_ref()
                .map(|m| json!(dataroom_core::vocab::media_type_to_iana_iri(m))),
            DistributionMember::Title => (!dist.title.is_empty()).then(|| json!(dist.title)),
        };
        if let Some(value) = value {
            out.insert(key.clone(), value);
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcat::graph_to_datasets;
    use dataroom_core::{CatalogueId, OrgId};
    use uuid::Uuid;

    const DCAT: &str = "http://www.w3.org/ns/dcat#";
    const DCT: &str = "http://purl.org/dc/terms/";
    const VCARD: &str = "http://www.w3.org/2006/vcard/ns#";

    fn ids() -> (OrgId, CatalogueId) {
        (
            OrgId::from_uuid(Uuid::from_u128(1)),
            CatalogueId::from_uuid(Uuid::from_u128(2)),
        )
    }

    fn catalogue(contact: Option<Value>) -> Catalogue {
        let (org, cat) = ids();
        let mut c = json!({
            "orgId": org, "id": cat, "name": "opendata", "host": "opendata.example.org",
            "title": {"cs": "Katalog", "sk": "Katalóg", "en": "Catalogue"},
            "description": {"cs": "Popis", "sk": "Popis katalógu"},
            "defaultLanguage": "sk", "languages": ["sk", "cs", "en"],
            "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
            "modified": "2026-11-01T10:00:00Z"
        });
        if let Some(cp) = contact {
            c["contactPoint"] = cp;
        }
        serde_json::from_value(c).expect("catalogue fixture")
    }

    const CZ_CATALOG_CONTEXT_IRI: &str =
        "https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/katalog/kontext.jsonld";

    fn compact(id: &str) -> (DcatProfile, &'static Compact) {
        let profile = DcatProfile::by_id(id).expect("installed");
        let keys = profile.manifest().render.json_ld.as_ref();
        (profile, keys.expect("a compact serialisation"))
    }

    fn serialize_cz_catalogue_jsonld(cat: &Catalogue, datasets: &[Dataset]) -> String {
        serialize_compact_catalogue(cat, datasets, compact("dcat-ap-cz").1)
    }

    fn serialize_sk_catalogue_jsonld(cat: &Catalogue, datasets: &[Dataset]) -> String {
        serialize_compact_catalogue(cat, datasets, compact("dcat-ap-sk-2.0").1)
    }

    fn serialize_cz_dataset_jsonld(ds: &Dataset, cat: &Catalogue, p: Option<&Publisher>) -> String {
        let (profile, keys) = compact("dcat-ap-cz");
        serialize_compact_dataset(ds, cat, p, profile, keys)
    }

    fn serialize_sk_dataset_jsonld(ds: &Dataset, cat: &Catalogue, p: Option<&Publisher>) -> String {
        let (profile, keys) = compact("dcat-ap-sk-2.0");
        serialize_compact_dataset(ds, cat, p, profile, keys)
    }

    fn publisher_map() -> BTreeMap<String, Publisher> {
        let p: Publisher = serde_json::from_value(json!({
            "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "pub-city", "name": "city",
            "title": {"sk": "Mesto"}, "description": {},
            "uri": "https://data.gov.sk/id/legal-subject/00313271",
            "state": "active", "editorGrants": [],
            "created": "2026-11-01T10:00:00Z", "modified": "2026-11-01T10:00:00Z"
        }))
        .unwrap();
        BTreeMap::from([("city".to_owned(), p)])
    }

    fn dataset() -> Dataset {
        serde_json::from_value(json!({
            "orgId": Uuid::nil(), "catalogueId": Uuid::nil(), "id": "pkg-air", "name": "air",
            "title": {"sk": "Kvalita ovzdušia", "cs": "Kvalita ovzduší", "en": "Air quality"},
            "description": {"sk": "Merania", "cs": "Měření"},
            "publisherId": "city", "visibility": "public", "state": "active",
            "themes": ["ENVI"], "frequency": "daily",
            "keywords": {"sk": ["ovzdušie", "PM10"], "cs": ["ovzduší"]},
            "spatial": ["http://data.europa.eu/nuts/code/SK032"],
            "contactPoint": {"kind": "Organization", "name": {"sk": "Oddelenie"}, "email": "data@city.example"},
            "distributions": [{
                "id": "res-1", "datasetId": "pkg-air", "title": {"sk": "CSV súbor"},
                "accessUrl": "https://files.example.org/air", "downloadUrl": "https://files.example.org/air.csv",
                "format": "CSV", "mediaType": "text/csv",
                "termsOfUse": {"authorsWork": "https://data.gov.sk/def/ontology/law/authorsWorkType/1"}
            }, {
                "id": "res-2", "datasetId": "pkg-air", "uri": "https://files.example.org/dist/2",
                "title": {}, "accessUrl": "https://api.example.org/air"
            }],
            "issued": "2026-11-01T10:00:00Z", "modified": "2026-11-02T10:00:00Z"
        }))
        .expect("dataset fixture")
    }

    fn objects(g: &RdfGraph, subject: &str, predicate: &str) -> Vec<String> {
        let mut v: Vec<String> = g
            .objects_of(&SubjectNode::Named(subject.into()), predicate)
            .iter()
            .map(|t| t.string_value().to_owned())
            .collect();
        v.sort();
        v
    }

    fn refused(doc: &str, limits: &RdfLimits) -> DataroomError {
        parse_jsonld(doc, limits).expect_err(doc)
    }

    fn kind(e: &DataroomError) -> &str {
        match e {
            DataroomError::RdfLimitExceeded { kind, .. } => kind,
            DataroomError::Rdf(_) => "rdf",
            _ => "other",
        }
    }

    #[test]
    fn graph_serialization_round_trips_every_term_kind() {
        let mut g = RdfGraph::new();
        let ds = SubjectNode::Named("https://e.org/ds".into());
        let bn = SubjectNode::Blank("cp".into());
        g.insert(
            ds.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{DCAT}Dataset")),
        );
        g.insert(
            ds.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{DCAT}Resource")),
        );
        g.insert(
            ds.clone(),
            format!("{DCT}title"),
            Term::Literal(LiteralValue::lang("Ovzdušie", "sk")),
        );
        g.insert(
            ds.clone(),
            format!("{DCT}title"),
            Term::Literal(LiteralValue::lang("Air", "en")),
        );
        g.insert(
            ds.clone(),
            format!("{DCT}identifier"),
            Term::Literal(LiteralValue::plain("air")),
        );
        g.insert(
            ds.clone(),
            format!("{DCT}issued"),
            Term::Literal(LiteralValue::typed("2026-01-01", ns::XSD_DATE)),
        );
        g.insert(
            ds.clone(),
            "http://www.w3.org/ns/hydra/core#totalItems",
            Term::Literal(LiteralValue::typed("628", ns::XSD_INTEGER)),
        );
        g.insert(
            ds.clone(),
            format!("{DCAT}byteSize"),
            Term::Literal(LiteralValue::typed("12", ns::XSD_NON_NEGATIVE_INTEGER)),
        );
        g.insert(ds.clone(), format!("{DCAT}contactPoint"), bn.to_term());
        g.insert(
            bn.clone(),
            format!("{VCARD}hasEmail"),
            Term::Iri("mailto:a@b.c".into()),
        );
        g.insert(bn, ns::RDF_TYPE, Term::Iri(format!("{VCARD}Kind")));

        let text = serialize_graph_jsonld(&g);
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            doc["@graph"], doc["@included"],
            "@included mirrors @graph for JSON-LD 1.1 readers"
        );
        let back = parse_jsonld(&text, &RdfLimits::default()).expect("own JSON-LD parses");
        assert_eq!(back.len(), g.len());
        assert_eq!(
            objects(&back, "https://e.org/ds", &format!("{DCT}title")),
            ["Air", "Ovzdušie"]
        );
        assert_eq!(
            objects(&back, "https://e.org/ds", ns::RDF_TYPE),
            [format!("{DCAT}Dataset"), format!("{DCAT}Resource")]
        );
        assert!(back
            .triples
            .iter()
            .any(|t| t.object == Term::Literal(LiteralValue::typed("2026-01-01", ns::XSD_DATE))));
        // prod's feed: an xsd:integer is a JSON number, a nonNegativeInteger a string
        let ds_node = doc["@graph"]
            .as_array()
            .and_then(|n| n.iter().find(|n| n["@id"] == "https://e.org/ds"))
            .expect("the dataset node");
        assert_eq!(
            ds_node["http://www.w3.org/ns/hydra/core#totalItems"]["@value"],
            json!(628)
        );
        assert_eq!(ds_node[format!("{DCAT}byteSize")]["@value"], json!("12"));
        for (value, datatype) in [
            ("628", ns::XSD_INTEGER),
            ("12", ns::XSD_NON_NEGATIVE_INTEGER),
        ] {
            assert!(
                back.triples
                    .iter()
                    .any(|t| t.object == Term::Literal(LiteralValue::typed(value, datatype))),
                "{value}^^{datatype}"
            );
        }
        assert!(back
            .triples
            .iter()
            .any(|t| t.object == Term::Iri("mailto:a@b.c".into())
                && matches!(t.subject, SubjectNode::Blank(_))));
    }

    #[test]
    fn cz_catalogue_json_ld_parses_offline_into_the_catalogue_triples() {
        let cat = catalogue(Some(
            json!({"kind": "Individual", "name": {"cs": "Jan"}, "email": "mailto:jan@e.org"}),
        ));
        let mut ds = dataset();
        ds.uri = Some("https://data.example.org/ds/air".into());
        let text = serialize_cz_catalogue_jsonld(&cat, &[ds]);
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["@context"], CZ_CATALOG_CONTEXT_IRI);
        assert_eq!(doc["kontaktní_bod"]["typ"], "Jednotlivec");
        assert_eq!(
            doc["kontaktní_bod"]["e-mail"], "mailto:jan@e.org",
            "no double mailto:"
        );

        let g = parse_jsonld(&text, &RdfLimits::default()).expect("CZ catalogue parses offline");
        let cat_iri = "https://opendata.example.org/catalog";
        assert_eq!(
            objects(&g, cat_iri, ns::RDF_TYPE),
            [format!("{DCAT}Catalog")]
        );
        assert_eq!(
            objects(&g, cat_iri, &format!("{DCAT}dataset")),
            ["https://data.example.org/ds/air"]
        );
        assert!(
            objects(&g, cat_iri, &format!("{DCT}publisher")).is_empty(),
            "a dataset's publisher is not the catalogue's"
        );
        assert_eq!(
            objects(&g, cat_iri, &format!("{DCT}title")),
            ["Catalogue", "Katalog", "Katalóg"]
        );
        assert!(g
            .triples
            .iter()
            .any(|t| t.object == Term::Iri(format!("{VCARD}Individual"))));

        // The official CZ catalogue example parses offline as well.
        let example = include_str!("../../specs/dcat-ap-cz/priklady-katalog/katalog.jsonld");
        let g = parse_jsonld(example, &RdfLimits::default()).unwrap();
        assert_eq!(
            objects(
                &g,
                "https://data.gov.cz/lkod/mdcr/katalog",
                &format!("{DCAT}dataset")
            )
            .len(),
            3
        );

        // Without contact point or publisher neither is stated; the homepage is the catalogue's own.
        let bare = serialize_cz_catalogue_jsonld(&catalogue(None), &[]);
        let doc: Value = serde_json::from_str(&bare).unwrap();
        assert!(doc.get("kontaktní_bod").is_none());
        assert!(doc.get("poskytovatel").is_none());
        assert_eq!(doc["domovská_stránka"], "https://opendata.example.org");
    }

    #[test]
    fn sk_catalogue_json_ld_parses_offline_into_the_catalogue_triples() {
        let mut cat = catalogue(Some(
            json!({"kind": "Organization", "name": {"sk": "Mesto"}, "email": "data@city.example"}),
        ));
        cat.publisher_uri = Some("https://data.gov.sk/id/legal-subject/1".into());
        cat.homepage = Some("https://city.example".into());
        let text = serialize_sk_catalogue_jsonld(&cat, &[dataset()]);
        let g = parse_jsonld(&text, &RdfLimits::default()).expect("SK catalogue parses offline");
        let cat_iri = "https://opendata.example.org/catalog";
        assert_eq!(
            objects(&g, cat_iri, ns::RDF_TYPE),
            [format!("{DCAT}Catalog")]
        );
        assert_eq!(
            objects(&g, cat_iri, &format!("{DCAT}dataset")),
            ["https://opendata.example.org/dataset/air"]
        );
        assert_eq!(
            objects(&g, cat_iri, &format!("{DCT}publisher")),
            ["https://data.gov.sk/id/legal-subject/1"]
        );
        assert_eq!(
            objects(&g, cat_iri, "http://xmlns.com/foaf/0.1/homepage"),
            ["https://city.example"]
        );
        assert!(g
            .triples
            .iter()
            .any(|t| t.object == Term::Iri("mailto:data@city.example".into())));

        let bare = serialize_sk_catalogue_jsonld(&catalogue(None), &[]);
        let doc: Value = serde_json::from_str(&bare).unwrap();
        assert!(doc.get("poskytovateľ").is_none());
        assert!(doc.get("kontaktný_bod").is_none());
    }

    #[test]
    fn sk_dataset_json_ld_round_trips_into_a_dataset() {
        let cat = catalogue(None);
        let pubs = publisher_map();
        let text = serialize_sk_dataset_jsonld(&dataset(), &cat, pubs.get("city"));
        let g = parse_jsonld(&text, &RdfLimits::default()).expect("SK dataset parses offline");
        let (org, cid) = ids();
        let back = graph_to_datasets(&g, org, cid, "city", "sk");
        assert_eq!(back.len(), 1, "{text}");
        let ds = &back[0];
        assert_eq!(
            ds.title.get("sk").map(String::as_str),
            Some("Kvalita ovzdušia")
        );
        assert_eq!(ds.description.get("cs").map(String::as_str), Some("Měření"));
        assert_eq!(ds.distributions.len(), 2);
        let csv = ds
            .distributions
            .iter()
            .find(|d| d.access_url == "https://files.example.org/air")
            .expect("csv distribution");
        assert_eq!(
            csv.download_url.as_deref(),
            Some("https://files.example.org/air.csv")
        );
        assert_eq!(csv.format.as_deref(), Some("CSV"));
        assert_eq!(ds.spatial, ["http://data.europa.eu/nuts/code/SK032"]);
        assert!(ds
            .keywords
            .get("sk")
            .is_some_and(|k| k.contains(&"PM10".to_owned())));
        assert!(
            ds.frequency.as_deref().is_some_and(|f| f.contains("DAILY")),
            "{:?}",
            ds.frequency
        );
        assert_eq!(
            ds.contact_point.as_ref().map(|c| c.email.as_str()),
            Some("data@city.example")
        );

        // A dataset without themes, frequency or publisher states none of them; its tags become
        // keywords in the catalogue language.
        let mut bare = dataset();
        bare.themes.clear();
        bare.frequency = None;
        bare.keywords.clear();
        bare.tags = vec!["ovzdusie".into()];
        bare.contact_point = None;
        bare.spatial.clear();
        let doc: Value =
            serde_json::from_str(&serialize_sk_dataset_jsonld(&bare, &cat, None)).unwrap();
        assert!(doc.get("téma").is_none());
        assert!(doc.get("periodicita_aktualizácie").is_none());
        assert_eq!(doc["kľúčové_slovo"]["sk"][0], "ovzdusie");
        assert!(doc.get("poskytovateľ").is_none());
        assert!(doc.get("geografické_územie").is_none());
        assert_eq!(
            doc["distribúcia"][1]["iri"],
            "https://files.example.org/dist/2"
        );
        assert!(doc["distribúcia"][1].get("názov").is_none());
    }

    #[test]
    fn cz_dataset_json_ld_parses_offline() {
        let text = serialize_cz_dataset_jsonld(&dataset(), &catalogue(None), None);
        let g = parse_jsonld(&text, &RdfLimits::default()).expect("CZ dataset parses offline");
        let iri = "https://opendata.example.org/dataset/air";
        assert_eq!(objects(&g, iri, ns::RDF_TYPE), [format!("{DCAT}Dataset")]);
        assert_eq!(
            objects(&g, iri, &format!("{DCAT}theme")),
            ["http://publications.europa.eu/resource/authority/data-theme/ENVI"]
        );
        assert!(objects(&g, iri, &format!("{DCAT}keyword")).contains(&"ovzduší".to_owned()));

        // The CZ document carries everything the CZ profile requires, as the SK one does.
        let (org, cid) = ids();
        let back = graph_to_datasets(&g, org, cid, "city", "cs");
        assert_eq!(back.len(), 1, "{text}");
        let ds = &back[0];
        assert_eq!(ds.distributions.len(), 2, "{text}");
        let csv = ds
            .distributions
            .iter()
            .find(|d| d.access_url == "https://files.example.org/air")
            .expect("csv distribution");
        assert_eq!(
            csv.download_url.as_deref(),
            Some("https://files.example.org/air.csv")
        );
        let tou = csv.terms_of_use.as_ref().expect("terms of use");
        assert_eq!(
            tou.authors_work.as_deref(),
            Some("https://data.gov.sk/def/ontology/law/authorsWorkType/1"),
            "the distribution's own field is kept"
        );
        assert_eq!(
            tou.personal_data, None,
            "never asserted without a declaration"
        );
        assert_eq!(ds.spatial, ["http://data.europa.eu/nuts/code/SK032"]);
        assert!(
            ds.frequency.as_deref().is_some_and(|f| f.contains("DAILY")),
            "{:?}",
            ds.frequency
        );
        assert_eq!(
            ds.contact_point.as_ref().map(|c| c.email.as_str()),
            Some("data@city.example")
        );

        let mut bare = dataset();
        bare.themes.clear();
        bare.keywords.clear();
        let doc: Value =
            serde_json::from_str(&serialize_cz_dataset_jsonld(&bare, &catalogue(None), None))
                .unwrap();
        assert!(doc.get("téma").is_none() && doc.get("klíčové_slovo").is_none());
        assert!(doc.get("poskytovatel").is_none());

        // A declaration is stated in the profile's own code list, whichever list it came in.
        let mut declared = dataset();
        declared
            .extras
            .insert("personal_data".into(), "contains".into());
        declared.distributions[1].terms_of_use = Some(dataroom_core::TermsOfUse {
            personal_data: Some("https://data.gov.sk/def/personal-data-occurence-type/2".into()),
            ..Default::default()
        });
        let doc: Value = serde_json::from_str(&serialize_cz_dataset_jsonld(
            &declared,
            &catalogue(None),
            None,
        ))
        .unwrap();
        let stated = |i: usize| doc["distribuce"][i]["podmínky_užití"]["osobní_údaje"].clone();
        assert_eq!(
            [stated(0), stated(1)],
            [
                "https://data.gov.cz/podmínky-užití/obsahuje-osobní-údaje/",
                "https://data.gov.cz/podmínky-užití/neobsahuje-osobní-údaje/"
            ]
        );
    }

    #[test]
    fn a1_json_ld_limits_and_context_guards() {
        let d = RdfLimits::default();
        assert_eq!(kind(&refused("{not json", &d)), "rdf");
        assert_eq!(
            kind(&refused("{}", &RdfLimits { max_bytes: 1, ..d })),
            "bytes"
        );
        assert_eq!(
            kind(&refused(
                r#"{"@id":"https://e.org/s","http://e.org/p":"0123456789"}"#,
                &RdfLimits {
                    max_literal_length: 5,
                    ..d
                }
            )),
            "literal-length"
        );
        assert_eq!(
            kind(&refused(
                r#"{"@id":"https://e.org/s","http://e.org/p":["a","b","c"]}"#,
                &RdfLimits {
                    max_triples: 2,
                    ..d
                }
            )),
            "triples"
        );
        // Too many context entries or terms, nested contexts too deep.
        let many: Vec<Value> = (0..17).map(|_| json!({})).collect();
        assert_eq!(
            kind(&refused(&json!({"@context": many}).to_string(), &d)),
            "context-bomb"
        );
        let terms: Map<String, Value> = (0..300)
            .map(|i| (format!("t{i}"), json!("http://e.org/x")))
            .collect();
        assert_eq!(
            kind(&refused(&json!({"@context": terms}).to_string(), &d)),
            "context-bomb"
        );
        let mut nested = json!({"x": "http://e.org/x"});
        for _ in 0..10 {
            nested = json!({"t": {"@id": "http://e.org/t", "@context": nested}});
        }
        assert_eq!(
            kind(&refused(&json!({"@context": nested}).to_string(), &d)),
            "context-bomb"
        );
        // A cyclic prefix chain is a context bomb as well.
        let cyc = r#"{"@context":{"a":"b:x","b":"a:y"},"@id":"https://e.org/s","a:p":"v"}"#;
        assert_eq!(kind(&refused(cyc, &d)), "context-bomb");
        // A remote context is refused at any position: top level, inside an array, inside a
        // node, and scoped to a term.
        for doc in [
            json!({"@context": ["https://evil.example/ctx.jsonld"], "@id": "https://e.org/s"}),
            json!({"@id": "https://e.org/s", "http://e.org/p": {"@context": "https://evil.example/c", "@id": "https://e.org/o"}}),
            json!({"@context": {"t": {"@id": "http://e.org/t", "@context": "https://evil.example/c"}}, "@id": "https://e.org/s"}),
        ] {
            assert_eq!(
                kind(&refused(&doc.to_string(), &d)),
                "remote-context",
                "{doc}"
            );
        }
        // An @import in a context is never fetched either.
        let import = json!({"@context": {"@version": 1.1, "@import": "https://evil.example/c"}, "@id": "https://e.org/s", "http://e.org/p": "v"});
        assert!(
            parse_jsonld(&import.to_string(), &d).is_err(),
            "@import must not load"
        );
        // A context of the wrong JSON type, and JSON-LD the processor refuses.
        assert_eq!(
            kind(&refused(r#"{"@context": 5, "@id": "https://e.org/s"}"#, &d)),
            "rdf"
        );
        assert_eq!(
            kind(&refused(r#"{"@id": 5, "http://e.org/p": "v"}"#, &d)),
            "rdf"
        );
    }

    #[test]
    fn accepted_contexts_and_key_order() {
        let d = RdfLimits::default();
        // schema.org maps to its vocabulary without a fetch; a null context is a reset.
        let g = parse_jsonld(
            r#"{"@context": "https://schema.org/", "@id": "https://e.org/s", "name": "X"}"#,
            &d,
        )
        .unwrap();
        assert_eq!(
            objects(&g, "https://e.org/s", "http://schema.org/name"),
            ["X"]
        );
        let g = parse_jsonld(
            r#"{"@context": [null, {"n": "http://e.org/n"}], "@id": "https://e.org/s", "n": "Y"}"#,
            &d,
        )
        .unwrap();
        assert_eq!(objects(&g, "https://e.org/s", "http://e.org/n"), ["Y"]);
        // Keyword aliases (CZ `typ`, `iri`) work whatever order the keys come in.
        let doc = format!(
            r#"{{"název": {{"cs": "K"}}, "typ": "Katalog", "iri": "https://e.org/k", "@context": "{CZ_CATALOG_CONTEXT_IRI}"}}"#
        );
        let g = parse_jsonld(&doc, &d).unwrap();
        assert_eq!(
            objects(&g, "https://e.org/k", ns::RDF_TYPE),
            [format!("{DCAT}Catalog")]
        );
        // `@included` alone is read as the graph.
        let g = parse_jsonld(
            r#"{"@included": [{"@id": "https://e.org/a", "http://e.org/p": "1"}]}"#,
            &d,
        )
        .unwrap();
        assert_eq!(objects(&g, "https://e.org/a", "http://e.org/p"), ["1"]);
        // Typed and plain literals keep their datatypes; bytes are accepted directly.
        let g = parse_jsonld_bytes(
            br#"{"@id": "https://e.org/s", "http://e.org/d": {"@value": "2026-01-01", "@type": "http://www.w3.org/2001/XMLSchema#date"}, "http://e.org/b": true}"#,
            &d,
        )
        .unwrap();
        assert!(g
            .triples
            .iter()
            .any(|t| t.object == Term::Literal(LiteralValue::typed("2026-01-01", ns::XSD_DATE))));
        assert!(g
            .triples
            .iter()
            .any(|t| t.object == Term::Literal(LiteralValue::typed("true", ns::XSD_BOOLEAN))));
    }
}
