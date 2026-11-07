//! A page image made ready for the text reader: a photographed page cut out of
//! its background, its background flattened to white, its lines straightened,
//! and its text enlarged when too small to read.

/// The image the reader is handed, and the way back from its pixels to the
/// page's.
pub struct Prepared {
    pub width: u32,
    pub height: u32,
    /// One grey byte per pixel.
    pub grey: Vec<u8>,
    page: Option<Page>,
    source: (f64, f64),
    turned: (f64, f64),
    angle: f64,
    scale: f64,
}

impl Prepared {
    /// The box, on the source image, that covers a box the reader returned.
    pub fn source_box(&self, x: f64, y: f64, width: f64, height: f64) -> [f64; 4] {
        let (cos, sin) = (self.angle.cos(), self.angle.sin());
        let corners = [
            (x, y),
            (x + width, y),
            (x, y + height),
            (x + width, y + height),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (px, py) in corners {
            let (dx, dy) = (
                px / self.scale - self.turned.0 / 2.0,
                py / self.scale - self.turned.1 / 2.0,
            );
            let point = (
                self.source.0 / 2.0 + dx * cos - dy * sin,
                self.source.1 / 2.0 + dx * sin + dy * cos,
            );
            let (sx, sy) = match &self.page {
                Some(page) => page.source(point),
                None => point,
            };
            x0 = x0.min(sx);
            y0 = y0.min(sy);
            x1 = x1.max(sx);
            y1 = y1.max(sy);
        }
        [x0, y0, x1 - x0, y1 - y0]
    }
}

/// Median height, in pixels, of the letters below which the reader is handed
/// the page twice as large, which it reads better. A black and white image is
/// never enlarged, only blurred: its broken strokes pass for small print, and
/// enlarging loses.
const SMALL_TEXT: usize = 8;

/// Narrowest grey range stretched to full range. Under 1 % of ink the low
/// percentile falls on the paper: stretched, a clean page comes out all black
/// and a noisy one as noise.
const MIN_SPAN: f32 = 64.0;

/// A black and white image is blurred before reading, by a Gaussian of its
/// letter height divided by this: the reader takes grey levels, and reads a
/// soft edge better than a jagged one. The divisor sharpens the reading of
/// photocopies and faxes without blurring a clean page.
const LETTERS_PER_BLUR: f32 = 11.0;

/// Tilts beyond this are searched for no gain: a scan is never that crooked.
const MAX_TILT_DEGREES: f64 = 8.0;

pub fn prepare(rgba: &[u8], width: u32, height: u32) -> Prepared {
    let (w, h) = (width as usize, height as usize);
    let grey: Vec<f32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let luma = 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
            255.0 - f32::from(p[3]) / 255.0 * (255.0 - luma)
        })
        .collect();
    let page = Page::find(&grey, w, h);
    let (grey, w, h) = match &page {
        Some(page) => {
            let (cut, w, h) = page.cut(&grey, w, h);
            (smooth(&cut, w, h), w, h)
        }
        None => (grey, w, h),
    };
    let flat = flatten(&grey, w, h);
    let angle = tilt(&flat, w, h);
    let (turned, tw, th) = match angle.abs() < 0.25f64.to_radians() {
        true => (flat, w, h),
        false => turn(&flat, w, h, angle),
    };
    let binary = grey.iter().filter(|v| (32.0..224.0).contains(*v)).count() * 100 < grey.len();
    let (grey, scale) = match (binary, letter_height(&turned, tw, th)) {
        (true, Some(letter)) => {
            let turned: Vec<f32> = turned.iter().map(|v| f32::from(*v)).collect();
            let soft = gaussian(&turned, tw, th, letter as f32 / LETTERS_PER_BLUR);
            (soft.into_iter().map(round_u8).collect(), 1)
        }
        (false, Some(letter)) if letter < SMALL_TEXT => (double(&turned, tw, th), 2),
        _ => (turned, 1),
    };
    Prepared {
        width: (tw * scale) as u32,
        height: (th * scale) as u32,
        grey,
        page,
        source: (w as f64, h as f64),
        turned: (tw as f64, th as f64),
        angle,
        scale: scale as f64,
    }
}

/// A page photographed on a darker background: the map from the page cut out
/// straight to the photo.
struct Page {
    /// The map from the unit square to the page's corners on the photo.
    square: [f64; 8],
    size: (f64, f64),
    /// What is left of the page's edge on each side, so the background's
    /// blurred border does not come along.
    inset: f64,
}

/// Smallest step, in grey levels, between the background and the page's edge.
const EDGE_CONTRAST: f32 = 20.0;

/// Share of the rays across a side whose step lies on one line.
const EDGE_AGREEMENT: f64 = 0.75;

impl Page {
    /// The page, when all four of its edges stand out as straight steps from a
    /// darker background, and they enclose most of the image.
    fn find(grey: &[f32], w: usize, h: usize) -> Option<Page> {
        let step = w.max(h).div_ceil(500).max(1);
        let (small, sw, sh) = shrink(grey, w, h, step);
        let closed = extremum(&extremum(&small, sw, sh, 2, at_least), sw, sh, 2, at_most);
        let scale = step as f64;
        let edges = [Side::Left, Side::Top, Side::Right, Side::Bottom].map(|side| {
            side.edge(&closed, sw, sh)
                .map(|[a, b, c]| [a, b, c * scale])
        });
        let [Some(left), Some(top), Some(right), Some(bottom)] = edges else {
            return None;
        };
        let corners = [
            meet(left, top)?,
            meet(top, right)?,
            meet(right, bottom)?,
            meet(bottom, left)?,
        ];
        let (fw, fh) = (w as f64, h as f64);
        let margin = 0.02 * fw.max(fh);
        let within = corners
            .iter()
            .all(|(x, y)| (-margin..fw + margin).contains(x) && (-margin..fh + margin).contains(y));
        let turns: Vec<f64> = (0..4)
            .map(|i| {
                let (a, b, c) = (corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4]);
                (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0)
            })
            .collect();
        let area = (0..4)
            .map(|i| {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum::<f64>()
            / 2.0;
        if !within || turns.iter().any(|t| *t <= 0.0) || area < 0.3 * fw * fh {
            return None;
        }
        let length = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
        let size = (
            (length(corners[0], corners[1]) + length(corners[3], corners[2])) / 2.0,
            (length(corners[0], corners[3]) + length(corners[1], corners[2])) / 2.0,
        );
        Some(Page {
            square: square_to(corners),
            size,
            inset: (0.005 * size.0.max(size.1)).ceil(),
        })
    }

    /// The point of the photo under a point of the page cut out.
    fn source(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (u, v) = (
            (x + self.inset) / self.size.0,
            (y + self.inset) / self.size.1,
        );
        let [a, b, c, d, e, f, g, h] = self.square;
        let t = g * u + h * v + 1.0;
        ((a * u + b * v + c) / t, (d * u + e * v + f) / t)
    }

    /// The page cut out of the photo and laid straight: [`Self::source`] at
    /// each pixel's centre, its terms in `u` and in `v` computed once.
    fn cut(&self, grey: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
        let pw = (self.size.0 - 2.0 * self.inset).round() as usize;
        let ph = (self.size.1 - 2.0 * self.inset).round() as usize;
        let [a, b, c, d, e, f, g, k] = self.square;
        let columns: Vec<[f64; 3]> = (0..pw)
            .map(|x| {
                let u = (x as f64 + 0.5 + self.inset) / self.size.0;
                [a * u, d * u, g * u]
            })
            .collect();
        let mut out = Vec::with_capacity(pw * ph);
        for y in 0..ph {
            let v = (y as f64 + 0.5 + self.inset) / self.size.1;
            let (bv, ev, kv) = (b * v, e * v, k * v);
            for [au, du, gu] in &columns {
                let t = gu + kv + 1.0;
                let (sx, sy) = ((au + bv + c) / t, (du + ev + f) / t);
                out.push(bilinear(grey, w, h, sx as f32 - 0.5, sy as f32 - 0.5));
            }
        }
        (out, pw, ph)
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

impl Side {
    /// The side's edge as `(a, b, c)`, the line `a x + b y + c = 0` in the
    /// image's pixels. Each ray runs inward across the outer 45% of the image
    /// and keeps the peak of its first step up to the light at least half as
    /// high as its strongest, so a dark heading inside the page does not pass for its
    /// edge; the edge is the line most of those steps agree on.
    fn edge(self, image: &[f32], w: usize, h: usize) -> Option<[f64; 3]> {
        const REACH: usize = 3;
        let (across, along) = match self {
            Side::Left | Side::Right => (w, h),
            Side::Top | Side::Bottom => (h, w),
        };
        let at = |x: usize, y: usize| match self {
            Side::Left => image[y * w + x],
            Side::Right => image[y * w + w - 1 - x],
            Side::Top => image[x * w + y],
            Side::Bottom => image[(h - 1 - x) * w + y],
        };
        let reach = across * 45 / 100;
        let mut profile = Vec::with_capacity(reach + REACH);
        let mut ray = Vec::with_capacity(reach);
        let mut steps: Vec<(f64, f64, f32)> = Vec::new();
        for y in along * 15 / 100..along * 85 / 100 {
            profile.clear();
            profile.extend((0..reach + REACH - 1).map(|x| at(x, y)));
            let mean = |from: usize| profile[from..from + REACH].iter().sum::<f32>();
            ray.clear();
            ray.extend((REACH..reach).map(|x| (mean(x) - mean(x - REACH)) / REACH as f32));
            let strongest = ray.iter().copied().fold(0.0, at_least);
            let mut first = ray.iter().position(|step| *step >= strongest / 2.0)?;
            while first + 1 < ray.len() && ray[first + 1] > ray[first] {
                first += 1;
            }
            steps.push((y as f64 + 0.5, (first + REACH) as f64, ray[first]));
        }
        let mut line = theil_sen(&steps)?;
        let mut kept: Vec<&(f64, f64, f32)> = Vec::new();
        for tolerance in [3.0, 1.5] {
            kept = steps
                .iter()
                .filter(|(y, x, _)| (x - line.0 * y - line.1).abs() <= tolerance)
                .collect();
            line = fit(&kept)?;
        }
        kept.retain(|(y, x, _)| (x - line.0 * y - line.1).abs() <= 1.5);
        let mut contrast: Vec<f32> = kept.iter().map(|s| s.2).collect();
        let middle = contrast.len() / 2;
        if (kept.len() as f64) < EDGE_AGREEMENT * steps.len() as f64
            || line.0.abs() > 0.35
            || *contrast.select_nth_unstable_by(middle, f32::total_cmp).1 < EDGE_CONTRAST
        {
            return None;
        }
        let point = |y: f64| {
            let x = line.0 * y + line.1;
            match self {
                Side::Left => (x, y),
                Side::Right => (w as f64 - x, y),
                Side::Top => (y, x),
                Side::Bottom => (y, h as f64 - x),
            }
        };
        let ((x0, y0), (x1, y1)) = (point(0.0), point(along as f64));
        Some([y1 - y0, x0 - x1, x1 * y0 - x0 * y1])
    }
}

/// The line `x = a y + b` through the points `(y, x)` that most of them lie
/// on: the median slope between points half the side apart, then the median
/// offset.
fn theil_sen(points: &[(f64, f64, f32)]) -> Option<(f64, f64)> {
    let half = points.len() / 2;
    if half < 10 {
        return None;
    }
    let median = |mut values: Vec<f64>| {
        let middle = values.len() / 2;
        *values.select_nth_unstable_by(middle, f64::total_cmp).1
    };
    let a = median(
        (0..half)
            .map(|i| (points[i + half].1 - points[i].1) / (points[i + half].0 - points[i].0))
            .collect(),
    );
    Some((a, median(points.iter().map(|p| p.1 - a * p.0).collect())))
}

/// The least-squares line `x = a y + b` through the points `(y, x)`.
fn fit(points: &[&(f64, f64, f32)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if n < 10.0 {
        return None;
    }
    let (sy, sx) = points
        .iter()
        .fold((0.0, 0.0), |(sy, sx), p| (sy + p.0, sx + p.1));
    let (my, mx) = (sy / n, sx / n);
    let (syy, syx) = points.iter().fold((0.0, 0.0), |(syy, syx), p| {
        (syy + (p.0 - my).powi(2), syx + (p.0 - my) * (p.1 - mx))
    });
    let a = syx / syy;
    Some((a, mx - a * my))
}

fn meet(a: [f64; 3], b: [f64; 3]) -> Option<(f64, f64)> {
    let t = a[0] * b[1] - a[1] * b[0];
    (t.abs() > 1e-9).then(|| {
        (
            (a[1] * b[2] - a[2] * b[1]) / t,
            (a[2] * b[0] - a[0] * b[2]) / t,
        )
    })
}

/// The projective map taking the unit square's corners, clockwise from the
/// origin, onto `corners`.
fn square_to(corners: [(f64, f64); 4]) -> [f64; 8] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = corners;
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    let det = dx1 * dy2 - dx2 * dy1;
    let g = (dx3 * dy2 - dx2 * dy3) / det;
    let h = (dx1 * dy3 - dx3 * dy1) / det;
    [
        x1 - x0 + g * x1,
        x3 - x0 + h * x3,
        x0,
        y1 - y0 + g * y1,
        y3 - y0 + h * y3,
        y0,
        g,
        h,
    ]
}

/// A Gaussian blur of one pixel. A phone's grain, once the paper is divided
/// out, reads as specks that swell the boxes of the words, and the blur
/// removes them.
fn smooth(image: &[f32], w: usize, h: usize) -> Vec<f32> {
    const KERNEL: [f32; 7] = [0.0044, 0.054, 0.242, 0.3992, 0.242, 0.054, 0.0044];
    convolve(image, w, h, &KERNEL)
}

/// Divide each pixel by the paper around it, measured with the ink closed
/// over at a quarter of the resolution, then stretch the result to full range.
fn flatten(grey: &[f32], w: usize, h: usize) -> Vec<u8> {
    const STEP: usize = 4;
    let (small, sw, sh) = shrink(grey, w, h, STEP);
    let paper = mean_filter(
        &extremum(&extremum(&small, sw, sh, 3, at_least), sw, sh, 3, at_most),
        sw,
        sh,
        3,
    );
    // The bilinear paper at each pixel, its horizontal step done once per
    // row of the reduced image.
    let columns = Lerp::table(w, sw, |x| x as f32 / STEP as f32);
    let across: Vec<f32> = paper
        .chunks_exact(sw)
        .flat_map(|row| columns.iter().map(|lerp| lerp.apply(row)))
        .collect();
    let mut out = vec![0f32; w * h];
    for (y, (pixels, target)) in grey
        .chunks_exact(w)
        .zip(out.chunks_exact_mut(w))
        .enumerate()
    {
        let down = Lerp::new(y as f32 / STEP as f32, sh);
        let (top, bottom) = (&across[down.from * w..][..w], &across[down.to * w..][..w]);
        for x in 0..w {
            let background = top[x] * down.keep + bottom[x] * down.take;
            let value = pixels[x] / at_least(background, 1.0) * 255.0;
            target[x] = at_most(at_least(value, 0.0), 255.0);
        }
    }
    let mut histogram = [0usize; 256];
    for value in &out {
        histogram[*value as usize] += 1;
    }
    let percentile = |share: f64| {
        let target = (share * out.len() as f64) as usize;
        let mut seen = 0;
        histogram
            .iter()
            .position(|n| {
                seen += n;
                seen > target
            })
            .unwrap_or(255) as f32
    };
    let (low, high) = (percentile(0.01), percentile(0.99));
    let low = low.min(high - MIN_SPAN);
    let span = high - low;
    out.iter()
        .map(|value| ((value - low) / span * 255.0).clamp(0.0, 255.0) as u8)
        .collect()
}

/// The image reduced `step` times, each pixel the mean of those it covers.
fn shrink(grey: &[f32], w: usize, h: usize, step: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
    let mut small = vec![0f32; sw * sh];
    for (y, row) in grey.chunks_exact(w).enumerate() {
        for (total, cell) in small[y / step * sw..][..sw]
            .iter_mut()
            .zip(row.chunks(step))
        {
            for value in cell {
                *total += value;
            }
        }
    }
    for (sy, totals) in small.chunks_exact_mut(sw).enumerate() {
        let rows = step.min(h - sy * step);
        for (sx, total) in totals.iter_mut().enumerate() {
            *total /= (rows * step.min(w - sx * step)) as f32;
        }
    }
    (small, sw, sh)
}

fn extremum(
    image: &[f32],
    w: usize,
    h: usize,
    radius: usize,
    pick: impl Fn(f32, f32) -> f32 + Copy,
) -> Vec<f32> {
    // Edges repeated outward, which changes neither a maximum nor a minimum,
    // so every window has the same taps.
    let mut rows = vec![0f32; w * h];
    let mut padded = vec![0f32; w + 2 * radius];
    for (line, out) in image.chunks_exact(w).zip(rows.chunks_exact_mut(w)) {
        for (i, value) in padded.iter_mut().enumerate() {
            *value = line[i.saturating_sub(radius).min(w - 1)];
        }
        out.copy_from_slice(&padded[..w]);
        for k in 1..=2 * radius {
            for (value, other) in out.iter_mut().zip(&padded[k..]) {
                *value = pick(*value, *other);
            }
        }
    }
    let mut out = vec![0f32; w * h];
    for (y, target) in out.chunks_exact_mut(w).enumerate() {
        let (from, to) = (y.saturating_sub(radius), (y + radius).min(h - 1));
        target.copy_from_slice(&rows[from * w..][..w]);
        for row in rows[(from + 1) * w..(to + 1) * w].chunks_exact(w) {
            for (value, other) in target.iter_mut().zip(row) {
                *value = pick(*value, *other);
            }
        }
    }
    out
}

fn mean_filter(image: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut rows = vec![0f32; w * h];
    let inner = radius..w.saturating_sub(radius).max(radius);
    for (line, out) in image.chunks_exact(w).zip(rows.chunks_exact_mut(w)) {
        for (x, value) in out.iter_mut().enumerate() {
            if !inner.contains(&x) {
                let window = &line[x.saturating_sub(radius)..=(x + radius).min(w - 1)];
                *value = window.iter().fold(0.0, |total, v| total + v) / window.len() as f32;
            }
        }
        let full = &mut out[inner.clone()];
        for k in 0..=2 * radius {
            for (value, v) in full.iter_mut().zip(&line[k..]) {
                *value += v;
            }
        }
        for value in full {
            *value /= (2 * radius + 1) as f32;
        }
    }
    let mut out = vec![0f32; w * h];
    for (y, target) in out.chunks_exact_mut(w).enumerate() {
        let (from, to) = (y.saturating_sub(radius), (y + radius).min(h - 1));
        for row in rows[from * w..(to + 1) * w].chunks_exact(w) {
            for (value, other) in target.iter_mut().zip(row) {
                *value += other;
            }
        }
        let n = (to + 1 - from) as f32;
        for value in target.iter_mut() {
            *value /= n;
        }
    }
    out
}

/// One axis of a bilinear sample: the two neighbours of a coordinate clamped
/// to the image, and their weights.
#[derive(Clone, Copy)]
struct Lerp {
    from: usize,
    to: usize,
    keep: f32,
    take: f32,
}

impl Lerp {
    fn new(at: f32, len: usize) -> Lerp {
        let at = at.clamp(0.0, (len - 1) as f32);
        let from = at as i32;
        let take = at - from as f32;
        let from = from as usize;
        Lerp {
            from,
            to: (from + 1).min(len - 1),
            keep: 1.0 - take,
            take,
        }
    }

    fn table(count: usize, len: usize, at: impl Fn(usize) -> f32) -> Vec<Lerp> {
        (0..count).map(|i| Lerp::new(at(i), len)).collect()
    }

    fn apply(&self, line: &[f32]) -> f32 {
        line[self.from] * self.keep + line[self.to] * self.take
    }
}

fn bilinear(image: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let (across, down) = (Lerp::new(x, w), Lerp::new(y, h));
    let top = across.apply(&image[down.from * w..][..w]);
    let bottom = across.apply(&image[down.to * w..][..w]);
    top * down.keep + bottom * down.take
}

/// `value.max(floor)` for values that are never NaN: wasm32 has no instruction
/// for `f32::max`, which calls `fmaxf`, and a comparison vectorises.
fn at_least(value: f32, floor: f32) -> f32 {
    if value < floor {
        floor
    } else {
        value
    }
}

/// `value.min(ceiling)`, likewise.
fn at_most(value: f32, ceiling: f32) -> f32 {
    if value > ceiling {
        ceiling
    } else {
        value
    }
}

/// `value.round() as u8` without `roundf`, a library call on wasm32 as on
/// x86-64 without SSE4.1.
fn round_u8(value: f32) -> u8 {
    let whole = value as i32;
    let up = value - whole as f32 >= 0.5;
    whole.saturating_add(i32::from(up)).clamp(0, 255) as u8
}

/// A separable convolution by `weights`, centred, the edges repeated outward,
/// each output the weights' products summed in order.
fn convolve(image: &[f32], w: usize, h: usize, weights: &[f32]) -> Vec<f32> {
    let radius = weights.len() / 2;
    // One tap at a time over a whole line, so the loop runs across pixels.
    fn accumulate<'a>(target: &mut [f32], weights: &[f32], line: impl Fn(usize) -> &'a [f32]) {
        for (value, v) in target.iter_mut().zip(line(0)) {
            *value = v * weights[0];
        }
        for (k, weight) in weights.iter().enumerate().skip(1) {
            for (value, v) in target.iter_mut().zip(line(k)) {
                *value += v * weight;
            }
        }
    }
    let mut rows = vec![0f32; w * h];
    let mut padded = vec![0f32; w + 2 * radius];
    for (line, target) in image.chunks_exact(w).zip(rows.chunks_exact_mut(w)) {
        for (i, value) in padded.iter_mut().enumerate() {
            *value = line[i.saturating_sub(radius).min(w - 1)];
        }
        accumulate(target, weights, |k| &padded[k..][..w]);
    }
    let mut out = vec![0f32; w * h];
    for (y, target) in out.chunks_exact_mut(w).enumerate() {
        accumulate(target, weights, |k| {
            &rows[(y + k).saturating_sub(radius).min(h - 1) * w..][..w]
        });
    }
    out
}

/// The slope of the text lines, in radians, positive when they fall to the
/// right: the angle at which the ink's row profile is sharpest.
fn tilt(grey: &[u8], w: usize, h: usize) -> f64 {
    let ink: Vec<(f64, f64)> = (0..h)
        .step_by(2)
        .flat_map(|y| (0..w).step_by(2).map(move |x| (x, y)))
        .filter(|(x, y)| grey[y * w + x] < 128)
        .map(|(x, y)| (x as f64 - w as f64 / 2.0, y as f64 - h as f64 / 2.0))
        .collect();
    if ink.len() < 200 {
        return 0.0;
    }
    let reach = ((w * w + h * h) as f64).sqrt() as usize / 2 + 2;
    let sharpness = |degrees: f64| {
        let (cos, sin) = (degrees.to_radians().cos(), degrees.to_radians().sin());
        let mut rows = vec![0u32; reach * 2];
        for (x, y) in &ink {
            rows[((y * cos - x * sin) / 2.0 + reach as f64) as usize] += 1;
        }
        rows.windows(2)
            .map(|pair| (f64::from(pair[1]) - f64::from(pair[0])).powi(2))
            .sum::<f64>()
    };
    let best = |from: f64, to: f64, step: f64| {
        let steps = ((to - from) / step).round() as usize;
        (0..=steps)
            .map(|i| from + i as f64 * step)
            .map(|degrees| (degrees, sharpness(degrees)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0.0, |(degrees, _)| degrees)
    };
    let coarse = best(-MAX_TILT_DEGREES, MAX_TILT_DEGREES, 0.5);
    best(coarse - 0.5, coarse + 0.5, 0.1).to_radians()
}

/// The image turned by `-angle`, on a canvas grown to hold all of it.
fn turn(grey: &[u8], w: usize, h: usize, angle: f64) -> (Vec<u8>, usize, usize) {
    let (cos, sin) = (angle.cos(), angle.sin());
    let tw = (w as f64 * cos.abs() + h as f64 * sin.abs()).ceil() as usize;
    let th = (w as f64 * sin.abs() + h as f64 * cos.abs()).ceil() as usize;
    let mut out = vec![255u8; tw * th];
    let columns: Vec<(f64, f64)> = (0..tw)
        .map(|tx| {
            let dx = tx as f64 - tw as f64 / 2.0;
            (w as f64 / 2.0 + dx * cos, h as f64 / 2.0 + dx * sin)
        })
        .collect();
    let (right, bottom) = ((w - 1) as f64, (h - 1) as f64);
    for (ty, target) in out.chunks_exact_mut(tw).enumerate() {
        let dy = ty as f64 - th as f64 / 2.0;
        let (dy_sin, dy_cos) = (dy * sin, dy * cos);
        for (pixel, (x_part, y_part)) in target.iter_mut().zip(&columns) {
            let (sx, sy) = (x_part - dy_sin, y_part + dy_cos);
            if sx < 0.0 || sy < 0.0 || sx > right || sy > bottom {
                continue;
            }
            // Through i32, which both targets convert in one instruction.
            let (x0, y0) = (sx as i32, sy as i32);
            let (fx, fy) = (sx - f64::from(x0), sy - f64::from(y0));
            let (x0, y0) = (x0 as usize, y0 as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let at = |x: usize, y: usize| f64::from(grey[y * w + x]);
            let value = (at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx) * (1.0 - fy)
                + (at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx) * fy;
            let whole = value as i32;
            *pixel = (whole + i32::from(value - f64::from(whole) >= 0.5)) as u8;
        }
    }
    (out, tw, th)
}

/// Median height of the ink's connected pieces, specks under four pixels left
/// out: noise would otherwise pass for small print.
fn letter_height(grey: &[u8], w: usize, h: usize) -> Option<usize> {
    let mut parent: Vec<u32> = (0..(w * h) as u32).collect();
    fn root(parent: &mut [u32], mut at: u32) -> u32 {
        while parent[at as usize] != at {
            parent[at as usize] = parent[parent[at as usize] as usize];
            at = parent[at as usize];
        }
        at
    }
    let ink = |at: usize| grey[at] < 128;
    for y in 0..h {
        for x in 0..w {
            let at = y * w + x;
            if !ink(at) {
                continue;
            }
            for neighbour in [(x > 0).then(|| at - 1), (y > 0).then(|| at - w)]
                .into_iter()
                .flatten()
            {
                if ink(neighbour) {
                    let (a, b) = (
                        root(&mut parent, at as u32),
                        root(&mut parent, neighbour as u32),
                    );
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        }
    }
    // A piece's root is its first pixel, so its top row: point every pixel
    // straight at its root, then let the root hold the piece's last row.
    for at in 0..w * h {
        if ink(at) {
            parent[at] = root(&mut parent, at as u32);
        }
    }
    let mut roots = Vec::new();
    for at in (0..w * h).filter(|at| ink(*at)) {
        let piece = parent[at] as usize;
        if piece == at {
            roots.push(at);
        }
        parent[piece] = (at / w) as u32;
    }
    let mut heights: Vec<usize> = roots
        .iter()
        .map(|root| parent[*root] as usize - root / w + 1)
        .filter(|height| (4..=h / 10).contains(height))
        .collect();
    if heights.is_empty() {
        return None;
    }
    let middle = heights.len() / 2;
    Some(*heights.select_nth_unstable(middle).1)
}

/// A Gaussian blur of standard deviation `sigma`, the edges repeated outward.
fn gaussian(image: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil() as usize;
    let weights: Vec<f32> = (0..=2 * radius)
        .map(|i| (-(i as f32 - radius as f32).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    let weights: Vec<f32> = weights.iter().map(|weight| weight / total).collect();
    convolve(image, w, h, &weights)
}

fn double(grey: &[u8], w: usize, h: usize) -> Vec<u8> {
    let source: Vec<f32> = grey.iter().map(|v| f32::from(*v)).collect();
    let columns = Lerp::table(w * 2, w, |x| (x as f32 - 0.5) / 2.0);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h * 2 {
        let down = Lerp::new((y as f32 - 0.5) / 2.0, h);
        let (top, bottom) = (&source[down.from * w..][..w], &source[down.to * w..][..w]);
        out.extend(columns.iter().map(|across| {
            round_u8(across.apply(top) * down.keep + across.apply(bottom) * down.take)
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of dark bars standing for lines of text, drawn tilted by
    /// `degrees` under a light falling off to the right.
    fn page(degrees: f64, letter: usize) -> (Vec<u8>, u32, u32) {
        let (w, h) = (600usize, 800usize);
        let slope = degrees.to_radians().tan();
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let paper = 235.0 - 110.0 * x as f64 / w as f64;
                let line =
                    (y as f64 - 100.0 - (x as f64 - 300.0) * slope).rem_euclid(letter as f64 * 3.0);
                let inked =
                    x > 60 && x < 540 && line < letter as f64 && (x / (letter + 2)) % 3 != 2;
                let value = if inked { 40.0 } else { paper } as u8;
                rgba.extend([value, value, value, 255]);
            }
        }
        (rgba, w as u32, h as u32)
    }

    #[test]
    fn a_tilted_page_under_uneven_light_comes_out_straight_on_white() {
        let (rgba, w, h) = page(3.0, 12);
        let prepared = prepare(&rgba, w, h);
        assert!(
            (prepared.angle.to_degrees() - 3.0).abs() < 0.2,
            "{}",
            prepared.angle.to_degrees()
        );
        assert_eq!(prepared.scale, 1.0);
        assert!(prepared.page.is_none());
        let paper = prepared.grey[prepared.width as usize * 30 + prepared.width as usize - 80];
        assert!(paper > 240, "{paper}");
        let [x, y, width, height] =
            prepared.source_box(0.0, 0.0, prepared.width as f64, prepared.height as f64);
        assert!(x < 0.5 && y < 0.5 && x + width > 599.5 && y + height > 799.5);
    }

    #[test]
    fn a_page_with_almost_no_ink_keeps_white_paper_and_black_ink() {
        let (w, h) = (600usize, 800usize);
        let mut rgba = vec![255u8; w * h * 4];
        for y in 100..112 {
            for x in (60..400).filter(|x| x % 7 < 4) {
                rgba[(y * w + x) * 4..][..3].fill(0);
            }
        }
        let prepared = prepare(&rgba, w as u32, h as u32);
        assert_eq!(prepared.grey[w * 400 + 300], 255);
        assert!(prepared.grey[w * 105 + 64] < 32);
    }

    #[test]
    fn small_print_is_enlarged_and_its_boxes_come_back_to_the_source() {
        let (rgba, w, h) = page(0.0, 5);
        let prepared = prepare(&rgba, w, h);
        assert_eq!(
            (prepared.scale, prepared.width, prepared.height),
            (2.0, 1200, 1600)
        );
        let [x, y, width, height] = prepared.source_box(200.0, 400.0, 40.0, 20.0);
        assert!((x - 100.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9);
        assert!((width - 20.0).abs() < 1e-9 && (height - 10.0).abs() < 1e-9);
    }

    /// A page of text bars photographed on a dark table: `corners` are where
    /// the page's corners land on the photo, clockwise from the top left.
    fn photo(corners: [(f64, f64); 4]) -> (Vec<u8>, u32, u32) {
        let (w, h) = (760usize, 960usize);
        let [a, b, c, d, e, f, g, k] = square_to(corners);
        let inverse = [
            e - f * k,
            c * k - b,
            b * f - c * e,
            f * g - d,
            a - c * g,
            c * d - a * f,
            d * k - e * g,
            b * g - a * k,
            a * e - b * d,
        ];
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let t = inverse[6] * px + inverse[7] * py + inverse[8];
                let u = (inverse[0] * px + inverse[1] * py + inverse[2]) / t;
                let v = (inverse[3] * px + inverse[4] * py + inverse[5]) / t;
                let value = match (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                    false => 60 + (x * 7 + y * 3) % 30,
                    true => {
                        let (px, py) = ((u * 600.0) as usize, (v * 800.0) as usize);
                        let inked = px > 60 && px < 540 && py > 80 && py < 720;
                        match inked && (py % 36) < 12 && (px / 14) % 3 != 2 {
                            true => 40,
                            false => 225 - (u * 60.0) as usize,
                        }
                    }
                };
                rgba.extend([value as u8, value as u8, value as u8, 255]);
            }
        }
        (rgba, w as u32, h as u32)
    }

    #[test]
    fn a_photographed_page_is_cut_out_of_the_table_and_its_boxes_come_back_to_the_photo() {
        let corners = [(90.0, 60.0), (680.0, 95.0), (715.0, 905.0), (45.0, 880.0)];
        let square = square_to(corners);
        let at = |u: f64, v: f64| {
            let t = square[6] * u + square[7] * v + 1.0;
            (
                (square[0] * u + square[1] * v + square[2]) / t,
                (square[3] * u + square[4] * v + square[5]) / t,
            )
        };
        for ((u, v), corner) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .into_iter()
            .zip(corners)
        {
            let (x, y) = at(u, v);
            assert!((x - corner.0).abs() < 1e-9 && (y - corner.1).abs() < 1e-9);
        }

        let (rgba, w, h) = photo(corners);
        let prepared = prepare(&rgba, w, h);
        assert!(prepared.page.is_some());
        let (pw, ph) = (prepared.width as f64, prepared.height as f64);
        assert!(
            (pw - 620.0).abs() < 20.0 && (ph - 815.0).abs() < 20.0,
            "{pw} {ph}"
        );
        let border: Vec<u8> = (0..prepared.width as usize)
            .flat_map(|x| {
                [
                    x,
                    (prepared.height as usize - 1) * prepared.width as usize + x,
                ]
            })
            .chain((0..prepared.height as usize).flat_map(|y| {
                [
                    y * prepared.width as usize,
                    (y + 1) * prepared.width as usize - 1,
                ]
            }))
            .map(|at| prepared.grey[at])
            .collect();
        let dark = border.iter().filter(|v| **v < 128).count();
        assert!(dark * 100 < border.len(), "{dark} of {}", border.len());

        let near = |(x, y): (f64, f64), (ex, ey): (f64, f64), within: f64| {
            assert!(
                (x - ex).hypot(y - ey) < within,
                "({x}, {y}) for ({ex}, {ey})"
            );
        };
        let [x, y, _, _] = prepared.source_box(0.0, 0.0, 0.0, 0.0);
        near((x, y), corners[0], 12.0);
        let [x, y, _, _] = prepared.source_box(pw, ph, 0.0, 0.0);
        near((x, y), corners[2], 12.0);
        let [x, y, _, _] = prepared.source_box(pw / 2.0, ph / 2.0, 0.0, 0.0);
        near((x, y), at(0.5, 0.5), 2.0);
    }

    #[test]
    fn a_black_and_white_page_is_softened_at_its_own_size() {
        let (w, h) = (600usize, 800usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|at| {
                let (x, y) = (at % w, at / w);
                let inked = x > 60 && x < 540 && (y % 36) < 12 && (x / 14) % 3 != 2;
                let value = if inked { 0 } else { 255 };
                [value, value, value, 255]
            })
            .collect();
        let prepared = prepare(&rgba, w as u32, h as u32);
        assert_eq!((prepared.width, prepared.height), (600, 800));
        let greys = prepared
            .grey
            .iter()
            .filter(|v| (32..224).contains(*v))
            .count();
        assert!(greys * 100 > prepared.grey.len(), "{greys}");
        assert_eq!(
            prepared.source_box(100.0, 40.0, 30.0, 12.0),
            [100.0, 40.0, 30.0, 12.0]
        );

        let blank = prepare(&vec![255; w * h * 4], w as u32, h as u32);
        assert_eq!((blank.width, blank.height), (600, 800));
    }
}
