//! `imap` input (ekiben extension, T-7087): the attachments of the unread mails in one mailbox,
//! over IMAPS only, one message per attachment with metadata `email_from`, `email_subject`,
//! `email_date` (RFC 3339) and `attachment_filename`, read once per run. A mail is filtered by its
//! sender (`from`, an exact address, also searched on the server), its subject (`subject`, a
//! regular expression) and the attachment's file name (`attachment`, a glob). Mails are fetched
//! with `BODY.PEEK[]`, so nothing is marked read by reading: a mail is flagged `\Seen`, or moved
//! to `move_to`, only once the pipeline acknowledged every attachment it gave. A mail no filter
//! keeps, or with no attachment that matches, is left as it was.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use async_imap::{Client, Session};
use async_trait::async_trait;
use ekiben_core::config::parse_duration_str;
use ekiben_core::{AckResponder, Batch, ComponentConfig, CoreError, Input, Part, StreamContext};
use ekiben_io_http::outbound::{connect_checked, resolve_host_checked};
use futures::TryStreamExt;
use mail_parser::MessageParser;
use serde_json::{Map, Value as JsonValue};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::acks::{Acks, ACK_WAIT};
use crate::tls;

/// The largest attachment emitted when `max_attachment_bytes` is not set, and the most it may be.
const DEFAULT_MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 256 * 1024 * 1024;
/// What a mail may hold beside its attachment's encoded bytes and still be fetched.
const MAX_MAIL_OVERHEAD: u64 = 2 * 1024 * 1024;
/// The most mails one run reads when `max_messages` is not set, and the most it may be.
const DEFAULT_MAX_MESSAGES: usize = 50;
const MAX_MESSAGES: usize = 1000;

type ImapSession = Session<TlsStream<TcpStream>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum AfterAck {
    Seen,
    Move(String),
}

/// One attachment to emit, and the mail it belongs to.
struct Attachment {
    uid: u32,
    part: Part,
}

pub struct ImapInput {
    host: String,
    port: u16,
    root_cas: Option<String>,
    username: String,
    password: String,
    mailbox: String,
    from: Option<String>,
    subject: Option<regex::Regex>,
    attachment: glob::Pattern,
    after: AfterAck,
    max_attachment_bytes: u64,
    max_messages: usize,
    timeout: Duration,
    session: Option<ImapSession>,
    uids: VecDeque<u32>,
    ready: VecDeque<Attachment>,
    /// Per mail: the attachments given and those acknowledged so far.
    given: BTreeMap<u32, (usize, usize)>,
    acks: Acks<u32>,
    searched: bool,
}

fn field<'a>(obj: &'a Map<String, JsonValue>, name: &str) -> Option<&'a str> {
    obj.get(name)
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
}

/// A mailbox name: printable, at most 200 characters (async-imap quotes it on the wire).
fn mailbox_name(name: &str, field: &str) -> Result<String, CoreError> {
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(CoreError::config(format!(
            "imap {field} '{name}' is a mailbox name of 1 to 200 printable characters, such as INBOX/Processed"
        )));
    }
    Ok(name.to_string())
}

/// A sender address safe to put in a SEARCH: no quote, space, backslash or control character.
fn sender(from: &str) -> Result<String, CoreError> {
    let ok = from.len() <= 254
        && from
            .split_once('@')
            .is_some_and(|(l, d)| !l.is_empty() && !d.is_empty())
        && from
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@.!#$%&'*+/=?^_`{|}~-".contains(c));
    if ok {
        Ok(from.to_ascii_lowercase())
    } else {
        Err(CoreError::config(format!(
            "imap from '{from}' is one e-mail address, such as exports@stat.example.org"
        )))
    }
}

impl ImapInput {
    pub fn from_config(conf: &ComponentConfig, _ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("imap input config must be an object"))?;
        let known = [
            "address",
            "root_cas",
            "username",
            "password",
            "mailbox",
            "from",
            "subject",
            "attachment",
            "after_ack",
            "move_to",
            "max_attachment_bytes",
            "max_messages",
            "timeout",
        ];
        if let Some(unknown) = obj.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(CoreError::config(format!(
                "imap input has no field '{unknown}'; it takes {}",
                known.join(", ")
            )));
        }
        let address = field(obj, "address").ok_or_else(|| {
            CoreError::config("imap input requires 'address', such as imap.example.org:993")
        })?;
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') || host.ends_with(']') => (
                host.to_string(),
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(|| {
                        CoreError::config(format!(
                            "imap address '{address}': the port is 1 to 65535"
                        ))
                    })?,
            ),
            _ => (address.to_string(), 993),
        };
        if host.is_empty() || host.contains(['/', '@', ' ']) {
            return Err(CoreError::config(format!(
                "imap address '{address}' is host or host:port, such as imap.example.org:993"
            )));
        }
        let username = field(obj, "username")
            .ok_or_else(|| CoreError::config("imap input requires 'username'"))?
            .to_string();
        let password = field(obj, "password")
            .ok_or_else(|| {
                CoreError::config(
                    "imap input requires 'password', from a secret such as ${IMAP_PASSWORD}",
                )
            })?
            .to_string();
        let subject = field(obj, "subject")
            .map(|s| {
                regex::RegexBuilder::new(s)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| {
                        CoreError::config(format!("imap subject is a regular expression: {e}"))
                    })
            })
            .transpose()?;
        let attachment =
            glob::Pattern::new(field(obj, "attachment").unwrap_or("*")).map_err(|e| {
                CoreError::config(format!(
                    "imap attachment is a file-name glob such as *.xlsx: {e}"
                ))
            })?;
        let after = match (
            field(obj, "after_ack").unwrap_or("seen"),
            field(obj, "move_to"),
        ) {
            ("seen", None) => AfterAck::Seen,
            ("move", Some(to)) => AfterAck::Move(mailbox_name(to, "move_to")?),
            ("move", None) => {
                return Err(CoreError::config(
                    "imap after_ack: move needs move_to, the mailbox to move to",
                ))
            }
            ("seen", Some(_)) => {
                return Err(CoreError::config(
                    "imap move_to is only for after_ack: move",
                ))
            }
            (other, _) => {
                return Err(CoreError::config(format!(
                    "imap after_ack '{other}' is seen (the default) or move"
                )))
            }
        };
        let max_attachment_bytes = match obj.get("max_attachment_bytes") {
            None => DEFAULT_MAX_ATTACHMENT_BYTES,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=MAX_ATTACHMENT_BYTES).contains(n))
                .ok_or_else(|| {
                    CoreError::config(format!(
                        "imap max_attachment_bytes is 1 to {MAX_ATTACHMENT_BYTES}"
                    ))
                })?,
        };
        let max_messages = match obj.get("max_messages") {
            None => DEFAULT_MAX_MESSAGES,
            Some(v) => v
                .as_u64()
                .map(|n| n as usize)
                .filter(|n| (1..=MAX_MESSAGES).contains(n))
                .ok_or_else(|| {
                    CoreError::config(format!("imap max_messages is 1 to {MAX_MESSAGES}"))
                })?,
        };
        let timeout = match field(obj, "timeout") {
            None => Duration::from_secs(30),
            Some(s) => parse_duration_str(s)
                .ok()
                .filter(|d| !d.is_zero() && *d <= Duration::from_secs(300))
                .ok_or_else(|| {
                    CoreError::config(format!(
                        "imap timeout '{s}' is a duration up to 5m, such as 30s"
                    ))
                })?,
        };
        Ok(Self {
            host,
            port,
            root_cas: field(obj, "root_cas").map(str::to_owned),
            username,
            password,
            mailbox: mailbox_name(field(obj, "mailbox").unwrap_or("INBOX"), "mailbox")?,
            from: field(obj, "from").map(sender).transpose()?,
            subject,
            attachment,
            after,
            max_attachment_bytes,
            max_messages,
            timeout,
            session: None,
            uids: VecDeque::new(),
            ready: VecDeque::new(),
            given: BTreeMap::new(),
            acks: Acks::new(),
            searched: false,
        })
    }

    fn failed(&self, what: &str, e: impl std::fmt::Display) -> CoreError {
        CoreError::other(format!("imap {}:{}: {what}: {e}", self.host, self.port))
    }

    async fn open(&self, ctx: &StreamContext) -> Result<ImapSession, CoreError> {
        let addrs = resolve_host_checked(&self.host, self.port, &ctx.org).await?;
        let tcp = connect_checked(&addrs, self.timeout).await?;
        let connector =
            tokio_rustls::TlsConnector::from(tls::client_config(self.root_cas.as_deref())?);
        let tls = tokio::time::timeout(
            self.timeout,
            connector.connect(tls::server_name(&self.host)?, tcp),
        )
        .await
        .map_err(|_| self.failed("TLS", "no handshake within the timeout"))?
        .map_err(|e| self.failed("TLS", e))?;
        let mut client = Client::new(tls);
        tokio::time::timeout(self.timeout, client.read_response())
            .await
            .map_err(|_| self.failed("greeting", "none within the timeout"))?
            .map_err(|e| self.failed("greeting", e))?
            .ok_or_else(|| self.failed("greeting", "the server closed the connection"))?;
        let mut session =
            tokio::time::timeout(self.timeout, client.login(&self.username, &self.password))
                .await
                .map_err(|_| self.failed("login", "no answer within the timeout"))?
                .map_err(|(e, _)| self.failed(&format!("login as '{}'", self.username), e))?;
        if matches!(self.after, AfterAck::Move(_)) {
            let caps = session
                .capabilities()
                .await
                .map_err(|e| self.failed("CAPABILITY", e))?;
            if !caps.has_str("MOVE") {
                return Err(self.failed(
                    "after_ack: move",
                    "the server has no MOVE (RFC 6851); use after_ack: seen",
                ));
            }
        }
        session
            .select(&self.mailbox)
            .await
            .map_err(|e| self.failed(&format!("SELECT {}", self.mailbox), e))?;
        Ok(session)
    }

    fn session(&mut self) -> Result<&mut ImapSession, CoreError> {
        self.session
            .as_mut()
            .ok_or_else(|| CoreError::other("imap: not connected"))
    }

    /// The unread mails of the mailbox (from the sender, when named), oldest first.
    async fn search(&mut self) -> Result<(), CoreError> {
        let query = match &self.from {
            Some(from) => format!("UNSEEN FROM \"{from}\""),
            None => "UNSEEN".to_string(),
        };
        let found = self.session()?.uid_search(&query).await;
        let mut uids: Vec<u32> = found
            .map_err(|e| self.failed("SEARCH", e))?
            .into_iter()
            .collect();
        uids.sort_unstable();
        uids.truncate(self.max_messages);
        self.uids = uids.into();
        self.searched = true;
        Ok(())
    }

    /// Reads the next mail whose attachments match, queueing one message per attachment.
    async fn next_mail(&mut self) -> Result<bool, CoreError> {
        while let Some(uid) = self.uids.pop_front() {
            // An attachment travels base64-encoded, a third larger than its bytes.
            let limit = self.max_attachment_bytes * 4 / 3 + MAX_MAIL_OVERHEAD;
            let fetches: Vec<_> = {
                let session = self.session()?;
                let stream = session
                    .uid_fetch(uid.to_string(), "(UID RFC822.SIZE)")
                    .await;
                match stream {
                    Ok(stream) => stream.try_collect().await,
                    Err(e) => Err(e),
                }
            }
            .map_err(|e| self.failed("FETCH RFC822.SIZE", e))?;
            let size = fetches.iter().find_map(|f| f.size).unwrap_or(0) as u64;
            if size > limit {
                tracing::warn!(
                    uid,
                    size,
                    "imap: mail larger than max_attachment_bytes allows, left unread"
                );
                continue;
            }
            let fetches: Vec<_> = {
                let session = self.session()?;
                let stream = session.uid_fetch(uid.to_string(), "BODY.PEEK[]").await;
                match stream {
                    Ok(stream) => stream.try_collect().await,
                    Err(e) => Err(e),
                }
            }
            .map_err(|e| self.failed("FETCH BODY.PEEK[]", e))?;
            let Some(raw) = fetches.iter().find_map(|f| f.body()) else {
                continue;
            };
            let queued = self.attachments_of(uid, raw);
            if queued > 0 {
                self.given.insert(uid, (queued, 0));
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Queues the attachments of mail `uid` the filters keep; how many.
    fn attachments_of(&mut self, uid: u32, raw: &[u8]) -> usize {
        let Some(mail) = MessageParser::default().parse(raw) else {
            tracing::warn!(uid, "imap: a mail that does not parse, left unread");
            return 0;
        };
        let from = mail
            .from()
            .and_then(|a| a.first())
            .and_then(|a| a.address())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if self.from.as_ref().is_some_and(|want| *want != from) {
            return 0;
        }
        let subject = mail.subject().unwrap_or_default().to_string();
        if self
            .subject
            .as_ref()
            .is_some_and(|re| !re.is_match(&subject))
        {
            return 0;
        }
        let date = mail.date().map(|d| d.to_rfc3339()).unwrap_or_default();
        let mut queued = 0;
        for attachment in mail.attachments() {
            let Some(name) = attachment.attachment_name() else {
                continue;
            };
            // A name is metadata: its last path segment only, never a path the mail chose.
            let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
            if !self.attachment.matches(&name) {
                continue;
            }
            if attachment.decoded_len() as u64 > self.max_attachment_bytes {
                tracing::warn!(uid, name = %name, "imap: attachment over max_attachment_bytes, skipped");
                continue;
            }
            let mut part = Part::new(attachment.decoded().into_owned());
            part.meta_set_str("email_from", from.clone());
            part.meta_set_str("email_subject", subject.clone());
            part.meta_set_str("email_date", date.clone());
            part.meta_set_str("attachment_filename", name);
            self.ready.push_back(Attachment { uid, part });
            queued += 1;
        }
        queued
    }

    /// Flags or moves the mails whose every attachment was acknowledged.
    async fn settle(&mut self, acked: Vec<u32>) -> Result<(), CoreError> {
        let mut done = Vec::new();
        for uid in acked {
            if let Some((given, ok)) = self.given.get_mut(&uid) {
                *ok += 1;
                if *ok == *given {
                    done.push(uid);
                }
            }
        }
        for uid in done {
            self.given.remove(&uid);
            let after = self.after.clone();
            let session = self.session()?;
            let result = match &after {
                AfterAck::Seen => match session.uid_store(uid.to_string(), "+FLAGS (\\Seen)").await
                {
                    Ok(stream) => stream.try_collect::<Vec<_>>().await.map(|_| ()),
                    Err(e) => Err(e),
                },
                AfterAck::Move(to) => session.uid_mv(uid.to_string(), to).await,
            };
            if let Err(e) = result {
                // The mail stays unread and is read again next run: a duplicate, never a loss.
                tracing::warn!(uid, error = %e, "imap: an acknowledged mail was not marked");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Input for ImapInput {
    fn input_type(&self) -> &'static str {
        "imap"
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.session.is_none() {
            self.session = Some(self.open(ctx).await?);
        }
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        self.connect(ctx).await?;
        if !self.searched {
            self.search().await?;
        }
        let ready = self.acks.ready();
        self.settle(ready).await?;
        if self.ready.is_empty() && !self.next_mail().await? {
            let rest = self.acks.all(ACK_WAIT).await;
            self.settle(rest).await?;
            return Err(CoreError::EndOfInput);
        }
        let Some(Attachment { uid, part }) = self.ready.pop_front() else {
            return Err(CoreError::EndOfInput);
        };
        let ack = self.acks.responder(uid);
        Ok((vec![part], ack))
    }

    fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Some(mut session) = self.session.take() {
            let _ = session.logout().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_and_a_mailbox_cannot_carry_imap_syntax() {
        assert_eq!(
            sender("Exports@Stat.example.org").unwrap(),
            "exports@stat.example.org"
        );
        for bad in [
            "a\"b@x",
            "a b@x",
            "x",
            "@x",
            "a@",
            "a\\b@x",
            "a@x\r\nA1 LOGOUT",
        ] {
            assert!(sender(bad).is_err(), "{bad:?}");
        }
        assert!(mailbox_name("INBOX/Processed", "mailbox").is_ok());
        assert!(mailbox_name("INBOX\r\nA1 DELETE INBOX", "mailbox").is_err());
        assert!(mailbox_name("", "mailbox").is_err());
    }
}
