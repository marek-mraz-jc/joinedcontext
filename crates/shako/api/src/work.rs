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
        let file = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            tempfile::tempfile_in(&dir)
        })
        .await
        .map_err(|e| ShakoError::Internal(e.to_string()))?
        .map_err(spool_io)?;
        let spool = Spool {
            file,
            permit,
            capacity: self.spool.capacity(),
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
        tokio::task::spawn_blocking(move || {
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
        .map_err(|e| ShakoError::Internal(e.to_string()))?
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
    at: u64,
    refusal: Option<ShakoError>,
}

impl Spool {
    #[must_use]
    pub fn file(&self) -> &std::fs::File {
        &self.file
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
