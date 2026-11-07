// SPDX-License-Identifier: EUPL-1.2
//! The `KeyPerformanceIndicator` Entities of one source table of a view as a JSON-stat 2.0
//! dataset (T-4490): `GET /x/views/{view}/jsonstat/v1/{table}?lang=…`, `{table}` the Entities'
//! `dataSet` (a ŠÚ SR `DATAcube` code such as `om7014rr`).
//!
//! The Entities are read with one NGSI-LD Query Entities through the view
//! ([`antares_ops::ViewOps::scan`]), `q=dataSet=="{table}"`, so the view's narrowing, the Policy
//! and the Tenant decide as for NGSI-LD; `lang` means what it means there. A view that is
//! unknown, does not allow Query Entities or does not show the type answers 404, as does a
//! table it holds no Entity of. Read only. The mapping is in `docs/src/jsonstat.md`.
//!
//! ```
//! use antares_ops::Surface;
//! assert_eq!(antares_jsonstat::JsonStat.name(), "jsonstat");
//! assert_eq!(antares_jsonstat::JsonStat.prefix(), "/x/views");
//! ```

#![forbid(unsafe_code)]

pub mod cube;

use antares_ops::percent::encode;
use antares_ops::{
    Caller, OperationId, Ops, Page, Pushdown, Scanned, Surface, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Path, Query, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use serde_json::{json, Value};

/// Where the broker mounts the surface; the view, `jsonstat/v1` and the table follow.
pub const PREFIX: &str = "/x/views";

/// The type a dataset's cells are.
pub const TYPE: &str = "KeyPerformanceIndicator";

/// The metric an Entity left out of a dataset increments (EP-126).
pub const SKIPPED: &str = "antares_surface_records_skipped_total";

/// The header a dataset reports how many Entities it left out in.
pub const SKIPPED_HEADER: &str = "antares-skipped-records";

/// The surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonStat;

impl Surface for JsonStat {
    fn name(&self) -> &'static str {
        "jsonstat"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        json!({
            "endpoint": format!("{PREFIX}/{{view}}/jsonstat/v1/{{table}}"),
            "conformsTo": ["https://json-stat.org/full/"],
        })
    }

    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/jsonstat/v1/{table}",
            any(
                move |caller: Caller,
                      Path((view, table)): Path<(String, String)>,
                      Query(asked): Query<Vec<(String, String)>>,
                      req: Request| {
                    let ops = ops.clone();
                    async move {
                        let refused =
                            || problem(StatusCode::METHOD_NOT_ALLOWED, "this surface is read only");
                        let read = async {
                            serve(&ops, &caller, (&view, &table), &asked)
                                .await
                                .unwrap_or_else(|answer| *answer)
                        };
                        antares_ops::read_only(req.method(), refused, read).await
                    }
                },
            ),
        )
    }
}

/// Table `table` of view `id` in the language `asked` names.
async fn serve(
    ops: &Ops,
    caller: &Caller,
    (id, table): (&str, &str),
    asked: &[(String, String)],
) -> Result<Response, Box<Response>> {
    let not_found = || Box::new(problem(StatusCode::NOT_FOUND, "no such dataset"));
    if !code(table) {
        return Err(not_found());
    }
    let view = ops
        .view(id)
        .filter(|v| v.allows(OperationId::QueryEntities) && shows(v))
        .ok_or_else(not_found)?;
    let lang = lang(asked).map_err(|title| Box::new(problem(StatusCode::BAD_REQUEST, title)))?;
    let mut query = format!("type={TYPE}&q={}", encode(&format!("dataSet==\"{table}\"")));
    if let Some(l) = lang {
        query.push_str("&lang=");
        query.push_str(&encode(l));
    }
    let pushdown = Pushdown {
        query,
        ..Pushdown::default()
    };
    // ponytail: a dataset is one document, so it reads the whole answer, which the scan refuses
    // past RESIDUAL_SCAN Entities; the largest ŠÚ SR tables on dev are split per slice below that.
    let whole = Page {
        start: 0,
        size: RESIDUAL_SCAN,
        whole: true,
    };
    let entities = match view.scan(caller, &pushdown, None, whole).await {
        Ok(Scanned::Page { entities, .. }) => entities,
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => Vec::new(),
        Ok(Scanned::Answer(a)) => return Err(Box::new(a)),
        Err(refusal) => return Err(Box::new(refusal.ngsi_ld())),
    };
    if entities.is_empty() {
        return Err(not_found());
    }
    let (body, skipped) = cube::dataset(table, &entities);
    metrics::counter!(SKIPPED, "surface" => "jsonstat", "reason" => "no-cell")
        .increment(u64::try_from(skipped).unwrap_or(u64::MAX));
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "application/json".to_owned()),
            (HeaderName::from_static(SKIPPED_HEADER), skipped.to_string()),
        ],
        body.to_string(),
    )
        .into_response())
}

/// Whether `table` can be a `dataSet` code: 1 to 64 of `A-Z a-z 0-9 . _ -`, so the `q` it
/// becomes needs no escaping.
fn code(table: &str) -> bool {
    (1..=64).contains(&table.len())
        && table
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The `lang` the query string asks for, or why it is `400`: any other parameter or a
/// repeated one.
fn lang(asked: &[(String, String)]) -> Result<Option<&str>, &'static str> {
    match asked {
        [] => Ok(None),
        [(key, value)] if key == "lang" => Ok(Some(value)),
        _ => Err("the one parameter is lang, given once"),
    }
}

/// Whether `view` shows the type.
fn shows(view: &ViewOps) -> bool {
    view.shows_every_name() || view.shown_names().0.iter().any(|t| t == TYPE)
}

/// A refusal of the surface's own, with a fixed title that never names what a view hides.
fn problem(status: StatusCode, title: &str) -> Response {
    antares_ops::Problem::new(status, title).reply()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_is_a_plain_code_and_lang_the_one_parameter() {
        for ok in ["om7014rr", "a.b_c-1"] {
            assert!(code(ok), "{ok}");
        }
        for bad in ["", "a\"b", "a b", &"x".repeat(65)] {
            assert!(!code(bad), "{bad}");
        }
        assert_eq!(lang(&[]), Ok(None));
        assert_eq!(lang(&[("lang".into(), "en".into())]), Ok(Some("en")));
        assert!(lang(&[("q".into(), "a==1".into())]).is_err());
    }
}
