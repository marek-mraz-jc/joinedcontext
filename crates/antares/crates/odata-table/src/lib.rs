// SPDX-License-Identifier: MIT
//! OData 4.01 entity sets over any row source (T-6642): one [`Table`] interface, one query
//! [`Plan`], and three sources behind it.
//!
//! - [`MemoryTable`]: rows held in memory. Its evaluator is the reference semantics.
//! - `PgTable` (feature `postgres`): the plan pushed down to SQL with bound parameters only.
//! - `GitTable` (feature `git`): the YAML and JSON files of a repository at one commit.
//!
//! [`plan()`] turns parsed [`QueryOptions`] into a [`Plan`] once, for every source: a construct
//! outside the supported subset answers [`TableError::NotImplemented`] and an unknown property
//! [`TableError::UnknownProperty`] whichever source serves the set, so the sources cannot
//! disagree on what they accept. A [`Scope`] is the caller's row restriction (organization,
//! project, Policy); each source applies it before `$filter`, so a filter can only narrow it.
//!
//! The supported subset: `$filter` with `eq ne gt ge lt le and or not in`, `contains`,
//! `startswith`, `endswith`, null comparisons and nested complex property paths; `$orderby` on
//! primitive paths (nulls first ascending, last descending, the key breaking ties); `$select` of
//! top-level properties; `$top`, `$skip`, `$count` and server-driven paging by `$skiptoken`.
//!
//! ```
//! use std::sync::Arc;
//! use odata_table::{query, MemoryTable, Scope, TableInfo};
//! # use odata_csdl::{CsdlVersion, EntityContainer, EntitySet, EntityType, Model, Property, Schema, TypeRef};
//! # fn main() -> Result<(), String> {
//! # let mut ty = EntityType::new("Building");
//! # ty.key = vec!["id".into()];
//! # ty.properties = vec![Property::new("id", TypeRef::single("Edm.String")), Property::new("height", TypeRef::single("Edm.Int64"))];
//! # let mut city = Schema::new("City");
//! # city.entity_types.push(ty);
//! # let mut service = EntityContainer::new("Service");
//! # service.entity_sets.push(EntitySet::new("Buildings", "City.Building"));
//! # city.entity_container = Some(service);
//! # let model = Arc::new(Model { version: CsdlVersion::V4_01, references: vec![], schemas: vec![city] });
//! let info = TableInfo::new(model.clone(), "Buildings").ok_or("no set")?;
//! let rows = vec![
//!     serde_json::json!({"id": "b1", "height": 12}),
//!     serde_json::json!({"id": "b2", "height": 7}),
//! ];
//! let table = MemoryTable::new(info, rows).map_err(|e| e.to_string())?;
//! let uri = odata_url::parse("Buildings?$filter=height%20gt%2010", &odata_url::EdmModel(&model), &odata_url::Limits::strict())
//!     .map_err(|e| format!("{e:?}"))?;
//! let odata_url::ODataUri::Resource { options, .. } = uri else { return Err("not a resource".into()) };
//! let runtime = tokio::runtime::Builder::new_current_thread().build().map_err(|e| e.to_string())?;
//! let answer = runtime.block_on(query(&table, &options, &Scope::all(), 100)).map_err(|e| e.to_string())?;
//! assert_eq!(answer.rows, vec![serde_json::json!({"id": "b1", "height": 12})]);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod def;
mod eval;
#[cfg(feature = "git")]
mod git;
mod memory;
#[cfg(feature = "postgres")]
mod pg;
mod plan;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use def::{
    allowed, open_rules, Decision, EntityDef, Field, FieldKind, Operation, Rule, Service,
};
pub use eval::{compare, matches};
#[cfg(feature = "git")]
pub use git::{GitLimits, GitTable};
pub use memory::MemoryTable;
pub use odata_url::QueryOptions;
#[cfg(feature = "postgres")]
pub use pg::PgTable;
pub use plan::{plan, Cond, Plan, SortKey, TextOp};
pub use serde_json::Value;

/// A future a [`Table`] answers, boxed so `dyn Table` serves several sets from one router.
pub type Fetching<'a> = Pin<Box<dyn Future<Output = Result<Fetched, TableError>> + Send + 'a>>;

/// What every source knows of its entity set: the model, the set, its entity type and key.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// The service's model, holding the set.
    pub model: Arc<odata_csdl::Model>,
    /// The entity set's name.
    pub set: String,
    /// The qualified name of the set's entity type.
    pub entity_type: String,
    /// The key property: one primitive property (compound keys are not supported).
    pub key: String,
    /// The entity type's top-level properties, in declaration order: what a row answers.
    pub properties: Vec<String>,
    /// The operations offered and the permission each needs ([`open_rules`] for a set built
    /// from a bare model, the definition's for one built by [`Service::info`]).
    pub rules: Vec<Rule>,
}

impl TableInfo {
    /// The set `set` of `model`, when it names one whose entity type has a single-property key.
    #[must_use]
    pub fn new(model: Arc<odata_csdl::Model>, set: &str) -> Option<Self> {
        let entity_set = model.entity_set(set)?;
        let ty = model.entity_type(&entity_set.entity_type)?;
        let lineage = model.lineage(ty);
        let key = lineage
            .iter()
            .rev()
            .find(|t| !t.key.is_empty())?
            .key
            .clone();
        let [key] = <[String; 1]>::try_from(key).ok()?;
        let mut properties = Vec::new();
        for t in lineage.iter().rev() {
            properties.extend(t.properties.iter().map(|p| p.name.clone()));
        }
        if !properties.contains(&key) {
            return None;
        }
        Some(Self {
            entity_type: entity_set.entity_type.clone(),
            set: set.to_owned(),
            key,
            properties,
            rules: open_rules(),
            model,
        })
    }
}

/// One condition of a [`Scope`]: the value at `path` is one of `any_of`. An empty `any_of`
/// admits no row.
#[derive(Debug, Clone, PartialEq)]
pub struct Restriction {
    /// A property path of the entity type (`["organization"]`, `["owner", "project"]`).
    pub path: Vec<String>,
    /// The values a row may hold there: strings, numbers or booleans.
    pub any_of: Vec<Value>,
}

/// The permissions a caller holds, checked against each operation's [`Rule`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Permissions {
    /// Every permission.
    All,
    /// None: the default.
    #[default]
    None,
    /// Only these.
    Only(Vec<String>),
}

/// What a caller may read and do: the row restriction, applied by the source before `$filter`
/// (a row is visible when it meets every [`Restriction`]), and the permissions it holds. A
/// request without a scope is refused by the router (deny by default); [`Scope::all`] is the
/// explicit grant of every row and every permission, and the default scope holds no permission.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    /// Every condition a visible row meets.
    pub restrictions: Vec<Restriction>,
    /// The permissions held.
    pub permissions: Permissions,
}

impl Scope {
    /// Every row and every permission.
    #[must_use]
    pub fn all() -> Self {
        Self {
            restrictions: Vec::new(),
            permissions: Permissions::All,
        }
    }

    /// This scope holding only `permissions`.
    #[must_use]
    pub fn holding(mut self, permissions: &[&str]) -> Self {
        self.permissions = Permissions::Only(permissions.iter().map(|p| (*p).to_owned()).collect());
        self
    }

    /// Whether the scope holds `permission`.
    #[must_use]
    pub fn holds(&self, permission: &str) -> bool {
        match &self.permissions {
            Permissions::All => true,
            Permissions::None => false,
            Permissions::Only(held) => held.iter().any(|p| p == permission),
        }
    }

    /// Rows whose value at `path` is one of `any_of`, on top of this scope.
    #[must_use]
    pub fn and(mut self, path: &[&str], any_of: Vec<Value>) -> Self {
        self.restrictions.push(Restriction {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            any_of,
        });
        self
    }

    /// The scope as a [`Cond`], for a source to apply ahead of the plan's filter.
    #[must_use]
    pub fn cond(&self) -> Cond {
        self.restrictions
            .iter()
            .map(|r| {
                r.any_of
                    .iter()
                    .map(|v| Cond::Compare {
                        path: r.path.clone(),
                        op: odata_url::CompareOp::Eq,
                        value: v.clone(),
                    })
                    .reduce(|a, b| Cond::Or(Box::new(a), Box::new(b)))
                    .unwrap_or(Cond::Const(false))
            })
            .reduce(|a, b| Cond::And(Box::new(a), Box::new(b)))
            .unwrap_or(Cond::Const(true))
    }
}

/// What a source fetched for a plan: at most [`Plan::fetch`] rows from [`Plan::offset`], in
/// the plan's order, and the count of every matching row when the plan asks for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fetched {
    /// The rows, whole: the shared code projects them.
    pub rows: Vec<Value>,
    /// The count of rows meeting the scope and the filter, when [`Plan::count`].
    pub count: Option<u64>,
}

/// One answered page of an entity set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answer {
    /// The rows, each an object of the selected properties (every declared property without
    /// `$select`; a property a row lacks is `null`).
    pub rows: Vec<Value>,
    /// `$count`: rows meeting the scope and the filter.
    pub count: Option<u64>,
    /// The `$skiptoken` of the next page, when there is one.
    pub next: Option<u64>,
    /// Each row's key value, in row order, whatever `$select` keeps.
    pub keys: Vec<Value>,
    /// The `$select`ed properties; `None` when every declared property is answered.
    pub select: Option<Vec<String>>,
}

/// Why a query got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    /// Valid OData this service does not support (501), naming the construct.
    NotImplemented(String),
    /// A property path the entity type does not have (400), naming it.
    UnknownProperty(String),
    /// A request this service refuses (400), saying why.
    BadRequest(String),
    /// The source failed (500). The text is for the operator's log, never the response.
    Source(String),
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented(what) => write!(f, "{what} is not supported by this service"),
            Self::UnknownProperty(path) => write!(f, "the entity type has no property {path}"),
            Self::BadRequest(why) => f.write_str(why),
            Self::Source(why) => write!(f, "the source failed: {why}"),
        }
    }
}

impl std::error::Error for TableError {}

/// One entity set over one source.
pub trait Table: Send + Sync {
    /// The set, its type and key.
    fn info(&self) -> &TableInfo;

    /// The rows of `plan` within `scope`: `scope` applied first, then the plan's filter,
    /// ordered by the plan, [`Plan::fetch`] rows from [`Plan::offset`], and the count when
    /// asked.
    fn fetch<'a>(&'a self, plan: &'a Plan, scope: &'a Scope) -> Fetching<'a>;

    /// The entity tag of the data the set reads (a Git commit), when the source has one.
    fn etag(&self) -> Option<&str> {
        None
    }
}

/// Answers `options` on `table` within `scope`, at most `max_page` rows a page.
///
/// # Errors
///
/// [`TableError`]: what [`plan()`] refuses, or the source's failure.
pub async fn query<T: Table + ?Sized>(
    table: &T,
    options: &QueryOptions,
    scope: &Scope,
    max_page: u64,
) -> Result<Answer, TableError> {
    let plan = plan(table.info(), options, max_page)?;
    let fetched = table.fetch(&plan, scope).await?;
    let more = u64::try_from(fetched.rows.len()).unwrap_or(u64::MAX) > plan.limit;
    let take = usize::try_from(plan.limit).unwrap_or(usize::MAX);
    let info = table.info();
    let names = plan.select.as_ref().unwrap_or(&info.properties);
    let page = fetched.rows.iter().take(take);
    Ok(Answer {
        rows: page.clone().map(|row| project(row, names)).collect(),
        keys: page
            .map(|row| row.get(&info.key).cloned().unwrap_or(Value::Null))
            .collect(),
        count: fetched.count,
        next: more.then(|| plan.token.saturating_add(plan.limit)),
        select: plan.select,
    })
}

/// The options reading the one entity whose key is `key` (a string or an integer), keeping
/// `select` when given.
///
/// # Errors
///
/// [`TableError::BadRequest`] when `key` is neither a string nor an integer.
pub fn key_options(
    info: &TableInfo,
    key: &Value,
    select: Option<Vec<odata_url::SelectItem>>,
) -> Result<QueryOptions, TableError> {
    let literal = match key {
        Value::String(s) => odata_url::Literal::String(s.clone()),
        Value::Number(n) => n
            .as_i64()
            .map(odata_url::Literal::Int)
            .ok_or_else(|| TableError::BadRequest("a key is a string or an integer".into()))?,
        _ => {
            return Err(TableError::BadRequest(
                "a key is a string or an integer".into(),
            ))
        }
    };
    Ok(QueryOptions {
        filter: Some(odata_url::Expr::Compare(
            Box::new(odata_url::Expr::Member(odata_url::MemberPath {
                variable: None,
                path: vec![info.key.clone()],
            })),
            odata_url::CompareOp::Eq,
            Box::new(odata_url::Expr::Literal(literal)),
        )),
        select,
        top: Some(1),
        ..QueryOptions::default()
    })
}

/// Every row `options` matches within `scope`, page after page of `page` rows, as plain JSON
/// objects; at most `max_rows`.
///
/// # Errors
///
/// What [`query`] answers, or [`TableError::BadRequest`] past `max_rows` rows.
// ponytail: collected in memory up to max_rows; a streamed body when a set outgrows it.
pub async fn export<T: Table + ?Sized>(
    table: &T,
    options: &QueryOptions,
    scope: &Scope,
    page: u64,
    max_rows: usize,
) -> Result<Vec<Value>, TableError> {
    let mut options = options.clone();
    let mut rows = Vec::new();
    loop {
        let answer = query(table, &options, scope, page.max(1)).await?;
        rows.extend(answer.rows);
        if rows.len() > max_rows {
            return Err(TableError::BadRequest(format!(
                "the export is over {max_rows} rows: narrow it with $filter"
            )));
        }
        match answer.next {
            Some(token) => options.skiptoken = Some(token.to_string()),
            None => return Ok(rows),
        }
    }
}

/// `row` reduced to `names`, a missing one as `null`: every source answers the same members.
fn project(row: &Value, names: &[String]) -> Value {
    Value::Object(
        names
            .iter()
            .map(|n| (n.clone(), row.get(n).cloned().unwrap_or(Value::Null)))
            .collect(),
    )
}
