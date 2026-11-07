// SPDX-License-Identifier: EUPL-1.2
//! The bike-hire docking stations of a view as a GBFS 3.0 feed (T-4488):
//! `/x/views/{view}/gbfs/v1/{name}/{gbfs|system_information|station_information|station_status}.json`.
//!
//! A view serves a feed only for an output its `representations.gbfs` lists: the output names
//! the system (`title`, `language`, `timezone`, `feedContactEmail`, `openingHours`, `ttl`),
//! which no Entity carries. The stations are the view's `BikeHireDockingStation` Entities (SDM),
//! read with one NGSI-LD Query Entities through the view ([`antares_ops::ViewOps::scan`]), so
//! the view's narrowing, the Policy and the Tenant decide as for NGSI-LD. A view that is
//! unknown, does not allow Query Entities, does not show the type or lists no such output
//! answers the same 404. Read only. The mapping is in `docs/src/gbfs.md`.
//!
//! ```
//! use antares_ops::Surface;
//! assert_eq!(antares_gbfs::Gbfs.name(), "gbfs");
//! assert_eq!(antares_gbfs::Gbfs.prefix(), "/x/views");
//! ```

#![forbid(unsafe_code)]

mod map;

use antares_ops::percent::encode;
use antares_ops::{
    Caller, OperationId, Ops, Page, Pushdown, Scanned, Surface, ViewOps, RESIDUAL_SCAN,
};
use axum::extract::{Path, Request};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use map::System;
use serde_json::{json, Value};

/// Where the broker mounts the surface; the view, `gbfs/v1`, the output and the file follow.
pub const PREFIX: &str = "/x/views";

/// The SDM type a feed's stations are.
pub const TYPE: &str = "BikeHireDockingStation";

/// The metric a station left out of a file increments (EP-126).
pub const SKIPPED: &str = "antares_surface_records_skipped_total";

/// The header a file reports how many stations it left out in.
pub const SKIPPED_HEADER: &str = "antares-skipped-records";

/// The files of a feed, in `gbfs.json` order after `gbfs` itself.
pub const FILES: [&str; 4] = [
    "gbfs",
    "system_information",
    "station_information",
    "station_status",
];

/// The surface.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gbfs;

impl Surface for Gbfs {
    fn name(&self) -> &'static str {
        "gbfs"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> Value {
        json!({
            "endpoint": format!("{PREFIX}/{{view}}/gbfs/v1/{{name}}/gbfs.json"),
            "conformsTo": ["https://github.com/MobilityData/gbfs/blob/v3.0/gbfs.md"],
        })
    }

    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/gbfs/v1/{name}/{file}",
            any(
                move |caller: Caller,
                      Path((view, name, file)): Path<(String, String, String)>,
                      req: Request| {
                    let ops = ops.clone();
                    async move {
                        let refused =
                            || problem(StatusCode::METHOD_NOT_ALLOWED, "this surface is read only");
                        let base = antares_ops::public_base(req.headers());
                        let read = async {
                            let url = format!(
                                "{base}{PREFIX}/{}/gbfs/v1/{}",
                                encode(&view),
                                encode(&name)
                            );
                            serve(&ops, &caller, (&view, &name, &file), &url)
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

/// One file of output `name` of view `id`, its feed's files under `url`.
async fn serve(
    ops: &Ops,
    caller: &Caller,
    (id, name, file): (&str, &str, &str),
    url: &str,
) -> Result<Response, Box<Response>> {
    let not_found = || Box::new(problem(StatusCode::NOT_FOUND, "no such feed"));
    let file = file
        .strip_suffix(".json")
        .filter(|f| FILES.contains(f))
        .ok_or_else(not_found)?;
    let view = ops
        .view(id)
        .filter(|v| v.allows(OperationId::QueryEntities) && shows(v))
        .ok_or_else(not_found)?;
    let system = System::of(view.representation("gbfs"), name)
        .map_err(|_| {
            Box::new(problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the view's gbfs outputs do not hold; its Endpoint must be published again",
            ))
        })?
        .ok_or_else(not_found)?;
    let stations = stations(&view, caller).await?;
    let (body, skipped) = map::file(file, &system, id, &stations, url);
    metrics::counter!(SKIPPED, "surface" => "gbfs", "reason" => "unmapped")
        .increment(u64::try_from(skipped).unwrap_or(u64::MAX));
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "application/json".to_owned()),
            // `private`: the Policy decides per caller, so no shared cache may keep a file
            (CACHE_CONTROL, format!("private, max-age={}", system.ttl)),
            (HeaderName::from_static(SKIPPED_HEADER), skipped.to_string()),
        ],
        body.to_string(),
    )
        .into_response())
}

/// Whether `view` shows the stations.
fn shows(view: &ViewOps) -> bool {
    view.shows_every_name() || view.shown_names().0.iter().any(|t| t == TYPE)
}

/// Every station the caller reads through `view`.
async fn stations(view: &ViewOps, caller: &Caller) -> Result<Vec<Value>, Box<Response>> {
    let pushdown = Pushdown {
        query: format!("type={TYPE}"),
        ..Pushdown::default()
    };
    // ponytail: a feed is one file, so it reads the whole answer, which the scan refuses past
    // RESIDUAL_SCAN Entities; no city has that many stations.
    let whole = Page {
        start: 0,
        size: RESIDUAL_SCAN,
        whole: true,
    };
    match view.scan(caller, &pushdown, None, whole).await {
        Ok(Scanned::Page { entities, .. }) => Ok(entities),
        // A Tenant that holds nothing yet answers 404 (5.5.10): a feed without stations.
        Ok(Scanned::Answer(a)) if a.status() == StatusCode::NOT_FOUND => Ok(Vec::new()),
        Ok(Scanned::Answer(a)) => Err(Box::new(a)),
        Err(refusal) => Err(Box::new(refusal.ngsi_ld())),
    }
}

/// A refusal of the surface's own, with a fixed title that never names what a view hides.
fn problem(status: StatusCode, title: &str) -> Response {
    antares_ops::Problem::new(status, title).reply()
}
