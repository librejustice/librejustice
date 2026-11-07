//! Line boxes as LibreOffice measures them, in whole twips.

use crate::fonts::Metrics;
use crate::model::LineSpacing;

/// A line's height and the distance from its top to its baseline, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineBox {
    pub ascent: f64,
    pub height: f64,
}

impl LineBox {
    pub fn descent(&self) -> f64 {
        self.height - self.ascent
    }

    /// The box holding both.
    pub fn union(self, other: LineBox) -> LineBox {
        let ascent = self.ascent.max(other.ascent);
        LineBox {
            ascent,
            height: ascent + self.descent().max(other.descent()),
        }
    }
}

pub(crate) fn twips(points: f64) -> f64 {
    (points * 20.0).round()
}

/// The box a face gives a line at `size`. The font is measured on a 8640 dpi
/// reference device, then ascent and descent are rounded to twips together
/// and the leading apart, the leading going above the ascent: pitch and first
/// baseline match LibreOffice's to the twip. Without `leading`, the face's line
/// gap is dropped.
pub(crate) fn font_box(metrics: &Metrics, size: f64, leading: bool) -> LineBox {
    let em = (size * 120.0).round();
    let upm = f64::from(metrics.units_per_em);
    let device = |v: i32| (em * f64::from(v) / upm).round();
    let (ascent, descent, leading) = (
        device(metrics.ascent),
        device(metrics.descent),
        if leading {
            device(metrics.line_gap.max(0))
        } else {
            0.0
        },
    );
    let to_twips = |units: f64| (units / 6.0).round();
    LineBox {
        ascent: (to_twips(leading) + to_twips(ascent)) / 20.0,
        height: (to_twips(ascent + descent) + to_twips(leading)) / 20.0,
    }
}

/// A line box once the paragraph's line spacing applies. A proportional
/// spacing above single adds below the text; below single, and an exact
/// spacing, give the ascent four fifths of the box; an at-least spacing adds
/// above.
pub(crate) fn spaced(natural: LineBox, rule: LineSpacing) -> LineBox {
    match rule {
        LineSpacing::Proportional(percent) => {
            let percent = match percent {
                p if p <= 0.0 => 100.0,
                p if p < 50.0 => 50.0,
                p => p,
            };
            if percent == 100.0 {
                return natural;
            }
            let height = (twips(natural.height) * percent / 100.0).floor();
            let ascent = if percent < 100.0 {
                (4.0 * height / 5.0).floor() / 20.0
            } else {
                natural.ascent
            };
            LineBox {
                ascent,
                height: height / 20.0,
            }
        }
        LineSpacing::Exact(value) => {
            let height = twips(value);
            LineBox {
                ascent: (4.0 * height / 5.0).floor() / 20.0,
                height: height / 20.0,
            }
        }
        LineSpacing::AtLeast(value) if value > natural.height => LineBox {
            ascent: natural.ascent + value - natural.height,
            height: value,
        },
        LineSpacing::AtLeast(_) => natural,
    }
}

/// A line on a grid of `pitch`, as LibreOffice sets the body of a page that
/// has one: as many pitches as the text needs, or the exact spacing, the
/// text centred in them. A proportional spacing above single adds its share
/// of the text below; an at-least spacing is ignored.
pub(crate) fn gridded(natural: LineBox, rule: LineSpacing, pitch: f64) -> LineBox {
    let height = match rule {
        LineSpacing::Exact(value) => value,
        _ => (natural.height / pitch - 1e-6).ceil().max(1.0) * pitch,
    };
    let extra = match rule {
        LineSpacing::Proportional(percent) if percent > 100.0 => {
            (twips(natural.height) * (percent / 100.0 - 1.0)).floor() / 20.0
        }
        _ => 0.0,
    };
    LineBox {
        ascent: natural.ascent + ((twips(height) - twips(natural.height)) / 2.0).floor() / 20.0,
        height: height + extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carlito at 11 pt, whose single line LibreOffice sets 13.45 pt high
    /// with the baseline 10.7 pt down.
    fn carlito() -> LineBox {
        let metrics = Metrics {
            units_per_em: 2048,
            ascent: 1536,
            descent: 512,
            line_gap: 452,
            underline_position: 0,
            underline_thickness: 1,
            strikeout_position: 0,
            strikeout_size: 1,
        };
        font_box(&metrics, 11.0, true)
    }

    fn close(a: LineBox, ascent: f64, height: f64) {
        assert!(
            (a.ascent - ascent).abs() < 1e-9 && (a.height - height).abs() < 1e-9,
            "{a:?} is not {ascent} / {height}"
        );
    }

    #[test]
    fn a_single_line_is_measured_in_twips() {
        close(carlito(), 10.7, 13.45);
    }

    #[test]
    fn line_spacing_lands_where_libreoffice_sets_it() {
        let single = carlito();
        close(
            spaced(single, LineSpacing::Proportional(150.0)),
            10.7,
            20.15,
        );
        close(spaced(single, LineSpacing::Proportional(200.0)), 10.7, 26.9);
        close(spaced(single, LineSpacing::Proportional(80.0)), 8.6, 10.75);
        close(spaced(single, LineSpacing::Exact(18.0)), 14.4, 18.0);
        close(spaced(single, LineSpacing::Exact(10.0)), 8.0, 10.0);
        close(spaced(single, LineSpacing::AtLeast(20.0)), 17.25, 20.0);
        close(spaced(single, LineSpacing::AtLeast(5.0)), 10.7, 13.45);
    }

    #[test]
    fn a_line_grid_centres_the_text_in_whole_pitches_as_libreoffice_does() {
        let single = carlito();
        let auto = LineSpacing::Proportional(100.0);
        close(gridded(single, auto, 15.6), 11.75, 15.6);
        close(gridded(single, auto, 18.0), 12.95, 18.0);
        close(gridded(single, auto, 12.0), 15.95, 24.0);
        close(
            gridded(single, LineSpacing::Proportional(150.0), 15.6),
            11.75,
            22.3,
        );
        close(gridded(single, LineSpacing::Exact(15.0), 15.6), 11.45, 15.0);
    }
}
