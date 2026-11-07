//! The smallest Kubernetes client that can converge an app's objects (T-0411, AP-13, AP-18,
//! AP-21, AP-26).
//!
//! The Portal writes four kinds into one namespace and nothing else, so this is a hand-written
//! client over the `reqwest` the Portal already carries rather than a Kubernetes client
//! library: server-side apply is one `PATCH` with a content type, and a delete is a `DELETE`.
//! The dependency a library would add is larger than the code it would replace, and this way
//! the set of reachable objects is a `match` in one file.
//!
//! That match is a security control, not a convenience. A manifest that asked for a
//! `ClusterRoleBinding` is refused here, before the request is built, so the Portal's RBAC is
//! the second line of defence rather than the first (AP-13).
//!
//! The credential is a projected ServiceAccount token, which the kubelet rotates, so it is read
//! from disk per request instead of once at startup; a token cached for the lifetime of the pod
//! starts answering 401 after an hour.

use std::path::{Path, PathBuf};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Certificate, StatusCode};
use serde_json::Value;
use url::Url;

/// Where the kubelet mounts the pod's ServiceAccount.
pub(crate) const SERVICE_ACCOUNT_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

/// The in-cluster API server, whose certificate the mounted authority signs.
pub(crate) const IN_CLUSTER_API: &str = "https://kubernetes.default.svc";

/// The field manager every object the app reconciler owns is applied under (CC-18).
///
/// Server-side apply keeps one manager's fields separate from another's, so a field this
/// manager stops sending is removed, and a hand edit by a person shows up as a conflict that
/// `force` resolves in the reconciler's favour rather than as a silent divergence.
pub const FIELD_MANAGER: &str = "portal-app-reconciler";

/// Server-side apply's content type; the body is JSON, which is YAML.
const APPLY_PATCH: &str = "application/apply-patch+yaml";

/// The kinds an App, its agent runs and its build pods compile into (AP-13, AP-15, AP-26, AG-33,
/// EP-02, AP-131).
const KINDS: [(&str, &str, &str); 11] = [
    ("apps/v1", "Deployment", "deployments"),
    ("v1", "Service", "services"),
    ("v1", "Secret", "secrets"),
    ("networking.k8s.io/v1", "NetworkPolicy", "networkpolicies"),
    ("batch/v1", "Job", "jobs"),
    ("v1", "ServiceAccount", "serviceaccounts"),
    ("v1", "PersistentVolumeClaim", "persistentvolumeclaims"),
    // A project's apps namespace and the Portal's binding in it (AP-116); an admission policy
    // keeps both to `{release}-{project}-apps` (AP-117).
    ("v1", "Namespace", "namespaces"),
    (
        "rbac.authorization.k8s.io/v1",
        "RoleBinding",
        "rolebindings",
    ),
    // An App's host: its edge Ingress and its certificate, in the APISIX namespace (AP-133).
    ("networking.k8s.io/v1", "Ingress", "ingresses"),
    ("cert-manager.io/v1", "Certificate", "certificates"),
];

/// The one kind the Portal creates and deletes but never applies: the ConfigMap that carries a
/// version into its test sandbox (SDK-38), where the Portal's Role grants `create` and `delete`.
const CREATED_ONLY: (&str, &str, &str) = ("v1", "ConfigMap", "configmaps");

/// What one MQTT edge broker's release is made of (ADR-N-048 §2.6, T-3994), reachable only through
/// the `*_mqtt` methods: in the brokers' namespace, by a name that starts `mqtt-`, so a template
/// that asked for anything else is refused before a request is made.
const MQTT_KINDS: [(&str, &str, &str); 11] = [
    ("v1", "ServiceAccount", "serviceaccounts"),
    ("v1", "ConfigMap", "configmaps"),
    ("v1", "Service", "services"),
    ("apps/v1", "Deployment", "deployments"),
    ("cert-manager.io/v1", "Certificate", "certificates"),
    ("traefik.io/v1alpha1", "IngressRouteTCP", "ingressroutetcps"),
    ("traefik.io/v1alpha1", "MiddlewareTCP", "middlewaretcps"),
    ("policy.linkerd.io/v1beta3", "Server", "servers"),
    ("policy.linkerd.io/v1beta3", "HTTPRoute", "httproutes"),
    (
        "policy.linkerd.io/v1alpha1",
        "MeshTLSAuthentication",
        "meshtlsauthentications",
    ),
    (
        "policy.linkerd.io/v1alpha1",
        "AuthorizationPolicy",
        "authorizationpolicies",
    ),
];

/// The one kind of [`KINDS`] that lives outside every namespace.
const CLUSTER_SCOPED: &str = "namespaces";

/// Why a request to the API server did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    /// The object names a kind or an API group the Portal is not allowed to write.
    #[error("the Portal does not write {api_version} {kind}")]
    UnsupportedKind {
        /// The `apiVersion` of the object.
        api_version: String,
        /// The `kind` of the object.
        kind: String,
    },
    /// The object has no name, no namespace, or one that is not a DNS label.
    #[error("{field} is not a name the API server would accept: {value}")]
    NotAName {
        /// Which of the two names is wrong.
        field: &'static str,
        /// The value as written.
        value: String,
    },
    /// The API server answered, and refused.
    #[error("the API server answered {status} for {path}: {message}")]
    Api {
        /// The HTTP status.
        status: u16,
        /// The path that was called, which never carries a credential.
        path: String,
        /// The `message` of the Status object, or the body when it is not one.
        message: String,
    },
    /// The API server could not be reached.
    #[error("cannot reach the API server: {0}")]
    Transport(String),
    /// The ServiceAccount mount is not readable, so there is no identity to act as.
    #[error("cannot read {path}: {source}")]
    ServiceAccount {
        /// The file that could not be read.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// The base URL or the certificate authority is not usable.
    #[error("the API server client cannot be built: {0}")]
    Client(String),
}

/// Where the bearer token comes from.
#[derive(Debug, Clone)]
enum TokenSource {
    /// A projected ServiceAccount token, re-read per request because the kubelet rotates it.
    File(PathBuf),
    /// A fixed token, which is how the tests speak to a stub API server.
    Fixed(String),
}

/// A client for the four kinds the app reconciler owns, in one namespace at a time.
pub struct KubeClient {
    http: reqwest::Client,
    base: Url,
    token: TokenSource,
}

// The token never reaches a log, an error or a status page.
impl std::fmt::Debug for KubeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeClient")
            .field("base", &self.base.as_str())
            .field("token", &"redacted")
            .finish()
    }
}

impl KubeClient {
    /// The client a pod builds from its own ServiceAccount, or `None` outside a cluster.
    ///
    /// A Portal running on a laptop has no token mount and no cluster to converge, and that is
    /// not an error: it serves the UI and the resource API and applies nothing.
    pub fn in_cluster() -> Result<Option<Self>, KubeError> {
        Self::from_mount(Path::new(SERVICE_ACCOUNT_DIR), IN_CLUSTER_API)
    }

    /// The namespace this pod runs in, as the kubelet writes it beside the token.
    ///
    /// The Portal writes a Secret into its own namespace and nowhere else, so the namespace is
    /// read from the mount rather than configured: a variable could name a namespace this pod
    /// has no rights in, and the answer would be a refusal nobody expected.
    pub fn own_namespace() -> Option<String> {
        Self::namespace_of(Path::new(SERVICE_ACCOUNT_DIR))
    }

    /// [`KubeClient::own_namespace`] against a mount named explicitly, for tests.
    pub fn namespace_of(dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join("namespace"))
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
    }

    /// [`KubeClient::in_cluster`] against a mount and an API server named explicitly, which is
    /// what makes the in-cluster path testable at all.
    pub fn from_mount(dir: &Path, api: &str) -> Result<Option<Self>, KubeError> {
        let token = dir.join("token");
        if !token.is_file() {
            return Ok(None);
        }
        let authority = dir.join("ca.crt");
        let ca = match authority.is_file() {
            true => {
                Some(
                    std::fs::read(&authority).map_err(|source| KubeError::ServiceAccount {
                        path: authority,
                        source,
                    })?,
                )
            }
            false => None,
        };
        let base = Url::parse(api).map_err(|err| KubeError::Client(err.to_string()))?;
        Self::build(base, TokenSource::File(token), ca.as_deref()).map(Some)
    }

    /// A client against a named API server with a fixed token, for tests.
    pub fn with_token(api: &str, token: impl Into<String>) -> Result<Self, KubeError> {
        let base = Url::parse(api).map_err(|err| KubeError::Client(err.to_string()))?;
        Self::build(base, TokenSource::Fixed(token.into()), None)
    }

    fn build(base: Url, token: TokenSource, ca: Option<&[u8]>) -> Result<Self, KubeError> {
        // A hung API server costs one reconcile an error, never the loop (T-1716). Every call
        // here is a single get, apply or delete; nothing watches or streams.
        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30));
        if let Some(pem) = ca {
            let certificate =
                Certificate::from_pem(pem).map_err(|err| KubeError::Client(err.to_string()))?;
            builder = builder.add_root_certificate(certificate);
        }
        let http = builder
            .build()
            .map_err(|err| KubeError::Client(err.to_string()))?;
        Ok(Self { http, base, token })
    }

    /// Applies one object, creating it or bringing it back to what the manifest says (CC-18).
    pub async fn apply(&self, object: &Value) -> Result<(), KubeError> {
        let plural = object_kind(object)?;
        let namespace = if plural == CLUSTER_SCOPED {
            ""
        } else {
            namespace_of(object)?
        };
        let path = path_of(plural, namespace, name_of(object)?)?;
        let url = self.url(&format!("{path}?fieldManager={FIELD_MANAGER}&force=true"))?;
        let response = self
            .http
            .patch(url)
            .headers(self.headers(APPLY_PATCH)?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, path).await.map(|_| ())
    }

    /// Applies a WASM shard's placement ConfigMap (AP-157), the one ConfigMap the Portal applies:
    /// any other kind, or a ConfigMap of any other name, is refused before a request is made, so
    /// this does not widen what [`KubeClient::apply`] may write.
    pub async fn apply_placement(&self, object: &Value) -> Result<(), KubeError> {
        let (api_version, kind, plural) = CREATED_ONLY;
        let field = |name: &str| object.get(name).and_then(Value::as_str);
        if field("apiVersion") != Some(api_version) || field("kind") != Some(kind) {
            return Err(KubeError::UnsupportedKind {
                api_version: field("apiVersion").unwrap_or_default().to_owned(),
                kind: field("kind").unwrap_or_default().to_owned(),
            });
        }
        let name = name_of(object)?;
        if !is_placement_name(name) {
            return Err(KubeError::NotAName {
                field: "metadata.name",
                value: name.to_owned(),
            });
        }
        let path = path_of(plural, namespace_of(object)?, name)?;
        let url = self.url(&format!("{path}?fieldManager={FIELD_MANAGER}&force=true"))?;
        let response = self
            .http
            .patch(url)
            .headers(self.headers(APPLY_PATCH)?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, path).await.map(|_| ())
    }

    /// Creates one object that must not exist yet, for a namespace whose Role grants `create` and
    /// not `patch` (SDK-38): a name already taken is the API server's `409`.
    pub async fn create(&self, object: &Value) -> Result<(), KubeError> {
        let plural = created_or_applied(object_kind(object), object)?;
        let namespace = namespace_of(object)?;
        let path = path_of(plural, namespace, name_of(object)?)?;
        let collection = path
            .rsplit_once('/')
            .map(|(collection, _)| collection.to_owned())
            .unwrap_or_default();
        let response = self
            .http
            .post(self.url(&collection)?)
            .headers(self.headers("application/json")?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, collection).await.map(|_| ())
    }

    /// The names of the pods in `namespace` a label selector picks, such as a Job's
    /// `job-name={name}` (SDK-38).
    pub async fn pod_names(
        &self,
        namespace: &str,
        label_selector: &str,
    ) -> Result<Vec<String>, KubeError> {
        jc_core::names::validate_dns1123_label(namespace).map_err(|_| KubeError::NotAName {
            field: "metadata.namespace",
            value: namespace.to_owned(),
        })?;
        let path = format!("/api/v1/namespaces/{namespace}/pods");
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("labelSelector", label_selector);
        let response = self
            .http
            .get(url)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        let list = self.checked(response, path).await?;
        Ok(list["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pod| pod["metadata"]["name"].as_str().map(str::to_owned))
            .collect())
    }

    /// The end of one pod's log, at most `limit_bytes` of it, as text (SDK-38).
    pub async fn pod_log(
        &self,
        namespace: &str,
        pod: &str,
        limit_bytes: u64,
    ) -> Result<String, KubeError> {
        let path = format!("{}/log", path_of("pods", namespace, pod)?);
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("limitBytes", &limit_bytes.to_string())
            .append_pair("tailLines", "200");
        let response = self
            .http
            .get(url)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if status.is_success() {
            return Ok(body);
        }
        Err(KubeError::Api {
            status: status.as_u16(),
            path,
            message: serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
                .unwrap_or(body),
        })
    }

    /// A short token of the ServiceAccount `account` in `namespace`, for `audience` alone
    /// (TokenRequest). The Portal proves its second Keycloak client with one (PF-47, T-2868):
    /// its pod runs as one account, Keycloak finds a federated client by the token's subject,
    /// and the reconciler's client must be a subject of its own. The Role grants `create` on
    /// `serviceaccounts/token` of that one name. The token is returned and never kept.
    pub async fn service_account_token(
        &self,
        namespace: &str,
        account: &str,
        audience: &str,
        seconds: u32,
    ) -> Result<String, KubeError> {
        let path = format!("{}/token", path_of("serviceaccounts", namespace, account)?);
        let request = serde_json::json!({
            "apiVersion": "authentication.k8s.io/v1",
            "kind": "TokenRequest",
            "spec": { "audiences": [audience], "expirationSeconds": seconds },
        });
        let response = self
            .http
            .post(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .body(request.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        let minted = self.checked(response, path.clone()).await?;
        minted["status"]["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .ok_or(KubeError::Api {
                status: 200,
                path,
                message: "the TokenRequest answer carried no token".to_owned(),
            })
    }

    /// Removes one object; removing what is not there succeeds, so a retirement is re-runnable.
    /// Its dependents go with it in the background: a Job's pods included, which the API server
    /// would otherwise orphan.
    pub async fn delete(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<(), KubeError> {
        let object = serde_json::json!({ "apiVersion": api_version, "kind": kind });
        let plural = created_or_applied(plural_of(api_version, kind), &object)?;
        let path = path_of(plural, namespace, name)?;
        // A Job deleted over the API orphans its pods unless asked otherwise; kubectl asks for
        // the same background collection.
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("propagationPolicy", "Background");
        let response = self
            .http
            .delete(url)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.checked(response, path).await.map(|_| ())
    }

    /// Reads one object, or `None` when it does not exist.
    pub async fn get(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<Option<Value>, KubeError> {
        let path = path_of(plural_of(api_version, kind)?, namespace, name)?;
        let response = self
            .http
            .get(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = self.checked(response, path).await?;
        Ok(Some(body))
    }

    /// The objects of one kind in a namespace that carry `label_selector`, as the API server
    /// lists them; one page, because what the Portal lists is bounded by what it created.
    pub async fn list(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        label_selector: &str,
    ) -> Result<Vec<Value>, KubeError> {
        let plural = plural_of(api_version, kind)?;
        let item = path_of(plural, namespace, "list")?;
        let path = item.trim_end_matches("/list").to_owned();
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("labelSelector", label_selector);
        let response = self
            .http
            .get(url)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        let body = self.checked(response, path).await?;
        Ok(body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Reads one ConfigMap, or `None` when it does not exist. The one kind the Portal reads and
    /// never writes: the base of the edge file (ADR-N-030, AP-112), so it stays out of the kinds
    /// [`KubeClient::apply`] accepts.
    pub async fn get_config_map(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Option<Value>, KubeError> {
        let path = path_of("configmaps", namespace, name)?;
        let response = self
            .http
            .get(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        self.checked(response, path).await.map(Some)
    }

    /// Applies one object of an MQTT broker's release (T-3994): one of `MQTT_KINDS`, in
    /// `namespace`, named `mqtt-…`; anything else is refused before a request is made.
    pub async fn apply_mqtt(&self, namespace: &str, object: &Value) -> Result<(), KubeError> {
        let field = |name: &str| object.get(name).and_then(Value::as_str).unwrap_or_default();
        let plural = mqtt_plural(field("apiVersion"), field("kind"))?;
        let name = mqtt_name(name_of(object)?)?;
        let in_namespace = namespace_of(object)?;
        if in_namespace != namespace {
            return Err(KubeError::NotAName {
                field: "metadata.namespace",
                value: in_namespace.to_owned(),
            });
        }
        let path = path_of(plural, namespace, name)?;
        let url = self.url(&format!("{path}?fieldManager={FIELD_MANAGER}&force=true"))?;
        let response = self
            .http
            .patch(url)
            .headers(self.headers(APPLY_PATCH)?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, path).await.map(|_| ())
    }

    /// The objects of one of `MQTT_KINDS` in `namespace` that carry `label_selector`.
    pub async fn list_mqtt(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        label_selector: &str,
    ) -> Result<Vec<Value>, KubeError> {
        let path = path_of(mqtt_plural(api_version, kind)?, namespace, "list")?
            .trim_end_matches("/list")
            .to_owned();
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("labelSelector", label_selector);
        let response = self
            .http
            .get(url)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        let body = self.checked(response, path).await?;
        Ok(body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Removes one object of an MQTT broker's release; one that is not there is removed already.
    pub async fn delete_mqtt(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<(), KubeError> {
        let path = path_of(mqtt_plural(api_version, kind)?, namespace, mqtt_name(name)?)?;
        let response = self
            .http
            .delete(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.checked(response, path).await.map(|_| ())
    }

    /// Replaces one object that exists, as read with [`KubeClient::get`]: its
    /// `metadata.resourceVersion` makes a write that raced another one fail with `409` instead
    /// of overwriting it. For an object whose RBAC grants `update` and not `patch`.
    pub async fn replace(&self, object: &Value) -> Result<(), KubeError> {
        let path = path_of(
            object_kind(object)?,
            namespace_of(object)?,
            name_of(object)?,
        )?;
        let response = self
            .http
            .put(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, path).await.map(|_| ())
    }

    fn url(&self, path: &str) -> Result<Url, KubeError> {
        self.base
            .join(path)
            .map_err(|err| KubeError::Client(err.to_string()))
    }

    fn headers(&self, content_type: &str) -> Result<HeaderMap, KubeError> {
        let token = match &self.token {
            TokenSource::Fixed(token) => token.clone(),
            TokenSource::File(path) => std::fs::read_to_string(path)
                .map_err(|source| KubeError::ServiceAccount {
                    path: path.clone(),
                    source,
                })?
                .trim()
                .to_owned(),
        };
        // A mount the kubelet has not filled yet, or a truncated write, would send a bare
        // `Bearer`, which the API server reads as `system:anonymous` (T-2566, CC-06).
        if token.trim().is_empty() {
            return Err(match &self.token {
                TokenSource::File(path) => KubeError::ServiceAccount {
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the ServiceAccount token is empty",
                    ),
                },
                TokenSource::Fixed(_) => KubeError::Client("the bearer token is empty".into()),
            });
        }
        let mut headers = HeaderMap::new();
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|err| KubeError::Client(err.to_string()))?;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .map_err(|err| KubeError::Client(err.to_string()))?,
        );
        Ok(headers)
    }

    async fn checked(&self, response: reqwest::Response, path: String) -> Result<Value, KubeError> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(serde_json::from_str(&body).unwrap_or(Value::Null));
        }
        // A Status object says why in one sentence; anything else is passed on as it came, and
        // neither carries the token, which is a header.
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
            .unwrap_or(body);
        Err(KubeError::Api {
            status: status.as_u16(),
            path,
            message,
        })
    }
}

/// The `apiVersion` and `kind` of an object, as the plural path segment they address.
/// `jc-wasm-host-<shard>-placements`, the shard a decimal number (AP-157).
fn is_placement_name(name: &str) -> bool {
    name.strip_prefix("jc-wasm-host-")
        .and_then(|rest| rest.strip_suffix("-placements"))
        .is_some_and(|shard| !shard.is_empty() && shard.bytes().all(|b| b.is_ascii_digit()))
}

fn object_kind(object: &Value) -> Result<&'static str, KubeError> {
    let api_version = object
        .get("apiVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    plural_of(api_version, kind)
}

/// [`plural_of`], widened by the one kind the Portal creates and deletes and never applies.
fn created_or_applied(
    applied: Result<&'static str, KubeError>,
    object: &Value,
) -> Result<&'static str, KubeError> {
    applied.or_else(|err| {
        let (api_version, kind, plural) = CREATED_ONLY;
        let field = |name: &str| object.get(name).and_then(Value::as_str);
        if field("apiVersion") == Some(api_version) && field("kind") == Some(kind) {
            Ok(plural)
        } else {
            Err(err)
        }
    })
}

/// The path segment one kind is addressed by, and the proof that the Portal may write it.
fn plural_of(api_version: &str, kind: &str) -> Result<&'static str, KubeError> {
    KINDS
        .iter()
        .find(|(group, name, _)| *group == api_version && *name == kind)
        .map(|(_, _, plural)| *plural)
        .ok_or_else(|| KubeError::UnsupportedKind {
            api_version: api_version.to_owned(),
            kind: kind.to_owned(),
        })
}

/// The path segment of one of `MQTT_KINDS`, and the proof that a broker's release may hold it.
fn mqtt_plural(api_version: &str, kind: &str) -> Result<&'static str, KubeError> {
    MQTT_KINDS
        .iter()
        .find(|(group, name, _)| *group == api_version && *name == kind)
        .map(|(_, _, plural)| *plural)
        .ok_or_else(|| KubeError::UnsupportedKind {
            api_version: api_version.to_owned(),
            kind: kind.to_owned(),
        })
}

/// A broker's objects are all named `mqtt-…` (`jc_core::kinds::mqtt_broker::release_name`).
fn mqtt_name(name: &str) -> Result<&str, KubeError> {
    if name.starts_with("mqtt-") {
        Ok(name)
    } else {
        Err(KubeError::NotAName {
            field: "metadata.name",
            value: name.to_owned(),
        })
    }
}

fn namespace_of(object: &Value) -> Result<&str, KubeError> {
    object
        .get("metadata")
        .and_then(|meta| meta.get("namespace"))
        .and_then(Value::as_str)
        .ok_or(KubeError::NotAName {
            field: "metadata.namespace",
            value: String::new(),
        })
}

fn name_of(object: &Value) -> Result<&str, KubeError> {
    object
        .get("metadata")
        .and_then(|meta| meta.get("name"))
        .and_then(Value::as_str)
        .ok_or(KubeError::NotAName {
            field: "metadata.name",
            value: String::new(),
        })
}

/// The API path of one object, with both names checked before they become path segments.
///
/// A name is a DNS label everywhere in Kubernetes, so checking it here costs nothing and closes
/// the only way a manifest could reach a path the Portal was not meant to call. A namespace is
/// addressed by its name alone, and the namespace argument is ignored for it.
fn path_of(plural: &str, namespace: &str, name: &str) -> Result<String, KubeError> {
    if plural == CLUSTER_SCOPED {
        jc_core::names::validate_dns1123_label(name).map_err(|_| KubeError::NotAName {
            field: "metadata.name",
            value: name.to_owned(),
        })?;
        return Ok(format!("/api/v1/namespaces/{name}"));
    }
    for (field, value) in [("metadata.namespace", namespace), ("metadata.name", name)] {
        jc_core::names::validate_dns1123_label(value).map_err(|_| KubeError::NotAName {
            field,
            value: value.to_owned(),
        })?;
    }
    let group = KINDS
        .iter()
        .chain(MQTT_KINDS.iter())
        .find(|(_, _, candidate)| *candidate == plural)
        .map(|(group, _, _)| *group)
        .unwrap_or("v1");
    let prefix = match group {
        "v1" => "/api/v1".to_owned(),
        group => format!("/apis/{group}"),
    };
    Ok(format!("{prefix}/namespaces/{namespace}/{plural}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SDK-38: the sandbox's ConfigMap is created and deleted, never applied, so a manifest that
    /// compiles into one is still refused by `apply`.
    #[test]
    fn a_config_map_is_created_and_never_applied() {
        let config_map = serde_json::json!({ "apiVersion": "v1", "kind": "ConfigMap" });
        assert!(matches!(
            object_kind(&config_map),
            Err(KubeError::UnsupportedKind { .. })
        ));
        assert_eq!(
            created_or_applied(object_kind(&config_map), &config_map).ok(),
            Some("configmaps")
        );
        let role =
            serde_json::json!({ "apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role" });
        assert!(created_or_applied(object_kind(&role), &role).is_err());
    }

    /// T-3994: a broker's release reaches its eleven kinds by an `mqtt-` name and nothing else,
    /// and none of the five it adds becomes reachable by `apply`.
    #[test]
    fn a_brokers_kinds_are_its_own_and_named_mqtt() {
        assert_eq!(
            path_of(
                mqtt_plural("traefik.io/v1alpha1", "IngressRouteTCP").expect("allowed"),
                "jc-mqtt",
                "mqtt-helsinki-cameras"
            )
            .expect("a path"),
            "/apis/traefik.io/v1alpha1/namespaces/jc-mqtt/ingressroutetcps/mqtt-helsinki-cameras"
        );
        assert_eq!(
            path_of("configmaps", "jc-mqtt", "mqtt-a").expect("a path"),
            "/api/v1/namespaces/jc-mqtt/configmaps/mqtt-a"
        );
        assert!(mqtt_plural("rbac.authorization.k8s.io/v1", "Role").is_err());
        assert!(mqtt_plural("v1", "Secret").is_err());
        assert!(mqtt_name("mqtt-helsinki-cameras").is_ok());
        assert!(mqtt_name("portal").is_err());
        let route =
            serde_json::json!({ "apiVersion": "traefik.io/v1alpha1", "kind": "IngressRouteTCP" });
        assert!(matches!(
            object_kind(&route),
            Err(KubeError::UnsupportedKind { .. })
        ));
    }

    /// T-2521, CC-06: the bearer is a sensitive header, so a trace of the request or a debug
    /// print of the headers shows `Sensitive`, and the client's own debug print redacts it.
    #[test]
    fn the_bearer_header_is_marked_sensitive_and_a_debug_print_never_shows_the_token() {
        let dir = std::env::temp_dir().join(format!("t2521-sensitive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a mount");
        std::fs::write(dir.join("token"), "eyJ-secret-token\n").expect("a token");
        let mounted = KubeClient::from_mount(&dir, "https://10.0.0.1:443")
            .expect("built")
            .expect("a token file");
        let fixed =
            KubeClient::with_token("https://10.0.0.1:443", "eyJ-secret-token").expect("built");
        for client in [&mounted, &fixed] {
            let headers = client.headers("application/json").expect("headers");
            let bearer = &headers[AUTHORIZATION];
            assert!(bearer.is_sensitive());
            assert!(
                !format!("{headers:?}").contains("eyJ-secret-token"),
                "{headers:?}"
            );
            assert!(
                !format!("{client:?}").contains("eyJ-secret-token"),
                "{client:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
