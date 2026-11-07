#![forbid(unsafe_code)]
//! Turtle (`.ttl`) and N-Triples (`.nt`) parser (backed by `oxttl`) and serializer with
//! streaming resource limits (A1).

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use dataroom_core::{DataroomError, Result};
use oxttl::{NTriplesParser, TurtleParser};
use std::collections::BTreeMap;

/// Checks Turtle / N-Triples bracket and collection nesting depth (`[ ... ]`, `( ... )`) before
/// or during parsing so deeply nested blank-node lists are refused (A1).
pub fn check_turtle_nesting_depth(bytes: &[u8], max_depth: usize) -> Result<()> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut in_long_string = false;
    let mut string_delim = b'"';
    let mut in_iri = false;
    let mut in_comment = false;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if in_comment {
            if b == b'\n' {
                in_comment = false;
            }
            i += 1;
            continue;
        }
        if in_long_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if i + 2 < bytes.len()
                && bytes[i] == string_delim
                && bytes[i + 1] == string_delim
                && bytes[i + 2] == string_delim
            {
                in_long_string = false;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        if in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == string_delim {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if in_iri {
            if b == b'>' {
                in_iri = false;
            }
            i += 1;
            continue;
        }

        match b {
            b'#' => {
                in_comment = true;
            }
            b'<' => {
                in_iri = true;
            }
            b'"' | b'\'' => {
                string_delim = b;
                if i + 2 < bytes.len() && bytes[i + 1] == b && bytes[i + 2] == b {
                    in_long_string = true;
                    i += 3;
                    continue;
                }
                in_string = true;
            }
            b'[' | b'(' => {
                depth += 1;
                if depth > max_depth {
                    return Err(DataroomError::RdfLimitExceeded {
                        kind: "nesting-depth".into(),
                        message: format!(
                            "Turtle nesting depth {depth} exceeds maximum of {max_depth}"
                        ),
                    });
                }
            }
            b']' | b')' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// Converts an `oxrdf::Triple` into a `dataroom_rdf::Triple`.
pub(crate) fn from_ox_triple(t: oxrdf::Triple) -> Triple {
    let subject = match t.subject {
        oxrdf::Subject::NamedNode(n) => SubjectNode::Named(n.into_string()),
        oxrdf::Subject::BlankNode(b) => SubjectNode::Blank(b.into_string()),
    };
    let predicate = t.predicate.into_string();
    let object = match t.object {
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
    Triple {
        subject,
        predicate,
        object,
    }
}

/// Parses a Turtle document using `oxttl::TurtleParser`, enforcing [`RdfLimits`] on bytes,
/// nesting depth, literal length and triple count while parsing (refuse, never truncate, A1).
pub fn parse_turtle_bytes(bytes: &[u8], limits: &RdfLimits) -> Result<RdfGraph> {
    if bytes.len() > limits.max_bytes {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "bytes".into(),
            message: format!(
                "RDF input size {} exceeds maximum of {} bytes",
                bytes.len(),
                limits.max_bytes
            ),
        });
    }
    check_turtle_nesting_depth(bytes, limits.max_depth)?;

    let parser = TurtleParser::new()
        .with_base_iri("https://dataroom.local/")
        .map_err(|e| DataroomError::Rdf(format!("base IRI error: {e}")))?;

    let mut graph = RdfGraph::new();
    for item in parser.for_slice(bytes) {
        let ox_triple = item.map_err(|e| DataroomError::Rdf(format!("Turtle syntax error: {e}")))?;
        graph.insert_checked(from_ox_triple(ox_triple), limits)?;
    }
    Ok(graph)
}

/// Parses an N-Triples document using `oxttl::NTriplesParser`, enforcing [`RdfLimits`] (A1).
pub fn parse_ntriples_bytes(bytes: &[u8], limits: &RdfLimits) -> Result<RdfGraph> {
    if bytes.len() > limits.max_bytes {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "bytes".into(),
            message: format!(
                "RDF input size {} exceeds maximum of {} bytes",
                bytes.len(),
                limits.max_bytes
            ),
        });
    }
    let mut graph = RdfGraph::new();
    for item in NTriplesParser::new().for_slice(bytes) {
        let ox_triple =
            item.map_err(|e| DataroomError::Rdf(format!("N-Triples syntax error: {e}")))?;
        graph.insert_checked(from_ox_triple(ox_triple), limits)?;
    }
    Ok(graph)
}

/// Parses a Turtle string into an [`RdfGraph`], enforcing [`RdfLimits`].
pub fn parse_turtle(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    parse_turtle_bytes(input.as_bytes(), limits)
}

/// Alias kept for existing callers.
pub fn parse_turtle_checked(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    parse_turtle_bytes(input.as_bytes(), limits)
}

/// Serializes an [`RdfGraph`] as Turtle (`.ttl`).
pub fn serialize_turtle(graph: &RdfGraph) -> String {
    let prefixes = [
        ("adms", ns::ADMS),
        ("dcat", ns::DCAT),
        ("dcatap", ns::DCATAP),
        ("dct", ns::DCT),
        ("eli", ns::ELI),
        ("foaf", ns::FOAF),
        ("hydra", ns::HYDRA),
        ("leg", ns::SK_LEG),
        ("locn", ns::LOCN),
        ("odrl", ns::ODRL),
        ("prov", ns::PROV),
        ("pu", ns::CZ_PU),
        ("rdf", ns::RDF),
        ("rdfs", ns::RDFS),
        ("sh", ns::SHACL),
        ("skos", ns::SKOS),
        ("spdx", ns::SPDX),
        ("time", ns::TIME),
        ("vcard", ns::VCARD),
        ("xsd", ns::XSD),
    ];

    let mut out = String::new();
    for (p, uri) in prefixes {
        out.push_str(&format!("@prefix {p}: <{uri}> .\n"));
    }
    out.push('\n');

    let mut by_subject: BTreeMap<&SubjectNode, BTreeMap<&str, Vec<&Term>>> = BTreeMap::new();
    for triple in &graph.triples {
        by_subject
            .entry(&triple.subject)
            .or_default()
            .entry(triple.predicate.as_str())
            .or_default()
            .push(&triple.object);
    }

    let mut subjects: Vec<&SubjectNode> = by_subject.keys().copied().collect();
    subjects.sort_by(|a, b| match (a, b) {
        (SubjectNode::Named(x), SubjectNode::Named(y)) => x.cmp(y),
        (SubjectNode::Named(_), SubjectNode::Blank(_)) => std::cmp::Ordering::Less,
        (SubjectNode::Blank(_), SubjectNode::Named(_)) => std::cmp::Ordering::Greater,
        (SubjectNode::Blank(x), SubjectNode::Blank(y)) => x.cmp(y),
    });

    for subj in subjects {
        let preds = &by_subject[subj];
        out.push_str(&format_subject(subj));
        let mut pred_entries: Vec<(&str, &Vec<&Term>)> =
            preds.iter().map(|(k, v)| (*k, v)).collect();
        pred_entries.sort_by(|(pa, _), (pb, _)| {
            if *pa == ns::RDF_TYPE && *pb != ns::RDF_TYPE {
                std::cmp::Ordering::Less
            } else if *pa != ns::RDF_TYPE && *pb == ns::RDF_TYPE {
                std::cmp::Ordering::Greater
            } else {
                pa.cmp(pb)
            }
        });

        for (i, (pred, objs)) in pred_entries.iter().enumerate() {
            if i == 0 {
                out.push(' ');
            } else {
                out.push_str(" ;\n  ");
            }
            if *pred == ns::RDF_TYPE {
                out.push_str("a ");
            } else {
                out.push_str(&format_iri(pred, &prefixes));
                out.push(' ');
            }
            let formatted_objs: Vec<String> =
                objs.iter().map(|o| format_term(o, &prefixes)).collect();
            out.push_str(&formatted_objs.join(", "));
        }
        out.push_str(" .\n\n");
    }

    out
}

/// Serializes an [`RdfGraph`] as N-Triples (`.nt`).
pub fn serialize_ntriples(graph: &RdfGraph) -> String {
    let mut out = String::new();
    for t in &graph.triples {
        let s = match &t.subject {
            SubjectNode::Named(iri) => format!("<{iri}>"),
            SubjectNode::Blank(id) => format!("_:{id}"),
        };
        let p = format!("<{}>", t.predicate);
        let o = match &t.object {
            Term::Iri(iri) => format!("<{iri}>"),
            Term::Blank(id) => format!("_:{id}"),
            Term::Literal(lit) => {
                let esc = escape_turtle_string(&lit.value);
                if let Some(lang) = &lit.language {
                    format!("\"{esc}\"@{lang}")
                } else if lit.datatype == ns::XSD_STRING {
                    format!("\"{esc}\"")
                } else {
                    format!("\"{esc}\"^^<{}>", lit.datatype)
                }
            }
        };
        out.push_str(&format!("{s} {p} {o} .\n"));
    }
    out
}

fn format_subject(subj: &SubjectNode) -> String {
    match subj {
        SubjectNode::Named(iri) => format!("<{iri}>"),
        SubjectNode::Blank(id) => format!("_:{id}"),
    }
}

fn format_iri(iri: &str, prefixes: &[(&str, &str)]) -> String {
    for (prefix, ns_uri) in prefixes {
        if let Some(local) = iri.strip_prefix(ns_uri) {
            if !local.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
                && !local.starts_with('-')
                && !local.ends_with('-')
            {
                return format!("{prefix}:{local}");
            }
        }
    }
    format!("<{iri}>")
}

fn format_term(term: &Term, prefixes: &[(&str, &str)]) -> String {
    match term {
        Term::Iri(iri) => format_iri(iri, prefixes),
        Term::Blank(id) => format!("_:{id}"),
        Term::Literal(lit) => {
            let esc = escape_turtle_string(&lit.value);
            if let Some(lang) = &lit.language {
                format!("\"{esc}\"@{lang}")
            } else if lit.datatype == ns::XSD_STRING {
                format!("\"{esc}\"")
            } else {
                format!("\"{esc}\"^^{}", format_iri(&lit.datatype, prefixes))
            }
        }
    }
}

pub fn escape_turtle_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cz_and_sk_example_turtle_files() {
        let cz_ds = include_str!("../../specs/dcat-ap-cz/priklady-katalog/datova-sada.ttl");
        let g = parse_turtle_checked(cz_ds, &RdfLimits::default()).expect("parse cz dataset");
        assert!(!g.instances_of("http://www.w3.org/ns/dcat#Dataset").is_empty());

        let sk_ds = include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl");
        let g2 = parse_turtle_checked(sk_ds, &RdfLimits::default()).expect("parse sk dataset");
        assert!(!g2.instances_of("http://www.w3.org/ns/dcat#Dataset").is_empty());
    }
}
