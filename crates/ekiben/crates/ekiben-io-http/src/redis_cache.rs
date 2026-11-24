//! Bento's `redis` cache (`internal/impl/redis/cache.go`), `kind: simple`: GET, SET with PX for
//! the TTL, SET NX for `add`, DEL, over one RESP2 connection that is dialled through the
//! organization's outbound rule (`redis://` plain, `rediss://` or `tls.enabled` verified TLS).
//! The password and database come from the URL, which the pipeline gives as `${NAME}` from a
//! secretRef. ekiben deviations: `cluster` and `failover` kinds and `tls.skip_cert_verify` are
//! refused; a reply bigger than 64 MiB is refused.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{
    lint_fields, Cache, ComponentConfig, CoreError, OrgContext, Registry, SecretString,
    StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::websocket::{tls_connect, Io};

const FIELDS: &[&str] = &[
    "url",
    "kind",
    "master",
    "tls",
    "prefix",
    "default_ttl",
    "retries",
];
const MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

pub fn register(registry: &Registry) {
    registry.register_cache("redis", |conf, ctx| {
        Ok(Arc::new(RedisCache::from_config(conf, ctx)?))
    });
    registry.register_lint("cache", "redis", |conf| lint_fields(conf, FIELDS, &["url"]));
}

struct Target {
    host: String,
    port: u16,
    tls: bool,
    user: Option<String>,
    password: Option<SecretString>,
    db: u32,
}

pub struct RedisCache {
    target: Target,
    prefix: String,
    default_ttl: Option<Duration>,
    org: Arc<OrgContext>,
    conn: Mutex<Option<BufReader<Box<dyn Io>>>>,
}

#[derive(Debug, PartialEq)]
enum Reply {
    Simple(String),
    Int(i64),
    Bulk(Option<Vec<u8>>),
    /// A `-ERR ...` answer: the command failed, the connection is still good.
    Error(String),
}

fn parse_target(raw: &str, tls_enabled: bool) -> Result<Target, CoreError> {
    let url = url::Url::parse(raw).map_err(|_| {
        CoreError::config("redis cache: url must look like redis://:password@host:6379/0")
    })?;
    let tls = match url.scheme() {
        "redis" => tls_enabled,
        "rediss" => true,
        other => {
            return Err(CoreError::config(format!(
                "redis cache: url scheme '{other}' is not redis:// or rediss://"
            )))
        }
    };
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| CoreError::config("redis cache: url has no host"))?
        .to_string();
    let db = match url.path().trim_start_matches('/') {
        "" => 0,
        d => d.parse().map_err(|_| {
            CoreError::config(format!("redis cache: database '{d}' is not a number"))
        })?,
    };
    let decode = |s: &str| {
        percent_decode(s).map_err(|_| {
            CoreError::config("redis cache: url credentials are not valid percent-encoding")
        })
    };
    Ok(Target {
        port: url.port().unwrap_or(6379),
        tls,
        user: match url.username() {
            "" => None,
            u => Some(decode(u)?),
        },
        password: url
            .password()
            .map(decode)
            .transpose()?
            .map(SecretString::new),
        db,
        host,
    })
}

fn percent_decode(s: &str) -> Result<String, ()> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3).ok_or(())?;
            let v =
                u8::from_str_radix(std::str::from_utf8(h).map_err(|_| ())?, 16).map_err(|_| ())?;
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn encode(args: &[&[u8]]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", args.len()).into_bytes();
    for a in args {
        out.extend_from_slice(format!("${}\r\n", a.len()).as_bytes());
        out.extend_from_slice(a);
        out.extend_from_slice(b"\r\n");
    }
    out
}

async fn read_reply<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Result<Reply, CoreError> {
    let io = |e: std::io::Error| CoreError::other(format!("redis cache: {e}"));
    let mut line = String::new();
    if r.read_line(&mut line).await.map_err(io)? == 0 {
        return Err(CoreError::other(
            "redis cache: the server closed the connection",
        ));
    }
    let line = line.trim_end_matches(['\r', '\n']);
    // The type byte is ASCII; a reply opening with a wide character has no `rest` and is refused.
    let (kind, rest) = (line.get(..1), line.get(1..).unwrap_or_default());
    match kind.unwrap_or_default() {
        "+" => Ok(Reply::Simple(rest.to_string())),
        "-" => Ok(Reply::Error(rest.to_string())),
        ":" => rest
            .parse()
            .map(Reply::Int)
            .map_err(|_| CoreError::other("redis cache: bad integer reply")),
        "$" => {
            let n: i64 = rest
                .parse()
                .map_err(|_| CoreError::other("redis cache: bad bulk length"))?;
            if n < 0 {
                return Ok(Reply::Bulk(None));
            }
            let n = n as usize;
            if n > MAX_REPLY_BYTES {
                return Err(CoreError::other(format!(
                    "redis cache: a value of {n} bytes is over the {MAX_REPLY_BYTES} ekiben reads"
                )));
            }
            let mut buf = vec![0u8; n + 2];
            r.read_exact(&mut buf).await.map_err(io)?;
            buf.truncate(n);
            Ok(Reply::Bulk(Some(buf)))
        }
        _ => Err(CoreError::other(format!(
            "redis cache: unexpected reply '{}'",
            line.chars().take(40).collect::<String>()
        ))),
    }
}

impl Reply {
    fn ok(self) -> Result<Self, CoreError> {
        match self {
            Reply::Error(e) => Err(CoreError::other(format!("redis cache: server error: {e}"))),
            r => Ok(r),
        }
    }
}

impl RedisCache {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("redis cache config must be an object"))?;
        let s = |k: &str| obj.get(k).and_then(JsonValue::as_str).unwrap_or("");
        match s("kind") {
            "" | "simple" => {}
            other => {
                return Err(CoreError::config(format!(
                    "redis cache: kind '{other}' is not supported by ekiben, use simple"
                )))
            }
        }
        let tls = obj.get("tls").and_then(JsonValue::as_object);
        if tls
            .and_then(|t| t.get("skip_cert_verify"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
        {
            return Err(CoreError::config(
                "redis cache: tls.skip_cert_verify is refused, TLS is always verified",
            ));
        }
        let tls_enabled = tls
            .and_then(|t| t.get("enabled"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let url = s("url");
        if url.is_empty() {
            return Err(CoreError::config("redis cache: field url is required"));
        }
        let default_ttl = match s("default_ttl") {
            "" => None,
            t => Some(parse_duration_str(t)?).filter(|d| !d.is_zero()),
        };
        Ok(Self {
            target: parse_target(url, tls_enabled)?,
            prefix: s("prefix").to_string(),
            default_ttl,
            org: ctx.org.clone(),
            conn: Mutex::new(None),
        })
    }

    async fn dial(&self) -> Result<BufReader<Box<dyn Io>>, CoreError> {
        let t = &self.target;
        let addrs = crate::outbound::resolve_host_checked(&t.host, t.port, &self.org).await?;
        let tcp = crate::outbound::connect_checked(&addrs, TIMEOUT).await?;
        let io: Box<dyn Io> = if t.tls {
            tls_connect(tcp, &t.host, &[]).await?
        } else {
            Box::new(tcp)
        };
        let mut conn = BufReader::new(io);
        if let Some(pw) = &t.password {
            let mut args: Vec<&[u8]> = vec![b"AUTH"];
            if let Some(u) = &t.user {
                args.push(u.as_bytes());
            }
            args.push(pw.expose().as_bytes());
            Self::exchange(&mut conn, &args).await?.ok()?;
        }
        if t.db != 0 {
            let db = t.db.to_string();
            Self::exchange(&mut conn, &[b"SELECT", db.as_bytes()])
                .await?
                .ok()?;
        }
        Ok(conn)
    }

    async fn exchange(
        conn: &mut BufReader<Box<dyn Io>>,
        args: &[&[u8]],
    ) -> Result<Reply, CoreError> {
        let run = async {
            conn.get_mut()
                .write_all(&encode(args))
                .await
                .map_err(|e| CoreError::other(format!("redis cache: {e}")))?;
            read_reply(conn).await
        };
        tokio::time::timeout(TIMEOUT, run)
            .await
            .map_err(|_| CoreError::other("redis cache: the server did not answer in 10 s"))?
    }

    /// Runs one command, dialling when there is no connection and once more when a kept one
    /// fails; a server error reply keeps the connection and is the command's error.
    async fn command(&self, args: &[&[u8]]) -> Result<Reply, CoreError> {
        let mut guard = self.conn.lock().await;
        let mut retry = guard.is_some();
        loop {
            let mut conn = match guard.take() {
                Some(c) => c,
                None => self.dial().await?,
            };
            match Self::exchange(&mut conn, args).await {
                Ok(r) => {
                    *guard = Some(conn);
                    return r.ok();
                }
                Err(_) if retry => retry = false,
                Err(e) => return Err(e),
            }
        }
    }

    fn key(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    fn ttl_ms(&self, ttl: Option<Duration>) -> Option<String> {
        ttl.or(self.default_ttl)
            .map(|d| d.as_millis().max(1).to_string())
    }
}

#[async_trait]
impl Cache for RedisCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let k = self.key(key);
        match self.command(&[b"GET", k.as_bytes()]).await? {
            Reply::Bulk(Some(v)) => Ok(Bytes::from(v)),
            Reply::Bulk(None) => Err(CoreError::KeyNotFound(key.to_string())),
            other => Err(CoreError::other(format!(
                "redis cache: GET answered {other:?}"
            ))),
        }
    }

    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let k = self.key(key);
        let ms = self.ttl_ms(ttl);
        let mut args: Vec<&[u8]> = vec![b"SET", k.as_bytes(), &value];
        if let Some(ms) = &ms {
            args.extend([b"PX".as_slice(), ms.as_bytes()]);
        }
        self.command(&args).await.map(drop)
    }

    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        let k = self.key(key);
        let ms = self.ttl_ms(ttl);
        let mut args: Vec<&[u8]> = vec![b"SET", k.as_bytes(), &value, b"NX"];
        if let Some(ms) = &ms {
            args.extend([b"PX".as_slice(), ms.as_bytes()]);
        }
        match self.command(&args).await? {
            Reply::Bulk(None) => Err(CoreError::KeyAlreadyExists(key.to_string())),
            _ => Ok(()),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        let k = self.key(key);
        self.command(&[b"DEL", k.as_bytes()]).await.map(drop)
    }
}

/// Every whole reply at the front of `buf`; the first refusal ends the read.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_replies(buf: &[u8]) -> usize {
    let mut r = BufReader::new(buf);
    let mut whole = 0;
    while futures::executor::block_on(read_reply(&mut r)).is_ok() {
        whole += 1;
    }
    whole
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_gives_host_password_and_database_and_hides_the_password() {
        let t = parse_target("redis://:p%40ss@cache.example.org:6380/2", false).unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.db, t.tls),
            ("cache.example.org", 6380, 2, false)
        );
        assert_eq!(t.password.as_ref().map(SecretString::expose), Some("p@ss"));
        assert_eq!(format!("{:?}", t.password), "Some(\"REDACTED\")");
        assert!(parse_target("rediss://h", false).unwrap().tls);
        assert!(parse_target("redis://h", true).unwrap().tls);
        assert!(parse_target("http://h", false).is_err());
        assert!(parse_target("redis://h/x", false).is_err());
    }

    #[test]
    fn commands_are_resp_arrays() {
        assert_eq!(
            encode(&[b"SET", b"k", b"v"]),
            b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$1\r\nv\r\n"
        );
    }

    #[tokio::test]
    async fn replies_are_read_and_an_oversized_value_refused() {
        let mut r = BufReader::new(&b"+OK\r\n$-1\r\n$4\r\na\r\nb\r\n:7\r\n-ERR wrong\r\n"[..]);
        assert_eq!(
            read_reply(&mut r).await.unwrap(),
            Reply::Simple("OK".into())
        );
        assert_eq!(read_reply(&mut r).await.unwrap(), Reply::Bulk(None));
        assert_eq!(
            read_reply(&mut r).await.unwrap(),
            Reply::Bulk(Some(b"a\r\nb".to_vec()))
        );
        assert_eq!(read_reply(&mut r).await.unwrap(), Reply::Int(7));
        assert_eq!(
            read_reply(&mut r).await.unwrap(),
            Reply::Error("ERR wrong".into())
        );
        let big = format!("${}\r\n", MAX_REPLY_BYTES + 1);
        let mut r = BufReader::new(big.as_bytes());
        assert!(read_reply(&mut r)
            .await
            .unwrap_err()
            .to_string()
            .contains("over the"));
    }

    use crate::test_io::{Broken, Unreadable};
    use ekiben_core::{OrgId, OrgQuotas, ResourceManager, StreamMetrics};
    use serde_json::json;

    fn ctx() -> StreamContext {
        let resources = ResourceManager::new();
        let org = resources.set_org_quotas(
            &OrgId::new("org-redis-unit"),
            OrgQuotas {
                allow_private_ips: true,
                ..Default::default()
            },
        );
        StreamContext {
            stream_id: "s".into(),
            org,
            resources,
            registry: Arc::new(Registry::new()),
            blobl_env: ekiben_bloblang::Environment::new(),
            metrics: StreamMetrics::new("s", "org-redis-unit"),
            label: String::new(),
            path: "cache_resources".into(),
            file_root: None,
            env_vars: Arc::new(std::collections::HashMap::new()),
            shutdown_rx: tokio::sync::watch::channel(false).1,
        }
    }

    fn conf(v: JsonValue) -> ComponentConfig {
        ComponentConfig {
            component_type: "redis".into(),
            config: v,
            ..Default::default()
        }
    }

    /// A RESP server answering the n-th command of each connection with `script[conn][n]`; a
    /// connection past its script closes. The commands it read, by connection.
    async fn fake(script: Vec<Vec<&'static str>>) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let rec = seen.clone();
        tokio::spawn(async move {
            for replies in script {
                let (mut s, _) = l.accept().await.expect("a connection");
                let rec = rec.clone();
                tokio::spawn(async move {
                    for reply in replies {
                        let mut buf = [0u8; 4096];
                        let got = s.read(&mut buf).await.unwrap_or(0);
                        rec.lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&buf[..got]).into_owned());
                        s.write_all(reply.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        (port, seen)
    }

    #[tokio::test]
    async fn auth_and_select_run_before_the_first_command() {
        let (port, seen) = fake(vec![vec!["+OK\r\n", "+OK\r\n", "$2\r\nhi\r\n"]]).await;
        let reg = Registry::new();
        register(&reg);
        let url = format!("redis://us%20er:p%40ss@127.0.0.1:{port}/3");
        let cache = reg
            .build_cache(&conf(json!({ "url": url })), &ctx())
            .expect("registered");
        assert_eq!(cache.get("k").await.unwrap().as_ref(), b"hi");
        let seen = seen.lock().unwrap();
        assert!(
            seen[0].contains("AUTH\r\n$5\r\nus er\r\n$4\r\np@ss"),
            "{}",
            seen[0]
        );
        assert!(seen[1].contains("SELECT\r\n$1\r\n3"), "{}", seen[1]);
    }

    #[tokio::test]
    async fn a_refused_database_names_the_server_error_and_the_lint_reads_fields() {
        let (port, _) = fake(vec![vec!["-ERR DB index is out of range\r\n"]]).await;
        let url = format!("redis://127.0.0.1:{port}/99");
        let cache = RedisCache::from_config(&conf(json!({ "url": url })), &ctx()).unwrap();
        let err = cache.get("k").await.unwrap_err().to_string();
        assert!(
            err.contains("server error: ERR DB index is out of range"),
            "{err}"
        );
        let reg = Registry::new();
        register(&reg);
        let lints = reg.lint_component("cache", &conf(json!({ "nope": 1 })));
        assert!(lints.iter().any(|l| l.contains("url")), "{lints:?}");
        assert!(lints.iter().any(|l| l.contains("nope")), "{lints:?}");
    }

    #[tokio::test]
    async fn a_refused_password_names_the_server_error() {
        let (port, _) = fake(vec![vec!["-WRONGPASS invalid\r\n"]]).await;
        let cache = RedisCache::from_config(
            &conf(json!({ "url": format!("redis://:pw@127.0.0.1:{port}") })),
            &ctx(),
        )
        .unwrap();
        let err = cache.get("k").await.unwrap_err().to_string();
        assert!(
            err.contains("redis cache: server error: WRONGPASS invalid"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_kept_connection_that_broke_is_dialled_once_more() {
        // The first connection answers once and closes; the second answers; the third reads the
        // command and closes unanswered.
        let (port, _) = fake(vec![vec!["+OK\r\n"], vec!["+OK\r\n"], vec![""]]).await;
        let cache = RedisCache::from_config(
            &conf(json!({ "url": format!("redis://127.0.0.1:{port}") })),
            &ctx(),
        )
        .unwrap();
        cache.delete("a").await.expect("first connection");
        cache
            .delete("b")
            .await
            .expect("the broken connection is replaced");
        let err = cache.delete("c").await.unwrap_err().to_string();
        assert!(err.contains("the server closed the connection"), "{err}");
    }

    #[tokio::test]
    async fn a_tls_url_needs_a_tls_server() {
        let (port, _) = fake(vec![vec!["+OK\r\n"]]).await;
        let cache = RedisCache::from_config(
            &conf(json!({ "url": format!("rediss://localhost:{port}") })),
            &ctx(),
        )
        .unwrap();
        assert!(cache.get("k").await.is_err());
    }

    #[tokio::test]
    async fn a_reply_starting_with_a_wide_character_is_refused_not_a_panic() {
        // Found by the redis_reply fuzz target: "\u{3ea}" is two bytes, so a split after the
        // first byte cut it in half.
        let mut r = tokio::io::BufReader::new("\u{3ea}!\r\n".as_bytes());
        let e = read_reply(&mut r).await.expect_err("refused");
        assert!(e.to_string().contains("unexpected reply"), "{e}");
    }

    #[tokio::test]
    async fn a_failed_read_is_named() {
        let mut r = tokio::io::BufReader::new(Unreadable);
        let e = read_reply(&mut r).await.expect_err("read error");
        assert!(e.to_string().starts_with("redis cache: "), "{e}");
    }

    #[tokio::test]
    async fn odd_replies_are_named() {
        for (raw, want) in [
            (&b""[..], "the server closed the connection"),
            (b":x\r\n", "bad integer reply"),
            (b"$x\r\n", "bad bulk length"),
            (b"?what\r\n", "unexpected reply '?what'"),
        ] {
            let err = read_reply(&mut BufReader::new(raw))
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
    }

    #[tokio::test]
    async fn a_command_that_cannot_be_written_or_is_never_answered_fails() {
        let mut broken: BufReader<Box<dyn Io>> = BufReader::new(Box::new(Broken));
        let err = RedisCache::exchange(&mut broken, &[b"PING"])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("redis cache: pipe broke"), "{err}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_server_times_out() {
        let (a, _b) = tokio::io::duplex(64);
        let mut silent: BufReader<Box<dyn Io>> = BufReader::new(Box::new(a));
        let err = RedisCache::exchange(&mut silent, &[b"PING"])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not answer in 10 s"), "{err}");
    }

    #[test]
    fn a_config_or_url_ekiben_cannot_use_is_refused_by_name() {
        let ctx = ctx();
        for (v, want) in [
            (json!("redis://h"), "redis cache config must be an object"),
            (
                json!({ "url": "redis://h", "kind": "ring" }),
                "kind 'ring' is not supported",
            ),
            (
                json!({ "url": "redis://h", "tls": { "skip_cert_verify": true } }),
                "tls.skip_cert_verify is refused",
            ),
            (json!({}), "field url is required"),
            (json!({ "url": "not a url" }), "url must look like redis://"),
            (json!({ "url": "redis:///0" }), "url has no host"),
            (
                json!({ "url": "redis://%zz:pw@h" }),
                "not valid percent-encoding",
            ),
            (json!({ "url": "redis://h", "default_ttl": "soon" }), "soon"),
        ] {
            let err = RedisCache::from_config(&conf(v.clone()), &ctx)
                .err()
                .expect("refused")
                .to_string();
            assert!(err.contains(want), "{v}: {err}");
        }
        let c = RedisCache::from_config(
            &conf(json!({ "url": "redis://h", "tls": { "enabled": true }, "default_ttl": "1m" })),
            &ctx,
        )
        .unwrap();
        assert!(c.target.tls);
        assert_eq!(c.default_ttl, Some(Duration::from_secs(60)));
        let zero = RedisCache::from_config(
            &conf(json!({ "url": "redis://h", "default_ttl": "0s" })),
            &ctx,
        )
        .unwrap();
        assert_eq!(zero.default_ttl, None);
    }
}
