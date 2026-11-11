//! `feeds/{output}.csv` and `feeds/{filter}/{output}.csv` (API/02 §4a, T-6760): the rows of a
//! stored output's query as `file.csv` builds them. The output's types and `filter` (its `q`
//! and `scopeQ` AND-ed with the caller's, its geo-query fixed) narrow the caller's query, which
//! is then `file.csv`'s request: the same admit gate, Policy, caps, formula escaping and ETag.
//! The `q` texts are checked by the broker's own parser (`antares-ql`) before they are joined,
//! so a caller's `q` cannot close the output's group and OR past it.

use crate::app::Gateway;
use crate::auth::subject::admit;
use crate::handlers::files::file_csv;
use crate::handlers::ogc::{filter_query, output_name};
use crate::query;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header::CONTENT_DISPOSITION;
use axum::http::{HeaderValue, Response, Uri};
use axum::response::IntoResponse;
use jc_core::kinds::{OutputFilter, Representation};
use jc_core::ProblemDetails;
use std::sync::Arc;

/// What a caller may ask of an output's table: the Query Entities members the calendar and feed
/// outputs pass through, `type` within the output's types, and `file.csv`'s dialect.
const ASKED: [&str; 14] = [
    "q",
    "scopeQ",
    "georel",
    "geometry",
    "coordinates",
    "geoproperty",
    "id",
    "idPattern",
    "lang",
    "attrs",
    "type",
    "delimiter",
    "bom",
    "humanHeaders",
];

/// The members of a geo-query, which an output that names one keeps as its own.
const GEO: [&str; 4] = ["georel", "geometry", "coordinates", "geoproperty"];

/// The CSV table of the output `rest` names (`{output}.csv` or `{filter}/{output}.csv`, raw).
pub(crate) async fn preset_csv(
    gateway: Arc<Gateway>,
    slug: &str,
    rest: &str,
    mut request: Request,
) -> Response<Body> {
    let (filter, file) = match rest.split('/').collect::<Vec<_>>()[..] {
        [file] => (None, file),
        [filter, file] => (Some(filter), file),
        _ => return ProblemDetails::not_found().into_response(),
    };
    let output = file.strip_suffix(".csv").unwrap_or_default();
    if !output_name(output) {
        return ProblemDetails::not_found().into_response();
    }
    // Admitted before the output is looked up, so a refused caller learns no output's name.
    let endpoint = match admit(&gateway, slug, Some(Representation::Csv), request.headers()) {
        Ok((endpoint, _)) => endpoint,
        Err(problem) => return *problem,
    };
    let preset = endpoint
        .calendars
        .iter()
        .find(|o| o.name == output)
        .map(|o| {
            let types = o.sources.iter().map(|s| s.entity_type.clone());
            (
                types.collect::<Vec<_>>(),
                o.effective_filter().unwrap_or_default(),
            )
        })
        .or_else(|| {
            endpoint.feeds.iter().find(|o| o.name == output).map(|o| {
                let types = o.sources.iter().map(|s| s.entity_type.clone());
                (types.collect(), o.effective_filter())
            })
        });
    let Some((types, base)) = preset else {
        return ProblemDetails::not_found().into_response();
    };
    let asked = match filter {
        Some(filter) => match filter_query(filter, request.uri().query().is_some(), file) {
            Ok(asked) => asked,
            Err(problem) => return problem.into_response(),
        },
        None => request.uri().query().unwrap_or_default().to_owned(),
    };
    let params = match narrowed(query::parse(&asked), &types, &base) {
        Ok(params) => params,
        Err(why) => {
            let problem = ProblemDetails::new(400, "bad-request", "Bad Request");
            return problem.with_detail(why).into_response();
        }
    };
    let encoded: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", query::encode(v)))
        .collect();
    let target = format!("/api/endpoint/{slug}/file.csv?{}", encoded.join("&"));
    match target.parse::<Uri>() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(_) => return ProblemDetails::not_found().into_response(),
    }
    let mut response = file_csv(State(gateway), Path(slug.to_owned()), request).await;
    // The output names the file; its name is `[A-Za-z0-9_-]`, so it needs no quoting.
    if response.headers().contains_key(CONTENT_DISPOSITION) {
        if let Ok(disposition) =
            HeaderValue::from_str(&format!("attachment; filename=\"{output}.csv\""))
        {
            response
                .headers_mut()
                .insert(CONTENT_DISPOSITION, disposition);
        }
    }
    response
}

/// The caller's `asked` parameters under an output of `types` and `base`: anything outside
/// [`ASKED`] refused by name, `type` kept within the output's types, `q` and `scopeQ` AND-ed,
/// a fixed geo-query never replaced. The answer can only narrow the output.
fn narrowed(
    mut asked: Vec<(String, String)>,
    types: &[String],
    base: &OutputFilter,
) -> Result<Vec<(String, String)>, String> {
    if let Some((name, _)) = asked
        .iter()
        .find(|(name, _)| !ASKED.contains(&name.as_str()))
    {
        return Err(format!(
            "\"{name}\" is not a parameter of an output's table; it reads {}",
            ASKED.join(", ")
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if let Some((name, _)) = asked.iter().find(|(name, _)| !seen.insert(name.clone())) {
        return Err(format!("\"{name}\" is given more than once"));
    }
    let take = |asked: &mut Vec<(String, String)>, name: &str| {
        let at = asked.iter().position(|(key, _)| key == name)?;
        Some(asked.remove(at).1)
    };
    let wanted = match take(&mut asked, "type") {
        Some(named) => {
            let named: Vec<String> = named.split(',').map(str::to_owned).collect();
            if let Some(other) = named.iter().find(|t| !types.contains(t)) {
                return Err(format!(
                    "\"{other}\" is no type of this output; it lists {}",
                    types.join(", ")
                ));
            }
            named
        }
        None => types.to_vec(),
    };
    asked.push(("type".to_owned(), wanted.join(",")));
    if base.georel.is_some() && asked.iter().any(|(k, _)| GEO.contains(&k.as_str())) {
        return Err(
            "this output fixes its geo-query: georel, geometry, coordinates and geoproperty are its own"
                .to_owned(),
        );
    }
    if let Some(ours) = base.q.as_deref() {
        let q = match take(&mut asked, "q") {
            Some(theirs) => {
                for (whose, text) in [("this output's", ours), ("the", theirs.as_str())] {
                    antares_ql::parse_q(text).map_err(|_| {
                        format!("{whose} q is no query expression of its own (TS 104 175 4.9): check its parentheses and quotes")
                    })?;
                }
                format!("({ours});({theirs})")
            }
            None => ours.to_owned(),
        };
        asked.push(("q".to_owned(), q));
    }
    if let Some(ours) = base.scope_q.as_deref() {
        let scope = match take(&mut asked, "scopeQ") {
            Some(theirs) => antares_ql::scope::intersect_scope_q(ours, &theirs)
                .ok_or("this output's scopeQ and the request's cannot be joined")?,
            None => ours.to_owned(),
        };
        asked.push(("scopeQ".to_owned(), scope));
    }
    for (key, value) in [
        ("georel", &base.georel),
        ("geometry", &base.geometry),
        ("coordinates", &base.coordinates),
        ("geoproperty", &base.geoproperty),
    ] {
        if let Some(value) = value {
            asked.retain(|(k, _)| k != key);
            asked.push((key.to_owned(), value.clone()));
        }
    }
    Ok(asked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music() -> OutputFilter {
        OutputFilter {
            q: Some("category==\"music\"".to_owned()),
            georel: Some("near;maxDistance==5000".to_owned()),
            geometry: Some("Point".to_owned()),
            coordinates: Some("[19.15,48.73]".to_owned()),
            ..OutputFilter::default()
        }
    }

    fn asked(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn value<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
        query::first(params, name)
    }

    #[test]
    fn a_callers_q_is_and_ed_with_the_outputs() {
        let types = ["Event".to_owned()];
        let params =
            narrowed(asked(&[("q", "category==\"film\"")]), &types, &music()).expect("narrowed");
        assert_eq!(
            value(&params, "q"),
            Some("(category==\"music\");(category==\"film\")")
        );
        assert_eq!(value(&params, "type"), Some("Event"));
        assert_eq!(value(&params, "georel"), Some("near;maxDistance==5000"));
    }

    #[test]
    fn a_q_that_closes_the_outputs_group_is_refused() {
        let types = ["Event".to_owned()];
        let why = narrowed(asked(&[("q", "a==1)|(b==2")]), &types, &music()).expect_err("refused");
        assert!(why.contains("parentheses"), "{why}");
    }

    #[test]
    fn a_fixed_geo_query_and_a_foreign_type_are_refused() {
        let types = ["Event".to_owned()];
        let geo = narrowed(
            asked(&[("georel", "near;maxDistance==1")]),
            &types,
            &music(),
        );
        assert!(geo.expect_err("refused").contains("geo-query"));
        let other = narrowed(asked(&[("type", "Event,Building")]), &types, &music());
        assert!(other.expect_err("refused").contains("Building"));
    }

    #[test]
    fn an_unknown_or_repeated_parameter_is_refused_by_name() {
        let types = ["Event".to_owned()];
        let unknown = narrowed(asked(&[("from", "2026-01-01")]), &types, &music());
        assert!(unknown.expect_err("refused").contains("\"from\""));
        let twice = narrowed(asked(&[("q", "a==1"), ("q", "b==2")]), &types, &music());
        assert!(twice.expect_err("refused").contains("more than once"));
    }

    #[test]
    fn scope_q_is_intersected() {
        let types = ["Event".to_owned()];
        let base = OutputFilter {
            scope_q: Some("/bb/#".to_owned()),
            ..OutputFilter::default()
        };
        let params = narrowed(asked(&[]), &types, &base).expect("narrowed");
        assert_eq!(value(&params, "scopeQ"), Some("/bb/#"));
        assert_eq!(value(&params, "q"), None);
    }
}
