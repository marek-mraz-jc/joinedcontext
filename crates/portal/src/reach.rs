//! The sibling hosts the Portal dials at `jc`'s loopback listener instead of through DNS
//! (`JC_PORTAL_REACH` with `JC_PORTAL_REACH_HOSTS`, which `jc` fills in host mode).
//!
//! `jc` hands the Portal a host-mounted sibling as `http://{host}{prefix}`. A client of a sibling
//! (forge, runner, Grafana, broker, gateway, Keycloak back channel) is built with [`builder`], so
//! each such host connects to the listener and the request still names it: the listener routes
//! by that name as it does a request through the edge, and the call never leaves the pod, so the
//! edge's blocks (`/git`) and the public DNS do not apply. Other hosts resolve as before.

use std::net::SocketAddr;
use std::sync::RwLock;

use crate::config::ConfigError;

static REACH: RwLock<Vec<(String, SocketAddr)>> = RwLock::new(Vec::new());

/// Makes `hosts` the reached hosts of every client [`builder`] builds from now on.
pub fn install(hosts: &[(String, SocketAddr)]) {
    if let Ok(mut held) = REACH.write() {
        held.clone_from(&hosts.to_vec());
    }
}

/// A client builder that dials each reached host at its address.
pub fn builder() -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder();
    if let Ok(held) = REACH.read() {
        for (host, addr) in held.iter() {
            builder = builder.resolve(host, *addr);
        }
    }
    builder
}

/// `JC_PORTAL_REACH` (a loopback address and port) and `JC_PORTAL_REACH_HOSTS` (host names,
/// comma separated): both or neither. Any address but loopback would send a sibling's credential
/// in the clear to a machine its URL does not name.
pub fn from_env(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, SocketAddr)>, ConfigError> {
    let set = |key| lookup(key).filter(|v: &String| !v.trim().is_empty());
    let (addr, hosts) = match (set("JC_PORTAL_REACH"), set("JC_PORTAL_REACH_HOSTS")) {
        (None, None) => return Ok(Vec::new()),
        (Some(addr), Some(hosts)) => (addr, hosts),
        _ => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_REACH",
                reason: "JC_PORTAL_REACH and JC_PORTAL_REACH_HOSTS are set together".into(),
            })
        }
    };
    let addr = match addr.trim().parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_loopback() => addr,
        _ => {
            return Err(ConfigError::Invalid {
                var: "JC_PORTAL_REACH",
                reason: "a loopback address and port, e.g. 127.0.0.1:8080".into(),
            })
        }
    };
    hosts
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(|host| {
            let name = host.to_ascii_lowercase();
            let plain = name.split('.').all(|l| {
                !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
            if plain {
                Ok((name, addr))
            } else {
                Err(ConfigError::Invalid {
                    var: "JC_PORTAL_REACH_HOSTS",
                    reason: format!("`{host}` is not a plain host name"),
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(reach: Option<&str>, hosts: Option<&str>) -> impl Fn(&str) -> Option<String> {
        let (reach, hosts) = (reach.map(str::to_owned), hosts.map(str::to_owned));
        move |k| match k {
            "JC_PORTAL_REACH" => reach.clone(),
            "JC_PORTAL_REACH_HOSTS" => hosts.clone(),
            _ => None,
        }
    }

    #[test]
    fn the_reach_is_loopback_named_hosts_both_or_neither() {
        assert_eq!(from_env(&lookup(None, None)).unwrap(), vec![]);
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert_eq!(
            from_env(&lookup(
                Some("127.0.0.1:8080"),
                Some("City.example, api.city.example")
            ))
            .unwrap(),
            vec![
                ("city.example".to_owned(), addr),
                ("api.city.example".to_owned(), addr)
            ]
        );
        for (reach, hosts) in [
            (Some("127.0.0.1:8080"), None),
            (None, Some("city.example")),
            (Some("10.0.0.1:8080"), Some("city.example")),
            (Some("localhost:8080"), Some("city.example")),
            (Some("127.0.0.1:8080"), Some("city.example:443")),
            (Some("127.0.0.1:8080"), Some("evil.example/x")),
        ] {
            assert!(
                from_env(&lookup(reach, hosts)).is_err(),
                "{reach:?} {hosts:?}"
            );
        }
    }

    /// A reached host connects to the listener and names itself: what jc's router dispatches on.
    #[tokio::test]
    async fn a_reached_host_is_dialed_at_the_listener_under_its_own_name() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let served = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = vec![0; 4096];
            let n = socket.read(&mut head).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&head[..n]).to_ascii_lowercase()
        });
        install(&[("git.reach.invalid".to_owned(), addr)]);
        let answer = builder()
            .build()
            .unwrap()
            .get("http://git.reach.invalid/git/api/v1/version")
            .send()
            .await
            .unwrap();
        install(&[]);
        assert_eq!(answer.status(), reqwest::StatusCode::NO_CONTENT);
        let head = served.await.unwrap();
        assert!(
            head.starts_with("get /git/api/v1/version http/1.1"),
            "{head}"
        );
        assert!(head.contains("\r\nhost: git.reach.invalid\r\n"), "{head}");
    }
}
