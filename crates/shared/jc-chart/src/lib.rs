//! One chart renderer for both doors that draw one (T-6881): the Portal's `jc_render` and the
//! gateway's `render_chart`. [`spec`] checks the closed Vega-Lite subset and turns flat rows into
//! a [`raster::Chart`]; [`raster`] draws it to a PNG with std and flate2 only. [`view_page`] is
//! the chart view both doors serve as an MCP App (T-6866, T-6882).

pub mod raster;
pub mod spec;
pub mod theme;

/// The chart view's address, on both doors.
pub const VIEW_URI: &str = "ui://joinedcontext/chart-view.html";
/// The chart view's own code, before the theme and the library are written in: what the sink
/// tests read (T-6887), since the library itself holds `new Function` and `innerHTML`.
pub const VIEW_SOURCE: &str = include_str!("chart_view.html");
/// The chart library the view inlines (T-6887): each file as vendor/viz/vendor ships it, held to
/// its SHA256SUMS line by build.rs. The placeholder in [`VIEW_SOURCE`], the file name, its bytes.
pub const LIBRARY: [(&str, &str, &str); 3] = [
    (
        "__JC_VEGA__",
        "vega-6.4.0.min.js",
        include_str!("../../../../vendor/viz/vendor/vega-6.4.0.min.js"),
    ),
    (
        "__JC_VEGA_LITE__",
        "vega-lite-6.5.0.min.js",
        include_str!("../../../../vendor/viz/vendor/vega-lite-6.5.0.min.js"),
    ),
    (
        "__JC_VEGA_INTERPRETER__",
        "vega-interpreter-2.3.2.js",
        include_str!("../../../../vendor/viz/vendor/vega-interpreter-2.3.2.js"),
    ),
];

/// The chart view page with the chart theme (T-6886) and the chart library (T-6887) written in:
/// it draws the answer's `_meta` points and connects nowhere. The server that serves it writes
/// the origin of its one link over `__PORTAL_ORIGIN__`, or `null` for none.
pub fn view_page() -> &'static str {
    static PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        // The theme goes in first: a library file is never searched for a placeholder.
        let page = VIEW_SOURCE.replace("__JC_THEME__", theme::JSON.trim_end());
        LIBRARY
            .iter()
            .fold(page, |page, (mark, _, code)| page.replacen(mark, code, 1))
    });
    &PAGE
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn the_chart_view_inlines_the_checksummed_libraries() {
        let vendor =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../vendor/viz/vendor");
        let sums = std::fs::read_to_string(vendor.join("SHA256SUMS")).expect("SHA256SUMS");
        let page = view_page();
        for (mark, file, code) in LIBRARY {
            let bytes = std::fs::read(vendor.join(file)).expect(file);
            assert_eq!(code.as_bytes(), bytes, "{file} is inlined as vendored");
            let hex: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert!(
                sums.lines().any(|l| l == format!("{hex}  {file}")),
                "{file} has its SHA256SUMS line"
            );
            // Inlined whole, once, inside its own script element, which nothing in it closes early.
            assert_eq!(page.matches(code).count(), 1, "{file} once");
            assert!(
                page.contains(&format!("<script>{code}</script>")),
                "{file} in its own script"
            );
            let lower = code.to_ascii_lowercase();
            assert!(
                !lower.contains("</script") && !lower.contains("<!--"),
                "{file} closes no script"
            );
            assert!(
                !code.contains("__PORTAL_ORIGIN__") && !code.contains("__JC_"),
                "{file} holds no placeholder"
            );
            assert!(!page.contains(mark), "{mark} written");
        }
        assert!(!page.contains("__JC_THEME__"));
        assert!(
            LIBRARY
                .iter()
                .all(|(_, file, _)| !file.starts_with("vega-embed")),
            "no vega-embed: it writes innerHTML"
        );
        // The Portal's MCP answer cap is 4 MiB (crates/portal/src/mcp/mod.rs).
        assert!(page.len() < 1_000_000, "the page is {} bytes", page.len());
    }
}
