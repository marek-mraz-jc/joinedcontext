use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::redact::SecretString;
use ekiben_core::{
    lint_fields, AckResponder, Batch, Cache, ComponentConfig, CoreError, Input, OrgContext,
    StreamContext,
};
use ekiben_io_file::inputs::Scanner;
use russh::client::{self, AuthResult, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::keys::{decode_secret_key, HashAlg, PrivateKeyWithHashAlg};
use russh_sftp::client::SftpSession;
use serde_json::Value as JsonValue;
use tokio::io::AsyncReadExt;

/// The largest file the input reads, and the default of `max_file_bytes`.
pub const MAX_FILE_BYTES: usize = 256 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const FIELDS: &[&str] = &[
    "address",
    "host_key_fingerprint",
    "credentials",
    "paths",
    "auto_replay_nacks",
    "codec",
    "max_buffer",
    "scanner",
    "delete_on_finish",
    "watcher",
    "max_file_bytes",
];

pub fn lint(conf: &JsonValue) -> Vec<String> {
    match Settings::parse(conf) {
        Ok(_) => Vec::new(),
        Err(e) => vec![e],
    }
}

/// Where and as whom to connect: the parts of the config a test or a tool needs to reach the
/// same server.
#[derive(Clone)]
pub struct SftpTarget {
    pub host: String,
    pub port: u16,
    /// `SHA256:<base64>`, as `ssh-keygen -l` prints it.
    pub host_key_fingerprint: String,
    pub username: String,
    pub password: Option<SecretString>,
    pub private_key: Option<SecretString>,
    pub private_key_pass: Option<SecretString>,
}

struct Watcher {
    minimum_age: Duration,
    poll_interval: Duration,
    cache: String,
}

struct Settings {
    target: SftpTarget,
    /// `(directory, file-name pattern)` per entry of `paths`.
    paths: Vec<(String, glob::Pattern)>,
    scanner: Scanner,
    delete_on_finish: bool,
    watcher: Option<Watcher>,
    max_file_bytes: usize,
}

fn text(obj: &serde_json::Map<String, JsonValue>, k: &str) -> String {
    obj.get(k)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}

fn secret(obj: &serde_json::Map<String, JsonValue>, k: &str) -> Option<SecretString> {
    Some(text(obj, k))
        .filter(|s| !s.is_empty())
        .map(SecretString::new)
}

fn duration(
    obj: &serde_json::Map<String, JsonValue>,
    k: &str,
    default: &str,
) -> Result<Duration, String> {
    let raw = obj.get(k).and_then(JsonValue::as_str).unwrap_or(default);
    parse_duration_str(raw)
        .map_err(|_| format!("sftp input: watcher.{k} must be a duration, e.g. {default}"))
}

impl Settings {
    fn parse(conf: &JsonValue) -> Result<Self, String> {
        let errs = lint_fields(
            conf,
            FIELDS,
            &["address", "host_key_fingerprint", "credentials", "paths"],
        );
        if !errs.is_empty() {
            return Err(format!("sftp input: {}", errs.join("; ")));
        }
        let obj = conf.as_object().ok_or("sftp input: expected an object")?;
        let address = text(obj, "address");
        let (host, port) = match address.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() && !p.contains(']') => (
                h.trim_start_matches('[').trim_end_matches(']').to_string(),
                p.parse::<u16>().map_err(|_| {
                    "sftp input: address must be host:port, e.g. sftp.example.org:22".to_string()
                })?,
            ),
            _ if !address.is_empty() => (address.clone(), 22),
            _ => {
                return Err(
                    "sftp input: address must be host:port, e.g. sftp.example.org:22".into(),
                )
            }
        };
        let fp = text(obj, "host_key_fingerprint");
        if !fp.starts_with("SHA256:") || fp.len() < 20 {
            return Err("sftp input: host_key_fingerprint must pin the server's host key as SHA256:..., the fingerprint `ssh-keyscan -p 22 host | ssh-keygen -lf -` prints".into());
        }
        let creds = obj
            .get("credentials")
            .and_then(JsonValue::as_object)
            .ok_or(
            "sftp input: credentials must be an object with username and password or private_key",
        )?;
        for k in creds.keys() {
            if ![
                "username",
                "password",
                "private_key",
                "private_key_file",
                "private_key_pass",
            ]
            .contains(&k.as_str())
            {
                return Err(format!(
                    "sftp input: credentials.{k} is not a field of credentials"
                ));
            }
        }
        if !text(creds, "private_key_file").is_empty() {
            return Err("sftp input: credentials.private_key_file is not supported by ekiben; give the key's PEM in credentials.private_key from a secret".into());
        }
        let username = text(creds, "username");
        let password = secret(creds, "password");
        let private_key = secret(creds, "private_key");
        if username.is_empty() || (password.is_none() && private_key.is_none()) {
            return Err("sftp input: credentials needs a username and a password or private_key, e.g. password: ${SFTP_PASSWORD}".into());
        }
        if let Some(k) = &private_key {
            decode_secret_key(k.expose(), creds.get("private_key_pass").and_then(JsonValue::as_str).filter(|s| !s.is_empty()))
                .map_err(|_| "sftp input: credentials.private_key is not an OpenSSH Ed25519 or ECDSA private key, or private_key_pass does not open it".to_string())?;
        }
        if obj.get("codec").is_some_and(|c| !c.is_null()) {
            return Err("sftp input: codec is not supported by ekiben; use scanner, e.g. scanner: { lines: {} }".into());
        }
        let mut paths = Vec::new();
        for (i, p) in obj
            .get("paths")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let p = p.as_str().unwrap_or("");
            let Some((dir, name)) = p.rsplit_once('/').filter(|_| p.starts_with('/')) else {
                return Err(format!(
                    "sftp input: paths[{i}] must be an absolute path, e.g. /upload/*.csv"
                ));
            };
            if dir.contains(['*', '?', '[']) || dir.split('/').any(|s| s == "..") || name.is_empty()
            {
                return Err(format!("sftp input: paths[{i}]: a glob is matched in the file name only, e.g. /upload/*.csv"));
            }
            let pattern = glob::Pattern::new(name)
                .map_err(|e| format!("sftp input: paths[{i}] is not a valid glob: {e}"))?;
            paths.push((
                if dir.is_empty() {
                    "/".to_string()
                } else {
                    dir.to_string()
                },
                pattern,
            ));
        }
        if paths.is_empty() {
            return Err("sftp input: paths needs at least one path, e.g. /upload/*.csv".into());
        }
        let watcher = match obj.get("watcher").and_then(JsonValue::as_object) {
            Some(w)
                if w.get("enabled")
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false) =>
            {
                let cache = text(w, "cache");
                if cache.is_empty() {
                    return Err("sftp input: watcher.cache must name a cache resource that remembers the files already read, e.g. sftp_seen".into());
                }
                let poll_interval = duration(w, "poll_interval", "1s")?;
                if poll_interval < Duration::from_millis(100) {
                    return Err(
                        "sftp input: watcher.poll_interval must be at least 100ms, e.g. 30s".into(),
                    );
                }
                Some(Watcher {
                    minimum_age: duration(w, "minimum_age", "1s")?,
                    poll_interval,
                    cache,
                })
            }
            _ => None,
        };
        let max_file_bytes = match obj.get("max_file_bytes") {
            None => MAX_FILE_BYTES,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=MAX_FILE_BYTES as u64).contains(n))
                .ok_or_else(|| {
                    format!(
                        "sftp input: max_file_bytes must be 1 to {MAX_FILE_BYTES}, e.g. 10485760"
                    )
                })? as usize,
        };
        Ok(Self {
            target: SftpTarget {
                host,
                port,
                host_key_fingerprint: fp,
                username,
                password,
                private_key,
                private_key_pass: secret(creds, "private_key_pass"),
            },
            paths,
            scanner: Scanner::from_config(obj).map_err(|e| format!("sftp input: {e}"))?,
            delete_on_finish: obj
                .get("delete_on_finish")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            watcher,
            max_file_bytes,
        })
    }
}

/// Accepts the server only when its host key has the pinned fingerprint.
struct Pinned {
    fingerprint: String,
    /// The fingerprint the server presented, for the error when it does not match.
    seen: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for Pinned {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let data = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.key_data(),
            PublicKeyOrCertificate::Certificate(c) => c.public_key(),
        };
        let seen = data.fingerprint(HashAlg::Sha256).to_string();
        let ok = seen == self.fingerprint;
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(seen);
        }
        Ok(ok)
    }
}

/// An open SFTP session; the SSH connection lives as long as it does.
pub struct Session {
    pub sftp: SftpSession,
    _ssh: Handle<Pinned>,
}

/// Connects to `target` through `org`'s outbound rule, checks the pinned host key, logs in and
/// opens the SFTP subsystem.
pub async fn connect(target: &SftpTarget, org: &OrgContext) -> Result<Session, CoreError> {
    let failed = |what: &str, e: &dyn std::fmt::Display| {
        CoreError::other(format!("sftp input: {what}: {e}"))
    };
    let addrs =
        ekiben_io_http::outbound::resolve_host_checked(&target.host, target.port, org).await?;
    let tcp = ekiben_io_http::outbound::connect_checked(&addrs, CONNECT_TIMEOUT).await?;
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(300)),
        keepalive_interval: Some(Duration::from_secs(30)),
        ..client::Config::default()
    });
    let seen = Arc::new(std::sync::Mutex::new(None));
    let handler = Pinned {
        fingerprint: target.host_key_fingerprint.clone(),
        seen: seen.clone(),
    };
    let mut ssh = tokio::time::timeout(CONNECT_TIMEOUT, client::connect_stream(config, tcp, handler))
        .await
        .map_err(|_| CoreError::other("sftp input: the server did not complete the SSH handshake in 15 s"))?
        .map_err(|e| match e {
            russh::Error::UnknownKey => CoreError::config(format!(
                "sftp input: the server presented host key {}, which does not match host_key_fingerprint {}; confirm the server's fingerprint with its operator before changing the pin",
                seen.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| "(none)".into()),
                target.host_key_fingerprint
            )),
            e => failed("ssh handshake", &e),
        })?;
    let auth = match (&target.password, &target.private_key) {
        (Some(p), _) => ssh
            .authenticate_password(target.username.clone(), p.expose())
            .await
            .map_err(|e| failed("login", &e))?,
        (None, Some(k)) => {
            let key = decode_secret_key(
                k.expose(),
                target.private_key_pass.as_ref().map(SecretString::expose),
            )
            .map_err(|_| {
                CoreError::config("sftp input: credentials.private_key does not decode")
            })?;
            ssh.authenticate_publickey(
                target.username.clone(),
                PrivateKeyWithHashAlg::new(Arc::new(key), None),
            )
            .await
            .map_err(|e| failed("login", &e))?
        }
        (None, None) => return Err(CoreError::config("sftp input: no password or private_key")),
    };
    if !matches!(auth, AuthResult::Success) {
        return Err(CoreError::config(format!(
            "sftp input: the server refused the credentials of user {}",
            target.username
        )));
    }
    let channel = ssh
        .channel_open_session()
        .await
        .map_err(|e| failed("session", &e))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| failed("sftp subsystem", &e))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| failed("sftp", &e))?;
    Ok(Session { sftp, _ssh: ssh })
}

pub struct SftpInput {
    s: Settings,
    session: Option<Arc<Session>>,
    queue: VecDeque<String>,
    listed: bool,
    cache: Option<Arc<dyn Cache>>,
    backoff: Duration,
    label: String,
}

impl SftpInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let s = Settings::parse(&conf.config).map_err(CoreError::config)?;
        let cache = match &s.watcher {
            Some(w) => Some(ctx.resources.get_cache(&ctx.org.org_id, &w.cache)?),
            None => None,
        };
        Ok(Self {
            s,
            session: None,
            queue: VecDeque::new(),
            listed: false,
            cache,
            backoff: Duration::from_millis(100),
            label: conf.label.clone(),
        })
    }

    /// The files `paths` names now, in name order; with a watcher, only those older than
    /// `minimum_age` and not in its cache.
    async fn list(&self, sftp: &SftpSession) -> Result<Vec<String>, CoreError> {
        let mut found = Vec::new();
        let now = SystemTime::now();
        for (dir, pattern) in &self.s.paths {
            let entries = sftp
                .read_dir(dir.clone())
                .await
                .map_err(|e| CoreError::other(format!("sftp input: listing {dir}: {e}")))?;
            for entry in entries {
                let name = entry.file_name();
                let meta = entry.metadata();
                if !pattern.matches(&name) || !entry.file_type().is_file() {
                    continue;
                }
                let path = format!("{}/{name}", dir.trim_end_matches('/'));
                if meta.len() > self.s.max_file_bytes as u64 {
                    tracing::warn!(
                        "sftp input: {path} ({} bytes) is larger than max_file_bytes ({}) and is skipped",
                        meta.len(),
                        self.s.max_file_bytes
                    );
                    continue;
                }
                if let Some(w) = &self.s.watcher {
                    let old_enough = meta
                        .modified()
                        .ok()
                        .and_then(|m| now.duration_since(m).ok())
                        .is_some_and(|age| age >= w.minimum_age);
                    let seen = match &self.cache {
                        Some(c) => c.exists(&path).await?,
                        None => false,
                    };
                    if !old_enough || seen {
                        continue;
                    }
                }
                found.push(path);
            }
        }
        found.sort();
        found.dedup();
        Ok(found)
    }

    async fn read_file(&self, sftp: &SftpSession, path: &str) -> Result<Vec<u8>, CoreError> {
        let io = |e: &dyn std::fmt::Display| {
            CoreError::other(format!("sftp input: reading {path}: {e}"))
        };
        let file = sftp.open(path.to_string()).await.map_err(|e| io(&e))?;
        let mut body = Vec::new();
        file.take(self.s.max_file_bytes as u64 + 1)
            .read_to_end(&mut body)
            .await
            .map_err(|e| io(&e))?;
        if body.len() > self.s.max_file_bytes {
            return Err(CoreError::other(format!(
                "sftp input: {path} is larger than max_file_bytes ({} bytes)",
                self.s.max_file_bytes
            )));
        }
        Ok(body)
    }

    fn ack_for(&self, session: &Arc<Session>, path: String) -> AckResponder {
        if !self.s.delete_on_finish && self.cache.is_none() {
            return AckResponder::noop();
        }
        let (session, cache, delete) =
            (session.clone(), self.cache.clone(), self.s.delete_on_finish);
        AckResponder::from_fn(move |err| {
            let Ok(rt) = tokio::runtime::Handle::try_current() else {
                return;
            };
            rt.spawn(async move {
                match (err, cache) {
                    // A nacked file leaves the cache, so the next poll reads it again.
                    (Some(_), Some(c)) => {
                        if let Err(e) = c.delete(&path).await {
                            tracing::warn!("sftp input: forgetting {path} after a nack: {e}");
                        }
                    }
                    (None, _) if delete => {
                        if let Err(e) = session.sftp.remove_file(path.clone()).await {
                            tracing::warn!("sftp input: deleting {path} after ack failed: {e}");
                        }
                    }
                    _ => {}
                }
            });
        })
    }

    async fn ensure_session(&mut self, ctx: &StreamContext) -> Result<Arc<Session>, CoreError> {
        loop {
            if let Some(s) = &self.session {
                return Ok(s.clone());
            }
            match connect(&self.s.target, &ctx.org).await {
                Ok(s) => {
                    self.session = Some(Arc::new(s));
                    self.backoff = Duration::from_millis(100);
                }
                // A one-off listing fails at once; a watcher keeps dialling.
                Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => return Err(e),
                Err(e) if self.s.watcher.is_none() => return Err(e),
                Err(e) => {
                    tracing::warn!(
                        "sftp input: connect failed, retrying in {:?}: {e}",
                        self.backoff
                    );
                    tokio::time::sleep(self.backoff).await;
                    self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

#[async_trait]
impl Input for SftpInput {
    fn input_type(&self) -> &'static str {
        "sftp"
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            let session = self.ensure_session(ctx).await?;
            let Some(path) = self.queue.pop_front() else {
                match &self.s.watcher {
                    None if self.listed => return Err(CoreError::EndOfInput),
                    Some(w) if self.listed => {
                        tokio::time::sleep(w.poll_interval).await;
                        if ctx.is_shutting_down() {
                            return Err(CoreError::EndOfInput);
                        }
                    }
                    _ => {}
                }
                match self.list(&session.sftp).await {
                    Ok(found) => self.queue.extend(found),
                    Err(e) if self.s.watcher.is_some() => {
                        tracing::warn!("sftp input: {e}; reconnecting");
                        self.session = None;
                    }
                    Err(e) => return Err(e),
                }
                self.listed = true;
                continue;
            };
            if let Some(c) = &self.cache {
                c.set(&path, Vec::new().into(), None).await?;
            }
            let body = match self.read_file(&session.sftp, &path).await {
                Ok(b) => b,
                Err(e) => {
                    if let Some(c) = &self.cache {
                        let _ = c.delete(&path).await;
                    }
                    if self.s.watcher.is_none() {
                        return Err(e);
                    }
                    // A resident watcher skips the file this poll and reads it on a later one.
                    tracing::warn!("sftp input: {e}; reconnecting");
                    self.session = None;
                    continue;
                }
            };
            let mut parts = self.s.scanner.scan(&body, &path)?;
            if parts.is_empty() {
                if self.s.delete_on_finish {
                    let _ = session.sftp.remove_file(path.clone()).await;
                }
                continue;
            }
            for p in &mut parts {
                p.meta_set_str("sftp_path", path.clone());
            }
            let ack = self.ack_for(&session, path);
            return Ok((parts, ack));
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Some(s) = self.session.take() {
            let _ = s.sftp.close().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
