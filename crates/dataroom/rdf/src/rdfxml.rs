#![forbid(unsafe_code)]
//! RDF/XML (`.rdf`, `.xml`) parser (backed by `oxrdfxml`) and serializer for DCAT-AP graphs (A1, C7).

use crate::graph::{ns, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use crate::turtle::from_ox_triple;
use dataroom_core::{DataroomError, Result};
use oxrdfxml::RdfXmlParser;
use std::collections::BTreeMap;

/// Checks XML element nesting depth before parsing so deeply nested RDF/XML trees are refused (A1).
fn check_xml_nesting_depth(bytes: &[u8], max_depth: usize) -> Result<()> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| DataroomError::Rdf(format!("invalid UTF-8 in RDF/XML: {e}")))?;
    let mut depth = 0usize;
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if rest.starts_with("!--") {
            if let Some(end) = rest.find("-->") {
                rest = &rest[end + 3..];
                continue;
            }
            break;
        }
        if rest.starts_with('?') || rest.starts_with('!') {
            if let Some(gt) = rest.find('>') {
                rest = &rest[gt + 1..];
                continue;
            }
            break;
        }
        let Some(gt) = rest.find('>') else {
            break;
        };
        let tag_body = rest[..gt].trim();
        if tag_body.starts_with('/') {
            depth = depth.saturating_sub(1);
        } else if !tag_body.ends_with('/') {
            depth += 1;
            if depth > max_depth {
                return Err(DataroomError::RdfLimitExceeded {
                    kind: "nesting-depth".into(),
                    message: format!(
                        "RDF/XML element nesting depth {depth} exceeds maximum of {max_depth}"
                    ),
                });
            }
        }
        rest = &rest[gt + 1..];
    }
    Ok(())
}

/// Parses an RDF/XML byte slice using `oxrdfxml::RdfXmlParser`, enforcing [`RdfLimits`] (A1).
pub fn parse_rdfxml_bytes(bytes: &[u8], limits: &RdfLimits) -> Result<RdfGraph> {
    if bytes.len() > limits.max_bytes {
        return Err(DataroomError::RdfLimitExceeded {
            kind: "bytes".into(),
            message: format!(
                "RDF/XML input size {} exceeds maximum of {} bytes",
                bytes.len(),
                limits.max_bytes
            ),
        });
    }
    check_xml_nesting_depth(bytes, limits.max_depth)?;

    let parser = RdfXmlParser::new()
        .with_base_iri("https://dataroom.local/")
        .map_err(|e| DataroomError::Rdf(format!("base IRI error: {e}")))?;

    let mut graph = RdfGraph::new();
    for item in parser.for_slice(bytes) {
        let ox_triple =
            item.map_err(|e| DataroomError::Rdf(format!("RDF/XML syntax error: {e}")))?;
        graph.insert_checked(from_ox_triple(ox_triple), limits)?;
    }
    Ok(graph)
}

/// Parses an RDF/XML string into an [`RdfGraph`], enforcing [`RdfLimits`].
pub fn parse_rdfxml(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    parse_rdfxml_bytes(input.as_bytes(), limits)
}

/// Serializes an [`RdfGraph`] into RDF/XML (`application/rdf+xml`).
pub fn serialize_rdfxml(graph: &RdfGraph) -> String {
    let prefixes = [
        ("rdf", ns::RDF),
        ("rdfs", ns::RDFS),
        ("dcat", ns::DCAT),
        ("dcatap", ns::DCATAP),
        ("dct", ns::DCT),
        ("foaf", ns::FOAF),
        ("vcard", ns::VCARD),
        ("adms", ns::ADMS),
        ("skos", ns::SKOS),
        ("spdx", ns::SPDX),
        ("hydra", ns::HYDRA),
        ("eli", ns::ELI),
        ("pu", ns::CZ_PU),
        ("leg", ns::SK_LEG),
        ("xsd", ns::XSD),
    ];

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<rdf:RDF");
    for (p, uri) in prefixes {
        out.push_str(&format!("\n  xmlns:{p}=\"{uri}\""));
    }
    out.push_str(">\n");

    let mut by_subject: BTreeMap<&SubjectNode, Vec<&Triple>> = BTreeMap::new();
    for t in &graph.triples {
        by_subject.entry(&t.subject).or_default().push(t);
    }

    for (subj, triples) in by_subject {
        out.push_str("  <rdf:Description");
        match subj {
            SubjectNode::Named(iri) => {
                out.push_str(&format!(" rdf:about=\"{}\">\n", xml_escape(iri)))
            }
            SubjectNode::Blank(id) => {
                out.push_str(&format!(" rdf:nodeID=\"{}\">\n", xml_escape(id)))
            }
        }
        for t in triples {
            let (tag, xmlns_extra) = split_qname(&t.predicate, &prefixes);
            out.push_str(&format!("    <{tag}{xmlns_extra}"));
            match &t.object {
                Term::Iri(iri) => {
                    out.push_str(&format!(" rdf:resource=\"{}\"/>\n", xml_escape(iri)));
                }
                Term::Blank(id) => {
                    out.push_str(&format!(" rdf:nodeID=\"{}\"/>\n", xml_escape(id)));
                }
                Term::Literal(lit) => {
                    if let Some(lang) = &lit.language {
                        out.push_str(&format!(
                            " xml:lang=\"{}\">{}</{tag}>\n",
                            xml_escape(lang),
                            xml_escape(&lit.value)
                        ));
                    } else if lit.datatype != ns::XSD_STRING {
                        out.push_str(&format!(
                            " rdf:datatype=\"{}\">{}</{tag}>\n",
                            xml_escape(&lit.datatype),
                            xml_escape(&lit.value)
                        ));
                    } else {
                        out.push_str(&format!(">{}</{tag}>\n", xml_escape(&lit.value)));
                    }
                }
            }
        }
        out.push_str("  </rdf:Description>\n");
    }

    out.push_str("</rdf:RDF>\n");
    out
}

fn split_qname(iri: &str, prefixes: &[(&str, &str)]) -> (String, String) {
    for (p, ns_uri) in prefixes {
        if let Some(local) = iri.strip_prefix(ns_uri) {
            if !local.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
            {
                return (format!("{p}:{local}"), String::new());
            }
        }
    }
    if let Some(pos) = iri.rfind(['/', '#']) {
        let (ns_part, local) = iri.split_at(pos + 1);
        if !local.is_empty() {
            return (
                format!("ns0:{local}"),
                format!(" xmlns:ns0=\"{}\"", xml_escape(ns_part)),
            );
        }
    }
    ("rdf:value".into(), String::new())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
