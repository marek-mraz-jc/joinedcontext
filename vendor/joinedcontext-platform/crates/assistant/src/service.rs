//! The chat's settings and the shared state built from them. The library reads no environment:
//! `jc-assistant` fills [`ChatSettings`] from its `JC_ASSISTANT_*` variables, a host that links
//! the crate fills it from its own configuration.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use jc_core::ClientCredential;
use sqlx::PgPool;

use crate::catalogue::Reader;
use crate::chat::model::{Model, ModelConfig};
use crate::chat::{ChatState, MAX_QUESTIONS};
use crate::embed::Embedder;

/// The chat's settings as the deployment gives them; [`chat_config`] checks them.
pub struct ChatSettings {
    /// The connectors' MCP surfaces: the Context Gateway, scheme and authority.
    pub gateway_url: String,
    /// `jc-agent-proxy`, scheme and authority.
    pub proxy_url: String,
    /// The realm's token endpoint.
    pub token_url: String,
    /// The service's Keycloak client.
    pub client_id: String,
    /// How it proves that client: the pod's projected token or the client secret (PF-47).
    pub credential: ClientCredential,
    /// The model the completions name.
    pub llm: String,
    /// `jc-functions`, scheme and authority, for a deployment with `sandbox: true`.
    pub functions_url: Option<String>,
    /// The Portal's client, the one caller of the administration paths.
    pub portal_client: String,
    /// `https://assistant.{domain}`, whose widget may ask the chat.
    pub public_origin: Option<String>,
    /// The public origin `/api/endpoint/{slug}` is served under (AG-116).
    pub endpoint_url: Option<String>,
    /// `https://portal.{domain}`, which a `guide` source's sections link into (AG-118).
    pub portal_url: Option<String>,
    /// `https://viz.{domain}`, the renderer a data answer's maps and charts are drawn on.
    pub viz_url: Option<String>,
    /// The loopback listener of the `jc` that serves the gateway's host: the Endpoint counts
    /// go to that host over plain http there instead of through the edge.
    pub reach: Option<String>,
}

/// A setting [`chat_config`] refused, named by its [`ChatSettings`] field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    GatewayUrl,
    ProxyUrl,
    FunctionsUrl,
    EndpointUrl,
    PortalUrl,
    VizUrl,
    Reach,
}

impl Setting {
    /// The [`ChatSettings`] field.
    pub fn field(self) -> &'static str {
        match self {
            Setting::GatewayUrl => "gateway_url",
            Setting::ProxyUrl => "proxy_url",
            Setting::FunctionsUrl => "functions_url",
            Setting::EndpointUrl => "endpoint_url",
            Setting::PortalUrl => "portal_url",
            Setting::VizUrl => "viz_url",
            Setting::Reach => "reach",
        }
    }
}

/// What is wrong with an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Not a URL; the parser's reason.
    NotAUrl(String),
    /// A path or a query beside scheme and authority.
    NotSchemeAndAuthority,
    /// The chart renderer is not `https://viz.{domain}`.
    NotViz,
    /// The reach is not a loopback address and port.
    NotLoopback,
}

impl Problem {
    /// The problem with the setting called `name`, as a sentence.
    pub fn named(&self, name: &str) -> String {
        match self {
            Problem::NotAUrl(why) => format!("{name}: {why}"),
            Problem::NotSchemeAndAuthority => format!("{name} is scheme and authority only"),
            Problem::NotViz => format!("{name} is https://viz.{{domain}}"),
            Problem::NotLoopback => {
                format!("{name} is a loopback address and port, e.g. 127.0.0.1:8080")
            }
        }
    }
}

/// A refused setting and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub setting: Setting,
    pub problem: Problem,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.problem.named(self.setting.field()))
    }
}

impl std::error::Error for Invalid {}

/// The checked settings [`chat_state`] builds on.
pub struct ChatConfig {
    /// The Context Gateway, scheme and authority without its last `/`.
    pub gateway: String,
    pub model: ModelConfig,
    pub functions: Option<String>,
    pub portal_client: String,
    pub public_origin: Option<String>,
    /// [`ChatSettings::endpoint_url`] ending in `/`, for joins.
    pub endpoint_base: Option<url::Url>,
    pub portal: Option<String>,
    pub viz: Option<url::Url>,
    /// [`ChatSettings::reach`], checked to be loopback.
    pub reach: Option<SocketAddr>,
}

/// A service address: scheme and authority only, returned without its last `/`.
pub fn base(value: &str) -> Result<String, Problem> {
    let parsed = url::Url::parse(value).map_err(|err| Problem::NotAUrl(err.to_string()))?;
    if parsed.path() != "/" || parsed.query().is_some() {
        return Err(Problem::NotSchemeAndAuthority);
    }
    Ok(value.trim_end_matches('/').to_owned())
}

/// The public Endpoint origin as the base a `catalogue` page's Endpoint links join onto.
pub fn endpoint_base(value: &str) -> Result<url::Url, Problem> {
    url::Url::parse(&format!("{}/", base(value)?)).map_err(|e| Problem::NotAUrl(e.to_string()))
}

/// The chart renderer `https://viz.{domain}` (T-4657): a host that is not `viz.` of a domain
/// would leave the maps without their basemap's domain.
pub fn viz_url(value: &str) -> Result<url::Url, Problem> {
    let url = url::Url::parse(&base(value)?).map_err(|e| Problem::NotAUrl(e.to_string()))?;
    match (url.scheme(), crate::chat::viz::domain(&url)) {
        ("https", Some(_)) => Ok(url),
        _ => Err(Problem::NotViz),
    }
}

/// `jc`'s own listener as a reach: loopback only, as the Portal's `JC_PORTAL_REACH`. Any other
/// address would send the reads in the clear to a machine the gateway's URL does not name.
pub fn reach(value: &str) -> Result<SocketAddr, Problem> {
    match value.trim().parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_loopback() => Ok(addr),
        _ => Err(Problem::NotLoopback),
    }
}

/// Checks `settings`: every address scheme and authority only, the renderer on a `viz.` host.
pub fn chat_config(settings: ChatSettings) -> Result<ChatConfig, Invalid> {
    let at = |setting| move |problem| Invalid { setting, problem };
    let optional = |value: &Option<String>, setting| {
        value.as_deref().map(base).transpose().map_err(at(setting))
    };
    Ok(ChatConfig {
        gateway: base(&settings.gateway_url).map_err(at(Setting::GatewayUrl))?,
        functions: optional(&settings.functions_url, Setting::FunctionsUrl)?,
        endpoint_base: settings
            .endpoint_url
            .as_deref()
            .map(endpoint_base)
            .transpose()
            .map_err(at(Setting::EndpointUrl))?,
        portal: optional(&settings.portal_url, Setting::PortalUrl)?,
        viz: settings
            .viz_url
            .as_deref()
            .map(viz_url)
            .transpose()
            .map_err(at(Setting::VizUrl))?,
        reach: settings
            .reach
            .as_deref()
            .map(reach)
            .transpose()
            .map_err(at(Setting::Reach))?,
        model: ModelConfig {
            proxy: base(&settings.proxy_url).map_err(at(Setting::ProxyUrl))?,
            token_url: settings.token_url,
            client_id: settings.client_id,
            credential: settings.credential,
            model: settings.llm,
        },
        portal_client: settings.portal_client,
        public_origin: settings
            .public_origin
            .map(|o| o.trim_end_matches('/').to_owned()),
    })
}

/// The chat's shared state and the catalogue reader the crawl worker shares its client with.
pub fn chat_state(
    config: ChatConfig,
    pool: &PgPool,
    embedder: &Embedder,
    organization: &str,
) -> Result<(Arc<ChatState>, Reader), reqwest::Error> {
    // No redirect of its own: a token or a question is never carried to a host nobody named.
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?;
    let (count_http, count_base) = counter(&config, &http)?;
    let reader = Reader {
        http: count_http,
        gateway: count_base,
        endpoint_base: config.endpoint_base.clone(),
        portal: config.portal,
        held: Default::default(),
    };
    let chat_state = Arc::new(ChatState {
        pool: pool.clone(),
        embedder: embedder.clone(),
        model: Model::new(config.model, http.clone()),
        http,
        gateway: config.gateway,
        functions: config.functions,
        portal_client: config.portal_client,
        public_origin: config.public_origin,
        snapshot: RwLock::new(Arc::default()),
        limits: tokio::sync::Mutex::default(),
        questions: Arc::new(tokio::sync::Semaphore::new(MAX_QUESTIONS)),
        answers: Default::default(),
        endpoint_base: config.endpoint_base,
        viz: config.viz,
        organization: organization.to_owned(),
        contexts: Default::default(),
        turns: Default::default(),
    });
    Ok((chat_state, reader))
}

/// The client and base the Endpoint counts go out on (the minute round and the catalogue sync):
/// with a reach, the gateway's host over plain http dialed at `jc`'s own listener, which routes
/// it by that name through the same router, layers and Policy check as a read through the edge,
/// anonymous as there; else `http` at the gateway's base. Only the counts: the connectors' MCP
/// calls keep the gateway's base.
pub fn counter(
    config: &ChatConfig,
    http: &reqwest::Client,
) -> Result<(reqwest::Client, Option<url::Url>), reqwest::Error> {
    let gateway = url::Url::parse(&format!("{}/", config.gateway)).ok();
    let host = gateway.as_ref().and_then(url::Url::host_str);
    let (Some(addr), Some(host)) = (config.reach, host) else {
        return Ok((http.clone(), gateway));
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .resolve(host, addr)
        .build()?;
    Ok((client, url::Url::parse(&format!("http://{host}/")).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ChatSettings {
        ChatSettings {
            gateway_url: "http://gateway:8080/".into(),
            proxy_url: "http://proxy:8080".into(),
            token_url: "https://idm.test/token".into(),
            client_id: "jc-assistant".into(),
            credential: ClientCredential::Assertion("/var/run/token".into()),
            llm: "a-model".into(),
            functions_url: None,
            portal_client: "portal-api".into(),
            public_origin: Some("https://assistant.city.test/".into()),
            endpoint_url: Some("https://data.city.test".into()),
            portal_url: None,
            viz_url: Some("https://viz.dev.city.test".into()),
            reach: None,
        }
    }

    #[test]
    fn the_checked_config_keeps_scheme_and_authority_without_the_last_slash() {
        let config = chat_config(settings()).expect("valid");
        assert_eq!(config.gateway, "http://gateway:8080");
        assert_eq!(config.model.proxy, "http://proxy:8080");
        assert_eq!(config.model.model, "a-model");
        assert_eq!(
            config.public_origin.as_deref(),
            Some("https://assistant.city.test")
        );
        assert_eq!(
            config
                .endpoint_base
                .expect("set")
                .join("api/endpoint/air")
                .expect("joins")
                .as_str(),
            "https://data.city.test/api/endpoint/air"
        );
        assert_eq!(
            config.viz.expect("set").as_str(),
            "https://viz.dev.city.test/"
        );
        assert_eq!(config.functions, None);
    }

    /// A change that spoils one setting, the setting and the problem it gets.
    type Case = (fn(&mut ChatSettings), Setting, Problem);

    #[test]
    fn a_refused_setting_names_its_field() {
        let cases: [Case; 6] = [
            (
                |s| s.gateway_url = "http://gw/mcp".into(),
                Setting::GatewayUrl,
                Problem::NotSchemeAndAuthority,
            ),
            (
                |s| s.proxy_url = "http://proxy/?a=1".into(),
                Setting::ProxyUrl,
                Problem::NotSchemeAndAuthority,
            ),
            (
                |s| s.functions_url = Some("http://fn/x".into()),
                Setting::FunctionsUrl,
                Problem::NotSchemeAndAuthority,
            ),
            (
                |s| s.endpoint_url = Some("https://data/x".into()),
                Setting::EndpointUrl,
                Problem::NotSchemeAndAuthority,
            ),
            (
                |s| s.portal_url = Some("https://portal/x".into()),
                Setting::PortalUrl,
                Problem::NotSchemeAndAuthority,
            ),
            (
                |s| s.viz_url = Some("http://viz.dev.city.test".into()),
                Setting::VizUrl,
                Problem::NotViz,
            ),
        ];
        for (change, setting, problem) in cases {
            let mut wrong = settings();
            change(&mut wrong);
            let err = chat_config(wrong).err().expect("refused");
            assert_eq!(
                err,
                Invalid {
                    setting,
                    problem: problem.clone()
                }
            );
            assert!(err.to_string().starts_with(setting.field()), "{err}");
        }
        let mut wrong = settings();
        wrong.gateway_url = "not a url".into();
        assert!(matches!(
            chat_config(wrong).err(),
            Some(Invalid {
                setting: Setting::GatewayUrl,
                problem: Problem::NotAUrl(_)
            })
        ));
        for bad in [
            "https://charts.dev.city.test",
            "https://viz.dev.city.test/x",
        ] {
            assert!(viz_url(bad).is_err(), "{bad}");
        }
    }

    /// The reach is loopback only, as `reach.rs` holds the Portal's: anything else is refused.
    #[test]
    fn a_reach_is_a_loopback_address_and_port() {
        assert_eq!(
            reach("127.0.0.1:8080"),
            Ok("127.0.0.1:8080".parse().unwrap())
        );
        assert_eq!(reach("[::1]:8080"), Ok("[::1]:8080".parse().unwrap()));
        for bad in [
            "10.0.0.1:8080",
            "0.0.0.0:8080",
            "localhost:8080",
            "127.0.0.1",
            "api.city.test:80",
        ] {
            let mut wrong = settings();
            wrong.reach = Some(bad.into());
            assert_eq!(
                chat_config(wrong).err(),
                Some(Invalid {
                    setting: Setting::Reach,
                    problem: Problem::NotLoopback
                }),
                "{bad}"
            );
        }
    }

    /// With a reach the counts name the gateway's host over plain http and dial `jc`'s
    /// listener; without one they keep the gateway's base.
    #[tokio::test]
    async fn the_counts_go_to_the_gateways_host_at_the_reach() {
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
        let http = reqwest::Client::new();
        let mut config = chat_config(ChatSettings {
            gateway_url: "https://api.reach.invalid".into(),
            ..settings()
        })
        .unwrap();
        let (_, base) = counter(&config, &http).unwrap();
        assert_eq!(base.unwrap().as_str(), "https://api.reach.invalid/");
        config.reach = Some(addr);
        let (client, base) = counter(&config, &http).unwrap();
        let base = base.unwrap();
        assert_eq!(base.as_str(), "http://api.reach.invalid/");
        let answer = client
            .get(base.join("api/endpoint/x/ngsi-ld/v1/entities").unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), reqwest::StatusCode::NO_CONTENT);
        let head = served.await.unwrap();
        assert!(
            head.starts_with("get /api/endpoint/x/ngsi-ld/v1/entities http/1.1"),
            "{head}"
        );
        assert!(head.contains("\r\nhost: api.reach.invalid\r\n"), "{head}");
    }
}
