#![forbid(unsafe_code)]
//! The OData entity sets (R10, ODA-05): the caller's read scope, filter, order and keyset page
//! go to the store, which answers index rows; every row it answers is still checked by the
//! Policy, and a disagreement fails the request.
//!
//! A `$skiptoken` is the order and the last row's values. It carries no right: the scope and
//! the check apply to every page whatever it says, and its values are bound like a filter's.

use crate::odata::{can_read_organization, map_table_error, parse_odata_query_options};
use crate::state::DaiyaState;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daiya_model::rows::page_rows;
use daiya_model::{
    Action, CrError, CrResult, ObjectTarget, RequestCtx, RowQuery, RowSet, RowWindow,
};
use jc_time::TimestampTz;
use odata_table::{
    Cond, Fetched, Fetching, FieldKind, Operation, Plan, Scope, SortKey, Table, TableError,
    TableInfo, allowed as table_allowed, key_options, plan as table_plan, query as table_query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) const MAX_PAGE_SIZE: u64 = 500;
const MAX_EXPORT_ROWS: usize = 10_000;
/// The longest `$skiptoken` this service reads: an order holds eight properties and the key.
const MAX_CURSOR_BYTES: usize = 32 * 1024;

fn refused(state: &DaiyaState, ctx: &RequestCtx, set: RowSet, rows: &[Value]) -> Option<CrError> {
    rows.iter()
        .any(|row| {
            set.target(ctx.org_id, row)
                .is_none_or(|target| !state.authorizer.check(ctx, Action::Read, &target).allowed)
        })
        .then(|| {
            CrError::Internal(format!(
                "the read scope and the read decision disagree on a row of {}",
                set.name()
            ))
        })
}

/// Every index row of `set` the caller may read, by key: the lists that answer whole.
pub async fn readable_rows(
    state: &DaiyaState,
    ctx: &RequestCtx,
    set: RowSet,
) -> CrResult<Vec<Value>> {
    let scope = state.authorizer.read_scope(ctx, set.read_kind());
    let page = state
        .store
        .read_rows(ctx.org_id, RowQuery::every(set, scope))
        .await?;
    match refused(state, ctx, set, &page.rows) {
        Some(error) => Err(error),
        None => Ok(page.rows),
    }
}

#[derive(Debug, Clone, Copy)]
enum Source {
    Stored(RowSet),
    /// The process's own table, bounded by the cache.
    CacheStats,
}

/// Where a page starts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    set: String,
    order: Vec<(String, bool)>,
    /// The rows the pages before answered, for `$top`.
    seen: u64,
    after: Vec<Value>,
}

fn order_of(order: &[SortKey]) -> Vec<(String, bool)> {
    order
        .iter()
        .map(|key| (key.path.join("/"), key.descending))
        .collect()
}

fn not_ours() -> CrError {
    CrError::BadRequest("the $skiptoken is not one this service wrote".into())
}

/// A literal on a time as the rows write it; a date is its midnight in UTC.
fn canonical_time(text: &str) -> Option<String> {
    text.parse::<TimestampTz>()
        .ok()
        .or_else(|| jc_time::parse_date(text).and_then(TimestampTz::from_date))
        .map(TimestampTz::to_rfc3339)
}

fn without_nul(text: &str) -> Result<(), TableError> {
    if text.contains('\0') {
        return Err(TableError::BadRequest(
            "a value must not hold a NUL character".into(),
        ));
    }
    Ok(())
}

/// The query string without its `$skiptoken`, and that token.
fn split_token(raw: Option<&str>) -> CrResult<(Option<String>, Option<String>)> {
    let mut rest = Vec::new();
    let mut token = None;
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = percent_encoding::percent_decode_str(name).decode_utf8_lossy();
        if !decoded.trim().eq_ignore_ascii_case("$skiptoken") {
            rest.push(pair);
        } else if token.replace(value.to_owned()).is_some() {
            return Err(not_ours());
        }
    }
    Ok(((!rest.is_empty()).then(|| rest.join("&")), token))
}

/// What a request asks of a set, its `$skiptoken` read and checked.
struct Asked {
    options: odata_url::QueryOptions,
    plan: Plan,
    /// The query string a next link repeats.
    rest: Option<String>,
}

/// One entity set as `odata-table` queries it.
struct SetTable<'a> {
    state: &'a DaiyaState,
    ctx: &'a RequestCtx,
    info: TableInfo,
    source: Source,
    kinds: Vec<(&'a str, &'a FieldKind)>,
    after: Option<Vec<Value>>,
}

impl<'a> SetTable<'a> {
    fn open(state: &'a DaiyaState, ctx: &'a RequestCtx, name: &str) -> CrResult<Self> {
        let unknown = || CrError::NotFound(format!("unknown OData entity set '{name}'"));
        let service = &state.registry.service;
        let source = match name {
            "QueryCacheStats" => Source::CacheStats,
            _ => Source::Stored(RowSet::parse(name).ok_or_else(unknown)?),
        };
        let fields = &service
            .sets
            .iter()
            .find(|def| def.set == name)
            .ok_or_else(unknown)?
            .fields;
        Ok(Self {
            state,
            ctx,
            info: service
                .info(Arc::new(service.model()), name)
                .ok_or_else(unknown)?,
            source,
            kinds: fields
                .iter()
                .map(|field| (field.name.as_str(), &field.kind))
                .collect(),
            after: None,
        })
    }

    fn kind(&self, path: &[String]) -> Option<&'a FieldKind> {
        let [name] = path else { return None };
        self.kinds
            .iter()
            .find(|(field, _)| *field == name.as_str())
            .map(|(_, kind)| *kind)
    }

    fn is_time(&self, path: &[String]) -> bool {
        matches!(self.kind(path), Some(FieldKind::DateTime))
    }

    /// The filter as both stores read it alike: times in the rows' own spelling, no NUL.
    fn normalised(&self, cond: &Cond) -> Result<Cond, TableError> {
        let both = |a: &Cond, b: &Cond| -> Result<(Box<Cond>, Box<Cond>), TableError> {
            Ok((Box::new(self.normalised(a)?), Box::new(self.normalised(b)?)))
        };
        Ok(match cond {
            Cond::And(a, b) => both(a, b).map(|(a, b)| Cond::And(a, b))?,
            Cond::Or(a, b) => both(a, b).map(|(a, b)| Cond::Or(a, b))?,
            Cond::Not(inner) => Cond::Not(Box::new(self.normalised(inner)?)),
            Cond::Compare { path, op, value } => {
                let mut value = value.clone();
                if let Value::String(text) = &value {
                    without_nul(text)?;
                    if self.is_time(path) {
                        match canonical_time(text) {
                            Some(time) => value = Value::String(time),
                            None => return Ok(Cond::Const(false)),
                        }
                    }
                }
                Cond::Compare {
                    path: path.clone(),
                    op: *op,
                    value,
                }
            }
            Cond::Text { path, needle, .. } => {
                without_nul(needle)?;
                if self.is_time(path) {
                    return Err(TableError::BadRequest(format!(
                        "{} is a time: compare it with eq, gt, ge, lt or le",
                        path.join("/")
                    )));
                }
                cond.clone()
            }
            Cond::Const(_) | Cond::IsNull { .. } => cond.clone(),
        })
    }

    fn decode_cursor(&self, token: &str, order: &[SortKey]) -> CrResult<Cursor> {
        if token.len() > MAX_CURSOR_BYTES {
            return Err(not_ours());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| not_ours())?;
        let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| not_ours())?;
        let typed = |key: &SortKey, value: &Value| match (self.kind(&key.path), value) {
            (_, Value::Null) => true,
            (Some(FieldKind::String), Value::String(text)) => !text.contains('\0'),
            (Some(FieldKind::DateTime), Value::String(text)) => {
                canonical_time(text).as_deref() == Some(text.as_str())
            }
            (Some(FieldKind::Integer), Value::Number(number)) => number.is_i64(),
            (Some(FieldKind::Boolean), Value::Bool(_)) => true,
            _ => false,
        };
        let ours = cursor.set == self.info.set
            && cursor.order == order_of(order)
            && cursor.after.len() == order.len()
            && order
                .iter()
                .zip(&cursor.after)
                .all(|(key, v)| typed(key, v));
        ours.then_some(cursor).ok_or_else(not_ours)
    }

    /// Reads the query string and starts the table behind its `$skiptoken`.
    fn ask(&mut self, raw: Option<&str>, max_page: u64) -> CrResult<Asked> {
        let (rest, token) = split_token(raw)?;
        let mut options =
            parse_odata_query_options(&self.info.model, &self.info.set, rest.as_deref())?;
        if options.skiptoken.is_some() {
            return Err(not_ours());
        }
        let plan = table_plan(&self.info, &options, max_page).map_err(map_table_error)?;
        if let Some(token) = &token {
            let cursor = self.decode_cursor(token, &plan.order)?;
            // `odata-table` counts `$top` down by this; the rows start behind `after`.
            options.skiptoken = Some(cursor.seen.to_string());
            self.after = Some(cursor.after);
        }
        Ok(Asked {
            options,
            plan,
            rest,
        })
    }

    /// One page with every declared property, and the cursor of the next one.
    async fn page(
        &self,
        asked: &Asked,
        scope: &Scope,
        max_page: u64,
    ) -> CrResult<(odata_table::Answer, Option<Cursor>)> {
        // The cursor needs the order's values, which `$select` may leave out.
        let mut whole = asked.options.clone();
        whole.select = None;
        let answer = table_query(self, &whole, scope, max_page)
            .await
            .map_err(map_table_error)?;
        let next = match (answer.next, answer.rows.last()) {
            (Some(seen), Some(last)) => Some(Cursor {
                set: self.info.set.clone(),
                order: order_of(&asked.plan.order),
                seen,
                after: asked
                    .plan
                    .order
                    .iter()
                    .map(|key| {
                        key.path
                            .first()
                            .and_then(|name| last.get(name))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            }),
            _ => None,
        };
        Ok((answer, next))
    }

    fn cache_stats(&self) -> Vec<Value> {
        let (state, ctx) = (self.state, self.ctx);
        if !can_read_organization(state, ctx) {
            return Vec::new();
        }
        let readable = |source: &str| {
            let target = match source.strip_prefix("public:") {
                Some(dashboard) => ObjectTarget::PublicDashboard {
                    org_id: ctx.org_id,
                    dashboard_uid: dashboard.to_owned(),
                },
                None => ObjectTarget::Datasource {
                    org_id: ctx.org_id,
                    datasource_uid: source.to_owned(),
                },
            };
            state.authorizer.check(ctx, Action::Read, &target).allowed
        };
        state
            .query_cache
            .stats_for_org(ctx.org_id, ctx.instance_id)
            .into_iter()
            .filter(|s| readable(&s.datasource_uid))
            .map(|s| {
                json!({
                    "id": s.id,
                    "datasourceUid": s.datasource_uid,
                    "hits": s.hits,
                    "misses": s.misses,
                    "entries": s.entries,
                    "bytesUsed": s.bytes_used,
                })
            })
            .collect()
    }

    async fn read(&self, plan: &Plan, scope: &Scope) -> Result<Fetched, TableError> {
        let mut filter = self.normalised(&plan.filter)?;
        let restricted = scope.cond();
        if restricted != Cond::Const(true) {
            filter = Cond::And(Box::new(restricted), Box::new(filter));
        }
        let window = RowWindow {
            filter,
            order: plan.order.clone(),
            after: self.after.clone(),
            // `$skip` belongs to the first page: a cursor is already behind it.
            skip: if self.after.is_some() { 0 } else { plan.offset },
            limit: Some(plan.fetch),
            count: plan.count,
        };
        let page = match self.source {
            Source::CacheStats => page_rows(self.cache_stats(), &window),
            Source::Stored(set) => {
                let read = self.state.authorizer.read_scope(self.ctx, set.read_kind());
                let page = self
                    .state
                    .store
                    .read_rows(
                        self.ctx.org_id,
                        RowQuery {
                            set,
                            scope: read,
                            window,
                        },
                    )
                    .await
                    .map_err(|e| match e {
                        CrError::BadRequest(why) => TableError::BadRequest(why),
                        other => TableError::Source(other.to_string()),
                    })?;
                if let Some(error) = refused(self.state, self.ctx, set, &page.rows) {
                    return Err(TableError::Source(error.to_string()));
                }
                page
            }
        };
        Ok(Fetched {
            rows: page.rows,
            count: page.count,
        })
    }
}

impl Table for SetTable<'_> {
    fn info(&self) -> &TableInfo {
        &self.info
    }

    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a> {
        Box::pin(self.read(plan, scope))
    }
}

fn selected(rows: Vec<Value>, select: Option<&Vec<String>>) -> Vec<Value> {
    let Some(names) = select else { return rows };
    rows.into_iter()
        .map(|row| {
            Value::Object(
                names
                    .iter()
                    .map(|name| (name.clone(), row.get(name).cloned().unwrap_or(Value::Null)))
                    .collect(),
            )
        })
        .collect()
}

/// `GET Set('key')`.
pub(crate) async fn entity(
    state: &DaiyaState,
    ctx: &RequestCtx,
    scope: &Scope,
    name: &str,
    key: String,
) -> CrResult<Value> {
    let table = SetTable::open(state, ctx, name)?;
    let options = key_options(&table.info, &Value::String(key), None).map_err(map_table_error)?;
    let mut answer = table_query(&table, &options, scope, 1)
        .await
        .map_err(map_table_error)?;
    answer
        .rows
        .pop()
        .ok_or_else(|| CrError::NotFound(format!("{name} entity not found")))
}

/// `GET Set`: one page, its count when asked and the link to the next.
pub(crate) async fn list(
    state: &DaiyaState,
    ctx: &RequestCtx,
    scope: &Scope,
    name: &str,
    raw: Option<&str>,
) -> CrResult<Value> {
    let mut table = SetTable::open(state, ctx, name)?;
    let asked = table.ask(raw, MAX_PAGE_SIZE)?;
    let (answer, next) = table.page(&asked, scope, MAX_PAGE_SIZE).await?;
    let mut payload = json!({
        "@odata.context": format!("$metadata#{name}"),
        "value": selected(answer.rows, asked.plan.select.as_ref()),
    });
    if let Some(count) = answer.count {
        payload["@odata.count"] = Value::from(count);
    }
    if let Some(next) = next {
        let token = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&next).map_err(|e| CrError::Internal(format!("$skiptoken: {e}")))?,
        );
        let options = asked.rest.map(|rest| rest + "&").unwrap_or_default();
        payload["@odata.nextLink"] = Value::String(format!("{name}?{options}$skiptoken={token}"));
    }
    Ok(payload)
}

/// `GET Set/$count`: the count alone, no row read.
pub(crate) async fn count(
    state: &DaiyaState,
    ctx: &RequestCtx,
    scope: &Scope,
    name: &str,
    raw: Option<&str>,
) -> CrResult<u64> {
    let mut table = SetTable::open(state, ctx, name)?;
    let mut asked = table.ask(raw, 1)?;
    asked.options.count = Some(true);
    asked.options.top = Some(0);
    table.after = None;
    let answer = table_query(&table, &asked.options, scope, 1)
        .await
        .map_err(map_table_error)?;
    Ok(answer.count.unwrap_or(0))
}

/// `GET Set/$export`: page after page by cursor, so no page reads the ones before it.
pub(crate) async fn export(
    state: &DaiyaState,
    ctx: &RequestCtx,
    scope: &Scope,
    name: &str,
    raw: Option<&str>,
) -> CrResult<Vec<Value>> {
    let mut table = SetTable::open(state, ctx, name)?;
    let mut asked = table.ask(raw, MAX_PAGE_SIZE)?;
    asked.options.count = None;
    let mut rows = Vec::new();
    loop {
        let (answer, next) = table.page(&asked, scope, MAX_PAGE_SIZE).await?;
        rows.extend(answer.rows);
        if rows.len() > MAX_EXPORT_ROWS {
            return Err(CrError::BadRequest(format!(
                "the export is over {MAX_EXPORT_ROWS} rows: narrow it with $filter"
            )));
        }
        let Some(next) = next else { break };
        asked.options.skiptoken = Some(next.seen.to_string());
        table.after = Some(next.after);
    }
    Ok(selected(rows, asked.plan.select.as_ref()))
}

/// `GET Set/$allowed`.
pub(crate) async fn allowed(
    state: &DaiyaState,
    ctx: &RequestCtx,
    scope: &Scope,
    name: &str,
    operation: Operation,
    key: Option<Value>,
) -> CrResult<Value> {
    let table = SetTable::open(state, ctx, name)?;
    let decision = table_allowed(&table, scope, operation, key.as_ref())
        .await
        .map_err(map_table_error)?;
    Ok(decision.to_json())
}
