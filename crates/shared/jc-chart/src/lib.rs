//! One chart renderer for both doors that draw one (T-6881): the Portal's `jc_render` and the
//! gateway's `render_chart`. [`spec`] checks the closed Vega-Lite subset and turns flat rows into
//! a [`raster::Chart`]; [`raster`] draws it to a PNG with std and flate2 only. [`view_page`] is
//! the chart view both doors serve as an MCP App (T-6866, T-6882).

pub mod raster;
pub mod spec;
pub mod theme;

/// The chart view's address, on both doors.
pub const VIEW_URI: &str = "ui://joinedcontext/chart-view.html";
/// The chart view page with the chart theme written in (T-6886): it draws the answer's `_meta`
/// points and connects nowhere. The server that serves it writes the origin of its one link over
/// `__PORTAL_ORIGIN__`, or `null` for none.
pub fn view_page() -> &'static str {
    static PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        include_str!("chart_view.html").replace("__JC_THEME__", theme::JSON.trim_end())
    });
    &PAGE
}
