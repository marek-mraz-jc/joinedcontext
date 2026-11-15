//! `ftp` input (ekiben extension, T-7087): the files under one directory of an FTP server whose
//! names match a glob, one message per file with metadata `ftp_path` and `ftp_size`, read once per
//! run. Explicit FTPS (`AUTH TLS`, data channel protected) unless `tls: none`, which only an
//! organization with the `allow_plain_ftp` quota may use: plain FTP sends the password in the
//! clear. A file is deleted (`delete_on_finish`) or renamed to `<name><processed_suffix>` only
//! after the pipeline acknowledged it; renamed files are skipped by later runs.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{AckResponder, Batch, ComponentConfig, CoreError, Input, Part, StreamContext};
use ekiben_io_http::outbound::{connect_checked, resolve_host_checked};
use serde_json::{Map, Value as JsonValue};
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode};
use tokio::io::AsyncReadExt;

use crate::acks::{Acks, ACK_WAIT};
use crate::tls;

/// The largest file read when `max_file_bytes` is not set.
const DEFAULT_MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The most `max_file_bytes` may be.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// The most files one run reads when `max_files` is not set, and the most it may be.
const DEFAULT_MAX_FILES: usize = 100;
const MAX_FILES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tls {
    Explicit,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AfterAck {
    Keep,
    Delete,
    Rename(String),
}

pub struct FtpInput {
    host: String,
    port: u16,
    tls: Tls,
    root_cas: Option<String>,
    username: String,
    password: String,
    dir: String,
    pattern: glob::Pattern,
    after: AfterAck,
    max_file_bytes: u64,
    max_files: usize,
    timeout: Duration,
    stream: Option<AsyncRustlsFtpStream>,
    pending: VecDeque<String>,
    acks: Acks<String>,
    listed: bool,
}

fn field<'a>(obj: &'a Map<String, JsonValue>, name: &str) -> Option<&'a str> {
    obj.get(name)
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
}

/// `host` or `host:port` (`[v6]:port`), with the FTP port when none is named.
fn host_port(address: &str) -> Result<(String, u16), CoreError> {
    let bad = || {
        CoreError::config(format!(
            "ftp address '{address}' is host or host:port, such as files.example.org:21"
        ))
    };
    if address.contains(['/', '@', ' ']) {
        return Err(bad());
    }
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') || host.ends_with(']') => {
            (host, port.parse::<u16>().map_err(|_| bad())?)
        }
        _ => (address, 21),
    };
    if host.is_empty() || port == 0 {
        return Err(bad());
    }
    Ok((host.to_string(), port))
}

/// An absolute directory and a file-name glob: `/exports/*.csv` lists `/exports` and keeps the
/// names matching `*.csv`. The directory holds no glob and no `..`.
fn split_path(path: &str) -> Result<(String, glob::Pattern), CoreError> {
    let bad = |why: &str| CoreError::config(format!("ftp path '{path}': {why}"));
    if !path.starts_with('/') {
        return Err(bad("an absolute path such as /exports/*.csv"));
    }
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    if dir.split('/').any(|seg| seg == ".." || seg == ".")
        || dir.contains(['*', '?', '[', ']'])
        || path.chars().any(char::is_control)
    {
        return Err(bad(
            "only the file name may hold a glob, and no segment is . or ..",
        ));
    }
    if name.is_empty() {
        return Err(bad("name the files, such as /exports/*.csv"));
    }
    let pattern = glob::Pattern::new(name).map_err(|e| bad(&e.to_string()))?;
    Ok((
        if dir.is_empty() {
            "/".into()
        } else {
            dir.into()
        },
        pattern,
    ))
}

impl FtpInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("ftp input config must be an object"))?;
        let known = [
            "address",
            "tls",
            "root_cas",
            "username",
            "password",
            "path",
            "delete_on_finish",
            "processed_suffix",
            "max_file_bytes",
            "max_files",
            "timeout",
        ];
        if let Some(unknown) = obj.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(CoreError::config(format!(
                "ftp input has no field '{unknown}'; it takes {}",
                known.join(", ")
            )));
        }
        let address = field(obj, "address").ok_or_else(|| {
            CoreError::config("ftp input requires 'address', such as files.example.org:21")
        })?;
        let (host, port) = host_port(address)?;
        let tls = match field(obj, "tls").unwrap_or("explicit") {
            "explicit" => Tls::Explicit,
            "none" => Tls::None,
            other => {
                return Err(CoreError::config(format!(
                    "ftp tls '{other}' is explicit (FTPS, the default) or none"
                )))
            }
        };
        if tls == Tls::None && !ctx.org.get_quotas().allow_plain_ftp {
            return Err(CoreError::AccessDenied {
                org: ctx.org.org_id.0.clone(),
                reason: "ftp tls: none sends the password in the clear; it needs the organization's allow_plain_ftp quota, otherwise use FTPS".into(),
            });
        }
        let (dir, pattern) = split_path(field(obj, "path").ok_or_else(|| {
            CoreError::config("ftp input requires 'path', such as /exports/*.csv")
        })?)?;
        let delete = obj
            .get("delete_on_finish")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let after = match (delete, field(obj, "processed_suffix")) {
            (true, Some(_)) => {
                return Err(CoreError::config(
                    "ftp input takes delete_on_finish or processed_suffix, not both",
                ))
            }
            (true, None) => AfterAck::Delete,
            (false, Some(suffix)) => {
                if suffix.contains(['/', '*', '?', '[', ']'])
                    || suffix.chars().any(char::is_control)
                {
                    return Err(CoreError::config(format!(
                        "ftp processed_suffix '{suffix}' is a plain file-name ending such as .done"
                    )));
                }
                AfterAck::Rename(suffix.to_string())
            }
            (false, None) => AfterAck::Keep,
        };
        let max_file_bytes = match obj.get("max_file_bytes") {
            None => DEFAULT_MAX_FILE_BYTES,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=MAX_FILE_BYTES).contains(n))
                .ok_or_else(|| {
                    CoreError::config(format!("ftp max_file_bytes is 1 to {MAX_FILE_BYTES}"))
                })?,
        };
        let max_files = match obj.get("max_files") {
            None => DEFAULT_MAX_FILES,
            Some(v) => v
                .as_u64()
                .map(|n| n as usize)
                .filter(|n| (1..=MAX_FILES).contains(n))
                .ok_or_else(|| CoreError::config(format!("ftp max_files is 1 to {MAX_FILES}")))?,
        };
        let timeout = match field(obj, "timeout") {
            None => Duration::from_secs(30),
            Some(s) => parse_duration_str(s)
                .ok()
                .filter(|d| !d.is_zero() && *d <= Duration::from_secs(300))
                .ok_or_else(|| {
                    CoreError::config(format!(
                        "ftp timeout '{s}' is a duration up to 5m, such as 30s"
                    ))
                })?,
        };
        Ok(Self {
            host,
            port,
            tls,
            root_cas: field(obj, "root_cas").map(str::to_owned),
            username: field(obj, "username").unwrap_or("anonymous").to_string(),
            password: obj
                .get("password")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string(),
            dir,
            pattern,
            after,
            max_file_bytes,
            max_files,
            timeout,
            stream: None,
            pending: VecDeque::new(),
            acks: Acks::new(),
            listed: false,
        })
    }

    fn failed(&self, what: &str, e: FtpError) -> CoreError {
        CoreError::other(format!("ftp {}:{}: {what}: {e}", self.host, self.port))
    }

    async fn open(&self, ctx: &StreamContext) -> Result<AsyncRustlsFtpStream, CoreError> {
        let addrs = resolve_host_checked(&self.host, self.port, &ctx.org).await?;
        let tcp = connect_checked(&addrs, self.timeout).await?;
        let peer = tcp
            .peer_addr()
            .map_err(|e| CoreError::other(format!("ftp {}: {e}", self.host)))?;
        let stream =
            tokio::time::timeout(self.timeout, AsyncRustlsFtpStream::connect_with_stream(tcp))
                .await
                .map_err(|_| {
                    CoreError::other(format!(
                        "ftp {}: no greeting within {:?}",
                        self.host, self.timeout
                    ))
                })?
                .map_err(|e| self.failed("greeting", e))?;
        // A passive data connection goes only to the control connection's own peer: the address
        // a PASV reply names is the server's word, and a hostile one names an internal host.
        let timeout = self.timeout;
        let mut stream = stream.passive_stream_builder(move |offered: SocketAddr| {
            Box::pin(async move {
                let target = SocketAddr::new(peer.ip(), offered.port());
                tokio::time::timeout(timeout, tokio::net::TcpStream::connect(target))
                    .await
                    .map_err(|_| FtpError::ConnectionError(std::io::ErrorKind::TimedOut.into()))?
                    .map_err(FtpError::ConnectionError)
            })
        });
        stream.set_mode(Mode::Passive);
        stream.set_passive_nat_workaround(true);
        if self.tls == Tls::Explicit {
            let connector =
                tokio_rustls::TlsConnector::from(tls::client_config(self.root_cas.as_deref())?);
            let name = self
                .host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            tls::server_name(&name)?;
            stream = stream
                .into_secure(AsyncRustlsConnector::from(connector), &name)
                .await
                .map_err(|e| self.failed("AUTH TLS", e))?;
        }
        stream
            .login(self.username.as_str(), self.password.as_str())
            .await
            .map_err(|e| self.failed(&format!("login as '{}'", self.username), e))?;
        stream
            .transfer_type(FileType::Binary)
            .await
            .map_err(|e| self.failed("TYPE I", e))?;
        Ok(stream)
    }

    fn path_of(&self, name: &str) -> String {
        if self.dir == "/" {
            format!("/{name}")
        } else {
            format!("{}/{name}", self.dir)
        }
    }

    /// The files of the directory to read this run, by name, at most `max_files`.
    async fn list(&mut self) -> Result<(), CoreError> {
        let dir = self.dir.clone();
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| CoreError::other("ftp: not connected"))?;
        let names = stream.nlst(Some(&dir)).await;
        let names = names.map_err(|e| self.failed(&format!("NLST {dir}"), e))?;
        let mut names: Vec<String> = names
            .into_iter()
            // NLST answers bare names or paths, by server.
            .map(|n| {
                n.rsplit('/')
                    .next()
                    .unwrap_or(&n)
                    .trim_end_matches('\r')
                    .to_string()
            })
            .filter(|n| !n.is_empty() && n != "." && n != "..")
            .filter(|n| self.pattern.matches(n))
            .filter(|n| match &self.after {
                AfterAck::Rename(suffix) => !n.ends_with(suffix.as_str()),
                _ => true,
            })
            .collect();
        names.sort();
        names.dedup();
        names.truncate(self.max_files);
        self.pending = names.into();
        self.listed = true;
        Ok(())
    }

    async fn read_file(&mut self, name: &str) -> Result<Vec<u8>, CoreError> {
        let path = self.path_of(name);
        let cap = self.max_file_bytes;
        let timeout = self.timeout;
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| CoreError::other("ftp: not connected"))?;
        let mut transfer = stream
            .retr_as_stream(&path)
            .await
            .map_err(|e| CoreError::other(format!("ftp RETR {path}: {e}")))?;
        let mut body = Vec::new();
        let read = tokio::time::timeout(
            timeout,
            (&mut transfer).take(cap + 1).read_to_end(&mut body),
        )
        .await;
        match read {
            Err(_) => {
                return Err(CoreError::other(format!(
                    "ftp RETR {path}: no data within {timeout:?}"
                )))
            }
            Ok(Err(e)) => return Err(CoreError::other(format!("ftp RETR {path}: {e}"))),
            Ok(Ok(_)) => {}
        }
        if body.len() as u64 > cap {
            // The server still sends; closing the data connection ends the transfer.
            let _ = stream.abort(transfer).await;
            return Err(CoreError::other(format!(
                "ftp {path} is larger than max_file_bytes ({cap} bytes)"
            )));
        }
        transfer
            .finish()
            .await
            .map_err(|e| CoreError::other(format!("ftp RETR {path}: {e}")))?;
        Ok(body)
    }

    /// Deletes or renames what the pipeline acknowledged.
    async fn settle(&mut self, names: Vec<String>) -> Result<(), CoreError> {
        let after = self.after.clone();
        if after == AfterAck::Keep || names.is_empty() {
            return Ok(());
        }
        for name in names {
            let path = self.path_of(&name);
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| CoreError::other("ftp: not connected"))?;
            let done = match &after {
                AfterAck::Delete => stream.rm(&path).await,
                AfterAck::Rename(suffix) => stream.rename(&path, &format!("{path}{suffix}")).await,
                AfterAck::Keep => Ok(()),
            };
            if let Err(e) = done {
                // The file stays and is read again next run: a duplicate rather than a loss.
                tracing::warn!(path = %path, error = %e, "ftp: an acknowledged file was not removed");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Input for FtpInput {
    fn input_type(&self) -> &'static str {
        "ftp"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.stream.is_none() {
            self.stream = Some(self.open(ctx).await?);
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        self.connect(ctx).await?;
        if !self.listed {
            self.list().await?;
        }
        let ready = self.acks.ready();
        self.settle(ready).await?;
        let Some(name) = self.pending.pop_front() else {
            let rest = self.acks.all(ACK_WAIT).await;
            self.settle(rest).await?;
            return Err(CoreError::EndOfInput);
        };
        let body = self.read_file(&name).await?;
        let mut part = Part::new(body.clone());
        part.meta_set_str("ftp_path", self.path_of(&name));
        part.meta_set_str("ftp_size", body.len().to_string());
        let ack = self.acks.responder(name);
        Ok((vec![part], ack))
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.quit().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_and_a_path_are_read_strictly() {
        assert_eq!(
            host_port("files.example.org").unwrap(),
            ("files.example.org".into(), 21)
        );
        assert_eq!(
            host_port("files.example.org:2121").unwrap(),
            ("files.example.org".into(), 2121)
        );
        assert_eq!(
            host_port("[2001:db8::1]:21").unwrap(),
            ("[2001:db8::1]".into(), 21)
        );
        for bad in ["", "ftp://x", "user@x", "x:0", "x:port", "a b"] {
            assert!(host_port(bad).is_err(), "{bad}");
        }
        let (dir, pattern) = split_path("/exports/daily/*.csv").unwrap();
        assert_eq!(dir, "/exports/daily");
        assert!(pattern.matches("2026-11-15.csv") && !pattern.matches("a.xlsx"));
        assert_eq!(split_path("/*.xlsx").unwrap().0, "/");
        for bad in [
            "exports/*.csv",
            "/exports/../etc/*",
            "/ex*/a.csv",
            "/exports/",
            "/a/\u{7}.csv",
        ] {
            assert!(split_path(bad).is_err(), "{bad}");
        }
    }
}
