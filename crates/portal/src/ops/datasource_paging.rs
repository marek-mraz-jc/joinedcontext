//! How an `http` source pages, read from its first answer (T-7094, G-22): the data source wizard
//! asks for it once the person gave an address, and shows the guess in words ("reads 3 pages of
//! 200") before the person checks the source. The Portal reads the address once on the project's
//! runner, as discovery reads a listing (`datasource_discover.rs`): held to the public internet
//! first (AG-65), refused with a credential in it, and at most `PAGE_BYTES` of the answer.
//!
//! The guess looks at what the answer says about its next page, in this order: an RFC 8288
//! `Link: <…>; rel="next"` header; a WFS GetFeature address (`startIndex`/`count`); a next link in
//! the body (`@odata.nextLink`, `@iot.nextLink`, `nextLink`, `links[rel=next]`); ArcGIS's
//! `exceededTransferLimit`; a CKAN `package_search` count; and last a bare total. Only the first
//! five name a walk the runner can follow; a bare total is said, and the person picks the mode.

use jc_core::kinds::data_source::{refused_target, HttpPaging, HttpPagingMode};
use jc_core::kinds::PipelineSpec;
use jcctl::pipeline_test::{harness, Sample, SampleFormat, TestTrace, FETCH_FAILED};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use utoipa::ToSchema;

use crate::error::ApiError;

/// The bytes of the first answer the runner hands back; a longer one is not guessed from.
pub const PAGE_BYTES: usize = 1024 * 1024;

/// The address whose first answer is read.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schema(example = json!({ "url": "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?service=WFS&request=GetFeature&typeNames=avoindata:x&outputFormat=application/json" }))]
pub struct PagingRequest {
    /// The address the source reads, as the person typed it.
    pub url: String,
}

/// What the first answer says about the pages after it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PagingGuess {
    /// The walk to write into `spec.http.paging`; absent when the answer names no next page
    /// the runner can follow.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub paging: Option<HttpPaging>,
    /// Where the records sit, for `spec.http.records`: `features`, `value`, `result.results`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records: Option<String>,
    /// The records on the first page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_page: Option<u64>,
    /// The records in all, as the answer announces them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The pages a walk reads: the total over the first page's records, rounded up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u64>,
    /// The answer says more pages follow without saying how many (ArcGIS, a next link).
    pub more: bool,
    /// Why nothing was guessed: the address did not answer, or answered no JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// The address as it is read: http(s), and no credential in it, since the runner logs the URL it
/// fetches (T-3250). A source that needs a credential gets it as a secretRef header instead.
pub fn page_url(typed: &str) -> Result<Url, String> {
    let mut url = Url::parse(typed.trim()).map_err(|e| format!("the URL does not parse: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the URL must start with https:// or http://".into());
    }
    if crate::ops::compute::credential_in_address(&url) {
        return Err(
            "the URL carries a user name, a password or a key; give one without it and pick the credential as a secret"
                .into(),
        );
    }
    url.set_fragment(None);
    Ok(url)
}

/// The runner configuration that reads `url` once and posts its first `PAGE_BYTES`, its whole
/// length and its `Link` header to `capture`.
pub(crate) fn page_read(url: &Url, capture: &str) -> Result<Value, ApiError> {
    if let Some(reason) = refused_target(url.as_str()) {
        return Err(ApiError::BadRequest(format!("url: {reason}")));
    }
    let envelope = format!(
        concat!(
            "root = {{}}\n",
            "root.input = if errored() {{ \"\" }} else {{ content().string().slice(0, {}) }}\n",
            "root.output = if errored() {{ deleted() }} else {{ {{\"bytes\": content().length(), \"link\": (@link | @Link | \"\")}} }}\n",
            "root.error = if errored() {{ \"{}\" + error() }} else {{ null }}"
        ),
        PAGE_BYTES, FETCH_FAILED
    );
    let probe: PipelineSpec = serde_json::from_value(json!({
        "class": "auto",
        "targetEndpoint": "urn:ngsi-ld:Endpoint:probe.local:probe:probe"
    }))
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let sample = Sample {
        text: Some(String::new()),
        url: None,
        format: SampleFormat::Text,
        records: None,
    };
    let mut config =
        harness(&probe, &sample, capture).map_err(|e| ApiError::Internal(e.to_string()))?;
    config["input"] =
        json!({ "generate": { "count": 1, "interval": "", "mapping": "root = \"\"" } });
    config["pipeline"] = json!({ "processors": [
        { "http": { "url": url.as_str(), "verb": "GET", "timeout": "5s", "retries": 0,
                    "headers": { "Accept": "application/json" },
                    "extract_headers": { "include_patterns": ["(?i)^link$"] } } },
        { "mapping": envelope },
        { "catch": [] },
    ] });
    Ok(config)
}

/// The guess from one read: what the body and its `Link` header say, or why there is none.
pub(crate) fn guess_of(url: &Url, trace: TestTrace) -> PagingGuess {
    let out = trace.mapping.first().cloned().unwrap_or(Value::Null);
    let bytes = out["bytes"].as_u64().unwrap_or(0) as usize;
    let link = out["link"].as_str().filter(|l| !l.is_empty()).map(str::to_owned);
    let sample = trace.input.sample.clone();
    let probe = crate::api::pipeline_test::answer_of(trace);
    let skipped = |why: String| PagingGuess {
        skipped: Some(why),
        ..PagingGuess::default()
    };
    if let Some(reason) = probe.skipped {
        return skipped(reason);
    }
    if bytes > PAGE_BYTES {
        return skipped(format!(
            "the first answer is {bytes} bytes, more than the {PAGE_BYTES} the guess reads; set the paging under Advanced"
        ));
    }
    let body = match sample {
        Some(Value::String(text)) => serde_json::from_str(&text).unwrap_or(Value::Null),
        Some(value) => value,
        None => Value::Null,
    };
    if body.is_null() {
        return skipped("the address answered no JSON, so it names no next page".into());
    }
    guess(url, &body, link.as_deref())
}

/// The names a total goes by, tried in order.
const TOTALS: [&str; 7] = [
    "numberMatched",
    "totalFeatures",
    "@odata.count",
    "@iot.count",
    "total",
    "totalCount",
    "count",
];

/// The body paths a next page's URL goes by, tried in order.
const NEXTS: [&str; 4] = ["@odata.nextLink", "@iot.nextLink", "nextLink", "next"];

/// What `body`, the first answer at `url`, says about the pages after it; `link` is its `Link`
/// header.
pub fn guess(url: &Url, body: &Value, link: Option<&str>) -> PagingGuess {
    let records = records_path(body);
    let on_page = records
        .as_deref()
        .map_or(body.as_array(), |path| at(body, path).and_then(Value::as_array))
        .map(|items| items.len() as u64);
    let ckan = url.path().ends_with("/package_search");
    let total = if ckan {
        at(body, "result.count").and_then(Value::as_u64)
    } else {
        TOTALS
            .iter()
            .find_map(|name| body.get(*name).and_then(Value::as_u64))
    };
    let page = |mode, more: bool| HttpPaging {
        mode,
        total: TOTALS
            .iter()
            .find(|name| body.get(**name).and_then(Value::as_u64).is_some())
            .filter(|_| !more)
            .map(|name| (*name).to_owned()),
        ..HttpPaging::default()
    };
    let arcgis_more = ["exceededTransferLimit", "properties.exceededTransferLimit"]
        .into_iter()
        .find(|path| at(body, path).is_some_and(Value::is_boolean));
    let next_in_body = NEXTS
        .iter()
        .find(|name| body.get(**name).and_then(Value::as_str).is_some_and(is_url_like))
        .map(|name| (*name).to_owned())
        .or_else(|| {
            body.get("links")
                .and_then(Value::as_array)
                .is_some_and(|links| {
                    links
                        .iter()
                        .any(|l| l["rel"] == "next" && l["href"].is_string())
                })
                .then(|| "links[rel=next].href".to_owned())
        });
    let paging = if link.is_some_and(names_next) {
        Some(page(HttpPagingMode::Link, false))
    } else if is_wfs_get_feature(url) && total.is_some() {
        Some(HttpPaging {
            offset_param: Some("startIndex".into()),
            limit_param: Some("count".into()),
            limit: on_page.filter(|n| *n > 0),
            ..page(HttpPagingMode::Offset, false)
        })
    } else if let Some(next) = next_in_body {
        Some(HttpPaging {
            next: Some(next),
            ..page(HttpPagingMode::NextUrl, true)
        })
    } else if let Some(more) = arcgis_more {
        Some(HttpPaging {
            offset_param: Some("resultOffset".into()),
            limit_param: Some("resultRecordCount".into()),
            limit: on_page.filter(|n| *n > 0),
            more: Some(more.to_owned()),
            ..page(HttpPagingMode::Offset, true)
        })
    } else if ckan && total.is_some() {
        Some(HttpPaging {
            mode: HttpPagingMode::Offset,
            offset_param: Some("start".into()),
            limit_param: Some("rows".into()),
            limit: on_page.filter(|n| *n > 0),
            total: Some("result.count".into()),
            ..HttpPaging::default()
        })
    } else {
        None
    };
    let pages = match (total, on_page) {
        (Some(total), Some(on_page)) if on_page > 0 => Some(total.div_ceil(on_page)),
        _ => None,
    };
    let more = paging.is_some()
        && (arcgis_more.is_some_and(|path| at(body, path) == Some(&Value::Bool(true)))
            || (pages.is_none() && paging.as_ref().is_some_and(|p| p.total.is_none())));
    PagingGuess {
        paging,
        records,
        on_page,
        total,
        pages,
        more,
        skipped: None,
    }
}

/// The value at a dotted path of names.
fn at<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(body, |value, name| value.get(name))
}

/// Where the records sit: none for a top-level array, else the longest array among the body's
/// members and their members, one level down (`result.results`).
fn records_path(body: &Value) -> Option<String> {
    let object = body.as_object()?;
    let mut found: Option<(usize, String)> = None;
    let mut keep = |len: usize, path: String| {
        if found.as_ref().is_none_or(|(best, _)| len > *best) {
            found = Some((len, path));
        }
    };
    for (name, value) in object {
        if name == "links" {
            continue;
        }
        match value {
            Value::Array(items) if items.iter().any(Value::is_object) => keep(items.len(), name.clone()),
            Value::Object(inner) => {
                for (child, value) in inner {
                    if let Value::Array(items) = value {
                        if items.iter().any(Value::is_object) {
                            keep(items.len(), format!("{name}.{child}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    found
        .map(|(_, path)| path)
        .filter(|path| jc_core::kinds::data_source::records_mapping(path).is_ok())
}

/// A next-page value that is an address, absolute or relative (`Customers?$skiptoken=…`).
fn is_url_like(text: &str) -> bool {
    !text.is_empty() && !text.contains(char::is_whitespace)
}

/// Whether a `Link` header names a `rel="next"` target.
fn names_next(link: &str) -> bool {
    link.split(',').any(|part| {
        part.split(';').skip(1).any(|param| {
            let param = param.trim().to_ascii_lowercase();
            param
                .strip_prefix("rel=")
                .is_some_and(|rel| rel.trim_matches('"').split(' ').any(|r| r == "next"))
        })
    })
}

/// A WFS GetFeature address, which pages by `startIndex` and `count`.
fn is_wfs_get_feature(url: &Url) -> bool {
    url.query_pairs().any(|(key, value)| {
        key.eq_ignore_ascii_case("request") && value.eq_ignore_ascii_case("getfeature")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESETS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ekiben/crates/ekiben-io-http/tests/fixtures/presets"
    );

    fn recorded(folder: &str) -> (Value, Value) {
        let read = |file: &str| -> Value {
            let text = std::fs::read_to_string(format!("{PRESETS}/{folder}/{file}")).expect("fixture");
            serde_json::from_str(&text).expect("json")
        };
        (read("page-1.json"), read("render.json"))
    }

    /// The guess from each preset's first recorded page names the walk jc-manifest renders for
    /// that preset: the same mode, records, next path, stop rule and parameters.
    #[test]
    fn the_guess_from_each_recorded_first_page_is_the_walk_its_preset_renders() {
        for folder in ["arcgis", "wfs", "ckan", "odata", "sensorthings", "ogc"] {
            let (page, render) = recorded(folder);
            let url = Url::parse(render["url"].as_str().expect("url")).expect("url");
            let guessed = guess(&url, &page, None);
            let paging = guessed.paging.clone().expect(folder);
            let walk = &render["paginate"];
            assert_eq!(paging.mode.as_str(), walk["mode"], "{folder}");
            assert_eq!(guessed.records.as_deref(), walk["records"].as_str(), "{folder}");
            assert_eq!(paging.next.as_deref(), walk["next"].as_str(), "{folder}");
            assert_eq!(paging.more.as_deref(), walk["more"].as_str(), "{folder}");
            assert_eq!(paging.offset_param.as_deref(), walk["offset_param"].as_str(), "{folder}");
            assert_eq!(paging.limit_param.as_deref(), walk["limit_param"].as_str(), "{folder}");
            if walk["mode"] == "offset" {
                assert_eq!(paging.limit, walk["limit"].as_u64(), "{folder}");
                assert_eq!(paging.total.as_deref(), walk["total"].as_str(), "{folder}");
            }
        }
    }

    #[test]
    fn the_pages_are_the_total_over_the_first_page_and_an_open_walk_says_more() {
        let pages = |folder| {
            let (page, render) = recorded(folder);
            let url = Url::parse(render["url"].as_str().expect("url")).expect("url");
            let g = guess(&url, &page, None);
            (g.on_page, g.total, g.pages, g.more)
        };
        assert_eq!(pages("wfs"), (Some(200), Some(453), Some(3), false));
        assert_eq!(pages("ogc"), (Some(10), Some(25), Some(3), false));
        assert_eq!(pages("ckan"), (Some(1), Some(2), Some(2), false));
        assert_eq!(pages("arcgis"), (Some(60), None, None, true));
        assert_eq!(pages("odata"), (Some(20), None, None, true));
    }

    #[test]
    fn a_link_header_names_the_walk_and_a_bare_total_names_none() {
        let url = Url::parse("https://api.example/items?page=1").expect("url");
        let body = json!([{ "id": 1 }, { "id": 2 }]);
        let linked = guess(
            &url,
            &body,
            Some(r#"<https://api.example/items?page=2>; rel="next", <https://api.example/items?page=9>; rel="last""#),
        );
        assert_eq!(linked.paging.map(|p| p.mode), Some(HttpPagingMode::Link));
        assert_eq!((linked.records, linked.on_page), (None, Some(2)));
        assert!(!names_next(r#"<https://api.example/items?page=1>; rel="prev""#));

        let bare = guess(&url, &json!({ "total": 13500, "data": [{ "id": 1 }] }), None);
        assert_eq!(bare.paging, None);
        assert_eq!((bare.records.as_deref(), bare.total, bare.pages), (Some("data"), Some(13500), Some(13500)));
        assert!(!bare.more);
    }

    #[test]
    fn an_address_with_a_credential_or_another_scheme_is_refused_with_what_to_fix() {
        for typed in [
            "https://robot:hunter2@api.example/x",
            "https://api.example/x?apikey=abc123",
            "ftp://api.example/x",
        ] {
            let why = page_url(typed).expect_err(typed);
            assert!(!why.contains("hunter2") && !why.contains("abc123"), "{why}");
        }
        assert!(page_url("https://api.example/x?page=1#top").is_ok());
        let inside = page_url("http://127.0.0.1:4195/x").expect("parses");
        assert!(page_read(&inside, "http://portal/x").is_err());
    }

    #[test]
    fn the_read_keeps_the_link_header_and_a_failed_or_long_read_is_skipped_with_the_reason() {
        let url = page_url("https://api.example/items").expect("url");
        let config = page_read(&url, "http://portal/x").expect("config");
        let fetch = &config["pipeline"]["processors"][0]["http"];
        assert_eq!(fetch["url"], "https://api.example/items");
        assert!(fetch["extract_headers"].to_string().contains("link"));
        let failed = TestTrace::default();
        assert!(guess_of(&url, failed).skipped.is_some());
    }
}
