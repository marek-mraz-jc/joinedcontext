use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::value::Value;

/// Query target type (for `workflow` DAG dependency inference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetType {
    Metadata,
    Value,
    Root,
    Variable,
}

/// A path referenced by a query (`internal/bloblang/query/target.go`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetPath {
    pub target_type: TargetType,
    pub path: Vec<String>,
}

impl TargetPath {
    pub fn new(target_type: TargetType, path: Vec<String>) -> Self {
        Self { target_type, path }
    }
}

/// Assignment target type (`internal/bloblang/mapping/target.go`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssignTargetType {
    Metadata,
    Value,
    Variable,
}

/// A path assigned to by a mapping statement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssignTargetPath {
    pub target_type: AssignTargetType,
    pub path: Vec<String>,
}

/// Context used when statically walking an AST to collect `TargetPath`s.
/// Includes `visited_maps` to prevent infinite recursion when a named `map` calls itself!
#[derive(Clone, Default)]
pub struct TargetsContext {
    pub maps: Arc<HashMap<String, Arc<MapDef>>>,
    pub current_values: Vec<TargetPath>,
    pub main_context: Vec<TargetPath>,
    pub prev_contexts: Vec<Vec<TargetPath>>,
    pub named_contexts: HashMap<String, Vec<TargetPath>>,
    pub visited_maps: HashSet<String>,
}

impl TargetsContext {
    pub fn with_values(&self, paths: Vec<TargetPath>) -> Self {
        let mut next = self.clone();
        next.current_values = paths;
        next
    }

    pub fn with_values_as_context(&self) -> Self {
        let mut next = self.clone();
        next.prev_contexts.push(self.main_context.clone());
        next.main_context = self.current_values.clone();
        next.current_values.clear();
        next
    }

    pub fn with_context_as_named(&self, name: &str) -> Self {
        let mut next = self.clone();
        next.named_contexts
            .insert(name.to_string(), self.main_context.clone());
        if let Some(prev) = next.prev_contexts.pop() {
            next.main_context = prev;
        }
        next
    }

    pub fn pop_context(&self) -> Self {
        let mut next = self.clone();
        if let Some(prev) = next.prev_contexts.pop() {
            next.main_context = prev;
        }
        next
    }
}

#[derive(Debug, Clone)]
pub struct MapDef {
    pub name: String,
    pub statements: Vec<Statement>,
}

// Single is the common statement; boxing it would add an allocation per assignment.
#[expect(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Statement {
    Single {
        line: usize,
        assignment: Assignment,
        query: Expr,
    },
    If {
        line: usize,
        branches: Vec<(Option<Expr>, Vec<Statement>)>,
    },
}

#[derive(Debug, Clone)]
pub enum Assignment {
    Var(String),
    Meta(Option<String>),
    Json(SmallVec<[String; 4]>),
}

impl Assignment {
    pub fn target(&self) -> AssignTargetPath {
        match self {
            Assignment::Var(name) => AssignTargetPath {
                target_type: AssignTargetType::Variable,
                path: vec![name.clone()],
            },
            Assignment::Meta(key) => AssignTargetPath {
                target_type: AssignTargetType::Metadata,
                path: key.iter().cloned().collect(),
            },
            Assignment::Json(path) => AssignTargetPath {
                target_type: AssignTargetType::Value,
                path: path.to_vec(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Gt,
    Lt,
    Gte,
    Lte,
    And,
    Or,
    Pipe, // Coalesce `|`
}

impl BinaryOp {
    pub fn name(&self) -> &'static str {
        match self {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "subtract",
            BinaryOp::Div => "divide",
            BinaryOp::Mul => "multiply",
            BinaryOp::Mod => "modulo",
            BinaryOp::Eq
            | BinaryOp::Neq
            | BinaryOp::Gt
            | BinaryOp::Lt
            | BinaryOp::Gte
            | BinaryOp::Lte => "compare",
            BinaryOp::And => "boolean and",
            BinaryOp::Or => "boolean or",
            BinaryOp::Pipe => "coalesce",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ArgList {
    Positional(Vec<Expr>),
    Named(Vec<(String, Expr)>),
}

impl ArgList {
    pub fn is_empty(&self) -> bool {
        match self {
            ArgList::Positional(v) => v.is_empty(),
            ArgList::Named(v) => v.is_empty(),
        }
    }

    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            ArgList::Positional(v) => v.iter().collect(),
            ArgList::Named(v) => v.iter().map(|(_, e)| e).collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchCase {
    pub pattern: MatchPattern,
    pub query: Expr,
}

#[derive(Debug, Clone)]
pub enum MatchPattern {
    Wildcard,
    ValueLit(Value),
    Condition(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Value),
    ArrayLiteral(Vec<Expr>),
    ObjectLiteral(Vec<(Expr, Expr)>),
    FieldRef {
        named_context: Option<String>,
        from_root: bool,
        path: SmallVec<[String; 4]>,
    },
    VarRef(String),
    MetaRef(Option<String>),
    FunctionCall {
        name: String,
        args: ArgList,
        state_id: usize,
    },
    MethodCall {
        target: Box<Expr>,
        name: String,
        args: ArgList,
    },
    /// `target.(map_expr)`
    MapExpr {
        target: Box<Expr>,
        map_fn: Box<Expr>,
    },
    /// `target[index]`
    IndexExpr {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    /// `target[start:end:step]`
    SliceExpr {
        target: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    Not(Box<Expr>),
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_ifs: Vec<(Expr, Expr)>,
        else_branch: Option<Box<Expr>>,
    },
    Match {
        context_expr: Option<Box<Expr>>,
        cases: Vec<MatchCase>,
    },
    /// `name -> body`
    Lambda {
        name: String,
        body: Box<Expr>,
    },
}

impl Expr {
    pub fn annotation(&self) -> String {
        match self {
            Expr::Literal(v) => format!("{} literal", v.type_name()),
            Expr::ArrayLiteral(_) => "array literal".to_string(),
            Expr::ObjectLiteral(_) => "object literal".to_string(),
            Expr::FieldRef {
                named_context,
                from_root,
                path,
            } => {
                let mut base = if *from_root {
                    "root".to_string()
                } else if let Some(nc) = named_context {
                    nc.clone()
                } else {
                    "this".to_string()
                };
                if !path.is_empty() {
                    base.push('.');
                    base.push_str(&crate::value::slice_to_dot_path(path));
                }
                format!("field `{base}`")
            }
            Expr::VarRef(name) => format!("variable {name}"),
            Expr::MetaRef(Some(k)) => format!("meta field {k}"),
            Expr::MetaRef(None) => "meta object".to_string(),
            Expr::FunctionCall { name, args, .. } => {
                if name == "json" {
                    let p = match args {
                        ArgList::Positional(v) => v.first().and_then(|e| match e {
                            Expr::Literal(Value::String(s)) => Some(s.as_str()),
                            _ => None,
                        }),
                        ArgList::Named(v) => {
                            v.iter()
                                .find(|(k, _)| k == "path")
                                .and_then(|(_, e)| match e {
                                    Expr::Literal(Value::String(s)) => Some(s.as_str()),
                                    _ => None,
                                })
                        }
                    }
                    .unwrap_or("");
                    format!("json path `{p}`")
                } else {
                    format!("function {name}")
                }
            }
            Expr::MethodCall { name, .. } => format!("method {name}"),
            Expr::MapExpr { map_fn, .. } => map_fn.annotation(),
            Expr::IndexExpr { .. } => "index".to_string(),
            Expr::SliceExpr { .. } => "slice".to_string(),
            Expr::Not(inner) => format!("not {}", inner.annotation()),
            Expr::Binary { rhs, .. } => rhs.annotation(),
            Expr::If { .. } => "if expression".to_string(),
            Expr::Match { .. } => "match expression".to_string(),
            Expr::Lambda { body, .. } => body.annotation(),
        }
    }

    pub fn query_targets(&self, ctx: TargetsContext) -> (TargetsContext, Vec<TargetPath>) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match self {
            Expr::Literal(_) => (ctx, Vec::new()),
            Expr::ArrayLiteral(items) => {
                let mut out = Vec::new();
                for item in items {
                    let (_, p) = item.query_targets(ctx.clone());
                    out.extend(p);
                }
                (ctx, out)
            }
            Expr::ObjectLiteral(kvs) => {
                let mut out = Vec::new();
                for (k, v) in kvs {
                    let (_, pk) = k.query_targets(ctx.clone());
                    let (_, pv) = v.query_targets(ctx.clone());
                    out.extend(pk);
                    out.extend(pv);
                }
                (ctx, out)
            }
            Expr::FieldRef {
                named_context,
                from_root,
                path,
            } => {
                let base_paths = if *from_root {
                    vec![TargetPath::new(TargetType::Root, Vec::new())]
                } else if let Some(nc) = named_context {
                    ctx.named_contexts.get(nc).cloned().unwrap_or_default()
                } else if !ctx.main_context.is_empty() {
                    ctx.main_context.clone()
                } else {
                    vec![TargetPath::new(TargetType::Value, Vec::new())]
                };
                let mut paths = Vec::with_capacity(base_paths.len());
                for mut bp in base_paths {
                    bp.path.extend(path.iter().cloned());
                    paths.push(bp);
                }
                let next_ctx = ctx.with_values(paths.clone());
                (next_ctx, paths)
            }
            Expr::VarRef(name) => {
                let p = vec![TargetPath::new(TargetType::Variable, vec![name.clone()])];
                (ctx.with_values(p.clone()), p)
            }
            Expr::MetaRef(key) => {
                let p = vec![TargetPath::new(
                    TargetType::Metadata,
                    key.iter().cloned().collect(),
                )];
                (ctx.with_values(p.clone()), p)
            }
            Expr::FunctionCall { name, args, .. } => {
                let mut out = Vec::new();
                for e in args.exprs() {
                    let (_, p) = e.query_targets(ctx.clone());
                    out.extend(p);
                }
                if name == "json" {
                    let path_str = match args {
                        ArgList::Positional(v) => v.first().and_then(|e| match e {
                            Expr::Literal(Value::String(s)) => Some(s.as_str()),
                            _ => None,
                        }),
                        ArgList::Named(v) => {
                            v.iter()
                                .find(|(k, _)| k == "path")
                                .and_then(|(_, e)| match e {
                                    Expr::Literal(Value::String(s)) => Some(s.as_str()),
                                    _ => None,
                                })
                        }
                    }
                    .unwrap_or("");
                    let segs = crate::value::dot_path_to_slice(path_str).to_vec();
                    let p = vec![TargetPath::new(TargetType::Value, segs)];
                    let next_ctx = ctx.with_values(p.clone());
                    out.extend(p);
                    return (next_ctx, out);
                }
                if name == "metadata" || name == "meta" || name == "root_meta" {
                    let key_str = match args {
                        ArgList::Positional(v) => v.first().and_then(|e| match e {
                            Expr::Literal(Value::String(s)) => Some(s.as_str()),
                            _ => None,
                        }),
                        ArgList::Named(v) => {
                            v.iter()
                                .find(|(k, _)| k == "key")
                                .and_then(|(_, e)| match e {
                                    Expr::Literal(Value::String(s)) => Some(s.as_str()),
                                    _ => None,
                                })
                        }
                    }
                    .unwrap_or("");
                    let segs = if key_str.is_empty() {
                        Vec::new()
                    } else {
                        vec![key_str.to_string()]
                    };
                    let p = vec![TargetPath::new(TargetType::Metadata, segs)];
                    let next_ctx = ctx.with_values(p.clone());
                    out.extend(p);
                    return (next_ctx, out);
                }
                (ctx, out)
            }
            Expr::MethodCall { target, name, args } => {
                let (t_ctx, mut out) = target.query_targets(ctx.clone());
                if name == "get" {
                    if let Some(Expr::Literal(Value::String(path_str))) = args.exprs().first() {
                        let extra = crate::value::dot_path_to_slice(path_str);
                        let mut paths = Vec::new();
                        for mut bp in t_ctx.current_values.clone() {
                            bp.path.extend(extra.iter().cloned());
                            paths.push(bp);
                        }
                        let next_ctx = t_ctx.with_values(paths.clone());
                        out.extend(paths);
                        return (next_ctx, out);
                    }
                }
                if name == "apply" {
                    if let Some(Expr::Literal(Value::String(map_name))) = args.exprs().first() {
                        // Guard against recursive map cycle during static target analysis!
                        if !t_ctx.visited_maps.contains(map_name.as_str()) {
                            if let Some(mdef) = t_ctx.maps.get(map_name.as_str()).cloned() {
                                let mut map_ctx = t_ctx.with_values_as_context();
                                map_ctx.visited_maps.insert(map_name.clone());
                                for stmt in &mdef.statements {
                                    let (_, sp) = stmt.query_targets(map_ctx.clone());
                                    out.extend(sp);
                                }
                            }
                        }
                    }
                }
                let child_ctx = t_ctx.with_values_as_context();
                for e in args.exprs() {
                    let (_, p) = e.query_targets(child_ctx.clone());
                    out.extend(p);
                }
                (t_ctx, out)
            }
            Expr::MapExpr { target, map_fn } => {
                let (t_ctx, mut out) = target.query_targets(ctx);
                let map_ctx = t_ctx.with_values_as_context();
                let (ret_ctx, mp) = map_fn.query_targets(map_ctx);
                out.extend(mp);
                (ret_ctx, out)
            }
            Expr::IndexExpr { target, index } => {
                let (t_ctx, mut out) = target.query_targets(ctx);
                let (i_ctx, ip) = index.query_targets(t_ctx);
                out.extend(ip);
                (i_ctx, out)
            }
            Expr::SliceExpr {
                target,
                start,
                end,
                step,
            } => {
                let (mut c_ctx, mut out) = target.query_targets(ctx);
                for b in [start, end, step].into_iter().flatten() {
                    let (nc, bp) = b.query_targets(c_ctx);
                    c_ctx = nc;
                    out.extend(bp);
                }
                (c_ctx, out)
            }
            Expr::Not(inner) => inner.query_targets(ctx),
            Expr::Binary { lhs, rhs, .. } => {
                let (_, mut out) = lhs.query_targets(ctx.clone());
                let (_, rp) = rhs.query_targets(ctx.clone());
                out.extend(rp);
                (ctx, out)
            }
            Expr::If {
                cond,
                then_branch,
                else_ifs,
                else_branch,
            } => {
                let (_, mut out) = cond.query_targets(ctx.clone());
                let (_, tp) = then_branch.query_targets(ctx.clone());
                out.extend(tp);
                for (ec, eb) in else_ifs {
                    let (_, p1) = ec.query_targets(ctx.clone());
                    let (_, p2) = eb.query_targets(ctx.clone());
                    out.extend(p1);
                    out.extend(p2);
                }
                if let Some(el) = else_branch {
                    let (_, ep) = el.query_targets(ctx.clone());
                    out.extend(ep);
                }
                (ctx, out)
            }
            Expr::Match {
                context_expr,
                cases,
            } => {
                let (c_ctx, mut out) = if let Some(ce) = context_expr {
                    let (tc, tp) = ce.query_targets(ctx.clone());
                    (tc.with_values(tp.clone()).with_values_as_context(), tp)
                } else {
                    (ctx.clone(), Vec::new())
                };
                for c in cases {
                    if let MatchPattern::Condition(e) = &c.pattern {
                        let (_, p) = e.query_targets(c_ctx.clone());
                        out.extend(p);
                    }
                    let (_, qp) = c.query.query_targets(c_ctx.clone());
                    out.extend(qp);
                }
                (ctx, out)
            }
            Expr::Lambda { name, body } => {
                let next_ctx = if name == "_" {
                    ctx.pop_context()
                } else {
                    ctx.with_context_as_named(name)
                };
                body.query_targets(next_ctx)
            }
        })
    }
}

impl Statement {
    pub fn query_targets(&self, ctx: TargetsContext) -> (TargetsContext, Vec<TargetPath>) {
        match self {
            Statement::Single { query, .. } => query.query_targets(ctx),
            Statement::If { branches, .. } => {
                let mut out = Vec::new();
                for (cond, stmts) in branches {
                    if let Some(c) = cond {
                        let (_, cp) = c.query_targets(ctx.clone());
                        out.extend(cp);
                    }
                    for s in stmts {
                        let (_, sp) = s.query_targets(ctx.clone());
                        out.extend(sp);
                    }
                }
                (ctx, out)
            }
        }
    }

    pub fn assignment_targets(&self) -> Vec<AssignTargetPath> {
        match self {
            Statement::Single { assignment, .. } => vec![assignment.target()],
            Statement::If { branches, .. } => {
                let mut out = Vec::new();
                for (_, stmts) in branches {
                    for s in stmts {
                        out.extend(s.assignment_targets());
                    }
                }
                out
            }
        }
    }
}
