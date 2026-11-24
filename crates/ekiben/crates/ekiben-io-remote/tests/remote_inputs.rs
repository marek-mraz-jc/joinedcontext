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

/// Every config the `ftp` and `imap` inputs refuse, built through the registry, with the words
/// of the answer that tell the person what to write instead; a good config builds.
#[test]
fn a_bad_ftp_or_imap_config_is_refused_naming_what_to_write() {
    let registry = Registry::new();
    ekiben_io_remote::register_all(&registry);
    let ctx = context(OrgQuotas::default());
    let build = |kind: &str, config: Value| match registry.build_input(&conf(kind, config), &ctx) {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    let ftp = |extra: Value| {
        let mut c = json!({ "address": "files.example.org", "path": "/exports/*.csv" });
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    };
    for (kind, config) in [
        ("ftp", ftp(json!({}))),
        (
            "imap",
            json!({ "address": "imap.example.org", "username": "u", "password": "p" }),
        ),
    ] {
        let input = registry
            .build_input(&conf(kind, config), &ctx)
            .expect("builds");
        assert_eq!(input.input_type(), kind);
        assert!(
            !input.is_connected(),
            "{kind} connects only at its first read"
        );
    }
    assert_eq!(
        build(
            "ftp",
            ftp(json!({ "processed_suffix": ".done", "max_file_bytes": 10,
        "max_files": 5, "timeout": "10s" }))
        ),
        ""
    );
    for (config, words) in [
        (json!("not an object"), "must be an object"),
        (ftp(json!({ "folder": "/x" })), "has no field 'folder'"),
        (json!({ "path": "/exports/*.csv" }), "requires 'address'"),
        (json!({ "address": "files.example.org" }), "requires 'path'"),
        (
            ftp(json!({ "tls": "implicit" })),
            "explicit (FTPS, the default) or none",
        ),
        (ftp(json!({ "tls": "none" })), "allow_plain_ftp"),
        (
            ftp(json!({ "delete_on_finish": true, "processed_suffix": ".done" })),
            "not both",
        ),
        (
            ftp(json!({ "processed_suffix": "../x" })),
            "plain file-name ending",
        ),
        (
            ftp(json!({ "max_file_bytes": 0 })),
            "max_file_bytes is 1 to",
        ),
        (ftp(json!({ "max_files": 100000 })), "max_files is 1 to"),
        (ftp(json!({ "timeout": "10m" })), "duration up to 5m"),
        (
            ftp(json!({ "path": "/exports/[" })),
            "ftp path '/exports/['",
        ),
    ] {
        let err = build("ftp", config.clone());
        assert!(err.contains(words), "{config}: {err}");
    }

    let imap = |extra: Value| {
        let mut c = json!({ "address": "imap.example.org", "username": "u", "password": "p" });
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    };
    assert_eq!(build("imap", imap(json!({}))), "");
    assert_eq!(
        build(
            "imap",
            imap(json!({ "address": "[2001:db8::1]:993", "after_ack": "move",
        "move_to": "INBOX/Done", "subject": "^Export", "max_attachment_bytes": 10,
        "max_messages": 5, "timeout": "10s", "from": "exports@stat.example.org" }))
        ),
        ""
    );
    for (config, words) in [
        (json!([]), "must be an object"),
        (imap(json!({ "folder": "x" })), "has no field 'folder'"),
        (
            json!({ "username": "u", "password": "p" }),
            "requires 'address'",
        ),
        (
            imap(json!({ "address": "imap.example.org:0" })),
            "the port is 1 to 65535",
        ),
        (
            imap(json!({ "address": "user@imap.example.org" })),
            "is host or host:port",
        ),
        (
            json!({ "address": "imap.example.org", "password": "p" }),
            "requires 'username'",
        ),
        (
            json!({ "address": "imap.example.org", "username": "u" }),
            "requires 'password'",
        ),
        (
            imap(json!({ "subject": "(" })),
            "subject is a regular expression",
        ),
        (imap(json!({ "attachment": "[" })), "file-name glob"),
        (imap(json!({ "after_ack": "move" })), "needs move_to"),
        (
            imap(json!({ "move_to": "Done" })),
            "only for after_ack: move",
        ),
        (
            imap(json!({ "after_ack": "delete" })),
            "seen (the default) or move",
        ),
        (
            imap(json!({ "after_ack": "move", "move_to": "a\u{7}" })),
            "1 to 200 printable",
        ),
        (
            imap(json!({ "max_attachment_bytes": 0 })),
            "max_attachment_bytes is 1 to",
        ),
        (imap(json!({ "max_messages": 0 })), "max_messages is 1 to"),
        (imap(json!({ "timeout": "0s" })), "duration up to 5m"),
        (
            imap(json!({ "from": "two words@x.org" })),
            "one e-mail address",
        ),
        (
            imap(json!({ "mailbox": "m".repeat(201) })),
            "1 to 200 printable",
        ),
    ] {
        let err = build("imap", config.clone());
        assert!(err.contains(words), "{config}: {err}");
    }
}

/// Reads every message of one run, acking those `ack` keeps and nacking the others.
async fn run(
    input: &mut dyn Input,
    ctx: &StreamContext,
    ack: impl Fn(&str) -> bool,
) -> Vec<(String, Vec<u8>, BTreeMap<String, String>)> {
    let mut seen = Vec::new();
    input.connect(ctx).await.expect("connect");
    assert!(input.is_connected());
    // A second connect keeps the open connection.
    input.connect(ctx).await.expect("connect again");
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
    assert!(!input.is_connected());
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
    faults: Faults,
) {
    let mut s = BufReader::new(stream);
    let mut data: Option<TcpListener> = None;
    let mut protect = false;
    let mut rename_from = String::new();
    while let Some(cmd) = line(&mut s).await {
        log.lock().unwrap().push(cmd.clone());
        let (verb, arg) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        let upper = verb.to_ascii_uppercase();
        let reply = match upper.as_str() {
            v if faults.contains(&v) => "550 refused by the test server".to_string(),
            "PASV" if faults.contains(&"pasv-full") => {
                // A listener whose accept queue is full: a connect to it hangs until timeout.
                let socket = tokio::net::TcpSocket::new_v4().unwrap();
                socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
                let l = socket.listen(0).unwrap();
                let addr = l.local_addr().unwrap();
                for _ in 0..4 {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        tokio::net::TcpStream::connect(addr),
                    )
                    .await
                    .map(|c| c.map(std::mem::forget));
                }
                std::mem::forget(l);
                format!(
                    "227 Entering Passive Mode (127,0,0,1,{},{})",
                    addr.port() / 256,
                    addr.port() % 256
                )
            }
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
                        if faults.contains(&"retr-hang") && verb.eq_ignore_ascii_case("RETR") {
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            return;
                        }
                        if let (true, Some(acceptor), true) = (
                            faults.contains(&"retr-cut") && verb.eq_ignore_ascii_case("RETR"),
                            &tls,
                            protect,
                        ) {
                            // Half the file, then a close without TLS close_notify: a cut transfer.
                            let mut conn = acceptor.accept(conn).await.unwrap();
                            let _ = conn.write_all(&body[..body.len() / 2]).await;
                            let _ = conn.flush().await;
                            drop(conn);
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            return;
                        }
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
                        if faults.contains(&"retr-451") && verb.eq_ignore_ascii_case("RETR") {
                            "451 transfer aborted".into()
                        } else {
                            "226 done".into()
                        }
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
    ftp_server_with(files, tls, pasv_ip, &[]).await
}

/// Faults a test server injects: a verb it refuses (`TYPE`, `NLST`, `RETR`, `DELE`, `SELECT`, ...)
/// or a named misbehaviour (`hang-greeting`, `bad-greeting`, `retr-hang`, `retr-cut`, ...).
type Faults = &'static [&'static str];

/// `ftp_server` misbehaving on cue (`Faults`).
async fn ftp_server_with(
    files: Files,
    tls: Option<TlsAcceptor>,
    pasv_ip: &'static str,
    faults: Faults,
) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let out = log.clone();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            let (files, log, tls) = (files.clone(), log.clone(), tls.clone());
            tokio::spawn(async move {
                if faults.contains(&"hang-greeting") {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    return;
                }
                if faults.contains(&"bad-greeting") {
                    let _ = conn.write_all(b"421 too many users\r\n").await;
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    return;
                }
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
                        ftp_session(secured, files, log, Some(acceptor), pasv_ip, faults).await;
                    }
                    None => ftp_session(conn, files, log, None, pasv_ip, faults).await,
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

/// A plain FTP input (the org has `allow_plain_ftp`) against `ftp_server_with(faults)`, with a
/// one-second timeout so a server that hangs fails the read fast.
async fn faulty_ftp(
    faults: Faults,
    extra: Value,
) -> (ekiben_io_remote::FtpInput, StreamContext, Files) {
    let files = exports();
    let (port, _) = ftp_server_with(files.clone(), None, "127,0,0,1", faults).await;
    let ctx = context(OrgQuotas {
        allow_plain_ftp: true,
        ..loopback()
    });
    let mut c = json!({
        "address": format!("127.0.0.1:{port}"),
        "tls": "none",
        "username": "city",
        "password": "s3cret",
        "path": "/exports/*.csv",
        "timeout": "1s",
    });
    c.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let input = ekiben_io_remote::FtpInput::from_config(&conf("ftp", c), &ctx).expect("config");
    (input, ctx, files)
}

/// Every step of an FTP session that the server refuses, drops or never answers fails the read
/// with the step and the path named, within the timeout.
#[tokio::test]
async fn each_ftp_step_the_server_refuses_or_stalls_fails_the_read_naming_the_step() {
    for (faults, want) in [
        (&["hang-greeting"][..], "no greeting within 1s"),
        (&["bad-greeting"], "greeting: "),
        (&["TYPE"], "TYPE I: "),
        (&["NLST"], "NLST /exports: "),
        (&["pasv-full"], "NLST /exports: "),
        (&["RETR"], "ftp RETR /exports/a.csv: "),
        (&["retr-hang"], "ftp RETR /exports/a.csv: no data within 1s"),
        (&["retr-451"], "451"),
    ] {
        let (mut input, ctx, _) = faulty_ftp(faults, json!({})).await;
        let started = std::time::Instant::now();
        let err = input
            .read_batch(&ctx)
            .await
            .err()
            .unwrap_or_else(|| panic!("{faults:?} must fail the read"))
            .to_string();
        assert!(err.contains(want), "{faults:?}: {err}");
        assert!(err.contains("ftp "), "{faults:?}: {err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{faults:?}"
        );
    }
}

/// FTPS: a data connection the server cuts mid-file without TLS close_notify fails the read; the
/// half file is never given to the pipeline.
#[tokio::test]
async fn an_ftps_transfer_cut_mid_file_fails_the_read() {
    let (pem, acceptor) = tls();
    let (port, _) = ftp_server_with(exports(), Some(acceptor), "127,0,0,1", &["retr-cut"]).await;
    let ctx = context(loopback());
    let config = json!({ "address": format!("localhost:{port}"), "root_cas": pem, "username": "city", "password": "s3cret", "path": "/exports/*.csv", "timeout": "1s" });
    let mut input =
        ekiben_io_remote::FtpInput::from_config(&conf("ftp", config), &ctx).expect("config");
    let err = input
        .read_batch(&ctx)
        .await
        .err()
        .expect("a cut transfer")
        .to_string();
    assert!(err.starts_with("ftp RETR /exports/a.csv: "), "{err}");
}

/// A file the server will not delete after the ack stays where it was (read again next run, a
/// duplicate rather than a loss) and the run still ends; a path at the root lists `/`.
#[tokio::test]
async fn an_acked_file_the_server_will_not_delete_stays_and_a_root_path_lists_slash() {
    let (mut input, ctx, files) = faulty_ftp(&["DELE"], json!({ "delete_on_finish": true })).await;
    let seen = run(&mut input, &ctx, |_| true).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(
        files
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.ends_with(".csv"))
            .count(),
        2,
        "the refused deletes leave both files"
    );

    let (mut input, ctx, files) = faulty_ftp(&[], json!({ "path": "/*.csv" })).await;
    *files.lock().unwrap() = BTreeMap::from([
        ("/top.csv".to_string(), b"id\n9\n".to_vec()),
        ("/top.txt".to_string(), b"not a csv".to_vec()),
    ]);
    let seen = run(&mut input, &ctx, |_| true).await;
    assert_eq!(
        seen.iter()
            .map(|s| (s.0.as_str(), s.1.as_slice()))
            .collect::<Vec<_>>(),
        vec![("/top.csv", &b"id\n9\n"[..])]
    );
    assert_eq!(
        files.lock().unwrap().len(),
        2,
        "keep is the default: nothing removed"
    );
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
    imap_server_with(mails, acceptor, &[]).await
}

/// `imap_server` misbehaving on cue (`Faults`).
async fn imap_server_with(
    mails: BTreeMap<u32, Vec<u8>>,
    acceptor: TlsAcceptor,
    faults: Faults,
) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::default();
    let out = log.clone();
    tokio::spawn(async move {
        while let Ok((conn, _)) = listener.accept().await {
            let (mails, log, acceptor) = (mails.clone(), log.clone(), acceptor.clone());
            tokio::spawn(async move {
                let hang = || tokio::time::sleep(std::time::Duration::from_secs(10));
                if faults.contains(&"tls-hang") {
                    hang().await;
                    return;
                }
                if faults.contains(&"plain") {
                    let mut conn = conn;
                    let _ = conn.write_all(b"* OK plain imap, no TLS\r\n").await;
                    hang().await;
                    return;
                }
                let Ok(tls) = acceptor.accept(conn).await else {
                    return;
                };
                let mut s = BufReader::new(tls);
                if faults.contains(&"no-greeting") {
                    hang().await;
                    return;
                }
                if faults.contains(&"close") {
                    let _ = s.get_mut().shutdown().await;
                    return;
                }
                if faults.contains(&"bad-greeting") {
                    let _ = s.get_mut().write_all(b"\x01\x02 not imap\r\n").await;
                    hang().await;
                    return;
                }
                s.get_mut()
                    .write_all(b"* OK [CAPABILITY IMAP4rev1 MOVE] test imap ready\r\n")
                    .await
                    .unwrap();
                while let Some(cmd) = line(&mut s).await {
                    log.lock().unwrap().push(cmd.clone());
                    let (tag, rest) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
                    let upper = rest.to_ascii_uppercase();
                    let mut reply: Vec<u8> = Vec::new();
                    let refused = faults.iter().any(|f| upper.starts_with(f))
                        || (faults.contains(&"fetch-size") && upper.contains("RFC822.SIZE"))
                        || (faults.contains(&"fetch-body") && upper.contains("BODY.PEEK"));
                    if upper.starts_with("LOGIN") && faults.contains(&"login-hang") {
                        hang().await;
                        return;
                    } else if refused && upper.starts_with("UID STORE") {
                        // A NO to STORE is read as done too: garbage, then the line drops.
                        let _ = s.get_mut().write_all(b"\x01\x02 not imap\r\n").await;
                        return;
                    } else if refused {
                        // async-imap reads a NO to CAPABILITY, SEARCH or FETCH as an empty
                        // answer: garbage makes the refusal an error.
                        let _ = s.get_mut().write_all(b"\x01\x02 not imap\r\n").await;
                        hang().await;
                        return;
                    } else if upper.starts_with("CAPABILITY") && faults.contains(&"no-move") {
                        reply
                            .extend(format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n").bytes());
                    } else if upper.starts_with("LOGIN") {
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
                                let size = if faults.contains(&"big") {
                                    999_999_999
                                } else {
                                    body.len()
                                };
                                reply.extend(
                                    format!("* {uid} FETCH (UID {uid} RFC822.SIZE {size})\r\n")
                                        .bytes(),
                                );
                            } else if faults.contains(&"nobody") {
                                reply.extend(format!("* {uid} FETCH (UID {uid})\r\n").bytes());
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

/// An IMAP input for `imap_server_with(faults)` with a one-second timeout.
async fn faulty_imap(
    mails: BTreeMap<u32, Vec<u8>>,
    faults: Faults,
    extra: Value,
) -> (ekiben_io_remote::ImapInput, StreamContext, Log) {
    let (pem, acceptor) = tls();
    let (port, log) = imap_server_with(mails, acceptor, faults).await;
    let ctx = context(loopback());
    let mut c = json!({
        "address": format!("localhost:{port}"),
        "root_cas": pem,
        "username": "city",
        "password": "s3cret",
        "timeout": "1s",
    });
    c.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let input = ekiben_io_remote::ImapInput::from_config(&conf("imap", c), &ctx).expect("config");
    (input, ctx, log)
}

/// Every step of an IMAP session that the server refuses, drops or never answers fails the read
/// with the step named, within the timeout; after_ack: move needs a server with MOVE.
#[tokio::test]
async fn each_imap_step_the_server_refuses_or_stalls_fails_the_read_naming_the_step() {
    let moving = json!({ "after_ack": "move", "move_to": "INBOX/Done" });
    for (faults, extra, want) in [
        (
            &["tls-hang"][..],
            json!({}),
            "TLS: no handshake within the timeout",
        ),
        (&["plain"], json!({}), "TLS: "),
        (
            &["no-greeting"],
            json!({}),
            "greeting: none within the timeout",
        ),
        (
            &["close"],
            json!({}),
            "greeting: the server closed the connection",
        ),
        (&["bad-greeting"], json!({}), "greeting: "),
        (
            &["login-hang"],
            json!({}),
            "login: no answer within the timeout",
        ),
        (&["CAPABILITY"], moving.clone(), "CAPABILITY: "),
        (
            &["no-move"],
            moving.clone(),
            "the server has no MOVE (RFC 6851)",
        ),
        (&["SELECT"], json!({}), "SELECT INBOX: "),
        (&["UID SEARCH"], json!({}), "SEARCH: "),
        (&["fetch-size"], json!({}), "FETCH RFC822.SIZE: "),
        (&["fetch-body"], json!({}), "FETCH BODY.PEEK[]: "),
    ] {
        let (mut input, ctx, _) = faulty_imap(inbox(), faults, extra).await;
        let started = std::time::Instant::now();
        let err = input
            .read_batch(&ctx)
            .await
            .err()
            .unwrap_or_else(|| panic!("{faults:?} must fail the read"))
            .to_string();
        assert!(err.contains(want), "{faults:?}: {err}");
        assert!(
            err.starts_with("imap localhost:") || err.contains("imap localhost:"),
            "{faults:?}: {err}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{faults:?}"
        );
    }
}

/// Mails the input cannot use are left unread and the run goes on: one larger than the cap
/// allows, one the server sends no body for, one that does not parse, an attachment with no
/// name or over the cap. A mail with an attachment still unacked is not flagged, and a flag the
/// server refuses leaves the mail unread without failing the run.
#[tokio::test]
async fn unusable_mails_are_left_unread_and_a_refused_flag_does_not_fail_the_run() {
    for faults in [&["big"][..], &["nobody"]] {
        let (mut input, ctx, log) = faulty_imap(inbox(), faults, json!({})).await;
        assert!(
            run(&mut input, &ctx, |_| true).await.is_empty(),
            "{faults:?}"
        );
        assert!(
            !log.lock().unwrap().iter().any(|c| c.contains("UID STORE")),
            "{faults:?}"
        );
    }

    let nameless = b"From: <exports@stat.example.org>\r\nSubject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment\r\nContent-Transfer-Encoding: base64\r\n\r\nUEs=\r\n--b--\r\n".to_vec();
    let mails = BTreeMap::from([
        (
            1,
            mail(
                "exports@stat.example.org",
                "Daily export",
                &[("big.xlsx", b"PK-larger-than-cap"), ("ok.xlsx", b"PK")],
            ),
        ),
        (2, Vec::new()),
        (3, nameless),
        (
            4,
            mail(
                "exports@stat.example.org",
                "Daily export",
                &[("a.xlsx", b"PK"), ("b.csv", b"id")],
            ),
        ),
    ]);
    let (mut input, ctx, log) = faulty_imap(mails, &[], json!({ "max_attachment_bytes": 4 })).await;
    let seen = run(&mut input, &ctx, |key| !key.contains("b.csv")).await;
    assert_eq!(
        seen.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        vec!["ok.xlsx", "a.xlsx", "b.csv"]
    );
    let stores: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.contains("UID STORE"))
        .cloned()
        .collect();
    assert_eq!(
        stores.len(),
        1,
        "mail 4 has an unacked attachment: {stores:?}"
    );
    assert!(stores[0].contains("UID STORE 1 "), "{stores:?}");

    let one = BTreeMap::from([(2, inbox().remove(&2).unwrap())]);
    let (mut input, ctx, log) = faulty_imap(one, &["UID STORE"], json!({})).await;
    assert_eq!(run(&mut input, &ctx, |_| true).await.len(), 1);
    assert!(
        log.lock().unwrap().iter().any(|c| c.contains("UID STORE")),
        "the refused flag was tried"
    );
}
