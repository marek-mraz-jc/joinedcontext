#![forbid(unsafe_code)]
//! RDF/XML (`.rdf`, `.xml`) serializer and parser for DCAT-AP graphs (C7).

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use dataroom_core::{DataroomError, Result};
use std::collections::BTreeMap;

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

/// Parses an RDF/XML document produced by `serialize_rdfxml` or standard `<rdf:Description>` / typed elements.
pub fn parse_rdfxml(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    if input.len() > limits.max_bytes {
        return Err(DataroomError::Rdf(format!(
            "RDF/XML input size {} exceeds limit of {} bytes",
            input.len(),
            limits.max_bytes
        )));
    }
    let mut graph = RdfGraph::new();
    let prefixes = BTreeMap::from([
        ("rdf", ns::RDF),
        ("rdfs", ns::RDFS),
        ("dcat", ns::DCAT),
        ("dcatap", ns::DCATAP),
        ("dct", ns::DCT),
        ("dcterms", ns::DCT),
        ("foaf", ns::FOAF),
        ("vcard", ns::VCARD),
        ("vcard2006", ns::VCARD),
        ("adms", ns::ADMS),
        ("skos", ns::SKOS),
        ("spdx", ns::SPDX),
        ("hydra", ns::HYDRA),
        ("eli", ns::ELI),
        ("pu", ns::CZ_PU),
        ("leg", ns::SK_LEG),
        ("xsd", ns::XSD),
    ]);

    let expand_qname = |qname: &str, local_ns: Option<&str>| -> String {
        if let Some((p, local)) = qname.split_once(':') {
            if p == "ns0" {
                if let Some(ns_uri) = local_ns {
                    return format!("{ns_uri}{local}");
                }
            }
            if let Some(ns_uri) = prefixes.get(p) {
                return format!("{ns_uri}{local}");
            }
        }
        qname.to_owned()
    };

    for desc_block in input.split("<rdf:Description").skip(1) {
        let Some((attrs_part, rest)) = desc_block.split_once('>') else {
            continue;
        };
        let body = rest.split("</rdf:Description>").next().unwrap_or(rest);
        let subj = if let Some(about) = extract_xml_attr(attrs_part, "rdf:about") {
            SubjectNode::Named(xml_unescape(&about))
        } else if let Some(node_id) = extract_xml_attr(attrs_part, "rdf:nodeID") {
            SubjectNode::Blank(xml_unescape(&node_id))
        } else {
            graph.fresh_blank_node()
        };

        for line in body.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('<') || trimmed.starts_with("</") {
                continue;
            }
            let without_lt = &trimmed[1..];
            let tag_end = without_lt
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .unwrap_or(without_lt.len());
            let tag_name = &without_lt[..tag_end];
            let local_ns = extract_xml_attr(without_lt, "xmlns:ns0");
            let pred = expand_qname(tag_name, local_ns.as_deref());

            if trimmed.ends_with("/>") {
                if let Some(res) = extract_xml_attr(without_lt, "rdf:resource") {
                    graph.insert_checked(
                        Triple {
                            subject: subj.clone(),
                            predicate: pred,
                            object: Term::Iri(xml_unescape(&res)),
                        },
                        limits,
                    )?;
                } else if let Some(nid) = extract_xml_attr(without_lt, "rdf:nodeID") {
                    graph.insert_checked(
                        Triple {
                            subject: subj.clone(),
                            predicate: pred,
                            object: Term::Blank(xml_unescape(&nid)),
                        },
                        limits,
                    )?;
                }
            } else if let Some((_, after_gt)) = without_lt.split_once('>') {
                let text_val = after_gt
                    .rsplit_once("</")
                    .map(|(v, _)| v)
                    .unwrap_or(after_gt);
                let unescaped = xml_unescape(text_val);
                let lit = if let Some(lang) = extract_xml_attr(without_lt, "xml:lang") {
                    LiteralValue::lang(unescaped, xml_unescape(&lang))
                } else if let Some(dt) = extract_xml_attr(without_lt, "rdf:datatype") {
                    LiteralValue::typed(unescaped, xml_unescape(&dt))
                } else {
                    LiteralValue::plain(unescaped)
                };
                graph.insert_checked(
                    Triple {
                        subject: subj.clone(),
                        predicate: pred,
                        object: Term::Literal(lit),
                    },
                    limits,
                )?;
            }
        }
    }

    Ok(graph)
}

fn extract_xml_attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
