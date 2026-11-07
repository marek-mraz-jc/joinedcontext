// SPDX-License-Identifier: EUPL-1.2
//! The documents of one narrowed schema (rows L1 to L9) under the
//! platform's names and media types (`gateway-to-broker-map.md` M11: the
//! Portal, the SDK and the App generator read them), and the cache that
//! renders them once per view, schema and visible set.

use crate::narrow::{narrow, LIMITS};
use antares_ops::Visible;
use linkml_model::{Model, Range};
use serde_json::{json, Map, Value};
use sha2::{Digest as _, Sha256};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};

/// The media type of the index and of a class or slot.
pub(crate) const JSON: &str = "application/json";

/// One document a schema major publishes (L2 to L6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    /// LinkML YAML.
    LinkMl,
    /// JSON Schema.
    JsonSchema,
    /// The JSON-LD `@context`.
    Context,
    /// SHACL shapes.
    Shacl,
    /// OWL.
    Owl,
    /// RDFS.
    Rdf,
    /// Markdown.
    Markdown,
    /// The RDF Data Cube structure, only when a kept class is a Data
    /// Structure Definition (DM-60).
    Qb,
}

impl Artifact {
    /// Every document, in the order an agent should read them.
    pub const ALL: [Self; 8] = [
        Self::LinkMl,
        Self::JsonSchema,
        Self::Context,
        Self::Shacl,
        Self::Owl,
        Self::Rdf,
        Self::Markdown,
        Self::Qb,
    ];

    /// The file name it is published under.
    ///
    /// ```
    /// assert_eq!(antares_linkml::Artifact::Context.file_name(), "context.jsonld");
    /// ```
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::LinkMl => "model.linkml.yaml",
            Self::JsonSchema => "model.schema.json",
            Self::Context => "context.jsonld",
            Self::Shacl => "model.shacl.ttl",
            Self::Owl => "model.owl.ttl",
            Self::Rdf => "model.rdf.ttl",
            Self::Markdown => "model.md",
            Self::Qb => "model.qb.ttl",
        }
    }

    /// Its media type.
    ///
    /// ```
    /// assert_eq!(antares_linkml::Artifact::LinkMl.media_type(), "text/yaml");
    /// ```
    #[must_use]
    pub fn media_type(self) -> &'static str {
        match self {
            Self::LinkMl => "text/yaml",
            Self::JsonSchema => "application/schema+json",
            Self::Context => "application/ld+json",
            Self::Shacl | Self::Rdf | Self::Qb => "text/turtle",
            Self::Owl => "text/turtle; profile=\"owl\"",
            Self::Markdown => "text/markdown",
        }
    }

    /// The document one path segment names: a file name, its short name, or
    /// `model`, which `accept` picks the formalism of (L7); `model` never
    /// picks the cube, which a model has only when it declares one (L6).
    ///
    /// ```
    /// use antares_linkml::Artifact;
    /// assert_eq!(Artifact::of("shacl", ""), Some(Artifact::Shacl));
    /// assert_eq!(Artifact::of("model", "text/yaml"), Some(Artifact::LinkMl));
    /// assert_eq!(Artifact::of("qb", ""), Some(Artifact::Qb));
    /// ```
    #[must_use]
    pub fn of(segment: &str, accept: &str) -> Option<Self> {
        match segment {
            "model.linkml.yaml" | "linkml" => Some(Self::LinkMl),
            "model.schema.json" | "json-schema" => Some(Self::JsonSchema),
            "context.jsonld" | "context" => Some(Self::Context),
            "model.shacl.ttl" | "shacl" => Some(Self::Shacl),
            "model.owl.ttl" | "owl" => Some(Self::Owl),
            "model.rdf.ttl" | "rdf" => Some(Self::Rdf),
            "model.md" | "docs" => Some(Self::Markdown),
            "model.qb.ttl" | "qb" => Some(Self::Qb),
            "model" => Some(Self::negotiated(accept)),
            _ => None,
        }
    }

    /// The formalism `accept` asks `model` for, as the platform picks it.
    fn negotiated(accept: &str) -> Self {
        if accept.contains("turtle") {
            if accept.contains("owl") {
                Self::Owl
            } else if accept.contains("rdf") {
                Self::Rdf
            } else {
                Self::Shacl
            }
        } else if accept.contains("yaml") {
            Self::LinkMl
        } else if accept.contains("markdown") {
            Self::Markdown
        } else if accept.contains("ld+json") {
            Self::Context
        } else {
            Self::JsonSchema
        }
    }

    /// The document of `model`; `None` for the cube of a model that
    /// declares no Data Structure Definition, which then does not exist.
    fn render(self, model: &Model) -> Option<String> {
        Some(match self {
            Self::LinkMl => linkml_model::yaml(model),
            Self::JsonSchema => linkml_model::json_schema(model),
            Self::Context => linkml_model::context(model),
            Self::Shacl => linkml_model::shacl(model),
            Self::Owl => linkml_model::owl(model),
            Self::Rdf => linkml_model::rdfs(model),
            Self::Markdown => linkml_model::markdown(model),
            Self::Qb => return linkml_model::qb(model),
        })
    }
}

/// The lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Every document of one narrowed schema, rendered once.
#[derive(Debug)]
pub(crate) struct Rendered {
    model: Model,
    /// The schema's major version, the `v{major}` of its paths.
    pub(crate) major: String,
    documents: Vec<(Artifact, Arc<[u8]>)>,
    /// `schema/index.json` (L1).
    pub(crate) index: Arc<[u8]>,
}

impl Rendered {
    /// The bytes of its documents and index.
    fn bytes(&self) -> usize {
        self.documents
            .iter()
            .fold(self.index.len(), |n, (_, d)| n.saturating_add(d.len()))
    }

    fn new(model: Model, view: &str, left_out: bool) -> Self {
        let major = major(model.schema().version.as_deref());
        let documents: Vec<(Artifact, Arc<[u8]>)> = Artifact::ALL
            .iter()
            .filter_map(|a| Some((*a, Arc::from(a.render(&model)?.into_bytes()))))
            .collect();
        let artifacts: Map<String, Value> = documents
            .iter()
            .map(|(a, bytes)| {
                let described =
                    json!({"type": a.media_type(), "bytes": bytes.len(), "sha256": sha256(bytes)});
                (a.file_name().to_owned(), described)
            })
            .collect();
        let s = model.schema();
        let mut described = Map::new();
        described.insert("name".into(), json!(s.name));
        // a number, as the platform's index writes it (the SDK keeps only an integer, T-3963);
        // `major` holds digits that fit a u64, so the fallback never fires
        described.insert("version".into(), json!(major.parse::<u64>().unwrap_or(1)));
        described.insert("semver".into(), json!(s.version));
        described.insert("types".into(), json!(s.classes.keys().collect::<Vec<_>>()));
        described.insert("artifacts".into(), Value::Object(artifacts));
        if left_out {
            // Whether anything was left out, never what (the platform's EP-47).
            described.insert("redacted".into(), Value::Bool(true));
        }
        let index = json!({"endpoint": view, "models": [Value::Object(described)]});
        Self {
            index: Arc::from(index.to_string().into_bytes()),
            model,
            major,
            documents,
        }
    }

    pub(crate) fn document(&self, artifact: Artifact) -> Option<Arc<[u8]>> {
        self.documents
            .iter()
            .find(|(a, _)| *a == artifact)
            .map(|(_, b)| b.clone())
    }

    /// One visible class with its slots, its own and inherited (L8).
    pub(crate) fn class(&self, name: &str) -> Option<Arc<[u8]>> {
        let m = &self.model;
        let class = m.schema().classes.get(name)?;
        let slots: Vec<Value> = m
            .class_slots(name)
            .iter()
            .map(|(n, s)| Value::Object(self.slot_json(n, s)))
            .collect();
        let mut doc = Map::new();
        doc.insert("name".into(), json!(name));
        doc.insert("iri".into(), json!(m.class_iri(name)));
        doc.insert("slots".into(), Value::Array(slots));
        if let Some(parent) = &class.is_a {
            doc.insert("is_a".into(), json!(parent));
        }
        if let Some(d) = &class.description {
            doc.insert("description".into(), json!(d));
        }
        Some(Arc::from(Value::Object(doc).to_string().into_bytes()))
    }

    /// One visible slot with the visible classes that use it (L9).
    pub(crate) fn slot(&self, name: &str) -> Option<Arc<[u8]>> {
        let m = &self.model;
        let slot = m.schema().slots.get(name)?;
        let classes: Vec<&String> = m
            .schema()
            .classes
            .keys()
            .filter(|c| m.class_slots(c).iter().any(|(n, _)| *n == name))
            .collect();
        let mut doc = self.slot_json(name, slot);
        doc.insert("classes".into(), json!(classes));
        Some(Arc::from(Value::Object(doc).to_string().into_bytes()))
    }

    fn slot_json(&self, name: &str, slot: &linkml_model::Slot) -> Map<String, Value> {
        let m = &self.model;
        let range = match m.range(slot) {
            Range::Type(b) => b.name(),
            Range::Class(c) | Range::Enum(c) => c,
        };
        let mut doc = Map::new();
        doc.insert("name".into(), json!(name));
        doc.insert("iri".into(), json!(m.slot_iri(name)));
        doc.insert("range".into(), json!(range));
        doc.insert("kind".into(), json!(m.kind(slot)));
        doc.insert("required".into(), json!(slot.required.unwrap_or(false)));
        doc.insert(
            "multivalued".into(),
            json!(slot.multivalued.unwrap_or(false)),
        );
        if let Some(d) = &slot.description {
            doc.insert("description".into(), json!(d));
        }
        doc
    }
}

/// The leading number of a semantic version, `1` for none or for one past `u64`.
fn major(version: Option<&str>) -> String {
    version
        .and_then(|v| v.split('.').next())
        .filter(|m| m.bytes().all(|b| b.is_ascii_digit()) && m.parse::<u64>().is_ok())
        .unwrap_or("1")
        .to_owned()
}

struct Entry {
    view: String,
    schema: Arc<[u8]>,
    visible: Visible,
    rendered: Arc<Rendered>,
    /// The bytes it holds: its documents and the schema they were made of.
    weight: usize,
}

/// The bytes of rendered schemas one pod keeps by default (T-4354), beside
/// the count bound: a view of a 1 MiB schema renders a few MiB of documents.
/// 8 MiB is this cache's share of the broker's default 200 MiB memory budget
/// (T-5895); [`crate::set_cache_bytes`] sets another.
pub(crate) const MAX_BYTES: usize = 8 << 20;

pub(crate) static CACHE_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(MAX_BYTES);

/// The rendered schemas of this pod, keyed by view id, the schema's bytes
/// and the visible set (LK2: a visible set is part of the key, so no
/// caller is answered from another's narrowing), at most
/// [`CACHE_ENTRIES`](crate::CACHE_ENTRIES) and [`MAX_BYTES`] of them, the
/// oldest dropped first. The schema each is read from comes from
/// [`linkml_model::cache`], one parse per revision shared with the `OData`
/// surface.
#[derive(Default)]
pub(crate) struct Cache {
    entries: Mutex<(VecDeque<Entry>, usize)>,
}

impl Cache {
    /// The stored schema `schema` of `view`, revision `revision`, as read;
    /// `None` when it does not load (LK4: a metaslot outside the subset
    /// closes the view's LinkML surface, the reason goes to the log once).
    pub(crate) fn source(view: &str, revision: u64, schema: &Arc<[u8]>) -> Option<Arc<Model>> {
        match linkml_model::cache::read(revision, schema, &LIMITS) {
            (Ok(model), _) => Some(model),
            (Err(e), fresh) => {
                if fresh {
                    tracing::warn!(view, error = %e, "the view's LinkML schema does not load; its LinkML surface is closed");
                }
                None
            }
        }
    }

    /// The rendered schema of `view` for `visible`, rendered on a miss;
    /// `None` when the schema does not load (see [`Cache::source`]).
    pub(crate) fn get(
        &self,
        view: &str,
        revision: u64,
        schema: &Arc<[u8]>,
        visible: &Visible,
    ) -> Option<Arc<Rendered>> {
        {
            let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let hit = entries.0.iter().find(|e| {
                e.view == view
                    && (Arc::ptr_eq(&e.schema, schema) || e.schema == *schema)
                    && e.visible == *visible
            });
            if let Some(e) = hit {
                return Some(e.rendered.clone());
            }
        }
        let source = Self::source(view, revision, schema)?;
        let rendered = match narrow(&source, visible, view) {
            Ok((model, left_out)) => Arc::new(Rendered::new(model, view, left_out)),
            Err(e) => {
                tracing::warn!(view, error = %e, "the view's narrowed LinkML schema does not load");
                return None;
            }
        };
        let weight = rendered.bytes().saturating_add(schema.len());
        let mut guard = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let (entries, held) = &mut *guard;
        entries.push_back(Entry {
            view: view.to_owned(),
            schema: schema.clone(),
            visible: visible.clone(),
            rendered: rendered.clone(),
            weight,
        });
        *held = held.saturating_add(weight);
        // The oldest go first; a rendering past the whole budget alone would go too, answered
        // and not kept (a 1 MiB schema renders a few MiB, so none does).
        while entries.len() > crate::CACHE_ENTRIES
            || *held > CACHE_BYTES.load(std::sync::atomic::Ordering::Relaxed)
        {
            *held = held.saturating_sub(entries.pop_front().map_or(0, |old| old.weight));
        }
        Some(rendered)
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The revision a store gives `bytes`: their digest, as `Schema::new` makes it.
    fn rev(bytes: &[u8]) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }
    use antares_ops::policy::{Filter, Rule};

    const SCHEMA: &[u8] = br#"{"id": "https://e.org/s", "name": "s", "version": "3.0.1",
        "classes": {"Room": {"slots": ["temperature", "secret"]}},
        "slots": {"temperature": {"range": "float"}, "secret": {}}}"#;

    fn visible(attrs: &[&str]) -> Visible {
        Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: vec!["https://e.org/s/Room".into()],
                attributes: attrs
                    .iter()
                    .map(|a| format!("https://e.org/s/{a}"))
                    .collect(),
                ..Rule::default()
            }],
            ..Filter::default()
        })
    }

    #[test]
    fn the_major_is_the_leading_number_or_1() {
        assert_eq!(major(Some("3.0.1")), "3");
        assert_eq!(major(Some("12")), "12");
        assert_eq!(major(Some("v3")), "1");
        assert_eq!(major(Some(".1")), "1");
        assert_eq!(major(None), "1");
        assert_eq!(major(Some("99999999999999999999999.0.0")), "1");
    }

    #[test]
    fn linkml_schema_is_built_once_per_cache_key() {
        let cache = Cache::default();
        let schema: Arc<[u8]> = Arc::from(SCHEMA);
        let a = cache
            .get("v", rev(&schema), &schema, &visible(&["temperature"]))
            .expect("renders");
        let again = cache
            .get(
                "v",
                rev(SCHEMA),
                &Arc::from(SCHEMA),
                &visible(&["temperature"]),
            )
            .expect("hit");
        assert!(
            Arc::ptr_eq(&a, &again),
            "same bytes, same visible set: one render"
        );
        let wider = cache
            .get(
                "v",
                rev(&schema),
                &schema,
                &visible(&["temperature", "secret"]),
            )
            .expect("renders");
        assert!(!Arc::ptr_eq(&a, &wider));
        let other_view = cache
            .get("w", rev(&schema), &schema, &visible(&["temperature"]))
            .expect("renders");
        assert!(!Arc::ptr_eq(&a, &other_view));
        assert_eq!(cache.len(), 3);
        let yaml = a.document(Artifact::LinkMl).expect("yaml");
        assert!(!String::from_utf8_lossy(&yaml).contains("secret"));
    }

    #[test]
    fn the_cache_holds_at_most_its_bound() {
        let cache = Cache::default();
        let schema: Arc<[u8]> = Arc::from(SCHEMA);
        for i in 0..=crate::CACHE_ENTRIES {
            cache.get(&format!("v{i}"), rev(&schema), &schema, &visible(&[]));
        }
        assert_eq!(cache.len(), crate::CACHE_ENTRIES);
    }

    /// 20 schemas of 1 MiB, each rendered for two visible sets, stay under the byte bound
    /// (T-4354): a count bound alone would hold 256 renderings of several MiB each.
    #[test]
    fn twenty_schemas_of_a_mebibyte_stay_under_the_byte_bound() {
        let cache = Cache::default();
        for n in 0..20 {
            // Within the surface's 1 MiB, so every one renders.
            let pad = "x".repeat(1_000_000);
            let schema: Arc<[u8]> = Arc::from(
                format!(
                    r#"{{"id": "https://e.org/s", "name": "s", "version": "1.{n}",
                      "classes": {{"Room": {{"slots": ["temperature"], "description": "{pad}"}}}},
                      "slots": {{"temperature": {{}}}}}}"#
                )
                .into_bytes(),
            );
            assert!(schema.len() <= LIMITS.max_bytes);
            for shown in [&[][..], &["temperature"][..]] {
                cache
                    .get("v", rev(&schema), &schema, &visible(shown))
                    .expect("renders");
            }
            assert!(cache.held() <= MAX_BYTES, "{} > {MAX_BYTES}", cache.held());
        }
        // 40 renderings of several MiB each: the oldest went, the newest stayed.
        assert!((1..40).contains(&cache.len()), "{}", cache.len());
    }

    #[test]
    fn a_schema_outside_the_subset_renders_nothing() {
        let bad: Arc<[u8]> =
            Arc::from(&br#"{"id": "https://e.org/s", "name": "s", "imports": []}"#[..]);
        let cache = Cache::default();
        assert!(cache.get("v", rev(&bad), &bad, &visible(&[])).is_none());
        // Refused again from the parse cache, the reason logged the first time only.
        assert!(cache.get("v", rev(&bad), &bad, &visible(&[])).is_none());
    }

    #[test]
    fn the_index_lists_each_artifact_with_its_size_and_digest() {
        let r = Cache::default()
            .get(
                "v",
                rev(SCHEMA),
                &Arc::from(SCHEMA),
                &visible(&["temperature"]),
            )
            .expect("renders");
        let index: Value = serde_json::from_slice(&r.index).expect("json");
        let model = &index["models"][0];
        assert_eq!(index["endpoint"], "v");
        assert_eq!(
            (model["version"].as_u64(), model["redacted"].as_bool()),
            (Some(3), Some(true))
        );
        let yaml = r.document(Artifact::LinkMl).expect("yaml");
        assert_eq!(
            model["artifacts"]["model.linkml.yaml"]["sha256"],
            sha256(&yaml)
        );
        assert_eq!(model["artifacts"]["model.linkml.yaml"]["bytes"], yaml.len());
        assert_eq!(model["artifacts"].as_object().map(Map::len), Some(7));
    }

    /// The index's `version` is the major as a JSON integer, as the platform's index writes it:
    /// 1 for a schema with no version, 2 for `2.3.0` (T-3963).
    #[test]
    fn the_index_version_is_an_integer() {
        for (schema, want) in [
            (&br#"{"id": "https://e.org/s", "name": "s"}"#[..], 1),
            (
                &br#"{"id": "https://e.org/s", "name": "s", "version": "2.3.0"}"#[..],
                2,
            ),
        ] {
            let r = Cache::default()
                .get("v", rev(schema), &Arc::from(schema), &visible(&[]))
                .expect("renders");
            let index: Value = serde_json::from_slice(&r.index).expect("json");
            assert!(index["models"][0]["version"].is_u64(), "{index}");
            assert_eq!(index["models"][0]["version"], want);
            assert_eq!(r.major, want.to_string());
        }
    }

    #[test]
    fn model_picks_the_formalism_accept_names() {
        for (accept, artifact) in [
            ("text/turtle", Artifact::Shacl),
            ("text/turtle; profile=\"owl\"", Artifact::Owl),
            ("text/turtle; profile=\"rdf\"", Artifact::Rdf),
            ("text/yaml", Artifact::LinkMl),
            ("text/markdown", Artifact::Markdown),
            ("application/ld+json", Artifact::Context),
            ("application/json", Artifact::JsonSchema),
            ("", Artifact::JsonSchema),
        ] {
            assert_eq!(Artifact::of("model", accept), Some(artifact), "{accept}");
        }
    }

    #[test]
    fn a_schema_is_read_once_per_revision_and_each_view_schema_pair_rendered_apart() {
        let cache = Cache::default();
        let a = Cache::source("v", rev(SCHEMA), &Arc::from(SCHEMA)).expect("reads");
        let again = Cache::source("w", rev(SCHEMA), &Arc::from(SCHEMA)).expect("hit");
        assert!(Arc::ptr_eq(&a, &again), "one revision, one read");
        let other: Arc<[u8]> = Arc::from(&br#"{"id": "https://e.org/t", "name": "t"}"#[..]);
        let first = cache
            .get("v", rev(SCHEMA), &Arc::from(SCHEMA), &visible(&[]))
            .expect("renders");
        let second = cache
            .get("v", rev(&other), &other, &visible(&[]))
            .expect("renders");
        assert!(!Arc::ptr_eq(&first, &second), "same view, another schema");
    }

    #[test]
    fn a_class_and_a_slot_carry_what_they_state() {
        let schema: Arc<[u8]> = Arc::from(
            &br#"{"id": "https://e.org/s", "name": "s",
              "classes": {"Thing": {"slots": ["label"], "description": "Anything."},
                          "Room": {"is_a": "Thing", "slots": ["temperature"]}},
              "slots": {"label": {"description": "What it is called."}, "temperature": {}}}"#[..],
        );
        let all = Visible::from_filter(&Filter {
            rules: vec![Rule {
                types: vec![
                    "https://e.org/s/Thing".into(),
                    "https://e.org/s/Room".into(),
                ],
                attributes: vec![
                    "https://e.org/s/label".into(),
                    "https://e.org/s/temperature".into(),
                ],
                ..Rule::default()
            }],
            ..Filter::default()
        });
        let r = Cache::default()
            .get("v", rev(&schema), &schema, &all)
            .expect("renders");
        let json = |b: Option<Arc<[u8]>>| -> Value {
            serde_json::from_slice(&b.expect("served")).expect("json")
        };
        let room = json(r.class("Room"));
        assert_eq!(
            (room["is_a"].as_str(), room.get("description")),
            (Some("Thing"), None)
        );
        let thing = json(r.class("Thing"));
        assert_eq!(
            (thing.get("is_a"), thing["description"].as_str()),
            (None, Some("Anything."))
        );
        assert_eq!(json(r.slot("label"))["description"], "What it is called.");
        assert_eq!(json(r.slot("temperature")).get("description"), None);
        assert_eq!(json(r.slot("temperature"))["classes"], json!(["Room"]));
        let index: Value = serde_json::from_slice(&r.index).expect("json");
        assert_eq!(
            index["models"][0].get("redacted"),
            None,
            "nothing was left out"
        );
        assert!(r.class("Nothing").is_none());
        assert!(r.slot("nothing").is_none());
    }
}
