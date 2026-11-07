// SPDX-License-Identifier: EUPL-1.2
//! `/collections/{c}/schema`, `/collections/{c}/sortables` and `/collections/{c}/queryables`
//! (mapping rows S1 to S3, S5, F1,
//! OGC 23-058r1 Part 5): JSON Schema 2020-12 of the Features a collection serves, from the
//! Tenant's `LinkML` schema intersected with what the view shows, so no hidden Attribute is
//! named.

use crate::model::{Collection, Prop};
use serde_json::{Map, Value};

/// The JSON Schema dialect Part 5 requires (`/req/schemas/json-schema`).
const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The schema of `c` served at `id`: its `id`, its geometry and every property the view shows
/// (`sortables`: only those `sortby` may name), in Feature order (`x-ogc-propertySeq`).
pub(crate) fn document(c: &Collection, id: &str, sortables: bool) -> Value {
    Value::Object(members(c, id, sortables))
}

/// The members of [`document`].
fn members(c: &Collection, id: &str, sortables: bool) -> Map<String, Value> {
    let identity = fields([
        ("title", Some("id".into())),
        ("type", Some("string".into())),
        ("format", Some("uri".into())),
        ("x-ogc-role", Some("id".into())),
    ]);
    let geometry = c.geometry.as_ref().map(|iri| {
        fields([
            ("title", Some("geometry".into())),
            ("format", Some("geometry-any".into())),
            ("x-ogc-role", Some("primary-geometry".into())),
            ("x-ogc-definition", Some(iri.as_str().into())),
        ])
    });
    let head = [("id".to_owned(), identity)]
        .into_iter()
        .chain(geometry.map(|g| ("geometry".to_owned(), g)))
        .filter(|_| !sortables);
    let props = c
        .properties
        .iter()
        .filter(|p| !sortables || p.sortable)
        .map(|p| (p.name.clone(), property(p)));
    let properties: Map<String, Value> = (1_u64..)
        .zip(head.chain(props))
        .map(|(seq, (name, mut schema))| {
            schema.insert("x-ogc-propertySeq".into(), seq.into());
            (name, Value::Object(schema))
        })
        .collect();
    fields([
        ("$schema", Some(DIALECT.into())),
        ("$id", Some(id.into())),
        ("type", Some("object".into())),
        ("title", Some(c.id.as_str().into())),
        ("description", c.description.as_deref().map(Value::from)),
        ("properties", Some(Value::Object(properties))),
    ])
}

/// The queryables of `c` served at `id` (Part 3 `queryables`, F1): the properties of its schema,
/// and no other (`additionalProperties: false`), so a filter names only what the view shows; the
/// attributes the type's query guide filters on come first (T-4537).
pub(crate) fn queryables(c: &Collection, id: &str) -> Value {
    let mut c = c.clone();
    if let Some(guide) = antares_ops::guide::of(&c.id) {
        let rank = |p: &Prop| guide.filters.iter().position(|f| *f == p.name);
        c.properties
            .sort_by_key(|p| rank(p).unwrap_or(guide.filters.len()));
    }
    let mut doc = members(&c, id, false);
    doc.insert("additionalProperties".into(), Value::Bool(false));
    Value::Object(doc)
}

/// The members that are present, in order.
fn fields<const N: usize>(members: [(&str, Option<Value>); N]) -> Map<String, Value> {
    members
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)))
        .collect()
}

/// One property: its value's schema with its title, role, unit, definition and description.
fn property(p: &Prop) -> Map<String, Value> {
    let role = if p.reference {
        Some("reference")
    } else {
        p.role.as_deref()
    };
    let mut out = p.schema.as_object().cloned().unwrap_or_default();
    out.extend(fields([
        ("title", Some(p.name.as_str().into())),
        ("description", p.description.as_deref().map(Value::from)),
        ("x-ogc-role", role.map(Value::from)),
        ("x-ogc-unit", p.unit.as_deref().map(Value::from)),
        ("x-ogc-collectionId", p.target.as_deref().map(Value::from)),
        ("x-ogc-definition", Some(p.iri.as_str().into())),
    ]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{build, LOCATION};
    use linkml_model::{Limits, Model};
    use std::collections::{BTreeMap, BTreeSet};

    const LIMITS: Limits = Limits {
        max_bytes: 1 << 16,
        max_depth: 16,
        max_elements: 1024,
    };

    /// An SDM Event with no `ogc_role` annotation: the roles are the query guide's.
    const EVENTS: &str = r#"{
      "id": "https://example.org/sdm", "name": "sdm",
      "prefixes": {"sdm": {"prefix_prefix": "sdm", "prefix_reference": "https://example.org/sdm/"},
                   "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
      "default_prefix": "sdm",
      "classes": {"Event": {"slots": ["description", "name", "startDate", "endDate", "category", "location", "organizer"]}},
      "slots": {
        "description": {"range": "string"},
        "name": {"range": "string"},
        "startDate": {"range": "datetime"},
        "endDate": {"range": "datetime"},
        "category": {"range": "string"},
        "organizer": {"range": "string"},
        "location": {"slot_uri": "ngsi:location", "range": "string", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}
      }
    }"#;

    fn queryables_of(schema: &str, shown: &[&str]) -> Result<Value, String> {
        let model = Model::read(schema.as_bytes(), &LIMITS).map_err(|e| format!("{e:?}"))?;
        let attrs: BTreeSet<String> = shown
            .iter()
            .map(|a| match *a {
                "location" => LOCATION.to_owned(),
                a => format!("https://example.org/sdm/{a}"),
            })
            .collect();
        let view = BTreeMap::from([("https://example.org/sdm/Event".to_owned(), attrs)]);
        let collections = build(&model, &view);
        let event = collections.get("Event").ok_or("Event is shown")?;
        Ok(queryables(event, "q"))
    }

    fn event_queryables(shown: &[&str]) -> Result<Value, String> {
        queryables_of(EVENTS, shown)
    }

    fn role<'v>(doc: &'v Value, name: &str) -> Option<&'v str> {
        doc.pointer(&format!("/properties/{name}/x-ogc-role"))
            .and_then(Value::as_str)
    }

    #[test]
    fn an_event_marks_its_interval_and_place_and_puts_its_filters_first() -> Result<(), String> {
        let doc = event_queryables(&[
            "description",
            "name",
            "startDate",
            "endDate",
            "category",
            "location",
            "organizer",
        ])?;
        assert_eq!(role(&doc, "startDate"), Some("primary-interval-start"));
        assert_eq!(role(&doc, "endDate"), Some("primary-interval-end"));
        assert_eq!(role(&doc, "geometry"), Some("primary-geometry"));
        let order: Vec<&str> = doc
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|p| p.keys().map(String::as_str))
            .collect::<Vec<_>>();
        let seq = |name: &str| {
            doc.pointer(&format!("/properties/{name}/x-ogc-propertySeq"))
                .and_then(Value::as_u64)
        };
        // id, geometry, then category, name, organizer (the guide's order), then the rest
        assert_eq!(seq("category"), Some(3), "{order:?}");
        assert_eq!(seq("name"), Some(4), "{order:?}");
        assert_eq!(seq("organizer"), Some(5), "{order:?}");
        assert!(seq("description") > seq("organizer"), "{order:?}");
        Ok(())
    }

    #[test]
    fn a_hidden_end_leaves_the_start_an_instant_and_names_nothing_hidden() -> Result<(), String> {
        let doc = event_queryables(&["name", "startDate"])?;
        assert_eq!(role(&doc, "startDate"), Some("primary-instant"));
        let text = doc.to_string();
        for hidden in ["endDate", "geometry", "category", "organizer"] {
            assert!(!text.contains(hidden), "{hidden}: {text}");
        }
        let bare = event_queryables(&["name"])?.to_string();
        assert!(!bare.contains("primary-"), "no axis shown, no role: {bare}");
        Ok(())
    }

    #[test]
    fn the_schemas_own_roles_win_over_the_guide() -> Result<(), String> {
        // startDate annotated an instant and `where` the geometry: the guide adds neither
        let annotated = EVENTS
            .replace(
                r#""startDate": {"range": "datetime"}"#,
                r#""startDate": {"range": "datetime", "annotations": {"ogc_role": {"tag": "ogc_role", "value": "primary-instant"}}}"#,
            )
            .replace(
                r#""organizer": {"range": "string"}"#,
                r#""organizer": {"range": "string", "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}, "ogc_role": {"tag": "ogc_role", "value": "primary-geometry"}}}"#,
            );
        let doc = queryables_of(
            &annotated,
            &["startDate", "endDate", "location", "organizer"],
        )?;
        assert_eq!(role(&doc, "startDate"), Some("primary-instant"));
        assert_eq!(role(&doc, "endDate"), None);
        assert_eq!(role(&doc, "location"), None);
        Ok(())
    }
}
