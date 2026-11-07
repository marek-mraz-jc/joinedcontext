#![forbid(unsafe_code)]
//! Client address extractor: the [`jc_router::ClientAddr`] the mount router resolved (trusted
//! proxies honoured), else the TCP peer from `ConnectInfo`, else unknown. Never a placeholder.

use axum::extract::{ConnectInfo, FromRequestParts};
use http::request::Parts;
use jc_router::ClientAddr;
use std::convert::Infallible;
use std::net::SocketAddr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIp(pub Option<String>);

impl ClientIp {
    #[must_use]
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// The address for an audit record: the resolved address or `"unknown"`.
    #[must_use]
    pub fn for_audit(&self) -> String {
        self.0.clone().unwrap_or_else(|| "unknown".to_owned())
    }

    #[must_use]
    pub fn from_parts(parts: &Parts) -> Self {
        let ip = parts
            .extensions
            .get::<ClientAddr>()
            .map(|c| c.0)
            .or_else(|| {
                parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|c| c.0.ip())
            });
        Self(ip.map(|i| i.to_string()))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_parts(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_address_wins_then_peer_then_unknown() {
        let (mut parts, ()) = http::Request::new(()).into_parts();
        assert_eq!(ClientIp::from_parts(&parts).as_deref(), None);
        assert_eq!(ClientIp::from_parts(&parts).for_audit(), "unknown");
        parts.extensions.insert(ConnectInfo::<SocketAddr>(
            "198.51.100.4:5555".parse().unwrap(),
        ));
        assert_eq!(
            ClientIp::from_parts(&parts).as_deref(),
            Some("198.51.100.4")
        );
        parts
            .extensions
            .insert(ClientAddr("203.0.113.8".parse().unwrap()));
        assert_eq!(ClientIp::from_parts(&parts).for_audit(), "203.0.113.8");
    }
}
