//! The certificate and edge Ingress of every registry host on its way to `served`
//! (evidence/T-7129/design.md §3, T-7138).
//!
//! A host an operator's `HostAdd` approved (`approved`) moves to `dns-ok` once its name resolves
//! to an address of the edge. This wave then creates, in the edge's namespace, an Ingress
//! `jc-org-…` that carries only `/.well-known/acme-challenge/` and no TLS, and a cert-manager
//! `Certificate` of the same name by HTTP-01 from the edge certificate's issuer (or the dns01
//! issuer the installation names). jc's table does not hold the host yet: it answers `421`.
//! When cert-manager says the certificate is issued the host is `cert-issued`; the wave widens
//! the Ingress to `/` with its TLS Secret and marks the host `served`, and the edge file routes
//! it on its next write. Registry hosts keep one Certificate each, never a fold.
//!
//! New certificates are paced: at most [`PER_INSTANCE_DAY`] in a rolling 24 hours per
//! instance, and [`PER_DOMAIN_WEEK`] in a rolling 7 days per registered domain (the eTLD+1 of
//! the public suffix list), counted in `organization_host_certificates`; a host past either
//! waits and says until when. An object of that name the Portal did not label is never changed
//! or deleted: the host says `conflict`. A `removed` host loses its Ingress, then its
//! Certificate, only once the edge file no longer routes it. Every step writes one row of
//! `operator_audit` and its log line.
//!
//! The rights are the Role `edge-file-composer` (get, list, create, delete; no update, no
//! watch), so widening is a delete and a create, and the deployment's Kyverno policy holds the
//! names, labels, Secret, issuer and backend of every object this wave sends.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;

use jc_time::TimestampTz;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::app_hosts::{
    named_certificate, named_ingress, said, state_of, HostState, EDGE_CERTIFICATE, EDGE_INGRESS,
};
use crate::apps::kube::KubeClient;
use crate::domain_verification::Unanswered;
use crate::host_registry::HostRow;

const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
const MANAGED_VALUE: &str = "joinedcontext-portal";
const COMPONENT: &str = "app.kubernetes.io/component";
/// The component label of a registry host's objects.
pub const COMPONENT_VALUE: &str = "org-host";
const CERTIFICATE_API: &str = "cert-manager.io/v1";
const INGRESS_API: &str = "networking.k8s.io/v1";
/// The one path a host's Ingress carries until its certificate is issued.
pub const ACME_PATH: &str = "/.well-known/acme-challenge/";
/// New certificates per instance in a rolling 24 hours.
pub const PER_INSTANCE_DAY: usize = 10;
/// New certificates per registered domain and instance in a rolling 7 days.
pub const PER_DOMAIN_WEEK: usize = 20;
/// Who the audit names for a step of this wave.
pub const ACTOR: &str = "reconciler";

/// The name of a registry host's Ingress and Certificate: `jc-org-` and the host with dashes
/// for dots, cut so the whole stays a label value (63), then a hash of the host, so `a-b.c` and
/// `a.b-c` never share a name.
pub fn object_name(host: &str) -> String {
    let dashed: String = host.replace('.', "-").chars().take(47).collect();
    format!(
        "jc-org-{}-{}",
        dashed.trim_end_matches('-'),
        super::edge_file::hostname_key(host)
    )
}

fn labels() -> Value {
    json!({ MANAGED_BY: MANAGED_VALUE, COMPONENT: COMPONENT_VALUE })
}

/// Whether this wave labelled `object` as a registry host's.
pub fn managed(object: &Value) -> bool {
    let labels = &object["metadata"]["labels"];
    labels[MANAGED_BY] == MANAGED_VALUE && labels[COMPONENT] == COMPONENT_VALUE
}

/// The certificate of one registry host.
pub fn certificate(namespace: &str, host: &str, issuer_ref: &Value) -> Value {
    named_certificate(namespace, &object_name(host), labels(), host, issuer_ref)
}

/// The host's Ingress once its certificate is issued: the edge's, for this host alone, on `/`
/// with its TLS Secret. `None` when the template routes no backend.
pub fn served_ingress(template: &Value, namespace: &str, host: &str) -> Option<Value> {
    named_ingress(template, namespace, &object_name(host), labels(), host)
}

/// The host's Ingress before its certificate: the ACME path alone, no TLS entry and none of the
/// edge's TLS router annotations, so nothing but the challenge reaches the edge on this host.
pub fn acme_ingress(template: &Value, namespace: &str, host: &str) -> Option<Value> {
    let mut ingress = served_ingress(template, namespace, host)?;
    let spec = ingress["spec"].as_object_mut()?;
    spec.remove("tls");
    let path = &mut ingress["spec"]["rules"][0]["http"]["paths"][0];
    path["path"] = json!(ACME_PATH);
    path["pathType"] = json!("Prefix");
    // named_ingress always writes the annotations as an object.
    ingress["metadata"]["annotations"]
        .as_object_mut()
        .into_iter()
        .for_each(|annotations| {
            annotations
                .retain(|key, _| !key.starts_with("traefik.ingress.kubernetes.io/router.tls"))
        });
    Some(ingress)
}

/// Whether `ingress` is still the ACME-only one.
pub fn acme_only(ingress: &Value) -> bool {
    ingress["spec"]["tls"]
        .as_array()
        .is_none_or(|tls| tls.is_empty())
}

/// The registered domain a host's certificate counts against: its eTLD+1 from the public
/// suffix list (`smartcity.bbsk.sk` → `bbsk.sk`, `a.city.gov.uk` → `city.gov.uk`); the host
/// itself when the list has none.
pub fn registered_domain(host: &str) -> String {
    psl::domain_str(host).unwrap_or(host).to_owned()
}

/// When the next certificate may be requested, or `None` for now: `day` are this instance's
/// requests of the last 24 hours, `week` those of the last 7 days for the host's registered
/// domain, each oldest first.
pub fn wait_until(day: &[TimestampTz], week: &[TimestampTz]) -> Option<TimestampTz> {
    // The slot frees when the oldest request still counted leaves its window.
    let free = |held: &[TimestampTz], limit: usize, window: jiff::SignedDuration| {
        (held.len() >= limit).then(|| held[held.len() - limit] + window)
    };
    let by_day = free(day, PER_INSTANCE_DAY, jiff::SignedDuration::from_hours(24));
    let by_week = free(
        week,
        PER_DOMAIN_WEEK,
        jiff::SignedDuration::from_hours(24 * 7),
    );
    by_day.max(by_week)
}

/// Whether the host's addresses share one with the edge's: a CNAME to the edge or an A or AAAA
/// record of it (the resolver follows a CNAME).
pub fn points_at_edge(host: &BTreeSet<IpAddr>, edge: &BTreeSet<IpAddr>) -> bool {
    !host.is_disjoint(edge)
}

/// The addresses a name resolves to; a test hands canned ones.
pub trait Addresses: Send + Sync {
    fn addresses<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<BTreeSet<IpAddr>, Unanswered>>;
}

/// The hosts of the registry, in the edge's namespace.
pub struct OrgHosts {
    kube: KubeClient,
    namespace: String,
    pool: PgPool,
    dns: Arc<dyn Addresses>,
    /// The dns01 ClusterIssuer the installation names (`JC_PORTAL_DNS01_ISSUER`); the edge
    /// certificate's issuer without one. Its Secret stays cert-manager's: never read here.
    dns01_issuer: Option<String>,
}

/// Where one host's step ended: its next state, or a sentence of what it waits for.
enum Step {
    Moved(&'static str),
    Waits(String),
}

impl OrgHosts {
    pub fn new(kube: KubeClient, namespace: String, pool: PgPool, dns: Arc<dyn Addresses>) -> Self {
        Self {
            kube,
            namespace,
            pool,
            dns,
            dns01_issuer: None,
        }
    }

    /// Requests every certificate from the dns01 ClusterIssuer `issuer` instead of HTTP-01.
    pub fn with_dns01_issuer(mut self, issuer: Option<String>) -> Self {
        self.dns01_issuer = issuer;
        self
    }

    /// One pass: moves each host from `approved` on as far as it goes, then, when
    /// `edge_current` says the edge file was written from the registry this pass, deletes the
    /// objects of every host that is no longer on its way to `served` or served.
    pub async fn converge(&self, edge_current: bool) {
        let rows = match crate::host_registry::rows(&self.pool).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "the host registry was not read: no host moves this pass");
                return;
            }
        };
        let moving: Vec<&HostRow> = rows
            .iter()
            .filter(|row| matches!(row.state.as_str(), "approved" | "dns-ok" | "cert-issued"))
            .collect();
        if !moving.is_empty() {
            match self.edge().await {
                Ok((template, issuer)) => {
                    for row in moving {
                        self.walk(row, &template, &issuer).await;
                    }
                }
                Err(reason) => {
                    for row in moving {
                        self.detail(&row.host, &row.state, &reason).await;
                    }
                }
            }
        }
        if edge_current {
            self.retire(&rows).await;
        }
    }

    /// The edge Ingress every host's is copied from, and the issuer its certificates come from.
    async fn edge(&self) -> Result<(Value, Value), String> {
        let ns = self.namespace.as_str();
        let template = match self
            .kube
            .get(INGRESS_API, "Ingress", ns, EDGE_INGRESS)
            .await
        {
            Ok(Some(template)) => template,
            Ok(None) => {
                return Err(format!(
                    "the edge Ingress {EDGE_INGRESS} is not in {ns}; the apisix chart renders it"
                ))
            }
            Err(err) => return Err(said(&format!("Ingress {EDGE_INGRESS}"), &err)),
        };
        if let Some(name) = &self.dns01_issuer {
            return Ok((template, json!({ "name": name, "kind": "ClusterIssuer" })));
        }
        match self
            .kube
            .get(CERTIFICATE_API, "Certificate", ns, EDGE_CERTIFICATE)
            .await
        {
            Ok(Some(edge)) if edge["spec"]["issuerRef"].is_object() => {
                Ok((template, edge["spec"]["issuerRef"].clone()))
            }
            Ok(_) => Err(format!(
                "the edge Certificate {EDGE_CERTIFICATE} in {ns} names no issuer to request a host's from"
            )),
            Err(err) => Err(said(&format!("Certificate {EDGE_CERTIFICATE}"), &err)),
        }
    }

    /// Steps one host until it waits or is served.
    async fn walk(&self, row: &HostRow, template: &Value, issuer: &Value) {
        let mut state = row.state.clone();
        loop {
            let step = match state.as_str() {
                "approved" => self.dns(&row.host, template).await,
                "dns-ok" => self.certify(&row.host, template, issuer).await,
                "cert-issued" => self.widen(&row.host, template).await,
                _ => return,
            };
            match step {
                Step::Moved(next) => {
                    if !self.moved(&row.host, &state, next).await {
                        return;
                    }
                    state = next.to_owned();
                }
                Step::Waits(reason) => {
                    self.detail(&row.host, &state, &reason).await;
                    return;
                }
            }
        }
    }

    /// `approved` → `dns-ok` once the host resolves to an address of the edge's host.
    async fn dns(&self, host: &str, template: &Value) -> Step {
        let Some(edge_host) = template["spec"]["rules"][0]["host"].as_str() else {
            return Step::Waits(format!(
                "the edge Ingress {EDGE_INGRESS} names no host to compare {host}'s DNS with"
            ));
        };
        let edge = match self.dns.addresses(edge_host).await {
            Ok(edge) if !edge.is_empty() => edge,
            _ => return Step::Waits(format!("waiting for DNS: {edge_host} did not resolve")),
        };
        match self.dns.addresses(host).await {
            Ok(found) if points_at_edge(&found, &edge) => Step::Moved("dns-ok"),
            Ok(found) if found.is_empty() => Step::Waits(format!(
                "waiting for DNS: {host} has no record yet; add a CNAME to {edge_host}"
            )),
            Ok(_) => Step::Waits(format!(
                "waiting for DNS: {host} points elsewhere; make it a CNAME to {edge_host}"
            )),
            Err(Unanswered) => Step::Waits(format!(
                "waiting for DNS: the lookup of {host} got no answer; tried again next pass"
            )),
        }
    }

    /// `dns-ok`: the ACME-only Ingress and the Certificate, created once; → `cert-issued` when
    /// cert-manager says it is issued.
    async fn certify(&self, host: &str, template: &Value, issuer: &Value) -> Step {
        let ns = self.namespace.as_str();
        let name = object_name(host);
        let Some(wanted) = acme_ingress(template, ns, host) else {
            return Step::Waits(format!(
                "the edge Ingress {EDGE_INGRESS} routes no backend to copy"
            ));
        };
        match self.kube.get(INGRESS_API, "Ingress", ns, &name).await {
            Ok(None) => match self.kube.create(&wanted).await {
                Ok(()) => {
                    self.step(host, "ingress.created", json!({ "path": ACME_PATH }))
                        .await
                }
                Err(err) => return Step::Waits(said(&format!("Ingress {name}"), &err)),
            },
            Ok(Some(held)) if !managed(&held) => {
                return Step::Waits(conflict("an Ingress", &name, ns))
            }
            Ok(Some(_)) => {}
            Err(err) => return Step::Waits(said(&format!("Ingress {name}"), &err)),
        }
        match self
            .kube
            .get(CERTIFICATE_API, "Certificate", ns, &name)
            .await
        {
            Ok(None) => self.request(host, &certificate(ns, host, issuer)).await,
            Ok(Some(held)) if !managed(&held) => Step::Waits(conflict("a Certificate", &name, ns)),
            Ok(Some(held)) => match state_of(&held) {
                HostState::Ready => {
                    self.step(host, "certificate.issued", json!({ "certificate": name }))
                        .await;
                    Step::Moved("cert-issued")
                }
                HostState::Pending(message) | HostState::Failed(message) => {
                    Step::Waits(format!("the certificate is not issued yet: {message}"))
                }
            },
            Err(err) => Step::Waits(said(&format!("Certificate {name}"), &err)),
        }
    }

    /// Creates `wanted` when a slot is free, and counts it.
    async fn request(&self, host: &str, wanted: &Value) -> Step {
        let domain = registered_domain(host);
        let (day, week) = match self.held(&domain).await {
            Ok(held) => held,
            Err(err) => {
                tracing::warn!(error = %err, "the certificate ledger was not read");
                return Step::Waits(
                    "the certificate ledger could not be read; tried again next pass".into(),
                );
            }
        };
        if let Some(at) = wait_until(&day, &week) {
            return Step::Waits(format!(
                "waiting for a certificate slot until {}",
                at.0.strftime("%Y-%m-%d %H:%M UTC")
            ));
        }
        let name = object_name(host);
        // Counted before it is requested: a request the ledger missed would let the pacing pass
        // Let's Encrypt's limit. A request that fails gives its slot back; if even that fails
        // the slot stays taken, which errs on the limit's side.
        let counted: i64 = match sqlx::query_scalar(
            "INSERT INTO organization_host_certificates (host, registered_domain) VALUES ($1, $2) \
             RETURNING id",
        )
        .bind(host)
        .bind(&domain)
        .fetch_one(&self.pool)
        .await
        {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(error = %err, %host, "the certificate ledger could not be written");
                return Step::Waits(
                    "the certificate ledger could not be written; tried again next pass".into(),
                );
            }
        };
        if let Err(err) = self.kube.create(wanted).await {
            if let Err(back) =
                sqlx::query("DELETE FROM organization_host_certificates WHERE id = $1")
                    .bind(counted)
                    .execute(&self.pool)
                    .await
            {
                tracing::warn!(error = %back, %host, "a failed request keeps its certificate slot");
            }
            return Step::Waits(said(&format!("Certificate {name}"), &err));
        }
        self.step(
            host,
            "certificate.created",
            json!({ "certificate": name, "registeredDomain": domain }),
        )
        .await;
        Step::Waits("the certificate is requested".into())
    }

    /// This instance's requests of the last 24 hours and the domain's of the last 7 days.
    async fn held(
        &self,
        domain: &str,
    ) -> Result<(Vec<TimestampTz>, Vec<TimestampTz>), sqlx::Error> {
        let day: Vec<i64> = sqlx::query_scalar(
            "SELECT extract(epoch FROM requested_at)::bigint FROM organization_host_certificates \
             WHERE requested_at > now() - interval '24 hours' ORDER BY requested_at",
        )
        .fetch_all(&self.pool)
        .await?;
        let week: Vec<i64> = sqlx::query_scalar(
            "SELECT extract(epoch FROM requested_at)::bigint FROM organization_host_certificates \
             WHERE registered_domain = $1 AND requested_at > now() - interval '7 days' \
             ORDER BY requested_at",
        )
        .bind(domain)
        .fetch_all(&self.pool)
        .await?;
        let at = |held: Vec<i64>| {
            held.into_iter()
                .filter_map(jc_time::TimestampTz::from_second)
                .collect()
        };
        Ok((at(day), at(week)))
    }

    /// `cert-issued`: the Ingress on `/` with its TLS Secret (a delete and a create: the Role
    /// grants no update); → `served`.
    async fn widen(&self, host: &str, template: &Value) -> Step {
        let ns = self.namespace.as_str();
        let name = object_name(host);
        let Some(wanted) = served_ingress(template, ns, host) else {
            return Step::Waits(format!(
                "the edge Ingress {EDGE_INGRESS} routes no backend to copy"
            ));
        };
        match self.kube.get(INGRESS_API, "Ingress", ns, &name).await {
            Ok(Some(held)) if !managed(&held) => {
                return Step::Waits(conflict("an Ingress", &name, ns))
            }
            Ok(Some(held)) if !acme_only(&held) => {}
            Ok(held) => {
                if held.is_some() {
                    if let Err(err) = self.kube.delete(INGRESS_API, "Ingress", ns, &name).await {
                        return Step::Waits(said(&format!("Ingress {name}"), &err));
                    }
                }
                if let Err(err) = self.kube.create(&wanted).await {
                    return Step::Waits(said(&format!("Ingress {name}"), &err));
                }
                self.step(
                    host,
                    "ingress.widened",
                    json!({ "ingress": name, "path": "/" }),
                )
                .await;
            }
            Err(err) => return Step::Waits(said(&format!("Ingress {name}"), &err)),
        }
        Step::Moved("served")
    }

    /// Deletes the Ingress, then the Certificate, this wave made for a host no longer on its
    /// way to `served`: removed, or never approved. A failure is logged and tried next pass.
    async fn retire(&self, rows: &[HostRow]) {
        let keep: BTreeSet<String> = rows
            .iter()
            .filter(|row| matches!(row.state.as_str(), "dns-ok" | "cert-issued" | "served"))
            .map(|row| object_name(&row.host))
            .collect();
        let hosts: std::collections::BTreeMap<String, &str> = rows
            .iter()
            .map(|row| (object_name(&row.host), row.host.as_str()))
            .collect();
        // A Certificate goes only once its Ingress is gone: an Ingress left behind would serve
        // the host from a Secret nobody renews. Ingresses not read this pass keep every one.
        let Some(left) = self
            .delete_unkept(INGRESS_API, "Ingress", &keep, &hosts)
            .await
        else {
            return;
        };
        let keep: BTreeSet<String> = keep.union(&left).cloned().collect();
        self.delete_unkept(CERTIFICATE_API, "Certificate", &keep, &hosts)
            .await;
    }

    /// Deletes this wave's objects of `kind` not in `keep`, each with its audit row; the names
    /// whose delete failed, `None` when the objects could not be listed.
    async fn delete_unkept(
        &self,
        api: &str,
        kind: &str,
        keep: &BTreeSet<String>,
        hosts: &std::collections::BTreeMap<String, &str>,
    ) -> Option<BTreeSet<String>> {
        let ns = self.namespace.as_str();
        let selector = format!("{MANAGED_BY}={MANAGED_VALUE},{COMPONENT}={COMPONENT_VALUE}");
        let held = match self.kube.list(api, kind, ns, &selector).await {
            Ok(held) => held,
            Err(err) => {
                tracing::warn!(error = %said(kind, &err), "registry hosts' objects were not listed");
                return None;
            }
        };
        let mut failed = BTreeSet::new();
        for object in held {
            let Some(name) = object["metadata"]["name"].as_str() else {
                continue;
            };
            if !managed(&object) || keep.contains(name) {
                continue;
            }
            match self.kube.delete(api, kind, ns, name).await {
                Ok(()) => {
                    let host = hosts.get(name).copied().unwrap_or(name);
                    let action = if kind == "Ingress" {
                        "ingress.deleted"
                    } else {
                        "certificate.deleted"
                    };
                    self.step(host, action, json!({ "object": name })).await;
                }
                Err(err) => {
                    tracing::warn!(%name, error = %said(&format!("{kind} {name}"), &err), "a removed host's object was not deleted");
                    failed.insert(name.to_owned());
                }
            }
        }
        Some(failed)
    }

    /// Moves `host` from `from` to `to`, clearing what it waited for, with its audit row in one
    /// transaction; `false` when the row moved meanwhile (an operator removed it).
    async fn moved(&self, host: &str, from: &str, to: &str) -> bool {
        let result = async {
            let mut tx = self.pool.begin().await?;
            let moved = sqlx::query(
                "UPDATE organization_hosts SET state = $3, detail = NULL, detail_at = NULL, \
                 updated_at = now() WHERE host = $1 AND state = $2",
            )
            .bind(host)
            .bind(from)
            .bind(to)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if moved {
                audit(
                    &mut tx,
                    &format!("host.{to}"),
                    Some(&json!({ "host": host, "state": from })),
                    &json!({ "host": host, "state": to }),
                )
                .await?;
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(moved)
        }
        .await;
        result.unwrap_or_else(|err| {
            tracing::warn!(error = %err, %host, to, "a host's state was not written");
            false
        })
    }

    /// What `host` in `state` waits for, written only when it changed (each write wakes jc's
    /// router rebuild).
    async fn detail(&self, host: &str, state: &str, detail: &str) {
        if let Err(err) = sqlx::query(
            "UPDATE organization_hosts SET detail = $3, detail_at = now() \
             WHERE host = $1 AND state = $2 AND detail IS DISTINCT FROM $3",
        )
        .bind(host)
        .bind(state)
        .bind(detail)
        .execute(&self.pool)
        .await
        {
            tracing::warn!(error = %err, %host, "a host's detail was not written");
        }
    }

    /// One audit row of a step on `host`'s objects; logged when the row is not written.
    async fn step(&self, host: &str, action: &str, mut after: Value) {
        after["host"] = json!(host);
        let written = async {
            let mut conn = self.pool.acquire().await?;
            audit(&mut conn, action, None, &after).await
        }
        .await;
        if let Err(err) = written {
            tracing::error!(error = %err, %host, action, "a reconciler step was not audited");
        }
    }
}

fn conflict(what: &str, name: &str, ns: &str) -> String {
    format!(
        "conflict: {what} {name} in {ns} exists that the Portal did not create; remove it so the host can be served"
    )
}

/// One reconciler row of the operators' audit and its log line.
async fn audit(
    conn: &mut sqlx::PgConnection,
    action: &str,
    before: Option<&Value>,
    after: &Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO operator_audit (actor, action, before, after) VALUES ($1, $2, $3, $4)",
    )
    .bind(ACTOR)
    .bind(action)
    .bind(before)
    .bind(after)
    .execute(conn)
    .await?;
    let before = before.map(Value::to_string).unwrap_or_default();
    tracing::info!(target: "operator_audit", actor = ACTOR, action, %before, %after, "operator audit");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> Value {
        json!({
            "metadata": {
                "name": "apisix",
                "annotations": {
                    "traefik.ingress.kubernetes.io/router.tls": "true",
                    "traefik.ingress.kubernetes.io/router.tls.options": "apisix-bsi-tr-02102@kubernetescrd",
                    "traefik.ingress.kubernetes.io/router.entrypoints": "websecure",
                    "cert-manager.io/cluster-issuer": "letsencrypt-prod",
                },
            },
            "spec": {
                "ingressClassName": "traefik",
                "rules": [{ "host": "dev.example", "http": { "paths": [{
                    "path": "/", "pathType": "ImplementationSpecific",
                    "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
                }] } }],
                "tls": [{ "hosts": ["dev.example"], "secretName": "apisix-edge-tls" }],
            },
        })
    }

    #[test]
    fn names_are_label_values_and_never_shared() {
        let long = format!("{}.bbsk.sk", "a".repeat(80));
        for host in ["smartcity.bbsk.sk", long.as_str(), "a-b.c", "a.b-c"] {
            let name = object_name(host);
            assert!(name.len() <= 63, "{name}");
            assert!(name.starts_with("jc-org-"));
            assert!(crate::resource::is_dns1123(&name), "{name}");
        }
        assert_ne!(object_name("a-b.c"), object_name("a.b-c"));
    }

    /// §7 O3 "Not served before its certificate": a `dns-ok` host's Ingress has the ACME path
    /// alone and no TLS.
    #[test]
    fn before_its_certificate_a_host_ingress_carries_the_acme_path_alone() {
        let made = acme_ingress(&template(), "apisix", "smartcity.bbsk.sk").expect("an Ingress");
        assert!(made["spec"].get("tls").is_none(), "{made}");
        assert!(acme_only(&made));
        let paths = made["spec"]["rules"][0]["http"]["paths"]
            .as_array()
            .expect("paths");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0]["path"], ACME_PATH);
        assert_eq!(paths[0]["pathType"], "Prefix");
        assert_eq!(made["spec"]["rules"][0]["host"], "smartcity.bbsk.sk");
        let annotations = made["metadata"]["annotations"]
            .as_object()
            .expect("annotations");
        assert!(
            annotations.keys().all(
                |k| !k.starts_with("traefik.ingress.kubernetes.io/router.tls")
                    && !k.starts_with("cert-manager.io/")
            ),
            "{annotations:?}"
        );
        assert!(managed(&made));

        let served =
            served_ingress(&template(), "apisix", "smartcity.bbsk.sk").expect("an Ingress");
        assert!(!acme_only(&served));
        let name = object_name("smartcity.bbsk.sk");
        assert_eq!(
            served["spec"]["tls"],
            json!([{ "hosts": ["smartcity.bbsk.sk"], "secretName": format!("{name}-tls") }])
        );
        assert_eq!(served["spec"]["rules"][0]["http"]["paths"][0]["path"], "/");
    }

    #[test]
    fn a_host_certificate_is_its_own_never_the_edges_secret() {
        let issuer = json!({ "name": "letsencrypt-prod", "kind": "ClusterIssuer" });
        let made = certificate("apisix", "smartcity.bbsk.sk", &issuer);
        let name = object_name("smartcity.bbsk.sk");
        assert_eq!(made["metadata"]["name"], name);
        assert_eq!(made["spec"]["secretName"], format!("{name}-tls"));
        assert_eq!(made["spec"]["dnsNames"], json!(["smartcity.bbsk.sk"]));
        assert_eq!(made["spec"]["issuerRef"], issuer);
        assert!(managed(&made));
        let mut foreign = made.clone();
        foreign["metadata"]["labels"] = json!({ MANAGED_BY: MANAGED_VALUE, COMPONENT: "app-host" });
        assert!(
            !managed(&foreign),
            "another wave's object is not a registry host's"
        );
    }

    #[test]
    fn the_registered_domain_is_the_etld_plus_one() {
        assert_eq!(registered_domain("smartcity.bbsk.sk"), "bbsk.sk");
        assert_eq!(
            registered_domain("a.b.dev.joinedcontext.com"),
            "joinedcontext.com"
        );
        assert_eq!(registered_domain("opendata.city.gov.uk"), "city.gov.uk");
        assert_eq!(registered_domain("x.y.co.uk"), "y.co.uk");
    }

    /// §7 O3 "Pacing": the eleventh new certificate in 24 hours waits, so does the 21st in 7 days
    /// for one registered domain; each waits until its oldest counted request leaves the window.
    #[test]
    fn the_eleventh_a_day_and_the_twenty_first_a_week_wait() {
        let now = TimestampTz::now();
        let hours = |n: i64| now - jiff::SignedDuration::from_hours(n);
        let day: Vec<_> = (0..9).rev().map(hours).collect();
        assert_eq!(wait_until(&day, &day), None, "the tenth goes");
        let day: Vec<_> = (0..10).rev().map(hours).collect();
        assert_eq!(
            wait_until(&day, &day),
            Some(hours(9) + jiff::SignedDuration::from_hours(24)),
            "the eleventh waits"
        );

        let week: Vec<_> = (0..20)
            .rev()
            .map(|n| now - jiff::SignedDuration::from_hours(6 * n))
            .collect();
        let today: Vec<_> = week
            .iter()
            .copied()
            .filter(|t| *t > now - jiff::SignedDuration::from_hours(24))
            .collect();
        assert!(today.len() < PER_INSTANCE_DAY);
        assert_eq!(
            wait_until(&today, &week),
            Some(week[0] + jiff::SignedDuration::from_hours(24 * 7)),
            "the 21st waits"
        );
        assert_eq!(wait_until(&today, &week[1..]), None);
    }

    #[test]
    fn dns_is_ok_when_the_host_shares_an_address_with_the_edge() {
        let ip = |s: &str| s.parse::<IpAddr>().expect("an address");
        let edge: BTreeSet<_> = [ip("203.0.113.10"), ip("2a01:4f8::1")].into();
        assert!(points_at_edge(&[ip("203.0.113.10")].into(), &edge));
        assert!(points_at_edge(&[ip("2a01:4f8::1")].into(), &edge));
        assert!(!points_at_edge(&[ip("198.51.100.9")].into(), &edge));
        assert!(!points_at_edge(&BTreeSet::new(), &edge));
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const CERTS: &str = "/apis/cert-manager.io/v1/namespaces/apisix/certificates";
    const INGRESSES: &str = "/apis/networking.k8s.io/v1/namespaces/apisix/ingresses";
    const HOST: &str = "smartcity.bbsk.sk";

    /// Canned lookups: a name's addresses, `None` for a lookup that got no answer.
    struct Canned(std::collections::BTreeMap<&'static str, Option<Vec<IpAddr>>>);

    impl Addresses for Canned {
        fn addresses<'a>(
            &'a self,
            name: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Result<BTreeSet<IpAddr>, Unanswered>> {
            let found = match self.0.get(name) {
                Some(Some(found)) => Ok(found.iter().copied().collect()),
                Some(None) => Err(Unanswered),
                None => Ok(BTreeSet::new()),
            };
            Box::pin(async move { found })
        }
    }

    /// A pool whose every query fails: its server's port is held bound and never listens.
    fn dead_pool() -> (tokio::net::TcpSocket, PgPool) {
        let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
        socket
            .bind("127.0.0.1:0".parse().expect("an address"))
            .expect("bound");
        let port = socket.local_addr().expect("its address").port();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy(&format!("postgres://u:p@127.0.0.1:{port}/db"))
            .expect("a lazy pool");
        (socket, pool)
    }

    fn wave(server: &MockServer, pool: PgPool, dns: Canned) -> OrgHosts {
        OrgHosts::new(
            KubeClient::with_token(&server.uri(), "t").expect("a client"),
            "apisix".into(),
            pool,
            Arc::new(dns),
        )
    }

    fn waits(step: Step) -> String {
        match step {
            Step::Waits(reason) => reason,
            Step::Moved(next) => format!("moved to {next}"),
        }
    }

    async fn answer(server: &MockServer, verb: &str, at: &str, status: u16, body: Value) {
        Mock::given(method(verb))
            .and(path(at))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    fn ours(kind: &str, extra: Value) -> Value {
        let mut object =
            json!({ "kind": kind, "metadata": { "name": object_name(HOST), "labels": labels() } });
        object
            .as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        object
    }

    /// The registry not read: no host moves, and the log says why.
    #[tokio::test]
    async fn an_unread_registry_moves_nothing_and_says_so() {
        let server = MockServer::start().await;
        let (_held, pool) = dead_pool();
        let hosts = wave(&server, pool, Canned(Default::default()));
        let ((), log) = super::super::logged(hosts.converge(true)).await;
        assert!(
            log.contains("the host registry was not read: no host moves this pass"),
            "{log}"
        );
        assert!(server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty());
    }

    /// T-7924: a registry with no host on its way to being served moves nothing and asks the
    /// cluster nothing, not even for the edge every moving host is copied from.
    #[tokio::test]
    async fn a_registry_with_no_host_on_its_way_asks_the_cluster_nothing() {
        let scratch = crate::test_db::scratch().await;
        let server = MockServer::start().await;
        let hosts = wave(&server, scratch.pool.clone(), Canned(Default::default()));
        let ((), log) = super::super::logged(hosts.converge(false)).await;
        assert!(
            !log.contains("the host registry was not read"),
            "the empty registry was read: {log}"
        );
        assert!(server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty());
        drop(hosts);
        scratch.remove().await;
    }

    /// Each way DNS keeps a host waiting is said: no edge host to compare, an edge that does not
    /// resolve, and a lookup with no answer.
    #[tokio::test]
    async fn dns_waits_are_each_said() {
        let server = MockServer::start().await;
        let (_held, pool) = dead_pool();
        let edge: Vec<IpAddr> = vec!["203.0.113.10".parse().expect("an address")];
        let hosts = wave(
            &server,
            pool.clone(),
            Canned([("dev.example", Some(edge.clone())), (HOST, None)].into()),
        );
        assert_eq!(
            waits(hosts.dns(HOST, &json!({ "spec": {} })).await),
            format!("the edge Ingress apisix names no host to compare {HOST}'s DNS with")
        );
        assert_eq!(
            waits(hosts.dns(HOST, &template()).await),
            format!("waiting for DNS: the lookup of {HOST} got no answer; tried again next pass")
        );
        let unset = wave(
            &server,
            pool.clone(),
            Canned([("dev.example", Some(edge))].into()),
        );
        assert_eq!(
            waits(unset.dns(HOST, &template()).await),
            format!("waiting for DNS: {HOST} has no record yet; add a CNAME to dev.example")
        );
        let unresolved = wave(&server, pool, Canned([("dev.example", None)].into()));
        assert_eq!(
            waits(unresolved.dns(HOST, &template()).await),
            "waiting for DNS: dev.example did not resolve"
        );
    }

    /// The edge's parts: its Ingress missing or refused, a certificate without an issuer, and the
    /// installation's dns01 issuer standing in for the edge certificate's.
    #[tokio::test]
    async fn the_edges_parts_are_read_or_the_reason_given() {
        let (_held, pool) = dead_pool();
        for (status, reason) in [
            (
                404,
                "the edge Ingress apisix is not in apisix; the apisix chart renders it",
            ),
            (500, "the API server answered 500 for Ingress apisix"),
        ] {
            let server = MockServer::start().await;
            answer(
                &server,
                "GET",
                &format!("{INGRESSES}/apisix"),
                status,
                json!({}),
            )
            .await;
            assert_eq!(
                wave(&server, pool.clone(), Canned(Default::default()))
                    .edge()
                    .await
                    .err()
                    .as_deref(),
                Some(reason)
            );
        }
        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/apisix"),
            200,
            template(),
        )
        .await;
        answer(
            &server,
            "GET",
            &format!("{CERTS}/apisix-edge"),
            200,
            json!({ "spec": {} }),
        )
        .await;
        let hosts = wave(&server, pool.clone(), Canned(Default::default()));
        assert_eq!(
            hosts.edge().await.err().as_deref(),
            Some("the edge Certificate apisix-edge in apisix names no issuer to request a host's from")
        );
        let refused = MockServer::start().await;
        answer(
            &refused,
            "GET",
            &format!("{INGRESSES}/apisix"),
            200,
            template(),
        )
        .await;
        answer(
            &refused,
            "GET",
            &format!("{CERTS}/apisix-edge"),
            502,
            json!({}),
        )
        .await;
        assert_eq!(
            wave(&refused, pool.clone(), Canned(Default::default()))
                .edge()
                .await
                .err()
                .as_deref(),
            Some("the API server answered 502 for Certificate apisix-edge")
        );
        let (_, issuer) = hosts
            .with_dns01_issuer(Some("letsencrypt-dns01".into()))
            .edge()
            .await
            .expect("the edge");
        assert_eq!(
            issuer,
            json!({ "name": "letsencrypt-dns01", "kind": "ClusterIssuer" })
        );
    }

    /// Each refusal on the way to a certificate keeps the host waiting with the object named; an
    /// issued-not-yet certificate says what cert-manager said; the ledger not read waits too.
    #[tokio::test]
    async fn every_certify_refusal_keeps_the_host_waiting() {
        let (_held, pool) = dead_pool();
        let name = object_name(HOST);
        let certify = |server: &MockServer| wave(server, pool.clone(), Canned(Default::default()));
        let server = MockServer::start().await;
        assert_eq!(
            waits(
                certify(&server)
                    .certify(HOST, &json!({ "spec": {} }), &json!({}))
                    .await
            ),
            "the edge Ingress apisix routes no backend to copy"
        );

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            500,
            json!({}),
        )
        .await;
        assert_eq!(
            waits(
                certify(&server)
                    .certify(HOST, &template(), &json!({}))
                    .await
            ),
            format!("the API server answered 500 for Ingress {name}")
        );

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            404,
            json!({}),
        )
        .await;
        answer(&server, "POST", INGRESSES, 403, json!({})).await;
        assert_eq!(
            waits(
                certify(&server)
                    .certify(HOST, &template(), &json!({}))
                    .await
            ),
            format!("the API server answered 403 for Ingress {name}")
        );

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            200,
            ours("Ingress", json!({})),
        )
        .await;
        answer(&server, "GET", &format!("{CERTS}/{name}"), 500, json!({})).await;
        assert_eq!(
            waits(
                certify(&server)
                    .certify(HOST, &template(), &json!({}))
                    .await
            ),
            format!("the API server answered 500 for Certificate {name}")
        );

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            200,
            ours("Ingress", json!({})),
        )
        .await;
        answer(&server, "GET", &format!("{CERTS}/{name}"), 200, ours("Certificate", json!({ "status": { "conditions": [
            { "type": "Ready", "status": "False", "message": "Issuing certificate as Secret does not exist" }] } }))).await;
        assert_eq!(
            waits(
                certify(&server)
                    .certify(HOST, &template(), &json!({}))
                    .await
            ),
            "the certificate is not issued yet: Issuing certificate as Secret does not exist"
        );

        // The Ingress is created (its audit row lost with the database), then the ledger is not read.
        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            404,
            json!({}),
        )
        .await;
        answer(&server, "POST", INGRESSES, 201, json!({})).await;
        answer(&server, "GET", &format!("{CERTS}/{name}"), 404, json!({})).await;
        answer(&server, "POST", CERTS, 201, json!({})).await;
        let hosts = certify(&server);
        let (step, log) = super::super::logged(hosts.certify(HOST, &template(), &json!({}))).await;
        assert_eq!(
            waits(step),
            "the certificate ledger could not be read; tried again next pass"
        );
        assert!(log.contains("a reconciler step was not audited"), "{log}");
        assert!(log.contains("the certificate ledger was not read"), "{log}");
        let created = server.received_requests().await.expect("recorded");
        assert!(
            created
                .iter()
                .all(|r| !(r.method.as_str() == "POST" && r.url.path() == CERTS)),
            "no certificate without the ledger"
        );
    }

    /// Widening: no backend, a foreign Ingress, each refused delete, create or read waits; an
    /// Ingress already on `/` serves at once, and a missing one is created.
    #[tokio::test]
    async fn widening_refusals_wait_and_a_widened_ingress_serves() {
        let (_held, pool) = dead_pool();
        let name = object_name(HOST);
        let widen = |server: &MockServer| wave(server, pool.clone(), Canned(Default::default()));
        let acme = ours("Ingress", json!({ "spec": { "rules": [] } }));
        let server = MockServer::start().await;
        assert_eq!(
            waits(widen(&server).widen(HOST, &json!({ "spec": {} })).await),
            "the edge Ingress apisix routes no backend to copy"
        );

        let cases: [(u16, Value, u16, u16, String); 4] = [
            (200, json!({ "metadata": { "name": name } }), 200, 201, format!("conflict: an Ingress {name} in apisix exists that the Portal did not create; remove it so the host can be served")),
            (200, acme.clone(), 500, 201, format!("the API server answered 500 for Ingress {name}")),
            (200, acme.clone(), 200, 422, format!("the API server answered 422 for Ingress {name}")),
            (503, json!({}), 200, 201, format!("the API server answered 503 for Ingress {name}")),
        ];
        for (get, held, delete, create, reason) in cases {
            let server = MockServer::start().await;
            answer(&server, "GET", &format!("{INGRESSES}/{name}"), get, held).await;
            answer(
                &server,
                "DELETE",
                &format!("{INGRESSES}/{name}"),
                delete,
                json!({}),
            )
            .await;
            answer(&server, "POST", INGRESSES, create, json!({})).await;
            assert_eq!(waits(widen(&server).widen(HOST, &template()).await), reason);
        }

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            200,
            ours(
                "Ingress",
                json!({ "spec": { "tls": [{ "hosts": [HOST] }] } }),
            ),
        )
        .await;
        assert!(matches!(
            widen(&server).widen(HOST, &template()).await,
            Step::Moved("served")
        ));
        assert!(server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .all(|r| r.method.as_str() == "GET"));

        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            &format!("{INGRESSES}/{name}"),
            404,
            json!({}),
        )
        .await;
        Mock::given(method("POST"))
            .and(path(INGRESSES))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        assert_eq!(
            waits(widen(&server).widen(HOST, &template()).await),
            "moved to served"
        );
    }

    /// A database that does not answer: a move is not made and says so, a detail is not written
    /// and says so; an object without a name is never deleted.
    #[tokio::test]
    async fn a_lost_database_keeps_the_host_where_it_is_and_logs_it() {
        let server = MockServer::start().await;
        let (_held, pool) = dead_pool();
        let hosts = wave(&server, pool, Canned(Default::default()));
        let (moved, log) = super::super::logged(hosts.moved(HOST, "approved", "dns-ok")).await;
        assert!(!moved);
        assert!(log.contains("a host's state was not written"), "{log}");
        let ((), log) = super::super::logged(hosts.detail(HOST, "approved", "waiting")).await;
        assert!(log.contains("a host's detail was not written"), "{log}");

        answer(
            &server,
            "GET",
            INGRESSES,
            200,
            json!({ "items": [{ "metadata": { "labels": labels() } }] }),
        )
        .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let failed = hosts
            .delete_unkept(
                INGRESS_API,
                "Ingress",
                &BTreeSet::new(),
                &Default::default(),
            )
            .await;
        assert_eq!(failed, Some(BTreeSet::new()));
    }

    /// The ACME Ingress drops the edge's TLS router annotations and keeps the others it copies.
    #[test]
    fn the_acme_ingress_keeps_only_the_non_tls_router_annotations() {
        let made = acme_ingress(&template(), "apisix", HOST).expect("an Ingress");
        assert_eq!(
            made["metadata"]["annotations"],
            json!({ "traefik.ingress.kubernetes.io/router.entrypoints": "websecure" })
        );
        let mut bare = template();
        bare["metadata"]
            .as_object_mut()
            .expect("metadata")
            .remove("annotations");
        assert_eq!(
            acme_ingress(&bare, "apisix", HOST).expect("an Ingress")["metadata"]["annotations"],
            json!({})
        );
    }
}
