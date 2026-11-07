// SPDX-License-Identifier: EUPL-1.2
//! The live `Vehicle` Entities of a view as a GTFS Realtime 2.0 `VehiclePositions` feed
//! (T-4489): `/x/views/{view}/gtfsrt/v1/vehicle-positions.pb` (protobuf) and
//! `/x/views/{view}/gtfsrt/v1/vehicle-positions.json` (the same message in its JSON form).
//!
//! The vehicles are read with one NGSI-LD Query Entities through the view
//! ([`antares_ops::ViewOps::scan`]), so the view's narrowing, the Policy and the Tenant decide
//! as for NGSI-LD. A position older than the freshness window (`maxAge`, seconds, default
//! [`MAX_AGE`]) is dropped and counted, as is a vehicle without a point or a time. A view that
//! is unknown, does not allow Query Entities or does not show `Vehicle` answers the same 404.
//! Read only. The mapping is in `docs/src/gtfsrt.md`.
//!
//! ```
//! use antares_ops::Surface;
//! assert_eq!(antares_gtfsrt::GtfsRt.name(), "gtfsrt");
//! assert_eq!(antares_gtfsrt::GtfsRt.prefix(), "/x/views");
//! ```

#![forbid(unsafe_code)]

pub mod feed;

use antares_ops::{
    Caller, OperationId, Ops, Page, Pushdown, Scanned, Surface, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Path, Query, Request};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use chrono::Utc;
use prost::Message as _;
use serde_json::{json, Value};

/// Where the broker mounts the surface; the view, `gtfsrt/v1` and the file follow.
pub const PREFIX: &str = "/x/views";

/// The SDM type a feed's vehicles are.
pub const TYPE: &str = "Vehicle";

/// The freshness window when the request names none, in seconds.
pub const MAX_AGE: u32 = 300;

/// The widest freshness window a request may ask for, in seconds: a day.
pub const MAX_AGE_LIMIT: u32 = 86_400;

/// The metric a vehicle left out of a feed increments (EP-126).
pub const SKIPPED: &str = "antares_surface_records_skipped_total";

/// The header a feed reports how many vehicles it left out in.
pub const SKIPPED_HEADER: &str = "antares-skipped-records";

/// The surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct GtfsRt;

impl Surface for GtfsRt {
    fn name(&self) -> &'static str {
        "gtfsrt"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        json!({
            "endpoint": format!("{PREFIX}/{{view}}/gtfsrt/v1/vehicle-positions.{{pb|json}}"),
            "conformsTo": ["https://gtfs.org/documentation/realtime/reference/"],
        })
    }

    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/gtfsrt/v1/{file}",
            any(
                move |caller: Caller,
                      Path((view, file)): Path<(String, String)>,
                      Query(asked): Query<Vec<(String, String)>>,
                      req: Request| {
                    let ops = ops.clone();
                    async move {
                        let refused =
                            || problem(StatusCode::METHOD_NOT_ALLOWED, "this surface is read only");
                        let read = async {
                            serve(&ops, &caller, (&view, &file), &asked)
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

/// The feed of view `id` as `file`, under the window `asked` names.
async fn serve(
    ops: &Ops,
    caller: &Caller,
    (id, file): (&str, &str),
    asked: &[(String, String)],
) -> Result<Response, Box<Response>> {
    let not_found = || Box::new(problem(StatusCode::NOT_FOUND, "no such feed"));
    let protobuf = match file {
        "vehicle-positions.pb" => true,
        "vehicle-positions.json" => false,
        _ => return Err(not_found()),
    };
    let view = ops
        .view(id)
        .filter(|v| v.allows(OperationId::QueryEntities) && shows(v))
        .ok_or_else(not_found)?;
    let window =
        max_age(asked).map_err(|title| Box::new(problem(StatusCode::BAD_REQUEST, title)))?;
    let vehicles = vehicles(&view, caller).await?;
    let (message, skipped) = feed::message(&vehicles, Utc::now(), window);
    metrics::counter!(SKIPPED, "surface" => "gtfsrt", "reason" => "stale-or-unplaced")
        .increment(u64::try_from(skipped).unwrap_or(u64::MAX));
    let (kind, body) = if protobuf {
        ("application/x-protobuf", message.encode_to_vec())
    } else {
        (
            "application/json",
            feed::json(&message).to_string().into_bytes(),
        )
    };
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, kind.to_owned()),
            // `private`: the Policy decides per caller, so no shared cache may keep a feed
            (CACHE_CONTROL, "private, no-cache".to_owned()),
            (HeaderName::from_static(SKIPPED_HEADER), skipped.to_string()),
        ],
        body,
    )
        .into_response())
}

/// The freshness window the query string asks for, or why it is `400`: any other parameter,
/// a repeated one, or a `maxAge` outside 1 to [`MAX_AGE_LIMIT`].
fn max_age(asked: &[(String, String)]) -> Result<u32, &'static str> {
    match asked {
        [] => Ok(MAX_AGE),
        [(key, value)] if key == "maxAge" => value
            .parse::<u32>()
            .ok()
            .filter(|s| (1..=MAX_AGE_LIMIT).contains(s))
            .ok_or("maxAge is a whole number of seconds, 1 to 86400"),
        _ => Err("the one parameter is maxAge, given once"),
    }
}

/// Whether `view` shows the vehicles.
fn shows(view: &ViewOps) -> bool {
    view.shows_every_name() || view.shown_names().0.iter().any(|t| t == TYPE)
}

/// Every vehicle the caller reads through `view`.
async fn vehicles(view: &ViewOps, caller: &Caller) -> Result<Vec<Value>, Box<Response>> {
    let pushdown = Pushdown {
        query: format!("type={TYPE}"),
        ..Pushdown::default()
    };
    // ponytail: a feed is one message, so it reads the whole answer, which the scan refuses
    // past RESIDUAL_SCAN Entities; HSL runs about 1 500 vehicles at the peak.
    let whole = Page {
        start: 0,
        size: RESIDUAL_SCAN,
        whole: true,
    };
    match view.scan(caller, &pushdown, None, whole).await {
        Ok(Scanned::Page { entities, .. }) => Ok(entities),
        // A Tenant that holds nothing yet answers 404 (5.5.10): a feed without vehicles.
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => Ok(Vec::new()),
        Ok(Scanned::Answer(a)) => Err(Box::new(a)),
        Err(refusal) => Err(Box::new(refusal.ngsi_ld())),
    }
}

/// A refusal of the surface's own, with a fixed title that never names what a view hides.
fn problem(status: StatusCode, title: &str) -> Response {
    antares_ops::Problem::new(status, title).reply()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_window_is_max_age_alone_within_a_day() {
        assert_eq!(max_age(&[]), Ok(MAX_AGE));
        assert_eq!(max_age(&pairs(&[("maxAge", "60")])), Ok(60));
        for bad in [
            pairs(&[("maxAge", "0")]),
            pairs(&[("maxAge", "86401")]),
            pairs(&[("maxAge", "x")]),
            pairs(&[("limit", "1")]),
            pairs(&[("maxAge", "1"), ("maxAge", "2")]),
        ] {
            assert!(max_age(&bad).is_err(), "{bad:?}");
        }
    }
}
