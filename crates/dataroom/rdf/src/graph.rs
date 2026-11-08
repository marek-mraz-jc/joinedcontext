#![forbid(unsafe_code)]
//! In-memory RDF graph, terms, namespaces and parser safety limits (C8, C17).

use dataroom_core::{DataroomError, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

/// Standard RDF, DCAT-AP, SHACL, CZ and SK namespace constants.
pub mod ns {
    pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
    pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    pub const OWL: &str = "http://www.w3.org/2002/07/owl#";
    pub const SHACL: &str = "http://www.w3.org/ns/shacl#";
    pub const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
    pub const DCAT: &str = "http://www.w3.org/ns/dcat#";
    pub const DCT: &str = "http://purl.org/dc/terms/";
    pub const DCATAP: &str = "http://data.europa.eu/r5r/";
    pub const FOAF: &str = "http://xmlns.com/foaf/0.1/";
    pub const VCARD: &str = "http://www.w3.org/2006/vcard/ns#";
    pub const ADMS: &str = "http://www.w3.org/ns/adms#";
    pub const PROV: &str = "http://www.w3.org/ns/prov#";
    pub const SPDX: &str = "http://spdx.org/rdf/terms#";
    pub const LOCN: &str = "http://www.w3.org/ns/locn#";
    pub const ELI: &str = "http://data.europa.eu/eli/ontology#";
    pub const ODRL: &str = "http://www.w3.org/ns/odrl/2/";
    pub const TIME: &str = "http://www.w3.org/2006/time#";
    pub const HYDRA: &str = "http://www.w3.org/ns/hydra/core#";
    pub const SCHEMA: &str = "http://schema.org/";
    pub const CZ_PU: &str = "https://data.gov.cz/slovník/podmínky-užití/";
    pub const SK_LEG: &str = "https://data.gov.sk/def/ontology/legislation/";

    pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    pub const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    pub const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    pub const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
    pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
    pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
    pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
    pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
    pub const XSD_NON_NEGATIVE_INTEGER: &str =
        "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";
    pub const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";
    pub const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
    pub const XSD_DATE: &str = "http://www.w3.org/2001/XMLSchema#date";
    pub const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
    pub const XSD_DURATION: &str = "http://www.w3.org/2001/XMLSchema#duration";
    pub const XSD_HEX_BINARY: &str = "http://www.w3.org/2001/XMLSchema#hexBinary";
}

/// Safety limits for RDF parsing on uploads and harvests (A1, 00-contract.md Security).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RdfLimits {
    pub max_bytes: usize,
    pub max_triples: usize,
    pub max_depth: usize,
    pub max_literal_length: usize,
    pub forbid_owl_imports: bool,
}

impl Default for RdfLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_triples: 100_000,
            max_depth: 64,
            max_literal_length: 65_536,
            forbid_owl_imports: false,
        }
    }
}

impl RdfLimits {
    /// Strict limits for uploaded SHACL profile shapes (C17: no `owl:imports` remote fetch).
    pub fn uploaded_shapes() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024,
            max_triples: 50_000,
            max_depth: 32,
            max_literal_length: 32_768,
            forbid_owl_imports: true,
        }
    }
}

/// An IRI as RDF syntax may write it: characters no IRI may hold (controls, space and
/// `<>"{}|^`\`) are percent-encoded. Dataset URL fields are free text; written raw, a `>` ended
/// the IRI and the rest of the field wrote triples of its own into the feed.
pub fn iri_text(iri: &str) -> Cow<'_, str> {
    let forbidden = |c: char| {
        c.is_control()
            || matches!(
                c,
                ' ' | '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\'
            )
    };
    if !iri.contains(forbidden) {
        return Cow::Borrowed(iri);
    }
    let mut out = String::with_capacity(iri.len() + 16);
    for c in iri.chars() {
        if forbidden(c) {
            for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// An RDF subject node (IRI or blank node).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SubjectNode {
    Named(String),
    Blank(String),
}

impl SubjectNode {
    pub fn as_iri(&self) -> Option<&str> {
        match self {
            Self::Named(iri) => Some(iri),
            Self::Blank(_) => None,
        }
    }

    pub fn to_term(&self) -> Term {
        match self {
            Self::Named(iri) => Term::Iri(iri.clone()),
            Self::Blank(id) => Term::Blank(id.clone()),
        }
    }
}

impl fmt::Display for SubjectNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(iri) => write!(f, "<{iri}>"),
            Self::Blank(id) => write!(f, "_:{id}"),
        }
    }
}

/// An RDF literal with value, optional language tag and datatype IRI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LiteralValue {
    pub value: String,
    pub language: Option<String>,
    pub datatype: String,
}

impl LiteralValue {
    pub fn plain(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            language: None,
            datatype: ns::XSD_STRING.to_owned(),
        }
    }

    pub fn lang(value: impl Into<String>, lang: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            language: Some(lang.into().to_ascii_lowercase()),
            datatype: ns::RDF_LANG_STRING.to_owned(),
        }
    }

    pub fn typed(value: impl Into<String>, datatype: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            language: None,
            datatype: datatype.into(),
        }
    }
}

/// An RDF object term (IRI, blank node, or literal).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Term {
    Iri(String),
    Blank(String),
    Literal(LiteralValue),
}

impl Term {
    pub fn as_subject(&self) -> Option<SubjectNode> {
        match self {
            Self::Iri(i) => Some(SubjectNode::Named(i.clone())),
            Self::Blank(b) => Some(SubjectNode::Blank(b.clone())),
            Self::Literal(_) => None,
        }
    }

    pub fn as_iri(&self) -> Option<&str> {
        match self {
            Self::Iri(i) => Some(i),
            _ => None,
        }
    }

    pub fn as_literal(&self) -> Option<&LiteralValue> {
        match self {
            Self::Literal(l) => Some(l),
            _ => None,
        }
    }

    pub fn string_value(&self) -> &str {
        match self {
            Self::Iri(s) | Self::Blank(s) => s,
            Self::Literal(l) => &l.value,
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Iri(iri) => write!(f, "<{iri}>"),
            Self::Blank(id) => write!(f, "_:{id}"),
            Self::Literal(lit) => {
                if let Some(lang) = &lit.language {
                    write!(f, "\"{}\"@{}", lit.value, lang)
                } else if lit.datatype == ns::XSD_STRING {
                    write!(f, "\"{}\"", lit.value)
                } else {
                    write!(f, "\"{}\"^^<{}>", lit.value, lit.datatype)
                }
            }
        }
    }
}

/// One RDF triple `(subject, predicate, object)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Triple {
    pub subject: SubjectNode,
    pub predicate: String,
    pub object: Term,
}

/// An in-memory RDF graph backed by an ordered set of [`Triple`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdfGraph {
    pub triples: BTreeSet<Triple>,
    bnode_counter: usize,
}

impl RdfGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.triples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }

    pub fn fresh_blank_node(&mut self) -> SubjectNode {
        self.bnode_counter += 1;
        SubjectNode::Blank(format!("b{}", self.bnode_counter))
    }

    pub fn insert(&mut self, subject: SubjectNode, predicate: impl Into<String>, object: Term) {
        self.triples.insert(Triple {
            subject,
            predicate: predicate.into(),
            object,
        });
    }

    pub fn insert_checked(&mut self, triple: Triple, limits: &RdfLimits) -> Result<()> {
        if limits.forbid_owl_imports && triple.predicate == "http://www.w3.org/2002/07/owl#imports"
        {
            return Err(DataroomError::RdfLimitExceeded {
                kind: "owl-imports".into(),
                message: "owl:imports is forbidden in uploaded shapes (C17)".into(),
            });
        }
        if let Term::Literal(ref lit) = triple.object {
            if lit.value.len() > limits.max_literal_length {
                return Err(DataroomError::RdfLimitExceeded {
                    kind: "literal-length".into(),
                    message: format!(
                        "RDF literal length {} exceeds maximum of {} bytes",
                        lit.value.len(),
                        limits.max_literal_length
                    ),
                });
            }
        }
        if self.triples.len() >= limits.max_triples {
            return Err(DataroomError::RdfLimitExceeded {
                kind: "triples".into(),
                message: format!(
                    "RDF graph exceeded maximum of {} triples",
                    limits.max_triples
                ),
            });
        }
        self.triples.insert(triple);
        Ok(())
    }

    pub fn merge(&mut self, other: &RdfGraph) {
        for t in &other.triples {
            self.triples.insert(t.clone());
        }
    }

    /// Returns all subjects having `predicate == object`.
    pub fn subjects_with(&self, predicate: &str, object: &Term) -> Vec<SubjectNode> {
        self.triples
            .iter()
            .filter(|t| t.predicate == predicate && &t.object == object)
            .map(|t| t.subject.clone())
            .collect()
    }

    /// Returns all subjects of `rdf:type == class_iri`.
    pub fn instances_of(&self, class_iri: &str) -> Vec<SubjectNode> {
        self.subjects_with(ns::RDF_TYPE, &Term::Iri(class_iri.to_owned()))
    }

    /// Returns all objects for `(subject, predicate)`.
    pub fn objects_of(&self, subject: &SubjectNode, predicate: &str) -> Vec<Term> {
        self.triples
            .iter()
            .filter(|t| &t.subject == subject && t.predicate == predicate)
            .map(|t| t.object.clone())
            .collect()
    }

    /// Returns the first object for `(subject, predicate)`.
    pub fn first_object(&self, subject: &SubjectNode, predicate: &str) -> Option<Term> {
        self.triples
            .iter()
            .find(|t| &t.subject == subject && t.predicate == predicate)
            .map(|t| t.object.clone())
    }

    /// Returns all subjects where `(?, predicate, target)` holds (inverse path).
    pub fn subjects_for_object(&self, predicate: &str, target: &Term) -> Vec<SubjectNode> {
        self.triples
            .iter()
            .filter(|t| t.predicate == predicate && &t.object == target)
            .map(|t| t.subject.clone())
            .collect()
    }

    /// Extracts an RDF list (`rdf:first` / `rdf:rest` / `rdf:nil`) starting at `head`.
    pub fn rdf_list(&self, head: &Term) -> Vec<Term> {
        let mut items = Vec::new();
        let mut current = head.clone();
        for _ in 0..1024 {
            if let Term::Iri(iri) = &current {
                if iri == ns::RDF_NIL {
                    break;
                }
            }
            let Some(subj) = current.as_subject() else {
                break;
            };
            if let Some(first) = self.first_object(&subj, ns::RDF_FIRST) {
                items.push(first);
            } else {
                break;
            }
            if let Some(rest) = self.first_object(&subj, ns::RDF_REST) {
                current = rest;
            } else {
                break;
            }
        }
        items
    }
}
