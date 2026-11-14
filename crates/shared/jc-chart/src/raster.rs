//! The raster behind `jc_render` (T-6860): a chart of plain rows drawn into an RGB buffer and
//! encoded as PNG, in this process, with no font file, no SVG, no script engine, no network and
//! no file system. Text comes from a built-in 5x7 bitmap font that draws ASCII letters, digits
//! and a few signs; anything else is drawn as `?`. The caller's own words (title, axis titles)
//! are the only text drawn besides numbers and dates the raster computes itself: category
//! values come from the data, which other people wrote, so they are numbered on the picture
//! and named in the structured answer instead.

use std::io::Write as _;
use std::time::Instant;

/// One chart value: a number (quantitative, or a temporal value as epoch milliseconds) or a
/// category index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    Number(f64),
    Category(usize),
}

impl Cell {
    fn value(self) -> f64 {
        match self {
            Cell::Number(v) => v,
            Cell::Category(i) => i as f64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Line,
    Bar,
    Area,
    Point,
    Arc,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Quantitative,
    Temporal,
    Nominal,
}

/// One positional channel: how its values scale and the axis title the caller wrote.
#[derive(Debug, Clone)]
pub struct Channel {
    pub scale: Scale,
    pub title: String,
    /// The number of categories of a nominal channel.
    pub categories: usize,
}

/// One drawn row: its channels, absent where the row has no such value.
#[derive(Debug, Clone, Default)]
pub struct Point {
    pub x: Option<Cell>,
    pub y: Option<Cell>,
    pub color: Option<usize>,
    pub size: Option<f64>,
    pub theta: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Chart {
    pub mark: Mark,
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub x: Option<Channel>,
    pub y: Option<Channel>,
    /// The number of color groups, 0 without a color channel.
    pub colors: usize,
    pub points: Vec<Point>,
}

/// The render ran past its deadline.
#[derive(Debug, PartialEq, Eq)]
pub struct Limited;

/// The theme's colours (T-6886): text, tick labels, grid, axis lines and the background.
fn ink() -> [u8; 3] {
    crate::theme::light().text
}
fn muted() -> [u8; 3] {
    crate::theme::light().muted
}
fn grid() -> [u8; 3] {
    crate::theme::light().grid
}
fn axis() -> [u8; 3] {
    crate::theme::light().domain
}
fn paper() -> [u8; 3] {
    crate::theme::light().background
}

/// The series colours: the theme's `range.category`.
pub fn palette() -> &'static [[u8; 3]] {
    &crate::theme::light().category
}

/// Glyph scale: 5x7 glyphs drawn at 2x, one blank column between them.
const SCALE: i64 = 2;
const ADVANCE: i64 = 6 * SCALE;
const LINE: i64 = 9 * SCALE;

fn color_of(group: Option<usize>) -> [u8; 3] {
    let palette = palette();
    palette
        .get(group.unwrap_or(0) % palette.len().max(1))
        .copied()
        .unwrap_or_else(ink)
}

struct Canvas {
    width: i64,
    height: i64,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        let (width, height) = (i64::from(width), i64::from(height));
        let len = usize::try_from(width * height * 3).unwrap_or(0);
        Self {
            width,
            height,
            pixels: paper().repeat(len / 3),
        }
    }

    fn set(&mut self, x: i64, y: i64, rgb: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let at = usize::try_from((y * self.width + x) * 3).unwrap_or(usize::MAX);
        if let Some(px) = self.pixels.get_mut(at..at + 3) {
            px.copy_from_slice(&rgb);
        }
    }

    fn rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, rgb: [u8; 3]) {
        let (x0, x1) = (x0.min(x1).max(0), x0.max(x1).min(self.width - 1));
        let (y0, y1) = (y0.min(y1).max(0), y0.max(y1).min(self.height - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.set(x, y, rgb);
            }
        }
    }

    /// The outline of a rectangle, one pixel wide.
    fn frame(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, rgb: [u8; 3]) {
        for (a, b) in [
            ((x0, y0), (x1, y0)),
            ((x1, y0), (x1, y1)),
            ((x0, y1), (x1, y1)),
            ((x0, y0), (x0, y1)),
        ] {
            self.line(a, b, 1, rgb);
        }
    }

    /// `other` copied in with its top left corner at `x`, `y`; what falls outside is cut.
    fn blit(&mut self, other: &Canvas, x: i64, y: i64) {
        for row in 0..other.height {
            for col in 0..other.width {
                let at = usize::try_from((row * other.width + col) * 3).unwrap_or(usize::MAX);
                if let Some(px) = other.pixels.get(at..at + 3) {
                    self.set(x + col, y + row, [px[0], px[1], px[2]]);
                }
            }
        }
    }

    /// A line `thick` pixels wide (Bresenham, a square stamped at each step).
    fn line(&mut self, from: (i64, i64), to: (i64, i64), thick: i64, rgb: [u8; 3]) {
        let (mut x, mut y) = from;
        let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
        let (sx, sy) = (if x < to.0 { 1 } else { -1 }, if y < to.1 { 1 } else { -1 });
        let mut err = dx + dy;
        let half = thick / 2;
        loop {
            self.rect(
                x - half,
                y - half,
                x - half + thick - 1,
                y - half + thick - 1,
                rgb,
            );
            if (x, y) == to {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn disc(&mut self, cx: i64, cy: i64, r: i64, rgb: [u8; 3]) {
        for y in -r..=r {
            for x in -r..=r {
                if x * x + y * y <= r * r {
                    self.set(cx + x, cy + y, rgb);
                }
            }
        }
    }

    fn text(&mut self, x: i64, y: i64, text: &str, rgb: [u8; 3]) {
        for (i, ch) in text.chars().enumerate() {
            let rows = glyph(ch);
            let left = x + i as i64 * ADVANCE;
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        let (px, py) = (left + col * SCALE, y + row as i64 * SCALE);
                        self.rect(px, py, px + SCALE - 1, py + SCALE - 1, rgb);
                    }
                }
            }
        }
    }

    /// The PNG of the canvas: 8-bit RGB, no filter, zlib through flate2.
    fn png(&self) -> Vec<u8> {
        let w = usize::try_from(self.width).unwrap_or(0);
        let mut raw = Vec::with_capacity(self.pixels.len() + self.pixels.len() / w.max(1) + 1);
        for row in self.pixels.chunks(w * 3) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        // Writing into a Vec cannot fail.
        let idat = z
            .write_all(&raw)
            .and_then(|()| z.finish())
            .unwrap_or_default();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&u32::try_from(self.width).unwrap_or(0).to_be_bytes());
        ihdr.extend_from_slice(&u32::try_from(self.height).unwrap_or(0).to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [
            (b"IHDR", &ihdr[..]),
            (b"IDAT", &idat[..]),
            (b"IEND", &[][..]),
        ] {
            out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            out.extend_from_slice(&crc.sum().to_be_bytes());
        }
        out
    }
}

/// The 5x7 rows of `ch`, bit 4 the leftmost column; lowercase draws as uppercase.
fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x11, 0x1F, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        ' ' => [0; 7],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        ',' => [0, 0, 0, 0, 0x0C, 0x04, 0x08],
        ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        '/' => [0, 0x01, 0x02, 0x04, 0x08, 0x10, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0x1F],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '+' => [0, 0x04, 0x04, 0x1F, 0x04, 0x04, 0],
        _ => [0x0E, 0x11, 0x01, 0x02, 0x04, 0, 0x04],
    }
}

/// About five round ticks across `lo..hi`.
fn ticks(lo: f64, hi: f64) -> Vec<f64> {
    let span = hi - lo;
    if !span.is_finite() || span <= 0.0 {
        return vec![lo];
    }
    let raw = span / 5.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * mag)
        .find(|s| span / s <= 6.0)
        .unwrap_or(10.0 * mag);
    let mut out = Vec::new();
    let mut t = (lo / step).ceil() * step;
    while t <= hi + step * 1e-9 && out.len() < 12 {
        out.push(t);
        t += step;
    }
    out
}

fn number(v: f64) -> String {
    if v.abs() >= 1e6 || (v != 0.0 && v.abs() < 1e-3) {
        // The bitmap font has no exponent: scale by thousands with a suffix instead.
        let (div, suffix) = if v.abs() >= 1e9 {
            (1e9, "G")
        } else if v.abs() >= 1e6 {
            (1e6, "M")
        } else {
            (1e-3, "M")
        };
        return format!("{}{suffix}", number(v / div));
    }
    let text = format!("{v:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn date(ms: f64, span_ms: f64) -> String {
    let Some(at) = chrono::DateTime::from_timestamp_millis(ms as i64) else {
        return "?".into();
    };
    if span_ms <= 2.0 * 86_400_000.0 {
        at.format("%H:%M").to_string()
    } else {
        at.format("%Y-%m-%d").to_string()
    }
}

fn label(scale: Scale, v: f64, span: f64) -> String {
    match scale {
        Scale::Temporal => date(v, span),
        Scale::Nominal => format!("#{}", v as i64 + 1),
        Scale::Quantitative => number(v),
    }
}

/// The ticks of one axis with their labels, at most `room` of them: categories by number, time
/// on whole minutes, hours or days, numbers on round steps.
fn axis_marks(ch: &Channel, lo: f64, hi: f64, room: i64) -> Vec<(f64, String)> {
    let room = usize::try_from(room).unwrap_or(2).max(2);
    let marks: Vec<f64> = match ch.scale {
        Scale::Nominal => {
            let n = ch.categories.max(1);
            let every = n.div_ceil(room);
            (0..n).step_by(every).map(|i| i as f64).collect()
        }
        Scale::Temporal => time_ticks(lo, hi, room),
        Scale::Quantitative => ticks(lo, hi),
    };
    marks
        .into_iter()
        .map(|t| (t, label(ch.scale, t, hi - lo)))
        .collect()
}

/// Ticks on whole steps of time (UTC) across `lo..hi` milliseconds, at most `room`.
fn time_ticks(lo: f64, hi: f64, room: usize) -> Vec<f64> {
    const MIN: f64 = 60_000.0;
    let steps = [
        1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 180.0, 360.0, 720.0, 1440.0, 2880.0, 10_080.0, 43_200.0,
    ];
    let span = hi - lo;
    let step = steps
        .iter()
        .map(|m| m * MIN)
        .find(|s| span / s <= room as f64)
        .unwrap_or(span / room as f64);
    let mut out = Vec::new();
    let mut t = (lo / step).ceil() * step;
    while t <= hi && out.len() <= room {
        out.push(t);
        t += step;
    }
    out
}

/// The domain of a channel's values; a bar or area starts at zero.
fn domain(values: impl Iterator<Item = f64>, scale: Scale, from_zero: bool) -> (f64, f64) {
    let (mut lo, mut hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if !lo.is_finite() {
        return (0.0, 1.0);
    }
    if scale == Scale::Nominal {
        return (lo - 0.5, hi + 0.5);
    }
    if from_zero {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if hi - lo <= f64::EPSILON {
        let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        lo -= pad;
        hi += pad;
    }
    (lo, hi)
}

fn check(deadline: Instant) -> Result<(), Limited> {
    if Instant::now() > deadline {
        Err(Limited)
    } else {
        Ok(())
    }
}

/// Draws `chart` and answers its PNG, or [`Limited`] once `deadline` has passed.
pub fn render(chart: &Chart, deadline: Instant) -> Result<Vec<u8>, Limited> {
    Ok(canvas(chart, deadline)?.png())
}

/// One tile of a montage (T-6867): a chart, or a widget that was not drawn, a box with the
/// platform's own words.
#[derive(Debug, Clone)]
pub enum Tile {
    Chart(Chart),
    Error {
        width: u32,
        height: u32,
        text: String,
    },
}

/// Tiles of a montage side by side; below them the next row.
const COLUMNS: usize = 2;
const GAP: i64 = 16;

/// `tiles` in rows of [`COLUMNS`], in order, each in a light frame; one chart alone is its own
/// picture, none is a note saying so. [`Limited`] once `deadline` has passed.
pub fn montage(tiles: &[Tile], deadline: Instant) -> Result<Vec<u8>, Limited> {
    if let [Tile::Chart(chart)] = tiles {
        return render(chart, deadline);
    }
    if tiles.is_empty() {
        let mut c = Canvas::new(480, 64);
        c.text(16, 24, "This Dashboard has no widgets", ink());
        return Ok(c.png());
    }
    let mut drawn = Vec::with_capacity(tiles.len());
    for tile in tiles {
        drawn.push(match tile {
            Tile::Chart(chart) => canvas(chart, deadline)?,
            Tile::Error {
                width,
                height,
                text,
            } => {
                let mut c = Canvas::new(*width, *height);
                c.text(16, c.height / 2 - LINE / 2, text, ink());
                c
            }
        });
    }
    let columns = COLUMNS.min(drawn.len());
    let mut widths = vec![0; columns];
    let mut heights = vec![0; drawn.len().div_ceil(columns)];
    for (i, c) in drawn.iter().enumerate() {
        widths[i % columns] = widths[i % columns].max(c.width);
        heights[i / columns] = heights[i / columns].max(c.height);
    }
    let side = |sizes: &[i64]| sizes.iter().map(|s| s + GAP).sum::<i64>() + GAP;
    let (w, h) = (side(&widths), side(&heights));
    let mut out = Canvas::new(u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0));
    for (i, c) in drawn.iter().enumerate() {
        check(deadline)?;
        let x = GAP + widths[..i % columns].iter().map(|s| s + GAP).sum::<i64>();
        let y = GAP + heights[..i / columns].iter().map(|s| s + GAP).sum::<i64>();
        out.frame(x - 1, y - 1, x + c.width, y + c.height, grid());
        out.blit(c, x, y);
    }
    check(deadline)?;
    Ok(out.png())
}

/// `chart` drawn on a canvas of its own.
fn canvas(chart: &Chart, deadline: Instant) -> Result<Canvas, Limited> {
    check(deadline)?;
    let mut c = Canvas::new(chart.width, chart.height);
    let w = c.width;
    // The title's line, then the y axis title's line above the plot.
    let top = if chart.title.is_empty() {
        16
    } else {
        16 + LINE + 8
    } + LINE
        + 8;
    if !chart.title.is_empty() {
        c.text(16, 12, &chart.title, ink());
    }
    let legend = if chart.colors > 1 { 8 * ADVANCE } else { 0 };
    if chart.mark == Mark::Arc {
        draw_arc(&mut c, chart, top, legend, deadline)?;
    } else {
        draw_xy(&mut c, chart, top, legend, deadline)?;
    }
    if legend > 0 {
        let left = w - legend + ADVANCE;
        for group in 0..chart.colors.min(20) {
            let y = top + group as i64 * LINE;
            c.rect(
                left,
                y,
                left + 2 * SCALE * 4,
                y + 7 * SCALE,
                color_of(Some(group)),
            );
            c.text(left + 6 * SCALE * 2, y, &format!("#{}", group + 1), ink());
        }
    }
    check(deadline)?;
    Ok(c)
}

fn draw_arc(
    c: &mut Canvas,
    chart: &Chart,
    top: i64,
    legend: i64,
    deadline: Instant,
) -> Result<(), Limited> {
    let slices: Vec<(f64, usize)> = chart
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            p.theta
                .filter(|t| *t > 0.0 && t.is_finite())
                .map(|t| (t, p.color.unwrap_or(i)))
        })
        .collect();
    let total: f64 = slices.iter().map(|(t, _)| t).sum();
    if total <= 0.0 {
        return Ok(());
    }
    let (cw, ch) = (c.width - legend, c.height - top);
    let r = (cw.min(ch) / 2 - 16).max(4);
    let (cx, cy) = (cw / 2, top + ch / 2);
    let mut ends = Vec::with_capacity(slices.len());
    let mut acc = 0.0;
    for (t, group) in &slices {
        acc += t / total;
        ends.push((acc * std::f64::consts::TAU, *group));
    }
    for y in -r..=r {
        check(deadline)?;
        for x in -r..=r {
            if x * x + y * y > r * r {
                continue;
            }
            // Clockwise from twelve o'clock, as Vega-Lite draws theta.
            let mut angle = (x as f64).atan2(-(y as f64));
            if angle < 0.0 {
                angle += std::f64::consts::TAU;
            }
            let group = ends
                .iter()
                .find(|(end, _)| angle <= *end)
                .map_or(0, |(_, g)| *g);
            c.set(cx + x, cy + y, color_of(Some(group)));
        }
    }
    Ok(())
}

fn draw_xy(
    c: &mut Canvas,
    chart: &Chart,
    top: i64,
    legend: i64,
    deadline: Instant,
) -> Result<(), Limited> {
    let from_zero = matches!(chart.mark, Mark::Bar | Mark::Area);
    let xs = chart.x.as_ref();
    let ys = chart.y.as_ref();
    let xd = xs.map(|ch| {
        domain(
            chart.points.iter().filter_map(|p| p.x.map(Cell::value)),
            ch.scale,
            false,
        )
    });
    let yd = ys.map(|ch| {
        domain(
            chart.points.iter().filter_map(|p| p.y.map(Cell::value)),
            ch.scale,
            from_zero,
        )
    });
    let y_marks = match (ys, yd) {
        (Some(ch), Some((lo, hi))) => {
            axis_marks(ch, lo, hi, ((c.height - top) / (2 * LINE)).max(2))
        }
        _ => Vec::new(),
    };
    let widest = y_marks
        .iter()
        .map(|(_, text)| text.len() as i64)
        .max()
        .unwrap_or(0);
    let (left, right, bottom) = (
        16 + (widest + 1) * ADVANCE,
        c.width - legend - 16,
        c.height - 3 * LINE - 8,
    );
    if right <= left + 10 || bottom <= top + 10 {
        return Ok(());
    }
    let px = |v: f64| {
        xd.map_or(left, |(lo, hi)| {
            left + ((v - lo) / (hi - lo) * (right - left) as f64).round() as i64
        })
    };
    let py = |v: f64| {
        yd.map_or(bottom, |(lo, hi)| {
            bottom - ((v - lo) / (hi - lo) * (bottom - top) as f64).round() as i64
        })
    };

    // Grid, axes and tick labels.
    for (t, text) in &y_marks {
        let y = py(*t);
        c.line((left, y), (right, y), 1, grid());
        c.text(left - 8 - text.len() as i64 * ADVANCE, y - 7, text, muted());
    }
    if let (Some(ch), Some((lo, hi))) = (xs, xd) {
        // As many labels as fit side by side, each about ten glyphs wide.
        let room = ((right - left) / (11 * ADVANCE)).max(2);
        for (t, text) in axis_marks(ch, lo, hi, room) {
            let x = px(t);
            c.line((x, bottom), (x, bottom + 6), 1, axis());
            c.text(
                x - text.len() as i64 * ADVANCE / 2,
                bottom + 10,
                &text,
                muted(),
            );
        }
        c.text(left, bottom + 10 + LINE + 4, &ch.title, ink());
    }
    if let Some(ch) = ys {
        c.text(16, top - LINE - 4, &ch.title, ink());
    }
    c.line((left, top), (left, bottom), 2, axis());
    c.line((left, bottom), (right, bottom), 2, axis());

    let zero = py(yd.map_or(0.0, |(lo, hi)| 0f64.clamp(lo, hi)));
    let groups = chart.colors.max(1);
    let band = match (xs, xd) {
        (Some(ch), Some((lo, hi))) if ch.scale == Scale::Nominal => {
            ((right - left) as f64 / (hi - lo)) as i64
        }
        _ => ((right - left) / (chart.points.len() as i64 + 1).max(1)).clamp(2, 60),
    };
    match chart.mark {
        Mark::Bar => {
            let bw = (band * 8 / 10 / groups as i64).max(1);
            for p in &chart.points {
                check(deadline)?;
                let (Some(x), Some(y)) = (p.x, p.y) else {
                    continue;
                };
                let g = p.color.unwrap_or(0) as i64;
                let x0 = px(x.value()) - bw * groups as i64 / 2 + g * bw;
                c.rect(x0, py(y.value()), x0 + bw - 1, zero, color_of(p.color));
            }
        }
        Mark::Point => {
            let (lo, hi) = chart
                .points
                .iter()
                .filter_map(|p| p.size)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| {
                    (a.min(s), b.max(s))
                });
            for p in &chart.points {
                check(deadline)?;
                let (Some(x), Some(y)) = (p.x, p.y) else {
                    continue;
                };
                let r = p.size.map_or(4, |s| {
                    if hi > lo {
                        2 + ((s - lo) / (hi - lo) * 10.0) as i64
                    } else {
                        6
                    }
                });
                c.disc(px(x.value()), py(y.value()), r, color_of(p.color));
            }
        }
        Mark::Rule => {
            for p in &chart.points {
                check(deadline)?;
                match (p.x, p.y) {
                    (Some(x), Some(y)) => c.line(
                        (px(x.value()), zero),
                        (px(x.value()), py(y.value())),
                        2,
                        color_of(p.color),
                    ),
                    (Some(x), None) => c.line(
                        (px(x.value()), top),
                        (px(x.value()), bottom),
                        2,
                        color_of(p.color),
                    ),
                    (None, Some(y)) => c.line(
                        (left, py(y.value())),
                        (right, py(y.value())),
                        2,
                        color_of(p.color),
                    ),
                    (None, None) => {}
                }
            }
        }
        Mark::Line | Mark::Area => {
            // One pass over the points, so many groups cost no more than one.
            let mut lines: Vec<Vec<(i64, i64)>> = vec![Vec::new(); groups];
            for p in &chart.points {
                check(deadline)?;
                if let (Some(x), Some(y), Some(line)) =
                    (p.x, p.y, lines.get_mut(p.color.unwrap_or(0)))
                {
                    line.push((px(x.value()), py(y.value())));
                }
            }
            for line in &mut lines {
                check(deadline)?;
                line.sort_by_key(|(x, _)| *x);
            }
            // Every fill first, so no group's line hides under another's area.
            if chart.mark == Mark::Area {
                for (group, line) in lines.iter().enumerate() {
                    let (rgb, back) = (color_of(Some(group)), paper());
                    let light: [u8; 3] = std::array::from_fn(|i| {
                        ((u16::from(rgb[i]) + 2 * u16::from(back[i])) / 3) as u8
                    });
                    for pair in line.windows(2) {
                        check(deadline)?;
                        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                        for x in x0..=x1 {
                            let t = if x1 == x0 {
                                0.0
                            } else {
                                (x - x0) as f64 / (x1 - x0) as f64
                            };
                            let y = y0 + ((y1 - y0) as f64 * t).round() as i64;
                            c.line((x, y), (x, zero), 1, light);
                        }
                    }
                }
            }
            for (group, line) in lines.iter().enumerate() {
                let rgb = color_of(Some(group));
                for pair in line.windows(2) {
                    check(deadline)?;
                    c.line(pair[0], pair[1], 3, rgb);
                }
                if line.len() == 1 {
                    c.disc(line[0].0, line[0].1, 3, rgb);
                }
            }
        }
        Mark::Arc => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    fn png_size(png: &[u8]) -> (u32, u32) {
        let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        (be(16), be(20))
    }

    #[test]
    fn a_chart_is_a_png_of_the_asked_size() {
        let chart = Chart {
            mark: Mark::Line,
            width: 320,
            height: 200,
            title: "pm10".into(),
            x: Some(Channel {
                scale: Scale::Temporal,
                title: "observedAt".into(),
                categories: 0,
            }),
            y: Some(Channel {
                scale: Scale::Quantitative,
                title: "pm10".into(),
                categories: 0,
            }),
            colors: 0,
            points: (0..10)
                .map(|i| Point {
                    x: Some(Cell::Number(1.7e12 + f64::from(i) * 3.6e6)),
                    y: Some(Cell::Number(f64::from(i * i))),
                    ..Point::default()
                })
                .collect(),
        };
        let png = render(&chart, later()).expect("drawn");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png_size(&png), (320, 200));
    }

    #[test]
    fn a_chart_past_its_deadline_is_limited() {
        let chart = Chart {
            mark: Mark::Bar,
            width: 100,
            height: 100,
            title: String::new(),
            x: None,
            y: None,
            colors: 0,
            points: Vec::new(),
        };
        assert_eq!(
            render(&chart, Instant::now() - Duration::from_millis(1)),
            Err(Limited)
        );
    }

    #[test]
    fn ticks_are_round_and_cover_the_domain() {
        assert_eq!(ticks(0.0, 10.0), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(number(2.5), "2.5");
        assert_eq!(number(3_000_000.0), "3M");
        let hour = 3_600_000.0;
        assert_eq!(
            time_ticks(0.5 * hour, 11.5 * hour, 6),
            [2.0 * hour, 4.0 * hour, 6.0 * hour, 8.0 * hour, 10.0 * hour]
        );
    }
}
