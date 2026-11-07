//! What people told the platform's owners from a page of the Portal (T-3272, API/01 §38).
//!
//! Two storage arms like every store here: `Postgres` over migration `0030_feedback.sql`, and
//! `Memory` when the Portal runs without a database. Nothing here names who sent a feedback.

use std::sync::RwLock;

use serde::Serialize;
use utoipa::ToSchema;

/// The most items one page of the list carries.
pub const PAGE: i64 = 100;

/// One feedback as the administrators' list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    pub id: i64,
    /// RFC 3339.
    pub created_at: String,
    pub page: String,
    pub version: String,
    pub text: String,
    /// Whether a screenshot was sent with it.
    pub screenshot: bool,
}

/// What a feedback is made of, already checked and scrubbed.
pub struct NewFeedback<'a> {
    pub page: &'a str,
    pub version: &'a str,
    pub text: &'a str,
    pub screenshot: Option<&'a [u8]>,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<Vec<(Feedback, Option<Vec<u8>>)>>),
}

pub struct FeedbackStore {
    inner: Inner,
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl FeedbackStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::default()),
            },
        }
    }

    /// Keeps one feedback; its id.
    pub async fn add(&self, new: NewFeedback<'_>) -> Result<i64, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let (id,): (i64,) = sqlx::query_as(
                    "INSERT INTO feedback (page, version, body, screenshot) VALUES ($1, $2, $3, $4) RETURNING id",
                )
                .bind(new.page)
                .bind(new.version)
                .bind(new.text)
                .bind(new.screenshot)
                .fetch_one(pool)
                .await?;
                Ok(id)
            }
            Inner::Memory(rows) => {
                let mut rows = rows.write().unwrap_or_else(|p| p.into_inner());
                let id = rows.last().map_or(1, |(last, _)| last.id + 1);
                rows.push((
                    Feedback {
                        id,
                        created_at: rfc3339(time::OffsetDateTime::now_utc()),
                        page: new.page.to_owned(),
                        version: new.version.to_owned(),
                        text: new.text.to_owned(),
                        screenshot: new.screenshot.is_some(),
                    },
                    new.screenshot.map(<[u8]>::to_vec),
                ));
                Ok(id)
            }
        }
    }

    /// The feedback after `after`, oldest first, at most [`PAGE`].
    pub async fn after(&self, after: i64) -> Result<Vec<Feedback>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<(i64, time::OffsetDateTime, String, String, String, bool)> = sqlx::query_as(
                    "SELECT id, created_at, page, version, body, screenshot IS NOT NULL FROM feedback \
                     WHERE id > $1 ORDER BY id LIMIT $2",
                )
                .bind(after)
                .bind(PAGE)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, at, page, version, text, screenshot)| Feedback {
                        id,
                        created_at: rfc3339(at),
                        page,
                        version,
                        text,
                        screenshot,
                    })
                    .collect())
            }
            Inner::Memory(rows) => Ok(rows
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .filter(|(feedback, _)| feedback.id > after)
                .take(PAGE as usize)
                .map(|(feedback, _)| feedback.clone())
                .collect()),
        }
    }

    /// The screenshot of one feedback, `None` when it has none or does not exist.
    pub async fn screenshot(&self, id: i64) -> Result<Option<Vec<u8>>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let row: Option<(Option<Vec<u8>>,)> =
                    sqlx::query_as("SELECT screenshot FROM feedback WHERE id = $1")
                        .bind(id)
                        .fetch_optional(pool)
                        .await?;
                Ok(row.and_then(|(screenshot,)| screenshot))
            }
            Inner::Memory(rows) => Ok(rows
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .find(|(feedback, _)| feedback.id == id)
                .and_then(|(_, screenshot)| screenshot.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one<'a>(text: &'a str, screenshot: Option<&'a [u8]>) -> NewFeedback<'a> {
        NewFeedback {
            page: "/spaces",
            version: "1.0.0",
            text,
            screenshot,
        }
    }

    /// The administrators' list pages by id: oldest first, after the last one seen, at most
    /// [`PAGE`] per page.
    #[tokio::test]
    async fn the_list_pages_oldest_first_after_the_last_id_seen() {
        let store = FeedbackStore::new(None);
        for n in 0..PAGE + 2 {
            store
                .add(one(&format!("note {n}"), None))
                .await
                .expect("kept");
        }
        let first = store.after(0).await.expect("listed");
        assert_eq!(first.len(), PAGE as usize);
        assert_eq!(first.first().map(|f| f.id), Some(1));
        let last = first.last().map_or(0, |f| f.id);
        let rest = store.after(last).await.expect("listed");
        assert_eq!(
            rest.iter().map(|f| f.id).collect::<Vec<_>>(),
            vec![PAGE + 1, PAGE + 2]
        );
        assert_eq!(store.after(PAGE + 2).await.expect("listed"), Vec::new());
    }

    /// The list says a screenshot came along; the image itself is read by id alone.
    #[tokio::test]
    async fn a_screenshot_is_flagged_in_the_list_and_read_by_id() {
        let store = FeedbackStore::new(None);
        let plain = store.add(one("no image", None)).await.expect("kept");
        let shot = store
            .add(one("with image", Some(b"png")))
            .await
            .expect("kept");
        let listed = store.after(0).await.expect("listed");
        assert_eq!(
            listed.iter().map(|f| f.screenshot).collect::<Vec<_>>(),
            vec![false, true]
        );
        assert_eq!(
            store.screenshot(shot).await.expect("read"),
            Some(b"png".to_vec())
        );
        assert_eq!(store.screenshot(plain).await.expect("read"), None);
        assert_eq!(store.screenshot(99).await.expect("read"), None);
    }
}
