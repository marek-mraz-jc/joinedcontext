use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::ast::{ArgList, Assignment, BinaryOp, Expr, MapDef, MatchCase, MatchPattern, Statement};
use crate::env::Environment;
use crate::error::ParseError;
use crate::executor::{Executor, FieldExpression, FieldResolver};
use crate::value::{dot_path_to_slice, escape_quoted_path_segment, Value};

struct ParserState<'a> {
    env: &'a Environment,
    input: &'a str,
    pos: usize,
    named_contexts: Vec<String>,
    next_state_id: usize,
}

impl<'a> ParserState<'a> {
    fn new(env: &'a Environment, input: &'a str) -> Self {
        Self {
            env,
            input,
            pos: 0,
            named_contexts: Vec::new(),
            next_state_id: 1,
        }
    }

    fn alloc_state_id(&mut self) -> usize {
        let id = self.next_state_id;
        self.next_state_id += 1;
        id
    }

    fn has_named_context(&self, name: &str) -> bool {
        self.named_contexts.iter().any(|n| n == name)
    }

    fn rem(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.rem().chars().next()
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError::from_offset(self.input, self.pos, msg)
    }

    fn err_at(&self, offset: usize, msg: impl Into<String>) -> ParseError {
        ParseError::from_offset(self.input, offset, msg)
    }

    /// The statement line Bento reports: it counts newlines in the input's first `n` bytes where
    /// `n` is the statement's offset in characters, so every multibyte character before a
    /// statement moves the count back (a BOM in line 1 makes line 2 report as line 1).
    fn line_at(&self, offset: usize) -> usize {
        let clamped = offset.min(self.input.len());
        let chars = self.input[..clamped].chars().count();
        self.input.as_bytes()[..chars]
            .iter()
            .copied()
            .filter(|&b| b == b'\n')
            .count()
            + 1
    }

    fn skip_spaces_and_tabs(&mut self) {
        while let Some(c) = self.peek_char() {
            if c == ' ' || c == '\t' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            let start = self.pos;
            while let Some(c) = self.peek_char() {
                if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                    self.pos += c.len_utf8();
                } else {
                    break;
                }
            }
            if self.rem().starts_with('#') {
                if let Some(nl) = self.rem().find('\n') {
                    self.pos += nl + 1;
                } else {
                    self.pos = self.input.len();
                }
            }
            if self.pos == start {
                break;
            }
        }
    }

    fn consume_newline_or_comment(&mut self) -> bool {
        self.skip_spaces_and_tabs();
        if self.rem().starts_with('#') {
            if let Some(nl) = self.rem().find('\n') {
                self.pos += nl + 1;
                return true;
            } else {
                self.pos = self.input.len();
                return true;
            }
        }
        if let Some(after) = self.rem().strip_prefix("\r\n") {
            self.pos = self.input.len() - after.len();
            return true;
        }
        if let Some(after) = self.rem().strip_prefix('\n') {
            self.pos = self.input.len() - after.len();
            return true;
        }
        false
    }

    fn starts_with_keyword(&self, kw: &str) -> bool {
        let r = self.rem();
        if !r.starts_with(kw) {
            return false;
        }
        let after = &r[kw.len()..];
        !after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    fn parse_identifier(&mut self) -> Option<String> {
        let r = self.rem();
        let len = r
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if len == 0 {
            return None;
        }
        let ident = r[..len].to_string();
        self.pos += len;
        Some(ident)
    }

    fn parse_snake_case(&mut self) -> Option<String> {
        let r = self.rem();
        let len = r
            .bytes()
            .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
            .count();
        if len == 0 {
            return None;
        }
        let s = r[..len].to_string();
        self.pos += len;
        Some(s)
    }

    fn parse_quoted_string(&mut self) -> Result<String, ParseError> {
        if self.rem().starts_with("\"\"\"") {
            self.pos += 3;
            let rem = self.rem();
            if let Some(end_idx) = rem.find("\"\"\"") {
                let content = rem[..end_idx].to_string();
                self.pos += end_idx + 3;
                return Ok(content);
            }
            return Err(self.err("required: expected end triple-quote"));
        }
        if !self.rem().starts_with('"') {
            return Err(self.err("expected quoted string"));
        }
        let start = self.pos;
        let bytes = self.rem().as_bytes();
        let mut i = 1usize;
        let mut escaped = false;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'"' && !escaped {
                let raw = &self.input[start..start + i + 1];
                self.pos = start + i + 1;
                return unescape_go_quoted_string(raw).map_err(|e| {
                    self.err_at(
                        start,
                        format!("failed to unescape quoted string contents: {e}"),
                    )
                });
            }
            if b == b'\n' {
                return Err(self.err("required: expected end quote"));
            }
            if b == b'\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
            i += 1;
        }
        Err(self.err("required: expected end quote"))
    }
}

fn unescape_go_quoted_string(raw: &str) -> Result<String, String> {
    if let Ok(s) = serde_json::from_str::<String>(raw) {
        return Ok(s);
    }
    // Support Go-specific escapes like `\xFF`, `\a`, `\v`, `\'`
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(esc) = chars.next() else {
            return Err("trailing backslash".to_string());
        };
        match esc {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'v' => out.push('\x0b'),
            'x' => {
                let h1 = chars.next().ok_or("short \\x escape")?;
                let h2 = chars.next().ok_or("short \\x escape")?;
                let hex_s = format!("{h1}{h2}");
                let b = u8::from_str_radix(&hex_s, 16).map_err(|e| e.to_string())?;
                out.push(b as char);
            }
            'u' => {
                let mut hex_s = String::with_capacity(4);
                for _ in 0..4 {
                    hex_s.push(chars.next().ok_or("short \\u escape")?);
                }
                let code = u32::from_str_radix(&hex_s, 16).map_err(|e| e.to_string())?;
                let ch = char::from_u32(code).ok_or("invalid unicode codepoint")?;
                out.push(ch);
            }
            'U' => {
                let mut hex_s = String::with_capacity(8);
                for _ in 0..8 {
                    hex_s.push(chars.next().ok_or("short \\U escape")?);
                }
                let code = u32::from_str_radix(&hex_s, 16).map_err(|e| e.to_string())?;
                let ch = char::from_u32(code).ok_or("invalid unicode codepoint")?;
                out.push(ch);
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Ok(out)
}

pub fn parse_mapping(env: &Environment, input: &str) -> Result<Executor, ParseError> {
    let mut state = ParserState::new(env, input);
    state.skip_ws_and_comments();

    // 1. Check `from "<path>"` single root import
    if state.starts_with_keyword("from") {
        let saved = state.pos;
        state.pos += 4;
        state.skip_spaces_and_tabs();
        if state.rem().starts_with('"') {
            let path = state.parse_quoted_string()?;
            state.skip_ws_and_comments();
            if state.is_eof() {
                let bytes = env
                    .importer
                    .import(&path)
                    .map_err(|e| state.err_at(saved, format!("failed to read import: {e}")))?;
                let imported_str = String::from_utf8(bytes)
                    .map_err(|e| state.err_at(saved, format!("invalid UTF-8 in import: {e}")))?;
                return parse_mapping(env, &imported_str);
            }
        }
        state.pos = saved;
    }

    // 2. Try parsing as multi-statement executor vs single-root query expression (`BestMatch`)
    let mut stmt_state = ParserState::new(env, input);
    let mut maps: HashMap<String, Arc<MapDef>> = HashMap::new();
    let stmt_res = parse_statements_block(&mut stmt_state, true, Some(&mut maps), false);

    let mut single_state = ParserState::new(env, input);
    single_state.skip_ws_and_comments();
    let single_res = parse_query(&mut single_state).and_then(|expr| {
        single_state.skip_ws_and_comments();
        if single_state.is_eof() {
            Ok(expr)
        } else {
            Err(single_state.err("expected end of single root query expression"))
        }
    });

    match (stmt_res, single_res) {
        (Ok(statements), _) if stmt_state.is_eof() => Ok(Executor::new(
            String::new(),
            Arc::new(maps),
            statements,
            env.clone(),
        )),
        (_, Ok(expr)) => {
            let stmt = Statement::Single {
                line: 1,
                assignment: Assignment::Json(SmallVec::new()),
                query: expr,
            };
            Ok(Executor::new(
                String::new(),
                Arc::new(HashMap::new()),
                vec![stmt],
                env.clone(),
            ))
        }
        (Err(e1), Err(e2)) => {
            // Return the error that advanced further through the input
            if (e2.line, e2.col) > (e1.line, e1.col) {
                Err(e2)
            } else {
                Err(e1)
            }
        }
        (Ok(_), Err(e)) => Err(e),
    }
}

pub fn parse_field(env: &Environment, expr: &str) -> Result<FieldExpression, ParseError> {
    let mut resolvers = Vec::new();
    let mut static_buf = String::new();
    let mut pos = 0usize;

    while pos < expr.len() {
        let rem = &expr[pos..];
        if let Some(after_esc) = rem.strip_prefix("${{!") {
            if let Some(end_idx) = after_esc.find("}}") {
                static_buf.push_str("${!");
                static_buf.push_str(&after_esc[..end_idx]);
                static_buf.push('}');
                pos += 4 + end_idx + 2;
                continue;
            } else {
                return Err(ParseError::from_offset(
                    expr,
                    pos,
                    "required: expected end of escaped expression",
                ));
            }
        }
        if rem.starts_with("${!") {
            if !static_buf.is_empty() {
                resolvers.push(FieldResolver::Static(std::mem::take(&mut static_buf)));
            }
            pos += 3;
            let mut qstate = ParserState::new(env, &expr[pos..]);
            qstate.skip_spaces_and_tabs();
            let qexpr = parse_query(&mut qstate)
                .map_err(|e| ParseError::from_offset(expr, pos + qstate.pos, e.message))?;
            qstate.skip_spaces_and_tabs();
            if !qstate.rem().starts_with('}') {
                return Err(ParseError::from_offset(
                    expr,
                    pos + qstate.pos,
                    "required: expected end of expression '}'",
                ));
            }
            pos += qstate.pos + 1;
            resolvers.push(FieldResolver::Query(Box::new(qexpr)));
            continue;
        }
        let ch = rem.chars().next().unwrap();
        static_buf.push(ch);
        pos += ch.len_utf8();
    }

    if !static_buf.is_empty() {
        resolvers.push(FieldResolver::Static(static_buf));
    }

    Ok(FieldExpression::new(resolvers, env.clone()))
}

fn parse_statements_block(
    state: &mut ParserState<'_>,
    enable_meta: bool,
    mut maps: Option<&mut HashMap<String, Arc<MapDef>>>,
    stop_on_brace: bool,
) -> Result<Vec<Statement>, ParseError> {
    let mut statements = Vec::new();
    state.skip_ws_and_comments();
    if state.is_eof() || (stop_on_brace && state.rem().starts_with('}')) {
        return Ok(statements);
    }

    loop {
        state.skip_ws_and_comments();
        if state.is_eof() || (stop_on_brace && state.rem().starts_with('}')) {
            break;
        }

        if state.starts_with_keyword("import") {
            let err_pos = state.pos;
            state.pos += 6;
            state.skip_spaces_and_tabs();
            let path = state.parse_quoted_string()?;
            let Some(ref mut map_table) = maps else {
                return Err(state.err_at(
                    err_pos,
                    "importing mappings is not allowed within this block",
                ));
            };
            let bytes = state
                .env
                .importer
                .import(&path)
                .map_err(|e| state.err_at(err_pos, format!("failed to read import: {e}")))?;
            let content = String::from_utf8(bytes)
                .map_err(|e| state.err_at(err_pos, format!("invalid UTF-8 in import: {e}")))?;
            let mut sub_state = ParserState::new(state.env, &content);
            let mut imported_maps = HashMap::new();
            let _ = parse_statements_block(&mut sub_state, true, Some(&mut imported_maps), false)?;
            if imported_maps.is_empty() {
                return Err(state.err_at(err_pos, format!("no maps to import from '{path}'")));
            }
            for (k, v) in imported_maps {
                if map_table.contains_key(&k) {
                    return Err(state.err_at(
                        err_pos,
                        format!("map name collisions from import '{path}': [{k}]"),
                    ));
                }
                map_table.insert(k, v);
            }
        } else if state.starts_with_keyword("map") {
            let err_pos = state.pos;
            state.pos += 3;
            state.skip_spaces_and_tabs();
            let map_name = if state.rem().starts_with('"') {
                state.parse_quoted_string()?
            } else {
                state
                    .parse_identifier()
                    .ok_or_else(|| state.err("expected map name"))?
            };
            state.skip_spaces_and_tabs();
            if !state.rem().starts_with('{') {
                return Err(state.err("expected '{' starting map body"));
            }
            state.pos += 1;
            let body_stmts = parse_statements_block(state, false, None, true)?;
            state.skip_ws_and_comments();
            if !state.rem().starts_with('}') {
                return Err(state.err("expected '}' closing map body"));
            }
            state.pos += 1;
            let Some(ref mut map_table) = maps else {
                return Err(state.err_at(err_pos, "defining maps is not allowed within this block"));
            };
            if map_table.contains_key(&map_name) {
                return Err(state.err_at(err_pos, format!("map name collision: {map_name}")));
            }
            map_table.insert(
                map_name.clone(),
                Arc::new(MapDef {
                    name: map_name,
                    statements: body_stmts,
                }),
            );
        } else {
            let stmt = parse_single_or_if_statement(state, enable_meta)?;
            statements.push(stmt);
        }

        state.skip_spaces_and_tabs();
        if state.is_eof() || (stop_on_brace && state.rem().starts_with('}')) {
            break;
        }
        if !state.consume_newline_or_comment() {
            if stop_on_brace && state.rem().starts_with('}') {
                break;
            }
            return Err(state.err("expected line break between statements"));
        }
    }

    Ok(statements)
}

fn parse_single_or_if_statement(
    state: &mut ParserState<'_>,
    enable_meta: bool,
) -> Result<Statement, ParseError> {
    let stmt_offset = state.pos;
    let line = state.line_at(stmt_offset);

    if state.starts_with_keyword("if") {
        // Could be root-level `if cond { stmt1 \n stmt2 }` OR single root expression `if cond { expr }`.
        // Try root-level if statement first:
        let saved = state.pos;
        if let Ok(if_stmt) = parse_root_if_statement(state, enable_meta, line) {
            return Ok(if_stmt);
        }
        state.pos = saved;
    }

    if state.starts_with_keyword("let") {
        state.pos += 3;
        state.skip_spaces_and_tabs();
        let var_name = if state.rem().starts_with('"') {
            state.parse_quoted_string()?
        } else {
            // Also accept optional leading `$` if someone writes `let $foo = ...`
            if state.rem().starts_with('$') {
                state.pos += 1;
            }
            state
                .parse_identifier()
                .ok_or_else(|| state.err("expected variable name"))?
        };
        state.skip_spaces_and_tabs();
        if !state.rem().starts_with('=') {
            return Err(state.err("expected '=' in let assignment"));
        }
        state.pos += 1;
        state.skip_spaces_and_tabs();
        let query = parse_query(state)?;
        return Ok(Statement::Single {
            line,
            assignment: Assignment::Var(var_name),
            query,
        });
    }

    if state.starts_with_keyword("meta") {
        let saved = state.pos;
        state.pos += 4;
        state.skip_spaces_and_tabs();
        let key_opt = if state.rem().starts_with('=') {
            None
        } else if state.rem().starts_with('"') {
            Some(state.parse_quoted_string()?)
        } else {
            state.parse_identifier()
        };
        state.skip_spaces_and_tabs();
        if state.rem().starts_with('=') && !state.rem().starts_with("==") {
            if !enable_meta {
                return Err(state.err_at(
                    saved,
                    "setting meta fields is not allowed within this block",
                ));
            }
            state.pos += 1;
            state.skip_spaces_and_tabs();
            let query = parse_query(state)?;
            return Ok(Statement::Single {
                line,
                assignment: Assignment::Meta(key_opt),
                query,
            });
        }
        state.pos = saved;
    }

    // Plain JSON path assignment: `root.a.b = ...` or `root = ...` or implicit `a.b = ...`
    let path = parse_assignment_path(state)?;
    state.skip_spaces_and_tabs();
    if !state.rem().starts_with('=') || state.rem().starts_with("==") {
        return Err(state.err("expected '=' in assignment"));
    }
    state.pos += 1;
    state.skip_spaces_and_tabs();
    let query = parse_query(state)?;
    Ok(Statement::Single {
        line,
        assignment: Assignment::Json(path),
        query,
    })
}

fn parse_root_if_statement(
    state: &mut ParserState<'_>,
    enable_meta: bool,
    line: usize,
) -> Result<Statement, ParseError> {
    if !state.starts_with_keyword("if") {
        return Err(state.err("expected 'if'"));
    }
    state.pos += 2;
    state.skip_spaces_and_tabs();
    let cond = parse_query(state)?;
    state.skip_ws_and_comments();
    if !state.rem().starts_with('{') {
        return Err(state.err("expected '{' after if condition"));
    }
    state.pos += 1;
    // Bento accepts an empty statement block (`if cond { } else { ... }`).
    let then_stmts = parse_statements_block(state, enable_meta, None, true)?;
    state.skip_ws_and_comments();
    if !state.rem().starts_with('}') {
        return Err(state.err("expected '}' closing if block"));
    }
    state.pos += 1;

    let mut branches = vec![(Some(cond), then_stmts)];

    loop {
        let saved = state.pos;
        state.skip_ws_and_comments();
        if state.starts_with_keyword("else") {
            state.pos += 4;
            state.skip_spaces_and_tabs();
            if state.starts_with_keyword("if") {
                state.pos += 2;
                state.skip_spaces_and_tabs();
                let econd = parse_query(state)?;
                state.skip_ws_and_comments();
                if !state.rem().starts_with('{') {
                    return Err(state.err("expected '{' after else if condition"));
                }
                state.pos += 1;
                let estmts = parse_statements_block(state, enable_meta, None, true)?;
                state.skip_ws_and_comments();
                if !state.rem().starts_with('}') {
                    return Err(state.err("expected '}' closing else if block"));
                }
                state.pos += 1;
                branches.push((Some(econd), estmts));
                continue;
            }
            state.skip_ws_and_comments();
            if !state.rem().starts_with('{') {
                return Err(state.err("expected '{' after else"));
            }
            state.pos += 1;
            let estmts = parse_statements_block(state, enable_meta, None, true)?;
            state.skip_ws_and_comments();
            if !state.rem().starts_with('}') {
                return Err(state.err("expected '}' closing else block"));
            }
            state.pos += 1;
            branches.push((None, estmts));
            break;
        } else {
            state.pos = saved;
            break;
        }
    }

    Ok(Statement::If { line, branches })
}

fn parse_assignment_path(state: &mut ParserState<'_>) -> Result<SmallVec<[String; 4]>, ParseError> {
    let first = state
        .parse_identifier()
        .ok_or_else(|| state.err("expected assignment target path"))?;
    let mut segments: SmallVec<[String; 4]> = SmallVec::new();
    segments.push(first);

    while state.rem().starts_with('.') {
        state.pos += 1;
        if state.rem().starts_with('"') {
            let quoted = state.parse_quoted_string()?;
            let escaped = escape_quoted_path_segment(&quoted);
            segments.extend(dot_path_to_slice(&escaped));
        } else {
            let seg = state
                .parse_identifier()
                .ok_or_else(|| state.err("expected target path segment after '.'"))?;
            segments.extend(dot_path_to_slice(&seg));
        }
    }

    if segments.first().is_some_and(|s| s == "root") {
        segments.remove(0);
    }

    Ok(segments)
}

fn parse_query(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        state.skip_spaces_and_tabs();
        let first = parse_unary_with_tails(state)?;
        let mut fns = vec![first];
        let mut ops = Vec::new();

        loop {
            let saved = state.pos;
            state.skip_spaces_and_tabs();
            if let Some(op) = try_parse_binary_op(state) {
                state.skip_ws_and_comments();
                let next = parse_unary_with_tails(state)?;
                ops.push(op);
                fns.push(next);
            } else {
                state.pos = saved;
                break;
            }
        }

        if ops.is_empty() {
            return Ok(fns.pop().unwrap());
        }
        fold_arithmetic_passes(fns, ops)
    })
}

fn try_parse_binary_op(state: &mut ParserState<'_>) -> Option<BinaryOp> {
    let r = state.rem();
    // Do not confuse `=>` or `->` with `>` or `-`!
    if r.starts_with("=>") || r.starts_with("->") {
        return None;
    }
    let candidates: &[(&str, BinaryOp)] = &[
        ("&&", BinaryOp::And),
        ("||", BinaryOp::Or),
        ("==", BinaryOp::Eq),
        ("!=", BinaryOp::Neq),
        (">=", BinaryOp::Gte),
        ("<=", BinaryOp::Lte),
        ("+", BinaryOp::Add),
        ("-", BinaryOp::Sub),
        ("/", BinaryOp::Div),
        ("*", BinaryOp::Mul),
        ("%", BinaryOp::Mod),
        (">", BinaryOp::Gt),
        ("<", BinaryOp::Lt),
        ("|", BinaryOp::Pipe),
    ];
    for (tok, op) in candidates {
        if r.starts_with(tok) {
            state.pos += tok.len();
            return Some(*op);
        }
    }
    None
}

/// Folds binary operators in 4 precedence passes matching `internal/bloblang/query/arithmetic.go`:
/// 1. `*`, `/`, `%`, `|` (Pipe coalesce!)
/// 2. `+`, `-`
/// 3. `==`, `!=`, `<`, `<=`, `>`, `>=`
/// 4. `&&`, `||`
fn fold_arithmetic_passes(mut fns: Vec<Expr>, mut ops: Vec<BinaryOp>) -> Result<Expr, ParseError> {
    // Pass 1: Mul, Div, Mod, Pipe
    {
        let mut next_fns = vec![fns.remove(0)];
        let mut next_ops = Vec::new();
        for (op, right) in ops.into_iter().zip(fns) {
            if matches!(
                op,
                BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod | BinaryOp::Pipe
            ) {
                let left = next_fns.pop().unwrap();
                next_fns.push(Expr::Binary {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                });
            } else {
                next_fns.push(right);
                next_ops.push(op);
            }
        }
        fns = next_fns;
        ops = next_ops;
    }
    if fns.len() == 1 {
        return Ok(fns.pop().unwrap());
    }

    // Pass 2: Add, Sub
    {
        let mut next_fns = vec![fns.remove(0)];
        let mut next_ops = Vec::new();
        for (op, right) in ops.into_iter().zip(fns) {
            if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                let left = next_fns.pop().unwrap();
                next_fns.push(Expr::Binary {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                });
            } else {
                next_fns.push(right);
                next_ops.push(op);
            }
        }
        fns = next_fns;
        ops = next_ops;
    }
    if fns.len() == 1 {
        return Ok(fns.pop().unwrap());
    }

    // Pass 3: Eq, Neq, Gt, Lt, Gte, Lte
    {
        let mut next_fns = vec![fns.remove(0)];
        let mut next_ops = Vec::new();
        for (op, right) in ops.into_iter().zip(fns) {
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Neq
                    | BinaryOp::Gt
                    | BinaryOp::Lt
                    | BinaryOp::Gte
                    | BinaryOp::Lte
            ) {
                let left = next_fns.pop().unwrap();
                next_fns.push(Expr::Binary {
                    op,
                    lhs: Box::new(left),
                    rhs: Box::new(right),
                });
            } else {
                next_fns.push(right);
                next_ops.push(op);
            }
        }
        fns = next_fns;
        ops = next_ops;
    }
    if fns.len() == 1 {
        return Ok(fns.pop().unwrap());
    }

    // Pass 4: And, Or
    {
        let mut acc = fns.remove(0);
        for (op, right) in ops.into_iter().zip(fns) {
            acc = Expr::Binary {
                op,
                lhs: Box::new(acc),
                rhs: Box::new(right),
            };
        }
        Ok(acc)
    }
}

fn parse_unary_with_tails(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    let mut is_neg = false;
    if state.rem().starts_with('-') && !state.rem().starts_with("->") {
        // If followed by digits, `parse_primary` can parse negative number literal directly,
        // or we can treat `-` as `0 - expr` which works for both literals and `-this.max`!
        let after_minus = &state.rem()[1..];
        if !after_minus.starts_with(|c: char| c.is_ascii_digit()) {
            is_neg = true;
            state.pos += 1;
            state.skip_ws_and_comments();
        }
    }

    let mut is_not = false;
    if state.rem().starts_with('!') && !state.rem().starts_with("!=") {
        is_not = true;
        state.pos += 1;
        state.skip_spaces_and_tabs();
    }

    let mut expr = parse_primary(state)?;

    // Parse tails: `[index_or_slice]` and `.method_or_field_or_map`
    loop {
        if state.rem().starts_with('[') {
            expr = parse_bracket_tail(state, expr)?;
            continue;
        }

        let saved = state.pos;
        if state.rem().starts_with('.') {
            state.pos += 1;
            state.skip_ws_and_comments();
            expr = parse_dot_tail(state, expr)?;
            continue;
        }
        state.pos = saved;
        break;
    }

    if is_not {
        expr = Expr::Not(Box::new(expr));
    }
    if is_neg {
        expr = Expr::Binary {
            op: BinaryOp::Sub,
            lhs: Box::new(Expr::Literal(Value::Int(0))),
            rhs: Box::new(expr),
        };
    }

    Ok(expr)
}

fn parse_bracket_tail(state: &mut ParserState<'_>, target: Expr) -> Result<Expr, ParseError> {
    state.pos += 1; // consume '['
    state.skip_ws_and_comments();

    // Check if it starts with ':' (slice without start bound)
    if state.rem().starts_with(':') {
        return parse_slice_tail(state, target, None);
    }

    let first_expr = parse_query(state)?;
    state.skip_ws_and_comments();

    if state.rem().starts_with(':') {
        return parse_slice_tail(state, target, Some(Box::new(first_expr)));
    }

    if !state.rem().starts_with(']') {
        return Err(state.err("expected closing bracket ']'"));
    }
    state.pos += 1;
    Ok(Expr::IndexExpr {
        target: Box::new(target),
        index: Box::new(first_expr),
    })
}

fn parse_slice_tail(
    state: &mut ParserState<'_>,
    target: Expr,
    start: Option<Box<Expr>>,
) -> Result<Expr, ParseError> {
    state.pos += 1; // consume first ':'
    state.skip_ws_and_comments();

    let mut end: Option<Box<Expr>> = None;
    if !state.rem().starts_with(':') && !state.rem().starts_with(']') {
        end = Some(Box::new(parse_query(state)?));
        state.skip_ws_and_comments();
    }

    let mut step: Option<Box<Expr>> = None;
    if state.rem().starts_with(':') {
        state.pos += 1;
        state.skip_ws_and_comments();
        if !state.rem().starts_with(']') {
            step = Some(Box::new(parse_query(state)?));
            state.skip_ws_and_comments();
        }
    }

    if !state.rem().starts_with(']') {
        return Err(state.err("expected closing bracket ']'"));
    }
    state.pos += 1;

    Ok(Expr::SliceExpr {
        target: Box::new(target),
        start,
        end,
        step,
    })
}

fn parse_dot_tail(state: &mut ParserState<'_>, target: Expr) -> Result<Expr, ParseError> {
    // 1. `target.(map_expr)`
    if state.rem().starts_with('(') {
        state.pos += 1;
        state.skip_ws_and_comments();
        let inner = parse_query(state)?;
        state.skip_ws_and_comments();
        if !state.rem().starts_with(')') {
            return Err(state.err("expected closing bracket ')'"));
        }
        state.pos += 1;
        return Ok(Expr::MapExpr {
            target: Box::new(target),
            map_fn: Box::new(inner),
        });
    }

    // 2. Quoted field segment `target."foo.bar"`
    if state.rem().starts_with('"') {
        let quoted = state.parse_quoted_string()?;
        let escaped = escape_quoted_path_segment(&quoted);
        return Ok(append_field_or_get(target, &escaped));
    }

    // 3. Method call `target.method(...)` or field reference `target.field`
    let saved = state.pos;
    if let Some(ident) = state.parse_identifier() {
        if state.rem().starts_with('(') {
            // Bento parses the whole call first and reports an unknown name after its arguments.
            let args = parse_arg_list(state)?;
            if !state.env.has_method(&ident) {
                return Err(state.err(format!("unrecognised method '{ident}'")));
            }
            check_required_param(
                state,
                crate::required_params::REQUIRED_METHODS,
                &ident,
                &args,
            )?;
            validate_method_args_at_parse(state, saved, &ident, &args)?;
            return Ok(Expr::MethodCall {
                target: Box::new(target),
                name: ident,
                args,
            });
        }
        return Ok(append_field_or_get(target, &ident));
    }

    Err(state.err("expected field path or method after '.'"))
}

fn append_field_or_get(target: Expr, path_str: &str) -> Expr {
    let extra = dot_path_to_slice(path_str);
    match target {
        Expr::FieldRef {
            named_context,
            from_root,
            mut path,
        } => {
            path.extend(extra);
            Expr::FieldRef {
                named_context,
                from_root,
                path,
            }
        }
        other => Expr::MethodCall {
            target: Box::new(other),
            name: "get".to_string(),
            args: ArgList::Positional(vec![Expr::Literal(Value::String(path_str.to_string()))]),
        },
    }
}

fn parse_primary(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    state.skip_spaces_and_tabs();

    // 1. `match` expression
    if state.starts_with_keyword("match") {
        return parse_match_expr(state);
    }

    // 2. `if` expression
    if state.starts_with_keyword("if") {
        return parse_if_expr(state);
    }

    // 3. Lambda `name -> expr`
    if let Some(lambda) = try_parse_lambda(state)? {
        return Ok(lambda);
    }

    // 4. Bracketed expression `( query )`
    if state.rem().starts_with('(') {
        state.pos += 1;
        state.skip_ws_and_comments();
        let inner = parse_query(state)?;
        state.skip_ws_and_comments();
        if !state.rem().starts_with(')') {
            return Err(state.err("required: expected closing bracket ')'"));
        }
        state.pos += 1;
        return Ok(inner);
    }

    // 5. Literals: bool, null, number, quoted string, array `[...]`, object `{...}`
    if state.starts_with_keyword("true") {
        state.pos += 4;
        return Ok(Expr::Literal(Value::Bool(true)));
    }
    if state.starts_with_keyword("false") {
        state.pos += 5;
        return Ok(Expr::Literal(Value::Bool(false)));
    }
    if state.starts_with_keyword("null") {
        state.pos += 4;
        return Ok(Expr::Literal(Value::Null));
    }
    if state.rem().starts_with('"') {
        let s = state.parse_quoted_string()?;
        return Ok(Expr::Literal(Value::String(s)));
    }
    if state.rem().starts_with('[') {
        return parse_array_literal(state);
    }
    if state.rem().starts_with('{') {
        return parse_object_literal(state);
    }
    if state.rem().starts_with(|c: char| {
        c.is_ascii_digit()
            || (c == '-' && state.rem()[1..].starts_with(|d: char| d.is_ascii_digit()))
    }) {
        return parse_number_literal(state);
    }

    // 6. `@` metadata reference (`@`, `@foo`, `@"foo"`, `@.foo`)
    if state.rem().starts_with('@') {
        state.pos += 1;
        if state.rem().starts_with('"') {
            let k = state.parse_quoted_string()?;
            return Ok(Expr::MetaRef(Some(k)));
        }
        if let Some(k) = state.parse_identifier() {
            return Ok(Expr::MetaRef(Some(k)));
        }
        return Ok(Expr::MetaRef(None));
    }

    // 7. `$var` variable reference
    if state.rem().starts_with('$') {
        state.pos += 1;
        let name = state
            .parse_identifier()
            .ok_or_else(|| state.err("expected variable name after '$'"))?;
        return Ok(Expr::VarRef(name));
    }

    // 8. Function call `fn_name(...)` or root field reference (`this`, `root`, named context, or implicit `this.foo`)
    if let Some(ident) = state.parse_identifier() {
        if state.rem().starts_with('(') {
            let args = parse_arg_list(state)?;
            if !state.env.has_function(&ident) {
                return Err(state.err(format!("unrecognised function '{ident}'")));
            }
            check_required_param(
                state,
                crate::required_params::REQUIRED_FUNCTIONS,
                &ident,
                &args,
            )?;
            let state_id = state.alloc_state_id();
            return Ok(Expr::FunctionCall {
                name: ident,
                args,
                state_id,
            });
        }
        return Ok(match ident.as_str() {
            "this" => Expr::FieldRef {
                named_context: None,
                from_root: false,
                path: SmallVec::new(),
            },
            "root" => Expr::FieldRef {
                named_context: None,
                from_root: true,
                path: SmallVec::new(),
            },
            other => {
                if state.has_named_context(other) {
                    Expr::FieldRef {
                        named_context: Some(other.to_string()),
                        from_root: false,
                        path: SmallVec::new(),
                    }
                } else {
                    Expr::FieldRef {
                        named_context: None,
                        from_root: false,
                        path: dot_path_to_slice(other),
                    }
                }
            }
        });
    }

    Err(state.err("expected query"))
}

fn try_parse_lambda(state: &mut ParserState<'_>) -> Result<Option<Expr>, ParseError> {
    let saved = state.pos;
    let Some(name) = state.parse_identifier() else {
        return Ok(None);
    };
    state.skip_spaces_and_tabs();
    if !state.rem().starts_with("->") {
        state.pos = saved;
        return Ok(None);
    }
    state.pos += 2;
    state.skip_spaces_and_tabs();

    if name != "_" {
        if name == "root" || name == "this" {
            return Err(state.err_at(saved, format!("context label `{name}` is not allowed")));
        }
        if state.has_named_context(&name) {
            return Err(state.err_at(
                saved,
                format!("context label `{name}` would shadow a parent context"),
            ));
        }
        state.named_contexts.push(name.clone());
    }

    let body = parse_query(state);
    if name != "_" {
        state.named_contexts.pop();
    }
    let body = body?;
    if let Expr::Lambda {
        name: inner_name, ..
    } = &body
    {
        return Err(state.err_at(
            saved,
            format!("it would be in poor taste to capture the same context under both '{name}' and '{inner_name}'"),
        ));
    }
    Ok(Some(Expr::Lambda {
        name,
        body: Box::new(body),
    }))
}

fn parse_number_literal(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    let start = state.pos;
    let mut neg = false;
    if state.rem().starts_with('-') {
        neg = true;
        state.pos += 1;
    }
    let int_len = state
        .rem()
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if int_len == 0 {
        return Err(state.err_at(start, "expected number"));
    }
    state.pos += int_len;
    let mut is_float = false;
    if state.rem().starts_with('.') {
        let frac_len = state.rem()[1..]
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if frac_len > 0 {
            is_float = true;
            state.pos += 1 + frac_len;
        }
    }
    // Scientific notation `e` / `E`
    if state.rem().starts_with('e') || state.rem().starts_with('E') {
        let exp_rem = &state.rem()[1..];
        let sign_len = usize::from(exp_rem.starts_with('+') || exp_rem.starts_with('-'));
        let exp_digits = exp_rem[sign_len..]
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if exp_digits > 0 {
            is_float = true;
            state.pos += 1 + sign_len + exp_digits;
        }
    }

    let raw = &state.input[start..state.pos];
    if is_float {
        let f = raw
            .parse::<f64>()
            .map_err(|e| state.err_at(start, format!("failed to parse '{raw}' as float: {e}")))?;
        Ok(Expr::Literal(Value::Float(f)))
    } else {
        let digits = if neg { &raw[1..] } else { raw };
        let i = digits
            .parse::<i64>()
            .map_err(|e| state.err_at(start, format!("failed to parse '{raw}' as integer: {e}")))?;
        Ok(Expr::Literal(Value::Int(if neg { -i } else { i })))
    }
}

fn parse_array_literal(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    state.pos += 1; // consume '['
    state.skip_ws_and_comments();
    let mut items = Vec::new();
    if state.rem().starts_with(']') {
        state.pos += 1;
        return Ok(Expr::Literal(Value::Array(Vec::new().into())));
    }
    loop {
        state.skip_ws_and_comments();
        if state.rem().starts_with(']') {
            state.pos += 1;
            break;
        }
        let item = parse_query(state)?;
        items.push(item);
        state.skip_spaces_and_tabs();
        if state.rem().starts_with(',') {
            state.pos += 1;
            state.skip_ws_and_comments();
            continue;
        }
        state.skip_ws_and_comments();
        if state.rem().starts_with(']') {
            state.pos += 1;
            break;
        }
        return Err(state.err("expected ',' or ']' in array literal"));
    }
    if items.iter().all(|e| matches!(e, Expr::Literal(_))) {
        let static_items: Vec<Value> = items
            .into_iter()
            .filter_map(|e| match e {
                Expr::Literal(v) if !matches!(v, Value::Delete | Value::Nothing) => Some(v),
                _ => None,
            })
            .collect();
        return Ok(Expr::Literal(Value::Array(static_items.into())));
    }
    Ok(Expr::ArrayLiteral(items))
}

fn parse_object_literal(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    let start = state.pos;
    state.pos += 1; // consume '{'
    state.skip_ws_and_comments();
    let mut pairs: Vec<(Expr, Expr)> = Vec::new();
    if state.rem().starts_with('}') {
        state.pos += 1;
        return Ok(Expr::Literal(Value::Object(
            std::collections::BTreeMap::new().into(),
        )));
    }
    loop {
        state.skip_ws_and_comments();
        if state.rem().starts_with('}') {
            state.pos += 1;
            break;
        }
        let key_expr = if state.rem().starts_with('"') {
            Expr::Literal(Value::String(state.parse_quoted_string()?))
        } else {
            parse_query(state)?
        };
        if let Expr::Literal(v) = &key_expr {
            if !matches!(v, Value::String(_)) {
                return Err(state.err_at(
                    start,
                    format!("object keys must be strings, received: {}", v.type_name()),
                ));
            }
        }
        state.skip_spaces_and_tabs();
        if !state.rem().starts_with(':') {
            return Err(state.err("expected ':' after object key"));
        }
        state.pos += 1;
        state.skip_ws_and_comments();
        let val_expr = parse_query(state)?;
        pairs.push((key_expr, val_expr));

        state.skip_spaces_and_tabs();
        if state.rem().starts_with(',') {
            state.pos += 1;
            state.skip_ws_and_comments();
            continue;
        }
        state.skip_ws_and_comments();
        if state.rem().starts_with('}') {
            state.pos += 1;
            break;
        }
        return Err(state.err("expected ',' or '}' in object literal"));
    }

    if pairs
        .iter()
        .all(|(k, v)| matches!(k, Expr::Literal(Value::String(_))) && matches!(v, Expr::Literal(_)))
    {
        let mut map = std::collections::BTreeMap::new();
        for (k, v) in pairs {
            if let (Expr::Literal(Value::String(ks)), Expr::Literal(vv)) = (k, v) {
                if !matches!(vv, Value::Delete | Value::Nothing) {
                    map.insert(ks, vv);
                }
            }
        }
        return Ok(Expr::Literal(Value::Object(map.into())));
    }

    Ok(Expr::ObjectLiteral(pairs))
}

fn parse_if_expr(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    state.pos += 2; // consume "if"
    state.skip_spaces_and_tabs();
    let cond = parse_query(state)?;
    state.skip_ws_and_comments();
    if !state.rem().starts_with('{') {
        return Err(state.err("required: expected '{' after if condition"));
    }
    state.pos += 1;
    state.skip_ws_and_comments();
    let then_branch = parse_query(state)?;
    state.skip_ws_and_comments();
    if !state.rem().starts_with('}') {
        return Err(state.err("required: expected '}' closing if branch"));
    }
    state.pos += 1;

    let mut else_ifs = Vec::new();
    let mut else_branch = None;

    loop {
        let saved = state.pos;
        state.skip_ws_and_comments();
        if state.starts_with_keyword("else") {
            state.pos += 4;
            state.skip_spaces_and_tabs();
            if state.starts_with_keyword("if") {
                state.pos += 2;
                state.skip_spaces_and_tabs();
                let econd = parse_query(state)?;
                state.skip_ws_and_comments();
                if !state.rem().starts_with('{') {
                    return Err(state.err("required: expected '{' after else if condition"));
                }
                state.pos += 1;
                state.skip_ws_and_comments();
                let ebody = parse_query(state)?;
                state.skip_ws_and_comments();
                if !state.rem().starts_with('}') {
                    return Err(state.err("required: expected '}' closing else if branch"));
                }
                state.pos += 1;
                else_ifs.push((econd, ebody));
                continue;
            }
            state.skip_ws_and_comments();
            if !state.rem().starts_with('{') {
                return Err(state.err("required: expected '{' after else"));
            }
            state.pos += 1;
            state.skip_ws_and_comments();
            let ebody = parse_query(state)?;
            state.skip_ws_and_comments();
            if !state.rem().starts_with('}') {
                return Err(state.err("required: expected '}' closing else branch"));
            }
            state.pos += 1;
            else_branch = Some(Box::new(ebody));
            break;
        } else {
            state.pos = saved;
            break;
        }
    }

    Ok(Expr::If {
        cond: Box::new(cond),
        then_branch: Box::new(then_branch),
        else_ifs,
        else_branch,
    })
}

fn parse_match_expr(state: &mut ParserState<'_>) -> Result<Expr, ParseError> {
    state.pos += 5; // consume "match"
    state.skip_spaces_and_tabs();
    let mut context_expr = None;
    let saved = state.pos;
    state.skip_ws_and_comments();
    if !state.rem().starts_with('{') {
        state.pos = saved;
        context_expr = Some(Box::new(parse_query(state)?));
        state.skip_ws_and_comments();
    }
    if !state.rem().starts_with('{') {
        return Err(state.err("required: expected '{' starting match cases"));
    }
    state.pos += 1;
    state.skip_ws_and_comments();

    let mut cases = Vec::new();
    while !state.rem().starts_with('}') {
        state.skip_ws_and_comments();
        if state.rem().starts_with('}') {
            break;
        }
        let pattern = if state.rem().starts_with('_')
            && !state.rem()[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            state.pos += 1;
            MatchPattern::Wildcard
        } else {
            let p_expr = parse_query(state)?;
            match p_expr {
                Expr::Literal(lit) => MatchPattern::ValueLit(lit),
                other => MatchPattern::Condition(other),
            }
        };
        state.skip_spaces_and_tabs();
        if !state.rem().starts_with("=>") {
            return Err(state.err("expected '=>' in match case"));
        }
        state.pos += 2;
        state.skip_spaces_and_tabs();
        let query = parse_query(state)?;
        cases.push(MatchCase { pattern, query });

        state.skip_spaces_and_tabs();
        if state.rem().starts_with(',') {
            state.pos += 1;
            state.skip_ws_and_comments();
            continue;
        }
        if state.consume_newline_or_comment() {
            state.skip_ws_and_comments();
            continue;
        }
        state.skip_ws_and_comments();
        if state.rem().starts_with('}') {
            break;
        }
        return Err(state.err("expected ',', newline, or '}' after match case"));
    }

    if !state.rem().starts_with('}') {
        return Err(state.err("required: expected '}' closing match expression"));
    }
    state.pos += 1;

    Ok(Expr::Match {
        context_expr,
        cases,
    })
}

fn parse_arg_list(state: &mut ParserState<'_>) -> Result<ArgList, ParseError> {
    let start = state.pos;
    if !state.rem().starts_with('(') {
        return Err(state.err("expected '(' starting arguments"));
    }
    state.pos += 1;
    state.skip_ws_and_comments();
    if state.rem().starts_with(')') {
        state.pos += 1;
        return Ok(ArgList::Positional(Vec::new()));
    }

    let mut positional = Vec::new();
    let mut named = Vec::new();
    let mut seen_names = HashSet::new();

    loop {
        state.skip_ws_and_comments();
        if state.rem().starts_with(')') {
            state.pos += 1;
            break;
        }
        let arg_start = state.pos;
        if let Some(arg_name) = state.parse_snake_case() {
            if state.rem().starts_with(':') {
                state.pos += 1;
                state.skip_spaces_and_tabs();
                let val = parse_query(state)?;
                if !seen_names.insert(arg_name.clone()) {
                    return Err(state.err_at(arg_start, format!("duplicate named arg: {arg_name}")));
                }
                named.push((arg_name, val));
            } else {
                state.pos = arg_start;
                let val = parse_query(state)?;
                positional.push(val);
            }
        } else {
            let val = parse_query(state)?;
            positional.push(val);
        }

        state.skip_spaces_and_tabs();
        if state.rem().starts_with(',') {
            state.pos += 1;
            state.skip_ws_and_comments();
            continue;
        }
        state.skip_ws_and_comments();
        if state.rem().starts_with(')') {
            state.pos += 1;
            break;
        }
        return Err(state.err("required: expected comma ',' or closing bracket ')'"));
    }

    if !positional.is_empty() && !named.is_empty() {
        return Err(state.err_at(start, "cannot mix named and nameless arguments"));
    }

    if !named.is_empty() {
        Ok(ArgList::Named(named))
    } else {
        Ok(ArgList::Positional(positional))
    }
}

/// Bento refuses a call that leaves out a required parameter while parsing, after the argument list.
fn check_required_param(
    state: &ParserState<'_>,
    table: &[(&str, &str)],
    name: &str,
    args: &ArgList,
) -> Result<(), ParseError> {
    let Some((_, param)) = table.iter().find(|(n, _)| *n == name) else {
        return Ok(());
    };
    let missing = match args {
        ArgList::Positional(v) => v.is_empty(),
        ArgList::Named(n) => !n.iter().any(|(k, _)| k == param),
    };
    if missing {
        return Err(state.err(format!("missing parameter: {param}")));
    }
    Ok(())
}

fn validate_method_args_at_parse(
    state: &ParserState<'_>,
    offset: usize,
    name: &str,
    args: &ArgList,
) -> Result<(), ParseError> {
    if name == "slice" {
        if let ArgList::Positional(v) = args {
            if let (Some(Expr::Literal(Value::Int(low))), Some(Expr::Literal(Value::Int(high)))) =
                (v.first(), v.get(1))
            {
                if *high > 0 && *low >= *high {
                    return Err(state.err_at(
                        offset,
                        format!("lower slice bound {low} must be lower than upper ({high})"),
                    ));
                }
            }
        }
    }
    Ok(())
}
