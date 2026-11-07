//! The list `filter` and `orderBy` of every kind (API/13 section 2): a subset of AIP-160 over
//! the manifest as JSON. Fields are dotted manifest paths (`spec.schedule`) or `labels.{key}`,
//! checked against the kind's jc-core JSON Schema, so a field the kind lacks is refused with its
//! position instead of matching nothing.
//!
//! Grammar, OR binding tighter than AND as AIP-160 has it:
//! `expr = seq {AND seq}`, `seq = factor {factor}` (juxtaposition is AND),
//! `factor = term {OR term}`, `term = [NOT] (restriction | "(" expr ")")`,
//! `restriction = field op value` with op one of `= != < <= > >= :`; `field:*` is "present".

use std::cmp::Ordering;

use serde_json::Value;

/// Longest filter text a list takes; a longer one is refused before it is parsed.
pub const MAX_LEN: usize = 2000;
/// Deepest nesting of parentheses and NOT a filter may have.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "filter at position {}: {}", self.position, self.message)
    }
}

fn err<T>(position: usize, message: impl Into<String>) -> Result<T, FilterError> {
    Err(FilterError {
        position,
        message: message.into(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Has,
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// `value` is `None` for `field:*`.
    Cmp {
        path: Vec<String>,
        op: Op,
        value: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Open,
    Close,
    Op(Op),
    And,
    Or,
    Not,
    /// A bare word; `true` when it was quoted, so `"AND"` stays a value.
    Word(String, bool),
}

fn tokens(text: &str) -> Result<Vec<(usize, Tok)>, FilterError> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&(pos, c)) = chars.get(i) {
        let next = chars.get(i + 1).map(|&(_, c)| c);
        i += 1;
        match c {
            c if c.is_whitespace() => {}
            '(' => out.push((pos, Tok::Open)),
            ')' => out.push((pos, Tok::Close)),
            '=' => out.push((pos, Tok::Op(Op::Eq))),
            ':' => out.push((pos, Tok::Op(Op::Has))),
            '!' if next == Some('=') => {
                i += 1;
                out.push((pos, Tok::Op(Op::Ne)));
            }
            '<' | '>' => {
                let or_equal = next == Some('=');
                i += usize::from(or_equal);
                let op = match (c, or_equal) {
                    ('<', false) => Op::Lt,
                    ('<', true) => Op::Le,
                    ('>', false) => Op::Gt,
                    _ => Op::Ge,
                };
                out.push((pos, Tok::Op(op)));
            }
            '"' => {
                let (word, next) = quoted(&chars, i, pos)?;
                i = next;
                out.push((pos, Tok::Word(word, true)));
            }
            '!' => return err(pos, "`!` is only part of `!=`; negate with NOT"),
            _ => {
                let (word, next) = bare(&chars, i - 1);
                i = next;
                out.push((
                    pos,
                    match word.as_str() {
                        "AND" => Tok::And,
                        "OR" => Tok::Or,
                        "NOT" => Tok::Not,
                        _ => Tok::Word(word, false),
                    },
                ));
            }
        }
    }
    Ok(out)
}

/// The text of a quoted string whose content starts at `i`, and the index after its closing quote.
fn quoted(
    chars: &[(usize, char)],
    mut i: usize,
    pos: usize,
) -> Result<(String, usize), FilterError> {
    let mut word = String::new();
    loop {
        match chars.get(i) {
            None => return err(pos, "the quoted string is not closed"),
            Some(&(_, '"')) => return Ok((word, i + 1)),
            Some(&(_, '\\')) => {
                let Some(&(_, escaped)) = chars.get(i + 1) else {
                    return err(pos, "the quoted string is not closed");
                };
                word.push(escaped);
                i += 2;
            }
            Some(&(_, c)) => {
                word.push(c);
                i += 1;
            }
        }
    }
}

/// A bare word starting at `i`, and the index after it.
fn bare(chars: &[(usize, char)], mut i: usize) -> (String, usize) {
    let mut word = String::new();
    while let Some(&(_, c)) = chars.get(i) {
        if c.is_whitespace() || "()=!<>:\"".contains(c) {
            break;
        }
        word.push(c);
        i += 1;
    }
    (word, i)
}

struct Parser<'a> {
    toks: Vec<(usize, Tok)>,
    at: usize,
    end: usize,
    schema: Option<&'a Value>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at).map(|(_, t)| t)
    }

    fn pos(&self) -> usize {
        self.toks.get(self.at).map_or(self.end, |(p, _)| *p)
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, FilterError> {
        if depth > MAX_DEPTH {
            return err(self.pos(), "the filter nests too deep");
        }
        let mut left = self.seq(depth)?;
        while self.peek() == Some(&Tok::And) {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.seq(depth)?));
        }
        Ok(left)
    }

    fn seq(&mut self, depth: usize) -> Result<Expr, FilterError> {
        let mut left = self.factor(depth)?;
        while matches!(self.peek(), Some(Tok::Word(..) | Tok::Open | Tok::Not)) {
            left = Expr::And(Box::new(left), Box::new(self.factor(depth)?));
        }
        Ok(left)
    }

    fn factor(&mut self, depth: usize) -> Result<Expr, FilterError> {
        let mut left = self.term(depth)?;
        while self.peek() == Some(&Tok::Or) {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.term(depth)?));
        }
        Ok(left)
    }

    fn term(&mut self, depth: usize) -> Result<Expr, FilterError> {
        match self.peek() {
            Some(Tok::Not) => {
                self.at += 1;
                Ok(Expr::Not(Box::new(self.term(depth + 1)?)))
            }
            Some(Tok::Open) => {
                let open = self.pos();
                self.at += 1;
                let inner = self.expr(depth + 1)?;
                if self.peek() != Some(&Tok::Close) {
                    return err(open, "the parenthesis is not closed");
                }
                self.at += 1;
                Ok(inner)
            }
            _ => self.restriction(),
        }
    }

    fn restriction(&mut self) -> Result<Expr, FilterError> {
        let start = self.pos();
        let Some(Tok::Word(field, false)) = self.peek().cloned() else {
            return err(
                start,
                "expected a field such as metadata.name or labels.team",
            );
        };
        self.at += 1;
        let Some(Tok::Op(op)) = self.peek().cloned() else {
            return err(
                self.pos(),
                format!("`{field}` needs an operator: = != < <= > >= or :"),
            );
        };
        self.at += 1;
        let value = match self.peek().cloned() {
            Some(Tok::Word(word, quoted)) => {
                self.at += 1;
                (quoted || word != "*").then_some(word)
            }
            _ => {
                return err(
                    self.pos(),
                    format!("`{field}` needs a value after its operator"),
                )
            }
        };
        if value.is_none() && op != Op::Has {
            return err(
                start,
                format!("`*` means present only after `:`, as `{field}:*`"),
            );
        }
        let path = field_path(&field).ok_or_else(|| FilterError {
            position: start,
            message: format!("`{field}` is not a field: use a dotted path such as spec.space"),
        })?;
        if !known(self.schema, &path) {
            return err(
                start,
                format!(
                    "`{field}` is not a field of this kind; read its schema with jc_schema_get"
                ),
            );
        }
        Ok(Expr::Cmp { path, op, value })
    }
}

/// `labels.team` is `metadata.labels.team`; every other field is its dotted path.
fn field_path(field: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = field.split('.').map(str::to_owned).collect();
    if parts.iter().any(String::is_empty) {
        return None;
    }
    Some(match parts.first().map(String::as_str) {
        Some("labels") => ["metadata".to_owned()].into_iter().chain(parts).collect(),
        _ => parts,
    })
}

/// Whether `path` names a field of the kind's schema. Without a schema (a Portal-only kind) the
/// envelope's own members are known: metadata, status and an open spec.
fn known(schema: Option<&Value>, path: &[String]) -> bool {
    match schema {
        Some(schema) => has_path(schema, schema, path, 0),
        None => matches!(
            path.first().map(String::as_str),
            Some("metadata" | "spec" | "status" | "kind" | "apiVersion")
        ),
    }
}

fn has_path(root: &Value, node: &Value, path: &[String], hops: usize) -> bool {
    // A self-referencing schema stops here rather than walking for ever.
    if hops > 64 {
        return false;
    }
    let Some((head, rest)) = path.split_first() else {
        return true;
    };
    if let Some(target) = node.get("$ref").and_then(Value::as_str) {
        let name = target.rsplit('/').next().unwrap_or_default();
        let defs = root.get("definitions").or_else(|| root.get("$defs"));
        return defs
            .and_then(|defs| defs.get(name))
            .is_some_and(|def| has_path(root, def, path, hops + 1));
    }
    for combinator in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = node.get(combinator).and_then(Value::as_array) {
            if branches.iter().any(|b| has_path(root, b, path, hops + 1)) {
                return true;
            }
        }
    }
    if let Some(items) = node.get("items") {
        if has_path(root, items, path, hops + 1) {
            return true;
        }
    }
    if let Some(child) = node.get("properties").and_then(|p| p.get(head)) {
        return has_path(root, child, rest, hops + 1);
    }
    match node.get("additionalProperties") {
        Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(child @ Value::Object(_)) => has_path(root, child, rest, hops + 1),
        // An object with named members takes no other; a node that names none is open.
        _ => {
            node.get("properties").is_none()
                && !has_combinator(node)
                && node.get("items").is_none()
                && node.get("$ref").is_none()
                && is_open(node)
        }
    }
}

fn has_combinator(node: &Value) -> bool {
    ["allOf", "anyOf", "oneOf"]
        .iter()
        .any(|c| node.get(c).is_some())
}

/// A node with no `type` or `type: object` takes any member; a scalar has none.
fn is_open(node: &Value) -> bool {
    match node.get("type") {
        None => true,
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t == "object"),
        Some(_) => false,
    }
}

/// A parsed filter, ready to run over manifests of the kind it was checked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter(Expr);

impl Filter {
    /// Parses `text` and checks every field against `schema` (the kind's jc-core JSON Schema).
    pub fn parse(text: &str, schema: Option<&Value>) -> Result<Self, FilterError> {
        if text.len() > MAX_LEN {
            return err(MAX_LEN, format!("a filter is at most {MAX_LEN} characters"));
        }
        let mut parser = Parser {
            toks: tokens(text)?,
            at: 0,
            end: text.len(),
            schema,
        };
        if parser.toks.is_empty() {
            return err(0, "the filter is empty; leave it out to list everything");
        }
        let expr = parser.expr(0)?;
        if parser.at < parser.toks.len() {
            return err(parser.pos(), "unexpected text; join terms with AND or OR");
        }
        Ok(Self(expr))
    }

    pub fn matches(&self, doc: &Value) -> bool {
        eval(&self.0, doc)
    }
}

fn eval(expr: &Expr, doc: &Value) -> bool {
    match expr {
        Expr::And(a, b) => eval(a, doc) && eval(b, doc),
        Expr::Or(a, b) => eval(a, doc) || eval(b, doc),
        Expr::Not(a) => !eval(a, doc),
        Expr::Cmp { path, op, value } => {
            let found = values_at(doc, path);
            match (op, value) {
                (Op::Has, None) => !found.is_empty(),
                (Op::Has, Some(want)) => found.iter().any(|v| match v {
                    Value::Object(map) => map.contains_key(want),
                    other => text_of(other).as_deref() == Some(want.as_str()),
                }),
                (Op::Ne, Some(want)) => !found.iter().any(|v| text_of(v).as_deref() == Some(want)),
                (op, Some(want)) => found.iter().any(|v| {
                    let order = compare(v, want);
                    match op {
                        Op::Eq => order == Some(Ordering::Equal),
                        Op::Lt => order == Some(Ordering::Less),
                        Op::Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
                        Op::Gt => order == Some(Ordering::Greater),
                        _ => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
                    }
                }),
                (_, None) => false,
            }
        }
    }
}

/// Every non-null value at `path`, stepping into each element of an array on the way.
fn values_at<'a>(doc: &'a Value, path: &[String]) -> Vec<&'a Value> {
    let mut current = vec![doc];
    for segment in path {
        current = current
            .into_iter()
            .flat_map(|v| match v {
                Value::Array(items) => items.iter().filter_map(|i| i.get(segment)).collect(),
                other => other.get(segment).into_iter().collect::<Vec<_>>(),
            })
            .collect();
    }
    current
        .into_iter()
        .flat_map(|v| match v {
            Value::Array(items) => items.iter().collect(),
            other => vec![other],
        })
        .filter(|v| !v.is_null())
        .collect()
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A number compares as a number when both sides are numbers, everything else as text, so
/// RFC 3339 instants order by time.
fn compare(value: &Value, want: &str) -> Option<Ordering> {
    if let (Some(a), Ok(b)) = (value.as_f64(), want.parse::<f64>()) {
        return a.partial_cmp(&b);
    }
    Some(text_of(value)?.as_str().cmp(want))
}

/// One `orderBy`: a field of the kind, ascending unless followed by `desc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBy {
    path: Vec<String>,
    pub descending: bool,
}

impl Default for OrderBy {
    fn default() -> Self {
        Self {
            path: vec!["metadata".into(), "name".into()],
            descending: false,
        }
    }
}

impl OrderBy {
    pub fn parse(text: &str, schema: Option<&Value>) -> Result<Self, FilterError> {
        let mut words = text.split_whitespace();
        let Some(field) = words.next() else {
            return Ok(Self::default());
        };
        let descending = match words.next() {
            None | Some("asc") => false,
            Some("desc") => true,
            Some(other) => return err(field.len() + 1, format!("`{other}` is not asc or desc")),
        };
        if words.next().is_some() {
            return err(0, "orderBy takes one field, optionally followed by desc");
        }
        let path = field_path(field)
            .filter(|p| known(schema, p))
            .ok_or_else(|| FilterError {
                position: 0,
                message: format!("`{field}` is not a field of this kind"),
            })?;
        Ok(Self { path, descending })
    }

    /// An order by the tie-breaking name alone, for a list whose rows have no field it sorts by
    /// first (the organization's Endpoints: project, then name).
    pub fn by_name_alone() -> Self {
        Self {
            path: Vec::new(),
            descending: false,
        }
    }

    /// The value a manifest sorts by: its first value at the field, null when it has none.
    pub fn key_of(&self, doc: &Value) -> Value {
        if self.path.is_empty() {
            return Value::Null;
        }
        values_at(doc, &self.path)
            .first()
            .map_or(Value::Null, |v| (*v).clone())
    }

    /// Orders two (key, name) pairs: missing keys last, numbers as numbers, then by name.
    pub fn cmp(&self, a: (&Value, &str), b: (&Value, &str)) -> Ordering {
        let keys = match (a.0, b.0) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => return Ordering::Greater,
            (_, Value::Null) => return Ordering::Less,
            (x, y) => match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                _ => text_of(x).cmp(&text_of(y)),
            },
        };
        let keys = if self.descending {
            keys.reverse()
        } else {
            keys
        };
        keys.then_with(|| a.1.cmp(b.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pipeline_schema() -> Value {
        jc_core::registry::schema_of("Pipeline").expect("Pipeline schema")
    }

    fn doc() -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1", "kind": "Pipeline",
            "metadata": {"name": "air", "namespace": "p", "labels": {"team": "air", "tier": "1"}},
            "spec": {"schedule": "0 * * * *", "outputs": [{"targetEndpoint": "urn:a"}, {"targetEndpoint": "urn:b"}]},
            "status": {"phase": "Live"}
        })
    }

    fn check(text: &str) -> bool {
        let schema = pipeline_schema();
        Filter::parse(text, Some(&schema))
            .expect(text)
            .matches(&doc())
    }

    #[test]
    fn restrictions_match_as_aip_160_says() {
        assert!(check(r#"labels.team = "air""#));
        assert!(check("labels.team=air AND spec.schedule:*"));
        assert!(!check("labels.team = water"));
        assert!(check("labels.team != water"));
        assert!(check("labels:team"));
        assert!(!check("labels:owner"));
        assert!(check(r#"spec.outputs.targetEndpoint = "urn:b""#));
        assert!(check("NOT spec.period:*"));
        assert!(check("labels.tier >= 1 labels.tier < 2"));
        assert!(check(
            "(labels.team = water OR labels.team = air) AND status.phase = Live"
        ));
        assert!(check("metadata.name > a"));
    }

    #[test]
    fn or_binds_tighter_than_and() {
        // a AND b OR c is a AND (b OR c): false AND ... is false whatever c is.
        assert!(!check(
            "labels.team = water AND labels.team = water OR labels.team = air"
        ));
    }

    #[test]
    fn an_unknown_field_is_refused_with_its_position() {
        let schema = pipeline_schema();
        let refused = Filter::parse("labels.team = air AND spec.nope = 1", Some(&schema))
            .expect_err("spec.nope is no Pipeline field");
        assert_eq!(refused.position, 22);
        assert!(refused.message.contains("spec.nope"), "{}", refused.message);
        for bad in [
            "",
            "labels.team",
            "labels.team =",
            "= air",
            "(labels.team = air",
            "labels.team = \"air",
            "spec..x = 1",
            "metadata.name = *",
            "a ! b",
        ] {
            assert!(Filter::parse(bad, Some(&schema)).is_err(), "{bad} parsed");
        }
        let deep = format!("{}labels.team=a{}", "(".repeat(40), ")".repeat(40));
        assert!(Filter::parse(&deep, Some(&schema)).is_err());
        assert!(Filter::parse(&"a".repeat(MAX_LEN + 1), Some(&schema)).is_err());
    }

    #[test]
    fn without_a_schema_only_the_envelope_is_known() {
        assert!(Filter::parse("spec.anything = 1", None).is_ok());
        assert!(Filter::parse("other = 1", None).is_err());
    }

    #[test]
    fn order_by_sorts_missing_keys_last_and_breaks_ties_by_name() {
        let schema = pipeline_schema();
        let order = OrderBy::parse("labels.tier desc", Some(&schema)).expect("orderBy");
        let (one, two, none) = (json!("1"), json!("2"), Value::Null);
        assert_eq!(order.cmp((&two, "a"), (&one, "b")), Ordering::Less);
        assert_eq!(order.cmp((&none, "a"), (&one, "b")), Ordering::Greater);
        assert_eq!(order.cmp((&one, "a"), (&one, "b")), Ordering::Less);
        assert_eq!(order.key_of(&doc()), json!("1"));
        assert!(OrderBy::parse("spec.nope", Some(&schema)).is_err());
        assert!(OrderBy::parse("metadata.name sideways", Some(&schema)).is_err());
        assert_eq!(
            OrderBy::parse("", Some(&schema)).expect("empty"),
            OrderBy::default()
        );
    }
}
