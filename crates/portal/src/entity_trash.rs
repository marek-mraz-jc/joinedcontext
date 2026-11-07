//! The trash of a space's entities (API/01 §31, ADR-N-042 §3.3, T-3107): each person's own copy
//! of what they deleted from a data view, for 30 days. Durable with a database, in memory without.

use std::collections::HashMap;
use std::sync::RwLock;

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

/// How long a copy is kept.
pub const KEPT_DAYS: i64 = 30;
/// The most copies one person keeps per space; the oldest goes first.
pub const KEPT_PER_SPACE: usize = 1000;
/// The largest entity a copy holds, in bytes of JSON.
pub const MAX_ENTITY_BYTES: usize = 256 * 1024;

/// One deleted entity, as its keeper read it.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    pub id: i64,
    pub urn: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    #[schema(value_type = Object)]
    pub entity: Value,
    /// RFC 3339.
    pub deleted_at: String,
    /// RFC 3339: when the copy is gone.
    pub expires_at: String,
}

type Key = (String, String, String);
/// One kept copy in memory: id, when, urn, type, entity.
type Kept = (i64, time::OffsetDateTime, String, String, Value);
/// The next id and every keeper's copies.
type Memory = RwLock<(i64, HashMap<Key, Vec<Kept>>)>;

pub struct TrashStore {
    inner: Inner,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(Memory),
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn item(
    id: i64,
    at: time::OffsetDateTime,
    urn: String,
    entity_type: String,
    entity: Value,
) -> TrashItem {
    TrashItem {
        id,
        urn,
        entity_type,
        entity,
        deleted_at: rfc3339(at),
        expires_at: rfc3339(at + time::Duration::days(KEPT_DAYS)),
    }
}

impl TrashStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new((0, HashMap::new()))),
            },
        }
    }

    /// Keeps one copy and drops the keeper's oldest beyond [`KEPT_PER_SPACE`] and everything past
    /// [`KEPT_DAYS`].
    pub async fn keep(
        &self,
        (project, space, owner): (&str, &str, &str),
        urn: &str,
        entity_type: &str,
        entity: &Value,
    ) -> Result<TrashItem, sqlx::Error> {
        let now = time::OffsetDateTime::now_utc();
        match &self.inner {
            Inner::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let (id, at): (i64, time::OffsetDateTime) = sqlx::query_as(
                    "INSERT INTO entity_trash (project, space, owner, urn, type, entity) \
                     VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, deleted_at",
                )
                .bind(project)
                .bind(space)
                .bind(owner)
                .bind(urn)
                .bind(entity_type)
                .bind(entity)
                .fetch_one(&mut *tx)
                .await?;
                sqlx::query(
                    "DELETE FROM entity_trash WHERE project = $1 AND space = $2 AND owner = $3 AND \
                     (deleted_at < now() - make_interval(days => $4) OR id NOT IN \
                      (SELECT id FROM entity_trash WHERE project = $1 AND space = $2 AND owner = $3 \
                       ORDER BY id DESC LIMIT $5))",
                )
                .bind(project)
                .bind(space)
                .bind(owner)
                .bind(KEPT_DAYS as i32)
                .bind(KEPT_PER_SPACE as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(item(
                    id,
                    at,
                    urn.to_owned(),
                    entity_type.to_owned(),
                    entity.clone(),
                ))
            }
            Inner::Memory(lock) => {
                let mut guard = lock.write().unwrap_or_else(|e| e.into_inner());
                guard.0 += 1;
                let id = guard.0;
                let list = guard
                    .1
                    .entry((project.to_owned(), space.to_owned(), owner.to_owned()))
                    .or_default();
                list.push((
                    id,
                    now,
                    urn.to_owned(),
                    entity_type.to_owned(),
                    entity.clone(),
                ));
                list.retain(|(_, at, ..)| *at > now - time::Duration::days(KEPT_DAYS));
                let over = list.len().saturating_sub(KEPT_PER_SPACE);
                list.drain(..over);
                Ok(item(
                    id,
                    now,
                    urn.to_owned(),
                    entity_type.to_owned(),
                    entity.clone(),
                ))
            }
        }
    }

    /// The keeper's copies of one space, newest first, none older than [`KEPT_DAYS`].
    pub async fn list(
        &self,
        (project, space, owner): (&str, &str, &str),
    ) -> Result<Vec<TrashItem>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<(i64, time::OffsetDateTime, String, String, Value)> = sqlx::query_as(
                    "SELECT id, deleted_at, urn, type, entity FROM entity_trash \
                     WHERE project = $1 AND space = $2 AND owner = $3 \
                     AND deleted_at >= now() - make_interval(days => $4) ORDER BY id DESC LIMIT $5",
                )
                .bind(project)
                .bind(space)
                .bind(owner)
                .bind(KEPT_DAYS as i32)
                .bind(KEPT_PER_SPACE as i64)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, at, urn, entity_type, entity)| {
                        item(id, at, urn, entity_type, entity)
                    })
                    .collect())
            }
            Inner::Memory(lock) => {
                let guard = lock.read().unwrap_or_else(|e| e.into_inner());
                let since = time::OffsetDateTime::now_utc() - time::Duration::days(KEPT_DAYS);
                Ok(guard
                    .1
                    .get(&(project.to_owned(), space.to_owned(), owner.to_owned()))
                    .map(|list| {
                        list.iter()
                            .rev()
                            .filter(|(_, at, ..)| *at >= since)
                            .map(|(id, at, urn, entity_type, entity)| {
                                item(*id, *at, urn.clone(), entity_type.clone(), entity.clone())
                            })
                            .collect()
                    })
                    .unwrap_or_default())
            }
        }
    }

    /// Forgets one copy of its keeper; `false` when they keep none by that id.
    pub async fn forget(
        &self,
        (project, space, owner): (&str, &str, &str),
        id: i64,
    ) -> Result<bool, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query(
                "DELETE FROM entity_trash WHERE project = $1 AND space = $2 AND owner = $3 AND id = $4",
            )
            .bind(project)
            .bind(space)
            .bind(owner)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut guard = lock.write().unwrap_or_else(|e| e.into_inner());
                let Some(list) = guard
                    .1
                    .get_mut(&(project.to_owned(), space.to_owned(), owner.to_owned()))
                else {
                    return Ok(false);
                };
                let before = list.len();
                list.retain(|(kept, ..)| *kept != id);
                Ok(list.len() < before)
            }
        }
    }
}
