use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::resource::selector::{FieldSelector, LabelSelector};
use crate::resource::{is_dns1123, is_manifest_name, ResourceEnvelope, ResourceKey};

#[derive(Debug, Default)]
pub struct ListOptions {
    pub label_selector: Option<LabelSelector>,
    pub field_selector: Option<FieldSelector>,
    pub limit: Option<usize>,
    pub continue_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListPage {
    pub items: Vec<ResourceEnvelope>,
    pub continue_token: Option<String>,
    pub remaining: usize,
}

/// The read-optimized mirror of the configuration repository's default branch (UI-08): every
/// list and read the API serves comes from here, and the sync replaces it whole when the branch
/// moves, so a request never reads Git.
#[derive(Default)]
pub struct Mirror {
    resources: RwLock<BTreeMap<ResourceKey, ResourceEnvelope>>,
    /// Layout 2: each registered project's own repository in the forge, by slug (PF-86). Empty
    /// in layout 1, where every project lives in the organization repository.
    repositories: RwLock<BTreeMap<String, String>>,
    /// The organization's `.jc/layout` (CC-85): 2 when every project lives in a repository of
    /// its own; 0 until the first sync says, which reads as 1.
    layout: std::sync::atomic::AtomicU32,
}

impl Mirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// The forge repository `project`'s own kinds are written to (CC-87), or `None` when they
    /// live in the organization repository (layout 1, or a project outside the forge).
    pub fn repository_of(&self, project: &str) -> Option<String> {
        let lock = self.repositories.read().unwrap_or_else(|p| p.into_inner());
        lock.get(project).cloned()
    }

    /// The layout of the organization repository the mirror was read from (CC-85).
    pub fn layout(&self) -> u32 {
        self.layout
            .load(std::sync::atomic::Ordering::Relaxed)
            .max(1)
    }

    /// Records the layout the sync read.
    pub fn set_layout(&self, layout: u32) {
        self.layout
            .store(layout, std::sync::atomic::Ordering::Relaxed);
    }

    /// Every registered project's own repository, by slug.
    pub fn repositories(&self) -> BTreeMap<String, String> {
        self.repositories
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Records where each registered project lives, replacing what was recorded.
    pub fn set_repositories(&self, repositories: BTreeMap<String, String>) {
        let mut lock = self.repositories.write().unwrap_or_else(|p| p.into_inner());
        *lock = repositories;
    }

    /// Holds one resource, unless its namespace is not a namespace (T-2298, MF-02) or its name is
    /// not a name of its kind (T-2544).
    ///
    /// The mirror's key is the namespace as the manifest spells it, and the reconciler builds a
    /// runner URL by putting that string into an operator's template: a manifest committed with
    /// `metadata.namespace: "a@evil.example"` would move the Portal's own scrape to another host.
    /// A namespace that is not a DNS-1123 label names no project the Portal can serve, so nothing
    /// is lost by leaving it out — and every consumer of the mirror is closed at once rather than
    /// each one being taught to check.
    ///
    /// The name is held to its kind's rule for the same reason: the assistant's prompt joins
    /// names into its prose, and a name with lines of its own would add a heading or a turn to
    /// it; neither a prompt, a URL nor a log line ever gets a name outside that form.
    pub fn upsert(&self, env: ResourceEnvelope) {
        let key = env.key();
        if !is_dns1123(&key.namespace) || !is_manifest_name(&key.kind, &key.name) {
            // Loud in the log and silent towards callers: a dropped manifest is an operator's
            // problem to find, never a 500 on somebody's page. Debug-quoted, so a newline in
            // either is written as `\n` and not as a line of the log.
            tracing::warn!(
                namespace = ?key.namespace,
                kind = ?key.kind,
                name = ?key.name,
                "a manifest whose namespace or name is not a name of its kind is left out of the mirror"
            );
            return;
        }
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        lock.insert(key, env);
    }

    pub fn remove(&self, key: &ResourceKey) -> Option<ResourceEnvelope> {
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        lock.remove(key)
    }

    pub fn len(&self) -> usize {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        lock.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every project the repository holds, sorted: the distinct `metadata.namespace` of the
    /// mirrored manifests, since a project is exactly a `projects/<slug>/` directory with
    /// something in it (PF-05). Cluster-scoped resources carry no namespace and are skipped;
    /// so is the organization scope: the reconciler files every manifest outside `projects/`
    /// (`blueprints/`, the Organization at the root) under the namespace `org`
    /// (`reconciler::daemon::namespace_of`, `api::blueprints::ORG_NAMESPACE`).
    pub fn namespaces(&self) -> Vec<String> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        let mut names: Vec<String> = lock
            .keys()
            .map(|key| key.namespace.clone())
            .filter(|ns| !ns.is_empty() && ns != crate::api::blueprints::ORG_NAMESPACE)
            .collect();
        names.dedup();
        names
    }

    pub fn replace_all(&self, other: &Mirror) {
        let new_resources = other
            .resources
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut lock = self.resources.write().unwrap_or_else(|p| p.into_inner());
        *lock = new_resources;
        drop(lock);
        let repositories = other
            .repositories
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        self.set_repositories(repositories);
        self.set_layout(other.layout());
    }

    /// Every resource matching a predicate, in no particular order.
    pub fn matching(
        &self,
        mut predicate: impl FnMut(&ResourceEnvelope) -> bool,
    ) -> Vec<ResourceEnvelope> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        lock.values()
            .filter(|env| predicate(env))
            .cloned()
            .collect()
    }

    /// The first resource matching a predicate. Used where a name identifies a resource on its
    /// own, without the project segment the key carries (an app URL is `/apps/{name}/`).
    pub fn find(
        &self,
        mut predicate: impl FnMut(&ResourceEnvelope) -> bool,
    ) -> Option<ResourceEnvelope> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        lock.values().find(|env| predicate(env)).cloned()
    }

    pub fn get(&self, namespace: &str, kind: &str, name: &str) -> Option<ResourceEnvelope> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        let key = ResourceKey {
            namespace: namespace.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
        };
        lock.get(&key).cloned()
    }

    /// The resources of `kind` in `namespace` that `keep` accepts, by name. The map is ordered by
    /// namespace, kind and name, so this reads one range, and only what `keep` accepts is
    /// copied: a caller after a few of many (the caller's own RoleBindings) parses the rest in
    /// place (T-5978).
    pub fn kind_where(
        &self,
        namespace: &str,
        kind: &str,
        mut keep: impl FnMut(&ResourceEnvelope) -> bool,
    ) -> Vec<ResourceEnvelope> {
        let lock = self.resources.read().unwrap_or_else(|p| p.into_inner());
        let from = ResourceKey {
            namespace: namespace.to_owned(),
            kind: kind.to_owned(),
            name: String::new(),
        };
        lock.range(from..)
            .take_while(|(key, _)| key.namespace == namespace && key.kind == kind)
            .filter(|(_, env)| keep(env))
            .map(|(_, env)| env.clone())
            .collect()
    }

    pub fn list(&self, namespace: &str, kind: &str, opts: &ListOptions) -> ListPage {
        let filtered = self.kind_where(namespace, kind, |env| {
            opts.label_selector
                .as_ref()
                .is_none_or(|ls| ls.matches(&env.metadata.labels))
                && opts
                    .field_selector
                    .as_ref()
                    .is_none_or(|fs| fs.matches(env))
        });

        let mut items = filtered;
        if let Some(ref token) = opts.continue_token {
            items.retain(|item| item.metadata.name.as_str() > token.as_str());
        }

        let total = items.len();
        match opts.limit {
            Some(limit) if limit < total => {
                let page_items: Vec<ResourceEnvelope> = items.into_iter().take(limit).collect();
                let continue_token = page_items.last().map(|i| i.metadata.name.clone());
                let remaining = total - limit;
                ListPage {
                    items: page_items,
                    continue_token,
                    remaining,
                }
            }
            _ => ListPage {
                items,
                continue_token: None,
                remaining: 0,
            },
        }
    }

    /// What `fresh` holds of `kinds` that this mirror lacks or holds with another spec, as
    /// `fresh` has it.
    pub fn behind(&self, fresh: &Mirror, kinds: &[&str]) -> Vec<ResourceEnvelope> {
        let held = self.resources.read().unwrap_or_else(|p| p.into_inner());
        let fresh = fresh.resources.read().unwrap_or_else(|p| p.into_inner());
        fresh
            .iter()
            .filter(|(key, env)| {
                kinds.contains(&key.kind.as_str())
                    && held.get(*key).is_none_or(|old| old.spec != env.spec)
            })
            .map(|(_, env)| env.clone())
            .collect()
    }

    /// A copy of what the mirror holds now, to overlay without touching the original.
    pub fn snapshot(&self) -> Mirror {
        let copy = Mirror::new();
        copy.replace_all(self);
        copy
    }
}

/// The manifest one file holds, when it holds one of a kind and version this platform serves;
/// anything else, a malformed file included, is no manifest.
pub fn envelope_of(content: &str) -> Option<ResourceEnvelope> {
    let value: serde_norway::Value = serde_norway::from_str(content).ok()?;
    let api_version = value.get("apiVersion")?.as_str()?;
    let kind = value.get("kind")?.as_str()?;
    if !jc_core::serves(kind, api_version) {
        return None;
    }
    let envelope: ResourceEnvelope = serde_norway::from_value(value).ok()?;
    crate::resource::validate_meta(&envelope.metadata).ok()?;
    Some(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ObjectMeta;
    use std::collections::BTreeMap;

    fn sample(
        namespace: &str,
        kind: &str,
        name: &str,
        labels: &[(&str, &str)],
    ) -> ResourceEnvelope {
        let label_map: BTreeMap<String, String> = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_string(),
            kind: kind.to_string(),
            metadata: ObjectMeta {
                name: name.to_string(),
                namespace: Some(namespace.to_string()),
                labels: label_map,
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        }
    }

    /// T-2544, MF-02: a name that is not a name of its kind is left out, like a bad namespace;
    /// a space keeps its own, looser form.
    #[test]
    fn kind_where_reads_one_namespace_and_kind_by_name() {
        let mirror = Mirror::new();
        for (ns, kind, name) in [
            ("org", "RoleBinding", "b"),
            ("org", "RoleBinding", "a"),
            ("org", "RoleBinding", "c"),
            ("org", "Role", "a"),
            ("orga", "RoleBinding", "a"),
            ("doprava", "RoleBinding", "a"),
        ] {
            mirror.upsert(sample(ns, kind, name, &[]));
        }
        let names = |envs: Vec<ResourceEnvelope>| -> Vec<String> {
            envs.into_iter().map(|e| e.metadata.name).collect()
        };
        assert_eq!(
            names(mirror.kind_where("org", "RoleBinding", |_| true)),
            ["a", "b", "c"]
        );
        assert_eq!(
            names(mirror.kind_where("org", "RoleBinding", |e| e.metadata.name != "b")),
            ["a", "c"]
        );
        assert!(mirror.kind_where("org", "Policy", |_| true).is_empty());
    }

    #[test]
    fn a_name_outside_its_kinds_form_is_left_out_of_the_mirror() {
        let mirror = Mirror::new();
        for name in [
            "a\nb",
            "open\n\n## THIS TURN",
            "Upper",
            "a.b",
            "",
            &"a".repeat(64),
        ] {
            mirror.upsert(sample("helsinki", "Endpoint", name, &[]));
            assert!(
                mirror.get("helsinki", "Endpoint", name).is_none(),
                "{name:?}"
            );
        }
        mirror.upsert(sample("helsinki", "Endpoint", "bikes-open", &[]));
        assert!(mirror.get("helsinki", "Endpoint", "bikes-open").is_some());
        mirror.upsert(sample("helsinki", "ContextSpace", "air-", &[]));
        assert!(mirror.get("helsinki", "ContextSpace", "air-").is_some());
        mirror.upsert(sample("helsinki", "ContextSpace", "air\n", &[]));
        assert!(mirror.get("helsinki", "ContextSpace", "air\n").is_none());
    }

    #[test]
    fn replace_all_replaces_resources() {
        let mirror1 = Mirror::new();
        mirror1.upsert(sample("ns1", "Endpoint", "old-a", &[]));
        assert_eq!(mirror1.len(), 1);

        let mirror2 = Mirror::new();
        mirror2.upsert(sample("ns2", "Endpoint", "new-b", &[]));
        mirror2.upsert(sample("ns2", "Endpoint", "new-c", &[]));

        mirror1.replace_all(&mirror2);
        assert_eq!(mirror1.len(), 2);
        assert!(mirror1.get("ns1", "Endpoint", "old-a").is_none());
        assert!(mirror1.get("ns2", "Endpoint", "new-b").is_some());
        assert!(mirror1.get("ns2", "Endpoint", "new-c").is_some());
    }

    #[test]
    fn namespace_and_kind_isolation() {
        let mirror = Mirror::new();
        mirror.upsert(sample("ns1", "Endpoint", "res-a", &[]));
        mirror.upsert(sample("ns2", "Endpoint", "res-a", &[]));
        mirror.upsert(sample("ns1", "ContextSpace", "res-a", &[]));

        assert_eq!(mirror.len(), 3);

        let list_ns1_endpoints = mirror.list("ns1", "Endpoint", &ListOptions::default());
        assert_eq!(list_ns1_endpoints.items.len(), 1);
        assert_eq!(list_ns1_endpoints.items[0].metadata.name, "res-a");
        assert_eq!(
            list_ns1_endpoints.items[0].metadata.namespace.as_deref(),
            Some("ns1")
        );

        let list_ns2_endpoints = mirror.list("ns2", "Endpoint", &ListOptions::default());
        assert_eq!(list_ns2_endpoints.items.len(), 1);
        assert_eq!(
            list_ns2_endpoints.items[0].metadata.namespace.as_deref(),
            Some("ns2")
        );

        let list_ns1_spaces = mirror.list("ns1", "ContextSpace", &ListOptions::default());
        assert_eq!(list_ns1_spaces.items.len(), 1);
        assert_eq!(list_ns1_spaces.items[0].kind, "ContextSpace");
    }

    #[test]
    fn selector_filtering() {
        let mirror = Mirror::new();
        mirror.upsert(sample(
            "ovzdusie",
            "Endpoint",
            "public-air",
            &[("env", "prod")],
        ));
        mirror.upsert(sample(
            "ovzdusie",
            "Endpoint",
            "test-air",
            &[("env", "dev")],
        ));

        let opts_label = ListOptions {
            label_selector: Some(LabelSelector::parse("env=prod").expect("selector")),
            ..Default::default()
        };
        let page = mirror.list("ovzdusie", "Endpoint", &opts_label);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].metadata.name, "public-air");

        let opts_field = ListOptions {
            field_selector: Some(FieldSelector::parse("metadata.name=test-air").expect("selector")),
            ..Default::default()
        };
        let page_field = mirror.list("ovzdusie", "Endpoint", &opts_field);
        assert_eq!(page_field.items.len(), 1);
        assert_eq!(page_field.items[0].metadata.name, "test-air");
    }

    #[test]
    fn two_page_walk_returns_every_item_exactly_once() {
        let mirror = Mirror::new();
        for name in &["item-1", "item-2", "item-3", "item-4", "item-5"] {
            mirror.upsert(sample("ovzdusie", "Endpoint", name, &[]));
        }

        let page1 = mirror.list(
            "ovzdusie",
            "Endpoint",
            &ListOptions {
                limit: Some(3),
                continue_token: None,
                ..Default::default()
            },
        );
        assert_eq!(page1.items.len(), 3);
        assert_eq!(page1.remaining, 2);
        assert_eq!(page1.items[0].metadata.name, "item-1");
        assert_eq!(page1.items[1].metadata.name, "item-2");
        assert_eq!(page1.items[2].metadata.name, "item-3");
        let token = page1.continue_token.expect("continue token");

        let page2 = mirror.list(
            "ovzdusie",
            "Endpoint",
            &ListOptions {
                limit: Some(3),
                continue_token: Some(token),
                ..Default::default()
            },
        );
        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.remaining, 0);
        assert_eq!(page2.items[0].metadata.name, "item-4");
        assert_eq!(page2.items[1].metadata.name, "item-5");
        assert!(page2.continue_token.is_none());
    }

    #[test]
    fn namespaces_are_distinct_sorted_and_skip_cluster_scope() {
        let mirror = Mirror::new();
        mirror.upsert(sample("helsinki", "ContextSpace", "helsinki", &[]));
        mirror.upsert(sample("banskabystrica", "Endpoint", "public-air", &[]));
        mirror.upsert(sample("banskabystrica", "ContextSpace", "ovzdusie", &[]));
        mirror.upsert(sample("", "Organization", "hel", &[]));
        mirror.upsert(sample("org", "Blueprint", "mqtt-feed", &[]));
        mirror.upsert(sample("org", "Organization", "hel-fi", &[]));
        assert_eq!(mirror.namespaces(), vec!["banskabystrica", "helsinki"]);
        assert!(Mirror::new().namespaces().is_empty());
    }
}
