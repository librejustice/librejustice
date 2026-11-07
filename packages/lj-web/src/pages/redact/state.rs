//! What the workbench holds, in plain data: the same signals serve a PDF, a
//! word-processing document and an image, so one interface renders the three.

use std::collections::HashMap;

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Pdf,
    /// `.docx` or `.doc`.
    Word,
    OpenDocument,
    Image,
}

impl Format {
    /// What the chip in the toolbar says.
    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Word => "Document Word",
            Format::OpenDocument => "OpenDocument",
            Format::Image => "Image",
        }
    }

    /// Laid out by the office module, whose pages carry their own text.
    pub fn laid_out(self) -> bool {
        matches!(self, Format::Word | Format::OpenDocument)
    }

    /// Pages drawn as the SVG their module writes, so a zoom only resizes
    /// their box.
    pub fn drawn_as_svg(self) -> bool {
        matches!(self, Format::Pdf | Format::Word | Format::OpenDocument)
    }

    /// Pages a rectangle can blacken and a reader can read. A laid-out
    /// document has no pixels to rewrite, only characters.
    pub fn has_pixels(self) -> bool {
        matches!(self, Format::Pdf | Format::Image)
    }

    /// PDF user space counts upwards from the bottom left; an image and a
    /// laid-out page count downwards from the top left.
    pub fn flips_y(self) -> bool {
        matches!(self, Format::Pdf)
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub enum Status {
    #[default]
    Empty,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Pages,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Read,
    Draw,
}

/// Which reader reads pages with no text of their own: Tesseract, or the
/// slower one that also reads handwriting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eye {
    Fast,
    Slow,
}

/// What the text reader is doing, stage and progress, or what stopped it.
#[derive(Clone, PartialEq, Debug)]
pub enum Reading {
    Stage(String, f64),
    Failed(String),
}

/// Whether every file the tool may need is held in the page, so the network
/// can be cut before a document is dropped.
#[derive(Clone, PartialEq, Debug, Default)]
pub enum Offline {
    #[default]
    Online,
    Loading(f64),
    Ready,
    Failed(String),
}

/// One string to remove. `kind` is where it came from: a rule, or the user.
/// `label` is what the user set in its place, `None` keeping the computed one;
/// empty erases the string.
#[derive(Clone, PartialEq, Debug)]
pub struct Needle {
    pub text: String,
    pub kind: String,
    pub on: bool,
    pub label: Option<String>,
}

/// One located occurrence, in the coordinates the viewer draws on.
#[derive(Clone, PartialEq, Debug)]
pub struct Hit {
    pub needle: usize,
    pub page: Option<usize>,
    pub quads: Vec<[[f64; 2]; 4]>,
    pub start: usize,
    pub end: usize,
}

/// Hits alongside the list they were searched with, so `Hit::needle` always
/// resolves even after the user edits the list.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Hits {
    pub needles: Vec<String>,
    pub hits: Vec<Hit>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One word of a drawn page, boxed in page space as `[x0, y0, x1, y1]`.
#[derive(Clone, PartialEq, Debug, serde::Deserialize)]
pub struct PageWord {
    pub text: String,
    pub bounds: [f64; 4],
}

#[derive(Clone, PartialEq, Debug)]
pub struct TextBlock {
    pub start: usize,
    pub text: String,
}

/// What one pass produced. `leaks` non-empty means nothing is handed over.
#[derive(Clone, PartialEq, Debug)]
pub struct Verdict {
    pub found: usize,
    pub removed: usize,
    pub leaks: Vec<String>,
    pub size: usize,
    /// How the pass was run, which reopening the result would otherwise erase.
    pub by_eye: bool,
    pub check: Check,
}

/// Where the re-reading of pages blackened by eye stands. It runs behind the
/// result, which is shown and handed over before it ends.
#[derive(Clone, PartialEq, Debug)]
pub enum Check {
    Done,
    Running,
    Failed(String),
}

/// Where a string took its labels in an anonymisation already applied: the
/// pass and its rank there, and its family.
#[derive(Clone)]
pub struct Placed {
    pub place: (usize, usize),
    pub kind: String,
}

/// The whole workbench, passed to every part by copy.
#[derive(Clone, Copy)]
pub struct Session {
    pub status: RwSignal<Status>,
    pub name: RwSignal<String>,
    pub format: RwSignal<Format>,
    pub pages: RwSignal<Vec<(f64, f64)>>,
    pub characters: RwSignal<usize>,
    pub blocks: RwSignal<Vec<TextBlock>>,
    pub needles: RwSignal<Vec<Needle>>,
    /// The computed label of each ticked string of a Word document, by text.
    pub labels: RwSignal<HashMap<String, String>>,
    /// The place each string took in the anonymisations already applied,
    /// which it keeps once the result is reopened.
    pub applied: StoredValue<HashMap<String, Placed>>,
    /// Anonymisations applied since the file was chosen.
    pub pass: StoredValue<usize>,
    pub hits: RwSignal<Hits>,
    pub rects: RwSignal<Vec<Rect>>,
    /// The rectangle being dragged, drawn but not yet committed.
    pub drawing: RwSignal<Option<Rect>>,
    /// Index in `rects` of the one the reader last touched.
    pub selected: RwSignal<Option<usize>>,
    /// The occurrence the reader last jumped to: a string of the list, and its
    /// rank among that string's hits in reading order.
    pub focus: RwSignal<Option<(String, usize)>>,
    pub visible: RwSignal<Vec<usize>>,
    pub page: RwSignal<usize>,
    pub zoom: RwSignal<f64>,
    pub fit: RwSignal<bool>,
    pub view: RwSignal<View>,
    pub mode: RwSignal<Mode>,
    pub verdict: RwSignal<Option<Verdict>>,
    /// The ticked strings the file ready to download was produced from.
    pub produced_from: StoredValue<Option<Vec<Needle>>>,
    pub busy: RwSignal<bool>,
    /// Bumped whenever the bytes behind the viewer change, which is what makes
    /// a drawn page repaint rather than keep the pixels of the previous one.
    pub generation: RwSignal<usize>,
    /// Where the text came from. Read from the pixels, a match has no glyph
    /// behind it: it is covered by a rectangle, and re-reading the file proves
    /// nothing about what the pages show.
    pub read_by_eye: RwSignal<bool>,
    /// The pages that draw a picture or yield no text, which a reader reads
    /// and nothing else does.
    pub to_read: RwSignal<Vec<usize>>,
    /// The reader the pages were last read with, which reads them again once
    /// redacted.
    pub eye: RwSignal<Eye>,
    /// Where the reader is, or why it stopped. A reading that fails leaves the
    /// document open and says so in place of the button.
    pub reading: RwSignal<Option<Reading>>,
    pub offline: RwSignal<Offline>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            status: RwSignal::new(Status::Empty),
            name: RwSignal::new(String::new()),
            format: RwSignal::new(Format::Pdf),
            pages: RwSignal::new(Vec::new()),
            characters: RwSignal::new(0),
            blocks: RwSignal::new(Vec::new()),
            needles: RwSignal::new(Vec::new()),
            labels: RwSignal::new(HashMap::new()),
            applied: StoredValue::new(HashMap::new()),
            pass: StoredValue::new(0),
            hits: RwSignal::new(Hits::default()),
            rects: RwSignal::new(Vec::new()),
            drawing: RwSignal::new(None),
            selected: RwSignal::new(None),
            focus: RwSignal::new(None),
            visible: RwSignal::new(Vec::new()),
            page: RwSignal::new(0),
            zoom: RwSignal::new(1.0),
            fit: RwSignal::new(true),
            view: RwSignal::new(View::Pages),
            mode: RwSignal::new(Mode::Read),
            verdict: RwSignal::new(None),
            produced_from: StoredValue::new(None),
            busy: RwSignal::new(false),
            generation: RwSignal::new(0),
            read_by_eye: RwSignal::new(false),
            to_read: RwSignal::new(Vec::new()),
            eye: RwSignal::new(Eye::Fast),
            reading: RwSignal::new(None),
            offline: RwSignal::new(Offline::Online),
        }
    }

    pub fn remove_rect(&self, index: usize) {
        self.rects.update(|rects| {
            if index < rects.len() {
                rects.remove(index);
            }
        });
        self.selected.set(None);
    }

    pub fn add_needle(&self, text: &str, kind: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // The search folds case and accents, so `iris` typed over a detected
        // `Iris` is the same string: it ticks that one rather than a twin
        // whose every hit the first already owns.
        let key = lj_fold::fold(text);
        self.needles.update(
            |list| match list.iter_mut().find(|n| lj_fold::fold(&n.text) == key) {
                Some(same) => same.on = true,
                None => list.push(Needle {
                    text: text.to_string(),
                    kind: kind.to_string(),
                    on: true,
                    label: None,
                }),
            },
        );
    }

    pub fn ticked(&self) -> Vec<Needle> {
        self.needles
            .with_untracked(|list| list.iter().filter(|n| n.on).cloned().collect())
    }

    /// Whether the file ready to download misses what the reader has changed
    /// since it was produced: a pass under way, a rectangle, a string ticked.
    pub fn stale(&self) -> bool {
        self.busy.get_untracked()
            || self.drawing.with_untracked(Option::is_some)
            || self.rects.with_untracked(|rects| !rects.is_empty())
            || self.produced_from.get_value().as_ref() != Some(&self.ticked())
    }

    pub fn active(&self) -> Vec<String> {
        self.needles.with(|list| {
            list.iter()
                .filter(|n| n.on)
                .map(|n| n.text.clone())
                .collect::<Vec<_>>()
        })
    }

    /// The strings labels are given to, in order, each with whether it is
    /// held: those an earlier anonymisation replaced keep their place, and
    /// hold their letters when no longer ticked; the ticked others follow in
    /// the order they first occur, `Jean Dupont` giving `[A] [B]` wherever the
    /// list shows `Dupont`, and a string never found goes last.
    pub fn labelled(&self) -> Vec<(Needle, bool)> {
        let mut out: Vec<(Needle, bool)> = self.needles.with(|list| {
            list.iter()
                .filter(|n| n.on)
                .map(|n| (n.clone(), false))
                .collect()
        });
        let pass = self.pass.get_value();
        self.applied.with_value(|applied| {
            for (text, Placed { kind, .. }) in applied {
                if !out.iter().any(|(n, _)| n.text == *text) {
                    let held = Needle {
                        text: text.clone(),
                        kind: kind.clone(),
                        on: false,
                        label: None,
                    };
                    out.push((held, true));
                }
            }
            self.hits.with(|found| {
                let mut first: HashMap<&str, usize> = HashMap::new();
                for hit in &found.hits {
                    let at = first.entry(&found.needles[hit.needle]).or_insert(hit.start);
                    *at = (*at).min(hit.start);
                }
                out.sort_by_key(|(n, _)| match applied.get(&n.text) {
                    Some(applied) => applied.place,
                    None => first
                        .get(n.text.as_str())
                        .map_or((usize::MAX, 0), |at| (pass, *at)),
                })
            });
        });
        out
    }

    /// Keep the place each string took in the anonymisation just applied,
    /// `entries` as text and family in the order their labels were given in.
    pub fn keep_places(&self, entries: impl IntoIterator<Item = (String, String)>) {
        let pass = self.pass.get_value();
        self.applied.update_value(|applied| {
            for (rank, (text, kind)) in entries.into_iter().enumerate() {
                applied.entry(text).or_insert(Placed {
                    place: (pass, rank),
                    kind,
                });
            }
        });
        self.pass.set_value(pass + 1);
    }

    /// What replaces `text` in a Word document: the label the user set, else
    /// the computed one. `None` for a string left unticked.
    pub fn label_of(&self, text: &str) -> Option<String> {
        let computed = self.labels.with(|labels| labels.get(text).cloned())?;
        self.needles.with(|list| {
            list.iter()
                .find(|n| n.text == text && n.on)
                .map(|n| n.label.clone().unwrap_or(computed))
        })
    }

    /// How many times `text` was located, ticked or not.
    pub fn count_of(&self, text: &str) -> usize {
        self.hits
            .with(|found| match found.needles.iter().position(|n| n == text) {
                Some(index) => found.hits.iter().filter(|h| h.needle == index).count(),
                None => 0,
            })
    }

    /// Every hit of `text`, in reading order.
    fn hits_of(&self, text: &str) -> Vec<Hit> {
        self.hits.with(|found| {
            let Some(index) = found.needles.iter().position(|n| n == text) else {
                return Vec::new();
            };
            let mut hits: Vec<Hit> = found
                .hits
                .iter()
                .filter(|h| h.needle == index)
                .cloned()
                .collect();
            hits.sort_by_key(|h| (h.start, h.page));
            hits
        })
    }

    /// The hit the reader last jumped to, while it still exists.
    pub fn focused(&self) -> Option<Hit> {
        let (text, rank) = self.focus.get()?;
        self.hits_of(&text).into_iter().nth(rank)
    }

    /// Move the focus to the next occurrence of `text`, wrapping after the
    /// last one, and return it.
    pub fn step(&self, text: &str) -> Option<Hit> {
        let hits = self.hits_of(text);
        if hits.is_empty() {
            return None;
        }
        let rank = match self.focus.get_untracked() {
            Some((current, rank)) if current == text => (rank + 1) % hits.len(),
            _ => 0,
        };
        self.focus.set(Some((text.to_string(), rank)));
        hits.into_iter().nth(rank)
    }

    /// Hits to draw: those of the needles still ticked.
    pub fn live_hits(&self) -> Vec<Hit> {
        let on: Vec<String> = self.active();
        self.hits.with(|found| {
            found
                .hits
                .iter()
                .filter(|h| {
                    found
                        .needles
                        .get(h.needle)
                        .is_some_and(|text| on.iter().any(|t| t == text))
                })
                .cloned()
                .collect()
        })
    }
}

/// What a label says of a string replaced by nothing.
pub const ERASED: &str = "effacé";

/// French heading for a rule family, and the order families are shown in.
pub const FAMILIES: [(&str, &str); 9] = [
    ("manual", "Ajoutés"),
    ("person", "Noms"),
    ("address", "Adresses"),
    ("email", "Courriels"),
    ("phone", "Téléphones"),
    ("identifier", "Identifiants"),
    ("docket", "Numéros de dossier"),
    ("birth-date", "Dates de naissance"),
    ("birth-place", "Lieux de naissance"),
];
