// SPDX-License-Identifier: EUPL-1.2
//! Write confirmations (T-7507, migration 0047), shared by every replica.
//! Every statement runs in the Tenant's transaction: the id the decision
//! page names carries its Tenant, so none needs the escape of 0042.

use antares_model::TenantId;
use antares_store::confirmations::{Caps, Confirmation, Subject, Taken};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::{Postgres, Row, Transaction};
use std::time::Duration;

/// Held to the transaction's end: the caps are a count and then an insert.
const LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('mcp_confirmations:' || $1, 0))";
/// ponytail: counts the Tenant's live rows on each insert, at most the cap;
/// keep a counter row when a Tenant confirms thousands of writes a minute.
const HELD_SQL: &str = "SELECT count(*), count(*) FILTER (WHERE subject = $2)
     FROM mcp_confirmations
     WHERE tenant_id = $1 AND (view IS NOT NULL) = $3 AND expires_at > now()";
const INSERT_SQL: &str = "INSERT INTO mcp_confirmations
       (tenant_id, id, subject, view, tool, message, expires_at)
     VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))
     ON CONFLICT (tenant_id, id) DO NOTHING";
const READ_SQL: &str = "SELECT subject, view, tool, message, decision,
       extract(epoch FROM expires_at - now())::float8 AS secs_left
     FROM mcp_confirmations
     WHERE tenant_id = $1 AND id = $2 AND view IS NOT NULL AND expires_at > now()";
const DECIDE_SQL: &str = "UPDATE mcp_confirmations SET decision = $3
     WHERE tenant_id = $1 AND id = $2 AND view IS NOT NULL AND expires_at > now()
     RETURNING subject, view, tool, message, decision,
       extract(epoch FROM expires_at - now())::float8 AS secs_left";
/// One statement, so of two retries on two replicas one gets the decision.
const TAKE_SQL: &str = "DELETE FROM mcp_confirmations
     WHERE tenant_id = $1 AND id = $2 AND subject = $3 AND view IS NOT NULL
       AND decision IS NOT NULL AND expires_at > now()
     RETURNING decision";
const WAITING_SQL: &str = "SELECT EXISTS (SELECT 1 FROM mcp_confirmations
     WHERE tenant_id = $1 AND id = $2 AND subject = $3 AND view IS NOT NULL
       AND decision IS NULL AND expires_at > now())";
const SWEEP_SQL: &str =
    "DELETE FROM mcp_confirmations WHERE tenant_id = $1 AND expires_at <= now()";

async fn scoped(
    pool: &PgPool,
    tenant: &TenantId,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    super::set_tenant(&mut tx, tenant).await?;
    Ok(tx)
}

/// The subject as stored and compared: its JSON text, header bytes as numbers
/// (what `serde_json::to_string` writes, built without its never-taken error).
fn key(subject: &Subject) -> String {
    use serde_json::Value;
    let pairs = subject.iter().map(|(name, bytes)| {
        Value::Array(vec![Value::from(name.as_str()), Value::from(bytes.clone())])
    });
    Value::Array(pairs.collect()).to_string()
}

fn question(row: &PgRow) -> Result<Confirmation, sqlx::Error> {
    Ok(Confirmation {
        subject: serde_json::from_str(row.try_get("subject")?).map_err(super::undecodable)?,
        view: row.try_get("view")?,
        tool: row.try_get("tool")?,
        message: row.try_get("message")?,
        decision: row.try_get("decision")?,
        left: Duration::try_from_secs_f64(row.try_get("secs_left")?).unwrap_or_default(),
    })
}

/// A row to insert: a question, or a spent nonce when `about` is `None`.
struct New<'a> {
    id: &'a str,
    subject: &'a Subject,
    about: Option<&'a Confirmation>,
    ttl: Duration,
}

async fn put(
    pool: &PgPool,
    tenant: &TenantId,
    new: New<'_>,
    caps: Caps,
) -> Result<bool, sqlx::Error> {
    let subject = key(new.subject);
    let mut tx = scoped(pool, tenant).await?;
    sqlx::query(LOCK_SQL)
        .bind(tenant.as_str())
        .execute(&mut *tx)
        .await?;
    // also what frees an expired row that still holds the id
    sqlx::query(SWEEP_SQL)
        .bind(tenant.as_str())
        .execute(&mut *tx)
        .await?;
    let (held, own): (i64, i64) = sqlx::query_as(HELD_SQL)
        .bind(tenant.as_str())
        .bind(&subject)
        .bind(new.about.is_some())
        .fetch_one(&mut *tx)
        .await?;
    if held >= i64::from(caps.tenant) || own >= i64::from(caps.subject) {
        return Ok(false);
    }
    let written = sqlx::query(INSERT_SQL)
        .bind(tenant.as_str())
        .bind(new.id)
        .bind(&subject)
        .bind(new.about.map(|q| q.view.as_str()))
        .bind(new.about.map(|q| q.tool.as_str()))
        .bind(new.about.map(|q| q.message.as_str()))
        .bind(new.ttl.as_secs_f64())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(written == 1)
}

pub async fn ask(
    pool: &PgPool,
    tenant: &TenantId,
    id: &str,
    q: &Confirmation,
    caps: Caps,
) -> Result<bool, sqlx::Error> {
    let new = New {
        id,
        subject: &q.subject,
        about: Some(q),
        ttl: q.left,
    };
    put(pool, tenant, new, caps).await
}

pub async fn spend(
    pool: &PgPool,
    tenant: &TenantId,
    id: &str,
    subject: &Subject,
    ttl: Duration,
    caps: Caps,
) -> Result<bool, sqlx::Error> {
    let new = New {
        id,
        subject,
        about: None,
        ttl,
    };
    put(pool, tenant, new, caps).await
}

/// `tenant`'s open question `id`.
pub async fn find(
    pool: &PgPool,
    tenant: &TenantId,
    id: &str,
) -> Result<Option<Confirmation>, sqlx::Error> {
    let mut tx = scoped(pool, tenant).await?;
    let row = sqlx::query(READ_SQL)
        .bind(tenant.as_str())
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    row.as_ref().map(question).transpose()
}

pub async fn decide(
    pool: &PgPool,
    tenant: &TenantId,
    id: &str,
    accept: bool,
) -> Result<Option<Confirmation>, sqlx::Error> {
    let mut tx = scoped(pool, tenant).await?;
    let row = sqlx::query(DECIDE_SQL)
        .bind(tenant.as_str())
        .bind(id)
        .bind(accept)
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    row.as_ref().map(question).transpose()
}

pub async fn take(
    pool: &PgPool,
    tenant: &TenantId,
    id: &str,
    subject: &Subject,
) -> Result<Taken, sqlx::Error> {
    let subject = key(subject);
    let mut tx = scoped(pool, tenant).await?;
    let decided: Option<bool> = sqlx::query_scalar(TAKE_SQL)
        .bind(tenant.as_str())
        .bind(id)
        .bind(&subject)
        .fetch_optional(&mut *tx)
        .await?;
    let taken = match decided {
        Some(true) => Taken::Accepted,
        Some(false) => Taken::Declined,
        None => {
            let waiting: bool = sqlx::query_scalar(WAITING_SQL)
                .bind(tenant.as_str())
                .bind(id)
                .bind(&subject)
                .fetch_one(&mut *tx)
                .await?;
            if waiting {
                Taken::Waiting
            } else {
                Taken::Unknown
            }
        }
    };
    tx.commit().await?;
    Ok(taken)
}

pub async fn sweep(pool: &PgPool, tenant: &TenantId) -> Result<u64, sqlx::Error> {
    let mut tx = scoped(pool, tenant).await?;
    let gone = sqlx::query(SWEEP_SQL)
        .bind(tenant.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(gone)
}

#[cfg(test)]
mod tests {
    /// `key` writes exactly the text serde writes, which rows stored before it
    /// hold and `question` parses back; an empty subject and non-ASCII bytes too.
    #[test]
    fn key_is_the_serde_text() {
        for subject in [
            vec![],
            vec![("x-jc-subject".to_owned(), b"alice".to_vec())],
            vec![
                ("a\"b".to_owned(), vec![0, 255, 34]),
                ("\u{e9}".to_owned(), Vec::new()),
            ],
        ] {
            let text = super::key(&subject);
            assert_eq!(text, serde_json::to_string(&subject).expect("serde"));
            let back: antares_store::confirmations::Subject =
                serde_json::from_str(&text).expect("parses back");
            assert_eq!(back, subject);
        }
    }
}
