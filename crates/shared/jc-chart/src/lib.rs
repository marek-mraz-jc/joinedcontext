//! One chart renderer for both doors that draw one (T-6881): the Portal's `jc_render` and the
//! gateway's `render_chart`. [`spec`] checks the closed Vega-Lite subset and turns flat rows into
//! a [`raster::Chart`]; [`raster`] draws it to a PNG with std and flate2 only. [`VIEW_PAGE`] is
//! the chart view both doors serve as an MCP App (T-6866, T-6882).

pub mod raster;
pub mod spec;
