// SPDX-License-Identifier: MIT
//! Writes through the read filter (T-6659): an update or a delete names its rows by a key or a
//! `$filter`, resolved by the same [`query`](crate::query) inside the caller's [`Scope`]. Every matched row is
//! checked against what the caller may change before anything changes; one row outside refuses
//! the whole write. A dry run answers what would change and changes nothing.

use serde_json::{Map, Value};

use crate::eval::matches;
use crate::{allowed, export, plan, Cond, Operation, QueryOptions, Scope, Table, TableError};

/// What a write does to each matched row.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Sets these top-level properties (`null` clears one); the others keep their values.
    Update(Map<String, Value>),
    /// Removes the row.
    Delete,
}

impl Change {
    fn operation(&self) -> Operation {
        match self {
            Self::Update(_) => Operation::Update,
            Self::Delete => Operation::Delete,
        }
    }
}

/// What a source applies: the change, the keys of the rows it applies to, and the condition
/// each of those rows must still meet when it is changed (the read scope, the change scope and
/// the filter), so a row that moved since it was matched refuses the batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    /// The change.
    pub change: Change,
    /// The matched rows' keys.
    pub keys: Vec<Value>,
    /// The condition every one of those rows still meets.
    pub guard: Cond,
}

/// The answer to a write: the rows it changed (or would change, on a dry run).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Written {
    /// Whether nothing was changed because the caller asked for a dry run.
    pub dry_run: bool,
    /// Each changed row's key.
    pub keys: Vec<Value>,
    /// Each changed row: as it is after an update, as it was before a delete.
    pub rows: Vec<Value>,
}

/// Whether `scope` may change the visible `row`: the same test [`allowed`] runs for one key.
#[must_use]
pub fn may_change(scope: &Scope, row: &Value) -> bool {
    matches(row, &scope.change_cond())
}

/// At most this many keys are named in a refusal.
const NAMED: usize = 10;

/// Applies `change` to the rows `options` (a `$filter`, nothing else) matches on `table`
/// within `scope`, all or nothing, at most `max_rows` rows; with `dry_run` it answers the same
/// rows and changes nothing.
///
/// # Errors
///
/// [`TableError::Forbidden`] when the operation is not offered, the permission is not held,
/// the source is read only, a matched row is outside what the caller may change, or an update
/// would move a row out of it; [`TableError::BadRequest`] for a query option other than
/// `$filter`, more than `max_rows` rows, or an update naming the key, an unknown property or a
/// value of the wrong type; [`TableError::Conflict`] when a row changed between the match and
/// the write; the source's failure.
pub async fn write<T: Table + ?Sized>(
    table: &T,
    options: &QueryOptions,
    scope: &Scope,
    change: Change,
    dry_run: bool,
    max_rows: usize,
) -> Result<Written, TableError> {
    let decision = allowed(table, scope, change.operation(), None).await?;
    if !decision.allowed {
        return Err(TableError::Forbidden(decision.reason));
    }
    let only = QueryOptions {
        filter: options.filter.clone(),
        aliases: options.aliases.clone(),
        ..QueryOptions::default()
    };
    if only != *options {
        return Err(TableError::BadRequest(
            "a write takes $filter only, e.g. $filter=org eq 'a'".into(),
        ));
    }
    let info = table.info();
    if let Change::Update(patch) = &change {
        checked(info, patch)?;
    }
    let page = u64::try_from(max_rows.clamp(1, 1000)).unwrap_or(1000);
    let matched = export(table, &only, scope, page, max_rows).await?;
    let key_of = |row: &Value| row.get(&info.key).cloned().unwrap_or(Value::Null);
    let outside: Vec<String> = matched
        .iter()
        .filter(|row| !may_change(scope, row))
        .map(|row| key_of(row).to_string())
        .collect();
    if !outside.is_empty() {
        return Err(TableError::Forbidden(format!(
            "{} of the {} rows the filter matches are outside what you may change ({}); nothing was changed",
            outside.len(),
            matched.len(),
            named(&outside)
        )));
    }
    let rows: Vec<Value> = match &change {
        Change::Delete => matched,
        Change::Update(patch) => {
            let read = scope.cond();
            let mut after = Vec::with_capacity(matched.len());
            for mut row in matched {
                if let Some(o) = row.as_object_mut() {
                    o.extend(patch.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                if !matches(&row, &read) || !may_change(scope, &row) {
                    return Err(TableError::Forbidden(format!(
                        "the change would move {} outside what you may change; nothing was changed",
                        key_of(&row)
                    )));
                }
                after.push(row);
            }
            after
        }
    };
    let keys: Vec<Value> = rows.iter().map(key_of).collect();
    if dry_run || keys.is_empty() {
        return Ok(Written {
            dry_run,
            keys,
            rows,
        });
    }
    let filter = plan(info, &only, 1)?.filter;
    let guard = Cond::And(
        Box::new(Cond::And(
            Box::new(scope.cond()),
            Box::new(scope.change_cond()),
        )),
        Box::new(filter),
    );
    table
        .apply(&Batch {
            change,
            keys: keys.clone(),
            guard,
        })
        .await?;
    Ok(Written {
        dry_run: false,
        keys,
        rows,
    })
}

fn named(keys: &[String]) -> String {
    let mut text = keys
        .iter()
        .take(NAMED)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if keys.len() > NAMED {
        text.push_str(", ...");
    }
    text
}

/// Refuses an update that is empty, names the key or a property the entity type lacks, or
/// gives a property a value of another type.
fn checked(info: &crate::TableInfo, patch: &Map<String, Value>) -> Result<(), TableError> {
    if patch.is_empty() {
        return Err(TableError::BadRequest(
            "an update names at least one property, e.g. {\"name\": \"x\"}".into(),
        ));
    }
    let model = &info.model;
    let ty = model
        .entity_type(&info.entity_type)
        .ok_or_else(|| TableError::Source(format!("{} has no entity type", info.set)))?;
    let lineage = model.lineage(ty);
    for (name, value) in patch {
        if *name == info.key {
            return Err(TableError::BadRequest(format!(
                "{name} is the key and cannot be changed"
            )));
        }
        let property = lineage
            .iter()
            .find_map(|t| t.properties.iter().find(|p| p.name == *name))
            .ok_or_else(|| TableError::UnknownProperty(name.clone()))?;
        typed(model, property, value, name)?;
    }
    Ok(())
}

/// Refuses `value` when it is not of `property`'s type.
fn typed(
    model: &odata_csdl::Model,
    property: &odata_csdl::Property,
    value: &Value,
    path: &str,
) -> Result<(), TableError> {
    let wrong = |what: &str| TableError::BadRequest(format!("{path} takes {what}"));
    if value.is_null() {
        return if property.nullable {
            Ok(())
        } else {
            Err(wrong("a value, not null"))
        };
    }
    if property.ty.collection {
        let Value::Array(items) = value else {
            return Err(wrong("a list"));
        };
        let item = odata_csdl::Property {
            ty: odata_csdl::TypeRef::single(&property.ty.name),
            ..property.clone()
        };
        return items.iter().try_for_each(|v| typed(model, &item, v, path));
    }
    let ok = match property.ty.name.as_str() {
        "Edm.String" | "Edm.Guid" | "Edm.Date" | "Edm.TimeOfDay" | "Edm.Duration"
        | "Edm.DateTimeOffset" => value.is_string(),
        "Edm.Int64" | "Edm.Int32" | "Edm.Int16" | "Edm.Byte" | "Edm.SByte" => value.is_i64(),
        "Edm.Double" | "Edm.Single" | "Edm.Decimal" => value.is_number(),
        "Edm.Boolean" => value.is_boolean(),
        name => {
            let Some(complex) = model.complex_type(name) else {
                return Err(TableError::NotImplemented(format!("writing {name}")));
            };
            let Value::Object(members) = value else {
                return Err(wrong("an object"));
            };
            for (member, v) in members {
                let inner = complex
                    .properties
                    .iter()
                    .find(|p| p.name == *member)
                    .ok_or_else(|| TableError::UnknownProperty(format!("{path}/{member}")))?;
                typed(model, inner, v, &format!("{path}/{member}"))?;
            }
            return Ok(());
        }
    };
    if ok {
        Ok(())
    } else {
        Err(wrong(match property.ty.name.as_str() {
            "Edm.Boolean" => "true or false",
            "Edm.Double" | "Edm.Single" | "Edm.Decimal" => "a number",
            n if n.starts_with("Edm.Int") || n.ends_with("Byte") => "a whole number",
            _ => "text",
        }))
    }
}

/// The positions in `rows` of `batch`'s keys, each row still meeting its guard: what an
/// in-memory source checks under its write lock before it applies the batch.
pub(crate) fn located(rows: &[Value], key: &str, batch: &Batch) -> Result<Vec<usize>, TableError> {
    batch
        .keys
        .iter()
        .map(|k| {
            rows.iter()
                .position(|row| row.get(key) == Some(k))
                .filter(|at| rows.get(*at).is_some_and(|row| matches(row, &batch.guard)))
                .ok_or_else(|| moved(k))
        })
        .collect()
}

/// The refusal of a batch whose row `key` changed since it was matched.
pub(crate) fn moved(key: &Value) -> TableError {
    TableError::Conflict(format!(
        "{key} changed since the filter matched it; nothing was changed, read the rows again and retry"
    ))
}

/// `rows` with `batch` applied at `at` (the positions [`located`] answered).
pub(crate) fn applied(rows: &mut Vec<Value>, at: &[usize], change: &Change) {
    match change {
        Change::Update(patch) => {
            for i in at {
                if let Some(Value::Object(o)) = rows.get_mut(*i) {
                    o.extend(patch.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
        }
        Change::Delete => {
            let mut i = 0usize;
            rows.retain(|_| {
                let keep = !at.contains(&i);
                i = i.saturating_add(1);
                keep
            });
        }
    }
}
