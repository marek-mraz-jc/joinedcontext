#![forbid(unsafe_code)]
//! JSON-LD parser (backed by `oxjsonld`) and serializer supporting:
//! - Standard DCAT-AP 3.0.1 JSON-LD
//! - Czech OFN LKOD JSON-LD (`https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/katalog/kontext.jsonld`)
//! - Slovak DCAT-AP-SK 2.0 JSON-LD (`https://datova-kancelaria.github.io/dcat-ap-sk-2.0/kontexty/rozhranie-katalógu-otvorených-dát.jsonld`)
//!
//! Security (A1):
//! - Remote `@context` URLs not in the built-in offline context map are refused without any network fetch.
//! - Billion-laughs / recursive `@context` term expansion or excessive context size is refused.
//! - Byte, triple, nesting-depth and literal-length caps are enforced while parsing.

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use dataroom_core::{
    text_in_lang, Catalogue, ContactKind, DataroomError, Dataset, Publisher, Result,
};
use oxjsonld::JsonLdParser;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const CZ_CATALOG_CONTEXT_IRI: &str =
    "https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/katalog/kontext.jsonld";
pub const CZ_DATASET_CONTEXT_IRI: &str =
    "https://ofn.gov.cz/dcat-ap-cz/2026-09-23/datová-sada/kontext.jsonld";
pub const SK_CATALOG_CONTEXT_IRI: &str =
    "https://datova-kancelaria.github.io/dcat-ap-sk-2.0/kontexty/rozhranie-katalógu-otvorených-dát.jsonld";

const SK_BUILTIN_CONTEXT_JSON: &str =
    include_str!("../../specs/dcat-ap-sk/kontexty/rozhranie-katalógu-otvorených-dát.jsonld");

/// Built-in CZ OFN LKOD context so CZ JSON-LD documents parse offline via `oxjsonld` with zero
/// network fetches.
const CZ_BUILTIN_CONTEXT_JSON: &str = r#"{
  "@context": {
    "@language": "cs",
    "dcat": "http://www.w3.org/ns/dcat#",
    "dcatap": "http://data.europa.eu/r5r/",
    "dct": "http://purl.org/dc/terms/",
    "foaf": "http://xmlns.com/foaf/0.1/",
    "vcard": "http://www.w3.org/2006/vcard/ns#",
    "skos": "http://www.w3.org/2004/02/skos/core#",
    "xsd": "http://www.w3.org/2001/XMLSchema#",
    "pu": "https://data.gov.cz/slovník/podmínky-užití/",
    "iri": "@id",
    "typ": "@type",
    "Katalog": "dcat:Catalog",
    "Datová sada": "dcat:Dataset",
    "Série datových sad": "dcat:DatasetSeries",
    "Distribuce": "dcat:Distribution",
    "Datová služba": "dcat:DataService",
    "Organizace": "vcard:Organization",
    "Jednotlivec": "vcard:Individual",
    "Časový interval": "dct:PeriodOfTime",
    "Podmínky užití": "pu:Specifikace",
    "název": { "@id": "dct:title", "@container": "@language" },
    "popis": { "@id": "dct:description", "@container": "@language" },
    "poskytovatel": { "@id": "dct:publisher", "@type": "@id" },
    "domovská_stránka": { "@id": "foaf:homepage", "@type": "@id" },
    "datová_sada": { "@id": "dcat:dataset", "@type": "@id" },
    "přístupový_bod": { "@id": "dcat:endpointURL", "@type": "@id" },
    "popis_přístupového_bodu": { "@id": "dcat:endpointDescription", "@type": "@id" },
    "poskytuje_datovou_sadu": { "@id": "dcat:servesDataset", "@type": "@id" },
    "kontaktní_bod": { "@id": "dcat:contactPoint", "@type": "@id" },
    "jméno": { "@id": "vcard:fn", "@container": "@language" },
    "e-mail": { "@id": "vcard:hasEmail", "@type": "@id" },
    "téma": { "@id": "dcat:theme", "@type": "@id" },
    "periodicita_aktualizace": { "@id": "dct:accrualPeriodicity", "@type": "@id" },
    "klíčové_slovo": { "@id": "dcat:keyword", "@container": "@language" },
    "prvek_rúian": { "@id": "dct:spatial", "@type": "@id" },
    "geografické_území": { "@id": "dct:spatial", "@type": "@id" },
    "související_geografické_území": { "@id": "dct:spatial", "@type": "@id" },
    "časové_pokrytí": { "@id": "dct:temporal", "@type": "@id" },
    "začátek": { "@id": "dcat:startDate", "@type": "xsd:date" },
    "konec": { "@id": "dcat:endDate", "@type": "xsd:date" },
    "vstupní_stránka": { "@id": "dcat:landingPage", "@type": "@id" },
    "dokumentace": { "@id": "foaf:page", "@type": "@id" },
    "specifikace": { "@id": "dct:conformsTo", "@type": "@id" },
    "příslušný_právní_předpis": { "@id": "dcatap:applicableLegislation", "@type": "@id" },
    "koncept_euroVoc": { "@id": "dcat:theme", "@type": "@id" },
    "prostorové_rozlišení_v_metrech": { "@id": "dcat:spatialResolutionInMeters", "@type": "xsd:decimal" },
    "časové_rozlišení": { "@id": "dcat:temporalResolution", "@type": "xsd:duration" },
    "je_součástí": { "@id": "dcat:inSeries", "@type": "@id" },
    "distribuce": { "@id": "dcat:distribution", "@type": "@id" },
    "podmínky_užití": { "@id": "pu:specifikace", "@type": "@id" },
    "autorské_dílo": { "@id": "pu:autorské-dílo", "@type": "@id" },
    "autor": { "@id": "pu:autor", "@container": "@language" },
    "databáze_jako_autorské_dílo": { "@id": "pu:databáze-jako-autorské-dílo", "@type": "@id" },
    "autor_databáze": { "@id": "pu:autor-databáze", "@container": "@language" },
    "databáze_chráněná_zvláštními_právy": { "@id": "pu:databáze-chráněná-zvláštními-právy", "@type": "@id" },
    "osobní_údaje": { "@id": "pu:osobní-údaje", "@type": "@id" },
    "přístupové_url": { "@id": "dcat:accessURL", "@type": "@id" },
    "soubor_ke_stažení": { "@id": "dcat:downloadURL", "@type": "@id" },
    "formát": { "@id": "dct:format", "@type": "@id" },
    "typ_média": { "@id": "dcat:mediaType", "@type": "@id" },
    "schéma": { "@id": "dct:conformsTo", "@type": "@id" },
    "typ_média_komprese": { "@id": "dcat:compressFormat", "@type": "@id" },
    "typ_média_balíčku": { "@id": "dcat:packageFormat", "@type": "@id" },
    "přístupová_služba": { "@id": "dcat:accessService", "@type": "@id" }
  }
}"#;

/// Maximum number of terms allowed in a JSON-LD `@context` to prevent billion-laughs context bombs (A1).
const MAX_CONTEXT_TERMS: usize = 256;

/// Resolves a known built-in `@context` URL to its local inline `@context` Value, or refuses any
/// unknown remote `@context` URL without fetching (A1).
fn inline_and_check_jsonld(
    val: &mut Value,
    limits: &RdfLimits,
    depth: usize,
) -> Result<()> {
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
            if trimmed == SK_CATALOG_CONTEXT_IRI
                || trimmed.ends_with("rozhranie-katalógu-otvorených-dát.jsonld")
            {
                let mut parsed: Value = serde_json::from_str(SK_BUILTIN_CONTEXT_JSON)
                    .map_err(|e| DataroomError::Rdf(format!("built-in SK context: {e}")))?;
                if let Some(inner) = parsed.get_mut("@context") {
                    if let Some(map) = inner.as_object_mut() {
                        // Add missing prefix `dct:` used in SK context `dct:issued`, `dct:modified`, `dct:relation`
                        map.insert("dct".into(), json!("http://purl.org/dc/terms/"));
                        // Add aliases used in `specs/dcat-ap-sk/schémy/dataset.json` and `dataset.jsonld`
                        map.insert(
                            "jméno".into(),
                            json!({ "@id": "vcard:fn", "@container": "@language" }),
                        );
                        map.insert(
                            "začátek".into(),
                            json!({ "@id": "dcat:startDate", "@type": "xsd:date" }),
                        );
                        map.insert(
                            "autor_databáze".into(),
                            json!({ "@id": "leg:originalDatabaseAuthorName", "@container": "@language" }),
                        );
                        map.insert(
                            "dokumentácia".into(),
                            json!({ "@id": "foaf:page", "@type": "@id" }),
                        );
                        map.insert(
                            "prvek_rúian".into(),
                            json!({ "@id": "dcterms:spatial", "@type": "@id" }),
                        );
                    }
                    *ctx_val = inner.take();
                }
                Ok(())
            } else if trimmed == CZ_CATALOG_CONTEXT_IRI
                || trimmed == CZ_DATASET_CONTEXT_IRI
                || trimmed.starts_with("https://ofn.gov.cz/dcat-ap-cz")
            {
                let mut parsed: Value = serde_json::from_str(CZ_BUILTIN_CONTEXT_JSON)
                    .map_err(|e| DataroomError::Rdf(format!("built-in CZ context: {e}")))?;
                if let Some(inner) = parsed.get_mut("@context") {
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
        .with_base_iri("https://dataroom.local/")
        .map_err(|e| DataroomError::Rdf(format!("base IRI error: {e}")))?;

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
    for (subj, preds) in by_subj {
        let mut obj = Map::new();
        match subj {
            SubjectNode::Named(iri) => {
                obj.insert("@id".into(), json!(iri));
            }
            SubjectNode::Blank(id) => {
                obj.insert("@id".into(), json!(format!("_:{id}")));
            }
        }
        for (pred, terms) in preds {
            if pred == ns::RDF_TYPE {
                let types: Vec<Value> = terms
                    .iter()
                    .map(|t| json!(t.string_value()))
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
                    Term::Iri(iri) => json!({ "@id": iri }),
                    Term::Blank(id) => json!({ "@id": format!("_:{id}") }),
                    Term::Literal(lit) => {
                        if let Some(lang) = &lit.language {
                            json!({ "@value": lit.value, "@language": lang })
                        } else if lit.datatype == ns::XSD_STRING {
                            json!(lit.value)
                        } else {
                            json!({ "@value": lit.value, "@type": lit.datatype })
                        }
                    }
                })
                .collect();
            obj.insert(
                pred.to_owned(),
                if values.len() == 1 {
                    values[0].clone()
                } else {
                    Value::Array(values)
                },
            );
        }
        nodes.push(Value::Object(obj));
    }

    let doc = json!({
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
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
}

/// Serializes a [`Catalogue`] and its dataset IRIs in the Czech OFN LKOD JSON-LD format
/// (`specs/dcat-ap-cz/spec-dcat-ap-cz-rozhrani-katalogu.txt` & `priklady-katalog/katalog.jsonld`).
pub fn serialize_cz_catalogue_jsonld(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
) -> String {
    let base = catalogue.base_url();
    let cat_iri = format!("{base}/catalog");
    let publisher_iri = catalogue
        .publisher_uri
        .clone()
        .or_else(|| publishers.values().next().map(|p| p.uri.clone()))
        .unwrap_or_else(|| "https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/00000000".into());

    let ds_iris: Vec<String> = datasets
        .iter()
        .map(|d| d.canonical_uri(&base))
        .collect();

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(CZ_CATALOG_CONTEXT_IRI));
    doc.insert("iri".into(), json!(cat_iri));
    doc.insert("typ".into(), json!("Katalog"));
    doc.insert("název".into(), json!(catalogue.title));
    doc.insert("popis".into(), json!(catalogue.description));
    if let Some(cp) = &catalogue.contact_point {
        let email = if cp.email.starts_with("mailto:") {
            cp.email.clone()
        } else {
            format!("mailto:{}", cp.email)
        };
        doc.insert(
            "kontaktní_bod".into(),
            json!({
                "typ": match cp.kind {
                    ContactKind::Organization => "Organizace",
                    ContactKind::Individual => "Jednotlivec",
                },
                "jméno": cp.name,
                "e-mail": email,
            }),
        );
    }
    doc.insert(
        "domovská_stránka".into(),
        json!(catalogue.homepage.as_deref().unwrap_or(&base)),
    );
    doc.insert("poskytovatel".into(), json!(publisher_iri));
    doc.insert("přístupový_bod".into(), json!(cat_iri));
    doc.insert("datová_sada".into(), json!(ds_iris));

    serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_else(|_| "{}".into())
}

/// Serializes a [`Catalogue`] and its dataset IRIs in the Slovak DCAT-AP-SK 2.0 JSON-LD format
/// (`specs/dcat-ap-sk/schémy/katalóg.json` & `príklady/katalóg.jsonld`).
pub fn serialize_sk_catalogue_jsonld(
    catalogue: &Catalogue,
    datasets: &[Dataset],
    publishers: &BTreeMap<String, Publisher>,
) -> String {
    let base = catalogue.base_url();
    let cat_iri = format!("{base}/catalog");
    let publisher_iri = catalogue
        .publisher_uri
        .clone()
        .or_else(|| publishers.values().next().map(|p| p.uri.clone()))
        .unwrap_or_else(|| "https://data.gov.sk/id/legal-subject/00000000".into());

    let ds_iris: Vec<String> = datasets
        .iter()
        .map(|d| d.canonical_uri(&base))
        .collect();

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(SK_CATALOG_CONTEXT_IRI));
    doc.insert("iri".into(), json!(cat_iri));
    doc.insert("typ".into(), json!("Katalóg"));
    doc.insert("názov".into(), json!(catalogue.title));
    doc.insert("popis".into(), json!(catalogue.description));
    if let Some(cp) = &catalogue.contact_point {
        let email = if cp.email.starts_with("mailto:") {
            cp.email.clone()
        } else {
            format!("mailto:{}", cp.email)
        };
        doc.insert(
            "kontaktný_bod".into(),
            json!({
                "typ": "Organizácia",
                "meno": cp.name,
                "e-mail": email,
            }),
        );
    }
    doc.insert(
        "domovská_stránka".into(),
        json!(catalogue.homepage.as_deref().unwrap_or(&base)),
    );
    doc.insert("poskytovateľ".into(), json!(publisher_iri));
    doc.insert("dataset".into(), json!(ds_iris));

    serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_else(|_| "{}".into())
}

/// Serializes a [`Dataset`] in the Slovak DCAT-AP-SK 2.0 JSON-LD format
/// (`specs/dcat-ap-sk/schémy/dataset.json` & `príklady/dataset.jsonld`).
pub fn serialize_sk_dataset_jsonld(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
) -> String {
    let base = catalogue.base_url();
    let ds_iri = dataset.canonical_uri(&base);
    let pub_iri = publisher
        .map(|p| p.uri.clone())
        .or_else(|| dataset.publisher.as_ref().and_then(|a| a.uri.clone()))
        .unwrap_or_else(|| format!("{base}/publisher/{}", dataset.publisher_id));

    let themes: Vec<String> = if dataset.themes.is_empty() {
        vec!["http://publications.europa.eu/resource/authority/data-theme/GOVE".into()]
    } else {
        dataset
            .themes
            .iter()
            .map(|t| dataroom_core::vocab::theme_to_iri(t))
            .collect()
    };

    let freq = dataset
        .frequency
        .as_deref()
        .map(dataroom_core::vocab::frequency_to_eu_iri)
        .unwrap_or_else(|| "http://publications.europa.eu/resource/authority/frequency/IRREG".into());

    let mut kw = dataset.keywords.clone();
    if kw.is_empty() && !dataset.tags.is_empty() {
        kw.insert(catalogue.default_language.clone(), dataset.tags.clone());
    }

    let dists: Vec<Value> = dataset
        .distributions
        .iter()
        .map(|d| {
            let dist_iri = d
                .uri
                .clone()
                .unwrap_or_else(|| format!("{ds_iri}/resource/{}", d.id));
            let tou = d.terms_of_use.clone().unwrap_or_default();
            let mut dist_obj = Map::new();
            dist_obj.insert("iri".into(), json!(dist_iri));
            dist_obj.insert("typ".into(), json!("Distribúcia"));
            dist_obj.insert(
                "podmienky_použitia".into(),
                json!({
                    "typ": "Špecifikácia podmienok použitia",
                    "autorské_dielo": tou.authors_work.unwrap_or_else(|| "https://creativecommons.org/licenses/by/4.0/".into()),
                    "databáza_ako_autorské_dielo": tou.database_as_work.unwrap_or_else(|| "https://creativecommons.org/licenses/by/4.0/".into()),
                    "databáza_chránená_zvláštnými_právami": tou.sui_generis_database.unwrap_or_else(|| "https://creativecommons.org/publicdomain/zero/1.0/".into()),
                    "osobné_údaje": tou.personal_data.unwrap_or_else(|| "https://data.gov.sk/def/personal-data-occurence-type/2".into()),
                }),
            );
            dist_obj.insert("prístupové_url".into(), json!(d.access_url));
            if let Some(dl) = &d.download_url {
                dist_obj.insert("súbor_na_stiahnutie".into(), json!(dl));
            }
            if let Some(fmt) = &d.format {
                dist_obj.insert(
                    "formát".into(),
                    json!(dataroom_core::vocab::format_to_eu_iri(fmt)),
                );
            }
            if let Some(mt) = &d.media_type {
                dist_obj.insert(
                    "typ_média".into(),
                    json!(dataroom_core::vocab::media_type_to_iana_iri(mt)),
                );
            }
            if !d.title.is_empty() {
                dist_obj.insert("názov".into(), json!(d.title));
            }
            Value::Object(dist_obj)
        })
        .collect();

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(SK_CATALOG_CONTEXT_IRI));
    doc.insert("iri".into(), json!(ds_iri));
    doc.insert("typ".into(), json!("Dataset"));
    doc.insert("názov".into(), json!(dataset.title));
    doc.insert("popis".into(), json!(dataset.description));
    doc.insert("poskytovateľ".into(), json!(pub_iri));
    doc.insert("téma".into(), json!(themes));
    doc.insert("periodicita_aktualizácie".into(), json!(freq));
    doc.insert("kľúčové_slovo".into(), json!(kw));
    if !dataset.spatial.is_empty() {
        doc.insert("geografické_územie".into(), json!(dataset.spatial));
    }
    if let Some(cp) = &dataset.contact_point {
        let email = if cp.email.starts_with("mailto:") {
            cp.email.clone()
        } else {
            format!("mailto:{}", cp.email)
        };
        doc.insert(
            "kontaktný_bod".into(),
            json!({
                "typ": "Organizácia",
                "jméno": cp.name,
                "meno": cp.name,
                "e-mail": email,
            }),
        );
    }
    doc.insert("distribúcia".into(), Value::Array(dists));

    serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_else(|_| "{}".into())
}

/// Serializes a [`Dataset`] in the Czech DCAT-AP-CZ JSON-LD format.
pub fn serialize_cz_dataset_jsonld(
    dataset: &Dataset,
    catalogue: &Catalogue,
    publisher: Option<&Publisher>,
) -> String {
    let base = catalogue.base_url();
    let ds_iri = dataset.canonical_uri(&base);
    let pub_iri = publisher
        .map(|p| p.uri.clone())
        .or_else(|| dataset.publisher.as_ref().and_then(|a| a.uri.clone()))
        .unwrap_or_else(|| "https://rpp-opendata.egon.gov.cz/odrpp/zdroj/orgán-veřejné-moci/00000000".into());

    let mut doc = Map::new();
    doc.insert("@context".into(), json!(CZ_DATASET_CONTEXT_IRI));
    doc.insert("iri".into(), json!(ds_iri));
    doc.insert("typ".into(), json!("Datová sada"));
    doc.insert("název".into(), json!(dataset.title));
    doc.insert("popis".into(), json!(dataset.description));
    doc.insert("poskytovatel".into(), json!(pub_iri));
    if !dataset.themes.is_empty() {
        let themes: Vec<String> = dataset
            .themes
            .iter()
            .map(|t| dataroom_core::vocab::theme_to_iri(t))
            .collect();
        doc.insert("téma".into(), json!(themes));
    }
    if !dataset.keywords.is_empty() {
        doc.insert("klíčové_slovo".into(), json!(dataset.keywords));
    }
    if let Some(l) = text_in_lang(&dataset.title, Some("cs")) {
        let _ = l;
    }
    serde_json::to_string_pretty(&Value::Object(doc)).unwrap_or_else(|_| "{}".into())
}
