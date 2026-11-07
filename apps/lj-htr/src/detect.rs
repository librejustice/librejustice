//! Where the lines of text lie on a page: the detector's probability map, cut
//! into boxes by the differentiable binarisation post-processing.

use serde::{Deserialize, Serialize};

/// One run of text the detector found, on the page's pixels, and the line of
/// the page it belongs to.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Segment {
    pub line: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Probability above which a pixel is text.
const THRESHOLD: f32 = 0.3;
/// Mean probability over its pixels below which a region is dropped. Taken
/// over its box instead, a slanted handwritten line would fall below it.
const BOX_THRESHOLD: f32 = 0.5;
/// The detector draws the core of a line; its box grows by the area times
/// this over the perimeter to take the whole letters back.
const UNCLIP: f32 = 1.6;
/// Thinner regions, in pixels of the map, are noise.
const MIN_SIDE: usize = 3;

/// The long and the short side of the map a page is scaled to fit: a DIN page
/// at 150 dpi, where the detector's cost is the map's area.
pub const LONG: usize = 1760;
pub const SHORT: usize = 1248;

/// The map's size for a page, portrait or landscape, and the page's scale on it.
pub fn canvas(width: usize, height: usize) -> (usize, usize, f32) {
    let (w, h) = match width > height {
        true => (LONG, SHORT),
        false => (SHORT, LONG),
    };
    let scale = (w as f32 / width as f32).min(h as f32 / height as f32);
    (w, h, scale)
}

/// Map rows a band reads past its own on each side, so that a line its edge
/// cuts is still seen whole.
pub const OVERLAP: usize = 96;

/// Band `index` of `count` over a map `h` rows high: the rows it answers for,
/// the row its input starts at, above the map for the first, and its height,
/// the same for every band and a multiple of the detector's stride of 32.
pub fn band(h: usize, index: usize, count: usize) -> (std::ops::Range<usize>, isize, usize) {
    if count == 1 {
        return (0..h, 0, h);
    }
    let own = h.div_ceil(count);
    let rows = (own + 2 * OVERLAP).next_multiple_of(32);
    let core = (index * own).min(h)..((index + 1) * own).min(h);
    (core.clone(), core.start as isize - OVERLAP as isize, rows)
}

/// The detector's input for band `index` of `count`: the page scaled onto the
/// map, padded with white, its grey repeated on three channels, each
/// normalised to [-1, 1]. Also the map's width, and the band's rows.
pub fn input(
    grey: &[u8],
    width: usize,
    index: usize,
    count: usize,
) -> (Vec<f32>, usize, usize, std::ops::Range<usize>, usize) {
    let height = grey.len() / width;
    let (w, h, scale) = canvas(width, height);
    let plane = crate::image::scaled(
        grey,
        width,
        [0.0, 0.0, width as f32, height as f32],
        ((width as f32 * scale).round() as usize).clamp(1, w),
        ((height as f32 * scale).round() as usize).clamp(1, h),
        w,
        h,
    );
    let (core, start, rows) = band(h, index, count);
    let mut one = vec![1.0; w * rows];
    for (row, out) in one.chunks_exact_mut(w).enumerate() {
        let y = start + row as isize;
        if (0..h as isize).contains(&y) {
            let y = y as usize;
            for (to, v) in out.iter_mut().zip(&plane[y * w..(y + 1) * w]) {
                *to = v / 127.5 - 1.0;
            }
        }
    }
    let skip = (core.start as isize - start) as usize;
    (one.repeat(3), w, rows, core, skip)
}

/// The boxes of text on the map, brought back to the page by `scale`, each
/// given the line it reads on, top to bottom.
pub fn segments(map: &[f32], w: usize, h: usize, scale: f32) -> Vec<Segment> {
    let mut on = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let hit =
                |dx: usize, dy: usize| x >= dx && y >= dy && map[(y - dy) * w + x - dx] > THRESHOLD;
            on[y * w + x] = hit(0, 0) || hit(1, 0) || hit(0, 1) || hit(1, 1);
        }
    }
    let mut seen = vec![false; w * h];
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !on[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut sum, mut count) = (0.0, 0);
        seen[start] = true;
        stack.push(start);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % w, at / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            sum += map[at];
            count += 1;
            for (nx, ny) in neighbours(x, y, w, h) {
                let next = ny * w + nx;
                if on[next] && !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        if bw.min(bh) < MIN_SIDE {
            continue;
        }
        if sum / (count as f32) < BOX_THRESHOLD {
            continue;
        }
        let (bw, bh) = (bw as f32, bh as f32);
        let grow = bw * bh * UNCLIP / (2.0 * (bw + bh));
        let x = (x0 as f32 - grow).max(0.0);
        let y = (y0 as f32 - grow).max(0.0);
        let right = (x1 as f32 + 1.0 + grow).min(w as f32);
        let bottom = (y1 as f32 + 1.0 + grow).min(h as f32);
        found.push(Segment {
            line: 0,
            x: x / scale,
            y: y / scale,
            width: (right - x) / scale,
            height: (bottom - y) / scale,
        });
    }
    lines(found)
}

fn neighbours(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    (-1i64..=1)
        .flat_map(|dy| (-1i64..=1).map(move |dx| (dx, dy)))
        .filter(|&d| d != (0, 0))
        .map(move |(dx, dy)| (x as i64 + dx, y as i64 + dy))
        .filter(move |&(nx, ny)| nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64)
        .map(|(nx, ny)| (nx as usize, ny as usize))
}

/// Segments whose middles lie within half a height of each other read on one
/// line; lines are numbered top to bottom, segments left to right.
fn lines(mut segments: Vec<Segment>) -> Vec<Segment> {
    let middle = |s: &Segment| s.y + s.height / 2.0;
    segments.sort_by(|a, b| middle(a).total_cmp(&middle(b)));
    let mut line = 0;
    let mut level: Option<f32> = None;
    for segment in segments.iter_mut() {
        let at = middle(segment);
        match level {
            Some(level) if (at - level).abs() < segment.height / 2.0 => {}
            Some(_) => {
                line += 1;
                level = Some(at);
            }
            None => level = Some(at),
        }
        segment.line = line;
    }
    segments.sort_by(|a, b| a.line.cmp(&b.line).then(a.x.total_cmp(&b.x)));
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(w: usize, h: usize, boxes: &[(usize, usize, usize, usize, f32)]) -> Vec<f32> {
        let mut map = vec![0.0; w * h];
        for &(x, y, bw, bh, p) in boxes {
            for yy in y..y + bh {
                for xx in x..x + bw {
                    map[yy * w + xx] = p;
                }
            }
        }
        map
    }

    #[test]
    fn a_region_grows_back_to_its_letters_and_returns_to_the_page() {
        let map = map_with(100, 40, &[(10, 10, 40, 6, 0.9)]);
        let found = segments(&map, 100, 40, 0.5);
        assert_eq!(found.len(), 1);
        let s = &found[0];
        // 41 x 7 once dilated, scored 0.9 * 240 / 287.
        let grow = 41.0 * 7.0 * UNCLIP / 96.0;
        assert!((s.x - (10.0 - grow) / 0.5).abs() < 1e-3);
        assert!((s.width - (41.0 + 2.0 * grow) / 0.5).abs() < 1e-3);
        assert!((s.height - (7.0 + 2.0 * grow) / 0.5).abs() < 1e-3);
    }

    #[test]
    fn bands_tile_the_map_and_share_one_planned_height() {
        for (h, count) in [(LONG, 1), (LONG, 3), (LONG, 4), (SHORT, 4)] {
            let bands: Vec<_> = (0..count).map(|i| band(h, i, count)).collect();
            assert_eq!(bands[0].0.start, 0);
            assert_eq!(bands[count - 1].0.end, h);
            assert!(bands.windows(2).all(|b| b[0].0.end == b[1].0.start));
            for (core, start, rows) in &bands {
                assert_eq!(rows % 32, 0);
                assert!(*start <= core.start as isize);
                assert!(*start + *rows as isize >= core.end as isize);
            }
        }
    }

    #[test]
    fn faint_and_thin_regions_are_dropped() {
        let map = map_with(100, 40, &[(10, 5, 40, 6, 0.35), (10, 20, 40, 1, 0.9)]);
        assert!(segments(&map, 100, 40, 1.0).is_empty());
    }

    #[test]
    fn segments_at_one_height_share_a_line_left_to_right() {
        let map = map_with(
            200,
            60,
            &[
                (120, 10, 40, 6, 0.9),
                (10, 11, 40, 6, 0.9),
                (10, 40, 40, 6, 0.9),
            ],
        );
        let found = segments(&map, 200, 60, 1.0);
        let order: Vec<(usize, f32)> = found.iter().map(|s| (s.line, s.x.round())).collect();
        assert_eq!(order.len(), 3);
        assert_eq!((order[0].0, order[1].0, order[2].0), (0, 0, 1));
        assert!(order[0].1 < order[1].1);
    }
}
