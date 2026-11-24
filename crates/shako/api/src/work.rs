#![forbid(unsafe_code)]
//! The two budgets of Git work (T-7522, LIM-01): disk for what a peer still sends or reads, and
//! memory for the CPU phase alone, so a slow peer holds no memory. Both are keyed by
//! Organization. The lock order is spool, repository, memory.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt as _;
use jc_limits::{FairBudget, Permit, Refused};
use shako_core::{OrgId, ShakoError, ShakoResult};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const MIB: u64 = 1024 * 1024;

/// What one process spools at once, the volume permitting.
pub const SPOOL_BYTES: u64 = 4096 * MIB;
/// What Git's CPU phases hold at once (S22).
pub const MEMORY_BYTES: u64 = 192 * MIB;
/// Left free on the repository volume by every spool.
const VOLUME_RESERVE: u64 = 256 * MIB;
const SPOOL_STEP: u64 = 4 * MIB;
const SPOOL_MIN: u64 = MIB;
const MEMORY_MIN: u64 = 4 * MIB;
const SPOOL_WAIT: Duration = Duration::from_secs(2);
const MEMORY_WAIT: Duration = Duration::from_secs(8);
/// A peer that sends nothing for this long is dropped.
const SEND_IDLE: Duration = Duration::from_secs(30);
/// A peer that reads nothing for this long is dropped.
const READ_IDLE: Duration = Duration::from_secs(60);
/// The longest transfer, however steady the peer.
const TRANSFER: Duration = Duration::from_secs(30 * 60);

fn refused(what: &str, refusal: Refused) -> ShakoError {
    match refusal {
        Refused::TooLarge(limit) => ShakoError::QuotaExceeded {
            resource: what.to_owned(),
            limit,
        },
        Refused::Busy => ShakoError::Overloaded(format!("{what} is in use; retry after 5 seconds")),
    }
}

fn spool_io(e: std::io::Error) -> ShakoError {
    ShakoError::Storage(format!("spool: {e}"))
}

/// The Git budgets of one process.
#[derive(Clone)]
pub struct GitWork {
    spool: FairBudget<OrgId>,
    memory: FairBudget<OrgId>,
    dir: PathBuf,
}

impl GitWork {
    /// Budgets of `spool` and `memory` bytes; spool files are unlinked files in `dir`.
    #[must_use]
    pub fn new(dir: PathBuf, spool: u64, memory: u64) -> Self {
        Self {
            spool: FairBudget::new(spool),
            memory: FairBudget::new(memory),
            dir,
        }
    }

    /// An empty spool for at least `bytes`.
    pub async fn spool(&self, org: OrgId, bytes: u64) -> ShakoResult<Spool> {
        let permit = self
            .spool
            .acquire(org, bytes.max(SPOOL_MIN), SPOOL_WAIT)
            .await
            .map_err(|e| refused("spool_bytes", e))?;
        let dir = self.dir.clone();
        let file = crate::service::blocking(move || {
            std::fs::create_dir_all(&dir)
                .and_then(|()| tempfile::tempfile_in(&dir))
                .map_err(spool_io)
        })
        .await?;
        let spool = Spool {
            file,
            permit,
            capacity: self.spool.capacity(),
            limit: u64::MAX,
            at: 0,
            refusal: None,
        };
        spool.volume_has_room(spool.permit.bytes())?;
        Ok(spool)
    }

    /// Spools a request body of at most `limit` bytes; a declared length over it is refused
    /// unread.
    pub async fn receive(
        &self,
        org: OrgId,
        headers: &HeaderMap,
        body: Body,
        limit: u64,
    ) -> ShakoResult<Spool> {
        let over = || ShakoError::QuotaExceeded {
            resource: "request_body_bytes".into(),
            limit,
        };
        let declared = headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if declared > limit {
            return Err(over());
        }
        let mut spool = self.spool(org, declared).await?;
        let mut out = tokio::fs::File::from_std(spool.file.try_clone().map_err(spool_io)?);
        let deadline = tokio::time::Instant::now() + TRANSFER;
        let mut chunks = body.into_data_stream();
        let mut total = 0u64;
        loop {
            let until = deadline.min(tokio::time::Instant::now() + SEND_IDLE);
            let next = tokio::time::timeout_at(until, chunks.next())
                .await
                .map_err(|_| ShakoError::BadRequest("the request body stopped arriving".into()))?;
            let Some(chunk) = next else { break };
            let chunk =
                chunk.map_err(|e| ShakoError::BadRequest(format!("body read error: {e}")))?;
            total += chunk.len() as u64;
            if total > limit {
                return Err(over());
            }
            spool.hold(total)?;
            out.write_all(&chunk).await.map_err(spool_io)?;
        }
        out.flush().await.map_err(spool_io)?;
        Ok(spool)
    }

    /// Runs `job` on a blocking thread under a memory permit of `org`. The job names the bytes
    /// it holds through `admit` before it allocates them.
    pub async fn run<T: Send + 'static>(
        &self,
        org: OrgId,
        job: impl FnOnce(shako_git::Admit<'_>) -> ShakoResult<T> + Send + 'static,
    ) -> ShakoResult<T> {
        let first = self
            .memory
            .acquire(org, MEMORY_MIN.min(self.memory.capacity()), MEMORY_WAIT)
            .await
            .map_err(|e| refused("git_memory_bytes", e))?;
        let memory = self.memory.clone();
        let runtime = tokio::runtime::Handle::current();
        crate::service::blocking(move || {
            let mut held = Some(first);
            job(&mut |bytes| {
                // Over the whole budget means: alone.
                let bytes = bytes.min(memory.capacity());
                let Some(mut permit) = held.take() else {
                    return Err(refused("git_memory_bytes", Refused::Busy));
                };
                if permit.grow_to(bytes).is_err() {
                    // Waiting while holding could wait for itself; the waiters are bounded.
                    drop(permit);
                    permit = runtime
                        .block_on(memory.acquire(org, bytes, MEMORY_WAIT))
                        .map_err(|e| refused("git_memory_bytes", e))?;
                }
                held = Some(permit);
                Ok(())
            })
        })
        .await
    }

    /// Runs `job` like [`Self::run`] with a new spool to write its answer into.
    pub async fn produce<T: Send + 'static>(
        &self,
        org: OrgId,
        job: impl FnOnce(&mut Spool, shako_git::Admit<'_>) -> ShakoResult<T> + Send + 'static,
    ) -> ShakoResult<(Spool, T)> {
        let mut spool = self.spool(org, 0).await?;
        self.run(org, move |admit| match job(&mut spool, admit) {
            Ok(value) => Ok((spool, value)),
            // A writer under the job saw an io error; the spool knows why it refused.
            Err(e) => Err(spool.refusal.take().unwrap_or(e)),
        })
        .await
    }
}

/// An unlinked file of the repository volume with the disk permit that admits its bytes.
pub struct Spool {
    file: std::fs::File,
    permit: Permit<OrgId>,
    capacity: u64,
    limit: u64,
    at: u64,
    refusal: Option<ShakoError>,
}

impl Spool {
    #[must_use]
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }

    /// Refuses to grow past `bytes`.
    pub(crate) fn cap(&mut self, bytes: u64) {
        self.limit = bytes;
    }

    fn volume_has_room(&self, more: u64) -> ShakoResult<()> {
        let volume =
            rustix::fs::fstatvfs(&self.file).map_err(|e| spool_io(std::io::Error::from(e)))?;
        if volume.f_bavail.saturating_mul(volume.f_frsize) < more.saturating_add(VOLUME_RESERVE) {
            return Err(ShakoError::Overloaded(
                "the repository volume is nearly full".into(),
            ));
        }
        Ok(())
    }

    /// Admits a spool of `bytes`.
    fn hold(&mut self, bytes: u64) -> ShakoResult<()> {
        if bytes > self.limit {
            return Err(ShakoError::QuotaExceeded {
                resource: "spool_bytes".into(),
                limit: self.limit,
            });
        }
        let held = self.permit.bytes();
        if bytes <= held {
            return Ok(());
        }
        let step = bytes.max(held.saturating_add(SPOOL_STEP).min(self.capacity));
        self.volume_has_room(step - held)?;
        self.permit
            .grow_to(step)
            .map_err(|e| refused("spool_bytes", e))
    }

    /// The spool as a response body of known length, streamed in 64 KiB chunks; the disk permit
    /// goes back when the body is sent or its reader is gone.
    pub async fn respond(self, content_type: &'static str) -> ShakoResult<Response> {
        let Self { file, permit, .. } = self;
        let mut file = tokio::fs::File::from_std(file);
        let len = file.metadata().await.map_err(spool_io)?.len();
        tokio::io::AsyncSeekExt::rewind(&mut file)
            .await
            .map_err(spool_io)?;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        tokio::spawn(async move {
            let _held = permit;
            let deadline = tokio::time::Instant::now() + TRANSFER;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let item = match file.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => Ok(Bytes::copy_from_slice(&buf[..n])),
                    Err(e) => Err(e),
                };
                let failed = item.is_err();
                let until = deadline.min(tokio::time::Instant::now() + READ_IDLE);
                let sent = tokio::time::timeout_at(until, tx.send(item)).await;
                if failed || !matches!(sent, Ok(Ok(()))) {
                    break;
                }
            }
        });
        let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
        Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                (header::CONTENT_LENGTH, HeaderValue::from(len)),
            ],
            body,
        )
            .into_response())
    }
}

impl Write for Spool {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if let Err(e) = self.hold(self.at.saturating_add(data.len() as u64)) {
            let text = e.to_string();
            self.refusal = Some(e);
            return Err(std::io::Error::other(text));
        }
        let written = self.file.write(data)?;
        self.at += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Spool {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.at = self.file.seek(to)?;
        Ok(self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(dir: &std::path::Path, spool: u64) -> GitWork {
        GitWork::new(dir.to_path_buf(), spool, MEMORY_BYTES)
    }

    fn quota(e: &ShakoError) -> Option<(&str, u64)> {
        match e {
            ShakoError::QuotaExceeded { resource, limit } => Some((resource.as_str(), *limit)),
            _ => None,
        }
    }

    /// A body declared or sent past its limit is refused naming `request_body_bytes`; one within
    /// it is spooled and answered back with its length.
    #[tokio::test]
    async fn a_body_past_its_limit_is_refused_and_one_within_is_spooled() -> ShakoResult<()> {
        let dir = tempfile::tempdir().map_err(spool_io)?;
        let work = work(dir.path(), 64 * MIB);
        let org = OrgId::from_u128(1);
        let mut declared = HeaderMap::new();
        declared.insert(header::CONTENT_LENGTH, HeaderValue::from(11u64));
        let refused = work.receive(org, &declared, Body::from("small"), 10).await;
        assert_eq!(
            refused.err().as_ref().and_then(quota),
            Some(("request_body_bytes", 10))
        );
        let sent = work
            .receive(org, &HeaderMap::new(), Body::from(vec![7u8; 11]), 10)
            .await;
        assert_eq!(
            sent.err().as_ref().and_then(quota),
            Some(("request_body_bytes", 10))
        );
        let spool = work
            .receive(org, &HeaderMap::new(), Body::from("hello"), 10)
            .await?;
        let response = spool.respond("application/x-git").await?;
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "5");
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .map_err(|e| ShakoError::Internal(e.to_string()))?;
        assert_eq!(&bytes[..], b"hello");
        Ok(())
    }

    /// A spool larger than the whole budget is refused naming `spool_bytes`, and so is a spool
    /// grown past the cap its caller set.
    #[tokio::test]
    async fn a_spool_past_the_budget_or_its_cap_is_refused() -> ShakoResult<()> {
        let dir = tempfile::tempdir().map_err(spool_io)?;
        let work = work(dir.path(), 2 * MIB);
        let org = OrgId::from_u128(2);
        let big = work.spool(org, 3 * MIB).await;
        assert_eq!(
            big.err().as_ref().and_then(quota).map(|q| q.0),
            Some("spool_bytes")
        );
        let mut spool = work.spool(org, 0).await?;
        spool.cap(4);
        assert_eq!(
            spool.hold(5).err().as_ref().and_then(quota),
            Some(("spool_bytes", 4))
        );
        assert!(spool.hold(4).is_ok());
        let failed = spool.write(b"12345").err().map(|e| e.to_string());
        assert!(
            failed.is_some_and(|e| e.contains("spool_bytes")),
            "the write says why"
        );
        assert!(
            spool.refusal.is_some(),
            "the refusal is kept for the job's caller"
        );
        Ok(())
    }

    /// A job that writes past the spool budget answers the spool's refusal, not the io error its
    /// writer saw.
    #[tokio::test]
    async fn a_job_writing_past_the_budget_answers_the_spools_refusal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = work(dir.path(), 2 * MIB);
        let produced = work
            .produce(OrgId::from_u128(3), |spool, _admit| {
                spool
                    .write_all(&vec![0u8; 3 * MIB as usize])
                    .map_err(|e| ShakoError::Storage(e.to_string()))
            })
            .await;
        assert_eq!(
            produced.err().as_ref().and_then(quota).map(|q| q.0),
            Some("spool_bytes")
        );
    }

    /// A spool budget another spool holds answers `Busy` once the wait is over: an overload
    /// naming the budget, not a quota.
    #[tokio::test]
    async fn a_spool_budget_in_use_answers_busy_after_the_wait() -> ShakoResult<()> {
        let dir = tempfile::tempdir().map_err(spool_io)?;
        let work = work(dir.path(), 2 * MIB);
        let _held = work.spool(OrgId::from_u128(4), 2 * MIB).await?;
        let busy = work.spool(OrgId::from_u128(4), MIB).await;
        assert!(busy.as_ref().err().and_then(quota).is_none());
        assert!(matches!(&busy, Err(ShakoError::Overloaded(m)) if m.contains("spool_bytes")));
        Ok(())
    }

    /// A job that asks for more memory than is free gives back what it holds and waits; when the
    /// wait runs out it answers `Busy`, and a later ask in the same job is refused at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_job_that_cannot_grow_its_memory_answers_busy()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let work = std::sync::Arc::new(GitWork::new(dir.path().to_path_buf(), 64 * MIB, 12 * MIB));
        let (held_tx, held_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let holder = tokio::spawn({
            let work = work.clone();
            async move {
                work.run(OrgId::from_u128(5), move |admit| {
                    admit(8 * MIB)?;
                    let _ = held_tx.send(());
                    let _ = done_rx.recv();
                    Ok(())
                })
                .await
            }
        });
        held_rx.await?;
        let answers = work
            .run(OrgId::from_u128(6), |admit| {
                let first = admit(8 * MIB).err();
                let again = admit(MIB).err();
                Ok((first, again))
            })
            .await?;
        let _ = done_tx.send(());
        holder.await??;
        for answer in [answers.0, answers.1] {
            assert!(
                matches!(&answer, Some(ShakoError::Overloaded(m)) if m.contains("git_memory_bytes")),
                "{answer:?}"
            );
        }
        Ok(())
    }

    /// A spool directory that cannot be made is a storage error naming the spool.
    #[tokio::test]
    async fn a_spool_directory_that_cannot_be_made_is_a_storage_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("file");
        std::fs::write(&file, b"x")?;
        let spool = work(&file.join("spool"), 2 * MIB)
            .spool(OrgId::from_u128(7), 0)
            .await;
        assert!(matches!(&spool, Err(ShakoError::Storage(m)) if m.starts_with("spool: ")));
        Ok(())
    }

    /// A job that cannot get even its first memory waits and answers `Busy`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_job_without_its_first_memory_answers_busy() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let work =
            std::sync::Arc::new(GitWork::new(dir.path().to_path_buf(), 64 * MIB, MEMORY_MIN));
        let (held_tx, held_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let holder = tokio::spawn({
            let work = work.clone();
            async move {
                work.run(OrgId::from_u128(8), move |_admit| {
                    let _ = held_tx.send(());
                    let _ = done_rx.recv();
                    Ok(())
                })
                .await
            }
        });
        held_rx.await?;
        let ran = work.run(OrgId::from_u128(9), |_admit| Ok(())).await;
        let _ = done_tx.send(());
        holder.await??;
        assert!(matches!(&ran, Err(ShakoError::Overloaded(m)) if m.contains("git_memory_bytes")));
        Ok(())
    }
}
