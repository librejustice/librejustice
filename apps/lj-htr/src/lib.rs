//! Reads a page written by hand, or printed and scanned too poorly for the fast
//! reader: a text detector, a line reader, and a handwriting reader.

mod detect;
mod image;
mod model;
mod read;

pub use detect::Segment;
pub use read::{Line, Word};

use model::Network;
use read::{Alphabet, Lexicon, Lexicons, Reading};
use tract_onnx::prelude::*;
use wasm_bindgen::prelude::*;

/// The line reader's input height, and the widths it is planned for. A wider
/// line is squeezed into the last.
const LINE_HEIGHT: usize = 48;
const LINE_WIDTHS: [usize; 5] = [320, 640, 960, 1280, 1920];

const HAND_HEIGHT: usize = 128;
const HAND_WIDTHS: [usize; 5] = [512, 1024, 2048, 3072, 4096];
/// Lines the handwriting reader takes at once, padded with blank ones. Its
/// LSTM runs a batch of one far slower a line in WASM.
const HAND_BATCH: usize = 4;

pub struct Reader {
    detector: Network,
    lines: Network,
    hand: Network,
    printed: Alphabet,
    written: Alphabet,
    lexicons: Lexicons,
}

/// A line resampled to a reader's height: its width, and the planned width it
/// is padded to.
fn rect(s: &Segment) -> [f32; 4] {
    [s.x, s.y, s.width, s.height]
}

fn fit(segment: &Segment, height: usize, widths: &[usize]) -> (usize, usize) {
    let last = widths[widths.len() - 1];
    let seen = ((segment.width * height as f32 / segment.height).round() as usize).clamp(1, last);
    let wide = widths.iter().copied().find(|&w| w >= seen).unwrap_or(last);
    (seen, wide)
}

impl Reader {
    pub fn new(
        detector: &[u8],
        lines: &[u8],
        dictionary: &str,
        hand: &[u8],
        table: &str,
        french: &str,
        others: &[String],
    ) -> TractResult<Reader> {
        Ok(Reader {
            detector: Network::parse(detector)?,
            lines: Network::parse(lines)?,
            hand: Network::parse(hand)?,
            printed: Alphabet::paddle(dictionary),
            written: Alphabet::laia(table),
            lexicons: Lexicons {
                french: Lexicon::parse(french),
                others: others.iter().map(|l| Lexicon::parse(l)).collect(),
            },
        })
    }

    /// The detector's map over band `index` of `count` of a grey page `width`
    /// pixels wide: the rows that band answers for, the map's full width each.
    pub fn band(
        &mut self,
        grey: &[u8],
        width: usize,
        index: usize,
        count: usize,
    ) -> TractResult<Vec<f32>> {
        let (input, w, rows, core, skip) = detect::input(grey, width, index, count);
        let map = self.detector.run(&[1, 3, rows, w], input)?;
        Ok(map.as_slice::<f32>()?[skip * w..(skip + core.len()) * w].to_vec())
    }

    /// The runs of text on the map of a page `width` by `height`, its bands
    /// laid end to end.
    pub fn segments(&self, map: &[f32], width: usize, height: usize) -> Vec<Segment> {
        let (w, h, scale) = detect::canvas(width, height);
        detect::segments(map, w, h, scale)
    }

    /// The lines of `segments` on the grey page `width` pixels wide, read by
    /// the line reader.
    pub fn printed(
        &mut self,
        grey: &[u8],
        width: usize,
        segments: &[Segment],
    ) -> TractResult<Vec<Line>> {
        let mut read = Vec::with_capacity(segments.len());
        for segment in segments {
            let (seen, wide) = fit(segment, LINE_HEIGHT, &LINE_WIDTHS);
            let plane = image::scaled(
                grey,
                width,
                rect(segment),
                seen,
                LINE_HEIGHT,
                seen,
                LINE_HEIGHT,
            );
            // Padded with the zero it was trained on, a mid grey once normalised.
            let mut input = vec![0.0; 3 * LINE_HEIGHT * wide];
            for channel in 0..3 {
                for (y, row) in plane.chunks_exact(seen).enumerate() {
                    let at = (channel * LINE_HEIGHT + y) * wide;
                    for (to, v) in input[at..at + seen].iter_mut().zip(row) {
                        *to = v / 127.5 - 1.0;
                    }
                }
            }
            let out = self.lines.run(&[1, 3, LINE_HEIGHT, wide], input)?;
            let classes = out.shape()[2];
            let reading = Reading::decode(out.as_slice::<f32>()?, classes);
            read.push(Line {
                confidence: reading.confidence(),
                printed: reading.words(&self.printed, segment, seen as f32, wide as f32),
                hand: None,
            });
        }
        Ok(read)
    }

    /// The lines the handwriting reader reads again, by batches of lines of
    /// about one width: a batch is padded to its widest line.
    pub fn batches(&self, segments: &[Segment], lines: &[Line]) -> Vec<Vec<usize>> {
        let mut doubtful: Vec<usize> = (0..lines.len())
            .filter(|&i| read::doubtful(&lines[i]))
            .collect();
        doubtful.sort_by_key(|&i| fit(&segments[i], HAND_HEIGHT, &HAND_WIDTHS).0);
        doubtful.chunks(HAND_BATCH).map(<[usize]>::to_vec).collect()
    }

    /// `segments`, a batch at most, read by the handwriting reader.
    pub fn hand(
        &mut self,
        grey: &[u8],
        width: usize,
        segments: &[Segment],
    ) -> TractResult<Vec<Vec<Word>>> {
        let fits: Vec<(usize, usize)> = segments
            .iter()
            .map(|s| fit(s, HAND_HEIGHT, &HAND_WIDTHS))
            .collect();
        let wide = fits.iter().map(|f| f.1).max().unwrap_or(HAND_WIDTHS[0]);
        let mut input = vec![0.0; HAND_BATCH * HAND_HEIGHT * wide];
        for (slot, (segment, &(seen, _))) in segments.iter().zip(&fits).enumerate() {
            let plane = image::scaled(
                grey,
                width,
                rect(segment),
                seen,
                HAND_HEIGHT,
                seen,
                HAND_HEIGHT,
            );
            for (y, row) in plane.chunks_exact(seen).enumerate() {
                let at = (slot * HAND_HEIGHT + y) * wide;
                for (to, v) in input[at..at + seen].iter_mut().zip(row) {
                    *to = 1.0 - v / 255.0;
                }
            }
        }
        let out = self.hand.run(&[HAND_BATCH, 1, HAND_HEIGHT, wide], input)?;
        let (frames, classes) = (out.shape()[0], out.shape()[2]);
        let probs = out.as_slice::<f32>()?;
        Ok(segments
            .iter()
            .zip(&fits)
            .enumerate()
            .map(|(slot, (segment, &(seen, _)))| {
                let own: Vec<f32> = (0..frames)
                    .flat_map(|t| {
                        let at = (t * HAND_BATCH + slot) * classes;
                        probs[at..at + classes].iter().copied()
                    })
                    .collect();
                Reading::decode(&own, classes).words(
                    &self.written,
                    segment,
                    seen as f32,
                    wide as f32,
                )
            })
            .collect())
    }

    /// The words of a whole page's lines, each read by the reader it trusts.
    pub fn page(&self, lines: Vec<Line>) -> Vec<Word> {
        read::page(&self.lexicons, lines)
    }
}

fn fail(e: impl std::fmt::Display) -> JsError {
    JsError::new(&format!("{e:#}"))
}

/// The reader as a worker holds it.
#[wasm_bindgen]
pub struct Htr(Reader);

#[wasm_bindgen]
impl Htr {
    #[wasm_bindgen(constructor)]
    pub fn new(
        detector: &[u8],
        lines: &[u8],
        dictionary: &str,
        hand: &[u8],
        table: &str,
        french: &str,
        others: JsValue,
    ) -> Result<Htr, JsError> {
        let others: Vec<String> = serde_wasm_bindgen::from_value(others).map_err(fail)?;
        Reader::new(detector, lines, dictionary, hand, table, french, &others)
            .map(Htr)
            .map_err(fail)
    }

    pub fn band(
        &mut self,
        grey: &[u8],
        width: usize,
        index: usize,
        count: usize,
    ) -> Result<Vec<f32>, JsError> {
        self.0.band(grey, width, index, count).map_err(fail)
    }

    pub fn segments(&self, map: &[f32], width: usize, height: usize) -> Result<JsValue, JsError> {
        serde_wasm_bindgen::to_value(&self.0.segments(map, width, height)).map_err(fail)
    }

    pub fn printed(
        &mut self,
        grey: &[u8],
        width: usize,
        segments: JsValue,
    ) -> Result<JsValue, JsError> {
        let segments: Vec<Segment> = serde_wasm_bindgen::from_value(segments).map_err(fail)?;
        let found = self.0.printed(grey, width, &segments).map_err(fail)?;
        serde_wasm_bindgen::to_value(&found).map_err(fail)
    }

    pub fn batches(&self, segments: JsValue, lines: JsValue) -> Result<JsValue, JsError> {
        let segments: Vec<Segment> = serde_wasm_bindgen::from_value(segments).map_err(fail)?;
        let lines: Vec<Line> = serde_wasm_bindgen::from_value(lines).map_err(fail)?;
        serde_wasm_bindgen::to_value(&self.0.batches(&segments, &lines)).map_err(fail)
    }

    pub fn hand(
        &mut self,
        grey: &[u8],
        width: usize,
        segments: JsValue,
    ) -> Result<JsValue, JsError> {
        let segments: Vec<Segment> = serde_wasm_bindgen::from_value(segments).map_err(fail)?;
        let found = self.0.hand(grey, width, &segments).map_err(fail)?;
        serde_wasm_bindgen::to_value(&found).map_err(fail)
    }

    pub fn page(&self, lines: JsValue) -> Result<JsValue, JsError> {
        let lines: Vec<Line> = serde_wasm_bindgen::from_value(lines).map_err(fail)?;
        serde_wasm_bindgen::to_value(&self.0.page(lines)).map_err(fail)
    }
}
