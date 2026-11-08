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
            // Outside strings and IRIs a backslash is a local-name escape (`ex:a\]`): the
            // escaped byte is no bracket.
            b'\\' => {
                i += 2;
                continue;
            }
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
        let ox_triple =
            item.map_err(|e| DataroomError::Rdf(format!("Turtle syntax error: {e}")))?;
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
                    .all(|c| pn_chars_base(c) || c.is_ascii_digit() || matches!(c, '_' | '-'))
                && !local.starts_with('-')
                && !local.ends_with('-')
            {
                return format!("{prefix}:{local}");
            }
        }
    }
    format!("<{iri}>")
}

/// Turtle's `PN_CHARS_BASE`: the letters a prefixed local name may hold. `char::is_alphanumeric`
/// also admits symbols such as `²` or `µ`, which made the serializer write invalid Turtle.
pub(crate) fn pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
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
        assert!(!g
            .instances_of("http://www.w3.org/ns/dcat#Dataset")
            .is_empty());

        let sk_ds = include_str!("../../specs/dcat-ap-sk/príklady/dataset.ttl");
        let g2 = parse_turtle_checked(sk_ds, &RdfLimits::default()).expect("parse sk dataset");
        assert!(!g2
            .instances_of("http://www.w3.org/ns/dcat#Dataset")
            .is_empty());
    }

    fn kind(e: DataroomError) -> String {
        match e {
            DataroomError::RdfLimitExceeded { kind, .. } => kind,
            DataroomError::Rdf(m) => format!("rdf: {m}"),
            other => format!("other: {other}"),
        }
    }

    fn depth(max_depth: usize) -> RdfLimits {
        RdfLimits {
            max_depth,
            ..RdfLimits::default()
        }
    }

    #[test]
    fn a1_turtle_depth_scan_skips_strings_iris_comments_and_escapes() {
        let deep = |n: usize| {
            format!(
                "<https://e.org/s> <https://e.org/p> {}<https://e.org/o>{} .",
                "( ".repeat(n),
                " )".repeat(n)
            )
        };
        assert!(parse_turtle(&deep(4), &depth(4)).is_ok());
        assert_eq!(
            kind(parse_turtle(&deep(5), &depth(4)).unwrap_err()),
            "nesting-depth"
        );
        // Brackets inside strings, long strings, IRIs and comments are no nesting; a closing
        // bracket there does not lower the depth either.
        let hidden = r#"@prefix ex: <https://e.org/> .
ex:s ex:a "[[[(((\"]]]" ; ex:b '[[[' ; ex:c """[[[
\"""]]]""" ;
  ex:d '''(((''' ; ex:e <https://e.org/x(((> . # ((((((
"#;
        let g = parse_turtle(hidden, &depth(1)).expect(hidden);
        assert_eq!(g.len(), 5);
        // `\)` and `\(` in a prefixed local name are escapes, not brackets: they must neither
        // close a nesting level (which let unbounded nesting through) nor open one.
        let mut sneaky = String::from("@prefix ex: <https://e.org/> .\nex:s ex:p ");
        for _ in 0..100 {
            sneaky.push_str(r"( ex:o\) ");
        }
        sneaky.push_str("ex:end");
        sneaky.push_str(&" )".repeat(100));
        sneaky.push_str(" .");
        assert_eq!(
            kind(parse_turtle(&sneaky, &depth(8)).unwrap_err()),
            "nesting-depth",
            "escaped closing brackets must not hide 100 nesting levels"
        );
        let escaped = "@prefix ex: <https://e.org/> .\nex:s ex:p ex:a\\(\\(\\(\\) .";
        let g = parse_turtle(escaped, &depth(1)).expect(escaped);
        assert_eq!(
            g.objects_of(
                &SubjectNode::Named("https://e.org/s".into()),
                "https://e.org/p"
            ),
            [Term::Iri("https://e.org/a((()".into())]
        );
    }

    #[test]
    fn a1_turtle_and_ntriples_caps_and_syntax_errors() {
        let d = RdfLimits::default();
        let small = RdfLimits { max_bytes: 3, ..d };
        assert_eq!(
            kind(parse_turtle("<a> <b> <c> .", &small).unwrap_err()),
            "bytes"
        );
        assert!(
            kind(parse_turtle("<https://e.org/s> <https://e.org/p> .", &d).unwrap_err())
                .starts_with("rdf: Turtle syntax error")
        );
        let short = RdfLimits {
            max_literal_length: 4,
            ..d
        };
        assert_eq!(
            kind(
                parse_turtle(
                    r#"<https://e.org/s> <https://e.org/p> "long literal" ."#,
                    &short
                )
                .unwrap_err()
            ),
            "literal-length"
        );
        let imports = "<https://e.org/s> <http://www.w3.org/2002/07/owl#imports> <https://evil.example/shapes> .";
        assert_eq!(
            kind(parse_turtle(imports, &RdfLimits::uploaded_shapes()).unwrap_err()),
            "owl-imports"
        );
        assert!(
            parse_turtle(imports, &d).is_ok(),
            "only uploaded shapes refuse owl:imports"
        );

        let nt = concat!(
            r#"<https://e.org/s> <https://e.org/p> "a\"b"@sk ."#,
            "\n",
            r#"_:b <https://e.org/p> "1"^^<http://www.w3.org/2001/XMLSchema#integer> ."#,
            "\n"
        );
        assert_eq!(parse_ntriples_bytes(nt.as_bytes(), &d).unwrap().len(), 2);
        let tiny = RdfLimits { max_bytes: 10, ..d };
        assert_eq!(
            kind(parse_ntriples_bytes(nt.as_bytes(), &tiny).unwrap_err()),
            "bytes"
        );
        let one = RdfLimits {
            max_triples: 1,
            ..d
        };
        assert_eq!(
            kind(parse_ntriples_bytes(nt.as_bytes(), &one).unwrap_err()),
            "triples"
        );
        assert!(kind(
            parse_ntriples_bytes(b"<https://e.org/s> <https://e.org/p> .", &d).unwrap_err()
        )
        .starts_with("rdf: N-Triples syntax error"));
        assert!(
            parse_ntriples_bytes(b"@prefix ex: <https://e.org/> .", &d).is_err(),
            "N-Triples has no prefixes"
        );
    }

    fn sample() -> RdfGraph {
        let mut g = RdfGraph::new();
        let s = SubjectNode::Named("https://e.org/ds".into());
        let dct = |l: &str| format!("{}{l}", ns::DCT);
        g.insert(
            s.clone(),
            ns::RDF_TYPE,
            Term::Iri(format!("{}Dataset", ns::DCAT)),
        );
        g.insert(
            s.clone(),
            dct("title"),
            Term::Literal(LiteralValue::lang(
                "Riadok 1\nTab\t\"úvodzovky\" \\ \r",
                "sk",
            )),
        );
        g.insert(
            s.clone(),
            dct("identifier"),
            Term::Literal(LiteralValue::plain("air")),
        );
        g.insert(
            s.clone(),
            dct("issued"),
            Term::Literal(LiteralValue::typed("2026-01-01", ns::XSD_DATE)),
        );
        g.insert(
            s.clone(),
            "https://unknown.example/v#x",
            Term::Literal(LiteralValue::typed("1", "https://unknown.example/dt")),
        );
        // Local names Turtle cannot write as a prefixed name: empty, a slash, a leading or
        // trailing dash, and Unicode alphanumerics outside PN_CHARS (superscript two, micro
        // sign, feminine ordinal).
        for local in ["", "a/b", "-a", "a-", "m²", "µg", "ª", "1st", "ľúbivý"] {
            g.insert(
                s.clone(),
                dct(&format!("p{local}")),
                Term::Iri(format!("{}{local}", ns::DCAT)),
            );
        }
        g.insert(
            s.clone(),
            format!("{}contactPoint", ns::DCAT),
            Term::Blank("cp".into()),
        );
        g.insert(
            SubjectNode::Blank("cp".into()),
            format!("{}fn", ns::VCARD),
            Term::Literal(LiteralValue::plain("Ján")),
        );
        g.insert(
            SubjectNode::Blank("a2".into()),
            format!("{}fn", ns::VCARD),
            Term::Literal(LiteralValue::plain("B")),
        );
        g
    }

    /// The graph with every blank node label replaced, for comparing parsed graphs.
    fn unlabelled(g: &RdfGraph) -> Vec<String> {
        let mut v: Vec<String> = g
            .triples
            .iter()
            .map(|t| {
                let s = match &t.subject {
                    SubjectNode::Blank(_) => "_".to_owned(),
                    n => n.to_string(),
                };
                let o = match &t.object {
                    Term::Blank(_) => "_".to_owned(),
                    o => o.to_string(),
                };
                format!("{s} <{}> {o}", t.predicate)
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn turtle_and_ntriples_serializers_round_trip() {
        let g = sample();
        let ttl = serialize_turtle(&g);
        assert!(ttl.contains("a dcat:Dataset"), "{ttl}");
        assert!(
            ttl.contains("dct:pľúbivý"),
            "letters stay prefixed names: {ttl}"
        );
        assert!(
            ttl.find("<https://e.org/ds>").unwrap() < ttl.find("_:a2").unwrap(),
            "named subjects first"
        );
        let back =
            parse_turtle(&ttl, &RdfLimits::default()).unwrap_or_else(|e| panic!("{e}\n{ttl}"));
        assert_eq!(unlabelled(&back), unlabelled(&g), "{ttl}");

        let nt = serialize_ntriples(&g);
        let back = parse_ntriples_bytes(nt.as_bytes(), &RdfLimits::default())
            .unwrap_or_else(|e| panic!("{e}\n{nt}"));
        assert_eq!(unlabelled(&back), unlabelled(&g), "{nt}");
    }
}
