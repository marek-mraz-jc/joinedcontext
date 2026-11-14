//! One chart renderer for both doors that draw one (T-6881): the Portal's `jc_render` and the
//! gateway's `render_chart`. [`spec`] checks the closed Vega-Lite subset and turns flat rows into
//! a [`raster::Chart`]; [`raster`] draws it to a PNG with std and flate2 only. [`VIEW_PAGE`] is
//! the chart view both doors serve as an MCP App (T-6866, T-6882).

pub mod raster;
pub mod spec;

/// The chart view's address, on both doors.
pub const VIEW_URI: &str = "ui://joinedcontext/chart-view.html";
/// The chart view page: it draws the answer's `_meta` points and connects nowhere. The server
/// that serves it writes the origin of its one link over `__PORTAL_ORIGIN__`, or `null` for none.
pub const VIEW_PAGE: &str = include_str!("chart_view.html");
