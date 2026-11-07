//! Keycloak OIDC authorization code flow with PKCE (CC-40, I1, I4).
//!
//! The portal is a confidential client: the code exchange happens server side and only an
//! encrypted session cookie reaches the browser. No token is ever written to a log.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use axum_extra::extract::cookie::{CookieJar, PrivateCookieJar};
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
    CoreClientAuthMethod, CoreGrantType, CoreIdToken, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
    CoreResponseType, CoreSubjectIdentifierType,
};
use openidconnect::{
    AdditionalProviderMetadata, ClientId, CsrfToken, EndpointMaybeSet, EndpointNotSet, EndpointSet,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, ProviderMetadata, RedirectUrl, Scope,
    TokenResponse,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth::csrf;
use crate::auth::session::{self, secure_cookie, Identity, Session, FLOW_COOKIE};
use crate::config::{ClientAuth, OidcConfig};
use crate::error::ApiError;
use crate::state::AppState;

/// RP-initiated logout is an OIDC extension, so `end_session_endpoint` is additional metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogoutMetadata {
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
}

impl AdditionalProviderMetadata for LogoutMetadata {}

type PortalProviderMetadata = ProviderMetadata<
    LogoutMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

type PortalClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// How long a login may stay in flight before the PKCE verifier cookie expires.
const FLOW_TTL_SECS: i64 = 600;
/// Session lifetime when the refresh token carries no `exp` of its own (an opaque one).
const DEFAULT_SESSION_TTL_SECS: i64 = 3600;
/// Access-token lifetime when the token response does not state one.
const DEFAULT_ACCESS_TTL_SECS: i64 = 300;

/// The gateway's hub audience, which with an `endpoint:{slug}` scope reaches one Endpoint
/// (context-gateway `HUB_AUDIENCE`, ADR-N-025 4b).
const GATEWAY_HUB_AUDIENCE: &str = "mcp-hub";

/// The realm scope that maps [`GATEWAY_HUB_AUDIENCE`] into the Portal's exchanged token, optional
/// on `portal-api` alone (deployment `components/keycloak`, T-6063): Keycloak's standard token
/// exchange hands out only an audience one of the requester's scopes maps.
const GATEWAY_HUB_SCOPE: &str = "gateway-hub";

/// A refusal of the realm's token endpoint: the text names the status and the OAuth `error`
/// code; `description` is the realm's `error_description`, kept to tell refusals apart and
/// never put into an answer (the realm writes fixed sentences there, no token).
#[derive(Debug, Default)]
struct TokenRefusal {
    text: String,
    code: String,
    description: String,
}

impl std::fmt::Display for TokenRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<String> for TokenRefusal {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

/// What a refused Endpoint exchange tells the caller (T-6063). Keycloak answers
/// `invalid_request` both for a subject token whose session ended ("Invalid token") and for a
/// realm not set up for the exchange, so the description decides between them.
/// How often the deployment links a new Endpoint's scope (its admin-permissions CronJob,
/// `*/5 * * * *`, T-6388): the wait a refused exchange names.
const LINK_MINUTES: u32 = 5;

fn exchange_refused(refusal: &TokenRefusal, slug: &str) -> ApiError {
    let ended = refusal.code == "invalid_grant"
        || refusal.code == "invalid_token"
        || (refusal.code == "invalid_request" && refusal.description == "Invalid token");
    if ended {
        return ApiError::Denied(
            "your sign-in has ended (signed out, disabled or expired): sign in to your MCP \
             client again"
                .into(),
        );
    }
    if refusal.code == "invalid_scope" {
        return ApiError::Unavailable(format!(
            "Endpoint {slug} is not open to data reads yet: the realm links its \
             `endpoint:{slug}` scope within {LINK_MINUTES} minutes of the Endpoint going Live, \
             when the keycloak-admin-permissions-sync CronJob runs; try again then, and ask an \
             administrator to check that CronJob if it still answers this after \
             {} minutes",
            2 * LINK_MINUTES
        ));
    }
    ApiError::Unavailable(format!(
        "the realm is not set up to hand your token on to the gateway ({}): an administrator \
         must enable standard token exchange on the Portal client `portal-api` with its \
         optional `{GATEWAY_HUB_SCOPE}` scope (deployment components/keycloak); signing in \
         again does not help",
        if refusal.code.is_empty() {
            "no answer"
        } else {
            refusal.code.as_str()
        }
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("cannot build the OIDC http client: {0}")]
    HttpClient(String),
    #[error("invalid issuer or redirect url: {0}")]
    Url(String),
    #[error("OIDC discovery against the realm failed: {0}")]
    Discovery(String),
}

/// The discovered Keycloak realm plus the http client used for every server-to-server call.
pub struct OidcClient {
    /// The discovered realm: the authorization URL and the ID token verifier. Token requests do
    /// not go through it, because its token requests always carry `client_id`, which a
    /// `federated-jwt` client refuses beside an assertion (PF-47, T-2868).
    client: PortalClient,
    http: reqwest::Client,
    token_url: Url,
    client_id: String,
    redirect_uri: String,
    auth: ClientAuth,
    end_session_endpoint: Option<Url>,
    /// Where the silent sign-in check of a framed App asks the realm (AP-122, T-3034).
    authorization_endpoint: Url,
    /// The Portal's own client-credentials token and when to stop using it.
    service: tokio::sync::Mutex<Option<(String, std::time::Instant)>>,
}

/// `Display` of a `DiscoveryError` is "Request failed" and the reason lives in its source, so a
/// TLS failure reads exactly like a 404 in the log. This walks the chain and joins it.
fn source_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut cause = error.source();
    while let Some(current) = cause {
        parts.push(current.to_string());
        cause = current.source();
    }
    parts.join(": ")
}

impl OidcClient {
    pub async fn discover(config: &OidcConfig, redirect_uri: &str) -> Result<Self, OidcError> {
        // No redirects: an authorization server must never bounce a token request elsewhere.
        let mut builder = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        // The binary trusts `webpki-roots` and nothing else, so an issuer served by a private
        // CA needs its root handed over explicitly (JC_OIDC_CA_FILE). Added, never swapped in:
        // the public roots stay, and certificate verification is untouched.
        if let Some(pem) = &config.extra_ca_pem {
            // Whole bundle, not `from_pem`: a private CA is usually a chain and the first block
            // alone would not verify. Empty is an error rather than "carry on with the public
            // roots" — rustls drops roots it cannot parse without a word, so a wrong mount would
            // otherwise fail later as a plain handshake error with nothing pointing at the file.
            let roots = reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|e| OidcError::HttpClient(format!("JC_OIDC_CA_FILE: {e}")))?;
            if roots.is_empty() {
                return Err(OidcError::HttpClient(
                    "JC_OIDC_CA_FILE: no PEM certificate in the file".to_string(),
                ));
            }
            for root in roots {
                builder = builder.add_root_certificate(root);
            }
        }
        let http = builder
            .build()
            .map_err(|e| OidcError::HttpClient(e.to_string()))?;

        // A trailing slash makes the discovered `issuer` claim mismatch; Keycloak publishes none.
        let issuer = IssuerUrl::new(config.issuer.as_str().trim_end_matches('/').to_string())
            .map_err(|e| OidcError::Url(e.to_string()))?;
        let metadata = PortalProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| OidcError::Discovery(source_chain(&e)))?;
        let end_session_endpoint = metadata
            .additional_metadata()
            .end_session_endpoint
            .as_ref()
            .and_then(|raw| Url::parse(raw).ok());
        let authorization_endpoint = metadata.authorization_endpoint().url().clone();
        let token_url = metadata
            .token_endpoint()
            .map(|endpoint| endpoint.url().clone())
            .ok_or_else(|| OidcError::Discovery("the realm publishes no token endpoint".into()))?;

        let redirect = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| OidcError::Url(e.to_string()))?;
        // No secret: the ID token verifier then accepts the realm's signing keys alone, never an
        // HMAC keyed with a client secret.
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            None,
        )
        .set_redirect_uri(redirect);

        Ok(Self {
            client,
            http,
            token_url,
            client_id: config.client_id.clone(),
            redirect_uri: redirect_uri.to_string(),
            auth: config.auth.clone(),
            end_session_endpoint,
            authorization_endpoint,
            service: tokio::sync::Mutex::new(None),
        })
    }

    /// The realm's authorization request of the silent sign-in check (AP-122, T-3034).
    ///
    /// `prompt=none` never shows a page: the realm answers at once, to the Portal's own
    /// [`SSO_CHECK_DONE`], `#error=login_required` when the person must sign in and `#code=…` when
    /// the realm session lives. The fragment keeps the answer out of every request and log. The
    /// PKCE verifier is dropped here, so a code the realm hands out can never be redeemed by
    /// anyone: the check learns the session's state and nothing else.
    pub fn sso_check_url(&self, public_base: &Url) -> Url {
        let (challenge, _never_redeemed) = PkceCodeChallenge::new_random_sha256();
        let mut url = self.authorization_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("client_id", SSO_CHECK_CLIENT)
            .append_pair(
                "redirect_uri",
                &format!(
                    "{}{SSO_CHECK_DONE}",
                    public_base.as_str().trim_end_matches('/')
                ),
            )
            .append_pair("response_type", "code")
            .append_pair("response_mode", "fragment")
            .append_pair("scope", "openid")
            .append_pair("prompt", "none")
            .append_pair("code_challenge", challenge.as_str())
            .append_pair("code_challenge_method", "S256");
        url
    }

    /// One request to the realm's token endpoint as the Portal's client: `grant` plus the
    /// client's proof (`ClientAuth::form`, PF-47). The error's text carries the status and the
    /// OAuth `error` code, never a token or the realm's free-text description.
    async fn token_request(
        &self,
        grant: &[(&'static str, &str)],
    ) -> Result<openidconnect::core::CoreTokenResponse, TokenRefusal> {
        let form = self.auth.form(&self.client_id, grant).await?;
        let response = self
            .http
            .post(self.token_url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|err| source_chain(&err.without_url()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| source_chain(&err.without_url()))?;
        if !status.is_success() {
            #[derive(Deserialize)]
            struct Refusal {
                error: String,
                #[serde(default)]
                error_description: String,
            }
            let refusal = serde_json::from_slice::<Refusal>(&body).ok();
            let (code, description) = refusal
                .map(|refusal| (refusal.error, refusal.error_description))
                .unwrap_or_default();
            return Err(TokenRefusal {
                text: format!("the token endpoint answered {status} {code}")
                    .trim_end()
                    .to_owned(),
                code,
                description,
            });
        }
        // Not the parser's message: it can quote a value of the body, and the body holds tokens.
        serde_json::from_slice(&body).map_err(|_| {
            format!("the token endpoint answered {status} with a body that is not a token response")
                .into()
        })
    }

    /// A token of the Portal's own client (client credentials), for a service that answers the
    /// Portal and nobody else, such as `jc-functions` (SDK-23). Kept until 30 s before it expires;
    /// the audience is the realm's mapper on the client, not something the Portal asks for.
    pub async fn service_token(&self) -> Result<String, ApiError> {
        let mut cached = self.service.lock().await;
        let now = std::time::Instant::now();
        if let Some((token, _)) = cached.as_ref().filter(|(_, until)| *until > now) {
            return Ok(token.clone());
        }
        let response = self
            .token_request(&[("grant_type", "client_credentials")])
            .await
            .map_err(|error| {
                tracing::warn!(%error, "the realm refused the Portal's client credentials");
                ApiError::Unavailable("the Portal could not get a token of its own".into())
            })?;
        let token = response.access_token().secret().clone();
        let lifetime = response
            .expires_in()
            .unwrap_or(Duration::from_secs(DEFAULT_ACCESS_TTL_SECS as u64));
        *cached = Some((
            token.clone(),
            now + lifetime.saturating_sub(Duration::from_secs(30)),
        ));
        Ok(token)
    }

    /// The person's `subject` access token exchanged (RFC 8693) for one the gateway takes for
    /// Endpoint `slug` alone: audience the hub, scope `endpoint:{slug}` (ADR-N-025 4b). The
    /// realm judges the subject token live, so a disabled person or a revoked session reads
    /// nothing; the gateway then decides the read on the person's own grants (AG-146).
    pub async fn exchange_for_endpoint(
        &self,
        subject: &str,
        slug: &str,
    ) -> Result<String, ApiError> {
        let scope = format!("{GATEWAY_HUB_SCOPE} endpoint:{slug}");
        let response = self
            .token_request(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
                ("subject_token", subject),
                ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token"),
                ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
                ("audience", GATEWAY_HUB_AUDIENCE),
                ("scope", &scope),
            ])
            .await
            .map_err(|error| {
                tracing::info!(%error, reason = %error.description, endpoint = slug, "the realm refused a data read's token exchange");
                exchange_refused(&error, slug)
            })?;
        Ok(response.access_token().secret().clone())
    }

    /// Trades the session's refresh token for a fresh access token at the realm's token
    /// endpoint. The identity stays; the roles follow the new id token when the realm sends one
    /// and it verifies. Any refusal is `401`: the session is over, only a login brings it back.
    pub async fn refresh(&self, session: &Session) -> Result<Session, ApiError> {
        let refresh_token = session
            .refresh_token
            .as_deref()
            .ok_or(ApiError::Unauthorized)?;
        let token_response = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await
            .map_err(|error| {
                tracing::info!(subject = %session.identity.subject, %error, "token refresh refused");
                ApiError::Unauthorized
            })?;
        let mut identity = session.identity.clone();
        let mut id_token = session.id_token.clone();
        if let Some(fresh) = token_response.id_token() {
            // Verified like the logout token: signature, issuer and audience, no nonce (there
            // was no browser round trip to bind one to). Only a verified token changes anything.
            match fresh.claims(&self.client.id_token_verifier(), |_: Option<&Nonce>| Ok(())) {
                Ok(_) => {
                    id_token = fresh.to_string();
                    identity.roles = realm_roles(&id_token);
                    identity.groups = token_groups(&id_token);
                    // A refresh keeps the factor this session proved: Keycloak need not repeat
                    // `amr` in a refreshed token, and the realm session is the same one.
                    identity.second_factor |= second_factor(&id_token);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "id_token from the refresh did not verify; keeping the old one")
                }
            }
        }
        Ok(mint_session(
            identity,
            id_token,
            session.issued_at,
            &token_response,
        ))
    }
}

/// Builds the cookie session from a token response, at login and at every refresh.
///
/// The access token itself is not kept: the portal calls nothing with it. Its `expires_in` sets
/// the refresh point; the refresh token's own `exp` (Keycloak slides it with the SSO session)
/// sets how long the session lives without a login.
fn mint_session(
    identity: Identity,
    id_token: String,
    issued_at: i64,
    token_response: &openidconnect::core::CoreTokenResponse,
) -> Session {
    let now = session::now_unix();
    let access_ttl = token_response
        .expires_in()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(DEFAULT_ACCESS_TTL_SECS);
    let refresh_token = token_response
        .refresh_token()
        .map(|t| t.secret().clone())
        .filter(|t| !t.is_empty());
    let expires_at = refresh_token
        .as_deref()
        .and_then(jwt_exp)
        .unwrap_or(now + DEFAULT_SESSION_TTL_SECS);
    Session {
        identity,
        expires_at,
        issued_at,
        signed_in_at: None,
        id_token,
        access_expires_at: now + access_ttl,
        refresh_token,
    }
}

/// What the login handler parks in an encrypted cookie until Keycloak redirects back.
#[derive(Serialize, Deserialize)]
struct FlowState {
    pkce_verifier: String,
    csrf_state: String,
    nonce: String,
    /// Where to send the browser afterwards; always a path on this portal.
    redirect_to: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Optional in-app path to return to after the login.
    #[serde(default)]
    pub redirect_to: Option<String>,
    /// The Portal's language, for the realm's sign-in page (`ui_locales`, T-4199).
    #[serde(default)]
    pub lang: Option<String>,
    /// `true` asks the realm for the second factor a decision needs (T-5567): `acr_values`
    /// [`session::STEP_UP_ACR`] and `prompt=login`, so the code is asked even with a live
    /// realm session.
    #[serde(default)]
    pub step_up: bool,
}

/// A language tag the realm may be handed: two or three ASCII letters, lower-cased; anything else
/// is dropped rather than passed into the provider's URL.
fn ui_locale(lang: Option<&str>) -> Option<String> {
    let lang = lang?.trim();
    ((2..=3).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| lang.to_ascii_lowercase())
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// The provider's back-channel logout call: one signed token, no session and no bearer (AP-29).
#[derive(Deserialize, utoipa::ToSchema)]
pub struct BackChannelLogoutForm {
    /// The JWT the provider signed, carrying the `logout_events` claim.
    pub logout_token: String,
}

/// The signed token stays out of every log line (T-4321).
impl std::fmt::Debug for BackChannelLogoutForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackChannelLogoutForm")
            .field("logout_token", &"[redacted]")
            .finish()
    }
}

/// Only same-origin paths are accepted, so an open redirect cannot be smuggled through login.
///
/// This says what a target *is* rather than listing what it is not, because the list was short of the
/// forms a browser accepts (T-2290): a `Location` of `/\evil.example` is scheme-relative once the
/// browser reads the backslash as a path separator (WHATWG URL, special schemes), and a tab or a
/// newline between the two separators is stripped before that parse. So: one leading slash, no second
/// separator of either kind, and no character a URL cannot carry. Anything else silently becomes `/` —
/// a caller is not told which forms are filtered.
fn safe_redirect(candidate: Option<String>) -> String {
    const HOME: &str = "/";
    let Some(path) = candidate else {
        return HOME.to_string();
    };
    let is_separator = |c: char| c == '/' || c == '\\';
    let mut characters = path.chars();
    if !characters.next().is_some_and(is_separator) {
        return HOME.to_string();
    }
    // `/\`, `//`, `/ /`, `/\t/`: whatever the second character is, it may not be a separator, and it
    // may not be whitespace or a control character that a browser drops before parsing — which would
    // make the character after it the second separator.
    if characters
        .next()
        .is_some_and(|c| is_separator(c) || c.is_whitespace() || c.is_control())
    {
        return HOME.to_string();
    }
    // A control character anywhere is a header-splitting attempt in a `Location`, and a backslash
    // anywhere else still normalises to a slash on the way to the browser.
    if path.chars().any(|c| c.is_control() || c == '\\') {
        return HOME.to_string();
    }
    path
}

fn oidc(state: &AppState) -> Result<&OidcClient, ApiError> {
    state
        .oidc
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no identity provider is configured".into()))
}

/// `GET /api/v1/auth/login` — starts the authorization code flow with PKCE.
#[utoipa::path(
    get,
    path = "/api/v1/auth/login",
    tag = "auth",
    params(
        ("redirect_to" = Option<String>, Query,
         description = "An in-app path to return to; honoured only when it is same-origin"),
        ("lang" = Option<String>, Query,
         description = "The Portal's language (2-3 letters), handed to the realm as `ui_locales`"),
        ("step_up" = Option<bool>, Query,
         description = "`true` asks the realm for the second factor a decision needs (`acr_values`, `prompt=login`)")
    ),
    responses(
        (status = 303, description = "Redirect to the provider's authorization endpoint"),
        (status = 503, description = "No identity provider is configured",
         body = crate::error::ProblemDetails)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
    jar: PrivateCookieJar,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        // `openid` is already in the request; the client adds it itself.
        .add_scope(Scope::new("profile".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .set_pkce_challenge(challenge);
    // The sign-in page in the Portal's language: a Slovak Portal led to an English login (T-4199).
    if let Some(locale) = ui_locale(query.lang.as_deref()) {
        request = request.add_extra_param("ui_locales", locale);
    }
    if query.step_up {
        request = request
            .add_extra_param("acr_values", session::STEP_UP_ACR)
            .add_extra_param("prompt", "login");
    }
    let (auth_url, csrf_state, nonce) = request.url();

    let flow = FlowState {
        pkce_verifier: verifier.into_secret(),
        csrf_state: csrf_state.secret().clone(),
        nonce: nonce.secret().clone(),
        redirect_to: safe_redirect(query.redirect_to),
    };
    let value = serde_json::to_string(&flow)
        .map_err(|e| ApiError::Internal(format!("serialize oidc flow: {e}")))?;
    let jar = jar.add(secure_cookie(FLOW_COOKIE, value, FLOW_TTL_SECS));

    Ok((jar, Redirect::to(auth_url.as_str())).into_response())
}

/// The realm's public browser client the silent check asks as: PKCE, any path of the Portal's
/// host (`portal-ui` in the deployment's `components/portal/keycloak-clients.yaml`).
pub const SSO_CHECK_CLIENT: &str = "portal-ui";

/// The Portal page the silent check lands on; its answer is in the fragment.
pub const SSO_CHECK_DONE: &str = "/api/v1/auth/sso-check/done";

/// `GET /api/v1/auth/sso-check` — asks the realm, without showing anything, whether the person's
/// realm session lives (AP-122, T-3034).
///
/// The Open page loads it in a hidden frame of its own origin when a framed App stays silent, and
/// reads the answer off the frame's address once it lands on [`SSO_CHECK_DONE`]: only a realm that
/// says the person must sign in brings the sign-in offer, never the App's silence alone.
#[utoipa::path(
    get,
    path = "/api/v1/auth/sso-check",
    tag = "auth",
    responses(
        (status = 303, description = "Redirect to the realm's authorization endpoint with prompt=none"),
        (status = 503, description = "No identity provider is configured",
         body = crate::error::ProblemDetails)
    )
)]
pub async fn sso_check(State(state): State<AppState>) -> Result<Response, ApiError> {
    let client = oidc(&state)?;
    let url = client.sso_check_url(&state.config.public_base_url);
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Redirect::to(url.as_str()),
    )
        .into_response())
}

/// `GET /api/v1/auth/sso-check/done` — the empty page the silent check lands on (T-3034). The
/// answer is in its fragment, which the browser never sends, so the page carries nothing and runs
/// nothing: the Open page reads the frame's address itself.
#[utoipa::path(
    get,
    path = "/api/v1/auth/sso-check/done",
    tag = "auth",
    responses((status = 200, description = "An empty HTML page", content_type = "text/html"))
)]
pub async fn sso_check_done() -> Response {
    (
        [
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
        ],
        "<!doctype html><meta charset=\"utf-8\"><title>Sign-in check</title>",
    )
        .into_response()
}

/// `GET /api/v1/auth/callback` — validates state and nonce, exchanges the code, mints the session.
#[utoipa::path(
    get,
    path = "/api/v1/auth/callback",
    tag = "auth",
    params(
        ("code" = Option<String>, Query, description = "The authorization code the provider issued"),
        ("state" = Option<String>, Query, description = "The CSRF state minted at login"),
        ("error" = Option<String>, Query, description = "Set when the provider refused the flow"),
        ("error_description" = Option<String>, Query, description = "The provider's own words")
    ),
    responses(
        (status = 303, description = "Session issued; redirect into the application"),
        (status = 400, description = "The provider refused, or state or nonce did not match",
         body = crate::error::ProblemDetails),
        (status = 503, description = "No identity provider is configured",
         body = crate::error::ProblemDetails)
    )
)]
pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    jar: PrivateCookieJar,
    cookies: CookieJar,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;

    if let Some(error) = query.error {
        // The provider's text is echoed only to the login it answers: the state of the flow
        // this browser started. Anyone else's link would make the Portal's origin say what the
        // link's author wrote (F-R2POR3-014).
        let ours = jar
            .get(FLOW_COOKIE)
            .and_then(|cookie| serde_json::from_str::<FlowState>(cookie.value()).ok())
            .is_some_and(|flow| query.state.as_deref() == Some(flow.csrf_state.as_str()));
        if !ours {
            return Err(ApiError::BadRequest(
                "the identity provider refused a login this browser did not start; sign in again"
                    .into(),
            ));
        }
        let detail = query.error_description.unwrap_or(error);
        return Err(ApiError::BadRequest(format!(
            "identity provider rejected the login: {detail}"
        )));
    }

    let flow_cookie = jar
        .get(FLOW_COOKIE)
        .ok_or_else(|| ApiError::BadRequest("no login is in flight".into()))?;
    let flow: FlowState = serde_json::from_str(flow_cookie.value())
        .map_err(|_| ApiError::BadRequest("the login cookie is unreadable".into()))?;
    let jar = jar.remove(session::removal(FLOW_COOKIE));

    let presented_state = query
        .state
        .ok_or_else(|| ApiError::BadRequest("missing state".into()))?;
    if presented_state != flow.csrf_state {
        return Err(ApiError::BadRequest(
            "state does not match the login that was started".into(),
        ));
    }
    let code = query
        .code
        .ok_or_else(|| ApiError::BadRequest("missing code".into()))?;

    let token_response = client
        .token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", client.redirect_uri.as_str()),
            ("code_verifier", flow.pkce_verifier.as_str()),
        ])
        .await
        .map_err(|error| {
            tracing::warn!(%error, "code exchange failed");
            ApiError::BadRequest("the authorization code could not be exchanged".into())
        })?;

    let id_token = token_response
        .id_token()
        .ok_or_else(|| ApiError::BadRequest("the identity provider returned no id_token".into()))?;
    let nonce = Nonce::new(flow.nonce);
    let claims = id_token
        .claims(&client.client.id_token_verifier(), &nonce)
        .map_err(|e| {
            tracing::warn!(error = %e, "id_token verification failed");
            ApiError::BadRequest("the id_token did not verify".into())
        })?;

    let issued_at = session::now_unix();
    let identity = Identity {
        client: None,
        subject: claims.subject().to_string(),
        username: claims
            .preferred_username()
            .map(|u| u.to_string())
            .unwrap_or_else(|| claims.subject().to_string()),
        email: super::session::verified_email(
            claims.email().map(|e| e.to_string()),
            claims.email_verified(),
        ),
        name: claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string()),
        roles: realm_roles(id_token.to_string().as_str()),
        groups: token_groups(id_token.to_string().as_str()),
        second_factor: second_factor(id_token.to_string().as_str()),
    };
    let session = mint_session(identity, id_token.to_string(), issued_at, &token_response);
    tracing::info!(subject = %session.identity.subject, "portal login");

    let jar = session::store(jar, &session)?;
    state.keep_session_claims(&session);
    let (cookies, _token) = csrf::issue(cookies);
    Ok((jar, cookies, Redirect::to(&flow.redirect_to)).into_response())
}

/// The payload of a JWT-shaped string, decoded and nothing more: no signature is checked here,
/// so a caller reads only what it has already verified or what it treats as a hint.
pub(crate) fn jwt_payload<T: serde::de::DeserializeOwned>(token: &str) -> Option<T> {
    let payload = token.split('.').nth(1)?;
    let bytes =
        base64::engine::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
            .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `exp` of a JWT-shaped token; `None` for an opaque one. Keycloak's refresh tokens are JWTs
/// whose `exp` is the SSO session's remaining life, which is exactly the session ceiling.
fn jwt_exp(token: &str) -> Option<i64> {
    #[derive(serde::Deserialize)]
    struct Payload {
        exp: Option<i64>,
    }
    jwt_payload::<Payload>(token)?.exp
}

/// Keycloak puts realm roles in `realm_access.roles`, which `openidconnect`'s core claim set does
/// not model. Reading them back out of the token's payload is safe here and only here: the caller
/// has already verified this exact token's signature, so the bytes are trusted. A malformed or
/// role-less token yields an empty list, never an error — roles are display and enablement only
/// (CC-42), and the resource API enforces the real boundary.
/// The `groups` claim a Keycloak group-membership mapper adds (`/parent/child` paths); the
/// leading `/` goes, so a `RoleBinding` names `air-quality-team`, not `/air-quality-team`.
fn token_groups(id_token: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(default)]
        groups: Vec<String>,
    }
    jwt_payload::<Payload>(id_token)
        .map(|p| {
            p.groups
                .into_iter()
                .map(|g| g.trim_start_matches('/').to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// [`session::proves_second_factor`] over a verified ID token's `acr` and `amr` (T-5567).
fn second_factor(id_token: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Payload {
        acr: Option<String>,
        #[serde(default)]
        amr: serde_json::Value,
    }
    jwt_payload::<Payload>(id_token)
        .is_some_and(|p| session::proves_second_factor(p.acr.as_deref(), &p.amr))
}

fn realm_roles(id_token: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct RealmAccess {
        #[serde(default)]
        roles: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Payload {
        realm_access: Option<RealmAccess>,
    }
    jwt_payload::<Payload>(id_token)
        .and_then(|p| p.realm_access)
        .map(|r| r.roles)
        .unwrap_or_default()
}

/// Who is signed in and through which front, so the UI knows whose logout to call.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Me {
    #[serde(flatten)]
    pub identity: Identity,
    /// `portal` for the Portal's own cookie session, `edge` for the APISIX `openid-connect`
    /// session, `bearer` for a token a service sent (ADR-N-019).
    pub front: session::Front,
}

/// `GET /api/v1/auth/me` — who is signed in.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The signed-in identity and its front", body = Me),
        (status = 401, description = "No live session", body = crate::error::ProblemDetails)
    )
)]
pub async fn me(
    user: crate::auth::CurrentUser,
    front: session::Front,
    jar: CookieJar,
) -> (CookieJar, Json<Me>) {
    (
        csrf_for_edge(front, jar),
        Json(Me {
            identity: user.0.identity,
            front,
        }),
    )
}

/// An edge session never passes the Portal's callback, the one place the double-submit cookie is
/// minted, so the first `/auth/me` of such a session issues it; the check on every mutation stays
/// the same for every front (ADR-N-019, AP-28, PF-50).
fn csrf_for_edge(front: session::Front, jar: CookieJar) -> CookieJar {
    if front == session::Front::Edge && jar.get(csrf::CSRF_COOKIE).is_none() {
        csrf::issue(jar).0
    } else {
        jar
    }
}

/// The URL the browser must visit to finish an RP-initiated logout at Keycloak.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogoutTarget {
    /// Absolute URL: Keycloak's `end_session_endpoint`, or the portal itself when no realm is
    /// configured.
    pub end_session_url: String,
}

/// `POST /api/v1/auth/logout` — clears the session and tells the SPA where to send the browser.
///
/// It answers with JSON rather than a 302 on purpose: the SPA calls this with `fetch`, and a
/// cross-origin redirect to Keycloak is unreadable to it. The browser navigation is the caller's.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 200, description = "Session cleared; navigate to endSessionUrl", body = LogoutTarget),
        (status = 403, description = "Missing or mismatched CSRF token", body = crate::error::ProblemDetails)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
    cookies: CookieJar,
) -> Result<Response, ApiError> {
    // All three cookies go, whether or not a session was loadable: behind the edge this is
    // the first half of a single logout, and the edge's own `/logout` clears none of them
    // (ADR-N-019, AP-29).
    let session = session::load(&jar);
    // Clearing the cookies tells *this* browser to forget the session; a copy taken off the
    // wire or out of a backup would otherwise keep working until the cookie's own expiry,
    // because a portal session is an encrypted cookie and nothing server-side (T-1678, PF-46).
    // The mark is at the session's own `issued_at`, not at `now`: it kills this session and
    // every older one of the same person, leaves a session they started later on another
    // device alone, and can never refuse the login they make next. Ending the sessions on
    // those other devices is the SSO logout at `endSessionUrl`, which comes back as a
    // back-channel logout and marks the subject at `now`.
    if let Some(ref session) = session {
        state
            .revoke_subject(&session.identity.subject, session.issued_at)
            .await;
    }
    let jar = session::clear(jar);
    let cookies = cookies.add(csrf::removal());

    let target = state
        .oidc
        .as_deref()
        .and_then(|client| client.end_session_endpoint.clone())
        .map(|mut url| {
            {
                let mut params = url.query_pairs_mut();
                params.append_pair(
                    "post_logout_redirect_uri",
                    state.config.public_base_url.as_str(),
                );
                if let Some(hint) = session.as_ref().and_then(|s| state.take_id_token(s)) {
                    params.append_pair("id_token_hint", &hint);
                }
                if let Some(oidc) = state.config.oidc.as_ref() {
                    params.append_pair("client_id", &oidc.client_id);
                }
            }
            url.to_string()
        })
        .unwrap_or_else(|| state.config.public_base_url.to_string());

    Ok((
        jar,
        cookies,
        Json(LogoutTarget {
            end_session_url: target,
        }),
    )
        .into_response())
}

/// The Keycloak client of the APISIX login front (components/apisix/keycloak-clients.yaml).
const EDGE_CLIENT: &str = "edge";

/// The member an OpenID Provider puts in `events` to say the token ends a session
/// (OpenID Connect Back-Channel Logout 1.0, Â§2.4).
const LOGOUT_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";

/// Whether this JWT's payload is a logout token rather than an ID token: it carries the
/// back-channel logout event and no `nonce`. The realm signs both, and an ID token travels
/// in the open as `id_token_hint`, so without this check its holder could end the session of
/// whoever it names (AP-29). Read before verification, and only to refuse; the signature is
/// still what admits it.
fn ends_a_session(jwt: &str) -> bool {
    use base64::Engine as _;
    let Some(payload) = jwt.split('.').nth(1) else {
        return false;
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    claims.get("nonce").is_none()
        && claims
            .get("events")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|events| events.contains_key(LOGOUT_EVENT))
}

/// `POST /api/v1/auth/backchannel-logout` — the Keycloak back-channel logout endpoint.
///
/// The logout token is verified against the realm keys before anything is revoked. The call
/// carries no session cookie and no bearer, because the provider makes it server to server:
/// the signed token is the whole authentication, which is why this route stands outside the
/// CSRF guard beside the forge webhook (AP-29).
#[utoipa::path(
    post,
    path = "/api/v1/auth/backchannel-logout",
    tag = "auth",
    request_body(
        content = BackChannelLogoutForm,
        content_type = "application/x-www-form-urlencoded",
        description = "The provider's signed logout token",
        example = json!({ "logout_token": "the-realms-signed-logout-token" })
    ),
    responses(
        (status = 200, description = "Every session issued at or before the token's mark is revoked"),
        (status = 400, description = "The token carries no logout event, or carries a nonce",
         body = crate::error::ProblemDetails),
        (status = 401, description = "The token's signature or issuer does not verify",
         body = crate::error::ProblemDetails),
        (status = 503, description = "No identity provider is configured",
         body = crate::error::ProblemDetails)
    )
)]
pub async fn backchannel_logout(
    State(state): State<AppState>,
    Form(form): Form<BackChannelLogoutForm>,
) -> Result<Response, ApiError> {
    let client = oidc(&state)?;
    if !ends_a_session(&form.logout_token) {
        return Err(ApiError::BadRequest(
            "the token carries no back-channel logout event, or carries a nonce".into(),
        ));
    }
    let token: CoreIdToken = form
        .logout_token
        .parse()
        .map_err(|_| ApiError::BadRequest("the logout token is not a JWT".into()))?;
    let subject = match token.claims(&client.client.id_token_verifier(), |_: Option<&Nonce>| {
        Ok(())
    }) {
        Ok(claims) => claims.subject().as_str().to_owned(),
        // The edge's sessions are the edge client's, so Keycloak addresses their logout token
        // to it (T-5568): signature, issuer, expiry and that audience, and only where an edge
        // is in front at all.
        Err(e) => match (state.config.trust_edge_token, state.bearer.as_ref()) {
            (true, Some(bearer)) => bearer
                .verify_logout_token(&form.logout_token, EDGE_CLIENT)
                .await
                .map_err(|_| {
                    tracing::warn!(error = %e, "logout token verification failed");
                    ApiError::BadRequest("the logout token did not verify".into())
                })?,
            _ => {
                tracing::warn!(error = %e, "logout token verification failed");
                return Err(ApiError::BadRequest(
                    "the logout token did not verify".into(),
                ));
            }
        },
    };

    state.revoke_subject(&subject, session::now_unix()).await;
    tracing::info!(%subject, "back-channel logout");
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/me", get(me))
        .route("/auth/sso-check", get(sso_check))
        .route("/auth/sso-check/done", get(sso_check_done))
        .route("/auth/logout", post(logout))
}

/// The provider's own call, authenticated by the signature on its logout token and by nothing
/// else: no cookie, so no CSRF token either (AP-29, CC-40).
pub fn backchannel_router() -> Router<AppState> {
    Router::new().route("/auth/backchannel-logout", post(backchannel_logout))
}

/// `safe_redirect` for the refresh middleware's test, which pins the target it builds against the
/// guard that reads it back (T-2099, T-2290). It sits here, below every `.route(` of this file, on
/// purpose: the route-coverage guard reads the crate's sources as text and stops at the first
/// top-level `#[cfg(test)]` (`src/ops/mod.rs:2472`), so a test-only item higher up would hide every
/// route under it from that check.
#[cfg(test)]
pub(crate) fn safe_redirect_for_tests(candidate: Option<String>) -> String {
    safe_redirect(candidate)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_logout_form_debug_never_shows_the_token() {
        let form = BackChannelLogoutForm {
            logout_token: "s3cr3t-test".into(),
        };
        let text = format!("{form:?}");
        assert!(
            !text.contains("s3cr3t-test") && text.contains("[redacted]"),
            "{text}"
        );
    }

    #[test]
    fn only_same_origin_paths_are_accepted_as_a_return_target() {
        assert_eq!(
            safe_redirect(Some("/projects/ovzdusie".into())),
            "/projects/ovzdusie"
        );
        assert_eq!(safe_redirect(Some("//evil.example".into())), "/");
        assert_eq!(safe_redirect(Some("https://evil.example".into())), "/");
        assert_eq!(safe_redirect(None), "/");
    }

    #[test]
    fn nothing_a_browser_would_resolve_to_another_origin_is_a_return_target() {
        // A browser resolving `Location: /\evil.example` reads the backslash as a path separator
        // (WHATWG URL, special schemes), so the value is scheme-relative and the person leaves the
        // Portal after logging in to it. Tabs and newlines are stripped before that parse, which is
        // why they cannot sit between the two separators either, and a CR/LF in a `Location` is a
        // header-splitting attempt in its own right (T-2290).
        for forged in [
            "//evil.example",
            "/\\evil.example",
            "/\\/evil.example",
            "\\\\evil.example",
            "\\/evil.example",
            "/\t/evil.example",
            "/\n/evil.example",
            "/\r\nLocation: https://evil.example",
            "/ /evil.example",
            "https://evil.example",
            "http:/evil.example",
            "javascript:alert(1)",
            "data:text/html,x",
            "evil.example",
            "",
        ] {
            assert_eq!(
                safe_redirect(Some(forged.into())),
                "/",
                "{forged:?} was accepted"
            );
        }

        // What a person actually comes back to survives untouched, query and all.
        for allowed in [
            "/",
            "/projects/ovzdusie",
            "/projects/ovzdusie/spaces?page=2&q=pm10%3E30",
            "/projects/ovzdusie#section",
        ] {
            assert_eq!(
                safe_redirect(Some(allowed.into())),
                allowed,
                "{allowed:?} was refused"
            );
        }
    }

    /// Shaped like `openidconnect::DiscoveryError`: the reason is in the source, not in `Display`.
    #[derive(Debug, thiserror::Error)]
    #[error("Request failed")]
    struct Opaque(#[source] std::io::Error);

    #[test]
    fn the_source_chain_survives_into_the_message() {
        let err = Opaque(std::io::Error::other(
            "invalid peer certificate: UnknownIssuer",
        ));
        assert_eq!(
            source_chain(&err),
            "Request failed: invalid peer certificate: UnknownIssuer"
        );
    }

    // ---------------------------------------------------------------------------------------
    // T-2096 `discover`, T-2097 `service_token`, T-2098 `refresh`.
    //
    // All three talk to the realm, so they get a realm: a `wiremock` server that answers discovery
    // and the token endpoint. What is asserted is what the Portal does with what comes back —
    // especially what it does NOT pass on to a caller or keep from a refusal.
    // ---------------------------------------------------------------------------------------

    use wiremock::matchers::{method as wm_method, path as wm_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const REALM: &str = "/realms/bb";

    pub(crate) async fn mock_realm() -> MockServer {
        let server = MockServer::start().await;
        let issuer = format!("{}{REALM}", server.uri().trim_end_matches('/'));
        Mock::given(wm_method("GET"))
            .and(wm_path(format!("{REALM}/.well-known/openid-configuration")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
                "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
                "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            })))
            .mount(&server)
            .await;
        // Discovery fetches the key set as well as the document; a realm that serves one and not
        // the other is not discoverable (this cost me a 404 that read like a wrong path).
        Mock::given(wm_method("GET"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/certs")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "keys": [] })),
            )
            .mount(&server)
            .await;
        server
    }

    pub(crate) fn config_for(server: &MockServer) -> crate::config::OidcConfig {
        let issuer = format!("{}{REALM}", server.uri().trim_end_matches('/'));
        crate::config::Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some(issuer.clone()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t-do-not-leak".to_string()),
            _ => None,
        })
        .expect("config")
        .oidc
        .expect("oidc present")
    }

    fn a_session(refresh_token: Option<&str>) -> Session {
        Session {
            identity: Identity {
                client: None,
                subject: "f:1:demo".into(),
                username: "demo".into(),
                email: None,
                name: None,
                roles: vec!["portal-viewer".into()],
                groups: vec!["editors".into()],
                second_factor: false,
            },
            expires_at: session::now_unix() + 600,
            issued_at: session::now_unix() - 60,
            signed_in_at: None,
            id_token: "opaque-id-token-for-tests".into(),
            access_expires_at: session::now_unix() + 60,
            refresh_token: refresh_token.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn an_empty_ca_bundle_stops_start_up_instead_of_quietly_using_the_public_roots() {
        // The other half of T-0350: a mounted file with no certificate in it must not read as
        // "carry on with the public roots", or an instance behind a private CA fails later as a
        // plain handshake error with nothing pointing at the file.
        let path =
            std::env::temp_dir().join(format!("jc-portal-empty-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"# no certificate here\n").expect("write pem");
        let config = oidc_config(Some(&path.display().to_string()));
        let err = OidcClient::discover(&config, "https://portal.example.test/api/v1/auth/callback")
            .await
            .map(|_| ())
            .expect_err("an empty bundle must stop start-up");
        let _ = std::fs::remove_file(&path);
        match err {
            OidcError::HttpClient(reason) => assert!(
                reason.contains("JC_OIDC_CA_FILE"),
                "the operator needs the variable in the message: {reason}"
            ),
            other => panic!("expected the ca file to be refused, got {other}"),
        }
    }

    #[tokio::test]
    async fn a_realm_that_does_not_answer_fails_discovery_with_the_reason_in_the_message() {
        // Port 9 (discard) refuses at once. The message has to carry the source chain, or an
        // operator reads "Request failed" and learns nothing (that is what `source_chain` is for).
        let config = crate::config::Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("http://127.0.0.1:9/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            _ => None,
        })
        .expect("config")
        .oidc
        .expect("oidc present");
        let err = OidcClient::discover(&config, "https://portal.example.test/api/v1/auth/callback")
            .await
            .map(|_| ())
            .expect_err("an unreachable realm cannot be discovered");
        match err {
            OidcError::Discovery(reason) => {
                assert!(
                    reason.len() > "Request failed".len(),
                    "no reason in: {reason}"
                );
                assert!(
                    reason.to_lowercase().contains("connect")
                        || reason.to_lowercase().contains("refused")
                        || reason.to_lowercase().contains("error"),
                    "the reason does not say what happened: {reason}",
                );
            }
            other => panic!("expected a discovery failure, got {other}"),
        }
    }

    #[tokio::test]
    async fn a_refused_service_token_says_nothing_of_the_realm_or_the_secret() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "unauthorized_client",
                "error_description": "Invalid client secret for portal at https://idm.internal.svc"
            })))
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        let Err(ApiError::Unavailable(message)) = client.service_token().await else {
            panic!("a refused client-credentials grant is Unavailable");
        };
        for leaked in [
            "s3cr3t",
            "unauthorized_client",
            "Invalid client secret",
            "idm.internal",
        ] {
            assert!(
                !message.contains(leaked),
                "the refusal carried {leaked:?}: {message}",
            );
        }
    }

    #[tokio::test]
    async fn a_service_token_is_asked_for_once_and_then_held() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "svc-token-1",
                "token_type": "Bearer",
                "expires_in": 300
            })))
            .expect(1) // one grant for many calls, or the realm is asked on every request
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        for _ in 0..5 {
            assert_eq!(client.service_token().await.expect("token"), "svc-token-1");
        }
        drop(client);
        server.verify().await;
    }

    #[tokio::test]
    async fn an_endpoint_exchange_asks_for_the_hub_and_one_endpoint_and_a_refusal_is_denied() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "endpoint-token",
                "token_type": "Bearer",
                "expires_in": 300
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "invalid_token" })),
            )
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        let token = client
            .exchange_for_endpoint("person-token", "counters-abc")
            .await
            .expect("exchanged");
        assert_eq!(token, "endpoint-token");
        let forms = token_forms(&server).await;
        let field = |key: &str| {
            forms[0]
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            field("grant_type"),
            Some("urn:ietf:params:oauth:grant-type:token-exchange")
        );
        assert_eq!(field("subject_token"), Some("person-token"));
        assert_eq!(field("audience"), Some(GATEWAY_HUB_AUDIENCE));
        // The realm hands out the hub audience only through a scope of the requester (T-6063).
        assert_eq!(field("scope"), Some("gateway-hub endpoint:counters-abc"));

        // A disabled person or a revoked session: the realm refuses, the caller reads nothing.
        let err = client
            .exchange_for_endpoint("revoked-token", "counters-abc")
            .await
            .expect_err("refused");
        assert!(matches!(err, ApiError::Denied(_)), "{err:?}");
    }

    /// What Keycloak 26.8.1 answers a refused exchange (T-6063, reproduced on a local realm):
    /// only an ended session is the person's to fix; a realm not set up says what an
    /// administrator must change and never "sign in again".
    #[test]
    fn an_endpoint_exchange_refusal_names_who_can_fix_it() {
        let refusal = |code: &str, description: &str| TokenRefusal {
            text: format!("the token endpoint answered 400 Bad Request {code}"),
            code: code.into(),
            description: description.into(),
        };
        for ended in [
            refusal("invalid_request", "Invalid token"),
            refusal("invalid_grant", ""),
            refusal("invalid_token", ""),
        ] {
            let err = exchange_refused(&ended, "counters-abc");
            assert!(
                matches!(&err, ApiError::Denied(m) if m.contains("sign in")),
                "{err:?}"
            );
        }
        let err = exchange_refused(
            &refusal("invalid_scope", "Invalid scopes: endpoint:counters-abc"),
            "counters-abc",
        );
        assert!(
            matches!(&err, ApiError::Unavailable(m) if m.contains("`endpoint:counters-abc`")
                && m.contains("within 5 minutes of the Endpoint going Live")
                && m.contains("keycloak-admin-permissions-sync CronJob")
                && !m.contains("a few minutes")),
            "{err:?}"
        );
        for setup in [
            refusal(
                "invalid_request",
                "Standard token exchange is not enabled for the requested client",
            ),
            refusal(
                "invalid_request",
                "Requested audience not available: mcp-hub",
            ),
            refusal("unauthorized_client", ""),
            TokenRefusal::from("the token endpoint did not answer".to_owned()),
        ] {
            let err = exchange_refused(&setup, "counters-abc");
            assert!(
                matches!(&err, ApiError::Unavailable(m) if m.contains("portal-api") && m.contains("gateway-hub") && !m.contains("sign in to")),
                "{err:?}"
            );
        }
    }

    /// The fields of every request the realm's token endpoint received, in order.
    async fn token_forms(server: &MockServer) -> Vec<Vec<(String, String)>> {
        server
            .received_requests()
            .await
            .expect("recorded")
            .into_iter()
            .filter(|request| request.url.path().ends_with("/token"))
            .map(|request| {
                url::form_urlencoded::parse(&request.body)
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect()
            })
            .collect()
    }

    /// A Portal client that proves itself with the projected token in `file` (PF-47).
    fn federated_config_for(
        server: &MockServer,
        file: &std::path::Path,
    ) -> crate::config::OidcConfig {
        let issuer = format!("{}{REALM}", server.uri().trim_end_matches('/'));
        let file = file.display().to_string();
        crate::config::Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some(issuer.clone()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_ASSERTION_FILE" => Some(file.clone()),
            _ => None,
        })
        .expect("config")
        .oidc
        .expect("oidc present")
    }

    #[tokio::test]
    async fn a_federated_client_sends_the_projected_token_and_neither_its_id_nor_a_secret() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "svc-token",
                "refresh_token": "fresh-refresh",
                "token_type": "Bearer",
                "expires_in": 300
            })))
            .mount(&server)
            .await;
        let dir = std::env::temp_dir().join(format!("jc-oidc-assertion-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("token");
        std::fs::write(&file, "eyJ.projected.one\n").expect("token file");
        let client = OidcClient::discover(
            &federated_config_for(&server, &file),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");

        assert_eq!(client.service_token().await.expect("token"), "svc-token");
        // The kubelet rotates the file; the next request reads it again.
        std::fs::write(&file, "eyJ.projected.two").expect("rotate");
        client
            .refresh(&a_session(Some("opaque-refresh")))
            .await
            .expect("refresh");

        let forms = token_forms(&server).await;
        let field = |form: &[(String, String)], key: &str| {
            form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        assert_eq!(forms.len(), 2);
        for (form, (grant, token)) in forms.iter().zip([
            ("client_credentials", "eyJ.projected.one"),
            ("refresh_token", "eyJ.projected.two"),
        ]) {
            assert_eq!(field(form, "grant_type").as_deref(), Some(grant));
            assert_eq!(field(form, "client_assertion").as_deref(), Some(token));
            assert_eq!(
                field(form, "client_assertion_type").as_deref(),
                Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
            );
            assert_eq!(field(form, "client_id"), None, "{form:?}");
            assert_eq!(field(form, "client_secret"), None, "{form:?}");
        }
        assert_eq!(
            field(&forms[1], "refresh_token").as_deref(),
            Some("opaque-refresh")
        );
        let requests = server.received_requests().await.expect("recorded");
        assert!(
            requests
                .iter()
                .all(|request| !request.headers.contains_key("authorization")),
            "no Basic header beside an assertion"
        );
    }

    #[tokio::test]
    async fn a_federated_client_without_its_token_file_asks_the_realm_nothing() {
        let server = mock_realm().await;
        let missing =
            std::env::temp_dir().join(format!("jc-oidc-absent-{}/token", std::process::id()));
        let client = OidcClient::discover(
            &federated_config_for(&server, &missing),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery reads no token");

        assert!(matches!(
            client.service_token().await,
            Err(ApiError::Unavailable(_))
        ));
        assert!(token_forms(&server).await.is_empty());
    }

    #[tokio::test]
    async fn a_secret_client_sends_its_id_and_secret_in_the_body() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "svc-token", "token_type": "Bearer", "expires_in": 300
            })))
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");
        client.service_token().await.expect("token");
        let forms = token_forms(&server).await;
        assert_eq!(
            forms,
            vec![vec![
                ("grant_type".to_owned(), "client_credentials".to_owned()),
                ("client_id".to_owned(), "portal".to_owned()),
                ("client_secret".to_owned(), "s3cr3t-do-not-leak".to_owned()),
            ]]
        );
    }

    #[tokio::test]
    async fn a_session_without_a_refresh_token_is_over_before_the_realm_is_asked() {
        let server = mock_realm().await;
        // No token endpoint is mounted: any request to it would fail the test with a 404 answer
        // turning into 401 as well, so the count is what proves it — wiremock records everything.
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        assert!(matches!(
            client.refresh(&a_session(None)).await,
            Err(ApiError::Unauthorized)
        ));
        let token_calls = server
            .received_requests()
            .await
            .expect("recorded")
            .into_iter()
            .filter(|request| request.url.path().ends_with("/token"))
            .count();
        assert_eq!(
            token_calls, 0,
            "the realm was asked without a refresh token"
        );
    }

    #[tokio::test]
    async fn a_refused_refresh_ends_the_session_with_nothing_of_the_realms_answer() {
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Session not active for user f:1:demo"
            })))
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        // 401 and nothing else: which refusal it was belongs in the log, where the middleware
        // decides whether to clear the cookies (`auth/refresh.rs`).
        let refused = client.refresh(&a_session(Some("opaque-refresh"))).await;
        assert!(
            matches!(refused, Err(ApiError::Unauthorized)),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_refresh_that_brings_no_id_token_keeps_the_roles_the_session_had() {
        // The realm may answer without an id token, and an id token that does not verify is
        // ignored on purpose. Either way the identity must stay exactly as it was: taking roles
        // from an unverified token is how a session grows rights it was never granted.
        let server = mock_realm().await;
        Mock::given(wm_method("POST"))
            .and(wm_path(format!("{REALM}/protocol/openid-connect/token")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-access",
                "refresh_token": "fresh-refresh",
                "token_type": "Bearer",
                "expires_in": 300
            })))
            .mount(&server)
            .await;
        let client = OidcClient::discover(&config_for(&server), "https://portal.test/cb")
            .await
            .expect("discovery");

        let before = a_session(Some("opaque-refresh"));
        let after = client.refresh(&before).await.expect("refresh");
        assert_eq!(after.identity.roles, before.identity.roles);
        assert_eq!(after.identity.groups, before.identity.groups);
        assert_eq!(after.identity.subject, before.identity.subject);
        assert_eq!(after.id_token, before.id_token);
        assert_eq!(
            after.issued_at, before.issued_at,
            "the session keeps its age"
        );
        assert_eq!(after.refresh_token.as_deref(), Some("fresh-refresh"));
        assert!(after.access_expires_at > session::now_unix());
    }

    fn oidc_config(ca_file: Option<&str>) -> crate::config::OidcConfig {
        crate::config::Config::from_vars(|k| match k {
            "JC_OIDC_ISSUER" => Some("https://idm.example.test/realms/bb".to_string()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal".to_string()),
            "JC_OIDC_CLIENT_SECRET" => Some("s3cr3t".to_string()),
            "JC_OIDC_CA_FILE" => ca_file.map(str::to_string),
            _ => None,
        })
        .expect("oidc config")
        .oidc
        .expect("oidc present")
    }

    /// The private root has to reach the http client, and the only failure that proves it does so
    /// without a network is an unusable one: a PEM the builder refuses stops discovery before the
    /// first request. Configured but ignored is exactly the bug this guards (T-0350).
    #[tokio::test]
    async fn an_unusable_extra_root_is_refused_by_name() {
        let path =
            std::env::temp_dir().join(format!("jc-portal-bad-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"not a certificate\n").expect("write pem");
        let config = oidc_config(Some(&path.display().to_string()));
        let err = OidcClient::discover(&config, "https://portal.example.test/api/v1/auth/callback")
            .await
            .map(|_| ())
            .expect_err("a root that cannot be parsed must stop start-up");
        let _ = std::fs::remove_file(&path);
        match err {
            OidcError::HttpClient(reason) => assert!(
                reason.contains("JC_OIDC_CA_FILE"),
                "the operator needs the variable in the message: {reason}"
            ),
            other => panic!("expected the ca file to be refused, got {other}"),
        }
    }
}

#[cfg(test)]
mod realm_roles_tests {
    use super::{jwt_exp, realm_roles, second_factor};
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};

    /// Builds a JWT-shaped string around `payload`. Nothing here is signed: `realm_roles` runs
    /// only on a token the caller already verified, so the test exercises the parsing alone.
    fn token(payload: &str) -> String {
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
            URL_SAFE_NO_PAD.encode(payload.as_bytes()),
            URL_SAFE_NO_PAD.encode(b"not-a-signature"),
        )
    }

    #[test]
    fn reads_realm_access_roles_in_order() {
        let t = token(r#"{"sub":"u1","realm_access":{"roles":["space-editor","portal-viewer"]}}"#);
        assert_eq!(realm_roles(&t), vec!["space-editor", "portal-viewer"]);
    }

    #[test]
    fn missing_or_empty_realm_access_is_no_roles() {
        assert!(realm_roles(&token(r#"{"sub":"u1"}"#)).is_empty());
        assert!(realm_roles(&token(r#"{"sub":"u1","realm_access":{}}"#)).is_empty());
        assert!(realm_roles(&token(r#"{"sub":"u1","realm_access":{"roles":[]}}"#)).is_empty());
    }

    #[test]
    fn the_refresh_token_exp_is_the_session_ceiling() {
        assert_eq!(
            jwt_exp(&token(r#"{"exp":1800000000}"#)),
            Some(1_800_000_000)
        );
        assert_eq!(jwt_exp(&token(r#"{"sub":"u1"}"#)), None);
        assert_eq!(jwt_exp("opaque-refresh-token"), None);
    }

    /// T-5567: the ID token of a login says whether it asked the one-time code.
    #[test]
    fn the_id_token_proves_a_second_factor_by_amr_otp_or_the_step_up_acr() {
        assert!(second_factor(&token(r#"{"acr":"mfa","amr":[]}"#)));
        assert!(second_factor(&token(r#"{"acr":"pwd","amr":["otp"]}"#)));
        assert!(!second_factor(&token(r#"{"acr":"pwd","amr":["pwd"]}"#)));
        assert!(!second_factor(&token(r#"{"acr":"1"}"#)));
        assert!(!second_factor(&token(r#"{"sub":"u1"}"#)));
        assert!(!second_factor("not-a-token"));
    }

    #[test]
    fn a_malformed_token_yields_no_roles_instead_of_an_error() {
        assert!(realm_roles("").is_empty());
        assert!(realm_roles("only-one-segment").is_empty());
        assert!(realm_roles("header.!!!not-base64!!!.sig").is_empty());
        assert!(realm_roles(&token("not json at all")).is_empty());
    }
}
