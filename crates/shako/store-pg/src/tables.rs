//! The typed tables (T-7508). Their columns are what is served: the Portal's Organization backup
//! exports and restores them by column. A row and a record are the same JSON, with the keys in
//! snake case and the few differences [`to_row`] and [`to_record`] name.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use shako_core::{InstanceId, OrgId, ShakoError, ShakoResult};
use sqlx::{AssertSqlSafe, Postgres, Transaction};
use std::fmt::Write as _;

/// A typed table and the key, beside the tenant, the Store names one of its rows by.
#[derive(Debug)]
pub(crate) struct Table {
    name: &'static str,
    key: &'static [&'static str],
    has_id: bool,
    /// What a read and a write's `RETURNING` take of a row `t`.
    read: &'static str,
}

/// A whole row; `row_to_json`, as `to_jsonb` takes half as long again on a list.
const WHOLE_ROW: &str = "row_to_json(t)";

const fn table(name: &'static str, key: &'static [&'static str]) -> Table {
    Table {
        name,
        key,
        has_id: true,
        read: WHOLE_ROW,
    }
}

pub(crate) const INSTANCE: Table = Table {
    name: "instance",
    key: &[],
    has_id: false,
    read: WHOLE_ROW,
};
pub(crate) const QUOTA_USAGE: Table = Table {
    name: "quota_usage",
    key: &[],
    has_id: false,
    read: WHOLE_ROW,
};
pub(crate) const REPO: Table = table("repo", &[]);
/// Branch and tag rules share the table and differ in `kind`.
pub(crate) const PROTECTION_RULE: Table =
    table("protection_rule", &["repo_id", "kind", "rule_name"]);
pub(crate) const PULL_REQUEST: Table = table("pull_request", &["repo_id", "number"]);
pub(crate) const REVIEW: Table = table("review", &[]);
pub(crate) const COMMIT_STATUS: Table = table("commit_status", &["repo_id", "sha", "context"]);
pub(crate) const WEBHOOK: Table = table("webhook", &[]);
pub(crate) const DELIVERY: Table = table("delivery", &[]);
pub(crate) const PUSH_MIRROR: Table = table("push_mirror", &["repo_id", "remote_name"]);
pub(crate) const PACKAGE: Table = table("package", &["owner", "package_type", "name"]);
/// Never `manifest_bytes`: a manifest is a blob (T-7523).
pub(crate) const PACKAGE_VERSION: Table = Table {
    read: "json_build_object('id', t.id, 'instance_id', t.instance_id, 'org_id', t.org_id, \
           'package_id', t.package_id, 'owner', t.owner, 'package_type', t.package_type, \
           'package_name', t.package_name, 'version', t.version, 'digest', t.digest, \
           'media_type', t.media_type, 'subject_digest', t.subject_digest, \
           'artifact_type', t.artifact_type, 'size_bytes', t.size_bytes, 'signed', t.signed, \
           'created_at', t.created_at)",
    ..table(
        "package_version",
        &["owner", "package_type", "package_name", "version"],
    )
};
pub(crate) const PACKAGE_FILE: Table = table(
    "package_file",
    &["owner", "package_name", "version", "filename"],
);
pub(crate) const BLOB: Table = table("blob", &["owner", "repo_name", "digest"]);
pub(crate) const OUTBOX: Table = table("outbox", &[]);

pub(crate) fn failed(e: sqlx::Error) -> ShakoError {
    let duplicate = e
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation);
    if duplicate {
        ShakoError::Conflict("a record with that key already exists".into())
    } else {
        ShakoError::Storage(e.to_string())
    }
}

fn unreadable(e: impl std::fmt::Display) -> ShakoError {
    ShakoError::Storage(format!("stored record unreadable: {e}"))
}

fn snake(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for c in key.chars() {
        if c == '_' {
            upper = true;
        } else if std::mem::take(&mut upper) {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The row of a record as serde writes it (the hidden fields included).
pub(crate) fn to_row(table: &Table, record: Value) -> ShakoResult<Map<String, Value>> {
    let Value::Object(record) = record else {
        return Err(ShakoError::Storage("a record is a JSON object".into()));
    };
    let mut row: Map<String, Value> = record.into_iter().map(|(k, v)| (snake(&k), v)).collect();
    match table.name {
        "push_mirror" => {
            if let Some(interval) = row.remove("interval") {
                row.insert("interval_str".into(), interval);
            }
        }
        "protection_rule" => {
            if let Some(rule) = row.remove("rule") {
                row.insert("doc".into(), rule);
            }
        }
        "package_version" => {
            if let Some(bytes) = row.remove("manifest_bytes") {
                let bytes: Vec<u8> = serde_json::from_value(bytes).map_err(unreadable)?;
                let mut hex = String::with_capacity(2 + bytes.len() * 2);
                hex.push_str("\\x");
                for byte in bytes {
                    write!(hex, "{byte:02x}").map_err(unreadable)?;
                }
                row.insert("manifest_bytes".into(), hex.into());
            }
        }
        _ => {}
    }
    Ok(row)
}

/// The record of a row as Postgres writes it in JSON.
pub(crate) fn to_record(table: &Table, row: Value) -> ShakoResult<Value> {
    let Value::Object(mut row) = row else {
        return Err(ShakoError::Storage("a row is a JSON object".into()));
    };
    match table.name {
        "push_mirror" => {
            if let Some(interval) = row.remove("interval_str") {
                row.insert("interval".into(), interval);
            }
        }
        "protection_rule" => {
            if let Some(doc) = row.remove("doc") {
                row.insert("rule".into(), doc);
            }
        }
        _ => {}
    }
    Ok(Value::Object(
        row.into_iter().map(|(k, v)| (camel(&k), v)).collect(),
    ))
}

/// What a write does when the key or id is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Taken {
    /// The new row replaces it.
    Replace,
    /// It stays, and the write answers `None`.
    Keep,
}

/// Writes `row`; answers the record as stored.
pub(crate) async fn put(
    tx: &mut Transaction<'_, Postgres>,
    table: &Table,
    row: Map<String, Value>,
    taken: Taken,
) -> ShakoResult<Option<Value>> {
    let columns: Vec<&str> = row.keys().map(String::as_str).collect();
    // The names are our own records' field names; nothing else becomes an identifier.
    if !columns.iter().all(|c| {
        !c.is_empty()
            && c.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }) {
        return Err(ShakoError::Storage("a field is no column name".into()));
    }
    let columns: Vec<String> = columns.iter().map(|c| format!("\"{c}\"")).collect();
    let update = columns
        .iter()
        .map(|c| format!("{c} = EXCLUDED.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let columns = columns.join(", ");
    let name = table.name;
    let doc = Value::Object(row);
    if taken == Taken::Replace && !table.key.is_empty() {
        let same: Vec<String> = table.key.iter().map(|k| format!("t.{k} = r.{k}")).collect();
        let same = same.join(" AND ");
        sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM shako.{name} t USING jsonb_populate_record(NULL::shako.{name}, $1) r
             WHERE t.instance_id = r.instance_id AND t.org_id = r.org_id
               AND t.id <> r.id AND {same}"
        )))
        .bind(&doc)
        .execute(&mut **tx)
        .await
        .map_err(failed)?;
    }
    let pk = if table.has_id {
        "id"
    } else {
        "instance_id, org_id"
    };
    // Updated in place: a column no record carries keeps its value.
    let conflict = match taken {
        Taken::Replace => format!("ON CONFLICT ({pk}) DO UPDATE SET {update}"),
        Taken::Keep => "ON CONFLICT DO NOTHING".to_owned(),
    };
    let read = table.read;
    let stored: Option<Value> = sqlx::query_scalar(AssertSqlSafe(format!(
        "INSERT INTO shako.{name} AS t ({columns})
         SELECT {columns} FROM jsonb_populate_record(NULL::shako.{name}, $1) {conflict}
         RETURNING {read}"
    )))
    .bind(&doc)
    .fetch_optional(&mut **tx)
    .await
    .map_err(failed)?;
    stored.map(|row| to_record(table, row)).transpose()
}

/// The tenant's records `rest` selects: SQL after the tenant's predicate, with `binds` from $3.
pub(crate) async fn select<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Postgres>,
    tenant: (InstanceId, OrgId),
    table: &Table,
    rest: &'static str,
    binds: &[Option<String>],
) -> ShakoResult<Vec<T>> {
    // Postgres holds no NUL in text, so nothing stored has such a name.
    if binds.iter().flatten().any(|b| b.contains('\0')) {
        return Ok(Vec::new());
    }
    let (name, read) = (table.name, table.read);
    let mut query = sqlx::query_scalar::<_, Value>(AssertSqlSafe(format!(
        "SELECT {read} FROM shako.{name} t WHERE t.instance_id = $1 AND t.org_id = $2 {rest}"
    )))
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid());
    for bind in binds {
        query = query.bind(bind.clone());
    }
    query
        .fetch_all(&mut **tx)
        .await
        .map_err(failed)?
        .into_iter()
        .map(|row| serde_json::from_value(to_record(table, row)?).map_err(unreadable))
        .collect()
}
