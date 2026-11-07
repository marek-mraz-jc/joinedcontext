#![forbid(unsafe_code)]
//! JSON-LD parser and serializer supporting:
//! - Standard DCAT-AP 3.0.1 JSON-LD
//! - Czech OFN LKOD JSON-LD (`https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/katalog/kontext.jsonld`)
//! - Slovak DCAT-AP-SK 2.0 JSON-LD (`https://datova-kancelaria.github.io/dcat-ap-sk-2.0/kontexty/rozhranie-katalógu-otvorených-dát.jsonld`)

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use dataroom_core::{
    text_in_lang, Catalogue, ContactKind, DataroomError, Dataset, Publisher, Result,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const CZ_CATALOG_CONTEXT_IRI: &str =
    "https://ofn.gov.cz/dcat-ap-cz-rozhraní-katalogů-dat/2026-09-23/katalog/kontext.jsonld";
pub const CZ_DATASET_CONTEXT_IRI: &str =
    "https://ofn.gov.cz/dcat-ap-cz/2026-09-23/datová-sada/kontext.jsonld";
pub const SK_CATALOG_CONTEXT_IRI: &str =
    "https://datova-kancelaria.github.io/dcat-ap-sk-2.0/kontexty/rozhranie-katalógu-otvorených-dát.jsonld";

#[derive(Clone, Copy, PartialEq, Eq)]
enum TermKind {
    Iri,
    LangContainer,
    PlainLiteral,
    TypedLiteral(&'static str),
}

/// Maps CZ and SK localized JSON-LD property terms to `(predicate_iri, TermKind)`.
fn resolve_localized_term(key: &str) -> Option<(&'static str, TermKind)> {
    Some(match key {
        // Catalogue & Dataset core
        "název" | "názov" | "dct:title" | "dcterms:title" => (ns::DCT, TermKind::LangContainer),
        "popis" | "dct:description" | "dcterms:description" => (ns::DCT, TermKind::LangContainer),
        "poskytovatel" | "poskytovateľ" | "dct:publisher" | "dcterms:publisher" => {
            ("http://purl.org/dc/terms/publisher", TermKind::Iri)
        }
        "domovská_stránka" | "foaf:homepage" => ("http://xmlns.com/foaf/0.1/homepage", TermKind::Iri),
        "datová_sada" | "dataset" | "dcat:dataset" => ("http://www.w3.org/ns/dcat#dataset", TermKind::Iri),
        "přístupový_bod" | "prístupový_bod" | "dcat:endpointURL" => {
            ("http://www.w3.org/ns/dcat#endpointURL", TermKind::Iri)
        }
        "popis_přístupového_bodu" | "popis_prístupového_bodu" | "dcat:endpointDescription" => {
            ("http://www.w3.org/ns/dcat#endpointDescription", TermKind::Iri)
        }
        "poskytuje_datovou_sadu" | "poskytuje_dataset" | "dcat:servesDataset" => {
            ("http://www.w3.org/ns/dcat#servesDataset", TermKind::Iri)
        }
        "kontaktní_bod" | "kontaktný_bod" | "dcat:contactPoint" => {
            ("http://www.w3.org/ns/dcat#contactPoint", TermKind::Iri)
        }
        "jméno" | "meno" | "vcard:fn" => ("http://www.w3.org/2006/vcard/ns#fn", TermKind::LangContainer),
        "e-mail" | "vcard:hasEmail" => ("http://www.w3.org/2006/vcard/ns#hasEmail", TermKind::Iri),
        "dátum_publikácie" | "dct:issued" | "dcterms:issued" => (
            "http://purl.org/dc/terms/issued",
            TermKind::TypedLiteral(ns::XSD_DATE_TIME),
        ),
        "dátum_modifikácie" | "dct:modified" | "dcterms:modified" => (
            "http://purl.org/dc/terms/modified",
            TermKind::TypedLiteral(ns::XSD_DATE_TIME),
        ),
        "typ_datasetu" | "dct:type" | "dcterms:type" => ("http://purl.org/dc/terms/type", TermKind::Iri),
        "téma" | "koncept_euroVoc" | "dcat:theme" => ("http://www.w3.org/ns/dcat#theme", TermKind::Iri),
        "periodicita_aktualizácie" | "periodicita_aktualizace" | "dct:accrualPeriodicity" => (
            "http://purl.org/dc/terms/accrualPeriodicity",
            TermKind::Iri,
        ),
        "kľúčové_slovo" | "klíčové_slovo" | "dcat:keyword" => (
            "http://www.w3.org/ns/dcat#keyword",
            TermKind::LangContainer,
        ),
        "geografické_územie"
        | "prvek_rúian"
        | "prvok_registra_adries"
        | "priestorové_pokrytie"
        | "související_geografické_území"
        | "dct:spatial" => ("http://purl.org/dc/terms/spatial", TermKind::Iri),
        "časové_pokrytie" | "časové_pokrytí" | "dct:temporal" => {
            ("http://purl.org/dc/terms/temporal", TermKind::Iri)
        }
        "začiatok" | "začátek" | "dcat:startDate" => (
            "http://www.w3.org/ns/dcat#startDate",
            TermKind::TypedLiteral(ns::XSD_DATE),
        ),
        "koniec" | "konec" | "dcat:endDate" => (
            "http://www.w3.org/ns/dcat#endDate",
            TermKind::TypedLiteral(ns::XSD_DATE),
        ),
        "je_súčásťou" | "dct:isPartOf" => ("http://purl.org/dc/terms/isPartOf", TermKind::Iri),
        "je_součástí" | "dcat:inSeries" => ("http://www.w3.org/ns/dcat#inSeries", TermKind::Iri),
        "webová_stránka_datasetu" | "vstupní_stránka" | "dcat:landingPage" => {
            ("http://www.w3.org/ns/dcat#landingPage", TermKind::Iri)
        }
        "súvisiaci_zdroj_datasetu" | "dct:relation" => ("http://purl.org/dc/terms/relation", TermKind::Iri),
        "dokumentácia" | "dokumentace" | "foaf:page" => ("http://xmlns.com/foaf/0.1/page", TermKind::Iri),
        "špecifikácia" | "specifikace" | "schéma" | "dct:conformsTo" => {
            ("http://purl.org/dc/terms/conformsTo", TermKind::Iri)
        }
        "příslušný_právní_předpis" | "dcatap:applicableLegislation" => (
            "http://data.europa.eu/r5r/applicableLegislation",
            TermKind::Iri,
        ),
        "priestorové_rozlíšenie_v_metroch"
        | "prostorové_rozlišení_v_metrech"
        | "dcat:spatialResolutionInMeters" => (
            "http://www.w3.org/ns/dcat#spatialResolutionInMeters",
            TermKind::TypedLiteral(ns::XSD_DECIMAL),
        ),
        "časové_rozlíšenie" | "časové_rozlišení" | "dcat:temporalResolution" => (
            "http://www.w3.org/ns/dcat#temporalResolution",
            TermKind::TypedLiteral(ns::XSD_DURATION),
        ),
        "distribúcia" | "distribuce" | "dcat:distribution" => {
            ("http://www.w3.org/ns/dcat#distribution", TermKind::Iri)
        }
        "podmienky_použitia" | "leg:termsOfUse" => (
            "https://data.gov.sk/def/ontology/legislation/termsOfUse",
            TermKind::Iri,
        ),
        "podmínky_užití" => (
            "https://data.gov.cz/slovník/podmínky-užití/specifikace",
            TermKind::Iri,
        ),
        "autorské_dielo" | "leg:authorsWorkType" => (
            "https://data.gov.sk/def/ontology/legislation/authorsWorkType",
            TermKind::Iri,
        ),
        "autorské_dílo" => (
            "https://data.gov.cz/slovník/podmínky-užití/autorské-dílo",
            TermKind::Iri,
        ),
        "autor" | "leg:authorName" => (
            "https://data.gov.sk/def/ontology/legislation/authorName",
            TermKind::LangContainer,
        ),
        "databáza_ako_autorské_dielo" | "leg:originalDatabaseType" => (
            "https://data.gov.sk/def/ontology/legislation/originalDatabaseType",
            TermKind::Iri,
        ),
        "databáze_jako_autorské_dílo" => (
            "https://data.gov.cz/slovník/podmínky-užití/databáze-jako-autorské-dílo",
            TermKind::Iri,
        ),
        "autor_databázy" | "autor_databáze" | "leg:originalDatabaseAuthorName" => (
            "https://data.gov.sk/def/ontology/legislation/originalDatabaseAuthorName",
            TermKind::LangContainer,
        ),
        "databáza_chránená_zvláštnými_právami" | "leg:databaseProtectedBySpecialRightsType" => (
            "https://data.gov.sk/def/ontology/legislation/databaseProtectedBySpecialRightsType",
            TermKind::Iri,
        ),
        "databáze_chráněná_zvláštními_právy" => (
            "https://data.gov.cz/slovník/podmínky-užití/databáze-chráněná-zvláštními-právy",
            TermKind::Iri,
        ),
        "osobné_údaje" | "leg:personalDataContainmentType" => (
            "https://data.gov.sk/def/ontology/legislation/personalDataContainmentType",
            TermKind::Iri,
        ),
        "osobní_údaje" => (
            "https://data.gov.cz/slovník/podmínky-užití/osobní-údaje",
            TermKind::Iri,
        ),
        "prístupové_url" | "přístupové_url" | "dcat:accessURL" => {
            ("http://www.w3.org/ns/dcat#accessURL", TermKind::Iri)
        }
        "súbor_na_stiahnutie" | "soubor_ke_stažení" | "dcat:downloadURL" => {
            ("http://www.w3.org/ns/dcat#downloadURL", TermKind::Iri)
        }
        "formát" | "dct:format" => ("http://purl.org/dc/terms/format", TermKind::Iri),
        "typ_média" | "dcat:mediaType" => ("http://www.w3.org/ns/dcat#mediaType", TermKind::Iri),
        "typ_média_kompresie" | "dcat:compressFormat" => {
            ("http://www.w3.org/ns/dcat#compressFormat", TermKind::Iri)
        }
        "typ_média_balíčka" | "dcat:packageFormat" => {
            ("http://www.w3.org/ns/dcat#packageFormat", TermKind::Iri)
        }
        "prístupová_služba" | "přístupová_služba" | "dcat:accessService" => {
            ("http://www.w3.org/ns/dcat#accessService", TermKind::Iri)
        }
        _ => return None,
    })
}

fn resolve_localized_class(ty: &str) -> String {
    match ty {
        "Katalog" | "Katalóg" | "dcat:Catalog" => "http://www.w3.org/ns/dcat#Catalog".into(),
        "Datová sada" | "Dataset" | "dcat:Dataset" => "http://www.w3.org/ns/dcat#Dataset".into(),
        "Série datových sad" | "dcat:DatasetSeries" => {
            "http://www.w3.org/ns/dcat#DatasetSeries".into()
        }
        "Distribuce" | "Distribúcia" | "dcat:Distribution" => {
            "http://www.w3.org/ns/dcat#Distribution".into()
        }
        "Datová služba" | "Dátová služba" | "dcat:DataService" => {
            "http://www.w3.org/ns/dcat#DataService".into()
        }
        "Organizace" | "Organizácia" | "vcard:Organization" => {
            "http://www.w3.org/2006/vcard/ns#Organization".into()
        }
        "Jednotlivec" | "vcard:Individual" => {
            "http://www.w3.org/2006/vcard/ns#Individual".into()
        }
        "vcard:Kind" => "http://www.w3.org/2006/vcard/ns#Kind".into(),
        "Časový interval" | "dct:PeriodOfTime" | "dcterms:PeriodOfTime" => {
            "http://purl.org/dc/terms/PeriodOfTime".into()
        }
        "Špecifikácia podmienok použitia" | "leg:TermsOfUse" => {
            "https://data.gov.sk/def/ontology/legislation/TermsOfUse".into()
        }
        "Podmínky užití" => "https://data.gov.cz/slovník/podmínky-užití/Specifikace".into(),
        "foaf:Agent" | "foaf:Organization" => "http://xmlns.com/foaf/0.1/Agent".into(),
        other => other.to_owned(),
    }
}

/// Parses a JSON-LD document (standard DCAT-AP, CZ OFN LKOD, or SK DCAT-AP-SK 2.0) into an [`RdfGraph`].
pub fn parse_jsonld(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    if input.len() > limits.max_bytes {
        return Err(DataroomError::Rdf(format!(
            "JSON-LD input size {} exceeds limit of {} bytes",
            input.len(),
            limits.max_bytes
        )));
    }
    let val: Value = serde_json::from_str(input)
        .map_err(|e| DataroomError::Rdf(format!("invalid JSON-LD: {e}")))?;
    let mut graph = RdfGraph::new();
    parse_node_value(&val, &mut graph, limits, 0)?;
    Ok(graph)
}

fn parse_node_value(
    val: &Value,
    graph: &mut RdfGraph,
    limits: &RdfLimits,
    depth: usize,
) -> Result<Option<SubjectNode>> {
    if depth > limits.max_depth {
        return Err(DataroomError::Rdf(format!(
            "JSON-LD nesting exceeded maximum depth of {}",
            limits.max_depth
        )));
    }
    match val {
        Value::Array(items) => {
            let mut last = None;
            for item in items {
                last = parse_node_value(item, graph, limits, depth + 1)?;
            }
            Ok(last)
        }
        Value::Object(obj) => {
            if let Some(Value::Array(graph_items)) = obj.get("@graph").or_else(|| obj.get("@included")) {
                for item in graph_items {
                    parse_node_value(item, graph, limits, depth + 1)?;
                }
            }
            let subj = if let Some(id) = obj
                .get("@id")
                .or_else(|| obj.get("iri"))
                .and_then(Value::as_str)
            {
                if let Some(b) = id.strip_prefix("_:") {
                    SubjectNode::Blank(b.to_owned())
                } else {
                    SubjectNode::Named(id.to_owned())
                }
            } else {
                graph.fresh_blank_node()
            };

            if let Some(ty_val) = obj.get("@type").or_else(|| obj.get("typ")) {
                let types: Vec<&str> = match ty_val {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(arr) => arr.iter().filter_map(Value::as_str).collect(),
                    _ => Vec::new(),
                };
                for t in types {
                    let class_iri = resolve_localized_class(t);
                    graph.insert_checked(
                        Triple {
                            subject: subj.clone(),
                            predicate: ns::RDF_TYPE.to_owned(),
                            object: Term::Iri(class_iri),
                        },
                        limits,
                    )?;
                }
            }

            for (key, prop_val) in obj {
                if matches!(
                    key.as_str(),
                    "@context" | "@id" | "iri" | "@type" | "typ" | "@graph" | "@included"
                ) {
                    continue;
                }
                let (pred_iri, kind) = match key.as_str() {
                    "název" | "názov" | "dct:title" | "dcterms:title" => {
                        ("http://purl.org/dc/terms/title".to_owned(), TermKind::LangContainer)
                    }
                    "popis" | "dct:description" | "dcterms:description" => (
                        "http://purl.org/dc/terms/description".to_owned(),
                        TermKind::LangContainer,
                    ),
                    other => match resolve_localized_term(other) {
                        Some((p, k)) => (p.to_owned(), k),
                        None => (expand_common_prefix(other), TermKind::PlainLiteral),
                    },
                };

                add_jsonld_property(
                    &subj,
                    &pred_iri,
                    kind,
                    prop_val,
                    graph,
                    limits,
                    depth + 1,
                )?;
            }

            Ok(Some(subj))
        }
        _ => Ok(None),
    }
}

fn add_jsonld_property(
    subj: &SubjectNode,
    pred: &str,
    kind: TermKind,
    val: &Value,
    graph: &mut RdfGraph,
    limits: &RdfLimits,
    depth: usize,
) -> Result<()> {
    match (kind, val) {
        (TermKind::LangContainer, Value::Object(lang_map))
            if !lang_map.contains_key("@value") && !lang_map.contains_key("@id") =>
        {
            for (lang, text_or_arr) in lang_map {
                match text_or_arr {
                    Value::String(s) => {
                        graph.insert_checked(
                            Triple {
                                subject: subj.clone(),
                                predicate: pred.to_owned(),
                                object: Term::Literal(LiteralValue::lang(s.clone(), lang.clone())),
                            },
                            limits,
                        )?;
                    }
                    Value::Array(items) => {
                        for item in items {
                            if let Some(s) = item.as_str() {
                                graph.insert_checked(
                                    Triple {
                                        subject: subj.clone(),
                                        predicate: pred.to_owned(),
                                        object: Term::Literal(LiteralValue::lang(
                                            s.to_owned(),
                                            lang.clone(),
                                        )),
                                    },
                                    limits,
                                )?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        (_, Value::Array(items)) => {
            for item in items {
                add_jsonld_property(subj, pred, kind, item, graph, limits, depth)?;
            }
            Ok(())
        }
        (_, Value::Object(obj)) if obj.contains_key("@value") => {
            let v = obj
                .get("@value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let lit = if let Some(lang) = obj.get("@language").and_then(Value::as_str) {
                LiteralValue::lang(v, lang)
            } else if let Some(dt) = obj.get("@type").and_then(Value::as_str) {
                LiteralValue::typed(v, expand_common_prefix(dt))
            } else {
                LiteralValue::plain(v)
            };
            graph.insert_checked(
                Triple {
                    subject: subj.clone(),
                    predicate: pred.to_owned(),
                    object: Term::Literal(lit),
                },
                limits,
            )
        }
        (_, Value::Object(_)) => {
            if let Some(child_subj) = parse_node_value(val, graph, limits, depth)? {
                graph.insert_checked(
                    Triple {
                        subject: subj.clone(),
                        predicate: pred.to_owned(),
                        object: child_subj.to_term(),
                    },
                    limits,
                )?;
            }
            Ok(())
        }
        (TermKind::Iri, Value::String(s)) => graph.insert_checked(
            Triple {
                subject: subj.clone(),
                predicate: pred.to_owned(),
                object: Term::Iri(s.clone()),
            },
            limits,
        ),
        (TermKind::TypedLiteral(dt), Value::String(s)) => graph.insert_checked(
            Triple {
                subject: subj.clone(),
                predicate: pred.to_owned(),
                object: Term::Literal(LiteralValue::typed(s.clone(), dt)),
            },
            limits,
        ),
        (_, Value::String(s)) => {
            let obj = if s.starts_with("http://")
                || s.starts_with("https://")
                || s.starts_with("mailto:")
            {
                Term::Iri(s.clone())
            } else {
                Term::Literal(LiteralValue::plain(s.clone()))
            };
            graph.insert_checked(
                Triple {
                    subject: subj.clone(),
                    predicate: pred.to_owned(),
                    object: obj,
                },
                limits,
            )
        }
        (_, Value::Number(n)) => {
            let dt = if n.is_f64() {
                ns::XSD_DECIMAL
            } else {
                ns::XSD_INTEGER
            };
            graph.insert_checked(
                Triple {
                    subject: subj.clone(),
                    predicate: pred.to_owned(),
                    object: Term::Literal(LiteralValue::typed(n.to_string(), dt)),
                },
                limits,
            )
        }
        (_, Value::Bool(b)) => graph.insert_checked(
            Triple {
                subject: subj.clone(),
                predicate: pred.to_owned(),
                object: Term::Literal(LiteralValue::typed(b.to_string(), ns::XSD_BOOLEAN)),
            },
            limits,
        ),
        _ => Ok(()),
    }
}

fn expand_common_prefix(term: &str) -> String {
    let prefixes = [
        ("rdf:", ns::RDF),
        ("rdfs:", ns::RDFS),
        ("xsd:", ns::XSD),
        ("dcat:", ns::DCAT),
        ("dcatap:", ns::DCATAP),
        ("dct:", ns::DCT),
        ("dcterms:", ns::DCT),
        ("foaf:", ns::FOAF),
        ("vcard:", ns::VCARD),
        ("adms:", ns::ADMS),
        ("skos:", ns::SKOS),
        ("spdx:", ns::SPDX),
        ("prov:", ns::PROV),
        ("hydra:", ns::HYDRA),
        ("eli:", ns::ELI),
        ("leg:", ns::SK_LEG),
        ("pu:", ns::CZ_PU),
    ];
    for (p, uri) in prefixes {
        if let Some(rest) = term.strip_prefix(p) {
            return format!("{uri}{rest}");
        }
    }
    term.to_owned()
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
        "@graph": nodes
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
