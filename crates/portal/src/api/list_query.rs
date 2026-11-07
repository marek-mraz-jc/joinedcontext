//! The one list contract of API/13 section 2 (T-6175): `limit`, `cursor`, `sort`, `q` and
//! `count`, read the same way by every list route. A list handler takes [`ListQuery`] as an
//! extractor beside its own parameters; a bad value is a `400` whose `errors[]` names the
//! parameter, and the handler answers a `sort` field its rows do not have with a `422`.
//!
//! `orderBy` (`field desc`) is the old name of `sort` and stays an alias until every caller sends
//! `sort` (API/13 section 7).

use std::sync::LazyLock;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// The largest page a list answers; NGSI-LD's own bound.
pub const MAX_LIMIT: usize = 1000;
/// The longest `q` a list searches with.
pub const MAX_Q: usize = 200;

/// One field to order by, `-` in front for descending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub field: String,
    pub descending: bool,
}

impl Sort {
    /// The `orderBy` text the kind filters understand: `field` or `field desc`.
    pub fn order_by(&self) -> String {
        if self.descending {
            format!("{} desc", self.field)
        } else {
            self.field.clone()
        }
    }
}

/// The paging, order, search and count a list call asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    /// Rows per page, 1 to [`MAX_LIMIT`]; absent means the whole list.
    pub limit: Option<usize>,
    /// The `nextCursor` of the previous page.
    pub cursor: Option<String>,
    pub sort: Option<Sort>,
    /// Text every row is searched for, case-insensitively, already lowercased.
    pub q: Option<String>,
    /// Whether the answer carries `totalCount`.
    pub count: bool,
}

fn refused(field: &str, message: &str) -> ApiError {
    ApiError::Invalid {
        detail: format!("the list parameter `{field}` is not valid: {message}"),
        errors: vec![format!("{field}: {message}")],
    }
}

impl ListQuery {
    /// Reads the parameters of one query string; every other parameter is the handler's.
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            if matches!(
                &*name,
                "limit" | "cursor" | "sort" | "orderBy" | "q" | "count"
            ) {
                if seen.iter().any(|(each, _)| *each == name) {
                    return Err(refused(&name, "send it once"));
                }
                seen.push((name.into_owned(), value.into_owned()));
            }
        }
        let get = |name: &str| {
            seen.iter()
                .find(|(each, _)| each == name)
                .map(|(_, value)| value.trim())
        };
        let limit = match get("limit") {
            None => None,
            Some(text) => match text.parse::<usize>() {
                Ok(n) if (1..=MAX_LIMIT).contains(&n) => Some(n),
                _ => {
                    return Err(refused(
                        "limit",
                        "must be a whole number from 1 to 1000, e.g. limit=50",
                    ))
                }
            },
        };
        let sort = match (get("sort"), get("orderBy")) {
            (Some(_), Some(_)) => {
                return Err(refused(
                    "orderBy",
                    "send sort or orderBy, not both; sort replaces orderBy",
                ))
            }
            (Some(text), None) => sort_of(text, "sort")?,
            (None, Some(text)) => order_by_of(text)?,
            (None, None) => None,
        };
        let q = match get("q") {
            Some(text) if text.chars().count() > MAX_Q => {
                return Err(refused("q", "must be at most 200 characters, e.g. q=air"))
            }
            Some(text) if !text.is_empty() => Some(text.to_lowercase()),
            _ => None,
        };
        let count = match get("count") {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => return Err(refused("count", "must be true or false, e.g. count=true")),
        };
        Ok(Self {
            limit,
            cursor: get("cursor")
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            sort,
            q,
            count,
        })
    }

    /// Whether `q` is absent or found in the row's name, title, labels or spec.
    pub fn finds(&self, doc: &Value) -> bool {
        let Some(q) = &self.q else { return true };
        let mut found = false;
        for at in [
            "/metadata/name",
            "/metadata/title",
            "/metadata/labels",
            "/spec",
        ] {
            if let Some(value) = doc.pointer(at) {
                visit_text(value, &mut |text| found |= text.to_lowercase().contains(q));
            }
            if found {
                return true;
            }
        }
        false
    }

    /// One listing: the caller, the route's own parameters and this query's order and search,
    /// so a cursor cannot be replayed under another filter, order, search or person.
    pub fn listing_id(&self, parts: &[Option<&str>]) -> String {
        let order = self.sort.as_ref().map(Sort::order_by);
        let mut hash = Sha256::new();
        for part in parts
            .iter()
            .copied()
            .chain([order.as_deref(), self.q.as_deref()])
        {
            hash.update(part.unwrap_or_default().as_bytes());
            hash.update([0]);
        }
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.finalize())
    }
}

fn visit_text(value: &Value, each: &mut impl FnMut(&str)) {
    match value {
        Value::String(text) => each(text),
        Value::Array(items) => items.iter().for_each(|item| visit_text(item, each)),
        Value::Object(members) => members.values().for_each(|item| visit_text(item, each)),
        _ => {}
    }
}

fn field_ok(field: &str) -> bool {
    !field.is_empty()
        && field.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

fn sort_of(text: &str, name: &str) -> Result<Option<Sort>, ApiError> {
    if text.is_empty() {
        return Ok(None);
    }
    let (field, descending) = match text.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (text, false),
    };
    if !field_ok(field) {
        return Err(refused(
            name,
            "must be one field, `-` in front for descending, e.g. sort=-metadata.name",
        ));
    }
    Ok(Some(Sort {
        field: field.to_owned(),
        descending,
    }))
}

fn order_by_of(text: &str) -> Result<Option<Sort>, ApiError> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let descending = match words.as_slice() {
        [] => return Ok(None),
        [_] | [_, "asc"] => false,
        [_, "desc"] => true,
        _ => {
            return Err(refused(
                "orderBy",
                "must be one field, optionally followed by asc or desc, e.g. orderBy=metadata.name desc",
            ))
        }
    };
    let Some(Sort { field, .. }) = sort_of(words[0], "orderBy")? else {
        return Ok(None);
    };
    Ok(Some(Sort { field, descending }))
}

impl<S: Send + Sync> FromRequestParts<S> for ListQuery {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Self::parse(parts.uri.query())
    }
}

/// Signs list cursors; a fresh key per process, so a restart ends every open listing with a
/// `400` that says to start again.
static CURSOR_KEY: LazyLock<String> = LazyLock::new(crate::auth::csrf::new_token);

/// Where a page ended, for the listing it was made for alone.
#[derive(Debug, Serialize, Deserialize)]
struct CursorBody {
    #[serde(rename = "q")]
    query: String,
    #[serde(rename = "k")]
    key: Value,
    #[serde(rename = "n")]
    name: String,
}

fn cursor_mac(payload: &str) -> Hmac<Sha256> {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(CURSOR_KEY.as_bytes()).expect("any key length");
    mac.update(payload.as_bytes());
    mac
}

/// The cursor of the page after the row with this sort `key` and tie-breaking `name`.
pub fn seal(listing_id: &str, key: &Value, name: &str) -> String {
    let body = CursorBody {
        query: listing_id.to_owned(),
        key: key.clone(),
        name: name.to_owned(),
    };
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = engine.encode(serde_json::to_vec(&body).unwrap_or_default());
    let tag = engine.encode(cursor_mac(&payload).finalize().into_bytes());
    format!("{payload}.{tag}")
}

/// The sort key and name a cursor of this listing ends at; any other text is a `400`.
pub fn open(text: &str, listing_id: &str) -> Result<(Value, String), ApiError> {
    let refused = || {
        refused(
            "cursor",
            "is not one this listing handed out: list again without cursor, with the same filter, sort and q",
        )
    };
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (payload, tag) = text.split_once('.').ok_or_else(refused)?;
    let tag = engine.decode(tag).map_err(|_| refused())?;
    cursor_mac(payload)
        .verify_slice(&tag)
        .map_err(|_| refused())?;
    let body: CursorBody = engine
        .decode(payload)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .ok_or_else(refused)?;
    if body.query != listing_id {
        return Err(refused());
    }
    Ok((body.key, body.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field_of(err: ApiError) -> String {
        match err {
            ApiError::Invalid { errors, .. } => errors.join("|"),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn a_bad_value_is_refused_naming_its_parameter() {
        for (raw, field) in [
            ("limit=0", "limit:"),
            ("limit=1001", "limit:"),
            ("limit=ten", "limit:"),
            ("limit=-1", "limit:"),
            ("count=yes", "count:"),
            ("sort=a b", "sort:"),
            ("sort=-", "sort:"),
            ("sort=metadata..name", "sort:"),
            ("orderBy=a%20up", "orderBy:"),
            ("sort=a&orderBy=b", "orderBy:"),
            ("limit=1&limit=2", "limit:"),
        ] {
            let err = ListQuery::parse(Some(raw)).expect_err(raw);
            assert!(field_of(err).starts_with(field), "{raw}");
        }
        let long = format!("q={}", "a".repeat(MAX_Q + 1));
        assert!(field_of(ListQuery::parse(Some(&long)).expect_err("long q")).starts_with("q:"));
    }

    #[test]
    fn sort_and_its_old_name_read_the_same() {
        let desc = Some(Sort {
            field: "metadata.name".into(),
            descending: true,
        });
        for raw in ["sort=-metadata.name", "orderBy=metadata.name%20desc"] {
            assert_eq!(ListQuery::parse(Some(raw)).expect(raw).sort, desc, "{raw}");
        }
        let query = ListQuery::parse(Some("limit=1000&count=true&q=%20Air%20&cursor=x&other=1"))
            .expect("valid");
        assert_eq!(
            (
                query.limit,
                query.count,
                query.q.as_deref(),
                query.cursor.as_deref()
            ),
            (Some(1000), true, Some("air"), Some("x"))
        );
        assert_eq!(ListQuery::parse(None).expect("empty"), ListQuery::default());
    }

    #[test]
    fn q_finds_name_title_labels_and_spec_text_alone() {
        let query = ListQuery::parse(Some("q=OZON")).expect("valid");
        assert!(query.finds(&json!({ "metadata": { "name": "ozone-sk" } })));
        assert!(query.finds(
            &json!({ "metadata": { "name": "a", "title": { "sk": "Ozón", "en": "Ozone" } } })
        ));
        assert!(query.finds(&json!({ "metadata": { "name": "a", "labels": { "gas": "ozon" } } })));
        assert!(query.finds(&json!({ "metadata": { "name": "a" }, "spec": { "outputs": [{ "target": "ozon-ep" }] } })));
        assert!(
            !query.finds(&json!({ "metadata": { "name": "a" }, "status": { "note": "ozon" } })),
            "status is not searched"
        );
        assert!(ListQuery::default().finds(&json!({})));
    }

    #[test]
    fn a_cursor_reads_back_only_for_its_own_listing() {
        let air = ListQuery::parse(Some("sort=-metadata.name")).expect("valid");
        let id = air.listing_id(&[Some("f:1:ana"), Some("ovzdusie"), Some("pipelines")]);
        let text = seal(&id, &json!("air"), "a");
        assert_eq!(
            open(&text, &id).expect("its own"),
            (json!("air"), "a".to_owned())
        );

        let other_caller = air.listing_id(&[Some("f:1:eva"), Some("ovzdusie"), Some("pipelines")]);
        let other_q = ListQuery::parse(Some("sort=-metadata.name&q=x"))
            .expect("valid")
            .listing_id(&[Some("f:1:ana"), Some("ovzdusie"), Some("pipelines")]);
        let other_sort = ListQuery::default().listing_id(&[
            Some("f:1:ana"),
            Some("ovzdusie"),
            Some("pipelines"),
        ]);
        for other in [other_caller, other_q, other_sort] {
            assert!(open(&text, &other).is_err());
        }
        let (payload, tag) = text.split_once('.').expect("two parts");
        let forged = format!(
            "{}.{tag}",
            seal(&id, &json!("air"), "zzz")
                .split_once('.')
                .expect("two")
                .0
        );
        for bad in [forged.as_str(), payload, "", "x.y", &format!("{payload}.")] {
            assert!(open(bad, &id).is_err(), "{bad} was accepted");
        }
    }
}
