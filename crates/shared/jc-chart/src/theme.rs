//! The one chart theme (T-6886): `theme.json`, a Vega-Lite `config` for the light and the dark
//! page. Every chart surface reads this file: the PNG raster here, the chart view (written into
//! the page where it is served), the Portal's Vega charts (a build import), the viz frame
//! (`vendor/viz/site/theme.js`, held equal by a test) and the public assistant through that frame.
//! It holds constants only: colours, sizes, a font stack.

use std::sync::LazyLock;

/// The theme file as shipped.
pub const JSON: &str = include_str!("theme.json");

/// What the raster draws with, read from the light config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub background: [u8; 3],
    /// Titles and axis titles.
    pub text: [u8; 3],
    /// Tick labels.
    pub muted: [u8; 3],
    pub grid: [u8; 3],
    /// Axis lines and ticks.
    pub domain: [u8; 3],
    /// `range.category`, in order.
    pub category: Vec<[u8; 3]>,
}

fn rgb(hex: Option<&str>) -> Option<[u8; 3]> {
    let hex = hex?.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn look_of(config: &serde_json::Value) -> Option<Look> {
    let at = |pointer: &str| rgb(config.pointer(pointer)?.as_str());
    Some(Look {
        background: at("/background")?,
        text: at("/axis/titleColor")?,
        muted: at("/axis/labelColor")?,
        grid: at("/axis/gridColor")?,
        domain: at("/axis/domainColor")?,
        category: config
            .pointer("/range/category")?
            .as_array()?
            .iter()
            .map(|c| rgb(c.as_str()))
            .collect::<Option<Vec<_>>>()?,
    })
}

/// The parsed file; the tests hold it to parse, so the empty fallback is never drawn with.
static THEME: LazyLock<serde_json::Value> =
    LazyLock::new(|| serde_json::from_str(JSON).unwrap_or_default());

static LIGHT: LazyLock<Look> = LazyLock::new(|| {
    THEME
        .get("light")
        .and_then(look_of)
        .unwrap_or(Look {
            background: [255; 3],
            text: [0; 3],
            muted: [0; 3],
            grid: [200; 3],
            domain: [0; 3],
            category: vec![[0; 3]],
        })
});

/// The light look, which the PNG is drawn in.
pub fn light() -> &'static Look {
    &LIGHT
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn theme() -> Value {
        serde_json::from_str(JSON).expect("theme.json is JSON")
    }

    fn luminance(c: [u8; 3]) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    /// WCAG 2.2 contrast ratio.
    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// CIE L*, the lightness a grey print keeps of a colour.
    fn lightness(c: [u8; 3]) -> f64 {
        let y = luminance(c);
        let f = if y > 0.008_856 {
            y.cbrt()
        } else {
            7.787 * y + 16.0 / 116.0
        };
        116.0 * f - 16.0
    }

    /// The number of `#rrggbb` colours in `line`.
    fn hex_colours(line: &str) -> usize {
        line.match_indices('#')
            .filter(|(at, _)| {
                let rest = &line.as_bytes()[at + 1..];
                rest.len() >= 6
                    && rest[..6].iter().all(u8::is_ascii_hexdigit)
                    && rest.get(6).is_none_or(|b| !b.is_ascii_alphanumeric())
            })
            .count()
    }

    /// No second palette (T-6886): none of the chart surfaces holds a list of colours of its
    /// own; each reads `theme.json`.
    #[test]
    fn the_theme_file_is_the_only_palette() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for path in [
            "crates/shared/jc-chart/src/raster.rs",
            "crates/shared/jc-chart/src/chart_view.html",
            "vendor/joinedcontext-platform/crates/assistant/src/chat/viz.rs",
            "crates/portal/ui/src/components/panels/specs.ts",
            "crates/portal/ui/src/components/panels/VegaChart.tsx",
            "crates/portal/ui/src/components/panels/palette.ts",
            "crates/portal/ui/src/components/dashboards/VegaLiteWidget.tsx",
            "crates/portal/ui/src/pages/knowledge/AssistantChat.tsx",
            "vendor/viz/site/render.js",
            "vendor/viz/site/render.css",
        ] {
            let text = std::fs::read_to_string(root.join(path)).expect(path);
            for (n, line) in text.lines().enumerate() {
                assert!(hex_colours(line) < 3, "{path}:{}: a colour list: {line}", n + 1);
            }
        }
        assert_eq!(hex_colours("[\"#1d4ed8\", \"#d97706\", \"#7e22ce\"]"), 3);
    }

    #[test]
    fn the_png_palette_is_the_theme_category_range() {
        let range: Vec<[u8; 3]> = theme()["light"]["range"]["category"]
            .as_array()
            .expect("range.category")
            .iter()
            .map(|c| rgb(c.as_str()).expect("a #rrggbb colour"))
            .collect();
        assert_eq!(light().category, range);
        assert_eq!(crate::raster::palette(), range.as_slice());
    }

    /// Text and axis labels read at 4.5:1 (WCAG 1.4.3), axis lines and marks at 3:1 (1.4.11),
    /// and neighbouring categories stay apart in a grey print: at least 10 L* between each and
    /// the next, the last and the first included.
    #[test]
    fn the_theme_reads_in_contrast_and_in_grey() {
        for mode in ["light", "dark"] {
            let config = &theme()[mode];
            let look = look_of(config).expect(mode);
            let title = rgb(config["title"]["color"].as_str()).expect("title colour");
            for (what, colour) in [
                ("title", title),
                ("text", look.text),
                ("labels", look.muted),
            ] {
                let ratio = contrast(colour, look.background);
                assert!(ratio >= 4.5, "{mode} {what}: {ratio:.2}:1");
            }
            assert!(contrast(look.domain, look.background) >= 3.0, "{mode} axis");
            for (i, c) in look.category.iter().enumerate() {
                let ratio = contrast(*c, look.background);
                assert!(ratio >= 3.0, "{mode} category {i}: {ratio:.2}:1");
                let next = look.category[(i + 1) % look.category.len()];
                let gap = (lightness(*c) - lightness(next)).abs();
                assert!(gap >= 10.0, "{mode} categories {i} and next: {gap:.1} L*");
            }
        }
    }

    /// A theme holds constants: no member that names an expression, a signal, a parameter, an
    /// address or an image, and each text is a colour, a font stack or a keyword.
    #[test]
    fn the_theme_holds_constants_only() {
        fn walk(value: &Value, at: &str) {
            match value {
                Value::Object(map) => {
                    for (key, inner) in map {
                        let lower = key.to_ascii_lowercase();
                        for word in ["expr", "signal", "param", "url", "href", "image", "data"] {
                            assert!(!lower.contains(word), "{at}/{key}");
                        }
                        walk(inner, &format!("{at}/{key}"));
                    }
                }
                Value::Array(items) => items.iter().for_each(|v| walk(v, at)),
                Value::String(text) => {
                    let colour = rgb(Some(text)).is_some();
                    let font = at.ends_with("/font")
                        && text
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || " ,-\"".contains(c));
                    let keyword = ["start", "square"].contains(&text.as_str());
                    assert!(colour || font || keyword, "{at}: {text}");
                }
                Value::Number(_) | Value::Bool(_) | Value::Null => {}
            }
        }
        let theme = theme();
        let mut modes: Vec<&String> = theme.as_object().expect("an object").keys().collect();
        modes.sort();
        assert_eq!(modes, ["dark", "light"]);
        walk(&theme, "");
    }
}
