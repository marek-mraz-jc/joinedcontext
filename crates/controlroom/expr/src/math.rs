#![forbid(unsafe_code)]
//! Math expression parser and evaluator ported from `GF pkg/expr/mathexp` (`parse/lex.go`,
//! `parse/parse.go`, `exp.go`, `funcs.go`).
//!
//! Enforces `MAX_PARSE_DEPTH = 100` (`parse.go:34` & R12).

use controlroom_model::{
    CrError, CrResult, FieldColumn, FieldType, Frame, FrameMeta, Notice, NoticeSeverity,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

pub const MAX_PARSE_DEPTH: usize = 100;

/// Intermediate value in the expression engine (`Scalar`, `Number`, `Series`, `NoData`).
#[derive(Debug, Clone, PartialEq)]
pub enum ExprValue {
    Scalar(Option<f64>),
    Number {
        name: String,
        labels: BTreeMap<String, String>,
        value: Option<f64>,
        notices: Vec<Notice>,
    },
    Series {
        name: String,
        labels: BTreeMap<String, String>,
        points: Vec<(i64, Option<f64>)>,
        notices: Vec<Notice>,
    },
    NoData,
}

impl ExprValue {
    #[must_use]
    pub fn labels(&self) -> &BTreeMap<String, String> {
        static EMPTY: BTreeMap<String, String> = BTreeMap::new();
        match self {
            Self::Number { labels, .. } | Self::Series { labels, .. } => labels,
            Self::Scalar(_) | Self::NoData => &EMPTY,
        }
    }

    /// Converts a [`Frame`] into one or more [`ExprValue`] items (matching `converter.go`).
    #[must_use]
    pub fn from_frame(frame: &Frame) -> Vec<Self> {
        if frame.fields.is_empty() {
            return vec![Self::NoData];
        }
        // Find time column and numeric value columns
        let time_idx = frame
            .fields
            .iter()
            .position(|f| f.field_type == FieldType::Time);
        let num_indices: Vec<usize> = frame
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.field_type == FieldType::Number)
            .map(|(i, _)| i)
            .collect();

        if let Some(t_idx) = time_idx {
            if !num_indices.is_empty() {
                let time_col = &frame.fields[t_idx];
                let mut out = Vec::with_capacity(num_indices.len());
                for v_idx in num_indices {
                    let val_col = &frame.fields[v_idx];
                    let len = time_col.len().min(val_col.len());
                    let mut points = Vec::with_capacity(len);
                    for i in 0..len {
                        let t = time_col.time_ms_at(i).unwrap_or(0);
                        let v = val_col.f64_at(i);
                        points.push((t, v));
                    }
                    let name = if !frame.name.is_empty() {
                        frame.name.clone()
                    } else {
                        val_col.effective_name().to_owned()
                    };
                    out.push(Self::Series {
                        name,
                        labels: val_col.labels.clone(),
                        points,
                        notices: frame.meta.notices.clone(),
                    });
                }
                return out;
            }
        }

        // Numeric single-row or number table
        if !num_indices.is_empty() {
            let v_idx = num_indices[0];
            let val_col = &frame.fields[v_idx];
            if val_col.len() == 0 {
                return vec![Self::NoData];
            }
            let mut out = Vec::with_capacity(val_col.len());
            for row in 0..val_col.len() {
                let mut labels = val_col.labels.clone();
                for (col_idx, col) in frame.fields.iter().enumerate() {
                    if col_idx != v_idx && col.field_type == FieldType::String {
                        if let Some(Value::String(s)) = col.values.get(row) {
                            labels.insert(col.name.clone(), s.clone());
                        }
                    }
                }
                out.push(Self::Number {
                    name: val_col.effective_name().to_owned(),
                    labels,
                    value: val_col.f64_at(row),
                    notices: frame.meta.notices.clone(),
                });
            }
            return out;
        }

        vec![Self::NoData]
    }

    /// Converts this [`ExprValue`] back into a [`Frame`] for `ref_id`.
    #[must_use]
    pub fn to_frame(&self, ref_id: &str) -> Frame {
        match self {
            Self::Scalar(v) => Frame {
                name: ref_id.to_owned(),
                ref_id: Some(ref_id.to_owned()),
                fields: vec![FieldColumn::numbers(ref_id, BTreeMap::new(), vec![*v])],
                meta: FrameMeta::default(),
            },
            Self::Number {
                name,
                labels,
                value,
                notices,
            } => Frame {
                name: if name.is_empty() {
                    ref_id.to_owned()
                } else {
                    name.clone()
                },
                ref_id: Some(ref_id.to_owned()),
                fields: vec![FieldColumn::numbers(ref_id, labels.clone(), vec![*value])],
                meta: FrameMeta {
                    notices: notices.clone(),
                    ..FrameMeta::default()
                },
            },
            Self::Series {
                name,
                labels,
                points,
                notices,
            } => {
                let times: Vec<i64> = points.iter().map(|(t, _)| *t).collect();
                let vals: Vec<Option<f64>> = points.iter().map(|(_, v)| *v).collect();
                Frame {
                    name: if name.is_empty() {
                        ref_id.to_owned()
                    } else {
                        name.clone()
                    },
                    ref_id: Some(ref_id.to_owned()),
                    fields: vec![
                        FieldColumn::time("Time", times),
                        FieldColumn::numbers(ref_id, labels.clone(), vals),
                    ],
                    meta: FrameMeta {
                        notices: notices.clone(),
                        ..FrameMeta::default()
                    },
                }
            }
            Self::NoData => Frame {
                name: "no data".to_owned(),
                ref_id: Some(ref_id.to_owned()),
                fields: Vec::new(),
                meta: FrameMeta::default(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Var(String),
    Ident(String),
    LParen,
    RParen,
    Comma,
    Not,
    And,
    Or,
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    Pow,
}

fn tokenize(input: &str) -> CrResult<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    let n = chars.len();
    let mut tokens = Vec::new();

    while i < n {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        match ch {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Div);
                i += 1;
            }
            '%' => {
                tokens.push(Token::Mod);
                i += 1;
            }
            '*' => {
                if i + 1 < n && chars[i + 1] == '*' {
                    tokens.push(Token::Pow);
                    i += 2;
                } else {
                    tokens.push(Token::Mul);
                    i += 1;
                }
            }
            '!' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    tokens.push(Token::Ne);
                    i += 2;
                } else {
                    tokens.push(Token::Not);
                    i += 1;
                }
            }
            '=' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    tokens.push(Token::Eq);
                    i += 2;
                } else {
                    return Err(CrError::ExpressionError(
                        "unexpected '=' (use '==' for equality)".into(),
                    ));
                }
            }
            '>' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    tokens.push(Token::Ge);
                    i += 2;
                } else {
                    tokens.push(Token::Gt);
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    tokens.push(Token::Le);
                    i += 2;
                } else {
                    tokens.push(Token::Lt);
                    i += 1;
                }
            }
            '&' => {
                if i + 1 < n && chars[i + 1] == '&' {
                    tokens.push(Token::And);
                    i += 2;
                } else {
                    return Err(CrError::ExpressionError("unexpected '&'".into()));
                }
            }
            '|' => {
                if i + 1 < n && chars[i + 1] == '|' {
                    tokens.push(Token::Or);
                    i += 2;
                } else {
                    return Err(CrError::ExpressionError("unexpected '|'".into()));
                }
            }
            '$' => {
                i += 1;
                if i < n && chars[i] == '{' {
                    i += 1;
                    let start = i;
                    while i < n && chars[i] != '}' {
                        let c = chars[i];
                        if !(c.is_alphanumeric() || c == '_' || c.is_whitespace()) {
                            return Err(CrError::ExpressionError(format!(
                                "unsupported variable character '{c}'"
                            )));
                        }
                        i += 1;
                    }
                    if i >= n || chars[i] != '}' {
                        return Err(CrError::ExpressionError(
                            "unterminated variable missing closing '}'".into(),
                        ));
                    }
                    let name: String = chars[start..i].iter().collect();
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        return Err(CrError::ExpressionError("incomplete variable".into()));
                    }
                    tokens.push(Token::Var(trimmed.to_owned()));
                    i += 1;
                } else {
                    let start = i;
                    while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        i += 1;
                    }
                    if i == start {
                        return Err(CrError::ExpressionError("incomplete variable".into()));
                    }
                    let name: String = chars[start..i].iter().collect();
                    tokens.push(Token::Var(name));
                }
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                if c == '0' && i + 1 < n && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                    i += 2;
                    while i < n && chars[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    let hex_str: String = chars[start + 2..i].iter().collect();
                    let val = u64::from_str_radix(&hex_str, 16).map_err(|_| {
                        CrError::ExpressionError(format!("bad hex number: 0x{hex_str}"))
                    })?;
                    tokens.push(Token::Number(val as f64));
                } else {
                    while i < n && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < n && chars[i] == '.' {
                        i += 1;
                        while i < n && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                        i += 1;
                        if i < n && (chars[i] == '+' || chars[i] == '-') {
                            i += 1;
                        }
                        while i < n && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let num_str: String = chars[start..i].iter().collect();
                    let val = num_str.parse::<f64>().map_err(|_| {
                        CrError::ExpressionError(format!("illegal number syntax: {num_str}"))
                    })?;
                    tokens.push(Token::Number(val));
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphabetic() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(ident));
            }
            other => {
                return Err(CrError::ExpressionError(format!(
                    "invalid character in math expression: '{other}'"
                )));
            }
        }
    }

    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq)]
pub enum AstNode {
    Scalar(f64),
    Var(String),
    Unary {
        op: String,
        arg: Box<AstNode>,
    },
    Binary {
        op: String,
        left: Box<AstNode>,
        right: Box<AstNode>,
    },
    Func {
        name: String,
        args: Vec<AstNode>,
    },
}

/// Parsed math expression (`GF pkg/expr/mathexp/exp.go:Expr`).
#[derive(Debug, Clone, PartialEq)]
pub struct MathExpr {
    pub raw: String,
    pub root: AstNode,
    pub var_names: Vec<String>,
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    max_depth: usize,
    var_names: Vec<String>,
}

const zero_arg_funcs: &[&str] = &["nan", "inf", "infn", "null"];
const one_arg_funcs: &[&str] = &[
    "abs",
    "log",
    "is_nan",
    "is_inf",
    "is_null",
    "is_number",
    "round",
    "ceil",
    "floor",
];

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn enter_depth(&mut self) -> CrResult<()> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err(CrError::ExpressionError(format!(
                "expression exceeds maximum nesting depth of {}",
                self.max_depth
            )));
        }
        Ok(())
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    // O -> A { "||" A }
    fn parse_or(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.next();
            let right = self.parse_and()?;
            left = AstNode::Binary {
                op: "||".into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // A -> C { "&&" C }
    fn parse_and(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_cmp()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.next();
            let right = self.parse_cmp()?;
            left = AstNode::Binary {
                op: "&&".into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // C -> P { ("==" | "!=" | ">" | ">=" | "<" | "<=") P }
    fn parse_cmp(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Some(Token::Eq) => "==",
                Some(Token::Ne) => "!=",
                Some(Token::Gt) => ">",
                Some(Token::Ge) => ">=",
                Some(Token::Lt) => "<",
                Some(Token::Le) => "<=",
                _ => break,
            };
            self.next();
            let right = self.parse_add()?;
            left = AstNode::Binary {
                op: op.into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // P -> M { ("+" | "-") M }
    fn parse_add(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => "+",
                Some(Token::Minus) => "-",
                _ => break,
            };
            self.next();
            let right = self.parse_mul()?;
            left = AstNode::Binary {
                op: op.into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // M -> E { ("*" | "/" | "%") E }
    fn parse_mul(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek() {
                Some(Token::Mul) => "*",
                Some(Token::Div) => "/",
                Some(Token::Mod) => "%",
                _ => break,
            };
            self.next();
            let right = self.parse_pow()?;
            left = AstNode::Binary {
                op: op.into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // E -> F { "**" F }
    fn parse_pow(&mut self) -> CrResult<AstNode> {
        let mut left = self.parse_factor()?;
        while matches!(self.peek(), Some(Token::Pow)) {
            self.next();
            let right = self.parse_factor()?;
            left = AstNode::Binary {
                op: "**".into(),
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    // F -> v | "(" O ")" | "!" F | "-" F
    fn parse_factor(&mut self) -> CrResult<AstNode> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.next();
                Ok(AstNode::Scalar(n))
            }
            Some(Token::Var(v)) => {
                self.next();
                if !self.var_names.contains(&v) {
                    self.var_names.push(v.clone());
                }
                Ok(AstNode::Var(v))
            }
            Some(Token::Ident(name)) => {
                self.next();
                self.enter_depth()?;
                let res = self.parse_func(name);
                self.leave_depth();
                res
            }
            Some(Token::Not) => {
                self.next();
                self.enter_depth()?;
                let arg = self.parse_factor();
                self.leave_depth();
                Ok(AstNode::Unary {
                    op: "!".into(),
                    arg: Box::new(arg?),
                })
            }
            Some(Token::Minus) => {
                self.next();
                self.enter_depth()?;
                let arg = self.parse_factor();
                self.leave_depth();
                Ok(AstNode::Unary {
                    op: "-".into(),
                    arg: Box::new(arg?),
                })
            }
            Some(Token::LParen) => {
                self.next();
                self.enter_depth()?;
                let inner = self.parse_or();
                self.leave_depth();
                let inner = inner?;
                match self.next() {
                    Some(Token::RParen) => Ok(inner),
                    other => Err(CrError::ExpressionError(format!(
                        "expected ')', got {other:?}"
                    ))),
                }
            }
            other => Err(CrError::ExpressionError(format!(
                "unexpected token in math expression: {other:?}"
            ))),
        }
    }

    fn parse_func(&mut self, name: String) -> CrResult<AstNode> {
        let is_zero = zero_arg_funcs.contains(&name.as_str());
        let is_one = one_arg_funcs.contains(&name.as_str());
        if !is_zero && !is_one {
            return Err(CrError::ExpressionError(format!(
                "non existent function {name}"
            )));
        }
        match self.next() {
            Some(Token::LParen) => {}
            other => {
                return Err(CrError::ExpressionError(format!(
                    "expected '(' after function {name}, got {other:?}"
                )));
            }
        }
        let mut args = Vec::new();
        if !matches!(self.peek(), Some(Token::RParen)) {
            loop {
                args.push(self.parse_or()?);
                if matches!(self.peek(), Some(Token::Comma)) {
                    self.next();
                } else {
                    break;
                }
            }
        }
        match self.next() {
            Some(Token::RParen) => {}
            other => {
                return Err(CrError::ExpressionError(format!(
                    "expected ')' closing function {name}, got {other:?}"
                )));
            }
        }
        let expected_args = if is_zero { 0 } else { 1 };
        if args.len() != expected_args {
            return Err(CrError::ExpressionError(format!(
                "function {name} expects {expected_args} argument(s), got {}",
                args.len()
            )));
        }
        Ok(AstNode::Func { name, args })
    }
}

impl MathExpr {
    /// Parses a math expression with the default depth cap (`MAX_PARSE_DEPTH = 100`).
    pub fn parse(expr: &str) -> CrResult<Self> {
        Self::parse_with_max_depth(expr, MAX_PARSE_DEPTH)
    }

    /// Parses a math expression enforcing `max_depth`.
    pub fn parse_with_max_depth(expr: &str, max_depth: usize) -> CrResult<Self> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return Err(CrError::ExpressionError(
                "math expression must not be empty".into(),
            ));
        }
        let tokens = tokenize(trimmed)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            depth: 0,
            max_depth,
            var_names: Vec::new(),
        };
        let root = parser.parse_or()?;
        if parser.pos != parser.tokens.len() {
            return Err(CrError::ExpressionError(format!(
                "unexpected trailing token {:?}",
                parser.tokens.get(parser.pos)
            )));
        }
        Ok(Self {
            raw: trimmed.to_owned(),
            root,
            var_names: parser.var_names,
        })
    }

    /// Evaluates the parsed expression against `vars`.
    pub fn eval(
        &self,
        ref_id: &str,
        vars: &HashMap<String, Vec<ExprValue>>,
    ) -> CrResult<Vec<ExprValue>> {
        eval_node(&self.root, ref_id, vars)
    }
}

/// Convenience helper to parse and evaluate `expr` against `vars`.
pub fn eval_math_on_vars(
    ref_id: &str,
    expr: &str,
    vars: &HashMap<String, Vec<ExprValue>>,
    max_depth: usize,
) -> CrResult<Vec<ExprValue>> {
    let parsed = MathExpr::parse_with_max_depth(expr, max_depth)?;
    parsed.eval(ref_id, vars)
}

fn unary_op(op: &str, a: f64) -> CrResult<f64> {
    if a.is_nan() {
        return Ok(f64::NAN);
    }
    match op {
        "!" => Ok(if a == 0.0 { 1.0 } else { 0.0 }),
        "-" => Ok(-a),
        _ => Err(CrError::ExpressionError(format!(
            "unknown unary operator {op}"
        ))),
    }
}

fn binary_op(op: &str, a: f64, b: f64) -> CrResult<f64> {
    // Short-circuit logical operators before NaN check (`exp.go:391`)
    match op {
        "||" if a != 0.0 && !a.is_nan() => return Ok(1.0),
        "&&" if a == 0.0 => return Ok(0.0),
        _ => {}
    }
    if a.is_nan() || b.is_nan() {
        return Ok(f64::NAN);
    }
    let r = match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => a / b,
        "**" => a.powf(b),
        "%" => a % b,
        "==" => {
            if (a - b).abs() == 0.0 {
                1.0
            } else {
                0.0
            }
        }
        "!=" => {
            if (a - b).abs() != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        ">" => {
            if a > b {
                1.0
            } else {
                0.0
            }
        }
        ">=" => {
            if a >= b {
                1.0
            } else {
                0.0
            }
        }
        "<" => {
            if a < b {
                1.0
            } else {
                0.0
            }
        }
        "<=" => {
            if a <= b {
                1.0
            } else {
                0.0
            }
        }
        "||" => {
            if a != 0.0 || b != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        "&&" => {
            if a != 0.0 && b != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        _ => {
            return Err(CrError::ExpressionError(format!(
                "unknown binary operator {op}"
            )));
        }
    };
    Ok(r)
}

fn labels_contains(superset: &BTreeMap<String, String>, subset: &BTreeMap<String, String>) -> bool {
    subset.iter().all(|(k, v)| superset.get(k) == Some(v))
}

struct UnionPair<'a> {
    labels: BTreeMap<String, String>,
    a: &'a ExprValue,
    b: &'a ExprValue,
}

fn union_values<'a>(a_vals: &'a [ExprValue], b_vals: &'a [ExprValue]) -> (Vec<UnionPair<'a>>, usize) {
    if a_vals.is_empty() || b_vals.is_empty() {
        return (Vec::new(), 0);
    }
    if (a_vals.len() == 1 && matches!(a_vals[0], ExprValue::NoData))
        || (b_vals.len() == 1 && matches!(b_vals[0], ExprValue::NoData))
    {
        return (
            vec![UnionPair {
                labels: BTreeMap::new(),
                a: &a_vals[0],
                b: &b_vals[0],
            }],
            0,
        );
    }

    let mut a_matched = vec![false; a_vals.len()];
    let mut b_matched = vec![false; b_vals.len()];
    let mut unions = Vec::new();

    for (ia, a) in a_vals.iter().enumerate() {
        for (ib, b) in b_vals.iter().enumerate() {
            let al = a.labels();
            let bl = b.labels();
            let labels = if al == bl || al.is_empty() || bl.is_empty() {
                if al.is_empty() {
                    bl.clone()
                } else {
                    al.clone()
                }
            } else if al.len() == bl.len() {
                continue;
            } else if labels_contains(al, bl) {
                al.clone()
            } else if labels_contains(bl, al) {
                bl.clone()
            } else {
                continue;
            };
            unions.push(UnionPair { labels, a, b });
            a_matched[ia] = true;
            b_matched[ib] = true;
        }
    }

    if unions.is_empty() && a_vals.len() == 1 && b_vals.len() == 1 {
        unions.push(UnionPair {
            labels: BTreeMap::new(),
            a: &a_vals[0],
            b: &b_vals[0],
        });
        a_matched[0] = true;
        b_matched[0] = true;
    }

    let dropped = a_matched.iter().filter(|m| !**m).count()
        + b_matched.iter().filter(|m| !**m).count();
    (unions, dropped)
}

fn eval_node(
    node: &AstNode,
    ref_id: &str,
    vars: &HashMap<String, Vec<ExprValue>>,
) -> CrResult<Vec<ExprValue>> {
    match node {
        AstNode::Scalar(v) => Ok(vec![ExprValue::Scalar(Some(*v))]),
        AstNode::Var(name) => Ok(vars
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![ExprValue::NoData])),
        AstNode::Unary { op, arg } => {
            let vals = eval_node(arg, ref_id, vars)?;
            let mut out = Vec::with_capacity(vals.len());
            for val in vals {
                match val {
                    ExprValue::Scalar(Some(f)) => {
                        out.push(ExprValue::Scalar(Some(unary_op(op, f)?)));
                    }
                    ExprValue::Scalar(None) => out.push(ExprValue::Scalar(None)),
                    ExprValue::Number {
                        labels,
                        value,
                        notices,
                        ..
                    } => {
                        let v = match value {
                            Some(f) => Some(unary_op(op, f)?),
                            None => None,
                        };
                        out.push(ExprValue::Number {
                            name: ref_id.to_owned(),
                            labels,
                            value: v,
                            notices,
                        });
                    }
                    ExprValue::Series {
                        labels,
                        points,
                        notices,
                        ..
                    } => {
                        let mut new_pts = Vec::with_capacity(points.len());
                        for (t, v) in points {
                            let nv = match v {
                                Some(f) => Some(unary_op(op, f)?),
                                None => None,
                            };
                            new_pts.push((t, nv));
                        }
                        out.push(ExprValue::Series {
                            name: ref_id.to_owned(),
                            labels,
                            points: new_pts,
                            notices,
                        });
                    }
                    ExprValue::NoData => out.push(ExprValue::NoData),
                }
            }
            Ok(out)
        }
        AstNode::Binary { op, left, right } => {
            let a_vals = eval_node(left, ref_id, vars)?;
            let b_vals = eval_node(right, ref_id, vars)?;
            let (unions, dropped) = union_values(&a_vals, &b_vals);
            let mut out = Vec::with_capacity(unions.len());
            for u in unions {
                let v = eval_binary_pair(ref_id, op, u.labels, u.a, u.b)?;
                out.push(v);
            }
            if dropped > 0 {
                if let Some(first) = out.first_mut() {
                    let notice = Notice {
                        severity: NoticeSeverity::Warning,
                        text: format!("{dropped} items dropped from union(s)"),
                    };
                    match first {
                        ExprValue::Number { notices, .. } | ExprValue::Series { notices, .. } => {
                            notices.push(notice);
                        }
                        _ => {}
                    }
                }
            }
            Ok(out)
        }
        AstNode::Func { name, args } => eval_func(name, args, ref_id, vars),
    }
}

fn eval_binary_pair(
    ref_id: &str,
    op: &str,
    labels: BTreeMap<String, String>,
    a: &ExprValue,
    b: &ExprValue,
) -> CrResult<ExprValue> {
    match (a, b) {
        (ExprValue::NoData, _) | (_, ExprValue::NoData) => Ok(ExprValue::NoData),
        (ExprValue::Scalar(af), ExprValue::Scalar(bf)) => match (af, bf) {
            (Some(x), Some(y)) => Ok(ExprValue::Scalar(Some(binary_op(op, *x, *y)?))),
            _ => Ok(ExprValue::Scalar(None)),
        },
        (ExprValue::Scalar(af), ExprValue::Number { value: bf, .. }) => {
            let v = match (af, bf) {
                (Some(x), Some(y)) => Some(binary_op(op, *x, *y)?),
                _ => None,
            };
            Ok(ExprValue::Number {
                name: ref_id.to_owned(),
                labels,
                value: v,
                notices: Vec::new(),
            })
        }
        (ExprValue::Number { value: af, .. }, ExprValue::Scalar(bf))
        | (ExprValue::Number { value: af, .. }, ExprValue::Number { value: bf, .. }) => {
            let v = match (af, bf) {
                (Some(x), Some(y)) => Some(binary_op(op, *x, *y)?),
                _ => None,
            };
            Ok(ExprValue::Number {
                name: ref_id.to_owned(),
                labels,
                value: v,
                notices: Vec::new(),
            })
        }
        (
            ExprValue::Series { points, .. },
            ExprValue::Scalar(bf) | ExprValue::Number { value: bf, .. },
        ) => {
            let mut new_pts = Vec::with_capacity(points.len());
            for (t, af) in points {
                let v = match (af, bf) {
                    (Some(x), Some(y)) => Some(binary_op(op, *x, *y)?),
                    _ => None,
                };
                new_pts.push((*t, v));
            }
            Ok(ExprValue::Series {
                name: ref_id.to_owned(),
                labels,
                points: new_pts,
                notices: Vec::new(),
            })
        }
        (
            ExprValue::Scalar(af) | ExprValue::Number { value: af, .. },
            ExprValue::Series { points, .. },
        ) => {
            let mut new_pts = Vec::with_capacity(points.len());
            for (t, bf) in points {
                let v = match (af, bf) {
                    (Some(x), Some(y)) => Some(binary_op(op, *x, *y)?),
                    _ => None,
                };
                new_pts.push((*t, v));
            }
            Ok(ExprValue::Series {
                name: ref_id.to_owned(),
                labels,
                points: new_pts,
                notices: Vec::new(),
            })
        }
        (
            ExprValue::Series {
                points: a_pts, ..
            },
            ExprValue::Series {
                points: b_pts, ..
            },
        ) => {
            let b_map: HashMap<i64, Option<f64>> = b_pts.iter().copied().collect();
            let mut new_pts = Vec::new();
            for (t, af) in a_pts {
                let Some(bf) = b_map.get(t) else {
                    continue;
                };
                let v = match (af, bf) {
                    (Some(x), Some(y)) => Some(binary_op(op, *x, *y)?),
                    _ => None,
                };
                new_pts.push((*t, v));
            }
            Ok(ExprValue::Series {
                name: ref_id.to_owned(),
                labels,
                points: new_pts,
                notices: Vec::new(),
            })
        }
    }
}

fn eval_func(
    name: &str,
    args: &[AstNode],
    ref_id: &str,
    vars: &HashMap<String, Vec<ExprValue>>,
) -> CrResult<Vec<ExprValue>> {
    match name {
        "nan" => Ok(vec![ExprValue::Scalar(Some(f64::NAN))]),
        "inf" => Ok(vec![ExprValue::Scalar(Some(f64::INFINITY))]),
        "infn" => Ok(vec![ExprValue::Scalar(Some(f64::NEG_INFINITY))]),
        "null" => Ok(vec![ExprValue::Scalar(None)]),
        "is_null" => {
            let input = eval_node(&args[0], ref_id, vars)?;
            Ok(input
                .into_iter()
                .map(|v| map_nullable(&v, ref_id, |f| Some(if f.is_none() { 1.0 } else { 0.0 })))
                .collect())
        }
        "is_number" => {
            let input = eval_node(&args[0], ref_id, vars)?;
            Ok(input
                .into_iter()
                .map(|v| {
                    map_nullable(&v, ref_id, |f| {
                        let ok = f.is_some_and(|x| x.is_finite());
                        Some(if ok { 1.0 } else { 0.0 })
                    })
                })
                .collect())
        }
        other => {
            let func: fn(f64) -> f64 = match other {
                "abs" => f64::abs,
                "log" => f64::ln,
                "round" => f64::round,
                "ceil" => f64::ceil,
                "floor" => f64::floor,
                "is_nan" => |f| if f.is_nan() { 1.0 } else { 0.0 },
                "is_inf" => |f| if f.is_infinite() { 1.0 } else { 0.0 },
                _ => {
                    return Err(CrError::ExpressionError(format!(
                        "unknown function {other}"
                    )));
                }
            };
            let input = eval_node(&args[0], ref_id, vars)?;
            Ok(input
                .into_iter()
                .map(|v| {
                    map_nullable(&v, ref_id, |f| match f {
                        Some(x) => Some(func(x)),
                        None => Some(f64::NAN),
                    })
                })
                .collect())
        }
    }
}

fn map_nullable(
    val: &ExprValue,
    ref_id: &str,
    f: impl Fn(Option<f64>) -> Option<f64>,
) -> ExprValue {
    match val {
        ExprValue::Scalar(v) => ExprValue::Scalar(f(*v)),
        ExprValue::Number {
            labels,
            value,
            notices,
            ..
        } => ExprValue::Number {
            name: ref_id.to_owned(),
            labels: labels.clone(),
            value: f(*value),
            notices: notices.clone(),
        },
        ExprValue::Series {
            labels,
            points,
            notices,
            ..
        } => ExprValue::Series {
            name: ref_id.to_owned(),
            labels: labels.clone(),
            points: points.iter().map(|(t, v)| (*t, f(*v))).collect(),
            notices: notices.clone(),
        },
        ExprValue::NoData => ExprValue::NoData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_parser_and_functions_match_grafana_semantics() {
        let mut vars = HashMap::new();
        let mut labels = BTreeMap::new();
        labels.insert("host".into(), "web-1".into());
        vars.insert(
            "A".into(),
            vec![ExprValue::Series {
                name: "A".into(),
                labels: labels.clone(),
                points: vec![(1000, Some(-4.2)), (2000, Some(9.8))],
                notices: Vec::new(),
            }],
        );
        vars.insert(
            "B".into(),
            vec![ExprValue::Series {
                name: "B".into(),
                labels,
                points: vec![(1000, Some(2.0)), (2000, Some(0.2))],
                notices: Vec::new(),
            }],
        );

        let res = eval_math_on_vars("C", "round(abs($A) + ${B} * 2)", &vars, 100).unwrap();
        let ExprValue::Series { points, .. } = &res[0] else {
            panic!("expected series");
        };
        assert_eq!(points, &[(1000, Some(8.0)), (2000, Some(10.0))]);

        let null_check = eval_math_on_vars("D", "is_null(null()) && !is_number(nan())", &vars, 100)
            .unwrap();
        assert_eq!(null_check, vec![ExprValue::Scalar(Some(1.0))]);
    }

    #[test]
    fn deeply_nested_expression_exceeding_cap_is_refused() {
        let deep = format!("{}$A{}", "(".repeat(105), ")".repeat(105));
        let err = MathExpr::parse(&deep).unwrap_err();
        assert!(err.to_string().contains("exceeds maximum nesting depth"));
    }
}
