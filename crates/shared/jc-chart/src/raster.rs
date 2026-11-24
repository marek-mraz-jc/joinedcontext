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
    let step = [1.0, 2.0, 5.0]
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
    let Ok(at) = jiff::Timestamp::from_millisecond(ms as i64) else {
        return "?".into();
    };
    if span_ms <= 2.0 * 86_400_000.0 {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%Y-%m-%d").to_string()
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

    // Times as chrono 0.4.45 read and printed them, pinned before the move to jiff (T-7240):
    // axis labels in UTC, `observedAt` back in milliseconds with `Z`, a temporal cell in epoch ms.
    #[test]
    fn chart_times_keep_chrono_bytes() {
        let ms = 1_774_747_800_120.0; // 2026-03-29T01:30:00.120Z, Bratislava's spring change
        assert_eq!(date(ms, 3_600_000.0), "01:30");
        assert_eq!(date(ms, 30.0 * 86_400_000.0), "2026-03-29");
        assert_eq!(date(f64::MAX, 1.0), "?");
        let point = Point {
            x: Some(Cell::Number(1.0)),
            y: None,
            color: None,
            size: None,
            theta: None,
        };
        let mut cats = crate::spec::Categories::default();
        for (input, observed, cell) in [
            (
                "2026-03-29T03:30:00.120+02:00",
                Some("2026-03-29T01:30:00.120Z"),
                Some(ms),
            ),
            (
                "2026-10-25T03:30:00+03:00",
                Some("2026-10-25T00:30:00.000Z"),
                Some(1_792_888_200_000.0),
            ),
            (
                "2026-03-29T01:30:00.1234Z",
                Some("2026-03-29T01:30:00.123Z"),
                Some(1_774_747_800_123.0),
            ),
            ("2026-03-29T01:30Z", None, None),
            ("2026-03-29", None, None),
        ] {
            let row = serde_json::json!({ "observedAt": input });
            let row = row.as_object().expect("row");
            let v = crate::spec::view_point(&point, row);
            assert_eq!(
                v.get("observedAt").and_then(serde_json::Value::as_str),
                observed,
                "{input}"
            );
            let got = crate::spec::cell(row.get("observedAt"), Scale::Temporal, &mut cats);
            assert_eq!(got, cell.map(Cell::Number), "{input}");
        }
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

    fn chart(mark: Mark, x: Option<Scale>, y: Option<Scale>, points: Vec<Point>) -> Chart {
        let channel = |scale: Option<Scale>, categories| {
            scale.map(|scale| Channel {
                scale,
                title: "t".into(),
                categories,
            })
        };
        Chart {
            mark,
            width: 300,
            height: 240,
            title: String::new(),
            x: channel(x, 3),
            y: channel(y, 0),
            colors: points
                .iter()
                .filter_map(|p| p.color)
                .max()
                .map_or(0, |c| c + 1),
            points,
        }
    }

    fn at(x: f64, y: f64, color: Option<usize>) -> Point {
        Point {
            x: Some(Cell::Number(x)),
            y: Some(Cell::Number(y)),
            color,
            ..Point::default()
        }
    }

    /// The pixels of one legend swatch.
    const SWATCH: usize = (2 * SCALE as usize * 4 + 1) * (7 * SCALE as usize + 1);

    /// How many pixels of `c` hold `rgb`.
    fn count(c: &Canvas, rgb: [u8; 3]) -> usize {
        c.pixels.chunks(3).filter(|px| *px == rgb).count()
    }

    fn pixel(c: &Canvas, x: i64, y: i64) -> [u8; 3] {
        let at = usize::try_from((y * c.width + x) * 3).expect("inside");
        [c.pixels[at], c.pixels[at + 1], c.pixels[at + 2]]
    }

    #[test]
    fn each_mark_draws_its_points_in_their_group_colours() {
        let (first, second) = (color_of(Some(0)), color_of(Some(1)));
        assert_ne!(first, second);
        let bars = chart(
            Mark::Bar,
            Some(Scale::Nominal),
            Some(Scale::Quantitative),
            vec![
                Point {
                    x: Some(Cell::Category(0)),
                    y: Some(Cell::Number(5.0)),
                    color: Some(0),
                    ..Point::default()
                },
                Point {
                    x: Some(Cell::Category(1)),
                    y: Some(Cell::Number(-2.0)),
                    color: Some(1),
                    ..Point::default()
                },
                Point {
                    x: None,
                    y: Some(Cell::Number(9.0)),
                    ..Point::default()
                },
            ],
        );
        let c = canvas(&bars, later()).expect("bars");
        assert!(
            count(&c, first) > 500 && count(&c, second) > 100,
            "both bars filled"
        );
        let points = chart(
            Mark::Point,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![
                Point {
                    size: Some(1.0),
                    ..at(0.0, 0.0, None)
                },
                Point {
                    size: Some(9.0),
                    ..at(5.0, 5.0, Some(1))
                },
                Point {
                    x: None,
                    ..at(1.0, 1.0, None)
                },
            ],
        );
        let c = canvas(&points, later()).expect("points");
        let (small, big) = (count(&c, first) - SWATCH, count(&c, second) - SWATCH);
        assert!(
            small > 0 && big > 4 * small,
            "size scales the disc: {small} {big}"
        );
        let even = chart(
            Mark::Point,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![
                Point {
                    size: Some(3.0),
                    ..at(0.0, 0.0, None)
                },
                Point {
                    size: Some(3.0),
                    ..at(1.0, 1.0, None)
                },
            ],
        );
        let unsized_ = chart(
            Mark::Point,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![at(0.0, 0.0, None), at(1.0, 1.0, None)],
        );
        let (even, unsized_) = (
            canvas(&even, later()).expect("even"),
            canvas(&unsized_, later()).expect("unsized"),
        );
        assert!(
            count(&even, first) > count(&unsized_, first),
            "one size draws r 6, none r 4"
        );
        let rules = chart(
            Mark::Rule,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![
                at(1.0, 4.0, Some(0)),
                Point {
                    y: None,
                    ..at(2.0, 0.0, Some(1))
                },
                Point {
                    x: None,
                    ..at(0.0, 2.0, Some(2))
                },
                Point::default(),
            ],
        );
        let c = canvas(&rules, later()).expect("rules");
        for group in 0..3 {
            assert!(count(&c, color_of(Some(group))) > 50, "rule {group} drawn");
        }
        let area = chart(
            Mark::Area,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![
                at(0.0, 1.0, None),
                at(1.0, 3.0, None),
                at(1.0, 2.0, None),
                at(2.0, 2.0, Some(1)),
                at(3.0, 2.0, Some(9)),
            ],
        );
        let c = canvas(&area, later()).expect("area");
        let back = paper();
        let light: [u8; 3] =
            std::array::from_fn(|i| ((u16::from(first[i]) + 2 * u16::from(back[i])) / 3) as u8);
        assert!(count(&c, light) > 1000, "the area under the line is filled");
        assert!(
            count(&c, first) > 50 && count(&c, second) > 0,
            "the line and the lone point drawn"
        );
        let mut line = area.clone();
        line.mark = Mark::Line;
        assert_eq!(
            count(&canvas(&line, later()).expect("line"), light),
            0,
            "a line fills nothing"
        );
    }

    #[test]
    fn an_arc_fills_its_slices_clockwise_from_twelve() {
        let slice = |theta: f64, color| Point {
            theta: Some(theta),
            color,
            ..Point::default()
        };
        let pie = Chart {
            mark: Mark::Arc,
            width: 200,
            height: 200,
            title: "Share".into(),
            x: None,
            y: None,
            colors: 2,
            points: vec![
                slice(1.0, Some(0)),
                slice(3.0, Some(1)),
                slice(-1.0, Some(0)),
                slice(f64::INFINITY, Some(0)),
            ],
        };
        let c = canvas(&pie, later()).expect("pie");
        let top = 16 + LINE + 8 + LINE + 8;
        let (cw, ch) = (200 - 8 * ADVANCE, 200 - top);
        let (cx, cy, r) = (cw / 2, top + ch / 2, (cw.min(ch) / 2 - 16).max(4));
        // A quarter (theta 1 of 4) on the right of twelve, the rest after it.
        assert_eq!(pixel(&c, cx + r / 2, cy - r / 2), color_of(Some(0)));
        assert_eq!(pixel(&c, cx - r / 2, cy - r / 2), color_of(Some(1)));
        assert_eq!(pixel(&c, cx, cy + r / 2), color_of(Some(1)));
        assert_eq!(pixel(&c, cx + r + 2, cy), paper(), "outside the disc");
        let legend = 200 - 8 * ADVANCE + ADVANCE;
        assert_eq!(pixel(&c, legend + 1, top + 1), color_of(Some(0)));
        assert_eq!(pixel(&c, legend + 1, top + LINE + 1), color_of(Some(1)));
        let uncoloured = Chart {
            colors: 0,
            points: vec![slice(1.0, None), slice(1.0, None)],
            ..pie.clone()
        };
        let c = canvas(&uncoloured, later()).expect("by index");
        assert!(
            count(&c, color_of(Some(1))) > 100,
            "a slice without colour takes its index"
        );
        let empty = Chart {
            points: vec![slice(0.0, Some(1))],
            ..pie.clone()
        };
        let c = canvas(&empty, later()).expect("nothing to draw");
        assert_eq!(
            count(&c, color_of(Some(1))),
            SWATCH,
            "only the legend swatch"
        );
        // A slice of nothing takes no pixel, not even the one at twelve o'clock.
        let zero_first = Chart {
            colors: 2,
            points: vec![slice(0.0, Some(1)), slice(1.0, Some(0))],
            ..pie.clone()
        };
        let c = canvas(&zero_first, later()).expect("zero first");
        assert_eq!(pixel(&c, cx, cy - r / 2), color_of(Some(0)));
        assert_eq!(
            count(&c, color_of(Some(1))),
            SWATCH,
            "only the legend swatch"
        );
        let late = render(&pie, Instant::now() - Duration::from_millis(1));
        assert_eq!(late, Err(Limited));
    }

    #[test]
    fn a_plot_too_small_for_its_axes_is_left_blank() {
        let tiny = Chart {
            width: 60,
            height: 60,
            ..chart(
                Mark::Line,
                Some(Scale::Quantitative),
                Some(Scale::Quantitative),
                vec![at(0.0, 0.0, None), at(1.0, 1.0, None)],
            )
        };
        let c = canvas(&tiny, later()).expect("tiny");
        assert_eq!(count(&c, axis()), 0, "no axis drawn");
        assert_eq!(count(&c, paper()), 60 * 60);
        // The y labels "0", "0.2" ... "1" make the plot start at x 64 and y 42: a plot at most
        // ten pixels wide or tall is left blank, each side alone.
        let unit = || {
            chart(
                Mark::Line,
                Some(Scale::Quantitative),
                Some(Scale::Quantitative),
                vec![at(0.0, 0.0, None), at(1.0, 1.0, None)],
            )
        };
        for (width, height, drawn) in [
            (85, 240, false),
            (86, 240, false),
            (91, 240, true),
            (300, 109, false),
            (300, 115, true),
        ] {
            let c = canvas(
                &Chart {
                    width,
                    height,
                    ..unit()
                },
                later(),
            )
            .expect("sized");
            assert_eq!(count(&c, axis()) > 0, drawn, "{width}x{height}");
        }
        let bare = chart(Mark::Point, None, None, vec![Point::default()]);
        let c = canvas(&bare, later()).expect("no channels");
        assert!(count(&c, axis()) > 0, "the axes still frame the plot");
        assert_eq!(count(&c, color_of(Some(0))), 0);
    }

    #[test]
    fn a_montage_lays_tiles_in_rows_of_two() {
        let one = chart(
            Mark::Line,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![at(0.0, 0.0, None)],
        );
        assert_eq!(
            montage(&[Tile::Chart(one.clone())], later()),
            render(&one, later())
        );
        let none = montage(&[], later()).expect("none");
        assert_eq!(png_size(&none), (480, 64));
        let error = Tile::Error {
            width: 200,
            height: 100,
            text: "Widget 2 failed".into(),
        };
        let three = [
            Tile::Chart(one.clone()),
            error.clone(),
            Tile::Chart(Chart {
                width: 100,
                height: 50,
                ..one.clone()
            }),
        ];
        let png = montage(&three, later()).expect("three");
        assert_eq!(
            png_size(&png),
            (GAP as u32 * 3 + 300 + 200, GAP as u32 * 3 + 240 + 50)
        );
        assert_eq!(
            montage(&three, Instant::now() - Duration::from_millis(1)),
            Err(Limited)
        );
        let mut out = Canvas::new(20, 20);
        let mut inner = Canvas::new(4, 4);
        inner.rect(0, 0, 3, 3, ink());
        out.frame(1, 1, 6, 6, grid());
        out.blit(&inner, 2, 2);
        out.blit(&inner, 18, 18);
        assert_eq!(
            (pixel(&out, 1, 1), pixel(&out, 6, 3), pixel(&out, 3, 6)),
            (grid(), grid(), grid())
        );
        assert_eq!(
            (pixel(&out, 2, 2), pixel(&out, 5, 5), pixel(&out, 19, 19)),
            (ink(), ink(), ink())
        );
        assert_eq!(count(&out, ink()), 16 + 4, "the blit past the edge is cut");
        out.set(-1, 0, ink());
        out.set(0, 20, ink());
        assert_eq!(count(&out, ink()), 20, "a pixel outside is dropped");
    }

    #[test]
    fn every_glyph_is_its_own_and_unknown_signs_are_a_question_mark() {
        let signs = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ .,:-/_%()#+";
        let glyphs: std::collections::HashSet<[u8; 7]> = signs.chars().map(glyph).collect();
        assert_eq!(
            glyphs.len(),
            signs.chars().count(),
            "no two signs share a glyph"
        );
        assert_eq!(glyph('q'), glyph('Q'));
        assert_eq!(glyph('é'), glyph('~'));
        assert!(!glyphs.contains(&glyph('~')));
        let mut c = Canvas::new(40, 20);
        c.text(0, 0, "I", ink());
        // I: a 3-wide top bar, the stem, a 3-wide foot; each bit a 2x2 block.
        assert_eq!(count(&c, ink()), (3 + 5 + 3) * 4);
    }

    #[test]
    fn domains_pad_flat_and_empty_values() {
        assert_eq!(
            domain(std::iter::empty(), Scale::Quantitative, false),
            (0.0, 1.0)
        );
        assert_eq!(
            domain([0.0, 2.0].into_iter(), Scale::Nominal, true),
            (-0.5, 2.5)
        );
        assert_eq!(
            domain([3.0, 5.0].into_iter(), Scale::Quantitative, true),
            (0.0, 5.0)
        );
        assert_eq!(
            domain([-3.0, -1.0].into_iter(), Scale::Quantitative, true),
            (-3.0, 0.0)
        );
        assert_eq!(
            domain([3.0, 5.0].into_iter(), Scale::Quantitative, false),
            (3.0, 5.0)
        );
        assert_eq!(
            domain([10.0].into_iter(), Scale::Quantitative, false),
            (9.0, 11.0)
        );
        assert_eq!(
            domain([0.0].into_iter(), Scale::Temporal, false),
            (-1.0, 1.0)
        );
        assert_eq!(
            domain([-10.0].into_iter(), Scale::Quantitative, false),
            (-11.0, -9.0)
        );
    }

    #[test]
    fn axis_marks_number_categories_and_fit_the_room() {
        let ch = |scale, categories| Channel {
            scale,
            title: String::new(),
            categories,
        };
        let marks = axis_marks(&ch(Scale::Nominal, 5), -0.5, 4.5, 2);
        assert_eq!(marks, [(0.0, "#1".to_owned()), (3.0, "#4".to_owned())]);
        let marks = axis_marks(&ch(Scale::Nominal, 0), -0.5, 0.5, -3);
        assert_eq!(marks, [(0.0, "#1".to_owned())]);
        let marks = axis_marks(&ch(Scale::Quantitative, 0), 0.0, 4.0, 9);
        assert_eq!(marks.first(), Some(&(0.0, "0".to_owned())));
        assert_eq!(marks.last(), Some(&(4.0, "4".to_owned())));
        let day = 86_400_000.0;
        let marks = axis_marks(&ch(Scale::Temporal, 0), 0.0, 3.0 * day, 4);
        assert_eq!(
            marks.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
            ["1970-01-01", "1970-01-02", "1970-01-03", "1970-01-04"]
        );
        assert_eq!(label(Scale::Nominal, 2.0, 1.0), "#3");
        // Past the longest whole step, the span is cut in `room` parts.
        let year = 365.0 * day;
        assert_eq!(time_ticks(0.0, 2.0 * year, 2), [0.0, year, 2.0 * year]);
    }

    /// The pixels of `c` holding `rgb`, as (x, y), row by row.
    fn lit(c: &Canvas, rgb: [u8; 3]) -> Vec<(i64, i64)> {
        (0..c.height)
            .flat_map(|y| (0..c.width).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(c, x, y) == rgb)
            .collect()
    }

    /// Width, height and RGB rows of a PNG this module wrote, each chunk's CRC checked.
    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        use std::io::Read as _;
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let (mut at, mut idat, mut kinds) = (8, Vec::new(), Vec::new());
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().expect("len")) as usize;
            let (kind, data) = (&png[at + 4..at + 8], &png[at + 8..at + 8 + len]);
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            let stored =
                u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().expect("crc"));
            assert_eq!(crc.sum(), stored, "{kind:?} crc");
            if kind == b"IDAT" {
                idat.extend_from_slice(data);
            }
            kinds.push(kind.to_vec());
            at += 12 + len;
        }
        assert_eq!(
            kinds,
            [b"IHDR".to_vec(), b"IDAT".to_vec(), b"IEND".to_vec()]
        );
        assert_eq!(&png[24..29], &[8, 2, 0, 0, 0], "8-bit RGB, no interlace");
        let (w, h) = png_size(png);
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&idat[..])
            .read_to_end(&mut raw)
            .expect("zlib");
        let row = w as usize * 3 + 1;
        assert_eq!(raw.len(), row * h as usize);
        let mut pixels = Vec::new();
        for line in raw.chunks(row) {
            assert_eq!(line[0], 0, "filter none");
            pixels.extend_from_slice(&line[1..]);
        }
        (w, h, pixels)
    }

    #[test]
    fn the_png_holds_the_canvas_pixels_row_by_row() {
        let mut c = Canvas::new(7, 3);
        c.set(6, 0, ink());
        c.set(0, 2, grid());
        let (w, h, pixels) = decode(&c.png());
        assert_eq!((w, h), (7, 3));
        assert_eq!(pixels, c.pixels);
        assert_eq!(&pixels[18..21], &ink());
        assert_eq!(&pixels[42..45], &grid());
    }

    #[test]
    fn lines_step_pixel_by_pixel_in_every_direction() {
        let rgb = ink();
        let mut c = Canvas::new(12, 12);
        c.line((1, 5), (4, 5), 1, rgb);
        assert_eq!(lit(&c, rgb), [(1, 5), (2, 5), (3, 5), (4, 5)]);
        let mut back = Canvas::new(12, 12);
        back.line((4, 5), (1, 5), 1, rgb);
        assert_eq!(
            lit(&back, rgb),
            lit(&c, rgb),
            "right to left draws the same"
        );
        let mut up = Canvas::new(12, 12);
        up.line((3, 4), (3, 1), 1, rgb);
        assert_eq!(lit(&up, rgb), [(3, 1), (3, 2), (3, 3), (3, 4)]);
        let mut steep = Canvas::new(12, 12);
        steep.line((1, 1), (3, 7), 1, rgb);
        assert_eq!(
            lit(&steep, rgb),
            [(1, 1), (1, 2), (2, 3), (2, 4), (2, 5), (3, 6), (3, 7)]
        );
        let mut rising = Canvas::new(12, 12);
        rising.line((1, 7), (7, 4), 1, rgb);
        assert_eq!(
            lit(&rising, rgb),
            [(6, 4), (7, 4), (4, 5), (5, 5), (2, 6), (3, 6), (1, 7)]
        );
        let mut thick = Canvas::new(12, 12);
        thick.line((3, 5), (5, 5), 3, rgb);
        let want: Vec<(i64, i64)> = (4..=6).flat_map(|y| (2..=6).map(move |x| (x, y))).collect();
        assert_eq!(lit(&thick, rgb), want, "3 wide: one pixel each side");
        let mut even = Canvas::new(12, 12);
        even.line((3, 5), (3, 5), 2, rgb);
        assert_eq!(
            lit(&even, rgb),
            [(2, 4), (3, 4), (2, 5), (3, 5)],
            "2 wide leans up and left"
        );
    }

    #[test]
    fn discs_rects_and_glyphs_land_where_asked() {
        let rgb = ink();
        let mut c = Canvas::new(12, 12);
        c.disc(5, 6, 2, rgb);
        let want: Vec<(i64, i64)> = (4..=8)
            .flat_map(|y| (3..=7).map(move |x| (x, y)))
            .filter(|&(x, y)| (x - 5) * (x - 5) + (y - 6) * (y - 6) <= 4)
            .collect();
        assert_eq!(want.len(), 13);
        assert_eq!(lit(&c, rgb), want);
        let mut r = Canvas::new(6, 6);
        r.rect(4, 3, 2, 1, rgb);
        assert_eq!(
            lit(&r, rgb),
            [
                (2, 1),
                (3, 1),
                (4, 1),
                (2, 2),
                (3, 2),
                (4, 2),
                (2, 3),
                (3, 3),
                (4, 3)
            ]
        );
        let mut all = Canvas::new(6, 6);
        all.rect(-9, -9, 99, 99, rgb);
        assert_eq!(count(&all, rgb), 36);
        let mut t = Canvas::new(40, 20);
        t.text(1, 2, "-.", rgb);
        // '-' is row 3 of glyph 0, '.' rows 5-6, columns 1-2 of glyph 1, each bit 2x2.
        let dash: Vec<(i64, i64)> = (8..=9)
            .flat_map(|y| (1..=10).map(move |x| (x, y)))
            .collect();
        let dot: Vec<(i64, i64)> = (12..=15)
            .flat_map(|y| (15..=18).map(move |x| (x, y)))
            .collect();
        assert_eq!(lit(&t, rgb), [dash, dot].concat());
        assert_eq!(ADVANCE, 12);
        let mut inner = Canvas::new(3, 2);
        inner.set(2, 0, rgb);
        inner.set(0, 1, rgb);
        let mut out = Canvas::new(8, 8);
        out.blit(&inner, 4, 5);
        assert_eq!(lit(&out, rgb), [(6, 5), (4, 6)]);
    }

    #[test]
    fn the_raster_draws_in_the_theme_light_colours() {
        let look = crate::theme::light();
        assert_eq!(
            (ink(), muted(), grid(), axis(), paper()),
            (
                look.text,
                look.muted,
                look.grid,
                look.domain,
                look.background
            )
        );
        assert_eq!(Canvas::new(2, 1).pixels, [paper(), paper()].concat());
        assert_eq!(color_of(None), color_of(Some(0)));
        assert_eq!(color_of(Some(palette().len() + 1)), color_of(Some(1)));
    }

    /// The sha256 of an RGB buffer, as hex.
    fn digest(pixels: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(pixels)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Each mark, the legend, the title and a montage drawn to the pixel, pinned by hash: a
    /// change to any coordinate the drawing computes changes one of them. A deliberate change
    /// of the drawing updates the hash it moved, with the reason in the commit.
    #[test]
    fn each_chart_draws_its_pinned_pixels() {
        let day = 86_400_000.0;
        let series = |group: usize, offset: f64| -> Vec<Point> {
            (0..6)
                .map(|i| {
                    at(
                        1.7e12 + f64::from(i) * day,
                        offset + f64::from(i * i % 7),
                        Some(group),
                    )
                })
                .collect()
        };
        let titled = |mut c: Chart, title: &str| {
            c.title = title.into();
            c
        };
        let line = titled(
            chart(
                Mark::Line,
                Some(Scale::Temporal),
                Some(Scale::Quantitative),
                series(0, 1.0),
            ),
            "PM10 (ug/m3)",
        );
        let area = chart(
            Mark::Area,
            Some(Scale::Temporal),
            Some(Scale::Quantitative),
            [series(0, -2.0), series(1, 3.0)].concat(),
        );
        let bars = titled(
            chart(
                Mark::Bar,
                Some(Scale::Nominal),
                Some(Scale::Quantitative),
                vec![
                    Point {
                        x: Some(Cell::Category(0)),
                        y: Some(Cell::Number(4.0)),
                        color: Some(0),
                        ..Point::default()
                    },
                    Point {
                        x: Some(Cell::Category(0)),
                        y: Some(Cell::Number(2.5)),
                        color: Some(1),
                        ..Point::default()
                    },
                    Point {
                        x: Some(Cell::Category(1)),
                        y: Some(Cell::Number(-1.0)),
                        color: Some(0),
                        ..Point::default()
                    },
                    Point {
                        x: Some(Cell::Category(2)),
                        y: Some(Cell::Number(7.0)),
                        color: Some(1),
                        ..Point::default()
                    },
                ],
            ),
            "Stations",
        );
        let points = chart(
            Mark::Point,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            (0..5)
                .map(|i| Point {
                    size: Some(f64::from(i) + 2.0),
                    ..at(f64::from(i) * 1e6, f64::from(i) * 0.25, Some(0))
                })
                .collect(),
        );
        // 6.5 days over 920 px: six labels' room takes two-day steps, seven would take one.
        let wide = Chart {
            width: 1000,
            points: (0..14)
                .map(|i| at(1.7e12 + f64::from(i) * day / 2.0, f64::from(i % 3), None))
                .collect(),
            ..line.clone()
        };
        let quantitative_bars = chart(
            Mark::Bar,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![at(1.0, 2.0, None), at(2.0, 3.0, None), at(4.0, 1.0, None)],
        );
        let mut named_rows = chart(
            Mark::Point,
            Some(Scale::Quantitative),
            Some(Scale::Nominal),
            (0..40)
                .map(|i| Point {
                    y: Some(Cell::Category(i)),
                    ..at(i as f64, 0.0, None)
                })
                .collect(),
        );
        named_rows.y = Some(Channel {
            scale: Scale::Nominal,
            title: "Station".into(),
            categories: 40,
        });
        let rules = chart(
            Mark::Rule,
            Some(Scale::Quantitative),
            Some(Scale::Quantitative),
            vec![
                at(1.0, 4.0, Some(0)),
                Point {
                    y: None,
                    ..at(2.0, 0.0, Some(1))
                },
                Point {
                    x: None,
                    ..at(0.0, 2.0, Some(2))
                },
            ],
        );
        let arc = Chart {
            mark: Mark::Arc,
            width: 240,
            height: 200,
            title: "Share".into(),
            x: None,
            y: None,
            colors: 3,
            points: [5.0, 3.0, 2.0]
                .iter()
                .enumerate()
                .map(|(i, t)| Point {
                    theta: Some(*t),
                    color: Some(i),
                    ..Point::default()
                })
                .collect(),
        };
        let mut got = Vec::new();
        for c in [
            &line,
            &area,
            &bars,
            &points,
            &rules,
            &arc,
            &wide,
            &quantitative_bars,
            &named_rows,
        ] {
            got.push(digest(&canvas(c, later()).expect("drawn").pixels));
        }
        let tiles = [
            Tile::Chart(line.clone()),
            Tile::Error {
                width: 200,
                height: 120,
                text: "Widget 2: no data".into(),
            },
            Tile::Chart(titled(
                Chart {
                    width: 180,
                    height: 140,
                    ..bars.clone()
                },
                "",
            )),
        ];
        got.push(digest(
            &decode(&montage(&tiles, later()).expect("montage")).2,
        ));
        let want = [
            "3d0d3a1418485101c9a0e165d174ebe1d6c84af74eaec2d3f5a76de5563cc128",
            "404114c34b79f4761d39c4b850252f2d8c382d49fe8fcdef3a7f0e14c84d2f22",
            "533f1d38ba50adfecd07fb3941c62f09b752bc3a713bea075ae43eb58f8e0be0",
            "95ebec0cf90f21ea06fc6144c71ece59aad984cfc9972fd8bcd6044b95ad114e",
            "591f720c1f39e59520089d2cc8260e67577d71a49b007d1daaa95fe1a43873a4",
            "e9825373ca3c6382a21c29d74fc6befb85320b23746208c1d028166dc69ca256",
            "b02a25f64634cd018d82f8765bb954a9bdee97db1cb3d3adf30bc81fe8593a6e",
            "ddc57b2c2d2ad0b3c8333af82dc4dd623048d69d98acdee6c45b3ec911c99143",
            "6a4e627770df543b0b5bb7adfeb964de658fefbf840199aef52b7868ce824a9a",
            "ed6f55af21f3f3e72ceabc1548db7d31695f7d0680877ea5d832716498c8eb5c",
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn ticks_are_round_and_cover_the_domain() {
        assert_eq!(ticks(0.0, 10.0), [0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        assert_eq!(number(2.5), "2.5");
        assert_eq!(number(3_000_000.0), "3M");
        assert_eq!(number(-4_500_000_000.0), "-4.5G");
        assert_eq!(number(0.0), "0");
        assert_eq!(number(0.0005), "0.5M");
        assert_eq!(ticks(5.0, 5.0), [5.0]);
        assert_eq!(ticks(0.0, f64::INFINITY), [0.0]);
        assert_eq!(ticks(0.0, 700.0), [0.0, 200.0, 400.0, 600.0]);
        assert_eq!(ticks(0.0, 1e6).len(), 6);
        assert_eq!(ticks(3.0, 17.0), [5.0, 10.0, 15.0]);
        assert_eq!(
            ticks(0.0, 60.0),
            [0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
            "a step of 10 mag"
        );
        // A step under half the float spacing at 1e17 never moves: twelve ticks and no more.
        assert_eq!(ticks(1e17, 1e17 + 16.0), [1e17; 12]);
        assert_eq!(number(0.001), "0.001");
        assert_eq!(date(0.0, 2.0 * 86_400_000.0), "00:00");
        assert_eq!(date(0.0, 1.5 * 86_400_000.0), "00:00");
        assert_eq!(date(0.0, 2.0 * 86_400_000.0 + 1.0), "1970-01-01");
        let hour = 3_600_000.0;
        let ch = Channel {
            scale: Scale::Temporal,
            title: String::new(),
            categories: 0,
        };
        let marks = axis_marks(&ch, 1e12, 1e12 + hour, 2);
        assert!(
            marks.iter().all(|(_, l)| l.len() == 5 && l.contains(':')),
            "{marks:?}"
        );
        assert_eq!(
            time_ticks(10.0 * hour, 11.0 * hour, 6),
            [
                10.0 * hour,
                10.25 * hour,
                10.5 * hour,
                10.75 * hour,
                11.0 * hour
            ]
        );
        let hour = 3_600_000.0;
        assert_eq!(
            time_ticks(0.5 * hour, 11.5 * hour, 6),
            [2.0 * hour, 4.0 * hour, 6.0 * hour, 8.0 * hour, 10.0 * hour]
        );
    }

    #[test]
    fn a_short_pixel_buffer_is_never_written_or_read_past() {
        // A canvas whose buffer is shorter than its sides: set and blit skip what is not there.
        let mut short = Canvas {
            width: 4,
            height: 4,
            pixels: vec![0; 3 * 4],
        };
        short.set(1, 0, [9, 9, 9]);
        short.set(3, 3, [9, 9, 9]);
        assert_eq!(short.pixels, [0, 0, 0, 9, 9, 9, 0, 0, 0, 0, 0, 0]);
        let mut target = Canvas::new(4, 4);
        let before = target.pixels.clone();
        target.blit(&short, 0, 0);
        assert_eq!(
            &target.pixels[..12],
            &short.pixels[..],
            "the first row comes over"
        );
        assert_eq!(&target.pixels[12..], &before[12..], "the rest stays paper");
    }

    #[test]
    fn a_line_point_without_a_y_or_with_an_unknown_group_is_left_out() {
        let light = palette()[0];
        let ch = |scale| Channel {
            scale,
            title: String::new(),
            categories: 0,
        };
        let line = Chart {
            mark: Mark::Line,
            width: 200,
            height: 150,
            title: String::new(),
            x: Some(ch(Scale::Quantitative)),
            y: Some(ch(Scale::Quantitative)),
            colors: 0,
            points: vec![
                Point {
                    x: Some(Cell::Number(1.0)),
                    ..Point::default()
                },
                Point {
                    x: Some(Cell::Number(2.0)),
                    y: Some(Cell::Number(2.0)),
                    color: Some(5),
                    ..Point::default()
                },
            ],
        };
        let drawn = canvas(&line, later()).expect("drawn");
        assert_eq!(count(&drawn, light), 0, "no point of the line is drawn");
    }

    #[test]
    fn the_xy_painter_draws_nothing_for_an_arc() {
        let pie = Chart {
            mark: Mark::Arc,
            width: 120,
            height: 120,
            title: String::new(),
            x: None,
            y: None,
            colors: 1,
            points: vec![Point {
                theta: Some(1.0),
                ..Point::default()
            }],
        };
        let mut c = Canvas::new(120, 120);
        draw_xy(&mut c, &pie, 40, 0, later()).expect("in time");
        assert_eq!(count(&c, palette()[0]), 0, "no slice is painted here");
    }
}
