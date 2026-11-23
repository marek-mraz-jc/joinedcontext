#![forbid(unsafe_code)]
//! Index rows in SQL ([`RowQuery`]): scope, filter, order, keyset cursor and page are the
//! statement's, and no model column is selected. Every request value is bound; only the column
//! expressions below are written into SQL.
//!
//! The answer is what `daiya_model::rows::answer_rows` answers over the same rows: text by its
//! bytes (`COLLATE "C"`), null first ascending and last descending, a comparison with null or
//! another type false and so true under `not`.

use daiya_model::{CrError, CrResult, OrgId, ReadScope, RowPage, RowQuery, RowSet};
use jc_time::TimestampTz;
use odata_table::{Cond, TextOp};
use odata_url::CompareOp;
use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use sqlx::{Postgres, QueryBuilder, Row as _, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Integer,
    Boolean,
    Time,
    /// Selected for the caller, never filtered or ordered by.
    Json,
}

struct Column {
    member: &'static str,
    sql: &'static str,
    kind: Kind,
    nullable: bool,
}

const fn column(member: &'static str, sql: &'static str, kind: Kind) -> Column {
    Column {
        member,
        sql,
        kind,
        nullable: false,
    }
}

const fn nullable(member: &'static str, sql: &'static str, kind: Kind) -> Column {
    Column {
        member,
        sql,
        kind,
        nullable: true,
    }
}

const DASHBOARDS: &[Column] = &[
    column("uid", "t.uid", Kind::Text),
    column("title", "t.title", Kind::Text),
    column("slug", "t.slug", Kind::Text),
    nullable("folderUid", "t.folder_uid", Kind::Text),
    column("panelCount", "t.panel_count", Kind::Integer),
    column("version", "t.version", Kind::Integer),
    column("schemaVersion", "t.schema_version", Kind::Integer),
    column("isProvisioned", "t.is_provisioned", Kind::Boolean),
    column("updatedBy", "t.updated_by", Kind::Text),
    column("updatedAt", "t.updated_at", Kind::Time),
    column("tags", "t.tags", Kind::Json),
    nullable(
        "provisionedExternalId",
        "t.provisioned_external_id",
        Kind::Text,
    ),
];
const DASHBOARD_VERSIONS: &[Column] = &[
    // The expression of idx_cr_dashboard_version_row.
    column(
        "id",
        "(t.dashboard_uid || ':' || t.version::text)",
        Kind::Text,
    ),
    column("dashboardUid", "t.dashboard_uid", Kind::Text),
    column("version", "t.version", Kind::Integer),
    column("parentVersion", "t.parent_version", Kind::Integer),
    column("restoredFrom", "t.restored_from", Kind::Integer),
    column("message", "t.message", Kind::Text),
    column("createdBy", "t.created_by", Kind::Text),
    column("createdAt", "t.created_at", Kind::Time),
    nullable("folderUid", "d.folder_uid", Kind::Text),
];
const FOLDERS: &[Column] = &[
    column("uid", "t.uid", Kind::Text),
    nullable("parentUid", "t.parent_uid", Kind::Text),
    column("title", "t.title", Kind::Text),
    column("version", "t.version", Kind::Integer),
    nullable("managedBy", "t.managed_by", Kind::Text),
    column("updatedAt", "t.updated_at", Kind::Time),
];
const DATASOURCES: &[Column] = &[
    column("uid", "t.uid", Kind::Text),
    column("name", "t.name", Kind::Text),
    column("type", "t.kind", Kind::Text),
    column("access", "t.access", Kind::Text),
    column("url", "t.url", Kind::Text),
    column("isDefault", "t.is_default", Kind::Boolean),
    column("readOnly", "t.read_only", Kind::Boolean),
    nullable("wasmDigest", "t.wasm_digest", Kind::Text),
    column("version", "t.version", Kind::Integer),
];
const LIBRARY_PANELS: &[Column] = &[
    column("uid", "t.uid", Kind::Text),
    nullable("folderUid", "t.folder_uid", Kind::Text),
    column("name", "t.name", Kind::Text),
    column("type", "t.panel_type", Kind::Text),
    column("description", "t.description", Kind::Text),
    column("version", "t.version", Kind::Integer),
];
const PUBLIC_DASHBOARDS: &[Column] = &[
    column("uid", "t.uid", Kind::Text),
    column("dashboardUid", "t.dashboard_uid", Kind::Text),
    column("isEnabled", "t.is_enabled", Kind::Boolean),
    column(
        "timeSelectionEnabled",
        "t.time_selection_enabled",
        Kind::Boolean,
    ),
    column("annotationsEnabled", "t.annotations_enabled", Kind::Boolean),
    column("showQueryText", "t.show_query_text", Kind::Boolean),
    column("share", "t.share", Kind::Text),
];
const IMPORT_REPORTS: &[Column] = &[
    column("id", "t.id", Kind::Text),
    column("dryRun", "t.dry_run", Kind::Boolean),
    column("status", "t.status", Kind::Text),
    column("dashboardCount", "t.dashboard_count", Kind::Integer),
    column("refusedCount", "t.refused_count", Kind::Integer),
    column("mappedCount", "t.mapped_count", Kind::Integer),
    column("createdAt", "t.created_at", Kind::Time),
];

/// The columns of `set` and where they come from.
const fn relation(set: RowSet) -> (&'static [Column], &'static str) {
    match set {
        RowSet::Dashboards => (DASHBOARDS, "cr_dashboard t"),
        RowSet::DashboardVersions => (
            DASHBOARD_VERSIONS,
            "cr_dashboard_version t JOIN cr_dashboard d ON d.org_id = t.org_id AND d.uid = t.dashboard_uid",
        ),
        RowSet::Folders => (FOLDERS, "cr_folder t"),
        RowSet::Datasources => (DATASOURCES, "cr_datasource t"),
        RowSet::LibraryPanels => (LIBRARY_PANELS, "cr_library_panel t"),
        RowSet::PublicDashboards => (PUBLIC_DASHBOARDS, "cr_public_dashboard t"),
        RowSet::ImportReports => (IMPORT_REPORTS, "cr_import_report t"),
    }
}

struct Statement<'q> {
    query: &'q RowQuery,
    columns: &'static [Column],
    from: &'static str,
}

impl Statement<'_> {
    fn column(&self, path: &[String]) -> CrResult<&'static Column> {
        let columns = self.columns;
        match path {
            [member] => columns
                .iter()
                .find(|c| c.member == member.as_str() && c.kind != Kind::Json),
            _ => None,
        }
        .ok_or_else(|| {
            CrError::BadRequest(format!(
                "{} has no property {}",
                self.query.set.name(),
                path.join("/")
            ))
        })
    }

    fn operand(qb: &mut QueryBuilder<Postgres>, column: &Column) {
        qb.push(column.sql);
        if column.kind == Kind::Text {
            qb.push(" COLLATE \"C\"");
        }
    }

    /// `column op value`; `false` without a bind for a value of another type.
    fn compared(qb: &mut QueryBuilder<Postgres>, column: &Column, op: &str, value: &Value) {
        match (column.kind, value) {
            (Kind::Text, Value::String(text)) => {
                Self::operand(qb, column);
                qb.push(op).push_bind(text.clone());
            }
            (Kind::Integer, Value::Number(number)) => {
                qb.push(column.sql);
                if let Some(whole) = number.as_i64() {
                    qb.push(op).push_bind(whole);
                } else {
                    qb.push("::float8").push(op).push_bind(number.as_f64());
                }
            }
            (Kind::Boolean, Value::Bool(flag)) => {
                qb.push(column.sql).push(op).push_bind(*flag);
            }
            (Kind::Time, Value::String(text)) => match text.parse::<TimestampTz>() {
                Ok(at) => {
                    qb.push(column.sql).push(op).push_bind(at);
                }
                Err(_) => {
                    qb.push("FALSE");
                }
            },
            _ => {
                qb.push("FALSE");
            }
        }
    }

    /// A condition that is true exactly where the reference says so. Null counts as false, which
    /// `AND`, `OR` and `WHERE` already do; `not` is where it needs saying.
    fn cond(&self, qb: &mut QueryBuilder<Postgres>, cond: &Cond) -> CrResult<()> {
        match cond {
            Cond::Const(flag) => {
                qb.push(if *flag { "TRUE" } else { "FALSE" });
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
            Cond::Not(inner) => {
                qb.push("((");
                self.cond(qb, inner)?;
                qb.push(") IS NOT TRUE)");
            }
            Cond::IsNull { path } => {
                qb.push(self.column(path)?.sql).push(" IS NULL");
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
                Self::compared(qb, self.column(path)?, op, value);
            }
            Cond::Text { path, op, needle } => {
                let column = self.column(path)?;
                if column.kind != Kind::Text {
                    qb.push("FALSE");
                    return Ok(());
                }
                match op {
                    TextOp::Contains => {
                        qb.push("strpos(").push(column.sql).push(", ");
                        qb.push_bind(needle.clone()).push(") > 0");
                    }
                    TextOp::StartsWith => {
                        qb.push("starts_with(").push(column.sql).push(", ");
                        qb.push_bind(needle.clone()).push(")");
                    }
                    TextOp::EndsWith => {
                        qb.push("right(").push(column.sql).push(", char_length(");
                        qb.push_bind(needle.clone()).push(")) = ");
                        qb.push_bind(needle.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn scope(&self, qb: &mut QueryBuilder<Postgres>) -> CrResult<()> {
        match &self.query.scope {
            ReadScope::All => {}
            ReadScope::Nothing => {
                qb.push(" AND FALSE");
            }
            ReadScope::In { column, values } => match self.query.set.scope_member(*column) {
                Some(member) => {
                    let column = self.column(&[member.to_owned()])?;
                    // Postgres stores no NUL, so a value with one names no row.
                    let values: Vec<String> = values
                        .iter()
                        .filter(|value| !value.contains('\0'))
                        .cloned()
                        .collect();
                    qb.push(" AND ").push(column.sql).push(" = ANY(");
                    qb.push_bind(values).push(")");
                }
                None => {
                    qb.push(" AND FALSE");
                }
            },
        }
        Ok(())
    }

    fn push_scope_and_filter(
        &self,
        qb: &mut QueryBuilder<Postgres>,
        org_id: OrgId,
    ) -> CrResult<()> {
        qb.push(" FROM ").push(self.from).push(" WHERE t.org_id = ");
        qb.push_bind(org_id.as_uuid());
        self.scope(qb)?;
        if self.query.window.filter != Cond::Const(true) {
            qb.push(" AND (");
            self.cond(qb, &self.query.window.filter)?;
            qb.push(")");
        }
        Ok(())
    }

    /// The rows strictly behind the cursor in the order.
    fn after(&self, qb: &mut QueryBuilder<Postgres>, after: &[Value]) -> CrResult<()> {
        if after.len() != self.query.window.order.len() {
            return Err(CrError::BadRequest(
                "the cursor is not one of this order".into(),
            ));
        }
        qb.push(" AND (");
        for (at, (key, value)) in self.query.window.order.iter().zip(after).enumerate() {
            qb.push(if at == 0 { "(" } else { " OR (" });
            for (before, seen) in self.query.window.order.iter().zip(after).take(at) {
                let column = self.column(&before.path)?;
                if seen.is_null() {
                    qb.push(column.sql).push(" IS NULL");
                } else {
                    Self::compared(qb, column, " = ", seen);
                }
                qb.push(" AND ");
            }
            let column = self.column(&key.path)?;
            match (key.descending, value.is_null()) {
                (false, true) => {
                    qb.push(column.sql).push(" IS NOT NULL");
                }
                (false, false) => Self::compared(qb, column, " > ", value),
                (true, true) => {
                    qb.push("FALSE");
                }
                (true, false) => {
                    qb.push("(");
                    Self::compared(qb, column, " < ", value);
                    if column.nullable {
                        qb.push(" OR ").push(column.sql).push(" IS NULL");
                    }
                    qb.push(")");
                }
            }
            qb.push(")");
        }
        qb.push(")");
        Ok(())
    }

    fn page(&self, org_id: OrgId) -> CrResult<QueryBuilder<Postgres>> {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT ");
        for (at, column) in self.columns.iter().enumerate() {
            if at > 0 {
                qb.push(", ");
            }
            qb.push(column.sql);
            if column.kind == Kind::Integer {
                qb.push("::int8");
            }
        }
        self.push_scope_and_filter(&mut qb, org_id)?;
        if let Some(after) = &self.query.window.after {
            self.after(&mut qb, after)?;
        }
        for (at, key) in self.query.window.order.iter().enumerate() {
            let column = self.column(&key.path)?;
            qb.push(if at == 0 { " ORDER BY " } else { ", " });
            Self::operand(&mut qb, column);
            qb.push(if key.descending { " DESC" } else { " ASC" });
            // Written only where it matters: the key's index has Postgres's own default.
            if column.nullable {
                qb.push(if key.descending {
                    " NULLS LAST"
                } else {
                    " NULLS FIRST"
                });
            }
        }
        if let Some(limit) = self.query.window.limit {
            qb.push(" LIMIT ")
                .push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
        }
        if self.query.window.skip > 0 {
            qb.push(" OFFSET ")
                .push_bind(i64::try_from(self.query.window.skip).unwrap_or(i64::MAX));
        }
        Ok(qb)
    }

    fn row(&self, row: &PgRow) -> Result<Value, sqlx::Error> {
        let mut out = Map::with_capacity(self.columns.len());
        for (at, column) in self.columns.iter().enumerate() {
            let value = match column.kind {
                Kind::Text => row.try_get::<Option<String>, _>(at)?.map(Value::from),
                Kind::Integer => row.try_get::<Option<i64>, _>(at)?.map(Value::from),
                Kind::Boolean => row.try_get::<Option<bool>, _>(at)?.map(Value::from),
                Kind::Time => row
                    .try_get::<Option<TimestampTz>, _>(at)?
                    .map(|time| Value::from(time.to_rfc3339())),
                Kind::Json => row
                    .try_get::<Option<Json<Value>>, _>(at)?
                    .map(|json| json.0),
            };
            out.insert(column.member.to_owned(), value.unwrap_or(Value::Null));
        }
        Ok(Value::Object(out))
    }
}

/// Answers `query` inside `tx`, whose organization setting and snapshot are the caller's.
pub(crate) async fn read(
    tx: &mut Transaction<'_, Postgres>,
    org_id: OrgId,
    query: &RowQuery,
) -> CrResult<RowPage> {
    let failed = |e: sqlx::Error| CrError::Internal(format!("index rows: {e}"));
    let (columns, from) = relation(query.set);
    let statement = Statement {
        query,
        columns,
        from,
    };
    let mut out = RowPage::default();
    if query.window.limit != Some(0) {
        let rows = statement
            .page(org_id)?
            .build()
            .fetch_all(&mut **tx)
            .await
            .map_err(failed)?;
        out.rows = rows
            .iter()
            .map(|row| statement.row(row))
            .collect::<Result<_, _>>()
            .map_err(failed)?;
    }
    if query.window.count {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT count(*)");
        statement.push_scope_and_filter(&mut qb, org_id)?;
        let counted: i64 = qb
            .build_query_scalar()
            .fetch_one(&mut **tx)
            .await
            .map_err(failed)?;
        out.count = Some(u64::try_from(counted).unwrap_or(0));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The column tables are built at compile time; the builders still mark each column's nullability.
    #[test]
    fn columns_are_required_unless_built_nullable() {
        let c = column("uid", "t.uid", Kind::Text);
        assert_eq!((c.member, c.sql, c.nullable), ("uid", "t.uid", false));
        assert!(c.kind == Kind::Text);
        let n = nullable("folderUid", "t.folder_uid", Kind::Text);
        assert_eq!(
            (n.member, n.sql, n.nullable),
            ("folderUid", "t.folder_uid", true)
        );
        assert!(DASHBOARDS.iter().any(|c| c.nullable) && DASHBOARDS.iter().any(|c| !c.nullable));
    }
}
