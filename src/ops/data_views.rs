//! Saved data views of a space (API/01 §30, ADR-N-042 §3.2, T-3104): the Portal's own record of
//! how one entity type is looked at — kind, filter, order, grouping, hidden attributes, colours.
//! The rows a view shows are the space's entities, read with the person's own session, so a view
//! narrows and never widens what its reader may see.
//!
//! Kept in the Portal's database (`data_views`), or in memory when none is configured, like the
//! other stores here.

use std::collections::BTreeMap;
use std::sync::Arc;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::activity::{chrono_to_odt, odt_to_chrono};

/// The view kinds; `map` places the rows by a GeoProperty (T-4884).
pub const KINDS: [&str; 7] = [
    "grid", "gallery", "kanban", "calendar", "timeline", "map", "form",
];
pub const MODES: [&str; 3] = ["personal", "collaborative", "locked"];
/// The colour tokens a rule may name: the Portal's tones, never a raw colour.
pub const COLOURS: [&str; 5] = ["neutral", "info", "success", "warning", "danger"];
pub const MAX_TITLE: usize = 120;
pub const MAX_Q: usize = 4096;
pub const MAX_SORT: usize = 3;
/// The fields a grid groups its rows by, outermost first (`settings.groupBy`, T-5188).
pub const MAX_GROUP_BY: usize = 3;
pub const MAX_COLOUR_RULES: usize = 20;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
/// Views one person keeps in one space: a list a person reads, not a dump.
pub const MAX_PER_PERSON: i64 = 200;
const MAX_ATTR: usize = 256;
/// Attributes a view orders or labels at most: a type's columns, not a dump.
pub const MAX_COLUMNS: usize = 256;
/// A view's own column label, in characters (T-4827).
pub const MAX_LABEL: usize = 80;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SortKey {
    pub attr: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ColourRule {
    /// An NGSI-LD `q` evaluated in the page on the rows it shows.
    pub when: String,
    /// One of `neutral`, `info`, `success`, `warning`, `danger`.
    pub colour: String,
}

/// How a view looks at its type (API/01 §30).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewConfig {
    /// An NGSI-LD query, sent as the person's own `q` when the view opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortKey>,
    /// An enum attribute the rows are grouped by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// Column widths in pixels, by attribute.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub width: BTreeMap<String, u32>,
    /// The columns in the order the view shows them, by attribute; the rest follow (T-4827).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// A label of the view's own, by attribute: shown as text, the model keeps its own (T-4827).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colour: Vec<ColourRule>,
    /// The view kind's own settings: card fields, the kanban's attribute, a calendar's dates.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    #[schema(value_type = Object)]
    pub settings: serde_json::Map<String, serde_json::Value>,
}

/// A saved view as the API answers it.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataView {
    pub id: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub kind: String,
    pub mode: String,
    pub title: String,
    /// The owner's username, for display.
    pub owner: String,
    pub config: ViewConfig,
    pub version: i64,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
    /// The owner's Keycloak `sub`, the key ownership is checked by; never answered.
    #[serde(skip)]
    pub owner_subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateView {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub kind: String,
    pub mode: String,
    pub title: String,
    #[serde(default)]
    pub config: ViewConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UpdateView {
    pub kind: String,
    pub mode: String,
    pub title: String,
    #[serde(default)]
    pub config: ViewConfig,
    /// The version read; a save against a newer view is refused instead of overwriting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DataViewError {
    #[error("{0}")]
    Invalid(String),
    #[error("no such view")]
    NotFound,
    #[error("the view was changed meanwhile: it is at version {0}, reload it before saving")]
    Conflict(i64),
    #[error("a person keeps at most {MAX_PER_PERSON} views per space; delete one first")]
    Limit,
    #[error("database error: {0}")]
    Db(String),
}

/// An NGSI-LD type name as the gateway accepts it.
pub(crate) fn is_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_ATTR
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// An attribute name, short or expanded (an IRI): no whitespace, quotes or control characters.
fn is_attr(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ATTR
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:/#@".contains(c))
}

fn check_q(what: &str, q: &str) -> Result<(), DataViewError> {
    if q.len() > MAX_Q || q.chars().any(char::is_control) {
        return Err(DataViewError::Invalid(format!(
            "{what} is a query of at most {MAX_Q} characters without control characters"
        )));
    }
    Ok(())
}

impl ViewConfig {
    pub fn validate(&self) -> Result<(), DataViewError> {
        let invalid = |m: String| Err(DataViewError::Invalid(m));
        if let Some(q) = &self.q {
            check_q("config.q", q)?;
        }
        if self.sort.len() > MAX_SORT {
            return invalid(format!("config.sort names at most {MAX_SORT} attributes"));
        }
        let names = self
            .sort
            .iter()
            .map(|key| key.attr.as_str())
            .chain(self.group.as_deref())
            .chain(self.hidden.iter().map(String::as_str))
            .chain(self.width.keys().map(String::as_str))
            .chain(self.order.iter().map(String::as_str))
            .chain(self.labels.keys().map(String::as_str));
        for name in names {
            if !is_attr(name) {
                return invalid(format!("'{name}' is not an attribute name"));
            }
        }
        if self.order.len() > MAX_COLUMNS || self.labels.len() > MAX_COLUMNS {
            return invalid(format!(
                "config.order and config.labels name at most {MAX_COLUMNS} attributes"
            ));
        }
        if let Some((attr, _)) = self.labels.iter().find(|(_, label)| {
            let length = label.trim().chars().count();
            length == 0 || label.chars().count() > MAX_LABEL || label.chars().any(char::is_control)
        }) {
            return invalid(format!(
                "config.labels['{attr}'] is 1 to {MAX_LABEL} characters without control characters"
            ));
        }
        if self.width.values().any(|w| !(40..=2000).contains(w)) {
            return invalid("config.width is between 40 and 2000 pixels".into());
        }
        if self.colour.len() > MAX_COLOUR_RULES {
            return invalid(format!(
                "config.colour holds at most {MAX_COLOUR_RULES} rules"
            ));
        }
        self.check_group_by()?;
        for rule in &self.colour {
            check_q("config.colour[].when", &rule.when)?;
            if !COLOURS.contains(&rule.colour.as_str()) {
                return invalid(format!(
                    "colour '{}' is not one of {}",
                    rule.colour,
                    COLOURS.join(", ")
                ));
            }
        }
        let size = serde_json::to_vec(self)
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        if size > MAX_CONFIG_BYTES {
            return invalid(format!(
                "config is {size} bytes, the limit is {MAX_CONFIG_BYTES}"
            ));
        }
        Ok(())
    }
}

impl ViewConfig {
    /// A grid's `settings.groupBy`: at most [`MAX_GROUP_BY`] distinct attribute names.
    fn check_group_by(&self) -> Result<(), DataViewError> {
        let Some(raw) = self.settings.get("groupBy") else {
            return Ok(());
        };
        let names: Vec<&str> = match raw.as_array() {
            Some(list) => list.iter().filter_map(serde_json::Value::as_str).collect(),
            None => Vec::new(),
        };
        let listed = raw.as_array().map_or(0, Vec::len);
        let distinct = names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if !raw.is_array() || names.len() != listed || listed > MAX_GROUP_BY || distinct != listed {
            return Err(DataViewError::Invalid(format!(
                "config.settings.groupBy is a list of at most {MAX_GROUP_BY} different attribute names"
            )));
        }
        match names.iter().find(|name| !is_attr(name)) {
            Some(name) => Err(DataViewError::Invalid(format!(
                "config.settings.groupBy: '{name}' is not an attribute name"
            ))),
            None => Ok(()),
        }
    }
}

/// The fields every save carries: kind, mode, title and config.
pub fn validate_fields(
    kind: &str,
    mode: &str,
    title: &str,
    config: &ViewConfig,
) -> Result<(), DataViewError> {
    if !KINDS.contains(&kind) {
        return Err(DataViewError::Invalid(format!(
            "kind '{kind}' is not one of {}",
            KINDS.join(", ")
        )));
    }
    if !MODES.contains(&mode) {
        return Err(DataViewError::Invalid(format!(
            "mode '{mode}' is not one of {}",
            MODES.join(", ")
        )));
    }
    let length = title.trim().chars().count();
    if length == 0 || title.chars().count() > MAX_TITLE || title.chars().any(char::is_control) {
        return Err(DataViewError::Invalid(format!(
            "a title is 1 to {MAX_TITLE} characters without control characters"
        )));
    }
    config.validate()
}

impl CreateView {
    pub fn validate(&self) -> Result<(), DataViewError> {
        if !is_type_name(&self.entity_type) {
            return Err(DataViewError::Invalid(format!(
                "'{}' is not an entity type name",
                self.entity_type
            )));
        }
        validate_fields(&self.kind, &self.mode, &self.title, &self.config)
    }
}

/// What the caller may do with one view, by its mode (API/01 §30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub see: bool,
    pub change: bool,
    /// Delete it, or change its mode.
    pub govern: bool,
}

/// `owner` is whether the caller owns the view; `steward` whether they may update the space.
pub fn access(mode: &str, owner: bool, steward: bool) -> Access {
    match mode {
        "personal" => Access {
            see: owner,
            change: owner,
            govern: owner,
        },
        "collaborative" => Access {
            see: true,
            change: true,
            govern: owner || steward,
        },
        // `locked`, and anything a later Portal wrote that this one does not know: read only.
        _ => Access {
            see: true,
            change: owner || steward,
            govern: owner || steward,
        },
    }
}

/// A random version-4 UUID: what a view is addressed by.
pub(crate) fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Whether `id` has the shape `new_id` makes, so a path segment never reaches SQL unchecked.
pub fn is_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(at, c)| match at {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        })
}

type Key = (String, String);

enum Inner {
    Db(sqlx::PgPool),
    Memory(RwLock<BTreeMap<Key, Vec<DataView>>>),
}

#[derive(Clone)]
pub struct DataViewStore {
    inner: Arc<Inner>,
}

/// The columns every read answers, as a literal: sqlx takes only `'static` query strings.
macro_rules! columns {
    () => {
        "id, entity_type, kind, mode, title, owner_subject, owner_name, config, version, created_at, updated_at"
    };
}

fn db(err: sqlx::Error) -> DataViewError {
    DataViewError::Db(err.to_string())
}

fn row_to_view(row: sqlx::postgres::PgRow) -> Result<DataView, DataViewError> {
    let config: serde_json::Value = row.get("config");
    Ok(DataView {
        id: row.get("id"),
        entity_type: row.get("entity_type"),
        kind: row.get("kind"),
        mode: row.get("mode"),
        title: row.get("title"),
        owner_subject: row.get("owner_subject"),
        owner: row.get("owner_name"),
        // A row a later Portal wrote with a setting this one does not know reads as no setting.
        config: serde_json::from_value(config).unwrap_or_default(),
        version: row.get("version"),
        created_at: odt_to_chrono(row.get("created_at")),
        updated_at: odt_to_chrono(row.get("updated_at")),
    })
}

/// Who saves a view: the durable key and the name shown.
pub struct Owner<'a> {
    pub subject: &'a str,
    pub name: &'a str,
}

impl DataViewStore {
    pub fn new(pool: Option<sqlx::PgPool>) -> Self {
        let inner = match pool {
            Some(pool) => Inner::Db(pool),
            None => Inner::Memory(RwLock::new(BTreeMap::new())),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Every view of one space, oldest first; who may see which is the caller's to filter.
    pub async fn list(&self, project: &str, space: &str) -> Result<Vec<DataView>, DataViewError> {
        match &*self.inner {
            Inner::Memory(map) => Ok(map
                .read()
                .await
                .get(&(project.to_owned(), space.to_owned()))
                .cloned()
                .unwrap_or_default()),
            Inner::Db(pool) => sqlx::query(concat!(
                "SELECT ",
                columns!(),
                " FROM data_views WHERE project = $1 AND space = $2 ORDER BY created_at, id"
            ))
            .bind(project)
            .bind(space)
            .fetch_all(pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(row_to_view)
            .collect(),
        }
    }

    pub async fn get(
        &self,
        project: &str,
        space: &str,
        id: &str,
    ) -> Result<DataView, DataViewError> {
        match &*self.inner {
            Inner::Memory(map) => map
                .read()
                .await
                .get(&(project.to_owned(), space.to_owned()))
                .and_then(|views| views.iter().find(|v| v.id == id).cloned())
                .ok_or(DataViewError::NotFound),
            Inner::Db(pool) => sqlx::query(concat!(
                "SELECT ",
                columns!(),
                " FROM data_views WHERE project = $1 AND space = $2 AND id = $3"
            ))
            .bind(project)
            .bind(space)
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db)?
            .map(row_to_view)
            .transpose()?
            .ok_or(DataViewError::NotFound),
        }
    }

    /// Saves a new view owned by `owner`; refused past [`MAX_PER_PERSON`]. In the database the
    /// count and the insert run under a transaction lock on (project, space, owner), so two
    /// windows saving at once cannot both pass the count.
    pub async fn create(
        &self,
        project: &str,
        space: &str,
        owner: Owner<'_>,
        request: CreateView,
    ) -> Result<DataView, DataViewError> {
        request.validate()?;
        let now = Utc::now();
        let view = DataView {
            id: new_id(),
            entity_type: request.entity_type,
            kind: request.kind,
            mode: request.mode,
            title: request.title.trim().to_owned(),
            owner: owner.name.to_owned(),
            owner_subject: owner.subject.to_owned(),
            config: request.config,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                let views = map
                    .entry((project.to_owned(), space.to_owned()))
                    .or_default();
                let mine = views
                    .iter()
                    .filter(|v| v.owner_subject == view.owner_subject)
                    .count();
                if mine as i64 >= MAX_PER_PERSON {
                    return Err(DataViewError::Limit);
                }
                views.push(view.clone());
                Ok(view)
            }
            Inner::Db(pool) => {
                let config = serde_json::to_value(&view.config)
                    .map_err(|e| DataViewError::Db(e.to_string()))?;
                let mut tx = pool.begin().await.map_err(db)?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || '/' || $2 || '/' || $3, 0))")
                    .bind(project)
                    .bind(space)
                    .bind(&view.owner_subject)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                let saved = sqlx::query(concat!(
                    "INSERT INTO data_views (id, project, space, entity_type, kind, mode, title, owner_subject, owner_name, config, version, created_at, updated_at) \
                     SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1, $11, $11 \
                     WHERE (SELECT count(*) FROM data_views WHERE project = $2 AND space = $3 AND owner_subject = $8) < $12 \
                     RETURNING ",
                    columns!()
                ))
                .bind(&view.id)
                .bind(project)
                .bind(space)
                .bind(&view.entity_type)
                .bind(&view.kind)
                .bind(&view.mode)
                .bind(&view.title)
                .bind(&view.owner_subject)
                .bind(&view.owner)
                .bind(config)
                .bind(chrono_to_odt(now))
                .bind(MAX_PER_PERSON)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
                .map(row_to_view)
                .transpose()?
                .ok_or(DataViewError::Limit)?;
                tx.commit().await.map_err(db)?;
                Ok(saved)
            }
        }
    }

    /// Replaces a view's kind, mode, title and config and counts its version up; with
    /// `expected_version`, only when the view is still at it.
    pub async fn update(
        &self,
        project: &str,
        space: &str,
        id: &str,
        request: UpdateView,
    ) -> Result<DataView, DataViewError> {
        validate_fields(
            &request.kind,
            &request.mode,
            &request.title,
            &request.config,
        )?;
        let title = request.title.trim().to_owned();
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                let view = map
                    .get_mut(&(project.to_owned(), space.to_owned()))
                    .and_then(|views| views.iter_mut().find(|v| v.id == id))
                    .ok_or(DataViewError::NotFound)?;
                if request
                    .expected_version
                    .is_some_and(|expected| expected != view.version)
                {
                    return Err(DataViewError::Conflict(view.version));
                }
                view.kind = request.kind;
                view.mode = request.mode;
                view.title = title;
                view.config = request.config;
                view.version += 1;
                view.updated_at = Utc::now();
                Ok(view.clone())
            }
            Inner::Db(pool) => {
                let config = serde_json::to_value(&request.config)
                    .map_err(|e| DataViewError::Db(e.to_string()))?;
                let updated = sqlx::query(concat!(
                    "UPDATE data_views SET kind = $4, mode = $5, title = $6, config = $7, version = version + 1, updated_at = $8 \
                     WHERE project = $1 AND space = $2 AND id = $3 AND ($9::bigint IS NULL OR version = $9) \
                     RETURNING ",
                    columns!()
                ))
                .bind(project)
                .bind(space)
                .bind(id)
                .bind(&request.kind)
                .bind(&request.mode)
                .bind(&title)
                .bind(config)
                .bind(chrono_to_odt(Utc::now()))
                .bind(request.expected_version)
                .fetch_optional(pool)
                .await
                .map_err(db)?;
                match updated {
                    Some(row) => row_to_view(row),
                    // Not written: gone, or moved on since the caller read it.
                    None => Err(DataViewError::Conflict(
                        self.get(project, space, id).await?.version,
                    )),
                }
            }
        }
    }

    pub async fn delete(&self, project: &str, space: &str, id: &str) -> Result<(), DataViewError> {
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                let views = map
                    .get_mut(&(project.to_owned(), space.to_owned()))
                    .ok_or(DataViewError::NotFound)?;
                let before = views.len();
                views.retain(|v| v.id != id);
                if views.len() == before {
                    return Err(DataViewError::NotFound);
                }
                Ok(())
            }
            Inner::Db(pool) => {
                let done = sqlx::query(
                    "DELETE FROM data_views WHERE project = $1 AND space = $2 AND id = $3",
                )
                .bind(project)
                .bind(space)
                .bind(id)
                .execute(pool)
                .await
                .map_err(db)?;
                if done.rows_affected() == 0 {
                    return Err(DataViewError::NotFound);
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create(json: serde_json::Value) -> CreateView {
        serde_json::from_value(json).expect("a create body")
    }

    fn view() -> CreateView {
        create(
            json!({ "type": "BikeHireDockingStation", "kind": "grid", "mode": "personal", "title": "Short of bikes" }),
        )
    }

    const JANA: Owner<'static> = Owner {
        subject: "sub-jana",
        name: "jana",
    };

    #[test]
    fn the_documented_example_is_valid_and_unknown_keys_are_not() {
        let example = create(json!({
            "type": "BikeHireDockingStation", "kind": "grid", "mode": "collaborative", "title": "Stations short of bikes",
            "config": {
                "q": "availableBikeNumber<3",
                "sort": [{ "attr": "availableBikeNumber", "desc": false }],
                "group": "status",
                "hidden": ["dateLastReported"],
                "width": { "name": 240 },
                "order": ["name", "ngsi-ld:status"],
                "labels": { "ngsi-ld:status": "Tila" },
                "colour": [{ "when": "availableBikeNumber==0", "colour": "danger" }],
                "settings": {}
            }
        }));
        assert_eq!(example.validate(), Ok(()));
        assert!(serde_json::from_value::<CreateView>(
            json!({ "type": "T", "kind": "grid", "mode": "personal", "title": "x", "owner": "eve" })
        )
        .is_err());
        assert!(serde_json::from_value::<ViewConfig>(json!({ "filter": "x" })).is_err());
    }

    #[test]
    fn refuses_each_bad_field_by_name() {
        let bad = |patch: serde_json::Value| {
            let mut body =
                json!({ "type": "Station", "kind": "grid", "mode": "personal", "title": "t" });
            body.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            create(body).validate().unwrap_err().to_string()
        };
        assert!(bad(json!({ "type": "1Station" })).contains("entity type"));
        assert!(bad(json!({ "kind": "pivot" })).contains("kind 'pivot'"));
        let mut map = view();
        map.kind = "map".into();
        assert_eq!(map.validate(), Ok(()));
        assert!(bad(json!({ "mode": "public" })).contains("mode 'public'"));
        assert!(bad(json!({ "title": "   " })).contains("title"));
        assert!(bad(json!({ "title": "x".repeat(MAX_TITLE + 1) })).contains("title"));
        assert!(bad(json!({ "config": { "q": "a==1\u{0007}" } })).contains("config.q"));
        assert!(bad(json!({ "config": { "sort": [{ "attr": "a" }, { "attr": "b" }, { "attr": "c" }, { "attr": "d" }] } })).contains("sort"));
        assert!(bad(json!({ "config": { "hidden": ["a b"] } })).contains("'a b'"));
        assert!(bad(json!({ "config": { "width": { "a": 5 } } })).contains("width"));
        assert!(bad(json!({ "config": { "order": ["a", "b c"] } })).contains("'b c'"));
        assert!(
            bad(json!({ "config": { "labels": { "a": "  " } } })).contains("config.labels['a']")
        );
        assert!(
            bad(json!({ "config": { "labels": { "a": "x".repeat(MAX_LABEL + 1) } } }))
                .contains("config.labels")
        );
        assert!(bad(json!({ "config": { "labels": { "a": "line\nbreak" } } })).contains("control"));
        let many: Vec<String> = (0..=MAX_COLUMNS).map(|i| format!("a{i}")).collect();
        assert!(bad(json!({ "config": { "order": many } })).contains("at most"));
        assert!(
            bad(json!({ "config": { "colour": [{ "when": "a==1", "colour": "#ff0000" }] } }))
                .contains("#ff0000")
        );
        for refused in [
            json!("status"),
            json!(["a", "b", "c", "d"]),
            json!(["a", "a"]),
            json!(["a", 1]),
        ] {
            assert!(
                bad(json!({ "config": { "settings": { "groupBy": refused } } }))
                    .contains("config.settings.groupBy")
            );
        }
        assert!(bad(json!({ "config": { "settings": { "groupBy": ["a b"] } } })).contains("'a b'"));
        let mut grouped = view();
        grouped
            .config
            .settings
            .insert("groupBy".into(), json!(["status", "city"]));
        assert_eq!(grouped.validate(), Ok(()));
        let huge = json!({ "config": { "settings": { "blob": "x".repeat(MAX_CONFIG_BYTES) } } });
        assert!(bad(huge).contains("bytes"));
    }

    #[test]
    fn who_sees_changes_and_governs_by_mode() {
        let all = Access {
            see: true,
            change: true,
            govern: true,
        };
        assert_eq!(access("personal", true, false), all);
        assert_eq!(
            access("personal", false, true),
            Access {
                see: false,
                change: false,
                govern: false
            }
        );
        assert_eq!(
            access("collaborative", false, false),
            Access {
                see: true,
                change: true,
                govern: false
            }
        );
        assert_eq!(access("collaborative", false, true), all);
        assert_eq!(
            access("locked", false, false),
            Access {
                see: true,
                change: false,
                govern: false
            }
        );
        assert_eq!(access("locked", false, true), all);
        assert_eq!(access("locked", true, false), all);
    }

    #[test]
    fn ids_are_uuids_and_nothing_else_passes_for_one() {
        let id = new_id();
        assert!(is_id(&id), "{id}");
        assert_eq!(&id[14..15], "4");
        assert_ne!(new_id(), id);
        for bad in [
            "",
            "x",
            "../../etc",
            "7F1C2A9E-4B1D-4A57-9A0E-2F6D1C3B8E01",
            "7f1c2a9e4b1d4a579a0e2f6d1c3b8e0100000",
        ] {
            assert!(!is_id(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn saves_lists_updates_with_versions_and_deletes() {
        let store = DataViewStore::new(None);
        let saved = store
            .create("hel", "bikes", JANA, view())
            .await
            .expect("saved");
        assert_eq!((saved.version, saved.owner.as_str()), (1, "jana"));
        assert_eq!(
            store.list("hel", "bikes").await.unwrap(),
            vec![saved.clone()]
        );
        assert!(store.list("hel", "other").await.unwrap().is_empty());

        let update = |expected: Option<i64>, title: &str| UpdateView {
            kind: "grid".into(),
            mode: "collaborative".into(),
            title: title.into(),
            config: ViewConfig::default(),
            expected_version: expected,
        };
        let renamed = store
            .update("hel", "bikes", &saved.id, update(Some(1), " Renamed "))
            .await
            .unwrap();
        assert_eq!(
            (
                renamed.version,
                renamed.title.as_str(),
                renamed.mode.as_str()
            ),
            (2, "Renamed", "collaborative")
        );
        // A window that read version 1 does not overwrite version 2.
        assert_eq!(
            store
                .update("hel", "bikes", &saved.id, update(Some(1), "stale"))
                .await,
            Err(DataViewError::Conflict(2))
        );
        assert_eq!(
            store
                .update("hel", "bikes", &saved.id, update(None, "last wins"))
                .await
                .unwrap()
                .version,
            3
        );

        store.delete("hel", "bikes", &saved.id).await.unwrap();
        assert_eq!(
            store.delete("hel", "bikes", &saved.id).await,
            Err(DataViewError::NotFound)
        );
        assert_eq!(
            store.get("hel", "bikes", &saved.id).await,
            Err(DataViewError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_person_keeps_at_most_the_limit_per_space() {
        let store = DataViewStore::new(None);
        for _ in 0..MAX_PER_PERSON {
            store.create("hel", "bikes", JANA, view()).await.unwrap();
        }
        assert_eq!(
            store.create("hel", "bikes", JANA, view()).await,
            Err(DataViewError::Limit)
        );
        // Someone else, or the same person in another space, is not held back.
        store
            .create(
                "hel",
                "bikes",
                Owner {
                    subject: "sub-eva",
                    name: "eva",
                },
                view(),
            )
            .await
            .unwrap();
        store.create("hel", "trams", JANA, view()).await.unwrap();
    }
}
