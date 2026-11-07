//! Comments on a space's entities and the notifications their mentions send (API/01 §35,
//! ADR-N-042 §3.1, T-3106). Each comment names its entity by `(project, space, urn)` and changes
//! no data. Durable with a database, in memory without.

use std::sync::RwLock;

use serde::Serialize;
use utoipa::ToSchema;

/// The most comments one entity keeps.
pub const MAX_PER_ENTITY: i64 = 1000;
/// The longest comment, in characters.
pub const MAX_TEXT: usize = 4000;
/// How much of a comment a notification carries, in characters.
pub const EXCERPT: usize = 200;
/// The most notifications one answer lists.
pub const LISTED: i64 = 100;

/// One comment as stored.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: i64,
    pub urn: String,
    /// Who wrote it: their username, as the Portal knows them.
    pub author: String,
    /// What the Portal shows for the author.
    pub author_name: String,
    /// The author's Keycloak `sub`, which alone decides whose comment it is: a later account
    /// with the same username is someone else. `None` on a comment stored before it was kept.
    #[serde(skip)]
    pub author_subject: Option<String>,
    pub text: String,
    /// The people the comment notified, by identifier.
    pub mentions: Vec<String>,
    /// RFC 3339.
    pub created_at: String,
}

/// One notification: a mention, or an event of a person's connectors (T-5979).
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: i64,
    /// `mention`, `change.opened` (a change opened through a connector waits for approval),
    /// `connector.pending` (a connector request waits above the ceiling) or `connector.expiring`
    /// (a connector of the reader's ends within seven days).
    pub kind: String,
    pub project: String,
    /// The mention's space; empty for a connector event.
    pub space: String,
    /// The mentioned entity, or for a connector event the change or connector it is about.
    pub urn: String,
    /// The comment of a mention.
    pub comment_id: Option<i64>,
    pub author: String,
    pub author_name: String,
    /// The first [`EXCERPT`] characters of the comment; for a connector event the connector's name.
    pub excerpt: String,
    /// The Portal page a connector event opens, e.g. `/projects/helsinki/approvals/cr-12`.
    pub link: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    pub read: bool,
}

/// A connector event (T-5979). It carries names and a link only, never manifest content.
pub struct Notice<'a> {
    pub kind: &'a str,
    pub project: &'a str,
    /// The change or connector id the event is about.
    pub about: &'a str,
    /// Who caused it, by identifier and as the Portal shows them.
    pub author: &'a str,
    pub author_name: &'a str,
    /// The connector's name.
    pub connector: &'a str,
    pub link: &'a str,
    /// One event, one notification per recipient: a second notice under the same key is dropped.
    pub key: &'a str,
}

/// The recipient a notice for everyone who may approve a change in `project` is stored under.
pub fn approvers_of(project: &str) -> String {
    format!("@approve:{project}")
}

/// The recipient a notice for everyone who decides connector requests is stored under.
pub const CONNECTOR_ADMINS: &str = "@connector-admin";

/// How long an audience notice waits for a person to read their notifications.
const AUDIENCE_DAYS: i64 = 30;

/// Who writes a comment.
pub struct Author<'a> {
    pub id: &'a str,
    pub name: &'a str,
    /// Keycloak `sub`.
    pub subject: &'a str,
}

/// What a new comment says and whom it notifies.
pub struct NewComment<'a> {
    pub project: &'a str,
    pub space: &'a str,
    pub urn: &'a str,
    pub text: &'a str,
    pub mentions: &'a [String],
}

#[derive(Default)]
struct Memory {
    next: i64,
    /// project, space, comment.
    comments: Vec<(String, String, Comment)>,
    /// recipient, notification.
    notifications: Vec<(String, Notification)>,
    /// recipient, event key and notification id of each connector event held.
    keys: Vec<(String, String, i64)>,
}

pub struct CommentStore {
    inner: Inner,
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<Memory>),
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The first [`EXCERPT`] characters of a comment, whole characters only.
pub fn excerpt(text: &str) -> String {
    text.chars().take(EXCERPT).collect()
}

type CommentRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    String,
    Vec<String>,
    time::OffsetDateTime,
);
type NotificationRow = (
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    String,
    String,
    String,
    Option<String>,
    time::OffsetDateTime,
    Option<time::OffsetDateTime>,
);

fn comment_of(
    (id, urn, author, author_name, author_subject, text, mentions, at): CommentRow,
) -> Comment {
    Comment {
        id,
        urn,
        author,
        author_name,
        author_subject,
        text,
        mentions,
        created_at: rfc3339(at),
    }
}

fn notification_of(
    (id, kind, project, space, urn, comment_id, author, author_name, excerpt, link, at, read_at): NotificationRow,
) -> Notification {
    Notification {
        id,
        kind,
        project,
        space,
        urn,
        comment_id,
        author,
        author_name,
        excerpt,
        link,
        created_at: rfc3339(at),
        read: read_at.is_some(),
    }
}

impl CommentStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::new(Memory::default())),
            },
        }
    }

    /// Stores one comment and one notification per mentioned person, in one transaction;
    /// `None` when the entity already holds [`MAX_PER_ENTITY`] comments.
    pub async fn add(
        &self,
        author: Author<'_>,
        new: NewComment<'_>,
    ) -> Result<Option<Comment>, sqlx::Error> {
        let short = excerpt(new.text);
        match &self.inner {
            Inner::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let (held,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM entity_comments WHERE project = $1 AND space = $2 AND urn = $3",
                )
                .bind(new.project)
                .bind(new.space)
                .bind(new.urn)
                .fetch_one(&mut *tx)
                .await?;
                if held >= MAX_PER_ENTITY {
                    return Ok(None);
                }
                let row: CommentRow = sqlx::query_as(
                    "INSERT INTO entity_comments (project, space, urn, author, author_name, author_subject, body, mentions) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                     RETURNING id, urn, author, author_name, author_subject, body, mentions, created_at",
                )
                .bind(new.project)
                .bind(new.space)
                .bind(new.urn)
                .bind(author.id)
                .bind(author.name)
                .bind(author.subject)
                .bind(new.text)
                .bind(new.mentions)
                .fetch_one(&mut *tx)
                .await?;
                for recipient in new.mentions {
                    sqlx::query(
                        "INSERT INTO notifications (recipient, project, space, urn, comment_id, author, author_name, excerpt) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                    )
                    .bind(recipient)
                    .bind(new.project)
                    .bind(new.space)
                    .bind(new.urn)
                    .bind(row.0)
                    .bind(author.id)
                    .bind(author.name)
                    .bind(&short)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(Some(comment_of(row)))
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let held = memory
                    .comments
                    .iter()
                    .filter(|(p, s, c)| p == new.project && s == new.space && c.urn == new.urn)
                    .count() as i64;
                if held >= MAX_PER_ENTITY {
                    return Ok(None);
                }
                let at = rfc3339(time::OffsetDateTime::now_utc());
                memory.next += 1;
                let comment = Comment {
                    id: memory.next,
                    urn: new.urn.to_owned(),
                    author: author.id.to_owned(),
                    author_name: author.name.to_owned(),
                    author_subject: Some(author.subject.to_owned()),
                    text: new.text.to_owned(),
                    mentions: new.mentions.to_vec(),
                    created_at: at.clone(),
                };
                memory.comments.push((
                    new.project.to_owned(),
                    new.space.to_owned(),
                    comment.clone(),
                ));
                for recipient in new.mentions {
                    memory.next += 1;
                    let notification = Notification {
                        id: memory.next,
                        project: new.project.to_owned(),
                        space: new.space.to_owned(),
                        kind: "mention".into(),
                        urn: new.urn.to_owned(),
                        comment_id: Some(comment.id),
                        author: author.id.to_owned(),
                        author_name: author.name.to_owned(),
                        excerpt: short.clone(),
                        link: None,
                        created_at: at.clone(),
                        read: false,
                    };
                    memory.notifications.push((recipient.clone(), notification));
                }
                Ok(Some(comment))
            }
        }
    }

    /// The comments on one entity, oldest first.
    pub async fn list(
        &self,
        project: &str,
        space: &str,
        urn: &str,
    ) -> Result<Vec<Comment>, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<CommentRow> = sqlx::query_as(
                    "SELECT id, urn, author, author_name, author_subject, body, mentions, created_at FROM entity_comments \
                     WHERE project = $1 AND space = $2 AND urn = $3 ORDER BY id LIMIT $4",
                )
                .bind(project)
                .bind(space)
                .bind(urn)
                .bind(MAX_PER_ENTITY)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(comment_of).collect())
            }
            Inner::Memory(lock) => {
                let memory = lock.read().unwrap_or_else(|e| e.into_inner());
                Ok(memory
                    .comments
                    .iter()
                    .filter(|(p, s, c)| p == project && s == space && c.urn == urn)
                    .map(|(_, _, c)| c.clone())
                    .collect())
            }
        }
    }

    /// Removes one of the author's comments, the author known by their Keycloak `sub`, and the
    /// notifications it sent; `false` when the author holds none by that id in this space.
    pub async fn remove(
        &self,
        project: &str,
        space: &str,
        subject: &str,
        id: i64,
    ) -> Result<bool, sqlx::Error> {
        match &self.inner {
            // The notifications go with it: `ON DELETE CASCADE` (migration 0025).
            Inner::Postgres(pool) => Ok(sqlx::query(
                "DELETE FROM entity_comments WHERE project = $1 AND space = $2 AND author_subject = $3 AND id = $4",
            )
            .bind(project)
            .bind(space)
            .bind(subject)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let before = memory.comments.len();
                memory
                    .comments
                    .retain(|(p, s, c)| {
                        !(p == project
                            && s == space
                            && c.author_subject.as_deref() == Some(subject)
                            && c.id == id)
                    });
                let removed = memory.comments.len() < before;
                if removed {
                    memory.notifications.retain(|(_, n)| n.comment_id != Some(id));
                }
                Ok(removed)
            }
        }
    }

    /// The notifications of a person known by any of `recipients`, newest first, at most
    /// [`LISTED`], and how many of all theirs are unread.
    /// Leaves a connector event for `recipient`, a person's identifier or an audience
    /// ([`approvers_of`], [`CONNECTOR_ADMINS`]); `false` when its key was there already.
    pub async fn notify(&self, recipient: &str, notice: Notice<'_>) -> Result<bool, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query(
                "INSERT INTO notifications (recipient, kind, project, space, urn, author, author_name, excerpt, link, event_key) \
                 VALUES ($1, $2, $3, '', $4, $5, $6, $7, $8, $9) ON CONFLICT (recipient, event_key) DO NOTHING",
            )
            .bind(recipient)
            .bind(notice.kind)
            .bind(notice.project)
            .bind(notice.about)
            .bind(notice.author)
            .bind(notice.author_name)
            .bind(notice.connector)
            .bind(notice.link)
            .bind(notice.key)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                if memory
                    .keys
                    .iter()
                    .any(|(r, k, _)| r == recipient && k == notice.key)
                {
                    return Ok(false);
                }
                memory.next += 1;
                let id = memory.next;
                memory
                    .keys
                    .push((recipient.to_owned(), notice.key.to_owned(), id));
                let notification = Notification {
                    id,
                    kind: notice.kind.to_owned(),
                    project: notice.project.to_owned(),
                    space: String::new(),
                    urn: notice.about.to_owned(),
                    comment_id: None,
                    author: notice.author.to_owned(),
                    author_name: notice.author_name.to_owned(),
                    excerpt: notice.connector.to_owned(),
                    link: Some(notice.link.to_owned()),
                    created_at: rfc3339(time::OffsetDateTime::now_utc()),
                    read: false,
                };
                memory
                    .notifications
                    .push((recipient.to_owned(), notification));
                Ok(true)
            }
        }
    }

    /// Copies to `me` each notice of the last `AUDIENCE_DAYS` days addressed to one of
    /// `audiences`, the ones the caller holds the right of today, except the events the caller
    /// caused (`ids`); a notice already copied stays one.
    pub async fn deliver(
        &self,
        me: &str,
        ids: &[String],
        audiences: &[String],
    ) -> Result<(), sqlx::Error> {
        if audiences.is_empty() {
            return Ok(());
        }
        match &self.inner {
            Inner::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO notifications (recipient, kind, project, space, urn, author, author_name, excerpt, link, event_key, created_at) \
                     SELECT $1, kind, project, space, urn, author, author_name, excerpt, link, event_key, created_at \
                     FROM notifications WHERE recipient = ANY($2) AND event_key IS NOT NULL AND author <> ALL($3) \
                     AND created_at > now() - make_interval(days => $4) ORDER BY id \
                     ON CONFLICT (recipient, event_key) DO NOTHING",
                )
                .bind(me)
                .bind(audiences)
                .bind(ids)
                .bind(AUDIENCE_DAYS as i32)
                .execute(pool)
                .await?;
                Ok(())
            }
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let due: Vec<(String, Notification)> = memory
                    .keys
                    .iter()
                    .filter(|(r, _, _)| audiences.contains(r))
                    .filter_map(|(_, key, id)| {
                        let (_, n) = memory.notifications.iter().find(|(_, n)| n.id == *id)?;
                        (!ids.contains(&n.author)).then(|| (key.clone(), n.clone()))
                    })
                    .collect();
                for (key, notification) in due {
                    if memory.keys.iter().any(|(r, k, _)| r == me && *k == key) {
                        continue;
                    }
                    memory.next += 1;
                    let id = memory.next;
                    memory.keys.push((me.to_owned(), key, id));
                    let copy = Notification {
                        id,
                        read: false,
                        ..notification
                    };
                    memory.notifications.push((me.to_owned(), copy));
                }
                Ok(())
            }
        }
    }

    pub async fn notifications(
        &self,
        recipients: &[String],
    ) -> Result<(Vec<Notification>, i64), sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => {
                let rows: Vec<NotificationRow> = sqlx::query_as(
                    "SELECT id, kind, project, space, urn, comment_id, author, author_name, excerpt, link, created_at, read_at \
                     FROM notifications WHERE recipient = ANY($1) ORDER BY id DESC LIMIT $2",
                )
                .bind(recipients)
                .bind(LISTED)
                .fetch_all(pool)
                .await?;
                let (unread,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM notifications WHERE recipient = ANY($1) AND read_at IS NULL",
                )
                .bind(recipients)
                .fetch_one(pool)
                .await?;
                Ok((rows.into_iter().map(notification_of).collect(), unread))
            }
            Inner::Memory(lock) => {
                let memory = lock.read().unwrap_or_else(|e| e.into_inner());
                let mine: Vec<&Notification> = memory
                    .notifications
                    .iter()
                    .filter(|(r, _)| recipients.contains(r))
                    .map(|(_, n)| n)
                    .collect();
                let unread = mine.iter().filter(|n| !n.read).count() as i64;
                Ok((
                    mine.into_iter()
                        .rev()
                        .take(LISTED as usize)
                        .cloned()
                        .collect(),
                    unread,
                ))
            }
        }
    }

    /// Marks one of the person's notifications read; `false` when none of theirs has that id.
    pub async fn mark_read(&self, recipients: &[String], id: i64) -> Result<bool, sqlx::Error> {
        match &self.inner {
            Inner::Postgres(pool) => Ok(sqlx::query(
                "UPDATE notifications SET read_at = coalesce(read_at, now()) WHERE id = $1 AND recipient = ANY($2)",
            )
            .bind(id)
            .bind(recipients)
            .execute(pool)
            .await?
            .rows_affected()
                > 0),
            Inner::Memory(lock) => {
                let mut memory = lock.write().unwrap_or_else(|e| e.into_inner());
                let mut found = false;
                for (recipient, notification) in memory.notifications.iter_mut() {
                    if notification.id == id && recipients.contains(recipient) {
                        notification.read = true;
                        found = true;
                    }
                }
                Ok(found)
            }
        }
    }
}
