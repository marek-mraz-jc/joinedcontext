//! A feed's records and the entities they become (AG-79): where the records sit in a probe's
//! sample, the field that identifies one, where its position is, and the Bloblang that writes one
//! entity per record under the slot names the inferred model gives the fields.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// Levels below the sample's root searched for the array of records.
const MAX_DEPTH: usize = 3;
/// Records handed to the model inference; model-tools reads no more of a column.
pub const MAX_RECORDS: usize = 1000;
/// The names model-tools reads as a latitude and a longitude (tools/model-tools, DM-54).
const LATITUDES: [&str; 3] = ["lat", "latitude", "y"];
const LONGITUDES: [&str; 5] = ["lon", "lng", "long", "longitude", "x"];
/// What every entity has from `Entity`: a record's own field of that name is no attribute.
const CORE: [&str; 5] = ["id", "type", "location", "observedAt", "@context"];
/// Words of a slot name that make it an identifier, a time or a coordinate, never a colour.
const NOT_A_MEASURE: [&str; 12] = [
    "id",
    "time",
    "date",
    "reported",
    "updated",
    "modified",
    "timestamp",
    "ttl",
    "version",
    "lat",
    "lon",
    "at",
];

/// Where a record's position is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position {
    /// A latitude and a longitude field of the record.
    LatLon { lat: String, lon: String },
    /// A GeoJSON feature's `geometry`.
    Geometry,
}

/// The records of one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Records {
    /// Keys from the sample's root to the array; empty when the sample is the array.
    pub path: Vec<String>,
    /// GeoJSON features: the fields are each feature's `properties`.
    pub features: bool,
    /// The fields of each record: a feature's properties, or the record itself.
    pub rows: Vec<Map<String, Value>>,
    /// A feature's own `id`, one per row, when the records are features.
    feature_ids: Vec<Value>,
    /// A feature's geometry, one per row, when the records are features.
    geometries: Vec<Value>,
}

/// The records of a sample: the sample when it is an array of objects, a FeatureCollection's
/// features, or the largest array of objects at most three levels below the root.
pub fn records(sample: &Value) -> Option<Records> {
    if sample["type"] == "FeatureCollection" {
        let features: Vec<&Map<String, Value>> = sample["features"]
            .as_array()?
            .iter()
            .filter_map(Value::as_object)
            .collect();
        if features.is_empty() {
            return None;
        }
        return Some(Records {
            path: vec!["features".to_owned()],
            features: true,
            rows: features
                .iter()
                .map(|f| {
                    f.get("properties")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect(),
            feature_ids: features
                .iter()
                .map(|f| f.get("id").cloned().unwrap_or(Value::Null))
                .collect(),
            geometries: features
                .iter()
                .map(|f| f.get("geometry").cloned().unwrap_or(Value::Null))
                .collect(),
        });
    }
    // (path, the array, how many objects it holds)
    let mut best: Option<(Vec<String>, &Vec<Value>, usize)> = None;
    let mut level: Vec<(Vec<String>, &Value)> = vec![(Vec::new(), sample)];
    for depth in 0..=MAX_DEPTH {
        let mut next = Vec::new();
        for (path, value) in level {
            match value {
                Value::Array(items) => {
                    let objects = items.iter().filter(|item| item.is_object()).count();
                    if objects > 0 && best.as_ref().is_none_or(|(_, _, held)| objects > *held) {
                        best = Some((path, items, objects));
                    }
                }
                Value::Object(fields) if depth < MAX_DEPTH => {
                    for (key, child) in fields {
                        let mut deeper = path.clone();
                        deeper.push(key.clone());
                        next.push((deeper, child));
                    }
                }
                _ => {}
            }
        }
        level = next;
    }
    let (path, items, _) = best?;
    Some(Records {
        path,
        features: false,
        rows: items.iter().filter_map(Value::as_object).cloned().collect(),
        feature_ids: Vec::new(),
        geometries: Vec::new(),
    })
}

/// A path segment as Bloblang reads it: bare when it is a plain name, quoted otherwise.
fn segment(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_owned()
    } else {
        Value::String(name.to_owned()).to_string()
    }
}

/// A number, or a string that reads as one.
fn numeric(value: &Value) -> bool {
    value.is_number()
        || value
            .as_str()
            .is_some_and(|text| text.trim().parse::<f64>().is_ok())
}

impl Records {
    /// Every field of the records, each once.
    pub fn fields(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.rows
            .iter()
            .flat_map(|row| row.keys())
            .filter(|key| seen.insert(key.as_str()))
            .cloned()
            .collect()
    }

    /// The Bloblang path of one field of `record`.
    fn field(&self, name: &str) -> String {
        if self.features {
            format!("record.properties.{}", segment(name))
        } else {
            format!("record.{}", segment(name))
        }
    }

    /// The Bloblang expression of the field that identifies a record: `id`, `uuid` or a field
    /// ending in `_id` or `Id` whose values are strings or integers, present in every record and
    /// unique in the sample; a feature's own `id` when no property does. `None` when none does.
    pub fn identifier(&self) -> Option<String> {
        let unique = |values: Vec<Option<&Value>>| {
            let mut seen = BTreeSet::new();
            !values.is_empty()
                && values.into_iter().all(|value| match value {
                    Some(Value::String(text)) if !text.is_empty() => seen.insert(text.clone()),
                    Some(Value::Number(number)) if number.is_i64() || number.is_u64() => {
                        seen.insert(number.to_string())
                    }
                    _ => false,
                })
        };
        let mut candidates: Vec<String> = self
            .fields()
            .into_iter()
            .filter(|name| {
                name == "id" || name == "uuid" || name.ends_with("_id") || name.ends_with("Id")
            })
            .collect();
        candidates.sort_by_key(|name| name != "id");
        candidates
            .into_iter()
            .find(|name| unique(self.rows.iter().map(|row| row.get(name)).collect()))
            .map(|name| self.field(&name))
            .or_else(|| {
                (self.features && unique(self.feature_ids.iter().map(Some).collect()))
                    .then(|| "record.id".to_owned())
            })
    }

    /// Where a record's position is: a feature's geometry, or a latitude and a longitude field
    /// whose first values read as numbers.
    pub fn position(&self) -> Option<Position> {
        if self.features {
            return self
                .geometries
                .iter()
                .all(Value::is_object)
                .then_some(Position::Geometry);
        }
        let fields = self.fields();
        let named = |names: &[&str]| {
            fields
                .iter()
                .find(|field| names.contains(&field.trim().to_ascii_lowercase().as_str()))
                .filter(|field| {
                    self.rows
                        .iter()
                        .find_map(|row| row.get(*field))
                        .is_some_and(numeric)
                })
                .cloned()
        };
        Some(Position::LatLon {
            lat: named(&LATITUDES)?,
            lon: named(&LONGITUDES)?,
        })
    }

    /// The records model-tools infers the model from: at most a thousand, a feature's geometry
    /// as its `location`.
    pub fn inference_sample(&self) -> Value {
        let rows = self.rows.iter().take(MAX_RECORDS);
        if !self.features {
            return Value::Array(rows.cloned().map(Value::Object).collect());
        }
        Value::Array(
            rows.zip(&self.geometries)
                .map(|(row, geometry)| {
                    let mut row = row.clone();
                    row.insert("location".to_owned(), geometry.clone());
                    Value::Object(row)
                })
                .collect(),
        )
    }

    /// The fields that become attributes: every field but the core ones and the coordinates.
    fn attributes(&self, position: Option<&Position>) -> Vec<String> {
        self.fields()
            .into_iter()
            .filter(|field| !CORE.contains(&field.as_str()))
            .filter(|field| match position {
                Some(Position::LatLon { lat, lon }) => field != lat && field != lon,
                _ => true,
            })
            .collect()
    }

    /// The mapping of the drafted Pipeline: one entity per record, its id
    /// `urn:ngsi-ld:{class}:{org}:{space}:{localId}` built from `env("JC_ORG_DOMAIN")` and
    /// `env("JC_SPACE")`, so the pipeline mints ids of wherever it runs (PL-57, CC-82), its
    /// `location` a GeoProperty, every other
    /// field a Property (a JsonProperty for an object or a list) under the slot name `slots`
    /// gives it, a null value left out.
    pub fn mapping(&self, class: &str, slots: &BTreeMap<String, String>) -> String {
        let array = std::iter::once("this".to_owned())
            .chain(self.path.iter().map(|key| segment(key)))
            .collect::<Vec<_>>()
            .join(".");
        let prefix = format!(
            "{} + env(\"JC_ORG_DOMAIN\") + \":\" + env(\"JC_SPACE\") + \":\"",
            Value::String(format!("urn:ngsi-ld:{class}:"))
        );
        let id = match self.identifier() {
            Some(field) => format!("{prefix} + {field}.string()"),
            None => format!("{prefix} + uuid_v4()"),
        };
        let mut entries = vec![
            format!("  \"id\": {id}"),
            format!("  \"type\": {}", Value::String(class.to_owned())),
        ];
        let position = self.position();
        match &position {
            Some(Position::LatLon { lat, lon }) => {
                let (lat, lon) = (self.field(lat), self.field(lon));
                entries.push(format!(
                    "  \"location\": if {lat} != null && {lon} != null {{ {{ \"type\": \"GeoProperty\", \"value\": {{ \"type\": \"Point\", \"coordinates\": [ {lon}.number(), {lat}.number() ] }} }} }} else {{ deleted() }}"
                ));
            }
            Some(Position::Geometry) => entries.push(
                "  \"location\": if record.geometry != null { { \"type\": \"GeoProperty\", \"value\": record.geometry } } else { deleted() }".to_owned(),
            ),
            None => {}
        }
        for field in self.attributes(position.as_ref()) {
            let slot = slots
                .get(&field)
                .cloned()
                .unwrap_or_else(|| slot_name(&field));
            entries.push(format!("  {}: {}", Value::String(slot), self.field(&field)));
        }
        format!(
            "root = {array}.map_each(record -> {{\n{}\n}}.map_each(kv -> if kv.value == null {{ deleted() }} else if [\"id\", \"type\", \"location\"].contains(kv.key) {{ kv.value }} else if [\"object\", \"array\"].contains(kv.value.type()) {{ {{ \"type\": \"JsonProperty\", \"json\": kv.value }} }} else {{ {{ \"type\": \"Property\", \"value\": kv.value }} }}))\n",
            entries.join(",\n")
        )
    }
}

/// A field's slot name the way model-tools makes one when the model does not say.
fn slot_name(field: &str) -> String {
    let bare = field.split(['(', '[']).next().unwrap_or(field).trim();
    let mut name = String::new();
    for c in bare.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name = name.trim_matches('_');
    match name.chars().next() {
        None => "field".to_owned(),
        Some(first) if first.is_ascii_digit() => format!("_{name}"),
        Some(_) => name.to_owned(),
    }
}

/// The class's slots in the inferred LinkML: `(slot, its definition)` in the class's order.
fn class_slots(linkml: &str, class: &str) -> Vec<(String, serde_norway::Value)> {
    let Ok(model) = serde_norway::from_str::<serde_norway::Value>(linkml) else {
        return Vec::new();
    };
    model["classes"][class]["slots"]
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(|slot| slot.as_str())
        .map(|slot| (slot.to_owned(), model["slots"][slot].clone()))
        .collect()
}

/// Which slot each field of the sample became: the slot's `title.en` names the field when the
/// two differ, the slot's own name when they do not.
pub fn slot_names(linkml: &str, class: &str) -> BTreeMap<String, String> {
    class_slots(linkml, class)
        .into_iter()
        .map(|(slot, definition)| {
            let field = definition["title"]["en"]
                .as_str()
                .map_or_else(|| slot.clone(), str::to_owned);
            (field, slot)
        })
        .collect()
}

/// The slot a map colours its dots by: the first numeric one that is not an identifier, a time
/// or a coordinate.
pub fn colour_slot(linkml: &str, class: &str) -> Option<String> {
    class_slots(linkml, class)
        .into_iter()
        .filter(|(_, definition)| {
            matches!(
                definition["range"].as_str(),
                Some("integer" | "float" | "double" | "decimal")
            )
        })
        .map(|(slot, _)| slot)
        .find(|slot| {
            !slot
                .split('_')
                .flat_map(split_camel)
                .any(|word| NOT_A_MEASURE.contains(&word.to_ascii_lowercase().as_str()))
        })
}

/// `availableBikeNumber` → `available`, `Bike`, `Number`.
fn split_camel(word: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    for (i, c) in word.char_indices().skip(1) {
        if c.is_ascii_uppercase() {
            words.push(&word[start..i]);
            start = i;
        }
    }
    words.push(&word[start..]);
    words
}

/// Whether the class has a slot of this name.
pub fn has_slot(linkml: &str, class: &str, name: &str) -> bool {
    class_slots(linkml, class)
        .iter()
        .any(|(slot, _)| slot == name)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stations() -> Value {
        json!({
            "last_updated": 1789460937, "ttl": 60, "version": "2.2",
            "data": { "stations": [
                { "station_id": "008", "name": "Vanha kirkkopuisto", "lat": 60.165288, "lon": 24.93915, "capacity": 24, "rental_uris": {} },
                { "station_id": "015", "name": "Ritarikatu", "lat": 60.171609, "lon": 24.956159, "capacity": null, "rental_uris": {} }
            ] }
        })
    }

    #[test]
    fn the_records_are_the_largest_array_of_objects_not_the_envelope() {
        let found = records(&stations()).expect("records");
        assert_eq!(found.path, ["data", "stations"]);
        assert_eq!(found.rows.len(), 2);
        let fields: BTreeSet<String> = found.fields().into_iter().collect();
        assert_eq!(
            fields,
            [
                "capacity",
                "lat",
                "lon",
                "name",
                "rental_uris",
                "station_id"
            ]
            .map(String::from)
            .into()
        );

        let top = records(&json!([{ "id": 1 }, { "id": 2 }])).expect("an array is its records");
        assert!(top.path.is_empty());

        let deeper =
            json!({ "a": { "b": { "c": [{ "x": 1 }] } }, "short": [{ "y": 1 }, { "y": 2 }] });
        assert_eq!(
            records(&deeper).expect("records").path,
            ["short"],
            "the larger array wins"
        );

        assert_eq!(
            records(&json!({ "temperature": 12.5, "list": [1, 2, 3] })),
            None
        );
        assert_eq!(
            records(&json!({ "a": { "b": { "c": { "d": [{ "x": 1 }] } } } })),
            None,
            "four levels down is not searched"
        );
    }

    #[test]
    fn the_identifier_is_a_field_unique_in_every_record() {
        let bikes = json!({ "data": { "bikes": [
            { "bike_id": "a", "system_id": "hel", "station_id": "900", "lat": 60.1, "lon": 24.7 },
            { "bike_id": "b", "system_id": "hel", "station_id": "900", "lat": 60.2, "lon": 24.8 }
        ] } });
        assert_eq!(
            records(&bikes).and_then(|r| r.identifier()).as_deref(),
            Some("record.bike_id")
        );
        assert_eq!(
            records(&stations()).and_then(|r| r.identifier()).as_deref(),
            Some("record.station_id")
        );

        let id_first = json!([{ "code_id": "x", "id": 7 }, { "code_id": "y", "id": 8 }]);
        assert_eq!(
            records(&id_first).and_then(|r| r.identifier()).as_deref(),
            Some("record.id")
        );

        let missing = json!([{ "station_id": "1" }, { "name": "no id" }]);
        assert_eq!(records(&missing).and_then(|r| r.identifier()), None);
        let fractional = json!([{ "id": 1.5 }, { "id": 2.5 }]);
        assert_eq!(records(&fractional).and_then(|r| r.identifier()), None);
    }

    #[test]
    fn a_feature_collection_takes_its_properties_and_its_geometry() {
        let collection = json!({ "type": "FeatureCollection", "features": [
            { "type": "Feature", "id": "f1", "geometry": { "type": "Point", "coordinates": [24.9, 60.1] }, "properties": { "name": "A", "kind": "x" } },
            { "type": "Feature", "id": "f2", "geometry": { "type": "Point", "coordinates": [24.8, 60.2] }, "properties": { "name": "B", "kind": "x" } }
        ] });
        let found = records(&collection).expect("features");
        assert!(found.features);
        assert_eq!(found.position(), Some(Position::Geometry));
        assert_eq!(found.identifier().as_deref(), Some("record.id"));
        assert_eq!(found.inference_sample()[0]["location"]["type"], "Point");
        let mapping = found.mapping("Place", &BTreeMap::new());
        assert!(mapping.starts_with("root = this.features.map_each(record -> {"));
        assert!(mapping.contains("\"name\": record.properties.name"));
        assert!(mapping.contains("\"value\": record.geometry"));
    }

    #[test]
    fn the_position_is_a_numeric_latitude_and_longitude() {
        let found = records(&stations()).expect("records");
        assert_eq!(
            found.position(),
            Some(Position::LatLon {
                lat: "lat".into(),
                lon: "lon".into()
            })
        );
        let words = json!([{ "Latitude": "60.1", "Longitude": "24.9" }]);
        assert_eq!(
            records(&words).and_then(|r| r.position()),
            Some(Position::LatLon {
                lat: "Latitude".into(),
                lon: "Longitude".into()
            })
        );
        assert_eq!(
            records(&json!([{ "lat": "north", "lon": "east" }])).and_then(|r| r.position()),
            None
        );
        assert_eq!(
            records(&json!([{ "lat": 60.1 }])).and_then(|r| r.position()),
            None
        );
    }

    #[test]
    fn the_mapping_writes_one_entity_per_record_under_the_models_slot_names() {
        let found = records(&stations()).expect("records");
        let slots = BTreeMap::from([
            ("capacity".to_owned(), "capacity".to_owned()),
            ("rental_uris".to_owned(), "rentalUris".to_owned()),
        ]);
        let mapping = found.mapping("BikeHireDockingStation", &slots);
        assert!(
            mapping.starts_with("root = this.data.stations.map_each(record -> {\n"),
            "{mapping}"
        );
        assert!(mapping.contains(
            "\"id\": \"urn:ngsi-ld:BikeHireDockingStation:\" + env(\"JC_ORG_DOMAIN\") + \":\" + env(\"JC_SPACE\") + \":\" + record.station_id.string()"
        ), "{mapping}");
        // Neither the domain nor the space is typed in (CC-82, PL-57).
        assert!(
            !mapping.contains("hel.fi") && !mapping.contains("city-bikes"),
            "{mapping}"
        );
        assert!(mapping.contains("\"type\": \"BikeHireDockingStation\""));
        assert!(mapping.contains("\"coordinates\": [ record.lon.number(), record.lat.number() ]"));
        assert!(
            mapping.contains("\"rentalUris\": record.rental_uris"),
            "the model's slot name"
        );
        assert!(
            mapping.contains("\"name\": record.name"),
            "a field the model does not name keeps its own"
        );
        assert!(
            !mapping.contains("\"lat\":") && !mapping.contains("\"lon\":"),
            "the coordinates are the location"
        );
        assert!(
            mapping.contains("if kv.value == null { deleted() }"),
            "a null is left out"
        );
        assert!(mapping.contains("\"JsonProperty\""));

        let odd = json!({ "rows": [{ "Station name (fi)": "A", "2nd": 1, "we\"ird": 2 }] });
        let mapping = records(&odd)
            .expect("records")
            .mapping("Row", &BTreeMap::new());
        assert!(
            mapping.contains("\"Station_name\": record.\"Station name (fi)\""),
            "{mapping}"
        );
        assert!(mapping.contains("\"_2nd\": record.\"2nd\""));
        assert!(
            mapping.contains("record.\"we\\\"ird\""),
            "a quote in a name stays inside its string"
        );
        assert!(mapping.contains("uuid_v4()"), "no identifying field");
    }

    const LINKML: &str = r#"
classes:
  BikeHireDockingStation:
    is_a: Entity
    slots: [station_id, name, last_reported, capacity, num_bikes_available, rentalUris]
slots:
  station_id: { range: string }
  name: { range: string }
  last_reported: { range: integer }
  capacity: { range: integer }
  num_bikes_available: { range: integer }
  rentalUris: { range: string, title: { en: rental_uris } }
"#;

    #[test]
    fn the_model_names_the_slots_and_the_colour_is_a_measure() {
        let slots = slot_names(LINKML, "BikeHireDockingStation");
        assert_eq!(
            slots.get("rental_uris").map(String::as_str),
            Some("rentalUris")
        );
        assert_eq!(slots.get("capacity").map(String::as_str), Some("capacity"));
        assert_eq!(
            colour_slot(LINKML, "BikeHireDockingStation").as_deref(),
            Some("capacity")
        );
        assert!(has_slot(LINKML, "BikeHireDockingStation", "name"));
        assert!(!has_slot(LINKML, "Other", "name"));
        let only_times = "classes:\n  T:\n    slots: [last_updated, bikeId, updatedAt]\nslots:\n  last_updated: { range: integer }\n  bikeId: { range: integer }\n  updatedAt: { range: integer }\n";
        assert_eq!(colour_slot(only_times, "T"), None);
        assert_eq!(colour_slot("not: [yaml", "T"), None);
    }
}
