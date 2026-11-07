// SPDX-License-Identifier: EUPL-1.2
//! `/api` (mapping row C2, Part 1 `/req/oas30/*`): the `OpenAPI` 3.0 document of exactly what
//! this view serves, one path per collection the view shows, so no hidden type is named.

use crate::crs;
use crate::model::Collection;
use antares_ops::OperationId;
use serde_json::{json, Map, Value};

const GEOJSON: &str = "application/geo+json";
use std::collections::BTreeMap;

/// The answers every operation may give besides its 200 (`/req/oas30/completeness`).
fn refusals() -> Map<String, Value> {
    let problem = |text: &str| json!({"description": text, "content": {"application/problem+json": {"schema": {"$ref": "#/components/schemas/problem"}}}});
    Map::from_iter(
        [
            ("400", "the query is not one this resource defines"),
            ("404", "no such resource in this view"),
            ("406", "the client accepts no JSON"),
            (
                "501",
                "a construct this endpoint does not serve (a LIKE pattern that is no literal, a conditional write)",
            ),
            (
                "500",
                "the broker failed; the detail is logged under a request id",
            ),
        ]
        .map(|(code, text)| (code.to_owned(), problem(text))),
    )
}

/// A write operation (W1 to W5): its success status and, but for `DELETE`, its body's media.
fn write(id: &str, summary: &str, ok: &str, body: Option<&str>) -> Value {
    let mut responses = refusals();
    responses.insert(ok.into(), json!({"description": summary}));
    for (code, text) in [
        ("403", "the view or its Policy refused the write"),
        ("413", "the body is above 1 MiB"),
        ("415", "the body's media type is not this operation's"),
        ("501", "a conditional write (If-Match, If-Unmodified-Since)"),
    ] {
        responses.insert(code.into(), json!({"description": text}));
    }
    let mut op = Map::from_iter([
        ("operationId".to_owned(), Value::from(id)),
        ("summary".to_owned(), Value::from(summary)),
        ("responses".to_owned(), Value::Object(responses)),
    ]);
    if let Some(media) = body {
        op.insert(
            "requestBody".into(),
            json!({"required": true, "content": {media: {"schema": {"type": "object"}}}}),
        );
    }
    Value::Object(op)
}

fn get(id: &str, summary: &str, media: &str, parameters: &[Value]) -> Value {
    Value::Object(read(id, summary, media, parameters))
}

/// A path item with its `get`.
fn read(id: &str, summary: &str, media: &str, parameters: &[Value]) -> Map<String, Value> {
    let mut responses = refusals();
    responses.insert(
        "200".into(),
        json!({"description": summary, "content": {media: {"schema": {"type": "object"}}}}),
    );
    Map::from_iter([(
        "get".to_owned(),
        json!({"operationId": id, "summary": summary, "parameters": parameters, "responses": responses}),
    )])
}

/// The problem schema and the query parameters the operations name.
fn components() -> Value {
    let mut parameters = json!({
        "limit": {"name": "limit", "in": "query", "required": false, "style": "form", "explode": false, "schema": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 10}},
        "bbox": {"name": "bbox", "in": "query", "required": false, "style": "form", "explode": false, "schema": {"type": "array", "minItems": 4, "maxItems": 6, "oneOf": [{"minItems": 4, "maxItems": 4}, {"minItems": 6, "maxItems": 6}], "items": {"type": "number"}}},
        "datetime": {"name": "datetime", "in": "query", "required": false, "style": "form", "explode": false, "schema": {"type": "string"}},
        "next": {"name": "next", "in": "query", "required": false, "description": "the token of a next link", "schema": {"type": "string"}},
        "sortby": {"name": "sortby", "in": "query", "required": false, "style": "form", "explode": false, "description": "[+|-]name of the collection's sortables", "schema": {"type": "array", "items": {"type": "string"}}},
        "properties": {"name": "properties", "in": "query", "required": false, "style": "form", "explode": false, "description": "the properties of the collection's schema to return", "schema": {"type": "array", "items": {"type": "string"}}},
        "crs": {"name": "crs", "in": "query", "required": false, "schema": {"type": "string", "format": "uri", "enum": [crs::CRS84, crs::CRS84H]}},
        "bbox-crs": {"name": "bbox-crs", "in": "query", "required": false, "schema": {"type": "string", "format": "uri", "enum": [crs::CRS84, crs::CRS84H]}},
        "filter": {"name": "filter", "in": "query", "required": false, "description": "a CQL2 filter over the collection's queryables", "schema": {"type": "string"}},
        "filter-lang": {"name": "filter-lang", "in": "query", "required": false, "schema": {"type": "string", "enum": ["cql2-text", "cql2-json"], "default": "cql2-text"}},
        "filter-crs": {"name": "filter-crs", "in": "query", "required": false, "schema": {"type": "string", "format": "uri", "enum": [crs::CRS84, crs::CRS84H]}},
        "profile": {"name": "profile", "in": "query", "required": false, "style": "form", "explode": false, "description": "how references are written", "schema": {"type": "string", "enum": ["rel-as-key", "rel-as-uri", "rel-as-link"]}},
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    parameters.extend(crate::items::NGSILD.iter().map(|name| {
        (
            format!("ngsild.{name}"),
            json!({"name": format!("ngsild.{name}"), "in": "query", "required": false, "description": format!("the NGSI-LD Query Entities `{name}` (TS 104 176 7.2.3.1), ANDed into the query; it only narrows"), "schema": {"type": "string"}}),
        )
    }));
    json!({
        "schemas": {"problem": {"type": "object", "properties": {"type": {"type": "string"}, "title": {"type": "string"}, "status": {"type": "integer"}, "detail": {"type": "string"}}}},
        "parameters": parameters,
    })
}

/// The paths of one collection: it, its schemas, its items and one Feature, with the writes
/// the view `offers`.
fn collection_paths(
    paths: &mut Map<String, Value>,
    c: &Collection,
    offers: &dyn Fn(OperationId) -> bool,
    (items_params, feature_id): (&[Value], &[Value]),
) {
    let at = format!("/collections/{}", c.id);
    paths.insert(
        at.clone(),
        get(
            &format!("collection-{}", c.id),
            "the collection",
            "application/json",
            &[],
        ),
    );
    for (which, summary) in [
        ("schema", "the schema of its Features"),
        ("sortables", "the properties sortby may name"),
        ("queryables", "the properties a filter may name"),
    ] {
        paths.insert(
            format!("{at}/{which}"),
            get(
                &format!("{which}-{}", c.id),
                summary,
                "application/schema+json",
                &[],
            ),
        );
    }
    // `queryables-query-parameters`: each scalar queryable as an equality.
    let own: Vec<Value> = items_params
        .iter()
        .cloned()
        .chain(
            ["id"]
                .into_iter()
                .chain(c.properties.iter().filter(|p| p.sortable).map(|p| p.name.as_str()))
                .map(|name| json!({"name": name, "in": "query", "required": false, "description": "the Features whose property equals the value", "schema": {"type": "string"}})),
        )
        .collect();
    let mut items = read(
        &format!("items-{}", c.id),
        "a page of Features",
        "application/geo+json",
        &own,
    );
    let mut item = read(
        &format!("item-{}", c.id),
        "one Feature",
        "application/geo+json",
        feature_id,
    );
    if offers(OperationId::CreateEntity) {
        items.insert(
            "post".into(),
            write(
                &format!("post-{}", c.id),
                "create a Feature",
                "201",
                Some(GEOJSON),
            ),
        );
    }
    for (method, op, body, summary) in [
        (
            "put",
            OperationId::ReplaceEntity,
            Some(GEOJSON),
            "replace the Feature",
        ),
        (
            "patch",
            OperationId::MergeEntity,
            Some("application/merge-patch+json"),
            "merge a patch into the Feature",
        ),
        (
            "delete",
            OperationId::DeleteEntity,
            None,
            "delete the Feature",
        ),
    ] {
        if offers(op) {
            item.insert(
                method.into(),
                write(&format!("{method}-{}", c.id), summary, "204", body),
            );
        }
    }
    paths.insert(format!("{at}/items"), Value::Object(items));
    paths.insert(format!("{at}/items/{{featureId}}"), Value::Object(item));
    if c.geometry.is_some() {
        tile_paths(paths, &at, &c.id);
    }
}

/// A collection's tilesets, its tileset and its tiles (Tiles Part 1 `/req/oas30`), each `operationId` ending in its Table 11 name.
fn tile_paths(paths: &mut Map<String, Value>, at: &str, id: &str) {
    let tms = crate::tiles::TMS;
    paths.insert(
        format!("{at}/tiles"),
        get(
            &format!("{id}.collection.vector.getTileSetsList"),
            "the vector tilesets",
            "application/json",
            &[],
        ),
    );
    paths.insert(
        format!("{at}/tiles/{tms}"),
        get(
            &format!("{id}.collection.vector.getTileSet"),
            "the tileset",
            "application/json",
            &[],
        ),
    );
    let index = |name: &str| json!({"name": name, "in": "path", "required": true, "schema": {"type": "integer", "minimum": 0}});
    let mut responses = refusals();
    for (code, text) in [
        ("204", "no Feature reaches the tile"),
        ("304", "the tile the client holds by its ETag"),
        ("413", "the tile is above 4 MiB: ask a deeper tile matrix"),
    ] {
        responses.insert(code.into(), json!({"description": text}));
    }
    responses.insert(
        "200".into(),
        json!({"description": "one Mapbox Vector Tile", "content": {crate::tiles::MVT: {"schema": {"type": "string", "format": "binary"}}}}),
    );
    let tile = json!({"get": {
        "operationId": format!("{id}.collection.vector.getTile"),
        "summary": "one Mapbox Vector Tile",
        "parameters": [index("tileMatrix"), index("tileRow"), index("tileCol")],
        "responses": responses,
    }});
    paths.insert(
        format!("{at}/tiles/{tms}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}"),
        tile,
    );
}

/// The document for the endpoint at `base` serving `collections`, with the writes the view
/// `offers` (W4).
pub(crate) fn document(
    base: &str,
    collections: &BTreeMap<String, Collection>,
    offers: &dyn Fn(OperationId) -> bool,
) -> Value {
    let mut paths = Map::new();
    paths.insert(
        "/".into(),
        get("landing", "the landing page", "application/json", &[]),
    );
    paths.insert(
        "/conformance".into(),
        get(
            "conformance",
            "the conformance classes",
            "application/json",
            &[],
        ),
    );
    paths.insert(
        "/api".into(),
        get(
            "api",
            "this document",
            "application/vnd.oai.openapi+json;version=3.0",
            &[],
        ),
    );
    paths.insert(
        "/collections".into(),
        get("collections", "the collections", "application/json", &[]),
    );
    paths.insert(
        "/functions".into(),
        get(
            "functions",
            "the functions a filter may call: none",
            "application/json",
            &[],
        ),
    );
    let items_params = [
        json!({"$ref": "#/components/parameters/limit"}),
        json!({"$ref": "#/components/parameters/bbox"}),
        json!({"$ref": "#/components/parameters/datetime"}),
        json!({"$ref": "#/components/parameters/next"}),
        json!({"$ref": "#/components/parameters/sortby"}),
        json!({"$ref": "#/components/parameters/properties"}),
        json!({"$ref": "#/components/parameters/profile"}),
        json!({"$ref": "#/components/parameters/crs"}),
        json!({"$ref": "#/components/parameters/bbox-crs"}),
        json!({"$ref": "#/components/parameters/filter"}),
        json!({"$ref": "#/components/parameters/filter-lang"}),
        json!({"$ref": "#/components/parameters/filter-crs"}),
    ]
    .into_iter()
    .chain(
        crate::items::NGSILD
            .iter()
            .map(|name| json!({"$ref": format!("#/components/parameters/ngsild.{name}")})),
    )
    .collect::<Vec<_>>();
    let feature_id = [
        json!({"name": "featureId", "in": "path", "required": true, "schema": {"type": "string"}}),
        json!({"$ref": "#/components/parameters/properties"}),
        json!({"$ref": "#/components/parameters/profile"}),
        json!({"$ref": "#/components/parameters/crs"}),
    ];
    paths.insert(
        "/tileMatrixSets".into(),
        get(
            "getTileMatrixSetsList",
            "the tile matrix sets",
            "application/json",
            &[],
        ),
    );
    paths.insert(
        format!("/tileMatrixSets/{}", crate::tiles::TMS),
        get(
            "getTileMatrixSet",
            "the WebMercatorQuad definition",
            "application/json",
            &[],
        ),
    );
    for c in collections.values() {
        // T-5100: a history offers no write.
        let read_only = |op| !c.history && c.output.is_none() && offers(op);
        collection_paths(&mut paths, c, &read_only, (&items_params, &feature_id));
    }
    json!({
        "openapi": "3.0.3",
        "info": {"title": "OGC API - Features", "version": "1.0.0"},
        "servers": [{"url": base}],
        "paths": paths,
        "components": components(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_documents_its_refusals_and_only_shown_collections_appear() {
        let car = Collection {
            history: false,
            output: None,
            id: "Car".into(),
            description: None,
            type_iri: "https://e.org/Car".into(),
            geometry: None,
            time: None,
            properties: Vec::new(),
        };
        let doc = document(
            "http://b",
            &BTreeMap::from([("Car".to_owned(), car)]),
            &|op| op != OperationId::MergeEntity,
        );
        let item = doc.pointer("/paths/~1collections~1Car~1items~1{featureId}");
        assert_eq!(
            ["put", "patch", "delete"].map(|m| item.and_then(|i| i.get(m)).is_some()),
            [true, false, true],
            "only the writes the view offers"
        );
        assert!(doc
            .pointer("/paths/~1collections~1Car~1items/post/requestBody")
            .is_some());
        let paths = doc
            .get("paths")
            .unwrap_or(&Value::Null)
            .as_object()
            .map(|p| p.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        assert_eq!(
            paths,
            [
                "/",
                "/api",
                "/collections",
                "/collections/Car",
                "/collections/Car/items",
                "/collections/Car/items/{featureId}",
                "/collections/Car/queryables",
                "/collections/Car/schema",
                "/collections/Car/sortables",
                "/conformance",
                "/functions",
                "/tileMatrixSets",
                "/tileMatrixSets/WebMercatorQuad"
            ]
        );
        for (_, op) in doc
            .get("paths")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            for code in ["200", "400", "404", "406", "500"] {
                assert!(
                    op.pointer(&format!("/get/responses/{code}")).is_some(),
                    "{op}"
                );
            }
        }
        assert_eq!(doc.pointer("/servers/0/url"), Some(&json!("http://b")));
    }
}
