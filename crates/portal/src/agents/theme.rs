//! Every generated App's own look, inside the organization's branding (AP-123, T-2779).
//!
//! A run starts from the template, whose `src/design-tokens.json` is the same file for every App,
//! so two Apps of one project looked alike unless the model happened to restyle one. The look is
//! a parameter instead: an accent drawn from one of the brand's three colours and turned by at
//! most 24° of hue, a corner radius set and a density, 135 combinations in all, and each App of
//! a project takes one no other App of it holds. Every colour is then pushed until it reads:
//! 4.5:1 for text on the surface and the card (WCAG 1.4.3), 3:1 for a chart colour on the surface
//! (WCAG 1.4.11). The file holds hex colours and rem or px lengths only, built here from numbers,
//! so nothing a branding file says reaches the page as CSS (OPS-46).

use crate::branding::{contrast, from_oklab, luminance, rgb_of, to_oklab, Branding};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Where the look lives in an application (SDK-25).
pub const PATH: &str = "src/design-tokens.json";

/// What text needs against its background (WCAG 1.4.3), with a hair of margin: the browser
/// rounds the colour the file names, and 4.5 exactly can land at 4.49.
const TEXT: f32 = 4.6;
/// What a chart mark needs against the surface it is drawn on (WCAG 1.4.11).
const GRAPHIC: f32 = 3.1;

const HUE_SHIFTS: [f64; 5] = [0.0, -24.0, 24.0, -12.0, 12.0];
/// Corner radii, small to large, in px: sharp, soft, round.
const RADII: [[u8; 3]; 3] = [[2, 3, 4], [4, 6, 10], [8, 12, 18]];
/// The spacing scale `1 2 3 4 6 8` in rem: compact, regular, airy.
const SPACES: [[f32; 6]; 3] = [
    [0.2, 0.4, 0.6, 0.8, 1.2, 1.6],
    [0.25, 0.5, 0.75, 1.0, 1.5, 2.0],
    [0.3, 0.6, 0.9, 1.25, 1.75, 2.5],
];
/// The type scale `sm md lg xl` in rem, by the same density.
const SIZES: [[f32; 4]; 3] = [
    [0.75, 0.8125, 0.9375, 1.125],
    [0.75, 0.875, 1.0, 1.25],
    [0.8125, 0.9375, 1.0625, 1.375],
];
const LOOKS: usize = 3 * HUE_SHIFTS.len() * RADII.len() * SPACES.len();

/// One of the [`LOOKS`] combinations: which brand colour, how far its hue turns, which radii,
/// which density.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    brand: usize,
    shift: usize,
    radius: usize,
    density: usize,
}

impl Look {
    fn of(index: usize) -> Self {
        let index = index % LOOKS;
        Self {
            brand: index % 3,
            shift: (index / 3) % HUE_SHIFTS.len(),
            radius: (index / (3 * HUE_SHIFTS.len())) % RADII.len(),
            density: index / (3 * HUE_SHIFTS.len() * RADII.len()),
        }
    }
}

/// Where a name's look search starts: the same name starts at the same place on every run.
fn start_of(name: &str) -> usize {
    let digest = Sha256::digest(name.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    (u64::from_le_bytes(first) % LOOKS as u64) as usize
}

/// The look of `app` among the project's `others`: the Apps are placed in name order, each at
/// the first look from its own starting point that no earlier one holds, so every name of the
/// project is placed the same way on every run.
///
/// ponytail: the others' looks are recomputed from their names, not read from their repositories;
/// an App whose person restyled it, or whose look moved because a name sorting before it was
/// added later, can share a look with a new one. Reading each App's tokens from the forge is the
/// upgrade when that matters.
fn look_for(app: &str, others: &[String]) -> Look {
    let mut names: Vec<&str> = others
        .iter()
        .map(String::as_str)
        .filter(|n| *n != app)
        .collect();
    names.push(app);
    names.sort_unstable();
    names.dedup();
    let mut taken = Vec::with_capacity(names.len());
    for name in names {
        let start = start_of(name);
        // More Apps than looks: the rest start over, sharing looks as little as they can.
        let free = (0..LOOKS)
            .map(|step| (start + step) % LOOKS)
            .find(|index| !taken.contains(index) || taken.len() >= LOOKS)
            .unwrap_or(start);
        if name == app {
            return Look::of(free);
        }
        taken.push(free);
    }
    Look::of(start_of(app))
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn lab_of(colour: &str) -> [f64; 3] {
    to_oklab(rgb_of(colour).unwrap_or([15, 23, 42]))
}

fn ratio(a: &str, b: &str) -> f32 {
    contrast(luminance(a).unwrap_or(0.0), luminance(b).unwrap_or(1.0))
}

/// The colour turned by `degrees` of hue in OkLCh, its lightness and chroma kept.
fn turned(colour: &str, degrees: f64) -> String {
    let [l, a, b] = lab_of(colour);
    let (chroma, hue) = (a.hypot(b), b.atan2(a) + degrees.to_radians());
    hex(from_oklab([l, chroma * hue.cos(), chroma * hue.sin()]))
}

/// `share` of `colour` mixed into `base` in Oklab, the space `color-mix(in oklab, …)` uses.
fn mixed(colour: &str, base: &str, share: f64) -> String {
    let (c, b) = (lab_of(colour), lab_of(base));
    hex(from_oklab(
        [0, 1, 2].map(|i| c[i] * share + b[i] * (1.0 - share)),
    ))
}

/// The colour darkened in Oklab until it reaches `least` against every one of `on`, or nearly
/// black when nothing lighter does. Every surface here is light, so darker always helps.
fn readable(colour: &str, on: &[&str], least: f32) -> String {
    let [mut l, a, b] = lab_of(colour);
    let mut out = colour.to_ascii_lowercase();
    for _ in 0..60 {
        out = hex(from_oklab([l, a, b]));
        if on.iter().all(|surface| ratio(&out, surface) >= least) {
            return out;
        }
        l -= 0.02;
        if l <= 0.05 {
            break;
        }
    }
    out
}

/// The generated `src/design-tokens.json` of `app` among the project's `others` (AP-123).
pub fn tokens(branding: &Branding, app: &str, others: &[String]) -> Value {
    tokens_of(branding, look_for(app, others))
}

fn tokens_of(branding: &Branding, look: Look) -> Value {
    let colours = &branding.colours;
    // An App is a light page: the brand's background when it is light, white otherwise.
    // Both written back as `#rrggbb`, whatever form the branding file used.
    let surface = match rgb_of(&colours.background) {
        Some(rgb) if luminance(&colours.background).is_some_and(|l| l >= 0.8) => hex(rgb),
        _ => "#ffffff".to_owned(),
    };
    let ink = match rgb_of(&colours.text) {
        Some(rgb) if ratio(&colours.text, &surface) >= 7.0 => hex(rgb),
        _ => "#0f172a".to_owned(),
    };
    let brand = [&colours.primary, &colours.secondary, &colours.accent];
    let shift = HUE_SHIFTS[look.shift];
    let base = turned(brand[look.brand], shift);
    let card = mixed(&base, &surface, 0.04);
    let accent = readable(&base, &[&surface, &card], TEXT);
    let line = mixed(&ink, &surface, 0.14);
    // Muted text: half way to the surface, darkened back as far as the card needs.
    let muted = readable(&mixed(&ink, &surface, 0.55), &[&surface, &card], TEXT);
    let status = |colour: &str| readable(colour, &[&surface, &card], TEXT);

    // The accent first, the brand's other two colours next, then turns of the accent: each drawn
    // dark enough to stand on the surface.
    let mut palette = vec![accent.clone()];
    for (index, colour) in brand.iter().enumerate() {
        if index != look.brand {
            palette.push(readable(&turned(colour, shift), &[&surface], GRAPHIC));
        }
    }
    for degrees in [60.0, 120.0, 180.0, 240.0, 300.0] {
        palette.push(readable(&turned(&accent, degrees), &[&surface], GRAPHIC));
    }

    let radius = RADII[look.radius];
    let space = SPACES[look.density];
    let size = SIZES[look.density];
    let rem = |value: f32| format!("{}rem", (value * 10_000.0).round() / 10_000.0);
    json!({
        "color": {
            "accent": accent,
            "ink": ink,
            "muted": muted,
            "surface": surface,
            "card": card,
            "line": line,
            "danger": status("#dc2626"),
            "success": status("#059669"),
            "warning": status("#d97706"),
        },
        "font": {
            "body": "\"Inter\", system-ui, -apple-system, \"Segoe UI\", sans-serif",
            "mono": "ui-monospace, monospace",
            "size": { "sm": rem(size[0]), "md": rem(size[1]), "lg": rem(size[2]), "xl": rem(size[3]) },
        },
        "space": {
            "1": rem(space[0]), "2": rem(space[1]), "3": rem(space[2]),
            "4": rem(space[3]), "6": rem(space[4]), "8": rem(space[5]),
        },
        "radius": {
            "sm": format!("{}px", radius[0]),
            "md": format!("{}px", radius[1]),
            "lg": format!("{}px", radius[2]),
        },
        "chart": { "palette": palette },
        "map": {
            "point": accent,
            "selected": palette[1],
            "low": palette[2],
            "high": palette[3],
            "stroke": surface,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branding::Colours;

    fn branding(
        primary: &str,
        secondary: &str,
        accent: &str,
        background: &str,
        text: &str,
    ) -> Branding {
        Branding {
            colours: Colours {
                primary: primary.into(),
                secondary: secondary.into(),
                accent: accent.into(),
                background: background.into(),
                text: text.into(),
            },
            ..Branding::default()
        }
    }

    /// The default branding, a pale yellow brand, a dark page background, and a text colour too
    /// light to read: the generator has to make each of them readable.
    fn brandings() -> Vec<Branding> {
        vec![
            Branding::default(),
            branding("#ffe977", "#fff3b0", "#f59e0b", "#ffffff", "#0f172a"),
            branding("#0b3d91", "#e30613", "#00a3e0", "#0b0f19", "#e2e8f0"),
            branding("#22c55e", "#a3e635", "#14b8a6", "#fafaf9", "#94a3b8"),
            branding("#fc0", "#0af", "#f0a", "#FFF", "#000"),
        ]
    }

    fn colour(tokens: &Value, path: &str) -> String {
        tokens
            .pointer(path)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{path} in {tokens}"))
            .to_owned()
    }

    #[test]
    fn every_look_of_every_branding_reads_at_aa() {
        for branding in brandings() {
            for index in 0..LOOKS {
                let name = format!("look {index}");
                let tokens = tokens_of(&branding, Look::of(index));
                let surface = colour(&tokens, "/color/surface");
                let card = colour(&tokens, "/color/card");
                for text in ["ink", "muted", "accent", "danger", "success", "warning"] {
                    let value = colour(&tokens, &format!("/color/{text}"));
                    for on in [&surface, &card] {
                        let got = ratio(&value, on);
                        assert!(
                            got >= 4.5,
                            "{text} {value} on {on}: {got:.2} for {name} of {:?}",
                            branding.colours
                        );
                    }
                }
                for mark in tokens["chart"]["palette"].as_array().expect("a palette") {
                    let mark = mark.as_str().expect("a colour");
                    let got = ratio(mark, &surface);
                    assert!(got >= 3.0, "chart {mark} on {surface}: {got:.2} for {name}");
                }
            }
        }
    }

    #[test]
    fn every_look_is_reached_and_each_is_a_different_file() {
        let mut seen = std::collections::HashSet::new();
        for index in 0..LOOKS {
            let look = Look::of(index);
            assert!(look.brand < 3 && look.shift < 5 && look.radius < 3 && look.density < 3);
            seen.insert((look.brand, look.shift, look.radius, look.density));
        }
        assert_eq!(seen.len(), LOOKS);
    }

    #[test]
    fn the_apps_of_a_project_get_different_looks_and_the_same_one_on_every_run() {
        let names: Vec<String> = (0..40).map(|n| format!("app-{n}")).collect();
        let files: Vec<String> = names
            .iter()
            .map(|name| tokens(&Branding::default(), name, &names).to_string())
            .collect();
        let distinct: std::collections::HashSet<&String> = files.iter().collect();
        assert_eq!(
            distinct.len(),
            names.len(),
            "two Apps of one project share a look"
        );
        assert_eq!(
            tokens(&Branding::default(), "app-3", &names),
            tokens(&Branding::default(), "app-3", &names),
        );
        // The App itself among the others, or not, is the same project.
        let others: Vec<String> = names.iter().filter(|n| *n != "app-3").cloned().collect();
        assert_eq!(
            tokens(&Branding::default(), "app-3", &names),
            tokens(&Branding::default(), "app-3", &others),
        );
    }

    #[test]
    fn more_apps_than_looks_still_get_a_look() {
        let names: Vec<String> = (0..LOOKS + 5).map(|n| format!("a{n}")).collect();
        let tokens = tokens(&Branding::default(), "a0", &names);
        assert!(tokens["color"]["accent"].is_string());
    }

    #[test]
    fn the_file_holds_hex_colours_and_lengths_only_and_the_template_keys_exactly() {
        let hex = regex::Regex::new(r"^#[0-9a-f]{6}$").expect("a regex");
        let length = regex::Regex::new(r"^[0-9]+(\.[0-9]+)?(rem|px)$").expect("a regex");
        let template: Value = serde_json::from_str(&crate::agents::preview::template_files()[PATH])
            .expect("the template's tokens are JSON");
        for branding in brandings() {
            let tokens = tokens(&branding, "air-quality-desk", &[]);
            fn keys(value: &Value, prefix: &str, out: &mut Vec<String>) {
                if let Value::Object(map) = value {
                    for (key, child) in map {
                        keys(child, &format!("{prefix}/{key}"), out);
                    }
                } else {
                    out.push(prefix.to_owned());
                }
            }
            let (mut ours, mut theirs) = (Vec::new(), Vec::new());
            keys(&tokens, "", &mut ours);
            keys(&template, "", &mut theirs);
            assert_eq!(ours, theirs, "the SDK reads exactly the template's keys");
            for (path, value) in ["/color", "/chart/palette", "/map"]
                .iter()
                .flat_map(|root| {
                    let node = tokens.pointer(root).expect("a section");
                    let values: Vec<(String, &Value)> = match node {
                        Value::Object(map) => map
                            .iter()
                            .map(|(k, v)| (format!("{root}/{k}"), v))
                            .collect(),
                        Value::Array(items) => items
                            .iter()
                            .enumerate()
                            .map(|(i, v)| (format!("{root}/{i}"), v))
                            .collect(),
                        _ => Vec::new(),
                    };
                    values
                })
            {
                let value = value.as_str().expect("a string");
                assert!(hex.is_match(value), "{path}: {value}");
            }
            for root in ["/space", "/radius", "/font/size"] {
                for (key, value) in tokens
                    .pointer(root)
                    .and_then(Value::as_object)
                    .expect("a section")
                {
                    let value = value.as_str().expect("a string");
                    assert!(length.is_match(value), "{root}/{key}: {value}");
                }
            }
        }
    }

    #[test]
    fn the_accent_stays_the_brands_hue_within_the_turn() {
        let tokens = tokens(&Branding::default(), "air-quality-desk", &[]);
        let look = look_for("air-quality-desk", &[]);
        let colours = Branding::default().colours;
        let brand = [&colours.primary, &colours.secondary, &colours.accent][look.brand];
        let hue = |colour: &str| {
            let [_, a, b] = lab_of(colour);
            b.atan2(a).to_degrees()
        };
        let turn = (hue(&colour(&tokens, "/color/accent")) - hue(brand) + 540.0) % 360.0 - 180.0;
        assert!(
            turn.abs() <= 24.5,
            "the accent turned {turn:.1}° from the brand"
        );
    }
}
