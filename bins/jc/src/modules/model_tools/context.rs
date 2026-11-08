//! LinkML → JSON-LD `@context` with NGSI-LD kind bindings (gen_context.py, DM-02, DM-04, DM-05):
//! what LinkML's `ContextGenerator` writes for the merged model (prefixes, `@vocab`, a term
//! per slot coerced by its range), then every class and slot bound to its IRI and a slot's
//! `ngsi_ld_kind` turned into the JSON-LD keyword NGSI-LD payloads expand with.

use super::checks::ngsi_ld_kind;
use super::schema::{
    Obj, View, annotation_value, attributes, camelcase, curie_for, mangled_attribute_name, py_str,
    slot_usage_name, texts, truthy, underscore,
};
use serde_json::{Map, Value, json};

/// Namespaces an organisation must never mint its own terms under (DM-04, DM-16).
const RESERVED_NAMESPACES: [&str; 8] = [
    "https://smartdatamodels.org/",
    "https://raw.githubusercontent.com/smart-data-models/",
    "https://github.com/smart-data-models/",
    "https://uri.etsi.org/",
    "http://uri.etsi.org/",
    "https://www.w3.org/",
    "http://www.w3.org/",
    "https://w3id.org/linkml/",
];

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
/// SHEX nonliteral, bnode and iri: a value of these types is an IRI (`@id`).
const URI_RANGES: [&str; 3] = [
    "http://www.w3.org/ns/shex#nonLiteral",
    "http://www.w3.org/ns/shex#bnode",
    "http://www.w3.org/ns/shex#iri",
];

/// The `ContextGenerator`'s namespace table and the prefixes it emits.
struct Generator {
    namespaces: Vec<(String, String)>,
    emit: std::collections::BTreeSet<String>,
    vocab: String,
}

impl Generator {
    fn ns(&self, prefix: &str) -> Option<&str> {
        self.namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, n)| n.as_str())
    }

    fn add_prefix(&mut self, prefix: &str) {
        if self.ns(prefix).is_none() {
            self.namespaces.push((
                prefix.to_owned(),
                format!("http://example.org/UNKNOWN/{prefix}/"),
            ));
        }
        self.emit.insert(prefix.to_owned());
    }

    /// `Namespaces.prefix_suffix`.
    fn prefix_suffix(&self, uri: &str) -> Result<(Option<String>, Option<String>), String> {
        let curie;
        let mut text = uri;
        if uri.contains("://") {
            match curie_for(&self.namespaces, uri) {
                Some(c) => {
                    curie = c;
                    text = &curie;
                }
                None => return Ok((None, None)),
            }
        }
        let parts: Vec<&str> = text.split(':').collect();
        match parts.as_slice() {
            [p] => Ok((Some((*p).to_owned()), Some(String::new()))),
            [p, s] => Ok((Some((*p).to_owned()), Some((*s).to_owned()))),
            _ => Err(format!("too many values to unpack (expected 2): {text}")),
        }
    }

    /// `Namespaces.uri_for`.
    fn uri_for(&self, uri: &str) -> Result<String, String> {
        if uri.contains("://") {
            return Ok(uri.to_owned());
        }
        match uri.split_once(':') {
            Some((p, local)) => match self.ns(p) {
                Some(ns) => Ok(format!("{ns}{local}")),
                None => Err(format!("Unknown CURIE prefix: {p}")),
            },
            None => Err(format!("Unknown CURIE prefix: @base ({uri})")),
        }
    }

    /// `_build_element_id`: the `@id` of a generated term, emitting its prefix.
    fn element_id(&mut self, uri: &str) -> Result<String, String> {
        let (prefix, suffix) = self.prefix_suffix(uri)?;
        let vocab_prefix = self.prefix_suffix(&self.vocab.clone())?.0;
        let is_default = prefix.as_deref() == Some(self.vocab.as_str())
            || (prefix.is_some() && prefix == vocab_prefix);
        let prefix = prefix.filter(|p| !p.is_empty());
        let suffix = suffix.filter(|s| !s.is_empty());
        let id = match (&prefix, &suffix) {
            (None, None) => uri.to_owned(),
            (None, Some(s)) => s.clone(),
            (Some(_), _) if is_default => suffix.clone().unwrap_or_default(),
            (Some(p), _) => format!("{p}:{}", suffix.clone().unwrap_or_default()),
        };
        if let Some(p) = prefix.filter(|_| !is_default) {
            self.add_prefix(&p);
        }
        Ok(id)
    }

    /// `add_mappings`: every mapping's prefix (and the element's own IRI's) is emitted.
    fn add_mappings(&mut self, name: &str, element: &Obj, uri: &str) -> Result<(), String> {
        for p in texts(element.get("id_prefixes")) {
            self.add_prefix(&p);
        }
        let mut mappings = Vec::new();
        for key in [
            "mappings",
            "related_mappings",
            "close_mappings",
            "narrow_mappings",
            "broad_mappings",
            "exact_mappings",
        ] {
            mappings.extend(texts(element.get(key)));
        }
        mappings.push(uri.to_owned());
        for mapping in mappings {
            let mapping = if mapping.contains("://") {
                match curie_for(&self.namespaces, &mapping) {
                    Some(c) => c,
                    None => continue,
                }
            } else {
                mapping
            };
            let parts: Vec<&str> = mapping.split(':').collect();
            let [prefix, _] = parts.as_slice() else {
                return Err(format!(
                    "Definition {name} - unrecognized prefix: {mapping}"
                ));
            };
            let prefix = (*prefix).to_owned();
            self.add_prefix(&prefix);
        }
        Ok(())
    }

    /// The `@type` a LinkML type range coerces to: none for a string, `@id` for an IRI type.
    fn type_coercion(&self, view: &View, range: &str) -> Result<Option<String>, String> {
        let uri = type_uri(view, range).ok_or_else(|| format!("KeyError: '{range}'"))?;
        let full = self.uri_for(&uri)?;
        Ok(if full == XSD_STRING {
            None
        } else if URI_RANGES.contains(&full.as_str()) {
            Some("@id".to_owned())
        } else {
            Some(uri)
        })
    }
}

/// A type's `uri`, through `typeof` when it declares none.
fn type_uri(view: &View, name: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let mut current = name.to_owned();
    loop {
        let t = view.type_def(&current)?;
        if let Some(uri) = t.get("uri").filter(|v| truthy(Some(v))) {
            return Some(py_str(uri));
        }
        let parent = t.get("typeof").filter(|v| !v.is_null()).map(py_str)?;
        if !seen.insert(parent.clone()) {
            return None;
        }
        current = parent;
    }
}

/// One slot as the `ContextGenerator` visits it (SchemaLoader promotes attributes and
/// slot_usage entries to slots of their own).
struct GenSlot<'a> {
    name: String,
    alias: String,
    element: &'a Obj,
    identifier: bool,
    usage: bool,
}

/// The JSON-LD `@context` of one merged model.
pub(super) fn context(view: &View, generator_version: &str) -> Result<Value, String> {
    check_namespaces(view)?;
    let schema = &view.schema;
    let mut generator = Generator {
        namespaces: schema.prefixes.clone(),
        emit: schema.prefixes.iter().map(|(p, _)| p.clone()).collect(),
        vocab: String::new(),
    };
    for p in &schema.emit_prefixes {
        generator.add_prefix(p);
    }
    let mut body = Map::new();
    let default_prefix = schema.default_prefix.clone().unwrap_or_else(|| {
        let id = schema.id.clone();
        if id.ends_with('/') || id.ends_with('#') {
            id
        } else {
            format!("{id}/")
        }
    });
    let default_ns = match generator.prefix_suffix(&default_prefix)?.0 {
        Some(p) if !p.is_empty() => Some(p),
        _ => None,
    };
    let vocab = match default_ns {
        Some(p) => {
            let ns = generator
                .ns(&p)
                .ok_or_else(|| format!("KeyError: '{p}'"))?
                .to_owned();
            generator.emit.insert(p);
            ns
        }
        None => {
            generator
                .namespaces
                .push((schema.name.clone(), default_prefix.clone()));
            generator.emit.insert(schema.name.clone());
            default_prefix.clone()
        }
    };
    body.insert("@vocab".to_owned(), Value::String(vocab.clone()));
    generator.vocab = vocab;

    // The slots SchemaLoader hands the generator, visited sorted by lower-cased name.
    let default_range = schema
        .default_range
        .clone()
        .unwrap_or_else(|| "string".to_owned());
    let mut gen_slots: Vec<GenSlot<'_>> = Vec::new();
    for (name, slot) in schema
        .slots
        .iter()
        .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
    {
        gen_slots.push(GenSlot {
            name: name.clone(),
            alias: slot
                .get("alias")
                .filter(|v| truthy(Some(v)))
                .map_or_else(|| name.clone(), py_str),
            element: slot,
            identifier: truthy(slot.get("identifier")),
            usage: false,
        });
    }
    for (cname, cls) in view.classes() {
        for (aname, attr) in attributes(cls) {
            gen_slots.push(GenSlot {
                name: mangled_attribute_name(cname, aname),
                alias: attr
                    .get("alias")
                    .filter(|v| truthy(Some(v)))
                    .map_or_else(|| aname.clone(), py_str),
                element: attr,
                identifier: truthy(attr.get("identifier")),
                usage: false,
            });
        }
    }
    for (cname, cls) in view.classes() {
        let Some(usages) = cls.get("slot_usage").and_then(Value::as_object) else {
            continue;
        };
        for (sname, usage) in usages {
            let Some(usage) = usage.as_object() else {
                continue;
            };
            let parent = schema.slots.get(sname).and_then(Value::as_object);
            let alias = parent
                .and_then(|p| p.get("alias"))
                .filter(|v| truthy(Some(v)))
                .map_or_else(|| sname.clone(), py_str);
            let identifier = match usage.get("identifier").filter(|v| !v.is_null()) {
                Some(v) => truthy(Some(v)),
                None => parent.is_some_and(|p| truthy(p.get("identifier"))),
            };
            gen_slots.push(GenSlot {
                name: slot_usage_name(sname, cname),
                alias,
                element: usage,
                identifier,
                usage: true,
            });
        }
    }
    gen_slots.sort_by_key(|s| s.name.to_lowercase());
    for slot in &gen_slots {
        let key = underscore(&slot.alias);
        if slot.identifier {
            body.insert(key, Value::String("@id".to_owned()));
            continue;
        }
        if slot.usage {
            continue;
        }
        let mut def = Map::new();
        let range = slot
            .element
            .get("range")
            .filter(|v| truthy(Some(v)))
            .map_or_else(|| default_range.clone(), py_str);
        let any_of: Vec<String> = slot
            .element
            .get("any_of")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| i.get("range").map_or_else(|| "None".to_owned(), py_str))
                    .collect()
            })
            .unwrap_or_default();
        let has_class = view.is_class(&range) || any_of.iter().any(|r| view.is_class(r));
        let has_literal = any_of.iter().any(|r| view.is_type(r));
        if has_class && has_literal {
            let mut coercions = std::collections::BTreeSet::new();
            for r in any_of.iter().filter(|r| view.is_type(r)) {
                coercions.insert(generator.type_coercion(view, r)?);
            }
            if coercions.len() == 1 {
                if let Some(Some(c)) = coercions.into_iter().next() {
                    def.insert("@type".to_owned(), Value::String(c));
                }
            }
        } else if has_class {
            def.insert("@type".to_owned(), Value::String("@id".to_owned()));
        } else if view.is_enum(&range) {
            def.insert(
                "@context".to_owned(),
                json!({"text": "skos:notation", "description": "skos:prefLabel", "meaning": "@id"}),
            );
            let skos = curie_for(&generator.namespaces, SKOS)
                .and_then(|c| c.split(':').next().map(str::to_owned));
            match skos {
                Some(p) => {
                    generator.emit.insert(p);
                }
                None => {
                    generator
                        .namespaces
                        .push(("skos".to_owned(), SKOS.to_owned()));
                    generator.emit.insert("skos".to_owned());
                }
            }
        } else if let Some(c) = generator.type_coercion(view, &range)? {
            def.insert("@type".to_owned(), Value::String(c));
        }
        let uri = slot
            .element
            .get("slot_uri")
            .filter(|v| truthy(Some(v)))
            .map_or_else(
                || format!("{}:{}", default_ns_or_vocab(view), underscore(&slot.name)),
                py_str,
            );
        let id = generator.element_id(&uri)?;
        def.insert("@id".to_owned(), Value::String(id));
        generator.add_mappings(&slot.name, slot.element, &uri)?;
        body.insert(key, Value::Object(def));
    }
    let mut class_list: Vec<(&String, &Obj)> = view.classes().collect();
    class_list.sort_by_key(|(n, _)| n.to_lowercase());
    let mut class_maps = Map::new();
    for (name, cls) in class_list {
        let uri = cls
            .get("class_uri")
            .filter(|v| truthy(Some(v)))
            .map_or_else(
                || format!("{}:{}", default_ns_or_vocab(view), camelcase(name)),
                py_str,
            );
        generator.add_mappings(name, cls, &uri)?;
        let id = generator.element_id(&uri)?;
        class_maps.insert(camelcase(name), json!({ "@id": id }));
    }

    let mut context = Map::new();
    context.insert("xsd".to_owned(), Value::String(XSD.to_owned()));
    for prefix in &generator.emit {
        let url = generator
            .ns(prefix)
            .ok_or_else(|| format!("KeyError: '{prefix}'"))?
            .to_owned();
        let value = if url.ends_with([':', '/', '?', '#', '[', ']', '@']) {
            Value::String(url)
        } else {
            json!({"@id": url, "@prefix": true})
        };
        context.insert(prefix.clone(), value);
    }
    context.extend(body);
    context.extend(class_maps);

    // gen_context.py: classes and slots bound to their IRIs, kinds to JSON-LD keywords.
    for (name, cls) in view.classes() {
        context.insert(name.clone(), json!({ "@id": term(view, true, name, cls) }));
    }
    for (name, slot) in view.all_slots() {
        let key = slot
            .get("alias")
            .filter(|v| truthy(Some(v)))
            .map_or_else(|| name.clone(), py_str);
        if let Some(keyword) = annotation_value(slot, "jsonld_keyword").filter(|k| !k.is_empty()) {
            context.insert(key, Value::String(keyword));
            continue;
        }
        let mut entry = match context.get(&key) {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        };
        entry.insert(
            "@id".to_owned(),
            Value::String(term(view, false, &name, slot)),
        );
        match ngsi_ld_kind(&name, slot)?.as_str() {
            "Relationship" => {
                entry.insert("@type".to_owned(), json!("@id"));
            }
            "LanguageProperty" => {
                entry.remove("@type");
                entry.insert("@container".to_owned(), json!("@language"));
            }
            "VocabProperty" => {
                entry.insert("@type".to_owned(), json!("@vocab"));
            }
            "ListProperty" => {
                entry.insert("@container".to_owned(), json!("@list"));
            }
            "JsonProperty" => {
                entry.insert("@type".to_owned(), json!("@json"));
            }
            _ if entry.get("@type") == Some(&json!("@id")) => {
                entry.remove("@type");
            }
            _ => {}
        }
        context.insert(key, Value::Object(entry));
    }
    Ok(json!({ "@context": context, "x-generator-version": generator_version }))
}

/// The prefix SchemaLoader names an element without an IRI under.
fn default_ns_or_vocab(view: &View) -> String {
    view.schema
        .default_prefix
        .clone()
        .unwrap_or_else(|| view.schema.name.clone())
}

/// `_term`: the IRI of a class or slot, as a CURIE where a prefix covers it.
fn term(view: &View, class: bool, name: &str, element: &Obj) -> String {
    let expanded = view.get_uri(class, name, element, true);
    view.curie_for(&expanded).unwrap_or(expanded)
}

/// `check_namespaces`: no term minted under someone else's namespace (DM-04, DM-16).
fn check_namespaces(view: &View) -> Result<(), String> {
    let mut squatted = Vec::new();
    let classes = view.classes().map(|(n, c)| (true, n.clone(), c));
    let slots = view.all_slots().into_iter().map(|(n, s)| (false, n, s));
    for (class, name, element) in classes.chain(slots) {
        let iri = view.get_uri(class, &name, element, true);
        if let Some(ns) = RESERVED_NAMESPACES.iter().find(|ns| iri.starts_with(*ns)) {
            if annotation_value(element, "upstream_source")
                .filter(|v| !v.is_empty())
                .is_none()
            {
                squatted.push(format!("'{name}' → {iri} (reserved: {ns})"));
            }
        }
    }
    if squatted.is_empty() {
        return Ok(());
    }
    squatted.sort();
    Err(format!(
        "these terms are minted under a namespace the organisation does not own; use the \
         organisation's own prefix, or import the term and keep its upstream_source: {}",
        squatted.join("; ")
    ))
}
