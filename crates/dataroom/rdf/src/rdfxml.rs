#![forbid(unsafe_code)]
//! RDF/XML (`.rdf`, `.xml`) parser (backed by `oxrdfxml`) and serializer for DCAT-AP graphs (A1, C7).

use crate::graph::{iri_text, ns, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use crate::turtle::from_ox_triple;
use dataroom_core::{DataroomError, Result};
use oxrdfxml::RdfXmlParser;
use std::collections::BTreeMap;

/// Most attributes one RDF/XML tag may carry. Kept after the quick-xml 0.42 port of
/// `vendor/oxrdfxml` fixed the quadratic duplicate check (RUSTSEC-2026-0194): a DCAT tag needs few.
pub const MAX_XML_ATTRIBUTES_PER_TAG: usize = 128;

/// Most namespace declarations one RDF/XML document may carry, a bound of its own beside
/// quick-xml's (RUSTSEC-2026-0195, fixed in the quick-xml 0.42 the vendored oxrdfxml uses).
pub const MAX_XML_NAMESPACE_DECLARATIONS: usize = 1024;

fn limit(kind: &str, message: String) -> DataroomError {
    DataroomError::RdfLimitExceeded {
        kind: kind.into(),
        message,
    }
}

/// Checks XML element nesting depth, attributes per tag, namespace declarations and entity
/// declarations before parsing, so a document that would exhaust the parser is refused (A1).
///
/// The scan follows XML's lexical structure: comments, CDATA sections and processing
/// instructions are skipped to their own terminators, and inside a tag quoted attribute values
/// are skipped whole (a `>` inside a value does not end the tag). Every `=` outside a value is
/// one attribute, whatever whitespace surrounds it.
fn check_xml_nesting_depth(bytes: &[u8], max_depth: usize) -> Result<()> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| DataroomError::Rdf(format!("invalid UTF-8 in RDF/XML: {e}")))?;
    let mut depth = 0usize;
    let mut namespace_declarations = 0usize;
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        let skip_to = |rest: &str, end: &str| rest.find(end).map(|i| i + end.len());
        if rest.starts_with("!--") {
            match skip_to(rest, "-->") {
                Some(i) => rest = &rest[i..],
                None => break,
            }
            continue;
        }
        if rest.starts_with("![CDATA[") {
            match skip_to(rest, "]]>") {
                Some(i) => rest = &rest[i..],
                None => break,
            }
            continue;
        }
        if rest.starts_with('?') {
            match skip_to(rest, "?>") {
                Some(i) => rest = &rest[i..],
                None => break,
            }
            continue;
        }
        if rest.starts_with('!') {
            // `<!DOCTYPE ...>`, maybe with an internal subset `[ ... ]>`. oxrdfxml expands the
            // entities declared there, nested ones included (an entity bomb allocates
            // gigabytes), and a DCAT feed has no use for them: entity declarations are refused.
            let Some(gt) = rest.find('>') else {
                break;
            };
            let end = if rest[..gt].contains('[') {
                match skip_to(rest, "]>") {
                    Some(i) => i,
                    None => break,
                }
            } else {
                gt + 1
            };
            if rest[..end].contains("<!ENTITY") {
                return Err(limit(
                    "entities",
                    "RDF/XML entity declarations are refused (entity expansion guard)".into(),
                ));
            }
            rest = &rest[end..];
            continue;
        }

        // A start, end or empty-element tag: walk it, skipping quoted values. `token` is the
        // name being read, `last` the one read before the next `=`.
        let mut attributes = 0usize;
        let mut token: Option<usize> = None;
        let mut last = "";
        let mut quote: Option<char> = None;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
                continue;
            }
            match c {
                '"' | '\'' => {
                    token = None;
                    quote = Some(c);
                }
                '>' => {
                    end = Some(i);
                    break;
                }
                '=' => {
                    if let Some(start) = token.take() {
                        last = &rest[start..i];
                    }
                    attributes += 1;
                    if attributes > MAX_XML_ATTRIBUTES_PER_TAG {
                        return Err(limit(
                            "attributes",
                            format!(
                                "RDF/XML tag with more than {MAX_XML_ATTRIBUTES_PER_TAG} attributes exceeds the maximum"
                            ),
                        ));
                    }
                    if last == "xmlns" || last.starts_with("xmlns:") {
                        namespace_declarations += 1;
                        if namespace_declarations > MAX_XML_NAMESPACE_DECLARATIONS {
                            return Err(limit(
                                "namespaces",
                                format!(
                                    "RDF/XML declares more than {MAX_XML_NAMESPACE_DECLARATIONS} namespaces"
                                ),
                            ));
                        }
                    }
                    last = "";
                }
                c if c.is_whitespace() => {
                    if let Some(start) = token.take() {
                        last = &rest[start..i];
                    }
                }
                _ => {
                    if token.is_none() {
                        token = Some(i);
                    }
                }
            }
        }
        let Some(end) = end else {
            break;
        };
        let tag_body = rest[..end].trim();
        if tag_body.starts_with('/') {
            depth = depth.saturating_sub(1);
        } else if !tag_body.ends_with('/') {
            depth += 1;
            if depth > max_depth {
                return Err(limit(
                    "nesting-depth",
                    format!("RDF/XML element nesting depth {depth} exceeds maximum of {max_depth}"),
                ));
            }
        }
        rest = &rest[end + 1..];
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
                out.push_str(&format!(" rdf:about=\"{}\">\n", xml_escape(&iri_text(iri))))
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
                    out.push_str(&format!(
                        " rdf:resource=\"{}\"/>\n",
                        xml_escape(&iri_text(iri))
                    ));
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
                            xml_escape(&iri_text(&lit.datatype)),
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
    let Some((ns_part, local)) = xml_name_split(iri) else {
        // ponytail: lossy for a predicate with no XML-name suffix (`…/m²`); no DCAT mapping
        // predicate has one. A Result from serialize_rdfxml would let it refuse instead.
        return ("rdf:value".into(), String::new());
    };
    match prefixes.iter().find(|(_, ns_uri)| *ns_uri == ns_part) {
        Some((p, _)) => (format!("{p}:{local}"), String::new()),
        None => (
            format!("ns0:{local}"),
            format!(" xmlns:ns0=\"{}\"", xml_escape(&iri_text(ns_part))),
        ),
    }
}

/// Splits an IRI into a namespace and its longest suffix that is an XML `NCName`, the only
/// form in which RDF/XML can write a predicate. A local part such as `1st` or `m²` is no XML
/// name, and writing it as an element name made the document ill-formed.
fn xml_name_split(iri: &str) -> Option<(&str, &str)> {
    let name_start = |c: char| crate::turtle::pn_chars_base(c) || c == '_';
    let name_char = |c: char| {
        name_start(c)
            || c.is_ascii_digit()
            || matches!(c, '-' | '.' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
    };
    let tail = iri
        .char_indices()
        .rev()
        .take_while(|(_, c)| name_char(*c))
        .last()?
        .0;
    let (start, _) = iri[tail..].char_indices().find(|(_, c)| name_start(*c))?;
    Some(iri.split_at(tail + start))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_rdfxml_refuses_attribute_and_namespace_floods() {
        let attrs: String = (0..=MAX_XML_ATTRIBUTES_PER_TAG)
            .map(|i| format!(" a{i}=\"x\""))
            .collect();
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description{attrs}/></rdf:RDF>"
        );
        let err = parse_rdfxml_bytes(doc.as_bytes(), &RdfLimits::default()).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "attributes"),
            "{err:?}"
        );

        let decls: String = (0..=MAX_XML_NAMESPACE_DECLARATIONS)
            .map(|i| format!("<x:e{i} xmlns:x=\"http://example.org/{i}#\"/>"))
            .collect();
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">{decls}</rdf:RDF>"
        );
        let err = parse_rdfxml_bytes(doc.as_bytes(), &RdfLimits::default()).unwrap_err();
        assert!(
            matches!(&err, DataroomError::RdfLimitExceeded { kind, .. } if kind == "namespaces"),
            "{err:?}"
        );

        let ok = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:dct="http://purl.org/dc/terms/"><rdf:Description rdf:about="https://example.org/ds"><dct:title>A</dct:title></rdf:Description></rdf:RDF>"#;
        let graph = parse_rdfxml_bytes(ok.as_bytes(), &RdfLimits::default()).unwrap();
        assert!(!graph.is_empty());
    }

    const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

    fn refused_kind(doc: &str) -> Option<String> {
        match parse_rdfxml_bytes(doc.as_bytes(), &RdfLimits::default()) {
            Err(DataroomError::RdfLimitExceeded { kind, .. }) => Some(kind),
            _ => None,
        }
    }

    #[test]
    fn a1_attribute_flood_with_spaces_around_equals_is_refused() {
        let attrs: String = (0..2000).map(|i| format!(" a{i} = \"x\"")).collect();
        let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><rdf:Description{attrs}/></rdf:RDF>");
        assert_eq!(refused_kind(&doc).as_deref(), Some("attributes"));
    }

    #[test]
    fn a1_attribute_flood_split_across_lines_is_refused() {
        let attrs: String = (0..2000).map(|i| format!("\n\ta{i}\n=\n'x'")).collect();
        let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><rdf:Description{attrs}/></rdf:RDF>");
        assert_eq!(refused_kind(&doc).as_deref(), Some("attributes"));
    }

    #[test]
    fn a1_attribute_flood_behind_a_gt_inside_a_value_is_refused() {
        // `>` is legal inside an attribute value; the attributes after it still count.
        let attrs: String = (0..2000).map(|i| format!(" a{i}=\"x\"")).collect();
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><rdf:Description rdf:about=\"https://e.org/>\"{attrs}/></rdf:RDF>"
        );
        assert_eq!(refused_kind(&doc).as_deref(), Some("attributes"));
    }

    #[test]
    fn a1_duplicate_attribute_names_flood_is_refused() {
        let attrs: String = (0..5000).map(|_| " a=\"x\"").collect();
        let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><rdf:Description{attrs}/></rdf:RDF>");
        assert_eq!(refused_kind(&doc).as_deref(), Some("attributes"));
    }

    #[test]
    fn a1_namespace_flood_on_one_tag_and_nested_tags_is_refused() {
        // Below the per-tag attribute cap on every tag, but over the document namespace cap.
        let mut doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\">");
        for i in 0..20 {
            let decls: String = (0..100)
                .map(|j| format!(" xmlns:n{i}x{j}=\"http://example.org/{i}/{j}#\""))
                .collect();
            doc.push_str(&format!("<rdf:Description{decls}>"));
        }
        for _ in 0..20 {
            doc.push_str("</rdf:Description>");
        }
        doc.push_str("</rdf:RDF>");
        assert_eq!(refused_kind(&doc).as_deref(), Some("namespaces"));

        // Default-namespace declarations (`xmlns=`) count as well.
        let decls: String = (0..=MAX_XML_NAMESPACE_DECLARATIONS)
            .map(|i| format!("<e{i} xmlns = \"http://example.org/{i}#\"/>"))
            .collect();
        let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\">{decls}</rdf:RDF>");
        assert_eq!(refused_kind(&doc).as_deref(), Some("namespaces"));
    }

    #[test]
    fn a1_cdata_and_comments_neither_hide_nor_invent_tags() {
        // Text that looks like an attribute flood inside a CDATA section or a comment is text.
        let fake: String = (0..2000).map(|i| format!(" a{i}=\"x\"")).collect();
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF_NS}\" xmlns:dct=\"http://purl.org/dc/terms/\">\
             <!-- <x{fake}> -->\
             <rdf:Description rdf:about=\"https://e.org/ds\">\
             <dct:title><![CDATA[ > <y{fake}/> ]]></dct:title>\
             </rdf:Description></rdf:RDF>"
        );
        let g = parse_rdfxml_bytes(doc.as_bytes(), &RdfLimits::default()).expect("valid document");
        assert_eq!(g.len(), 1);
        assert!(g
            .triples
            .iter()
            .next()
            .unwrap()
            .object
            .string_value()
            .contains("<y a0="));

        // A comment or CDATA section does not hide a real flood after it.
        let attrs: String = (0..2000).map(|i| format!(" a{i}=\"x\"")).collect();
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><!-- > --><![CDATA[ > ]]><rdf:Description{attrs}/></rdf:RDF>"
        );
        assert_eq!(refused_kind(&doc).as_deref(), Some("attributes"));
    }

    #[test]
    fn a1_nesting_depth_and_size_caps() {
        let open: String = (0..100).map(|_| "<rdf:Description><rdf:value>").collect();
        let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\">{open}");
        assert_eq!(refused_kind(&doc).as_deref(), Some("nesting-depth"));

        let small = RdfLimits {
            max_bytes: 10,
            ..RdfLimits::default()
        };
        assert!(matches!(
            parse_rdfxml(&format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\"/>"), &small),
            Err(DataroomError::RdfLimitExceeded { kind, .. }) if kind == "bytes"
        ));
    }

    #[test]
    fn a1_entity_expansion_bomb_is_refused() {
        let mut dtd = String::from("<!DOCTYPE rdf:RDF [<!ENTITY l0 \"lol\">");
        for i in 1..12 {
            let refs: String = (0..10).map(|_| format!("&l{};", i - 1)).collect();
            dtd.push_str(&format!("<!ENTITY l{i} \"{refs}\">"));
        }
        dtd.push(']');
        dtd.push('>');
        let doc = format!(
            "{dtd}<rdf:RDF xmlns:rdf=\"{RDF_NS}\" xmlns:dct=\"http://purl.org/dc/terms/\">\
             <rdf:Description rdf:about=\"https://e.org/ds\"><dct:title>&l11;</dct:title>\
             </rdf:Description></rdf:RDF>"
        );
        assert_eq!(refused_kind(&doc).as_deref(), Some("entities"));

        // A doctype without an internal subset, or with one but no entities, is accepted.
        for doctype in [
            "<!DOCTYPE rdf:RDF>".to_owned(),
            "<!DOCTYPE rdf:RDF [<!-- nothing declared -->]>".to_owned(),
        ] {
            let doc = format!(
                "<?xml version=\"1.0\"?>{doctype}<rdf:RDF xmlns:rdf=\"{RDF_NS}\" xmlns:dct=\"http://purl.org/dc/terms/\">\
                 <rdf:Description rdf:about=\"https://e.org/ds\"><dct:title>A</dct:title>\
                 </rdf:Description></rdf:RDF>"
            );
            let g = parse_rdfxml(&doc, &RdfLimits::default()).expect(&doctype);
            assert_eq!(g.len(), 1);
        }
    }

    #[test]
    fn unterminated_markup_reaches_the_parser_as_a_syntax_error() {
        for tail in [
            "<!-- open comment",
            "<![CDATA[ open",
            "<?pi open",
            "<!DOCTYPE x [ <!-- open",
            "<!DOCTYPE x",
            "<rdf:Description rdf:about=\"x",
        ] {
            let doc = format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\">{tail}");
            assert!(
                matches!(
                    parse_rdfxml(&doc, &RdfLimits::default()),
                    Err(DataroomError::Rdf(_))
                ),
                "{tail}"
            );
        }
    }

    /// The quick-xml 0.42 port of `vendor/oxrdfxml` reads text, entity references and CDATA as
    /// separate events: one property's text comes back whole, references resolved.
    #[test]
    fn text_entities_and_cdata_make_one_literal() {
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF_NS}\" xmlns:dct=\"http://purl.org/dc/terms/\">\
             <rdf:Description rdf:about=\"https://e.org/ds?a=1&amp;b=2\">\
             <dct:title xml:lang=\"sk\">Ovzdu&#353;ie &amp; <![CDATA[<voda>]]> &#x1F4A7;</dct:title>\
             </rdf:Description></rdf:RDF>"
        );
        let graph = parse_rdfxml(&doc, &RdfLimits::default()).unwrap();
        let [triple] = graph.triples.iter().collect::<Vec<_>>()[..] else {
            panic!("one triple expected: {:?}", graph.triples);
        };
        assert_eq!(triple.subject, SubjectNode::Named("https://e.org/ds?a=1&b=2".into()));
        let Term::Literal(literal) = &triple.object else {
            panic!("a literal expected: {:?}", triple.object);
        };
        assert_eq!(literal.value, "Ovzdušie & <voda> 💧");
        assert_eq!(literal.language.as_deref(), Some("sk"));
    }

    #[test]
    fn invalid_utf8_and_syntax_errors_are_rdf_errors() {
        assert!(matches!(
            parse_rdfxml_bytes(b"<rdf:RDF \xff\xfe>", &RdfLimits::default()),
            Err(DataroomError::Rdf(m)) if m.contains("UTF-8")
        ));
        assert!(matches!(
            parse_rdfxml(&format!("<rdf:RDF xmlns:rdf=\"{RDF_NS}\"><rdf:Description></rdf:RDF>"), &RdfLimits::default()),
            Err(DataroomError::Rdf(m)) if m.contains("RDF/XML syntax error")
        ));
        // A limit on triples applies to RDF/XML too.
        let one = RdfLimits {
            max_triples: 1,
            ..RdfLimits::default()
        };
        let doc = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF_NS}\" xmlns:dct=\"http://purl.org/dc/terms/\">\
             <rdf:Description rdf:about=\"https://e.org/ds\"><dct:title>A</dct:title><dct:title>B</dct:title>\
             </rdf:Description></rdf:RDF>"
        );
        assert!(matches!(
            parse_rdfxml(&doc, &one),
            Err(DataroomError::RdfLimitExceeded { kind, .. }) if kind == "triples"
        ));
    }

    #[test]
    fn serialize_round_trips_every_term_kind() {
        let mut g = RdfGraph::new();
        let ds = SubjectNode::Named("https://e.org/ds?a=1&b=2".into());
        let b = SubjectNode::Blank("b1".into());
        g.insert(
            ds.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Dataset", ns::DCAT)),
        );
        g.insert(
            ds.clone(),
            format!("{}title", ns::DCT),
            Term::Literal(crate::graph::LiteralValue::lang("Ovzdušie \"&\" <x>", "sk")),
        );
        g.insert(
            ds.clone(),
            format!("{}description", ns::DCT),
            Term::Literal(crate::graph::LiteralValue::plain("it's")),
        );
        g.insert(
            ds.clone(),
            format!("{}issued", ns::DCT),
            Term::Literal(crate::graph::LiteralValue::typed(
                "2026-01-01",
                ns::XSD_DATE,
            )),
        );
        g.insert(ds.clone(), format!("{}contactPoint", ns::DCAT), b.to_term());
        g.insert(
            b.clone(),
            "https://other.example/vocab#prop",
            Term::Literal(crate::graph::LiteralValue::plain("x")),
        );
        g.insert(
            b.clone(),
            "https://other.example/vocab/",
            Term::Iri("https://e.org/o".into()),
        );
        g.insert(b.clone(), "urn:nolocal", Term::Blank("b2".into()));
        let xml = serialize_rdfxml(&g);
        assert!(xml.contains("rdf:nodeID=\"b1\""));
        assert!(xml.contains("xmlns:ns0=\"https://other.example/vocab#\""));
        assert!(xml.contains("<rdf:value"), "{xml}");
        let back = parse_rdfxml(&xml, &RdfLimits::default()).expect("serialized RDF/XML parses");
        let titles: Vec<_> = back
            .triples
            .iter()
            .filter(|t| t.predicate == format!("{}title", ns::DCT))
            .collect();
        assert_eq!(titles[0].object.string_value(), "Ovzdušie \"&\" <x>");
        assert!(back
            .triples
            .iter()
            .any(|t| t.subject.as_iri() == Some("https://e.org/ds?a=1&b=2")));
        assert!(back.triples.iter().any(|t| t.object
            == Term::Literal(crate::graph::LiteralValue::typed(
                "2026-01-01",
                ns::XSD_DATE
            ))));
        assert!(back
            .triples
            .iter()
            .any(|t| t.object.string_value() == "it's"));
    }

    #[test]
    fn serialize_writes_only_valid_xml_names() {
        // Predicates whose local part is no XML name under a known prefix (a leading digit, a
        // superscript, a micro sign) or under no known prefix at all: each must still come out
        // as well-formed RDF/XML that parses back to the same predicate.
        let predicates = [
            format!("{}1st", ns::DCT),
            format!("{}µg", ns::DCAT),
            format!("{}ľúbivý", ns::DCT),
            format!("{}a/b2", ns::DCT),
            "https://other.example/v#2026-value".to_owned(),
            "urn:example:nolocal".to_owned(),
        ];
        let mut g = RdfGraph::new();
        let s = SubjectNode::Named("https://e.org/s".into());
        for p in &predicates {
            g.insert(
                s.clone(),
                p.clone(),
                Term::Literal(crate::graph::LiteralValue::plain("v")),
            );
        }
        let xml = serialize_rdfxml(&g);
        let back =
            parse_rdfxml(&xml, &RdfLimits::default()).unwrap_or_else(|e| panic!("{e}\n{xml}"));
        let mut got: Vec<&str> = back.triples.iter().map(|t| t.predicate.as_str()).collect();
        let mut want: Vec<&str> = predicates.iter().map(String::as_str).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "{xml}");
        assert!(
            xml.contains("<dct:ľúbivý"),
            "letters keep the known prefix: {xml}"
        );
        // XML 1.0 names start with a letter or `_` and hold no `²` or `µ`; oxrdfxml reads
        // such element names anyway, other RDF/XML readers refuse the document.
        for bad in ["<dct:1st", "<dcat:µg", ":2026-value"] {
            assert!(!xml.contains(bad), "{bad} is no XML name: {xml}");
        }
        // A predicate with no XML-name suffix at all cannot be written in RDF/XML; the
        // document stays well-formed.
        let mut g = RdfGraph::new();
        g.insert(
            s,
            format!("{}m²", ns::DCT),
            Term::Literal(crate::graph::LiteralValue::plain("v")),
        );
        let xml = serialize_rdfxml(&g);
        assert!(!xml.contains("m²>"), "{xml}");
        assert!(parse_rdfxml(&xml, &RdfLimits::default()).is_ok());
    }
}
