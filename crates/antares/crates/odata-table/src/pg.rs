// SPDX-License-Identifier: MIT
//! [`PgTable`]: a plan pushed down to PostgreSQL.
//!
//! Every value of the request reaches the database as a bound parameter, property names
//! included (a nested path is a bound `text[]`); the only names written into the SQL text are
//! the relation and the entity type's top-level properties as columns, both checked as plain
//! identifiers when the table is built. The statement runs on the pool the caller hands in, so
//! that role's row-level security holds as for any other statement. Each comparison is written
//! as a `CASE` on the JSON type, which makes it two-valued exactly as the reference evaluator
//! is, and strings compare under `COLLATE "C"` (their UTF-8 bytes).

use odata_url::CompareOp;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::{PgPool, Postgres, QueryBuilder};

use crate::plan::{Cond, SortKey, TextOp};
use crate::{
    Applying, Batch, Change, Fetched, Fetching, Plan, Scope, Table, TableError, TableInfo,
};

/// An entity set over one PostgreSQL relation: one column per top-level property of the entity
/// type, named as the property; a complex property is a `jsonb` column.
#[derive(Debug, Clone)]
pub struct PgTable {
    info: TableInfo,
    pool: PgPool,
    relation: String,
}

/// `name` when it is a plain SQL identifier, double-quoted.
fn identifier(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let first = chars.next()?;
    let plain = (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 63;
    plain.then(|| format!("\"{name}\""))
}

impl PgTable {
    /// The set `info` over `relation` (`table` or `schema.table`), read through `pool`.
    ///
    /// # Errors
    ///
    /// [`TableError::BadRequest`] when the relation or a property name is not a plain
    /// identifier (letters, digits, `_`, at most 63 bytes).
    pub fn new(info: TableInfo, pool: PgPool, relation: &str) -> Result<Self, TableError> {
        let refused =
            |what: &str| TableError::BadRequest(format!("{what} is not a plain SQL identifier"));
        let parts: Option<Vec<String>> = relation.split('.').map(identifier).collect();
        let relation = match parts {
            Some(parts) if (1..=2).contains(&parts.len()) => parts.join("."),
            _ => return Err(refused(relation)),
        };
        if let Some(bad) = info.properties.iter().find(|p| identifier(p).is_none()) {
            return Err(refused(bad));
        }
        Ok(Self {
            info,
            pool,
            relation,
        })
    }

    fn column(&self, name: &str) -> Result<String, TableError> {
        self.info
            .properties
            .iter()
            .find(|p| *p == name)
            .and_then(|p| identifier(p))
            .ok_or_else(|| TableError::UnknownProperty(name.to_owned()))
    }

    /// The `jsonb` value at `path`: the column, then the rest of the path as a bound `text[]`.
    fn json(&self, qb: &mut QueryBuilder<Postgres>, path: &[String]) -> Result<(), TableError> {
        let Some((first, rest)) = path.split_first() else {
            return Err(TableError::UnknownProperty(String::new()));
        };
        qb.push("to_jsonb(t.").push(self.column(first)?).push(")");
        if !rest.is_empty() {
            qb.push(" #> ").push_bind(rest.to_vec()).push("::text[]");
        }
        Ok(())
    }

    /// `CASE WHEN jsonb_typeof(<path>) = '<ty>' THEN <then> ELSE FALSE END`, `then` written by
    /// `body` with the path's text as `(<path> #>> '{}')`.
    fn typed(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        path: &[String],
        ty: &str,
        body: impl FnOnce(&mut QueryBuilder<Postgres>) -> Result<(), TableError>,
    ) -> Result<(), TableError> {
        qb.push("(CASE WHEN jsonb_typeof(");
        self.json(qb, path)?;
        qb.push(") = ").push_bind(ty.to_owned()).push(" THEN (");
        self.json(qb, path)?;
        qb.push(" #>> '{}')");
        body(qb)?;
        qb.push(" ELSE FALSE END)");
        Ok(())
    }

    fn cond(&self, qb: &mut QueryBuilder<Postgres>, cond: &Cond) -> Result<(), TableError> {
        match cond {
            Cond::Const(true) => {
                qb.push("TRUE");
            }
            Cond::Const(false) => {
                qb.push("FALSE");
            }
            Cond::And(a, b) | Cond::Or(a, b) => {
                qb.push("(");
                self.cond(qb, a)?;
                qb.push(if matches!(cond, Cond::And(..)) {
                    " AND "
                } else {
                    " OR "
                });
                self.cond(qb, b)?;
                qb.push(")");
            }
            Cond::Not(a) => {
                qb.push("(NOT ");
                self.cond(qb, a)?;
                qb.push(")");
            }
            Cond::IsNull { path } => {
                qb.push("(");
                self.json(qb, path)?;
                qb.push(" IS NULL OR jsonb_typeof(");
                self.json(qb, path)?;
                qb.push(") = 'null')");
            }
            Cond::Compare { path, op, value } => {
                let op = match op {
                    CompareOp::Eq => " = ",
                    CompareOp::Ne => " <> ",
                    CompareOp::Gt => " > ",
                    CompareOp::Ge => " >= ",
                    CompareOp::Lt => " < ",
                    CompareOp::Le => " <= ",
                };
                match value {
                    Value::String(s) => self.typed(qb, path, "string", |qb| {
                        qb.push(" COLLATE \"C\"").push(op).push_bind(s.clone());
                        Ok(())
                    })?,
                    Value::Number(n) => self.typed(qb, path, "number", |qb| {
                        qb.push("::numeric")
                            .push(op)
                            .push_bind(n.to_string())
                            .push("::numeric");
                        Ok(())
                    })?,
                    Value::Bool(b) => self.typed(qb, path, "boolean", |qb| {
                        qb.push("::boolean").push(op).push_bind(*b);
                        Ok(())
                    })?,
                    Value::Null | Value::Array(_) | Value::Object(_) => {
                        qb.push("FALSE");
                    }
                }
            }
            Cond::Text { path, op, needle } => self.typed(qb, path, "string", |qb| {
                qb.push(if *op == TextOp::StartsWith {
                    " COLLATE \"C\" LIKE "
                } else {
                    " COLLATE \"C\" LIKE '%' || "
                });
                qb.push_bind(like_escaped(needle));
                if *op != TextOp::EndsWith {
                    qb.push(" || '%'");
                }
                qb.push(" ESCAPE '\\'");
                Ok(())
            })?,
        }
        Ok(())
    }

    fn filtered(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        plan: &Plan,
        scope: &Scope,
    ) -> Result<(), TableError> {
        qb.push(" FROM ").push(&self.relation).push(" t WHERE ");
        // the scope first: the filter can only narrow it
        self.cond(qb, &scope.cond())?;
        qb.push(" AND ");
        self.cond(qb, &plan.filter)
    }

    /// One ordering key as four `ORDER BY` terms: the JSON type rank, then the boolean, the
    /// number and the string, the reference evaluator's [`compare`](crate::compare).
    fn order(&self, qb: &mut QueryBuilder<Postgres>, key: &SortKey) -> Result<(), TableError> {
        let dir = if key.descending { " DESC" } else { " ASC" };
        qb.push("CASE jsonb_typeof(");
        self.json(qb, &key.path)?;
        qb.push(") WHEN 'boolean' THEN 1 WHEN 'number' THEN 2 WHEN 'string' THEN 3 WHEN 'object' THEN 4 WHEN 'array' THEN 4 ELSE 0 END")
            .push(dir);
        for (ty, cast) in [
            ("boolean", "::boolean"),
            ("number", "::numeric"),
            ("string", ""),
        ] {
            qb.push(", (CASE WHEN jsonb_typeof(");
            self.json(qb, &key.path)?;
            qb.push(") = '").push(ty).push("' THEN (");
            self.json(qb, &key.path)?;
            qb.push(" #>> '{}')").push(cast).push(" END)");
            if ty == "string" {
                qb.push(" COLLATE \"C\"");
            }
            qb.push(dir);
        }
        Ok(())
    }

    async fn run(&self, plan: &Plan, scope: &Scope) -> Result<Fetched, TableError> {
        let failed = |e: sqlx::Error| TableError::Source(e.to_string());
        let mut qb = QueryBuilder::<Postgres>::new("SELECT ");
        // jsonb_build_object takes at most 100 arguments: 40 properties a call, joined by ||
        for (at, chunk) in self.info.properties.chunks(40).enumerate() {
            if at > 0 {
                qb.push(" || ");
            }
            qb.push("jsonb_build_object(");
            for (i, name) in chunk.iter().enumerate() {
                if i > 0 {
                    qb.push(", ");
                }
                qb.push_bind(name.clone())
                    .push("::text, t.")
                    .push(self.column(name)?);
            }
            qb.push(")");
        }
        self.filtered(&mut qb, plan, scope)?;
        qb.push(" ORDER BY ");
        for (i, key) in plan.order.iter().enumerate() {
            if i > 0 {
                qb.push(", ");
            }
            self.order(&mut qb, key)?;
        }
        qb.push(" LIMIT ")
            .push_bind(i64::try_from(plan.fetch).unwrap_or(i64::MAX))
            .push(" OFFSET ")
            .push_bind(i64::try_from(plan.offset).unwrap_or(i64::MAX));
        let rows: Vec<Json<Value>> = qb
            .build_query_scalar()
            .fetch_all(&self.pool)
            .await
            .map_err(failed)?;
        let count = if plan.count {
            let mut qb = QueryBuilder::<Postgres>::new("SELECT count(*)");
            self.filtered(&mut qb, plan, scope)?;
            let n: i64 = qb
                .build_query_scalar()
                .fetch_one(&self.pool)
                .await
                .map_err(failed)?;
            Some(u64::try_from(n).unwrap_or(0))
        } else {
            None
        };
        Ok(Fetched {
            rows: rows.into_iter().map(|Json(v)| v).collect(),
            count,
        })
    }
}

impl PgTable {
    /// Applies `batch` in one transaction: one `UPDATE` or `DELETE` over the batch's keys that
    /// meet its guard, rolled back unless it changed exactly the batch's rows. The new values
    /// reach the statement as one bound `jsonb`, cast to each column by
    /// `jsonb_populate_record`.
    async fn write(&self, batch: &Batch) -> Result<(), TableError> {
        let failed = |e: sqlx::Error| TableError::Source(e.to_string());
        let mut qb = match &batch.change {
            Change::Delete => {
                let mut qb = QueryBuilder::<Postgres>::new("DELETE FROM ");
                qb.push(&self.relation).push(" t WHERE ");
                qb
            }
            Change::Update(patch) => {
                let mut qb = QueryBuilder::<Postgres>::new("UPDATE ");
                qb.push(&self.relation).push(" t SET ");
                for (i, name) in patch.keys().enumerate() {
                    if i > 0 {
                        qb.push(", ");
                    }
                    let column = self.column(name)?;
                    qb.push(&column).push(" = r.").push(&column);
                }
                qb.push(" FROM jsonb_populate_record(NULL::")
                    .push(&self.relation)
                    .push(", ")
                    .push_bind(Json(Value::Object(patch.clone())))
                    .push("::jsonb) r WHERE ");
                qb
            }
        };
        qb.push("(")
            .push_bind(Json(Value::Array(batch.keys.clone())))
            .push("::jsonb @> jsonb_build_array(to_jsonb(t.")
            .push(self.column(&self.info.key)?)
            .push("))) AND ");
        self.cond(&mut qb, &batch.guard)?;
        let mut tx = self.pool.begin().await.map_err(failed)?;
        let done = qb.build().execute(&mut *tx).await.map_err(failed)?;
        if usize::try_from(done.rows_affected()).ok() != Some(batch.keys.len()) {
            tx.rollback().await.map_err(failed)?;
            return Err(TableError::Conflict(
                "a matched row changed since the filter matched it; nothing was changed, read the rows again and retry".into(),
            ));
        }
        tx.commit().await.map_err(failed)
    }
}

/// `needle` with the `LIKE` wildcards and the escape character escaped.
fn like_escaped(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len());
    for c in needle.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Table for PgTable {
    fn info(&self) -> &TableInfo {
        &self.info
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        Box::pin(self.run(plan, scope))
    }

    fn writable(&self) -> bool {
        true
    }

    fn apply<'a>(&'a self, batch: &'a Batch) -> Applying<'a> {
        Box::pin(self.write(batch))
    }
}
