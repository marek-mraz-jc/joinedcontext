//! Writes (T-7508): each takes its tenant's lock, brings the typed tables up to the log, appends
//! its operation and applies it, all in one transaction, so any replica reads it once committed.

use crate::tables::{self, Table, Taken, failed, put, to_row};
use crate::{Op, hidden};
use jc_audit::Tx;
use serde_json::{Value, json};
use shako_core::{InstanceId, OrgId, RepoId, ShakoError, ShakoResult};

/// Where an operation stands in the log: the time and the order its rows carry.
#[derive(Debug, Clone)]
pub(crate) struct At {
    seq: i64,
    time: String,
}

/// Holds the tenant's writers back until `tx` ends.
// ponytail: one writer per tenant, as the process-wide write lock was; a lock per repository when
// writes of one Organization queue.
pub(crate) async fn lock(tx: &mut Tx, tenant: (InstanceId, OrgId)) -> ShakoResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("shako.store:{}:{}", tenant.0, tenant.1))
        .execute(&mut **tx)
        .await
        .map_err(failed)?;
    Ok(())
}

/// Appends `op` to the tenant's log.
pub(crate) async fn append(tx: &mut Tx, tenant: (InstanceId, OrgId), op: &Op) -> ShakoResult<At> {
    let doc = serde_json::to_value(op).map_err(tables::unserialisable)?;
    let (seq, time): (i64, String) = sqlx::query_as(
        "INSERT INTO shako.store_log (instance_id, org_id, op) VALUES ($1, $2, $3)
         RETURNING seq, to_jsonb(created_at) #>> '{}'",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .bind(doc)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| ShakoError::Storage(format!("store log write failed: {e}")))?;
    Ok(At { seq, time })
}

/// Records that the tables hold the log up to `at`.
pub(crate) async fn projected(
    tx: &mut Tx,
    tenant: (InstanceId, OrgId),
    seq: i64,
) -> ShakoResult<()> {
    sqlx::query(
        "INSERT INTO shako.projection (instance_id, org_id, seq) VALUES ($1, $2, $3)
         ON CONFLICT (instance_id, org_id) DO UPDATE SET seq = EXCLUDED.seq",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .bind(seq)
    .execute(&mut **tx)
    .await
    .map_err(failed)?;
    Ok(())
}

/// Entries `trim` takes per transaction.
const TRIM_BATCH: i64 = 1000;

/// Deletes the oldest `TRIM_BATCH` log entries of the tenant that the `trim_projected` policy
/// lets go; the query repeats the policy's conditions, so a role past RLS trims no more. Answers
/// how many it picked and how many went.
pub(crate) async fn trim(tx: &mut Tx, tenant: (InstanceId, OrgId)) -> ShakoResult<(u64, u64)> {
    let (picked, gone): (i64, i64) = sqlx::query_as(
        "WITH batch AS (
             SELECT l.seq FROM shako.store_log l
             JOIN shako.projection p USING (instance_id, org_id)
             WHERE l.instance_id = $1 AND l.org_id = $2 AND l.op ->> 'op' <> 'create_repo'
               AND l.seq <= p.seq AND l.created_at < now() - interval '30 days'
               AND p.since < now() - interval '30 days'
             ORDER BY l.seq LIMIT $3),
         gone AS (
             DELETE FROM shako.store_log l USING batch WHERE l.seq = batch.seq RETURNING 1)
         SELECT (SELECT count(*) FROM batch), (SELECT count(*) FROM gone)",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .bind(TRIM_BATCH)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| ShakoError::Storage(format!("store log trim failed: {e}")))?;
    Ok((picked.unsigned_abs(), gone.unsigned_abs()))
}

impl At {
    pub(crate) const fn seq(&self) -> i64 {
        self.seq
    }
}

/// Applies what the log holds past the tables: the whole log on a database an earlier release
/// left, what that release wrote during a rollback, else nothing. Answers how many it read.
pub(crate) async fn catch_up(tx: &mut Tx, tenant: (InstanceId, OrgId)) -> ShakoResult<usize> {
    let held: Option<i64> = sqlx::query_scalar(
        "SELECT seq FROM shako.projection WHERE instance_id = $1 AND org_id = $2",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(failed)?;
    let after = held.unwrap_or(0);
    let mut next = entry_after(tx, tenant, after).await?;
    if next.is_none() && held.is_some() {
        return Ok(0);
    }
    // An earlier release wrote repository rows beside their entries, so the table holds their
    // last state: those first created past `after` start again, or an older name meets a later
    // owner of it. A repository the tables already held by then is left alone.
    sqlx::query(
        "DELETE FROM shako.repo WHERE instance_id = $1 AND org_id = $2 AND id IN (
             SELECT (op #>> '{args,id}')::uuid FROM shako.store_log
             WHERE instance_id = $1 AND org_id = $2 AND op ->> 'op' = 'create_repo'
             GROUP BY 1 HAVING min(seq) > $3)",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .bind(after)
    .execute(&mut **tx)
    .await
    .map_err(failed)?;
    let (mut count, mut last) = (0, after);
    while let Some((seq, time, doc)) = next {
        let op: Op = serde_json::from_value(doc)
            .map_err(|e| ShakoError::Storage(format!("store log entry unreadable: {e}")))?;
        sqlx::query("SAVEPOINT op")
            .execute(&mut **tx)
            .await
            .map_err(failed)?;
        let end = match apply(tx, tenant, op, &At { seq, time }).await {
            Ok(_) => "RELEASE SAVEPOINT op",
            // Refused when it was first made, and refused again (an earlier release logged those).
            Err(e @ (ShakoError::Conflict(_) | ShakoError::NotFound(_))) => {
                tracing::debug!(error = %e, seq, "shako store log: an operation refused again");
                "ROLLBACK TO SAVEPOINT op"
            }
            Err(e) => return Err(e),
        };
        sqlx::query(end).execute(&mut **tx).await.map_err(failed)?;
        (count, last) = (count + 1, seq);
        next = entry_after(tx, tenant, seq).await?;
    }
    if held.is_none() {
        // The first projection: an event without an entry was never to be sent from here.
        sqlx::query(
            "UPDATE shako.outbox SET dispatched = true
             WHERE instance_id = $1 AND org_id = $2 AND position = 0 AND NOT dispatched",
        )
        .bind(tenant.0.as_uuid())
        .bind(tenant.1.as_uuid())
        .execute(&mut **tx)
        .await
        .map_err(failed)?;
    }
    projected(tx, tenant, last).await?;
    Ok(count)
}

/// The first log entry past `seq`. One at a time: an entry of an earlier release holds up to
/// 4 MiB of manifest, five times that as JSON.
async fn entry_after(
    tx: &mut Tx,
    tenant: (InstanceId, OrgId),
    seq: i64,
) -> ShakoResult<Option<(i64, String, Value)>> {
    sqlx::query_as(
        "SELECT seq, to_jsonb(created_at) #>> '{}', op FROM shako.store_log
         WHERE instance_id = $1 AND org_id = $2 AND seq > $3 ORDER BY seq LIMIT 1",
    )
    .bind(tenant.0.as_uuid())
    .bind(tenant.1.as_uuid())
    .bind(seq)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ShakoError::Storage(format!("store log read failed: {e}")))
}

fn record(value: impl serde::Serialize) -> ShakoResult<Value> {
    serde_json::to_value(value).map_err(tables::unserialisable)
}

fn hidden_record(result: Result<Value, serde_json::Error>) -> ShakoResult<Value> {
    result.map_err(tables::unserialisable)
}

async fn replace(tx: &mut Tx, table: &Table, record: Value) -> ShakoResult<Value> {
    put(tx, table, to_row(table, record)?, Taken::Replace)
        .await?
        .ok_or_else(|| ShakoError::Storage("a write stored no row".into()))
}

/// Applies `op` to the typed tables and answers what the Store's method answers.
pub(crate) async fn apply(
    tx: &mut Tx,
    tenant: (InstanceId, OrgId),
    op: Op,
    at: &At,
) -> ShakoResult<Value> {
    let (instance, org) = (tenant.0.as_uuid(), tenant.1.as_uuid());
    let ser = serde_json::value::Serializer;
    match op {
        Op::UpsertInstance(r) => replace(tx, &tables::INSTANCE, record(r)?).await,
        Op::AdjustQuotaUsage {
            repo_delta,
            git_bytes_delta,
            package_bytes_delta,
        } => {
            let row: Value = sqlx::query_scalar(
                "INSERT INTO shako.quota_usage AS t
                    (instance_id, org_id, repo_count, git_bytes, package_bytes, updated_at)
                 VALUES ($1, $2, GREATEST($3, 0), GREATEST($4, 0), GREATEST($5, 0), $6::timestamptz)
                 ON CONFLICT (instance_id, org_id) DO UPDATE SET
                    repo_count = GREATEST(t.repo_count + $3, 0),
                    git_bytes = GREATEST(t.git_bytes + $4, 0),
                    package_bytes = GREATEST(t.package_bytes + $5, 0),
                    updated_at = $6::timestamptz
                 RETURNING to_jsonb(t)",
            )
            .bind(instance)
            .bind(org)
            .bind(repo_delta)
            .bind(git_bytes_delta)
            .bind(package_bytes_delta)
            .bind(&at.time)
            .fetch_one(&mut **tx)
            .await
            .map_err(failed)?;
            tables::to_record(&tables::QUOTA_USAGE, row)
        }
        Op::CreateRepo(r) => put(
            tx,
            &tables::REPO,
            to_row(&tables::REPO, record(r)?)?,
            Taken::Keep,
        )
        .await?
        .ok_or_else(|| ShakoError::Conflict("repository already exists".into())),
        Op::UpdateRepo(mut r) => {
            let next: i64 = sqlx::query_scalar(
                "SELECT next_pr_number FROM shako.repo
                 WHERE instance_id = $1 AND org_id = $2 AND id = $3",
            )
            .bind(instance)
            .bind(org)
            .bind(r.id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(failed)?
            .ok_or_else(|| ShakoError::NotFound("repository not found".into()))?;
            // Pull request numbers only move forward (as the memory store has it).
            r.next_pr_number = r.next_pr_number.max(u64::try_from(next).unwrap_or(0));
            let mut doc = record(r)?;
            doc["updatedAt"] = json!(at.time);
            replace(tx, &tables::REPO, doc).await
        }
        Op::DeleteRepo { repo_id } => delete_repo(tx, tenant, repo_id).await,
        Op::AllocatePrNumber { repo_id } => {
            let number: i64 = sqlx::query_scalar(
                "UPDATE shako.repo SET next_pr_number = GREATEST(next_pr_number, 1) + 1
                 WHERE instance_id = $1 AND org_id = $2 AND id = $3
                 RETURNING next_pr_number - 1",
            )
            .bind(instance)
            .bind(org)
            .bind(repo_id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(failed)?
            .ok_or_else(|| ShakoError::NotFound("repository not found".into()))?;
            Ok(json!(number))
        }
        Op::UpsertProtectionRule(r) => {
            let (name, mut doc) = (r.rule.rule_name.clone(), record(r)?);
            doc["kind"] = json!("branch");
            doc["ruleName"] = json!(name);
            doc["updatedAt"] = json!(at.time);
            replace(tx, &tables::PROTECTION_RULE, doc).await
        }
        Op::DeleteProtectionRule { repo_id, rule_name } => {
            let done = sqlx::query(
                "DELETE FROM shako.protection_rule WHERE instance_id = $1 AND org_id = $2
                 AND repo_id = $3 AND kind = 'branch' AND rule_name = $4",
            )
            .bind(instance)
            .bind(org)
            .bind(repo_id.as_uuid())
            .bind(rule_name)
            .execute(&mut **tx)
            .await
            .map_err(failed)?;
            Ok(json!(done.rows_affected() > 0))
        }
        Op::UpsertTagProtection(r) => {
            let (name, created, mut doc) = (r.rule.name_pattern.clone(), r.created_at, record(r)?);
            doc["kind"] = json!("tag");
            doc["ruleName"] = json!(name);
            doc["updatedAt"] = record(created)?;
            replace(tx, &tables::PROTECTION_RULE, doc).await
        }
        Op::CreatePullRequest(r) => {
            let number = r.number;
            let row = to_row(&tables::PULL_REQUEST, record(r)?)?;
            put(tx, &tables::PULL_REQUEST, row, Taken::Keep)
                .await?
                .ok_or_else(|| {
                    ShakoError::Conflict(format!("pull request #{number} already exists"))
                })
        }
        Op::UpdatePullRequest(r) => {
            let mut doc = record(r)?;
            doc["updatedAt"] = json!(at.time);
            replace(tx, &tables::PULL_REQUEST, doc).await
        }
        Op::CreateReview(r) => {
            let mut doc = record(r)?;
            doc["position"] = json!(at.seq);
            replace(tx, &tables::REVIEW, doc).await
        }
        Op::UpsertCommitStatus(r) => replace(tx, &tables::COMMIT_STATUS, record(r)?).await,
        Op::CreateWebhook(r) => {
            let doc = hidden_record(hidden::webhook::serialize(&r, ser))?;
            replace(tx, &tables::WEBHOOK, doc).await
        }
        Op::RecordDelivery(r) => {
            let mut doc = record(r)?;
            doc["position"] = json!(at.seq);
            replace(tx, &tables::DELIVERY, doc).await
        }
        Op::UpsertPushMirror(r) => {
            let doc = hidden_record(hidden::push_mirror::serialize(&r, ser))?;
            replace(tx, &tables::PUSH_MIRROR, doc).await
        }
        Op::DeletePushMirror {
            repo_id,
            remote_name,
        } => {
            let done = sqlx::query(
                "DELETE FROM shako.push_mirror WHERE instance_id = $1 AND org_id = $2
                 AND repo_id = $3 AND remote_name = $4",
            )
            .bind(instance)
            .bind(org)
            .bind(repo_id.as_uuid())
            .bind(remote_name)
            .execute(&mut **tx)
            .await
            .map_err(failed)?;
            Ok(json!(done.rows_affected() > 0))
        }
        Op::UpsertPackage(r) => {
            let key = [&r.owner, &r.package_type, &r.name].map(|v| Some(v.clone()));
            let row = to_row(&tables::PACKAGE, record(r)?)?;
            if let Some(stored) = put(tx, &tables::PACKAGE, row, Taken::Keep).await? {
                return Ok(stored);
            }
            tables::select::<Value>(
                tx,
                tenant,
                &tables::PACKAGE,
                "AND t.owner = $3 AND t.package_type = $4 AND t.name = $5",
                &key,
            )
            .await?
            .pop()
            .ok_or_else(|| ShakoError::Conflict("package id already taken".into()))
        }
        Op::UpsertPackageVersion(r) => {
            let mut doc = record(r.record)?;
            // Always written, empty for a new version: the release before reads a NULL as broken.
            doc["manifestBytes"] = json!(r.manifest_bytes);
            replace(tx, &tables::PACKAGE_VERSION, doc).await
        }
        Op::InsertPackageFile(r) => {
            let row = to_row(&tables::PACKAGE_FILE, record(r)?)?;
            put(tx, &tables::PACKAGE_FILE, row, Taken::Keep)
                .await?
                .ok_or_else(|| ShakoError::Conflict("package file already exists".into()))
        }
        Op::UpsertBlob(r) => replace(tx, &tables::BLOB, record(r)?).await,
        Op::AppendOutbox(r) => {
            let key = r.idempotency_key.clone();
            let mut doc = record(r)?;
            doc["position"] = json!(at.seq);
            let row = to_row(&tables::OUTBOX, doc)?;
            if let Some(stored) = put(tx, &tables::OUTBOX, row, Taken::Keep).await? {
                return Ok(stored);
            }
            // The event is there already: again, or written beside its entry by an earlier release.
            let row: Option<Value> = sqlx::query_scalar(
                "UPDATE shako.outbox t SET position = CASE WHEN position = 0 THEN $4 ELSE position END
                 WHERE instance_id = $1 AND org_id = $2 AND idempotency_key = $3
                 RETURNING to_jsonb(t)",
            )
            .bind(instance)
            .bind(org)
            .bind(key)
            .bind(at.seq)
            .fetch_optional(&mut **tx)
            .await
            .map_err(failed)?;
            // ponytail: the key is unique per instance, which an earlier release's ON CONFLICT
            // names; add org_id to it once no rollback reaches that release.
            let row = row.ok_or_else(|| ShakoError::Conflict("the event's key is taken".into()))?;
            tables::to_record(&tables::OUTBOX, row)
        }
        Op::MarkOutboxDispatched { event_id } => {
            let done = sqlx::query(
                "UPDATE shako.outbox
                 SET dispatched = true, dispatched_at = $4::timestamptz, claimed_until = NULL
                 WHERE instance_id = $1 AND org_id = $2 AND id = $3",
            )
            .bind(instance)
            .bind(org)
            .bind(event_id)
            .bind(&at.time)
            .execute(&mut **tx)
            .await
            .map_err(failed)?;
            Ok(json!(done.rows_affected() > 0))
        }
    }
}

async fn delete_repo(
    tx: &mut Tx,
    tenant: (InstanceId, OrgId),
    repo_id: RepoId,
) -> ShakoResult<Value> {
    let mut removed = false;
    for sql in [
        "DELETE FROM shako.protection_rule WHERE instance_id = $1 AND org_id = $2 AND repo_id = $3",
        "DELETE FROM shako.pull_request WHERE instance_id = $1 AND org_id = $2 AND repo_id = $3",
        "DELETE FROM shako.review WHERE instance_id = $1 AND org_id = $2 AND repo_id = $3",
        "DELETE FROM shako.push_mirror WHERE instance_id = $1 AND org_id = $2 AND repo_id = $3",
        "DELETE FROM shako.repo WHERE instance_id = $1 AND org_id = $2 AND id = $3",
    ] {
        let done = sqlx::query(sql)
            .bind(tenant.0.as_uuid())
            .bind(tenant.1.as_uuid())
            .bind(repo_id.as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(failed)?;
        removed = done.rows_affected() > 0;
    }
    Ok(json!(removed))
}
