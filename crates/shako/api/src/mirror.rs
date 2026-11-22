#![forbid(unsafe_code)]
//! Push mirrors follow a write as a job (T-7522, LOP-01): the write marks the repository due in
//! the store and answers; a drain claims it under a lease, so one replica syncs it and a crash
//! leaves it due.

use crate::state::ShakoState;
use crate::work::Spool;
use shako_core::{InstanceId, OrgId, PushMirrorRecord, RepoRecord, ShakoError, ShakoResult};
use shako_git::{MirrorRemote, write_send_pack};
use std::path::PathBuf;
use std::time::Duration;

/// A repository's sync, all its mirrors together; shorter than the lease.
const SYNC_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long "sync now" waits for the result.
const SYNC_NOW_WAIT: Duration = Duration::from_secs(10);
/// How often "sync now" looks at a sync another task or replica runs.
const SYNC_NOW_POLL: Duration = Duration::from_millis(200);

/// Records that `repo`'s sync-on-commit mirrors are behind. `None`: it has none.
pub(crate) async fn mark_due(state: &ShakoState, repo: &RepoRecord) -> Option<i64> {
    let marked = async {
        let store = state.store();
        let mirrors = store
            .list_push_mirrors(repo.instance_id, repo.org_id, Some(repo.id))
            .await?;
        if !mirrors.iter().any(|m| m.sync_on_commit) {
            return Ok(None);
        }
        let mark = store
            .mark_mirror_due(repo.instance_id, repo.org_id, repo.id)
            .await?;
        Ok::<_, ShakoError>(Some(mark))
    };
    marked.await.unwrap_or_else(|e| {
        tracing::warn!(repo = %repo.id, error = %e, "the push mirrors were not marked due; the next push tries again");
        None
    })
}

/// After a write moved refs: due, and a drain starts. Nothing waits for a remote.
pub(crate) async fn follow_write(state: &ShakoState, repo: &RepoRecord) {
    if mark_due(state, repo).await.is_some() {
        drain_mirrors(state, repo.instance_id, repo.org_id);
    }
}

/// Syncs what is due in the background, at most two drains in a process. A host calls it on a
/// timer for what a crash or another replica left.
pub fn drain_mirrors(state: &ShakoState, instance: InstanceId, org: OrgId) {
    let Ok(slot) = state.mirror_drains.clone().try_acquire_owned() else {
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let _slot = slot;
        loop {
            match state.store().claim_mirror_sync(instance, org, None).await {
                Ok(Some((repo, mark))) => sync_claimed(&state, instance, org, repo, mark).await,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "the due push mirrors could not be read, trying again");
                    break;
                }
            }
        }
    });
}

/// Syncs `repo` now and waits a little for the result; a longer sync ends in the background and
/// answers [`still_running`].
pub(crate) async fn sync_now(state: &ShakoState, repo: &RepoRecord) -> ShakoResult<()> {
    let (instance, org, id) = (repo.instance_id, repo.org_id, repo.id);
    let Some(wanted) = mark_due(state, repo).await else {
        return Ok(());
    };
    let deadline = tokio::time::Instant::now() + SYNC_NOW_WAIT;
    let store = state.store();
    let mark = loop {
        if let Some((_, mark)) = store.claim_mirror_sync(instance, org, Some(id)).await? {
            break mark;
        }
        // A sync of it runs elsewhere; the mark makes it run again, and that result answers.
        if store.mirror_synced(instance, org, id, wanted).await? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(still_running());
        }
        tokio::time::sleep(SYNC_NOW_POLL).await;
    };
    let job = tokio::spawn({
        let state = state.clone();
        async move { sync_claimed(&state, instance, org, id, mark).await }
    });
    match tokio::time::timeout_at(deadline, job).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(ShakoError::Internal(e.to_string())),
        Err(_) => Err(still_running()),
    }
}

/// A sync that has no result yet: not a success, so a caller reading the mirror's last error
/// next does not take an empty one for a pushed mirror.
fn still_running() -> ShakoError {
    ShakoError::Overloaded(format!(
        "the push mirror sync runs longer than {} s; its result shows in the mirror's last error",
        SYNC_NOW_WAIT.as_secs()
    ))
}

async fn sync_claimed(
    state: &ShakoState,
    instance: InstanceId,
    org: OrgId,
    id: shako_core::RepoId,
    mark: i64,
) {
    if let Err(e) = sync_repo(state, instance, org, id).await {
        tracing::warn!(repo = %id, error = %e, "the push mirrors were not synced");
    }
    if let Err(e) = state
        .store()
        .finish_mirror_sync(instance, org, id, mark)
        .await
    {
        tracing::warn!(repo = %id, error = %e, "a finished mirror sync was not recorded; it runs again");
    }
}

async fn sync_repo(
    state: &ShakoState,
    instance: InstanceId,
    org: OrgId,
    id: shako_core::RepoId,
) -> ShakoResult<()> {
    let Some(repo) = state.store().get_repo_by_id(instance, org, id).await? else {
        return Ok(());
    };
    let disk = shako_core::repo_disk_path(state.repo_root(), instance, id)?;
    let deadline = tokio::time::Instant::now() + SYNC_LIMIT;
    let mirrors = state
        .store()
        .list_push_mirrors(instance, org, Some(id))
        .await?;
    for mirror in mirrors.into_iter().filter(|m| m.sync_on_commit) {
        let result = tokio::time::timeout_at(deadline, push(state, &repo, &disk, &mirror))
            .await
            .unwrap_or_else(|_| {
                Err(ShakoError::Conflict(
                    "the sync took longer than 10 minutes; the next push tries again".into(),
                ))
            });
        record(state, &mirror, result).await?;
    }
    Ok(())
}

async fn push(
    state: &ShakoState,
    repo: &RepoRecord,
    disk: &std::path::Path,
    mirror: &PushMirrorRecord,
) -> ShakoResult<()> {
    if !shako_core::is_secret_ref(&mirror.credential_secret_ref) {
        // Stored before the API refused non-key names: still used, never logged.
        tracing::warn!(mirror = %mirror.id, "a push mirror's secret reference is not a secret-store key name; add the mirror again");
    }
    let cred = match state
        .secret_store
        .get_secret(repo.org_id, &mirror.credential_secret_ref)
        .await
    {
        Ok(held) => held.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(mirror = %mirror.id, error = %e, "a push mirror's secret could not be read; not pushed");
            return Err(ShakoError::Conflict(
                "the mirror's credential could not be read; the next push tries again".into(),
            ));
        }
    };
    let (user, pass) = cred
        .split_once(':')
        .map_or(("x-access-token", cred.as_str()), |(u, p)| (u, p));
    let remote = MirrorRemote::new(&mirror.remote_address, user, pass, &state.outbound_guard);
    let held = remote.refs().await?;
    let branch = Some(mirror.branch.clone()).filter(|b| !b.is_empty());
    let disk: PathBuf = disk.to_path_buf();
    let cap = shako_core::MAX_SINGLE_PUSH_BYTES;
    let (spool, plan): (Spool, _) = state
        .git_work
        .produce(repo.org_id, move |out, admit| {
            out.cap(cap);
            write_send_pack(&disk, &held, branch.as_deref(), out, admit)
        })
        .await?;
    let request = spool
        .file()
        .try_clone()
        .map_err(|e| ShakoError::Storage(format!("mirror spool: {e}")))?;
    remote.push(&plan, request).await
}

/// Writes the result where the mirror still is: a late one never brings a removed mirror back.
async fn record(
    state: &ShakoState,
    mirror: &PushMirrorRecord,
    result: ShakoResult<()>,
) -> ShakoResult<()> {
    let store = state.store();
    let now = store
        .list_push_mirrors(mirror.instance_id, mirror.org_id, Some(mirror.repo_id))
        .await?;
    let Some(mut current) = now.into_iter().find(|m| m.id == mirror.id) else {
        return Ok(());
    };
    match result {
        Ok(()) => {
            current.last_sync = Some(jc_time::TimestampTz::now());
            current.last_error = String::new();
        }
        Err(e) => current.last_error = e.public_detail(),
    }
    store.upsert_push_mirror(current).await.map(drop)
}
