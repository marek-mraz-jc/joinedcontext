//! A scripted Kafka broker for the tests: it speaks the API versions the client sends, answers
//! from a small model (brokers, topics, one log per partition, one consumer group of one member)
//! and lets a test queue the answer of one call by API key, so every refusal and every malformed
//! frame a real broker could send is forced in-process.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::client::{
    FETCH, FIND_COORDINATOR, HEARTBEAT, JOIN_GROUP, LEAVE_GROUP, LIST_OFFSETS, METADATA,
    OFFSET_COMMIT, OFFSET_FETCH, SYNC_GROUP,
};
use crate::records::encode::batch;
use crate::wire::{Dec, Enc};

pub const SASL_HANDSHAKE: i16 = 17;
pub const SASL_AUTHENTICATE: i16 = 36;

/// What the broker does with one request.
pub enum Reply {
    /// A response body; the broker frames it with the request's correlation id.
    Body(Vec<u8>),
    /// These bytes as they are, in place of a framed response; true closes the connection after.
    Raw(Vec<u8>, bool),
    /// The connection is closed without an answer.
    Close,
    /// No answer, ever.
    Stall,
}

pub struct Topic {
    pub name: String,
    pub code: i16,
    /// (error, partition, leader)
    pub parts: Vec<(i16, i32, i32)>,
}

pub type Hook = Box<dyn FnMut(i16, &[u8]) -> Option<Reply> + Send>;

pub struct State {
    pub brokers: Vec<(i32, String, i32)>,
    pub topics: Vec<Topic>,
    /// (topic, partition) -> the values of its records, offset = index.
    pub log: HashMap<(String, i32), Vec<String>>,
    /// The next answers by API key, ahead of the model's.
    pub script: HashMap<i16, VecDeque<Reply>>,
    /// Asked first for every request; Some answers it.
    pub hook: Option<Hook>,
    /// Every request: API key and body.
    pub seen: Vec<(i16, Bytes)>,
    pub member: String,
    /// The group's committed offsets.
    pub committed: HashMap<(String, i32), i64>,
}

pub struct Fake {
    pub port: u16,
    pub state: Arc<Mutex<State>>,
}

impl Fake {
    /// A broker on 127.0.0.1 with topic `t` of one partition led by itself (node 1).
    pub async fn start() -> Fake {
        Self::start_with(None).await
    }

    pub async fn start_with(tls: Option<tokio_rustls::TlsAcceptor>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(State {
            brokers: vec![(1, "127.0.0.1".into(), i32::from(port))],
            topics: vec![Topic {
                name: "t".into(),
                code: 0,
                parts: vec![(0, 0, 1)],
            }],
            log: HashMap::new(),
            script: HashMap::new(),
            hook: None,
            seen: Vec::new(),
            member: "m-1".into(),
            committed: HashMap::new(),
        }));
        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let st = st.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    let _ = match tls {
                        None => serve(tcp, st).await,
                        Some(acceptor) => match acceptor.accept(tcp).await {
                            Ok(s) => serve(s, st).await,
                            Err(e) => Err(e),
                        },
                    };
                });
            }
        });
        Fake { port, state }
    }

    pub fn seed(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn push(&self, api: i16, r: Reply) {
        self.lock().script.entry(api).or_default().push_back(r);
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("fake broker state")
    }

    /// The bodies of the requests with this API key, oldest first.
    pub fn seen(&self, api: i16) -> Vec<Bytes> {
        self.lock()
            .seen
            .iter()
            .filter(|(a, _)| *a == api)
            .map(|(_, b)| b.clone())
            .collect()
    }

    pub fn produce(&self, topic: &str, partition: i32, values: &[&str]) {
        self.lock()
            .log
            .entry((topic.into(), partition))
            .or_default()
            .extend(values.iter().map(|v| v.to_string()));
    }
}

async fn serve(
    mut io: impl AsyncRead + AsyncWrite + Unpin,
    st: Arc<Mutex<State>>,
) -> std::io::Result<()> {
    loop {
        let size = io.read_i32().await?;
        let mut buf = vec![0u8; size.max(0) as usize];
        io.read_exact(&mut buf).await?;
        // the client under test always sends a well-formed header
        let mut d = Dec(Bytes::from(buf));
        let api = d.i16().unwrap_or_default();
        d.i16().unwrap_or_default();
        let correlation = d.i32().unwrap_or_default();
        d.str().unwrap_or_default();
        let out = match answer(&st, api, d.0) {
            Reply::Body(body) => {
                let mut e = Enc::default();
                e.i32(4 + body.len() as i32).i32(correlation);
                e.0.extend_from_slice(&body);
                e.0
            }
            Reply::Raw(raw, false) => raw,
            Reply::Raw(raw, true) => {
                io.write_all(&raw).await?;
                return Ok(());
            }
            Reply::Close => return Ok(()),
            Reply::Stall => std::future::pending().await,
        };
        io.write_all(&out).await?;
        io.flush().await?;
    }
}

fn answer(st: &Mutex<State>, api: i16, body: Bytes) -> Reply {
    let mut s = st.lock().expect("fake broker state");
    s.seen.push((api, body.clone()));
    if let Some(mut hook) = s.hook.take() {
        let r = hook(api, &body);
        s.hook = Some(hook);
        if let Some(r) = r {
            return r;
        }
    }
    if let Some(r) = s.script.get_mut(&api).and_then(VecDeque::pop_front) {
        return r;
    }
    Reply::Body(model(&mut s, api, Dec(body)).unwrap_or_default())
}

/// The model's answer; None (an empty body) for a request it cannot read.
fn model(s: &mut State, api: i16, mut d: Dec) -> Option<Vec<u8>> {
    let mut e = Enc::default();
    match api {
        METADATA => {
            // -1 asks for every topic
            let asked = match d.0.first_chunk::<4>().map(|b| i32::from_be_bytes(*b)) {
                Some(-1) => None,
                _ => Some(d.array(|d| d.str()).ok()?),
            };
            let topics: Vec<&Topic> = s
                .topics
                .iter()
                .filter(|t| asked.as_ref().is_none_or(|a| a.contains(&t.name)))
                .collect();
            metadata(&mut e, &s.brokers, &topics);
        }
        LIST_OFFSETS => {
            d.i32().ok()?;
            let topics = d
                .array(|d| Ok((d.str()?, d.array(|d| Ok((d.i32()?, d.i64()?)))?)))
                .ok()?;
            e.array(&topics, |e, (t, ps)| {
                e.str(t).array(ps, |e, (p, time)| {
                    let end = s.log.get(&(t.clone(), *p)).map_or(0, Vec::len) as i64;
                    e.i32(*p)
                        .i16(0)
                        .i64(-1)
                        .i64(if *time == -2 { 0 } else { end });
                });
            });
        }
        FETCH => {
            for _ in 0..4 {
                d.i32().ok()?;
            }
            d.i8().ok()?;
            let topics = d
                .array(|d| {
                    let t = d.str()?;
                    Ok((t, d.array(|d| Ok((d.i32()?, d.i64()?, d.i32()?)))?))
                })
                .ok()?;
            e.i32(0).array(&topics, |e, (t, ps)| {
                e.str(t).array(ps, |e, (p, from, _)| {
                    let log = s.log.get(&(t.clone(), *p)).cloned().unwrap_or_default();
                    let hw = log.len() as i64;
                    e.i32(*p).i16(0).i64(hw).i64(hw).i32(0);
                    let rest: Vec<(&str, &str)> = log
                        .iter()
                        .skip(usize::try_from(*from).unwrap_or(usize::MAX))
                        .map(|v| ("k", v.as_str()))
                        .collect();
                    if rest.is_empty() {
                        e.bytes(&[]);
                    } else {
                        e.bytes(&batch(*from, 0, None, &rest));
                    }
                });
            });
        }
        FIND_COORDINATOR => {
            let (id, host, port) = s.brokers.first()?.clone();
            e.i32(0).i16(0).null_str().i32(id).str(&host).i32(port);
        }
        JOIN_GROUP => {
            d.str().ok()?;
            d.i32().ok()?;
            d.i32().ok()?;
            d.str().ok()?;
            d.str().ok()?;
            let protocols = d.array(|d| Ok((d.str()?, d.bytes()?))).ok()?;
            let meta = protocols.first()?.1.clone();
            e.i32(0)
                .i16(0)
                .i32(1)
                .str("range")
                .str(&s.member)
                .str(&s.member);
            e.array(&[()], |e, _| {
                e.str(&s.member).bytes(&meta);
            });
        }
        SYNC_GROUP => {
            d.str().ok()?;
            d.i32().ok()?;
            d.str().ok()?;
            let assignments = d.array(|d| Ok((d.str()?, d.bytes()?))).ok()?;
            let mine = assignments
                .into_iter()
                .find(|(m, _)| *m == s.member)
                .map(|(_, a)| a)
                .unwrap_or_default();
            e.i32(0).i16(0).bytes(&mine);
        }
        OFFSET_FETCH => {
            d.str().ok()?;
            let topics = d.array(|d| Ok((d.str()?, d.array(|d| d.i32())?))).ok()?;
            e.array(&topics, |e, (t, ps)| {
                e.str(t).array(ps, |e, p| {
                    let off = s.committed.get(&(t.clone(), *p)).copied().unwrap_or(-1);
                    e.i32(*p).i64(off).null_str().i16(0);
                });
            });
        }
        OFFSET_COMMIT => {
            d.str().ok()?;
            d.i32().ok()?;
            d.str().ok()?;
            d.i64().ok()?;
            let topics = d
                .array(|d| {
                    let t = d.str()?;
                    Ok((
                        t,
                        d.array(|d| {
                            let p = d.i32()?;
                            let off = d.i64()?;
                            d.null_str()?;
                            Ok((p, off))
                        })?,
                    ))
                })
                .ok()?;
            for (t, ps) in &topics {
                for (p, off) in ps {
                    s.committed.insert((t.clone(), *p), *off);
                }
            }
            e.array(&topics, |e, (t, ps)| {
                e.str(t).array(ps, |e, (p, _)| {
                    e.i32(*p).i16(0);
                });
            });
        }
        HEARTBEAT | LEAVE_GROUP => {
            e.i32(0).i16(0);
        }
        _ => return None,
    }
    Some(e.0)
}

/// A Metadata v4 answer.
pub fn metadata(e: &mut Enc, brokers: &[(i32, String, i32)], topics: &[&Topic]) {
    e.i32(0).array(brokers, |e, (id, host, port)| {
        e.i32(*id).str(host).i32(*port).null_str();
    });
    e.null_str().i32(1).array(topics, |e, t| {
        e.i16(t.code)
            .str(&t.name)
            .bool(false)
            .array(&t.parts, |e, (c, p, l)| {
                e.i16(*c).i32(*p).i32(*l).array(&[*l], |e, r| {
                    e.i32(*r);
                });
                e.array(&[*l], |e, r| {
                    e.i32(*r);
                });
            });
    });
}

/// A FindCoordinator v1 answer.
pub fn coordinator(code: i16, id: i32, host: &str, port: i32) -> Reply {
    let mut e = Enc::default();
    e.i32(0).i16(code).null_str().i32(id).str(host).i32(port);
    Reply::Body(e.0)
}

/// A JoinGroup v2 answer naming `leader` and the member this input becomes.
pub fn joined(
    code: i16,
    protocol: &str,
    leader: &str,
    member: &str,
    members: &[(&str, Vec<u8>)],
) -> Reply {
    let mut e = Enc::default();
    e.i32(0)
        .i16(code)
        .i32(1)
        .str(protocol)
        .str(leader)
        .str(member);
    e.array(members, |e, (m, meta)| {
        e.str(m).bytes(meta);
    });
    Reply::Body(e.0)
}

/// A SyncGroup v1 answer.
pub fn synced(c: i16, assignment: &[u8]) -> Reply {
    let mut e = Enc::default();
    e.i32(0).i16(c).bytes(assignment);
    Reply::Body(e.0)
}

/// An answer with one error code for one partition: OffsetCommit v2 or ListOffsets v1 shape.
pub fn partition_code(api: i16, topic: &str, partition: i32, c: i16) -> Reply {
    let mut e = Enc::default();
    e.array(&[()], |e, _| {
        e.str(topic).array(&[()], |e, _| {
            e.i32(partition).i16(c);
            if api == LIST_OFFSETS {
                e.i64(-1).i64(0);
            }
        });
    });
    Reply::Body(e.0)
}

/// An OffsetFetch v1 answer for one partition.
pub fn offset(topic: &str, partition: i32, off: i64, c: i16) -> Reply {
    let mut e = Enc::default();
    e.array(&[()], |e, _| {
        e.str(topic).array(&[()], |e, _| {
            e.i32(partition).i64(off).null_str().i16(c);
        });
    });
    Reply::Body(e.0)
}

/// A Fetch v4 answer for one partition with these record batch bytes.
pub fn fetched(topic: &str, partition: i32, c: i16, hw: i64, records: &[u8]) -> Reply {
    fetched_aborted(topic, partition, c, hw, &[], records)
}

/// A Fetch v4 answer naming aborted transactions as (producer id, first offset).
pub fn fetched_aborted(
    topic: &str,
    partition: i32,
    c: i16,
    hw: i64,
    aborted: &[(i64, i64)],
    records: &[u8],
) -> Reply {
    let mut e = Enc::default();
    e.i32(0).array(&[()], |e, _| {
        e.str(topic).array(&[()], |e, _| {
            e.i32(partition).i16(c).i64(hw).i64(hw);
            e.array(aborted, |e, (pid, first)| {
                e.i64(*pid).i64(*first);
            });
            e.bytes(records);
        });
    });
    Reply::Body(e.0)
}

/// A SaslHandshake v1 answer.
pub fn handshake(c: i16, offered: &[&str]) -> Reply {
    let mut e = Enc::default();
    e.i16(c).array(offered, |e, m| {
        e.str(m);
    });
    Reply::Body(e.0)
}

/// A SaslAuthenticate v0 answer.
pub fn authenticated(c: i16, message: Option<&str>, auth: &[u8]) -> Reply {
    let mut e = Enc::default();
    e.i16(c);
    match message {
        Some(m) => e.str(m),
        None => e.null_str(),
    };
    e.bytes(auth);
    Reply::Body(e.0)
}

/// The SASL bytes of a SaslAuthenticate request body.
pub fn sasl_bytes(body: &[u8]) -> Vec<u8> {
    Dec(Bytes::copy_from_slice(body))
        .bytes()
        .map(|b| b.to_vec())
        .unwrap_or_default()
}
