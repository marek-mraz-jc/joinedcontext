//! A visitor's map of every matching entity (T-7755): a read draws its first `READ_ROWS` rows,
//! and when the type holds more, the `data` event names a map source instead of carrying them.
//! `GET /api/v1/d/{publicId}/map/{id}?cursor=` answers the read again, page by page, as GeoJSON
//! the widget hands to the map: the label and popup columns and the location, nothing else.
//!
//! The source is held in this pod's memory for [`SOURCE_TTL`] under a random id, bound to the
//! deployment and to the address that asked (as a resumed turn is, T-4685); a miss answers `404`
//! and the widget keeps the rows it drew. Only a public Endpoint's read gets one: each page is read
//! as anyone, under the Endpoint's own Policy, and no token is ever held for it. The id stands for
//! the Endpoint, so an anonymous stream names no slug (T-4654).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Map, Value};

/// How long a source answers after its turn.
pub const SOURCE_TTL: Duration = Duration::from_secs(15 * 60);
/// The most sources held at once; the oldest goes first.
pub const MAX_SOURCES: usize = 5_000;
/// The entities one page asks for: a page of points stays under the surface's
/// `MAX_KEPT_CHARS` cut.
pub const PAGE: usize = 500;
/// The most pages of one source, so at most `PAGE * MAX_PAGES` (20,000) entities are drawn.
pub const MAX_PAGES: u32 = 40;
/// The pages one address may ask of a deployment in a minute: three maps of 20,000.
pub const PAGES_PER_MINUTE: u32 = 120;
/// The broker's longest cursor (Antares `cursor.rs`).
pub const MAX_CURSOR: usize = 2048;

/// What a map source reads again: the Endpoint, the type, the read's own `q` and the map's
/// columns. Never sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub slug: String,
    pub kind: String,
    pub q: Option<String>,
    pub timeout: Duration,
    /// The map spec's label column, its popup columns and its time column.
    pub fields: Vec<String>,
}

impl Read {
    /// The read of a public Endpoint's map spec: `None` for a spec without a label, or a
    /// connector whose `query_entities` cannot page `pick`ed key values.
    pub fn of(
        slug: &str,
        kind: &str,
        q: Option<&str>,
        timeout: Duration,
        spec: &Value,
        schema: &Value,
    ) -> Option<Self> {
        let offered = |name: &str| schema.pointer(&format!("/properties/{name}")).is_some();
        if !["pick", "format", "cursor", "limit"]
            .iter()
            .all(|n| offered(n))
        {
            return None;
        }
        let mut fields = vec![spec.get("label")?.as_str()?.to_owned()];
        let popup = spec.get("popup").and_then(Value::as_array);
        fields.extend(
            popup
                .into_iter()
                .flatten()
                .filter_map(|f| f.get("field")?.as_str().map(str::to_owned)),
        );
        fields.extend(spec.get("time").and_then(Value::as_str).map(str::to_owned));
        fields.dedup();
        Some(Self {
            slug: slug.to_owned(),
            kind: kind.to_owned(),
            q: q.map(str::to_owned),
            timeout,
            fields,
        })
    }

    /// One page's `query_entities` arguments: the read's type and `q`, its columns and the
    /// location as key values, `PAGE` of them from `cursor`.
    pub fn arguments(&self, cursor: Option<&str>) -> Value {
        let mut pick = self.fields.clone();
        pick.push("location".to_owned());
        let mut arguments = json!({
            "type": [self.kind],
            "pick": pick,
            "format": "keyValues",
            "limit": PAGE,
        });
        if let Some(q) = &self.q {
            arguments["q"] = json!(q);
        }
        if let Some(cursor) = cursor {
            arguments["cursor"] = json!(cursor);
        }
        arguments
    }

    /// A page's answer as the map draws it: each located entity a feature with the read's
    /// columns alone, and the next page's cursor. `None` when the answer is no list of entities.
    pub fn page(&self, text: &str) -> Option<(Vec<Value>, Option<String>)> {
        let answer: Value = serde_json::from_str(text).ok()?;
        let entities = answer.get("entities").unwrap_or(&answer).as_array()?;
        let features = entities
            .iter()
            .filter_map(|entity| {
                let location = entity.get("location")?;
                let geometry = super::viz::geometry(location.get("value").unwrap_or(location))?;
                let properties: Map<String, Value> = self
                    .fields
                    .iter()
                    .map(|f| (f.clone(), entity.get(f).map_or(Value::Null, cell)))
                    .collect();
                Some(json!({"type": "Feature", "geometry": geometry, "properties": properties}))
            })
            .collect();
        let next = answer
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|_| !entities.is_empty())
            .map(str::to_owned);
        Some((features, next))
    }
}

/// A key value as a map's popup shows it: a value inside `{"value"}` is taken out, a structured
/// one becomes its JSON text (as `viz::map_of` writes a row's cell).
fn cell(value: &Value) -> Value {
    let value = value.get("value").unwrap_or(value);
    match value {
        Value::Array(_) | Value::Object(_) => Value::String(value.to_string()),
        other => other.clone(),
    }
}

/// Whether a cursor is one the broker could have written: printable ASCII, at most
/// [`MAX_CURSOR`] characters.
pub fn cursor_ok(cursor: &str) -> bool {
    !cursor.is_empty() && cursor.len() <= MAX_CURSOR && cursor.bytes().all(|b| b.is_ascii_graphic())
}

struct Held {
    public_id: String,
    visitor: String,
    read: Read,
    at: Instant,
    pages: u32,
}

/// The map sources of this pod, by id.
#[derive(Default)]
pub struct Sources {
    held: Mutex<HashMap<String, Held>>,
}

/// What a page request finds under an id.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// The read, and whether this is its last page allowed.
    Page(Read, bool),
    /// Every page allowed was read already.
    Spent,
    Missing,
}

impl Sources {
    /// Holds `read` for `visitor` of `public_id` and returns its random id.
    pub fn hold(&self, public_id: &str, visitor: &str, read: Read, now: Instant) -> Option<String> {
        let mut bytes = [0u8; 16];
        SystemRandom::new().fill(&mut bytes).ok()?;
        let id = hex::encode(bytes);
        let mut held = self.held.lock().ok()?;
        held.retain(|_, h| now.duration_since(h.at) < SOURCE_TTL);
        if held.len() >= MAX_SOURCES {
            let oldest = held
                .iter()
                .min_by_key(|(_, h)| h.at)
                .map(|(id, _)| id.clone());
            held.remove(&oldest?);
        }
        held.insert(
            id.clone(),
            Held {
                public_id: public_id.to_owned(),
                visitor: visitor.to_owned(),
                read,
                at: now,
                pages: 0,
            },
        );
        Some(id)
    }

    /// The next page of source `id` as `visitor` of `public_id` may read it: another visitor's,
    /// another deployment's or an expired one is missing, as an unknown one is. Counts the page.
    pub fn page(&self, public_id: &str, visitor: &str, id: &str, now: Instant) -> Found {
        let Ok(mut held) = self.held.lock() else {
            return Found::Missing;
        };
        let Some(h) = held.get_mut(id) else {
            return Found::Missing;
        };
        if h.public_id != public_id
            || h.visitor != visitor
            || now.duration_since(h.at) >= SOURCE_TTL
        {
            return Found::Missing;
        }
        if h.pages >= MAX_PAGES {
            return Found::Spent;
        }
        h.pages += 1;
        Found::Page(h.read.clone(), h.pages == MAX_PAGES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({"properties": {"type": {}, "q": {}, "pick": {}, "format": {}, "cursor": {}, "limit": {}}})
    }

    fn spec() -> Value {
        json!({"label": "name", "popup": [{"field": "yearBuilt", "label": "yearBuilt"}], "time": "observedAt"})
    }

    fn read() -> Read {
        Read::of(
            "bbsk-registre",
            "Bridge",
            Some("yearBuilt<1900"),
            Duration::from_secs(10),
            &spec(),
            &schema(),
        )
        .expect("a read")
    }

    #[test]
    fn a_page_asks_the_reads_filter_and_only_the_maps_columns() {
        assert_eq!(
            read().arguments(Some("abc")),
            json!({"type": ["Bridge"], "q": "yearBuilt<1900", "pick": ["name", "yearBuilt", "observedAt", "location"],
                "format": "keyValues", "limit": PAGE, "cursor": "abc"})
        );
        assert!(read().arguments(None).get("cursor").is_none());
    }

    #[test]
    fn a_connector_that_cannot_page_picked_key_values_gets_no_source() {
        let without = json!({"properties": {"type": {}, "limit": {}}});
        assert_eq!(
            Read::of("e", "Bridge", None, Duration::ZERO, &spec(), &without),
            None
        );
        assert_eq!(
            Read::of("e", "Bridge", None, Duration::ZERO, &json!({}), &schema()),
            None
        );
    }

    #[test]
    fn a_page_keeps_the_located_entities_with_the_maps_columns_alone() {
        let text = json!({"entities": [
            {"id": "urn:ngsi-ld:Bridge:1", "type": "Bridge", "name": "Most 1", "yearBuilt": 1890, "owner": "x",
             "location": {"type": "Point", "coordinates": [19.1234567, 48.7]}},
            {"id": "urn:ngsi-ld:Bridge:2", "type": "Bridge", "name": "Most 2"},
            {"id": "urn:ngsi-ld:Bridge:3", "name": {"value": "Most 3"}, "location": {"value": {"type": "Point", "coordinates": [19.0, 48.0]}}}
        ], "nextCursor": "n2"})
        .to_string();
        let (features, next) = read().page(&text).expect("a page");
        assert_eq!(next.as_deref(), Some("n2"));
        assert_eq!(features.len(), 2);
        assert_eq!(
            features[0],
            json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [19.123457, 48.7]},
                "properties": {"name": "Most 1", "yearBuilt": 1890, "observedAt": null}})
        );
        assert_eq!(features[1]["properties"]["name"], "Most 3");
        let (_, last) = read()
            .page(r#"{"entities": [], "nextCursor": "x"}"#)
            .expect("a page");
        assert_eq!(last, None);
        assert!(read().page("not json").is_none());
    }

    #[test]
    fn a_source_is_the_askers_alone_and_ends_after_its_pages_or_its_time() {
        let sources = Sources::default();
        let now = Instant::now();
        let id = sources.hold("pub1", "v1", read(), now).expect("an id");
        assert_eq!(id.len(), 32);
        assert_eq!(sources.page("pub2", "v1", &id, now), Found::Missing);
        assert_eq!(sources.page("pub1", "v2", &id, now), Found::Missing);
        assert_eq!(
            sources.page("pub1", "v1", "0".repeat(32).as_str(), now),
            Found::Missing
        );
        for n in 1..=MAX_PAGES {
            assert_eq!(
                sources.page("pub1", "v1", &id, now),
                Found::Page(read(), n == MAX_PAGES)
            );
        }
        assert_eq!(sources.page("pub1", "v1", &id, now), Found::Spent);
        let other = sources.hold("pub1", "v1", read(), now).expect("an id");
        assert_eq!(
            sources.page("pub1", "v1", &other, now + SOURCE_TTL),
            Found::Missing
        );
    }

    #[test]
    fn a_cursor_is_printable_and_bounded() {
        assert!(cursor_ok("eyJvIjo1MDB9"));
        assert!(!cursor_ok(""));
        assert!(!cursor_ok("a b"));
        assert!(!cursor_ok("a\nb"));
        assert!(!cursor_ok(&"a".repeat(MAX_CURSOR + 1)));
    }
}
