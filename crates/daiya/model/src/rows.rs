#![forbid(unsafe_code)]
//! Index rows of the stored entity sets: what a list, a count and a page read (R10, ARCH/PRF-02).
//! A row holds the set's declared properties and what its read decision needs, never a model.
//!
//! The store applies scope, filter, order, keyset cursor and page (Postgres in SQL, the memory
//! store through [`answer_rows`], the reference both are tested against).

use crate::auth::{ObjectTarget, ReadKind, ReadScope, ScopeColumn};
use crate::ids::OrgId;
use odata_table::{Cond, SortKey, compare, matches};
use serde_json::Value;
use std::cmp::Ordering;

/// The entity sets a store answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSet {
    Dashboards,
    DashboardVersions,
    Folders,
    Datasources,
    LibraryPanels,
    PublicDashboards,
    ImportReports,
}

impl RowSet {
    pub const ALL: [Self; 7] = [
        Self::Dashboards,
        Self::DashboardVersions,
        Self::Folders,
        Self::Datasources,
        Self::LibraryPanels,
        Self::PublicDashboards,
        Self::ImportReports,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dashboards => "Dashboards",
            Self::DashboardVersions => "DashboardVersions",
            Self::Folders => "Folders",
            Self::Datasources => "Datasources",
            Self::LibraryPanels => "LibraryPanels",
            Self::PublicDashboards => "PublicDashboards",
            Self::ImportReports => "ImportReports",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.name() == name)
    }

    /// The property that is the set's key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::DashboardVersions | Self::ImportReports => "id",
            _ => "uid",
        }
    }

    /// The kind of object whose read decision shows a row of the set.
    #[must_use]
    pub const fn read_kind(self) -> ReadKind {
        match self {
            Self::Dashboards | Self::DashboardVersions => ReadKind::Dashboard,
            Self::Folders => ReadKind::Folder,
            Self::Datasources => ReadKind::Datasource,
            Self::LibraryPanels => ReadKind::LibraryPanel,
            Self::PublicDashboards => ReadKind::PublicDashboard,
            // These hang off no dashboard, folder or datasource: the Organization's read decides.
            Self::ImportReports => ReadKind::Organization,
        }
    }

    /// The row member a [`ScopeColumn`] restricts; `None` when the set's objects carry none.
    #[must_use]
    pub const fn scope_member(self, column: ScopeColumn) -> Option<&'static str> {
        match (self, column) {
            (Self::ImportReports, _)
            | (Self::Datasources | Self::PublicDashboards, ScopeColumn::Folder) => None,
            (Self::DashboardVersions | Self::PublicDashboards, ScopeColumn::Object) => {
                Some("dashboardUid")
            }
            (_, ScopeColumn::Object) | (Self::Folders, ScopeColumn::Folder) => Some("uid"),
            (_, ScopeColumn::Folder) => Some("folderUid"),
        }
    }

    /// The object whose read decision shows `row`.
    #[must_use]
    pub fn target(self, org_id: OrgId, row: &Value) -> Option<ObjectTarget> {
        let member = |column| {
            self.scope_member(column)
                .and_then(|name| row.get(name))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        // The object's uid; a row without one shows nothing. An import report has no object
        // member: the Organization's read decides it.
        let uid = || member(ScopeColumn::Object);
        let folder_uid = || member(ScopeColumn::Folder);
        Some(match self {
            Self::Dashboards | Self::DashboardVersions => ObjectTarget::Dashboard {
                org_id,
                dashboard_uid: uid()?,
                folder_uid: folder_uid(),
            },
            Self::Folders => ObjectTarget::Folder {
                org_id,
                folder_uid: uid()?,
            },
            Self::Datasources => ObjectTarget::Datasource {
                org_id,
                datasource_uid: uid()?,
            },
            Self::LibraryPanels => ObjectTarget::LibraryPanel {
                org_id,
                panel_uid: uid()?,
                folder_uid: folder_uid(),
            },
            Self::PublicDashboards => ObjectTarget::PublicDashboard {
                org_id,
                dashboard_uid: uid()?,
            },
            Self::ImportReports => ObjectTarget::Organization { org_id },
        })
    }
}

/// One read of a set's index rows.
#[derive(Debug, Clone, PartialEq)]
pub struct RowQuery {
    pub set: RowSet,
    pub scope: ReadScope,
    pub window: RowWindow,
}

/// Filter, order and page of a read, whatever the rows are.
#[derive(Debug, Clone, PartialEq)]
pub struct RowWindow {
    /// Applied after the scope, so it only narrows it. A time is compared with RFC 3339 text in
    /// the rows' spelling, and no value holds a NUL.
    pub filter: Cond,
    /// A total order: it names the key.
    pub order: Vec<SortKey>,
    /// The order values of the last row the caller has: the page starts behind it.
    pub after: Option<Vec<Value>>,
    pub skip: u64,
    /// `None` reads every row of the scope; `Some(0)` reads none.
    pub limit: Option<u64>,
    /// Count what meets scope and filter, in the snapshot the rows come from.
    pub count: bool,
}

impl RowQuery {
    /// Every row of `set` within `scope`, by key.
    #[must_use]
    pub fn every(set: RowSet, scope: ReadScope) -> Self {
        Self {
            set,
            scope,
            window: RowWindow {
                filter: Cond::Const(true),
                order: vec![SortKey {
                    path: vec![set.key().to_owned()],
                    descending: false,
                }],
                after: None,
                skip: 0,
                limit: None,
                count: false,
            },
        }
    }
}

/// What a store answers to a [`RowQuery`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowPage {
    pub rows: Vec<Value>,
    pub count: Option<u64>,
}

/// `query` over rows held in memory.
#[must_use]
pub fn answer_rows(rows: impl IntoIterator<Item = Value>, query: &RowQuery) -> RowPage {
    let in_scope = |row: &Value| match &query.scope {
        ReadScope::All => true,
        ReadScope::Nothing => false,
        ReadScope::In { column, values } => query
            .set
            .scope_member(*column)
            .and_then(|name| row.get(name))
            .and_then(Value::as_str)
            .is_some_and(|value| values.contains(value)),
    };
    let rows = rows.into_iter().filter(|row| in_scope(row)).collect();
    page_rows(rows, &query.window)
}

/// [`answer_rows`] for rows the caller already scoped, of a set no store holds.
#[must_use]
pub fn page_rows(mut rows: Vec<Value>, window: &RowWindow) -> RowPage {
    rows.retain(|row| matches(row, &window.filter));
    let count = window.count.then_some(rows.len() as u64);
    let against = |a: &Value, b: &dyn Fn(usize, &str) -> Value| {
        window
            .order
            .iter()
            .enumerate()
            .map(|(at, key)| {
                let name = key.path.first().map_or("", String::as_str);
                let order = compare(a.get(name), Some(&b(at, name)));
                if key.descending {
                    order.reverse()
                } else {
                    order
                }
            })
            .find(|order| order.is_ne())
            .unwrap_or(Ordering::Equal)
    };
    if let Some(after) = &window.after {
        rows.retain(|row| {
            against(row, &|at, _| after.get(at).cloned().unwrap_or(Value::Null)).is_gt()
        });
    }
    rows.sort_by(|a, b| against(a, &|_, name| b.get(name).cloned().unwrap_or(Value::Null)));
    let skip = usize::try_from(window.skip).unwrap_or(usize::MAX);
    let take = window
        .limit
        .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
    RowPage {
        rows: rows.into_iter().skip(skip).take(take).collect(),
        count,
    }
}
