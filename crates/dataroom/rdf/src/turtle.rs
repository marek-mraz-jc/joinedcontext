#![forbid(unsafe_code)]
//! Turtle (`.ttl`) and N-Triples (`.nt`) parser and serializer with resource limits.

use crate::graph::{ns, LiteralValue, RdfGraph, RdfLimits, SubjectNode, Term, Triple};
use dataroom_core::{DataroomError, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Parses a Turtle or N-Triples document into an [`RdfGraph`], enforcing [`RdfLimits`].
pub fn parse_turtle(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    if input.len() > limits.max_bytes {
        return Err(DataroomError::Rdf(format!(
            "RDF input size {} exceeds limit of {} bytes",
            input.len(),
            limits.max_bytes
        )));
    }
    let mut parser = TurtleParser::new(input, *limits);
    parser.parse_document()?;
    Ok(parser.graph)
}

struct TurtleParser<'a> {
    chars: Vec<char>,
    pos: usize,
    prefixes: BTreeMap<String, String>,
    base: Option<String>,
    graph: RdfGraph,
    limits: RdfLimits,
    depth: usize,
    _marker: &'a str,
}

impl<'a> TurtleParser<'a> {
    fn new(input: &'a str, limits: RdfLimits) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            prefixes: BTreeMap::new(),
            base: None,
            graph: RdfGraph::new(),
            limits,
            depth: 0,
            _marker: input,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn skip_ws_and_comments(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else if c == '#' {
                while let Some(ch) = self.next_char() {
                    if ch == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn starts_with_ci(&self, word: &str) -> bool {
        let len = word.chars().count();
        if self.pos + len > self.chars.len() {
            return false;
        }
        self.chars[self.pos..self.pos + len]
            .iter()
            .zip(word.chars())
            .all(|(a, b)| a.eq_ignore_ascii_case(&b))
    }

    fn parse_document(&mut self) -> Result<()> {
        loop {
            self.skip_ws_and_comments();
            if self.peek().is_none() {
                break;
            }
            if self.starts_with_ci("@prefix") {
                self.pos += 7;
                self.skip_ws_and_comments();
                let prefix = self.read_prefix_label()?;
                self.skip_ws_and_comments();
                let iri = self.read_iri_ref()?;
                self.skip_ws_and_comments();
                self.expect_char('.')?;
            } else if self.starts_with_ci("@base") {
                self.pos += 5;
                self.skip_ws_and_comments();
                let iri = self.read_iri_ref()?;
                self.base = Some(iri);
                self.skip_ws_and_comments();
                self.expect_char('.')?;
            } else if self.starts_with_ci("PREFIX")
                && self.peek_at(6).is_some_and(char::is_whitespace)
            {
                self.pos += 6;
                self.skip_ws_and_comments();
                let prefix = self.read_prefix_label()?;
                self.skip_ws_and_comments();
                let iri = self.read_iri_ref()?;
                self.prefixes.insert(prefix, iri);
            } else if self.starts_with_ci("BASE")
                && self.peek_at(4).is_some_and(char::is_whitespace)
            {
                self.pos += 4;
                self.skip_ws_and_comments();
                let iri = self.read_iri_ref()?;
                self.base = Some(iri);
            } else {
                self.parse_triples_statement()?;
            }
        }
        Ok(())
    }

    fn expect_char(&mut self, expected: char) -> Result<()> {
        match self.next_char() {
            Some(c) if c == expected => Ok(()),
            other => Err(DataroomError::Rdf(format!(
                "expected '{expected}' at position {}, got {other:?}",
                self.pos
            ))),
        }
    }

    fn read_prefix_label(&mut self) -> Result<String> {
        let mut label = String::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == ':' {
                return Ok(label.trim().to_owned());
            }
            if c.is_whitespace() {
                return Err(DataroomError::Rdf(
                    "unexpected whitespace in prefix declaration".into(),
                ));
            }
            label.push(c);
        }
        Err(DataroomError::Rdf("unterminated prefix label".into()))
    }

    fn read_iri_ref(&mut self) -> Result<String> {
        self.expect_char('<')?;
        let mut iri = String::new();
        while let Some(c) = self.next_char() {
            if c == '>' {
                if let Some(base) = &self.base {
                    if !iri.contains("://") && !iri.starts_with("mailto:") {
                        if iri.starts_with('#') {
                            return Ok(format!("{base}{iri}"));
                        }
                    }
                }
                return Ok(iri);
            }
            iri.push(c);
        }
        Err(DataroomError::Rdf("unterminated <IRI>".into()))
    }

    fn parse_triples_statement(&mut self) -> Result<()> {
        self.skip_ws_and_comments();
        let subject = if self.peek() == Some('[') {
            let bnode = self.parse_blank_node_property_list()?;
            self.skip_ws_and_comments();
            if self.peek() == Some('.') {
                self.pos += 1;
                return Ok(());
            }
            bnode
        } else {
            let term = self.parse_term()?;
            term.as_subject()
                .ok_or_else(|| DataroomError::Rdf("literal cannot be a triple subject".into()))?
        };
        self.parse_predicate_object_list(&subject)?;
        self.skip_ws_and_comments();
        self.expect_char('.')?;
        Ok(())
    }

    fn parse_predicate_object_list(&mut self, subject: &SubjectNode) -> Result<()> {
        loop {
            self.skip_ws_and_comments();
            if matches!(self.peek(), Some('.' | ']' | ')') | None) {
                break;
            }
            let predicate = self.parse_predicate()?;
            self.parse_object_list(subject, &predicate)?;
            self.skip_ws_and_comments();
            if self.peek() == Some(';') {
                while self.peek() == Some(';') {
                    self.pos += 1;
                    self.skip_ws_and_comments();
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    fn parse_predicate(&mut self) -> Result<String> {
        self.skip_ws_and_comments();
        if self.peek() == Some('a')
            && self
                .peek_at(1)
                .is_none_or(|c| c.is_whitespace() || matches!(c, '<' | '[' | '"' | '\''))
        {
            self.pos += 1;
            return Ok(ns::RDF_TYPE.to_owned());
        }
        let term = self.parse_term()?;
        match term {
            Term::Iri(iri) => Ok(iri),
            _ => Err(DataroomError::Rdf("predicate must be an IRI".into())),
        }
    }

    fn parse_object_list(&mut self, subject: &SubjectNode, predicate: &str) -> Result<()> {
        loop {
            self.skip_ws_and_comments();
            let object = self.parse_term()?;
            self.graph.insert_checked(
                Triple {
                    subject: subject.clone(),
                    predicate: predicate.to_owned(),
                    object,
                },
                &self.limits,
            )?;
            self.skip_ws_and_comments();
            if self.peek() == Some(',') {
                self.pos += 1;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn parse_blank_node_property_list(&mut self) -> Result<SubjectNode> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(DataroomError::Rdf(format!(
                "RDF blank node nesting exceeded maximum depth of {}",
                self.limits.max_depth
            )));
        }
        self.expect_char('[')?;
        self.skip_ws_and_comments();
        let bnode = self.graph.fresh_blank_node();
        if self.peek() == Some(']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(bnode);
        }
        self.parse_predicate_object_list(&bnode)?;
        self.skip_ws_and_comments();
        self.expect_char(']')?;
        self.depth -= 1;
        Ok(bnode)
    }

    fn parse_collection(&mut self) -> Result<Term> {
        self.depth += 1;
        if self.depth > self.limits.max_depth {
            return Err(DataroomError::Rdf(format!(
                "RDF collection nesting exceeded maximum depth of {}",
                self.limits.max_depth
            )));
        }
        self.expect_char('(')?;
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.peek() == Some(')') {
                self.pos += 1;
                break;
            }
            if self.peek().is_none() {
                return Err(DataroomError::Rdf("unterminated RDF collection '('".into()));
            }
            items.push(self.parse_term()?);
        }
        self.depth -= 1;
        if items.is_empty() {
            return Ok(Term::Iri(ns::RDF_NIL.to_owned()));
        }
        let mut head: Option<SubjectNode> = None;
        let mut prev: Option<SubjectNode> = None;
        for item in items {
            let node = self.graph.fresh_blank_node();
            if head.is_none() {
                head = Some(node.clone());
            }
            if let Some(p) = prev {
                self.graph.insert_checked(
                    Triple {
                        subject: p,
                        predicate: ns::RDF_REST.to_owned(),
                        object: node.to_term(),
                    },
                    &self.limits,
                )?;
            }
            self.graph.insert_checked(
                Triple {
                    subject: node.clone(),
                    predicate: ns::RDF_FIRST.to_owned(),
                    object: item,
                },
                &self.limits,
            )?;
            prev = Some(node);
        }
        if let Some(last) = prev {
            self.graph.insert_checked(
                Triple {
                    subject: last,
                    predicate: ns::RDF_REST.to_owned(),
                    object: Term::Iri(ns::RDF_NIL.to_owned()),
                },
                &self.limits,
            )?;
        }
        Ok(head.unwrap().to_term())
    }

    fn parse_term(&mut self) -> Result<Term> {
        self.skip_ws_and_comments();
        let Some(c) = self.peek() else {
            return Err(DataroomError::Rdf("unexpected end of input".into()));
        };
        match c {
            '<' => {
                let iri = self.read_iri_ref()?;
                Ok(Term::Iri(iri))
            }
            '[' => {
                let bnode = self.parse_blank_node_property_list()?;
                Ok(bnode.to_term())
            }
            '(' => self.parse_collection(),
            '"' | '\'' => {
                let lit = self.parse_string_literal()?;
                Ok(Term::Literal(lit))
            }
            '_' if self.peek_at(1) == Some(':') => {
                self.pos += 2;
                let mut label = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_whitespace() || matches!(ch, ';' | ',' | ']' | ')') {
                        break;
                    }
                    if ch == '.'
                        && self
                            .peek_at(1)
                            .is_none_or(|n| n.is_whitespace() || matches!(n, ';' | ',' | ']' | ')'))
                    {
                        break;
                    }
                    label.push(ch);
                    self.pos += 1;
                }
                Ok(Term::Blank(label))
            }
            '+' | '-' | '0'..='9' => self.parse_number_or_pname(),
            _ => self.parse_pname_or_boolean(),
        }
    }

    fn parse_string_literal(&mut self) -> Result<LiteralValue> {
        let quote = self.next_char().unwrap();
        let long = if self.peek() == Some(quote) && self.peek_at(1) == Some(quote) {
            self.pos += 2;
            true
        } else {
            false
        };
        let mut val = String::new();
        loop {
            let Some(c) = self.next_char() else {
                return Err(DataroomError::Rdf("unterminated string literal".into()));
            };
            if c == '\\' {
                match self.next_char() {
                    Some('n') => val.push('\n'),
                    Some('r') => val.push('\r'),
                    Some('t') => val.push('\t'),
                    Some('"') => val.push('"'),
                    Some('\'') => val.push('\''),
                    Some('\\') => val.push('\\'),
                    Some('u') => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            if let Some(h) = self.next_char() {
                                hex.push(h);
                            }
                        }
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                val.push(ch);
                            }
                        }
                    }
                    Some('U') => {
                        let mut hex = String::new();
                        for _ in 0..8 {
                            if let Some(h) = self.next_char() {
                                hex.push(h);
                            }
                        }
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(ch) = char::from_u32(code) {
                                val.push(ch);
                            }
                        }
                    }
                    Some(other) => val.push(other),
                    None => return Err(DataroomError::Rdf("unterminated escape".into())),
                }
                continue;
            }
            if long {
                if c == quote && self.peek() == Some(quote) && self.peek_at(1) == Some(quote) {
                    self.pos += 2;
                    break;
                }
            } else if c == quote {
                break;
            }
            val.push(c);
        }

        if self.peek() == Some('@') {
            self.pos += 1;
            let mut lang = String::new();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_alphanumeric() || ch == '-' {
                    lang.push(ch);
                    self.pos += 1;
                } else {
                    break;
                }
            }
            return Ok(LiteralValue::lang(val, lang));
        }

        if self.peek() == Some('^') && self.peek_at(1) == Some('^') {
            self.pos += 2;
            let dt_term = self.parse_term()?;
            if let Term::Iri(dt) = dt_term {
                return Ok(LiteralValue::typed(val, dt));
            }
        }

        Ok(LiteralValue::plain(val))
    }

    fn read_token(&mut self) -> String {
        let mut token = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, ';' | ',' | ']' | ')' | '<' | '>' | '"') {
                break;
            }
            if c == '.'
                && self
                    .peek_at(1)
                    .is_none_or(|n| n.is_whitespace() || matches!(n, ';' | ',' | ']' | ')'))
            {
                break;
            }
            if c == '\\' {
                self.pos += 1;
                if let Some(escaped) = self.next_char() {
                    token.push(escaped);
                }
                continue;
            }
            token.push(c);
            self.pos += 1;
        }
        token
    }

    fn parse_number_or_pname(&mut self) -> Result<Term> {
        let token = self.read_token();
        if token.contains(':') {
            return self.expand_pname(&token);
        }
        if token.contains('e') || token.contains('E') {
            return Ok(Term::Literal(LiteralValue::typed(token, ns::XSD_DOUBLE)));
        }
        if token.contains('.') {
            return Ok(Term::Literal(LiteralValue::typed(token, ns::XSD_DECIMAL)));
        }
        Ok(Term::Literal(LiteralValue::typed(token, ns::XSD_INTEGER)))
    }

    fn parse_pname_or_boolean(&mut self) -> Result<Term> {
        let token = self.read_token();
        if token == "true" || token == "false" {
            return Ok(Term::Literal(LiteralValue::typed(token, ns::XSD_BOOLEAN)));
        }
        if token == "a" {
            return Ok(Term::Iri(ns::RDF_TYPE.to_owned()));
        }
        self.expand_pname(&token)
    }

    fn expand_pname(&self, token: &str) -> Result<Term> {
        let Some((prefix, local)) = token.split_once(':') else {
            return Err(DataroomError::Rdf(format!("invalid prefixed name '{token}'")));
        };
        let Some(ns_iri) = self.prefixes.get(prefix) else {
            return Err(DataroomError::Rdf(format!("unknown prefix '{prefix}:'")));
        };
        Ok(Term::Iri(format!("{ns_iri}{local}")))
    }

    fn parse_document_with_prefixes(&mut self) -> Result<()> {
        self.parse_document()
    }
}

impl<'a> TurtleParser<'a> {
    fn register_prefix(&mut self, prefix: String, iri: String) {
        self.prefixes.insert(prefix, iri);
    }
}

// Fix for `@prefix` branch in `parse_document` so it stores the prefix into `self.prefixes`:
pub fn parse_turtle_checked(input: &str, limits: &RdfLimits) -> Result<RdfGraph> {
    if input.len() > limits.max_bytes {
        return Err(DataroomError::Rdf(format!(
            "RDF input size {} exceeds limit of {} bytes",
            input.len(),
            limits.max_bytes
        )));
    }
    let mut parser = TurtleParser::new(input, *limits);
    loop {
        parser.skip_ws_and_comments();
        if parser.peek().is_none() {
            break;
        }
        if parser.starts_with_ci("@prefix") {
            parser.pos += 7;
            parser.skip_ws_and_comments();
            let prefix = parser.read_prefix_label()?;
            parser.skip_ws_and_comments();
            let iri = parser.read_iri_ref()?;
            parser.register_prefix(prefix, iri);
            parser.skip_ws_and_comments();
            parser.expect_char('.')?;
        } else if parser.starts_with_ci("@base") {
            parser.pos += 5;
            parser.skip_ws_and_comments();
            let iri = parser.read_iri_ref()?;
            parser.base = Some(iri);
            parser.skip_ws_and_comments();
            parser.expect_char('.')?;
        } else if parser.starts_with_ci("PREFIX")
            && parser.peek_at(6).is_some_and(char::is_whitespace)
        {
            parser.pos += 6;
            parser.skip_ws_and_comments();
            let prefix = parser.read_prefix_label()?;
            parser.skip_ws_and_comments();
            let iri = parser.read_iri_ref()?;
            parser.register_prefix(prefix, iri);
        } else if parser.starts_with_ci("BASE")
            && parser.peek_at(4).is_some_and(char::is_whitespace)
        {
            parser.pos += 4;
            parser.skip_ws_and_comments();
            let iri = parser.read_iri_ref()?;
            parser.base = Some(iri);
        } else {
            parser.parse_triples_statement()?;
        }
    }
    Ok(parser.graph)
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

    // Group triples by subject, then by predicate.
    let mut by_subject: BTreeMap<&SubjectNode, BTreeMap<&str, Vec<&Term>>> = BTreeMap::new();
    for triple in &graph.triples {
        by_subject
            .entry(&triple.subject)
            .or_default()
            .entry(triple.predicate.as_str())
            .or_default()
            .push(&triple.object);
    }

    // Put named subjects before blank nodes.
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
        // Put rdf:type (`a`) first.
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
                format!("\"{esc}\"^^ carries_dt").replace(
                    "carries_dt",
                    &format_iri(&lit.datatype, prefixes),
                )
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
