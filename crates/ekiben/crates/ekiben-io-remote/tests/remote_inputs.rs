//! T-7087: the `ftp` and `imap` inputs against small in-process servers speaking the protocol
//! (FTP with explicit TLS on the control and data channel, IMAP over implicit TLS), with a
//! certificate made for the test and handed to the input as `root_cas`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use ekiben_bloblang::Environment;
use ekiben_core::metrics::StreamMetrics;
use ekiben_core::org::{OrgContext, OrgId, OrgQuotas, ResourceManager};
use ekiben_core::{BatchError, ComponentConfig, CoreError, Input, Registry, StreamContext};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

type Files = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;
type Log = Arc<Mutex<Vec<String>>>;

/// A certificate for `localhost`, its PEM (the input's `root_cas`) and the acceptor serving it.
fn tls() -> (String, TlsAcceptor) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert");
    let pem = cert.cert.pem();
    let key = rustls_pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert.cert.der().clone()], key)
    .expect("server config");
    (pem, TlsAcceptor::from(Arc::new(config)))
}

fn context(quotas: OrgQuotas) -> StreamContext {
    let resources = ResourceManager::new();
    let org: Arc<OrgContext> = resources.set_org_quotas(&OrgId::new("t"), quotas);
    let (_tx, rx) = watch::channel(false);
    StreamContext {
        stream_id: "remote-test".into(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("remote-test", "t"),
        label: "in".into(),
        path: "input".into(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    }
}

fn loopback() -> OrgQuotas {
    OrgQuotas {
        allow_private_ips: true,
        ..OrgQuotas::default()
    }
}

fn conf(kind: &str, config: Value) -> ComponentConfig {
    ComponentConfig {
        component_type: kind.into(),
        config,
        ..ComponentConfig::default()
    }
}

/// Reads every message of one run, acking those `ack` keeps and nacking the others.
async fn run(
    input: &mut dyn Input,
    ctx: &StreamContext,
    ack: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<u8>, BTreeMap<String, String>)> {
    let mut seen = Vec::new();
    loop {
        match input.read_batch(ctx).await {
            Ok((batch, responder)) => {
                let part = &batch[0];
                let meta: BTreeMap<String, String> = [
                    "ftp_path",
                    "email_from",
                    "email_subject",
                    "email_date",
                    "attachment_filename",
                ]
                .iter()
                .filter(|k| part.meta_get(k).is_some())
                .map(|k| (k.to_string(), part.meta_get_str(k)))
                .collect();
                let key = meta
                    .get("ftp_path")
                    .or(meta.get("attachment_filename"))
                    .cloned()
                    .unwrap_or_default();
                let keep = ack(&format!(
                    "{key} {}",
                    meta.get("email_subject").cloned().unwrap_or_default()
                ));
                seen.push((key, part.as_bytes_ref().to_vec(), meta));
                if keep {
                    responder.ack();
                } else {
                    responder.nack(BatchError::new(Vec::new(), "the output refused it"));
                }
            }
            Err(CoreError::EndOfInput) => break,
            Err(e) => panic!("read_batch: {e}"),
        }
    }
    input.close().await.expect("close");
    seen
}

// ---------------------------------------------------------------- FTP

async fn line<S: AsyncRead + Unpin>(reader: &mut BufReader<S>) -> Option<String> {
    let mut l = String::new();
    match reader.read_line(&mut l).await {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(l.trim_end().to_string()),
    }
}

/// One FTP session after the optional AUTH TLS: login, PASV, NLST, RETR, SIZE, DELE, RNFR/RNTO.
async fn ftp_session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    files: Files,
    log: Log,
    tls: Option<TlsAcceptor>,
    pasv_ip: &str,
) {
    let mut s = BufReader::new(stream);
    let mut data: Option<TcpListener> = None;
    let mut protect = false;
    let mut rename_from = String::new();
    while let Some(cmd) = line(&mut s).await {
        log.lock().unwrap().push(cmd.clone());
        let (verb, arg) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        let reply = match verb.to_ascii_uppercase().as_str() {
            "USER" => "331 password please".to_string(),
            "PASS" if arg == "s3cret" => "230 logged in".into(),
            "PASS" => "530 not logged in".into(),
            "TYPE" | "PBSZ" => "200 ok".into(),
            "PROT" => {
                protect = arg == "P";
                "200 ok".into()
            }
            "PASV" => {
                let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = l.local_addr().unwrap().port();
                data = Some(l);
                // The address is the server's word; the client must ignore it for the peer's.
                format!(
                    "227 Entering Passive Mode ({pasv_ip},{},{})",
                    port / 256,
                    port % 256
                )
            }
            "NLST" | "RETR" => {
                let body = if verb.eq_ignore_ascii_case("NLST") {
                    let dir = arg.trim_end_matches('/');
                    Some(
                        files
                            .lock()
                            .unwrap()
                            .keys()
                            .filter_map(|p| {
                                p.strip_prefix(&format!("{dir}/"))
                                    .map(|n| format!("{n}\r\n"))
                            })
                            .collect::<String>()
                            .into_bytes(),
                    )
                } else {
                    files.lock().unwrap().get(arg).cloned()
                };
                match (body, data.take()) {
                    (Some(body), Some(l)) => {
                        s.get_mut()
                            .write_all(b"150 opening data\r\n")
                            .await
                            .unwrap();
                        let (conn, _) = l.accept().await.unwrap();
                        match (&tls, protect) {
                            (Some(acceptor), true) => {
                                let mut conn = acceptor.accept(conn).await.unwrap();
                                let _ = conn.write_all(&body).await;
                                let _ = conn.shutdown().await;
                            }
                            _ => {
                                let mut conn = conn;
                                let _ = conn.write_all(&body).await;
                                let _ = conn.shutdown().await;
                            }
                        }
                        "226 done".into()
                    }
                    _ => "550 no such file".into(),
                }
            }
            "DELE" => match files.lock().unwrap().remove(arg) {
                Some(_) => "250 deleted".into(),
                None => "550 no such file".into(),
            },
            "RNFR" => {
                rename_from = arg.to_string();
                "350 ready".into()
            }
            "RNTO" => {
                let mut files = files.lock().unwrap();
                match files.remove(&rename_from) {
                    Some(body) => {
                        files.insert(arg.to_string(), body);
                        "250 renamed".into()
                    }
                    None => "550 no such file".into(),
                }
            }
            "QUIT" => {
                let _ = s.get_mut().write_all(b"221 bye\r\n").await;
                return;
            }
            _ => "502 not implemented".into(),
        };
        if s.get_mut()
            .write_all(format!("{reply}\r\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
    }
}

/// An FTP server on 127.0.0.1 holding `files`; with `tls`, it answers AUTH TLS.
async fn ftp_server(files: Files, tls: Option<TlsAcceptor>, pasv_ip: &'static str) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let out = log.clone();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            let (files, log, tls) = (files.clone(), log.clone(), tls.clone());
            tokio::spawn(async move {
                conn.write_all(b"220 test ftp\r\n").await.unwrap();
                match tls {
                    Some(acceptor) => {
                        // Byte by byte until AUTH TLS, so no TLS bytes sit in a buffer.
                        let mut cmd = Vec::new();
                        let mut b = [0u8; 1];
                        while conn.read_exact(&mut b).await.is_ok() && b[0] != b'\n' {
                            cmd.push(b[0]);
                        }
                        let cmd = String::from_utf8_lossy(&cmd).trim().to_string();
                        log.lock().unwrap().push(cmd.clone());
                        if !cmd.eq_ignore_ascii_case("AUTH TLS") {
                            let _ = conn.write_all(b"530 TLS required\r\n").await;
                            return;
                        }
                        conn.write_all(b"234 AUTH TLS ok\r\n").await.unwrap();
                        let Ok(secured) = acceptor.accept(conn).await else {
                            return;
                        };
                        ftp_session(secured, files, log, Some(acceptor), pasv_ip).await;
                    }
                    None => ftp_session(conn, files, log, None, pasv_ip).await,
                }
            });
        }
    });
    (port, out)
}

fn exports() -> Files {
    Arc::new(Mutex::new(BTreeMap::from([
        ("/exports/a.csv".to_string(), b"id,pm10\n1,18.2\n".to_vec()),
        ("/exports/b.csv".to_string(), b"id,pm10\n2,7.5\n".to_vec()),
        ("/exports/notes.txt".to_string(), b"not a csv".to_vec()),
        ("/exports/old.csv.done".to_string(), b"read before".to_vec()),
    ])))
}

/// FTPS: the files matching the glob, each once, with its path; only the acked one is renamed,
/// and a file renamed before is not read again. The PASV reply names another host, which the
/// input ignores for the control connection's own peer.
#[tokio::test]
async fn ftps_reads_the_matching_files_and_marks_only_what_was_acked() {
    let (pem, acceptor) = tls();
    let files = exports();
    let (port, log) = ftp_server(files.clone(), Some(acceptor), "10,255,255,1").await;
    let ctx = context(loopback());
    let mut input = ekiben_io_remote::FtpInput::from_config(
        &conf(
            "ftp",
            json!({
                "address": format!("localhost:{port}"),
                "root_cas": pem,
                "username": "city",
                "password": "s3cret",
                "path": "/exports/*.csv",
                "processed_suffix": ".done",
            }),
        ),
        &ctx,
    )
    .expect("config");
    let seen = run(&mut input, &ctx, |key| key.starts_with("/exports/a.csv")).await;
    let paths: Vec<_> = seen.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(paths, ["/exports/a.csv", "/exports/b.csv"]);
    assert_eq!(seen[0].1, b"id,pm10\n1,18.2\n");
    let files = files.lock().unwrap();
    assert!(files.contains_key("/exports/a.csv.done") && !files.contains_key("/exports/a.csv"));
    assert!(
        files.contains_key("/exports/b.csv"),
        "a nacked file stays to be read again"
    );
    let log = log.lock().unwrap();
    assert_eq!(log[0], "AUTH TLS");
    assert!(log.iter().any(|c| c == "PROT P"), "{log:?}");
}

/// Plain FTP needs the organization's quota; with it, an acked file is deleted, and a file over
/// max_file_bytes fails the run with a reason instead of being read whole.
#[tokio::test]
async fn plain_ftp_needs_the_quota_and_a_file_over_the_cap_is_refused() {
    let plain = json!({ "address": "localhost:21", "tls": "none", "path": "/exports/*.csv" });
    let err = ekiben_io_remote::FtpInput::from_config(&conf("ftp", plain), &context(loopback()))
        .err()
        .expect("refused without allow_plain_ftp")
        .to_string();
    assert!(err.contains("allow_plain_ftp"), "{err}");

    let files = exports();
    let (port, _) = ftp_server(files.clone(), None, "127,0,0,1").await;
    let ctx = context(OrgQuotas {
        allow_plain_ftp: true,
        ..loopback()
    });
    let config = |extra: Value| {
        let mut c = json!({
            "address": format!("127.0.0.1:{port}"),
            "tls": "none",
            "username": "city",
            "password": "s3cret",
            "path": "/exports/*.csv",
        });
        c.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        conf("ftp", c)
    };
    let mut input =
        ekiben_io_remote::FtpInput::from_config(&config(json!({ "delete_on_finish": true })), &ctx)
            .expect("config");
    let seen = run(&mut input, &ctx, |_| true).await;
    assert_eq!(seen.len(), 2);
    assert!(
        !files.lock().unwrap().keys().any(|k| k.ends_with(".csv")),
        "both acked files are deleted"
    );

    files
        .lock()
        .unwrap()
        .insert("/exports/big.csv".into(), vec![b'x'; 64]);
    let mut input =
        ekiben_io_remote::FtpInput::from_config(&config(json!({ "max_file_bytes": 10 })), &ctx)
            .expect("config");
    let err = input
        .read_batch(&ctx)
        .await
        .err()
        .expect("over the cap")
        .to_string();
    assert!(err.contains("max_file_bytes"), "{err}");
}

/// A certificate the input does not trust fails the connection; a wrong password says so.
#[tokio::test]
async fn ftps_refuses_an_untrusted_certificate_and_names_a_failed_login() {
    let (pem, acceptor) = tls();
    let (port, _) = ftp_server(exports(), Some(acceptor), "127,0,0,1").await;
    let ctx = context(loopback());
    let base = json!({ "address": format!("localhost:{port}"), "username": "city", "password": "s3cret", "path": "/exports/*.csv" });
    let mut untrusted =
        ekiben_io_remote::FtpInput::from_config(&conf("ftp", base.clone()), &ctx).expect("config");
    assert!(untrusted.read_batch(&ctx).await.is_err());

    let mut wrong = base;
    wrong["root_cas"] = json!(pem);
    wrong["password"] = json!("nope");
    let mut input =
        ekiben_io_remote::FtpInput::from_config(&conf("ftp", wrong), &ctx).expect("config");
    let err = input
        .read_batch(&ctx)
        .await
        .err()
        .expect("login")
        .to_string();
    assert!(err.contains("login as 'city'"), "{err}");

    // The outbound rule holds before anything is sent: loopback without the quota is refused.
    let refused = json!({ "address": format!("127.0.0.1:{port}"), "root_cas": pem, "path": "/exports/*.csv" });
    let mut input = ekiben_io_remote::FtpInput::from_config(
        &conf("ftp", refused),
        &context(OrgQuotas::default()),
    )
    .expect("config");
    let err = input
        .read_batch(&context(OrgQuotas::default()))
        .await
        .err()
        .expect("outbound")
        .to_string();
    assert!(err.contains("refused"), "{err}");
}

// ---------------------------------------------------------------- IMAP

fn mail(from: &str, subject: &str, attachments: &[(&str, &[u8])]) -> Vec<u8> {
    let mut m = format!(
        "From: Exports <{from}>\r\nTo: city@example.org\r\nSubject: {subject}\r\nDate: Sun, 15 Nov 2026 06:00:00 +0100\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nToday's export.\r\n"
    );
    for (name, body) in attachments {
        use base64_lite::encode;
        m.push_str(&format!(
            "--b\r\nContent-Type: application/octet-stream; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            encode(body)
        ));
    }
    m.push_str("--b--\r\n");
    m.into_bytes()
}

mod base64_lite {
    pub fn encode(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                out.push(if i <= chunk.len() {
                    T[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        out
    }
}

/// An IMAPS server whose INBOX holds `mails` by UID; it answers what the input sends and logs it.
async fn imap_server(mails: BTreeMap<u32, Vec<u8>>, acceptor: TlsAcceptor) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let out = log.clone();
    tokio::spawn(async move {
        while let Ok((conn, _)) = listener.accept().await {
            let (mails, log, acceptor) = (mails.clone(), log.clone(), acceptor.clone());
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(conn).await else {
                    return;
                };
                let mut s = BufReader::new(tls);
                s.get_mut()
                    .write_all(b"* OK [CAPABILITY IMAP4rev1 MOVE] test imap ready\r\n")
                    .await
                    .unwrap();
                while let Some(cmd) = line(&mut s).await {
                    log.lock().unwrap().push(cmd.clone());
                    let (tag, rest) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
                    let upper = rest.to_ascii_uppercase();
                    let mut reply: Vec<u8> = Vec::new();
                    if upper.starts_with("LOGIN") {
                        if rest.contains("\"s3cret\"") {
                            reply.extend(format!("{tag} OK LOGIN completed\r\n").bytes());
                        } else {
                            reply.extend(
                                format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n")
                                    .bytes(),
                            );
                        }
                    } else if upper.starts_with("CAPABILITY") {
                        reply.extend(
                            format!("* CAPABILITY IMAP4rev1 MOVE\r\n{tag} OK done\r\n").bytes(),
                        );
                    } else if upper.starts_with("SELECT") {
                        reply.extend(format!("* {} EXISTS\r\n* FLAGS (\\Seen)\r\n{tag} OK [READ-WRITE] SELECT completed\r\n", mails.len()).bytes());
                    } else if upper.starts_with("UID SEARCH") {
                        let uids: Vec<String> = mails.keys().map(u32::to_string).collect();
                        reply.extend(
                            format!(
                                "* SEARCH {}\r\n{tag} OK SEARCH completed\r\n",
                                uids.join(" ")
                            )
                            .bytes(),
                        );
                    } else if upper.starts_with("UID FETCH") {
                        let uid: u32 = rest
                            .split(' ')
                            .nth(2)
                            .and_then(|u| u.parse().ok())
                            .unwrap_or(0);
                        if let Some(body) = mails.get(&uid) {
                            if upper.contains("RFC822.SIZE") {
                                reply.extend(
                                    format!(
                                        "* {uid} FETCH (UID {uid} RFC822.SIZE {})\r\n",
                                        body.len()
                                    )
                                    .bytes(),
                                );
                            } else {
                                reply.extend(
                                    format!(
                                        "* {uid} FETCH (UID {uid} BODY[] {{{}}}\r\n",
                                        body.len()
                                    )
                                    .bytes(),
                                );
                                reply.extend(body);
                                reply.extend(b")\r\n");
                            }
                        }
                        reply.extend(format!("{tag} OK FETCH completed\r\n").bytes());
                    } else if upper.starts_with("UID STORE") {
                        let uid = rest.split(' ').nth(2).unwrap_or("0");
                        reply.extend(format!("* {uid} FETCH (UID {uid} FLAGS (\\Seen))\r\n{tag} OK STORE completed\r\n").bytes());
                    } else if upper.starts_with("UID MOVE") {
                        reply.extend(format!("{tag} OK MOVE completed\r\n").bytes());
                    } else if upper.starts_with("LOGOUT") {
                        reply.extend(format!("* BYE\r\n{tag} OK LOGOUT completed\r\n").bytes());
                        let _ = s.get_mut().write_all(&reply).await;
                        return;
                    } else {
                        reply.extend(format!("{tag} BAD unknown\r\n").bytes());
                    }
                    if s.get_mut().write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, out)
}

fn inbox() -> BTreeMap<u32, Vec<u8>> {
    BTreeMap::from([
        (
            1,
            mail(
                "exports@stat.example.org",
                "Daily export 2026-11-15",
                &[("stations.xlsx", b"PK-first"), ("notes.txt", b"n")],
            ),
        ),
        (
            2,
            mail(
                "exports@stat.example.org",
                "Daily export 2026-11-16",
                &[("stations.xlsx", b"PK-second")],
            ),
        ),
        (
            3,
            mail(
                "someone@else.example.org",
                "Daily export 2026-11-16",
                &[("stations.xlsx", b"PK-spoof")],
            ),
        ),
        (
            4,
            mail(
                "exports@stat.example.org",
                "Weekly summary",
                &[("summary.xlsx", b"PK-weekly")],
            ),
        ),
    ])
}

/// One message per matching attachment of the mails the filters keep, with the mail's metadata;
/// mails are read with BODY.PEEK and flagged \Seen only when every attachment given was acked.
#[tokio::test]
async fn imap_gives_matching_attachments_and_flags_only_acked_mails() {
    let (pem, acceptor) = tls();
    let (port, log) = imap_server(inbox(), acceptor).await;
    let ctx = context(loopback());
    let mut input = ekiben_io_remote::ImapInput::from_config(
        &conf(
            "imap",
            json!({
                "address": format!("localhost:{port}"),
                "root_cas": pem,
                "username": "city",
                "password": "s3cret",
                "from": "exports@stat.example.org",
                "subject": "^Daily export",
                "attachment": "*.xlsx",
            }),
        ),
        &ctx,
    )
    .expect("config");
    let seen = run(&mut input, &ctx, |key| key.contains("2026-11-15")).await;
    assert_eq!(
        seen.len(),
        2,
        "{:?}",
        seen.iter().map(|s| &s.2).collect::<Vec<_>>()
    );
    assert_eq!(seen[0].1, b"PK-first");
    assert_eq!(seen[0].2["attachment_filename"], "stations.xlsx");
    assert_eq!(seen[0].2["email_from"], "exports@stat.example.org");
    assert_eq!(seen[0].2["email_subject"], "Daily export 2026-11-15");
    assert_eq!(seen[0].2["email_date"], "2026-11-15T06:00:00+01:00");
    assert_eq!(seen[1].1, b"PK-second");
    let log = log.lock().unwrap();
    assert!(
        log.iter()
            .any(|c| c.contains("UID SEARCH UNSEEN FROM \"exports@stat.example.org\"")),
        "{log:?}"
    );
    assert!(log
        .iter()
        .filter(|c| c.contains("UID FETCH"))
        .all(|c| !c.contains("BODY[]") || c.contains("BODY.PEEK[]")));
    let stores: Vec<_> = log.iter().filter(|c| c.contains("UID STORE")).collect();
    assert_eq!(stores.len(), 1, "{log:?}");
    assert!(
        stores[0].contains("UID STORE 1 +FLAGS (\\Seen)"),
        "{stores:?}"
    );
}

/// after_ack: move moves the acked mail; a wrong password fails the run with the account named.
#[tokio::test]
async fn imap_moves_an_acked_mail_and_names_a_failed_login() {
    let (pem, acceptor) = tls();
    let (port, log) = imap_server(inbox(), acceptor).await;
    let ctx = context(loopback());
    let config = |password: &str| {
        conf(
            "imap",
            json!({
                "address": format!("localhost:{port}"),
                "root_cas": pem,
                "username": "city",
                "password": password,
                "mailbox": "INBOX",
                "subject": "^Weekly",
                "after_ack": "move",
                "move_to": "INBOX/Processed",
            }),
        )
    };
    let mut input =
        ekiben_io_remote::ImapInput::from_config(&config("s3cret"), &ctx).expect("config");
    let seen = run(&mut input, &ctx, |_| true).await;
    assert_eq!(seen.len(), 1);
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("UID MOVE 4 \"INBOX/Processed\"")),
        "{:?}",
        log.lock().unwrap()
    );

    let mut input =
        ekiben_io_remote::ImapInput::from_config(&config("nope"), &ctx).expect("config");
    let err = input
        .read_batch(&ctx)
        .await
        .err()
        .expect("login")
        .to_string();
    assert!(err.contains("login as 'city'"), "{err}");

    for bad in [
        json!({ "after_ack": "move" }),
        json!({ "from": "a\"b@x" }),
        json!({ "mailbox": "IN\r\nBOX" }),
        json!({ "subject": "(" }),
    ] {
        let mut c = json!({ "address": "imap.example.org", "username": "u", "password": "p" });
        c.as_object_mut()
            .unwrap()
            .extend(bad.as_object().unwrap().clone());
        assert!(
            ekiben_io_remote::ImapInput::from_config(&conf("imap", c), &ctx).is_err(),
            "{bad}"
        );
    }
}
