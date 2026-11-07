//! Applicability compiler. From the versions a source delivers, the counter
//! events read in the text and the repository's rules of reception, it writes
//! positive assertions: one state, a list of places, a half-open window
//! `[start, end)`, the origin of each bound. One assertion per place is the
//! meaning; the list of places is the compression of rows that differ by the
//! place alone, and the key of a rule the compression of a window its default
//! spans cut and nothing else touched. Pure, no I/O.
//!
//! What the source calls `date_fin` is inclusive; every bound here is
//! exclusive. An end that coincides with the start of the next state is
//! `Computed`, whoever wrote it: it is the closure by the next state. An end
//! nothing follows is `Received`: an abrogation, a declared end of validity.
//! A received end is never moved: two states of one place that overlap are
//! both kept and counted as a conflict.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use jiff::civil::Date;
use jiff::ToSpan;

use crate::journal_rules::{key_spans, resolve, rule_key, rules_of, DefaultSpan, Regime};

/// Open lower bound the store writes when the source gives no start.
pub const OPEN_START: Date = Date::constant(1, 1, 1);

const JOURNALS_TSV: &str = include_str!("../data/journals.tsv");

/// A named official journal of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    pub code: &'static str,
    pub label: &'static str,
    pub publisher: &'static str,
    pub languages: Vec<&'static str>,
    /// Journal whose rules this one resolves on (the BOCC and the BOFiP are
    /// received where the JORF is).
    pub defaults_from: Option<&'static str>,
}

static JOURNALS: LazyLock<Vec<Journal>> = LazyLock::new(|| {
    JOURNALS_TSV
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 5, "journals.tsv: {l}");
            Journal {
                code: f[0],
                label: f[1],
                publisher: f[2],
                languages: f[3].split(',').collect(),
                defaults_from: Some(f[4]).filter(|s| !s.is_empty()),
            }
        })
        .collect()
});

pub fn journals() -> &'static [Journal] {
    &JOURNALS
}

pub fn journal(code: &str) -> Option<&'static Journal> {
    JOURNALS.iter().find(|j| j.code == code)
}

/// Code of the journal a text belongs to, from its uid and jurisdiction. Every
/// State has an official journal: an unnamed one is `JO-<ISO>`.
pub fn journal_code(text_uid: &str, jurisdiction: &str) -> String {
    if text_uid.starts_with("KALI") {
        return "BOCC".to_string();
    }
    if text_uid.starts_with("BOI-") {
        return "BOFIP".to_string();
    }
    if let Some(site) = text_uid
        .strip_prefix("bo_")
        .and_then(|rest| rest.split_once('_'))
        .map(|(site, _)| site)
    {
        return format!("BO-{}", site.to_uppercase());
    }
    if jurisdiction == "EU" || text_uid.starts_with("EU/") {
        return "JOUE".to_string();
    }
    match jurisdiction {
        "FR" | "INTL" => "JORF".to_string(),
        "NC" => "JONC".to_string(),
        "PF" => "JOPF".to_string(),
        "BE" => "MB".to_string(),
        "CH" => "RO".to_string(),
        "DZ" => "JORADP".to_string(),
        "SN" => "JORS".to_string(),
        iso => format!("JO-{iso}"),
    }
}

/// Where an article applies when its source says nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Base {
    /// The default spans of a rule key: a window its events leave whole is
    /// written once, under the key.
    Key(String),
    /// Spans named place by place, never folded: the text's own place, the
    /// State of a journal the rules do not name.
    Named(Vec<DefaultSpan>),
}

impl Base {
    /// The base of an article of `journal_code` in `matter`. The text's own
    /// place replaces the rules; an unnamed State journal covers its State.
    pub fn of(journal_code: &str, matter: Option<&str>, text_place: Option<&str>) -> Base {
        if let Some(place) = text_place {
            return Base::Named(vec![DefaultSpan::undated(
                place,
                "lieu par défaut du texte",
            )]);
        }
        if let Some(key) = rule_key(journal_code, matter) {
            return Base::Key(key);
        }
        Base::Named(match journal_code.strip_prefix("JO-") {
            Some(iso) => vec![DefaultSpan::undated(
                &format!("geo:{iso}"),
                &format!("Journal officiel de l'État, geo:{iso}"),
            )],
            None => Vec::new(),
        })
    }

    pub fn spans(&self) -> &[DefaultSpan] {
        match self {
            Base::Key(key) => {
                key_spans(key).unwrap_or_else(|| panic!("rule key {key} unknown to the rules"))
            }
            Base::Named(spans) => spans,
        }
    }
}

/// A version as the source delivers it. `start` `None` is the open lower
/// bound; `end` is inclusive, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVersion {
    pub num_key: String,
    pub start: Option<Date>,
    pub end: Option<Date>,
    pub status: String,
}

/// First day of the placeholders DILA writes as a start it does not know
/// (2222-02-22, 2999-01-01): the row carries an edge, never a window.
pub const UNDATED_PLACEHOLDER: Date = Date::constant(2222, 1, 1);

impl SourceVersion {
    /// A wording that never entered into force: modified before its day,
    /// disjoined from the act, or dated by a placeholder. It occupies no slot.
    pub fn is_stillborn(&self) -> bool {
        matches!(self.status.as_str(), "MODIFIE_MORT_NE" | "DISJOINT")
            || self.start.is_some_and(|d| d >= UNDATED_PLACEHOLDER)
    }

    /// An open-ended row whose status says the article is gone: the source's
    /// way of dating an abrogation. It closes what precedes and makes no state.
    pub fn is_closing_marker(&self) -> bool {
        self.end.is_none() && self.start.is_some() && says_gone(&self.status)
    }
}

/// A status that says an article or a text is gone.
fn says_gone(status: &str) -> bool {
    status.starts_with("ABROGE")
        || matches!(
            status,
            "TRANSFERE" | "DEPLACE" | "PERIME" | "ANNULE" | "REMPLACE" | "DENONCE"
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Opens the article to a place outside the default list, on the wording of
    /// one version (the frozen wording of a specialty territory).
    Extend,
    /// Removes a place from the article's states during a window.
    Exclude,
}

/// What a counter or a clause of the text says about one article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub place: String,
    /// Start of the version whose wording the event carries (`Extend`).
    pub version_start: Option<Date>,
    /// The counter names a wording ("dans sa rédaction résultant de …"), so the
    /// place keeps it while the article moves on. Without it the counter only
    /// says where the article applies, and the article's own timeline says
    /// which wording: the extension is then cut on its version boundaries.
    pub frozen: bool,
    pub window_start: Date,
    /// Inclusive, as the source dates its versions.
    pub window_end: Option<Date>,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Received,
    Computed,
    Fallback,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Received => "received",
            Origin::Computed => "computed",
            Origin::Fallback => "fallback",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "received" => Origin::Received,
            "computed" => Origin::Computed,
            "fallback" => Origin::Fallback,
            _ => return None,
        })
    }
}

/// One compiled state: applies in `places` during `[start, end)`, worded by the
/// version starting at `version_start`. A keyed row names no place: it stands
/// for its window cut by the default spans of `rule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub num_key: String,
    pub places: Vec<String>,
    pub rule: Option<String>,
    pub start: Date,
    pub end: Option<Date>,
    pub start_origin: Origin,
    pub end_origin: Option<Origin>,
    pub start_source: Option<String>,
    pub end_source: Option<String>,
    pub version_start: Date,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArticleDiagnostics {
    pub never_applicable: u32,
    pub closing_markers: u32,
    pub inverted: u32,
    /// Two received windows overlap on one place: kept both, reported.
    pub conflicts: u32,
    /// Events the states cannot receive: a version the source does not hold,
    /// an exclusion on a place no state names.
    pub dangling_events: u32,
}

/// A window the source delivers, before the default table cuts it by place.
struct Window {
    start: Date,
    end: Option<Date>,
    start_origin: Origin,
    end_origin: Option<Origin>,
    end_source: Option<String>,
}

/// What makes two pieces one row: everything but the place.
type PieceKey = (
    Date,
    Option<Date>,
    Date,
    Origin,
    Option<Origin>,
    Option<String>,
    Option<String>,
);

/// One interval on one place, before compression.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    start: Date,
    end: Option<Date>,
    start_origin: Origin,
    end_origin: Option<Origin>,
    start_source: Option<String>,
    end_source: Option<String>,
    version_start: Date,
    /// The end came from a counter version losing currency, not from the
    /// article's own `date_fin`: it says that clause stopped speaking, so a
    /// later clause on the same place supersedes it instead of colliding.
    end_from_clause: bool,
    /// The window a default piece was cut from; `None` for an event's.
    window: Option<usize>,
}

impl Piece {
    fn key(&self) -> PieceKey {
        (
            self.start,
            self.end,
            self.version_start,
            self.start_origin,
            self.end_origin,
            self.start_source.clone(),
            self.end_source.clone(),
        )
    }
}

/// A window cut by one default span: a bound the span moves becomes received,
/// sourced by the line that sets it. `None` when nothing is left.
fn cut(w: &Window, span: &DefaultSpan) -> Option<Piece> {
    let (mut s, mut so, mut ss) = (w.start, w.start_origin, None);
    if let Some(since) = span.since {
        if since > s {
            (s, so, ss) = (since, Origin::Received, Some(span.since_source.clone()));
        }
    }
    let (mut e, mut eo, mut es) = (w.end, w.end_origin, w.end_source.clone());
    if let Some(until) = span.until {
        if before(Some(until), e) {
            (e, eo, es) = (
                Some(until),
                Some(Origin::Received),
                Some(span.until_source.clone()),
            );
        }
    }
    if e.is_some_and(|e| e <= s) {
        return None;
    }
    Some(Piece {
        start: s,
        end: e,
        start_origin: so,
        end_origin: eo,
        start_source: ss,
        end_source: es,
        version_start: w.start,
        end_from_clause: false,
        window: None,
    })
}

fn tomorrow(d: Date) -> Date {
    d.checked_add(1.day()).expect("date within range")
}

fn before(a: Option<Date>, b: Option<Date>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a < b,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Compiles the states of one article. `versions` are all the rows the source
/// holds for it, `events` the counters and clauses that name it, `base` where
/// it applies when they say nothing.
pub fn compile_article(
    versions: &[SourceVersion],
    events: &[Event],
    base: &Base,
) -> (Vec<Assertion>, ArticleDiagnostics) {
    let mut diag = ArticleDiagnostics::default();
    let num_key = versions
        .first()
        .map(|v| v.num_key.clone())
        .unwrap_or_default();

    // 1. Windows the source delivers, exclusive ends, stillborns and markers out.
    let mut markers: Vec<(Date, String)> = Vec::new();
    let mut windows: Vec<Window> = Vec::new();
    let mut sorted: Vec<&SourceVersion> = versions.iter().collect();
    sorted.sort_by_key(|v| v.start.unwrap_or(OPEN_START));
    for v in sorted {
        if v.is_stillborn() {
            diag.never_applicable += 1;
            continue;
        }
        if v.is_closing_marker() {
            diag.closing_markers += 1;
            markers.push((v.start.expect("marker is dated"), v.status.clone()));
            continue;
        }
        let start = v.start.unwrap_or(OPEN_START);
        let start_origin = if v.start.is_some() {
            Origin::Received
        } else {
            Origin::Fallback
        };
        let end = v.end.map(tomorrow);
        if end.is_some_and(|e| e <= start) {
            diag.inverted += 1;
            continue;
        }
        windows.push(Window {
            start,
            end,
            start_origin,
            end_origin: end.map(|_| Origin::Received),
            end_source: None,
        });
    }
    // Closure by the nearest marker, then by the next state; a received end
    // that runs past the next start stays and is counted per place in step 4.
    for i in 0..windows.len() {
        let next_start = windows.get(i + 1).map(|w| w.start);
        let marker = markers
            .iter()
            .filter(|(d, _)| *d > windows[i].start && next_start.is_none_or(|n| *d < n))
            .min_by_key(|(d, _)| *d);
        let w = &mut windows[i];
        if let Some((m, status)) = marker {
            if w.end.is_none_or(|e| e > *m) {
                w.end = Some(*m);
                w.end_origin = Some(Origin::Received);
                w.end_source = Some(format!("statut {status} de la version du {m}"));
            }
        }
        if let Some(n) = next_start {
            if w.end.is_none_or(|e| e == n) {
                w.end = Some(n);
                w.end_origin = Some(Origin::Computed);
                w.end_source = None;
            }
        }
    }

    // 2. One piece per place and per window, cut by the default spans.
    let spans = base.spans();
    let mut by_place: BTreeMap<String, Vec<Piece>> = BTreeMap::new();
    for (i, w) in windows.iter().enumerate() {
        for span in spans {
            if let Some(piece) = cut(w, span) {
                by_place.entry(span.place.clone()).or_default().push(Piece {
                    window: Some(i),
                    ..piece
                });
            }
        }
    }

    // 3. Extensions open a place on one wording; exclusions cut a place out.
    let version_starts: Vec<Date> = windows.iter().map(|w| w.start).collect();
    for ev in events.iter().filter(|e| e.kind == EventKind::Extend) {
        let Some(vs) = ev.version_start.filter(|d| version_starts.contains(d)) else {
            diag.dangling_events += 1;
            continue;
        };
        let end = ev.window_end.map(tomorrow);
        if end.is_some_and(|e| e <= ev.window_start) {
            diag.inverted += 1;
            continue;
        }
        let pieces = by_place.entry(ev.place.clone()).or_default();
        if ev.frozen {
            pieces.push(Piece {
                start: ev.window_start,
                end,
                start_origin: Origin::Received,
                end_origin: end.map(|_| Origin::Received),
                start_source: Some(ev.source.clone()),
                end_source: end.map(|_| ev.source.clone()),
                version_start: vs,
                end_from_clause: end.is_some(),
                window: None,
            });
            continue;
        }
        for w in &windows {
            let start = w.start.max(ev.window_start);
            let stop = match (w.end, end) {
                (None, e) => e,
                (Some(a), None) => Some(a),
                (Some(a), Some(b)) => Some(a.min(b)),
            };
            if stop.is_some_and(|s| s <= start) {
                continue;
            }
            let from_counter = stop.is_some() && stop == end;
            pieces.push(Piece {
                start,
                end: stop,
                start_origin: Origin::Received,
                end_origin: stop.map(|_| {
                    if from_counter {
                        Origin::Received
                    } else {
                        Origin::Computed
                    }
                }),
                start_source: Some(ev.source.clone()),
                end_source: from_counter.then(|| ev.source.clone()),
                version_start: w.start,
                end_from_clause: from_counter,
                window: None,
            });
        }
    }

    // Two counters that name the same wording on the same place say one thing:
    // their windows are a union, not a contradiction.
    for pieces in by_place.values_mut() {
        pieces.sort_by_key(|p| (p.version_start, p.start, p.end.is_none(), p.end));
        let mut merged: Vec<Piece> = Vec::new();
        for p in pieces.drain(..) {
            match merged.last_mut() {
                Some(last)
                    if last.version_start == p.version_start
                        && last.end.is_none_or(|e| e >= p.start) =>
                {
                    let longer = match (last.end, p.end) {
                        (None, _) | (_, None) => None,
                        (Some(a), Some(b)) => Some(a.max(b)),
                    };
                    if longer != last.end {
                        last.end = longer;
                        last.end_origin = p.end_origin;
                        last.end_source = p.end_source;
                        last.end_from_clause = p.end_from_clause;
                    }
                }
                _ => merged.push(p),
            }
        }
        *pieces = merged;
    }
    for ev in events.iter().filter(|e| e.kind == EventKind::Exclude) {
        let Some(pieces) = by_place.get_mut(&ev.place) else {
            diag.dangling_events += 1;
            continue;
        };
        let cut_start = ev.window_start;
        let cut_end = ev.window_end.map(tomorrow);
        let mut out = Vec::new();
        for p in pieces.drain(..) {
            let overlaps = p.start < cut_end.unwrap_or(Date::MAX) && before(Some(cut_start), p.end);
            if !overlaps {
                out.push(p);
                continue;
            }
            if p.start < cut_start {
                out.push(Piece {
                    end: Some(cut_start),
                    end_origin: Some(Origin::Received),
                    end_source: Some(ev.source.clone()),
                    end_from_clause: true,
                    ..p.clone()
                });
            }
            if let Some(ce) = cut_end {
                if before(Some(ce), p.end) || p.end.is_none() {
                    out.push(Piece {
                        start: ce,
                        start_origin: Origin::Received,
                        start_source: Some(ev.source.clone()),
                        ..p.clone()
                    });
                }
            }
        }
        *pieces = out;
    }

    // 4. On each place, a later state closes an earlier one left open; two
    //    received bounds that overlap are a conflict, kept and counted.
    for pieces in by_place.values_mut() {
        pieces.sort_by_key(|p| (p.start, p.version_start));
        for i in 0..pieces.len().saturating_sub(1) {
            let next_start = pieces[i + 1].start;
            let p = &mut pieces[i];
            if p.end.is_none_or(|e| e > next_start) {
                if p.end.is_some() && p.end_origin == Some(Origin::Received) && !p.end_from_clause {
                    diag.conflicts += 1;
                } else {
                    p.end = Some(next_start);
                    p.end_origin = Some(Origin::Computed);
                    p.end_source = None;
                    p.end_from_clause = false;
                }
            }
        }
        pieces.retain(|p| p.end.is_none_or(|e| e > p.start));
    }

    // 5. A window whose default pieces all came through whole is one row
    //    under its key; the rest is named.
    let mut keyed = Vec::new();
    if let Base::Key(key) = base {
        for (i, w) in windows.iter().enumerate() {
            let mut image: Vec<(&str, PieceKey)> = spans
                .iter()
                .filter_map(|s| cut(w, s).map(|p| (s.place.as_str(), p.key())))
                .collect();
            if image.is_empty() {
                continue;
            }
            let mut kept: Vec<(&str, PieceKey)> = by_place
                .iter()
                .flat_map(|(place, pieces)| {
                    pieces
                        .iter()
                        .filter(|p| p.window == Some(i))
                        .map(move |p| (place.as_str(), p.key()))
                })
                .collect();
            image.sort();
            kept.sort();
            if image != kept {
                continue;
            }
            for pieces in by_place.values_mut() {
                pieces.retain(|p| p.window != Some(i));
            }
            keyed.push(Assertion {
                num_key: num_key.clone(),
                places: Vec::new(),
                rule: Some(key.clone()),
                start: w.start,
                end: w.end,
                start_origin: w.start_origin,
                end_origin: w.end_origin,
                start_source: None,
                end_source: w.end_source.clone(),
                version_start: w.start,
            });
        }
    }

    // 6. Rows that differ by the place alone become one row.
    let named = by_place
        .iter()
        .flat_map(|(place, pieces)| pieces.iter().map(|p| one_place(&num_key, place, p.key())));
    let mut assertions = regroup(named.collect());
    assertions.extend(keyed);
    (assertions, diag)
}

fn one_place(num_key: &str, place: &str, key: PieceKey) -> Assertion {
    let (start, end, version_start, start_origin, end_origin, start_source, end_source) = key;
    Assertion {
        num_key: num_key.to_string(),
        places: vec![place.to_string()],
        rule: None,
        start,
        end,
        start_origin,
        end_origin,
        start_source,
        end_source,
        version_start,
    }
}

/// Named rows that differ by the place alone, as one row each.
pub fn regroup(rows: Vec<Assertion>) -> Vec<Assertion> {
    let mut grouped: BTreeMap<(String, PieceKey), Vec<String>> = BTreeMap::new();
    for a in rows {
        let key = (
            a.start,
            a.end,
            a.version_start,
            a.start_origin,
            a.end_origin,
            a.start_source,
            a.end_source,
        );
        grouped
            .entry((a.num_key, key))
            .or_default()
            .extend(a.places);
    }
    grouped
        .into_iter()
        .map(|((num_key, key), mut places)| {
            places.sort();
            places.dedup();
            Assertion {
                places,
                ..one_place(&num_key, "", key)
            }
        })
        .collect()
}

/// The named rows a keyed assertion stands for, one per place: its window cut
/// by each default span of its key, as the compiler cut it. A named row
/// stands for itself. `None` for a key this binary's rules cannot unfold.
pub fn unfold(a: &Assertion) -> Option<Vec<Assertion>> {
    let Some(key) = &a.rule else {
        return Some(vec![a.clone()]);
    };
    let w = Window {
        start: a.start,
        end: a.end,
        start_origin: a.start_origin,
        end_origin: a.end_origin,
        end_source: a.end_source.clone(),
    };
    Some(
        key_spans(key)?
            .iter()
            .filter_map(|s| cut(&w, s).map(|p| one_place(&a.num_key, &s.place, p.key())))
            .collect(),
    )
}

/// One interval of a text's coverage: on a place, or on every place a key
/// reaches on that day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub place: Option<String>,
    pub rule: Option<String>,
    pub start: Date,
    pub end: Option<Date>,
}

/// The coverage of a text: the windows of its assertions merged into islands,
/// under their key when every keyed row carries one and no named row falls
/// where that key reaches, place by place otherwise, so that a text counts
/// once in the corpus of a place. `None` for a key this binary cannot unfold.
pub fn coverage(rows: &[Assertion]) -> Option<Vec<Coverage>> {
    let mut keys: Vec<&str> = rows.iter().filter_map(|a| a.rule.as_deref()).collect();
    keys.sort_unstable();
    keys.dedup();
    let key = match keys.as_slice() {
        [key] => {
            let reach = key_spans(key)?;
            let clear = rows
                .iter()
                .flat_map(|a| &a.places)
                .all(|p| reach.iter().all(|s| &s.place != p));
            clear.then_some(*key)
        }
        _ => None,
    };
    let mut by_place: BTreeMap<Option<&str>, Vec<(Date, Option<Date>)>> = BTreeMap::new();
    let mut unfolded = Vec::new();
    for a in rows {
        match (&a.rule, key) {
            (Some(_), Some(key)) => by_place
                .entry(Some(key))
                .or_default()
                .push((a.start, a.end)),
            (Some(_), None) => unfolded.extend(unfold(a)?),
            (None, _) => unfolded.push(a.clone()),
        }
    }
    let mut named: BTreeMap<&str, Vec<(Date, Option<Date>)>> = BTreeMap::new();
    for a in &unfolded {
        for p in &a.places {
            named.entry(p.as_str()).or_default().push((a.start, a.end));
        }
    }
    let mut out = Vec::new();
    for (rule, windows) in by_place {
        for (start, end) in islands(windows) {
            out.push(Coverage {
                place: None,
                rule: rule.map(str::to_string),
                start,
                end,
            });
        }
    }
    for (place, windows) in named {
        for (start, end) in islands(windows) {
            out.push(Coverage {
                place: Some(place.to_string()),
                rule: None,
                start,
                end,
            });
        }
    }
    Some(out)
}

/// Half-open windows merged: `[a, b)` and `[b, c)` are one island, and an open
/// end swallows whatever follows it.
fn islands(mut windows: Vec<(Date, Option<Date>)>) -> Vec<(Date, Option<Date>)> {
    windows.sort_unstable_by_key(|(s, e)| (*s, e.is_none(), *e));
    let mut out: Vec<(Date, Option<Date>)> = Vec::new();
    for (start, end) in windows {
        match out.last_mut() {
            Some((_, open)) if open.is_none_or(|e| start <= e) => {
                *open = match (*open, end) {
                    (None, _) | (_, None) => None,
                    (Some(a), Some(b)) => Some(a.max(b)),
                };
            }
            _ => out.push((start, end)),
        }
    }
    out
}

/// The rows a frozen matter adds to a text's coverage: a frozen line covers its
/// place from its first day for as long as it holds, when an article it governs
/// there is in force that day, whose wording of that day stands there. Named
/// rows for [`coverage`] alone, never states: the reader derives the frozen
/// state from the rules.
pub fn frozen_rows(
    journal_code: &str,
    rows: &[Assertion],
    matter: impl Fn(&str) -> Option<&'static str>,
) -> Vec<Assertion> {
    let mut out: Vec<Assertion> = Vec::new();
    for line in rules_of(journal_code)
        .iter()
        .filter(|r| r.regime == Regime::Frozen)
    {
        let since = line.since.expect("a frozen line names its day");
        let held = rows.iter().any(|a| {
            a.start <= since
                && a.end.is_none_or(|e| since < e)
                && resolve(journal_code, line.place, matter(&a.num_key), since)
                    .is_some_and(|r| std::ptr::eq(r, line))
        });
        if held {
            out.push(Assertion {
                num_key: String::new(),
                places: vec![line.place.to_string()],
                rule: None,
                start: since,
                end: line.until,
                start_origin: Origin::Received,
                end_origin: line.until.map(|_| Origin::Received),
                start_source: Some(line.source.to_string()),
                end_source: None,
                version_start: since,
            });
        }
    }
    out
}

/// The coverage of a text without an article: one window, from its
/// publication, else its signature, to the first abrogation dated after it,
/// cut by its base as an article's would be. A date on or before the start is
/// an edge read the other way round, the text abrogating an older one. `None`
/// when the source says the text is gone without dating it: a doubt, which the
/// coverage leaves out.
pub fn bare_text_coverage(
    publication: Option<Date>,
    signature: Option<Date>,
    status: &str,
    abrogations: &[Date],
    base: &Base,
) -> Option<Vec<Coverage>> {
    let dated = |d: &Date| *d < UNDATED_PLACEHOLDER;
    let start = publication.filter(dated).or(signature.filter(dated));
    let abrogated = abrogations
        .iter()
        .copied()
        .filter(|d| dated(d) && start.is_none_or(|s| *d > s))
        .min();
    if abrogated.is_none() && (says_gone(status) || abrogations.iter().any(|d| !dated(d))) {
        return None;
    }
    let version = SourceVersion {
        num_key: String::new(),
        start,
        end: abrogated.map(|d| d.yesterday().expect("date within range")),
        status: String::new(),
    };
    let (assertions, _) = compile_article(&[version], &[], base);
    Some(coverage(&assertions).expect("a base names only keys of this binary"))
}

/// Version of the wording normalisation the fingerprint is computed on.
pub const FINGERPRINT_VERSION: i16 = 1;

/// Bumped by hand when a rule of [`compile_article`] changes, so that every
/// compiled text is recompiled: the stock never lags behind the compiler.
pub const COMPILER_VERSION: u16 = 5;

/// Lookup fingerprint of a wording: xxh3-64 of the text with every run of
/// whitespace folded to one space, and its length in characters. An index,
/// never an identity.
pub fn wording_fingerprint(text: &str) -> (u64, u32) {
    let folded: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (
        xxhash_rust::xxh3::xxh3_64(folded.as_bytes()),
        folded.chars().count() as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn v(start: &str, end: Option<&str>, status: &str) -> SourceVersion {
        SourceVersion {
            num_key: "l218-42".to_string(),
            start: Some(d(start)),
            end: end.map(d),
            status: status.to_string(),
        }
    }

    fn jorf() -> Base {
        Base::Key("JORF".to_string())
    }

    /// The rows a reader meets: keyed rows unfolded, all of them regrouped.
    fn flat(a: &[Assertion]) -> Vec<Assertion> {
        regroup(a.iter().flat_map(|x| unfold(x).unwrap()).collect())
    }

    fn at(a: &[Assertion], place: &str, date: &str) -> Option<Assertion> {
        let date = d(date);
        flat(a).into_iter().find(|x| {
            x.places.iter().any(|p| p == place) && x.start <= date && x.end.is_none_or(|e| e > date)
        })
    }

    #[test]
    fn registry_reads_and_borrows_defaults() {
        assert_eq!(journal_code("LEGITEXT000006070721", "FR"), "JORF");
        assert_eq!(journal_code("KALICONT000005635691", "FR"), "BOCC");
        assert_eq!(journal_code("EU/REG/2016/679", "EU"), "JOUE");
        assert_eq!(journal_code("SN/CODE-FAMILLE", "SN"), "JORS");
        assert_eq!(journal_code("MA/CODE-FAMILLE", "MA"), "JO-MA");
        assert_eq!(Base::of("BOCC", None, None), Base::of("JORF", None, None));
        assert_eq!(Base::of("JO-MA", None, None).spans()[0].place, "geo:MA");
        assert!(jorf().spans().iter().any(|p| p.place == "geo:FR-ALS"));
        assert!(journal("JOUE").unwrap().languages.contains(&"fr"));
    }

    #[test]
    fn a_modern_version_is_one_keyed_row_for_every_default_place() {
        let (a, diag) = compile_article(&[v("2015-12-04", None, "VIGUEUR")], &[], &jorf());
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].rule.as_deref(), Some("JORF"));
        assert!(a[0].places.is_empty());
        assert_eq!(a[0].start_origin, Origin::Received);
        assert_eq!(a[0].end, None);
        let rows = flat(&a);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].places.len(), jorf().spans().len());
        assert_eq!(diag, ArticleDiagnostics::default());
    }

    #[test]
    fn mayotte_gets_its_own_row_on_a_straddling_window() {
        let (a, _) = compile_article(
            &[
                v("2005-07-19", Some("2015-11-27"), "MODIFIE"),
                v("2015-11-28", None, "VIGUEUR"),
            ],
            &[],
            &jorf(),
        );
        assert!(
            a.iter().all(|x| x.rule.is_some()),
            "rien ne touche les fenêtres"
        );
        let yt = at(&a, "geo:YT", "2012-01-01").unwrap();
        assert_eq!(yt.places, vec!["geo:YT"]);
        assert_eq!(yt.start, d("2008-01-01"));
        assert_eq!(yt.start_origin, Origin::Received);
        assert!(yt.start_source.as_deref().unwrap().contains("LO 6113-1"));
        assert!(at(&a, "geo:YT", "2007-01-01").is_none());
        let re = at(&a, "geo:RE", "2010-01-01").unwrap();
        assert!(re.places.contains(&"geo:FR-M".to_string()));
        assert_eq!(re.end, Some(d("2015-11-28")));
        assert_eq!(re.end_origin, Some(Origin::Computed));
        assert_eq!(
            at(&a, "geo:FR-M", "2020-01-01").unwrap().version_start,
            d("2015-11-28")
        );
    }

    #[test]
    fn lifou_keeps_the_frozen_wording_beside_the_current_one() {
        let versions = [
            v("2015-11-28", Some("2015-12-03"), "MODIFIE"),
            v("2015-12-04", None, "VIGUEUR"),
        ];
        let events = [Event {
            kind: EventKind::Extend,
            place: "geo:NC".to_string(),
            version_start: Some(d("2015-11-28")),
            window_start: d("2024-10-03"),
            window_end: None,
            source: "compteur L. 612-1".to_string(),
            frozen: true,
        }];
        let (a, diag) = compile_article(&versions, &events, &jorf());
        let nc = at(&a, "geo:NC", "2025-06-01").unwrap();
        assert_eq!(nc.places, vec!["geo:NC"]);
        assert_eq!(nc.version_start, d("2015-11-28"));
        assert_eq!(nc.start, d("2024-10-03"));
        assert_eq!(nc.start_source.as_deref(), Some("compteur L. 612-1"));
        assert!(at(&a, "geo:NC", "2020-01-01").is_none());
        assert_eq!(
            at(&a, "geo:FR-M", "2025-06-01").unwrap().version_start,
            d("2015-12-04")
        );
        assert_eq!(diag.conflicts, 0);
    }

    #[test]
    fn a_later_extension_closes_the_earlier_one_on_the_same_place() {
        let versions = [
            v("2010-01-01", Some("2019-12-31"), "MODIFIE"),
            v("2020-01-01", None, "VIGUEUR"),
        ];
        let ext = |vs: &str, ws: &str| Event {
            kind: EventKind::Extend,
            place: "geo:PF".to_string(),
            version_start: Some(d(vs)),
            window_start: d(ws),
            window_end: None,
            source: format!("compteur {ws}"),
            frozen: true,
        };
        let (a, diag) = compile_article(
            &versions,
            &[
                ext("2010-01-01", "2012-01-01"),
                ext("2020-01-01", "2022-01-01"),
            ],
            &jorf(),
        );
        let first = at(&a, "geo:PF", "2015-01-01").unwrap();
        assert_eq!(first.end, Some(d("2022-01-01")));
        assert_eq!(first.end_origin, Some(Origin::Computed));
        assert_eq!(
            at(&a, "geo:PF", "2023-01-01").unwrap().version_start,
            d("2020-01-01")
        );
        assert_eq!(diag.conflicts, 0);
    }

    #[test]
    fn a_counter_that_names_no_wording_follows_the_article_timeline() {
        let versions = [
            v("2010-01-01", Some("2019-12-31"), "MODIFIE"),
            v("2020-01-01", None, "VIGUEUR"),
        ];
        let events = [Event {
            kind: EventKind::Extend,
            place: "geo:PF".to_string(),
            version_start: Some(d("2010-01-01")),
            window_start: d("2012-01-01"),
            window_end: None,
            source: "compteur L. 1822-1".to_string(),
            frozen: false,
        }];
        let (a, diag) = compile_article(&versions, &events, &jorf());
        let early = at(&a, "geo:PF", "2015-01-01").unwrap();
        assert_eq!(early.version_start, d("2010-01-01"));
        assert_eq!(early.end, Some(d("2020-01-01")));
        assert_eq!(early.end_origin, Some(Origin::Computed));
        let late = at(&a, "geo:PF", "2025-01-01").unwrap();
        assert_eq!(late.version_start, d("2020-01-01"));
        assert_eq!(late.end, None);
        assert_eq!(diag.conflicts, 0);
    }

    #[test]
    fn two_counters_naming_the_same_wording_are_one_window() {
        let versions = [v("2005-01-01", None, "VIGUEUR")];
        let ext = |ws: &str, we: Option<&str>, ident: &str| Event {
            kind: EventKind::Extend,
            place: "geo:PF".to_string(),
            version_start: Some(d("2005-01-01")),
            window_start: d(ws),
            window_end: we.map(d),
            source: format!("compteur {ident}"),
            frozen: false,
        };
        let (a, diag) = compile_article(
            &versions,
            &[
                ext("2008-03-01", Some("2014-07-08"), "L1822-1"),
                ext("2012-01-01", Some("2016-12-06"), "L2573-12"),
            ],
            &jorf(),
        );
        let rows = flat(&a);
        let pf: Vec<_> = rows.iter().filter(|r| r.places == ["geo:PF"]).collect();
        assert_eq!(pf.len(), 1, "deux compteurs, une fenêtre");
        assert_eq!(pf[0].start, d("2008-03-01"));
        assert_eq!(pf[0].end, Some(d("2016-12-07")));
        assert_eq!(diag.conflicts, 0);
    }

    #[test]
    fn a_later_clause_supersedes_the_end_an_earlier_counter_gave() {
        let versions = [
            v("1996-02-24", Some("2009-12-11"), "MODIFIE"),
            v("2009-12-12", Some("2019-12-28"), "MODIFIE"),
            v("2019-12-29", None, "VIGUEUR"),
        ];
        let (a, diag) = compile_article(
            &versions,
            &[
                Event {
                    kind: EventKind::Extend,
                    place: "geo:PF".to_string(),
                    version_start: Some(d("1996-02-24")),
                    window_start: d("2008-03-01"),
                    window_end: Some(d("2026-08-18")),
                    source: "compteur L. 2573-6 ; à l'exception de ses 1°".to_string(),
                    frozen: false,
                },
                Event {
                    kind: EventKind::Extend,
                    place: "geo:PF".to_string(),
                    version_start: Some(d("1996-02-24")),
                    window_start: d("2026-02-21"),
                    window_end: Some(d("2026-08-18")),
                    source: "compteur L. 2573-6 ; sous réserve des adaptations".to_string(),
                    frozen: true,
                },
            ],
            &jorf(),
        );
        let rows = flat(&a);
        let mut pf: Vec<_> = rows.iter().filter(|r| r.places == ["geo:PF"]).collect();
        pf.sort_by_key(|r| r.start);
        assert_eq!(
            pf.len(),
            4,
            "trois rédactions suivies, puis la clause gelée"
        );
        let last_followed = pf[2];
        assert_eq!(last_followed.version_start, d("2019-12-29"));
        assert_eq!(last_followed.end, Some(d("2026-02-21")));
        assert_eq!(last_followed.end_origin, Some(Origin::Computed));
        assert_eq!(pf[3].start, d("2026-02-21"));
        assert_eq!(pf[3].version_start, d("1996-02-24"));
        assert_eq!(diag.conflicts, 0);
    }

    #[test]
    fn an_exclusion_is_an_absence_with_the_clause_as_bound() {
        let (a, _) = compile_article(
            &[v("2000-01-01", None, "VIGUEUR")],
            &[Event {
                kind: EventKind::Exclude,
                place: "geo:FR-ALS".to_string(),
                version_start: Some(d("2000-01-01")),
                window_start: d("2010-01-01"),
                window_end: Some(d("2019-12-31")),
                source: "clause art. 3".to_string(),
                frozen: false,
            }],
            &jorf(),
        );
        assert!(at(&a, "geo:FR-ALS", "2015-01-01").is_none());
        let before_cut = at(&a, "geo:FR-ALS", "2005-01-01").unwrap();
        assert_eq!(before_cut.end, Some(d("2010-01-01")));
        assert_eq!(before_cut.end_source.as_deref(), Some("clause art. 3"));
        let after = at(&a, "geo:FR-ALS", "2021-01-01").unwrap();
        assert_eq!(after.start, d("2020-01-01"));
        assert_eq!(after.start_origin, Origin::Received);
        assert!(at(&a, "geo:FR-M", "2015-01-01").is_some());
        assert_eq!(at(&a, "geo:FR-M", "2015-01-01").unwrap().end, None);
    }

    #[test]
    fn stillborn_and_markers_make_no_state() {
        let (a, diag) = compile_article(
            &[
                v("1804-03-21", None, "VIGUEUR"),
                v("2005-01-01", Some("2004-12-31"), "MODIFIE_MORT_NE"),
                v("2006-03-24", None, "ABROGE"),
            ],
            &[],
            &jorf(),
        );
        assert_eq!(diag.never_applicable, 1);
        assert_eq!(diag.closing_markers, 1);
        assert!(a.iter().all(|x| x.version_start == d("1804-03-21")));
        let fr = at(&a, "geo:FR-M", "2000-01-01").unwrap();
        assert_eq!(fr.end, Some(d("2006-03-24")));
        assert_eq!(fr.end_origin, Some(Origin::Received));
        assert!(at(&a, "geo:FR-M", "2007-01-01").is_none());
    }

    #[test]
    fn a_text_place_replaces_the_defaults_and_an_open_start_is_a_fallback() {
        let (a, _) = compile_article(
            &[SourceVersion {
                num_key: "1".to_string(),
                start: None,
                end: None,
                status: "VIGUEUR".to_string(),
            }],
            &[],
            &Base::of("JORF", None, Some("geo:FR-ALS")),
        );
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].rule, None);
        assert_eq!(a[0].places, vec!["geo:FR-ALS"]);
        assert_eq!(a[0].start, OPEN_START);
        assert_eq!(a[0].start_origin, Origin::Fallback);
    }

    fn named(place: &str) -> Base {
        Base::Named(vec![DefaultSpan::undated(place, "")])
    }

    #[test]
    fn overlapping_received_ends_are_a_conflict_not_a_choice() {
        let (a, diag) = compile_article(
            &[
                v("2000-01-01", Some("2012-12-31"), "MODIFIE"),
                v("2010-01-01", None, "VIGUEUR"),
            ],
            &[],
            &named("geo:SN"),
        );
        assert_eq!(diag.conflicts, 1);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].end, Some(d("2013-01-01")));
        assert_eq!(a[0].end_origin, Some(Origin::Received));
        assert_eq!(
            at(&a, "geo:SN", "2011-06-01").unwrap().start,
            d("2000-01-01")
        );
    }

    #[test]
    fn a_received_end_on_the_next_start_is_the_computed_closure() {
        let (a, diag) = compile_article(
            &[
                v("2000-01-01", Some("2009-12-31"), "MODIFIE"),
                v("2010-01-01", None, "VIGUEUR"),
            ],
            &[],
            &named("geo:SN"),
        );
        assert_eq!(diag.conflicts, 0);
        assert_eq!(a[0].end, Some(d("2010-01-01")));
        assert_eq!(a[0].end_origin, Some(Origin::Computed));
    }

    #[test]
    fn a_placeholder_start_is_stillborn_and_closes_nothing() {
        let (a, diag) = compile_article(
            &[
                v("2016-10-01", None, "VIGUEUR"),
                v("2999-01-01", None, "VIGUEUR"),
                v("2222-02-22", None, "ABROGE"),
            ],
            &[],
            &named("geo:SN"),
        );
        assert_eq!(diag.never_applicable, 2);
        assert_eq!(diag.closing_markers, 0);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].end, None);
    }

    #[test]
    fn a_marker_carries_its_status_as_the_bound_source() {
        let (a, _) = compile_article(
            &[
                v("2000-01-01", None, "VIGUEUR"),
                v("2019-01-01", None, "DENONCE"),
            ],
            &[],
            &named("geo:SN"),
        );
        assert_eq!(a[0].end, Some(d("2019-01-01")));
        assert_eq!(a[0].end_origin, Some(Origin::Received));
        assert!(a[0].end_source.as_deref().unwrap().contains("DENONCE"));
    }

    #[test]
    fn an_inverted_window_and_an_unreceivable_event_are_counted() {
        let (a, diag) = compile_article(
            &[
                v("2020-01-01", Some("2019-06-30"), "MODIFIE"),
                v("2020-01-01", None, "VIGUEUR"),
            ],
            &[
                Event {
                    kind: EventKind::Exclude,
                    place: "geo:GP".to_string(),
                    version_start: None,
                    window_start: d("2021-01-01"),
                    window_end: None,
                    source: "clause".to_string(),
                    frozen: false,
                },
                Event {
                    kind: EventKind::Extend,
                    place: "geo:NC".to_string(),
                    version_start: Some(d("1999-01-01")),
                    window_start: d("2021-01-01"),
                    window_end: None,
                    source: "compteur".to_string(),
                    frozen: true,
                },
            ],
            &named("geo:FR-M"),
        );
        assert_eq!(diag.inverted, 1);
        assert_eq!(diag.dangling_events, 2);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn until_is_the_first_day_no_longer_covered() {
        let (a, _) = compile_article(
            &[v("2000-01-01", None, "VIGUEUR")],
            &[],
            &Base::Key("JOUE".to_string()),
        );
        let bl = at(&a, "geo:BL", "2011-12-31").unwrap();
        assert_eq!(bl.end, Some(d("2012-01-01")));
        assert_eq!(bl.end_origin, Some(Origin::Received));
        assert!(at(&a, "geo:BL", "2012-01-01").is_none());
    }

    /// A small deterministic generator: the timelines and the events a source
    /// can deliver, drawn over the dates the rules cut on.
    struct Draw(u64);

    impl Draw {
        fn next(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        fn day(&mut self) -> Date {
            const DAYS: &[&str] = &[
                "1804-03-21",
                "1946-03-19",
                "1990-01-01",
                "2001-01-01",
                "2001-07-13",
                "2004-03-02",
                "2007-07-15",
                "2008-01-01",
                "2010-06-01",
                "2011-03-31",
                "2012-01-01",
                "2013-07-01",
                "2014-01-01",
                "2016-05-01",
                "2018-01-01",
                "2021-01-01",
                "2024-10-03",
            ];
            d(DAYS[self.next(DAYS.len())])
        }
    }

    fn drawn_article(r: &mut Draw) -> (Vec<SourceVersion>, Vec<Event>) {
        let mut starts: Vec<Date> = (0..1 + r.next(4)).map(|_| r.day()).collect();
        starts.sort();
        starts.dedup();
        let mut versions = Vec::new();
        for (i, s) in starts.iter().enumerate() {
            let end = match (r.next(4), starts.get(i + 1)) {
                (0, Some(n)) => Some(n.yesterday().unwrap()),
                (1, _) => Some(r.day()),
                _ => None,
            };
            versions.push(SourceVersion {
                num_key: "1".to_string(),
                start: Some(*s),
                end,
                status: if r.next(6) == 0 { "ABROGE" } else { "VIGUEUR" }.to_string(),
            });
        }
        const PLACES: &[&str] = &[
            "geo:FR-ALS",
            "geo:YT",
            "geo:BL",
            "geo:PF",
            "geo:NC",
            "geo:BE",
        ];
        let mut events = Vec::new();
        for _ in 0..r.next(3) {
            let (a, b) = (r.day(), r.day());
            events.push(Event {
                kind: if r.next(2) == 0 {
                    EventKind::Extend
                } else {
                    EventKind::Exclude
                },
                place: PLACES[r.next(PLACES.len())].to_string(),
                version_start: Some(starts[r.next(starts.len())]),
                frozen: r.next(2) == 0,
                window_start: a.min(b),
                window_end: (r.next(2) == 0).then(|| a.max(b)),
                source: format!("clause {a}"),
            });
        }
        (versions, events)
    }

    #[test]
    fn a_keyed_row_unfolds_to_exactly_the_rows_it_replaces() {
        let keys = [
            "JORF",
            "JORF:tax",
            "JORF:nationality",
            "JORF:labour",
            "JORF:urban_planning",
            "JOUE",
            "JORF:administrative_procedure",
        ];
        let mut r = Draw(0x9e37_79b9_7f4a_7c15);
        let mut folded = 0;
        for case in 0..3000 {
            let (versions, events) = drawn_article(&mut r);
            let key = keys[case % keys.len()];
            let (keyed, keyed_diag) =
                compile_article(&versions, &events, &Base::Key(key.to_string()));
            let (named, named_diag) = compile_article(
                &versions,
                &events,
                &Base::Named(key_spans(key).unwrap().to_vec()),
            );
            assert_eq!(flat(&keyed), named, "{key} {versions:?} {events:?}");
            assert_eq!(keyed_diag, named_diag);
            folded += keyed.iter().filter(|a| a.rule.is_some()).count();
        }
        assert!(folded > 1000, "{folded} rows folded");
    }

    #[test]
    fn a_window_an_event_touches_stays_named() {
        let (a, _) = compile_article(
            &[v("2000-01-01", None, "VIGUEUR")],
            &[Event {
                kind: EventKind::Exclude,
                place: "geo:BL".to_string(),
                version_start: Some(d("2000-01-01")),
                window_start: d("2010-01-01"),
                window_end: None,
                source: "compteur".to_string(),
                frozen: false,
            }],
            &jorf(),
        );
        assert!(a.iter().all(|x| x.rule.is_none()));
        assert!(at(&a, "geo:BL", "2012-01-01").is_none());
        assert!(at(&a, "geo:MF", "2012-01-01").is_some());
    }

    fn named_row(places: &[&str], start: &str, end: Option<&str>) -> Assertion {
        Assertion {
            num_key: "1".to_string(),
            places: places.iter().map(|p| p.to_string()).collect(),
            rule: None,
            start: d(start),
            end: end.map(d),
            start_origin: Origin::Received,
            end_origin: end.map(|_| Origin::Computed),
            start_source: None,
            end_source: None,
            version_start: d(start),
        }
    }

    fn keyed_row(key: &str, start: &str, end: Option<&str>) -> Assertion {
        Assertion {
            places: Vec::new(),
            rule: Some(key.to_string()),
            ..named_row(&[], start, end)
        }
    }

    fn interval(
        place: Option<&str>,
        rule: Option<&str>,
        start: &str,
        end: Option<&str>,
    ) -> Coverage {
        Coverage {
            place: place.map(str::to_string),
            rule: rule.map(str::to_string),
            start: d(start),
            end: end.map(d),
        }
    }

    #[test]
    fn two_versions_that_touch_are_one_interval_and_a_gap_stays_two() {
        let rows = [
            named_row(&["geo:FR-M", "geo:RE"], "2000-01-01", Some("2010-01-01")),
            named_row(&["geo:FR-M", "geo:RE"], "2010-01-01", None),
            named_row(&["geo:NC"], "1990-01-01", Some("1995-01-01")),
            named_row(&["geo:NC"], "2005-01-01", Some("2020-01-01")),
            named_row(&["geo:NC"], "2012-06-01", Some("2015-01-01")),
        ];
        assert_eq!(
            coverage(&rows).unwrap(),
            vec![
                interval(Some("geo:FR-M"), None, "2000-01-01", None),
                interval(Some("geo:NC"), None, "1990-01-01", Some("1995-01-01")),
                interval(Some("geo:NC"), None, "2005-01-01", Some("2020-01-01")),
                interval(Some("geo:RE"), None, "2000-01-01", None),
            ]
        );
    }

    #[test]
    fn a_text_under_one_key_covers_under_the_key_unless_a_named_row_meets_its_reach() {
        let keyed = [
            keyed_row("JORF", "2000-01-01", Some("2010-01-01")),
            keyed_row("JORF", "2010-01-01", None),
            named_row(&["geo:PF"], "2012-01-01", None),
        ];
        assert_eq!(
            coverage(&keyed).unwrap(),
            vec![
                interval(None, Some("JORF"), "2000-01-01", None),
                interval(Some("geo:PF"), None, "2012-01-01", None),
            ]
        );
        let meets = [
            keyed_row("JORF", "2000-01-01", None),
            named_row(&["geo:FR-ALS"], "1990-01-01", Some("2000-01-01")),
        ];
        let c = coverage(&meets).unwrap();
        assert!(c.iter().all(|x| x.rule.is_none()));
        assert!(c.contains(&interval(Some("geo:FR-ALS"), None, "1990-01-01", None)));
        let two_keys = [
            keyed_row("JORF:nationality", "2000-01-01", None),
            keyed_row("JORF:civil_law", "2000-01-01", None),
        ];
        assert!(coverage(&two_keys)
            .unwrap()
            .iter()
            .all(|x| x.rule.is_none()));
    }

    #[test]
    fn a_frozen_matter_covers_its_place_with_what_was_in_force_on_the_transfer() {
        let civil = |k: &str| (k != "l1").then_some("civil_law");
        let old = keyed_row("JORF:civil_law", "1804-03-21", Some("2016-10-01"));
        let reform = keyed_row("JORF:civil_law", "2016-10-01", None);
        let frozen = frozen_rows("JORF", &[old.clone(), reform.clone()], civil);
        let pf: Vec<_> = frozen
            .iter()
            .filter(|a| a.places == ["geo:PF"])
            .map(|a| (a.start, a.end))
            .collect();
        assert_eq!(pf, vec![(d("2004-03-02"), None)]);
        assert!(frozen_rows("JORF", &[reform], civil).is_empty());
        let procedure = Assertion {
            num_key: "l1".to_string(),
            ..old
        };
        assert!(frozen_rows("JORF", &[procedure], civil).is_empty());
    }

    #[test]
    fn a_text_without_an_article_runs_from_its_publication_to_its_first_abrogation() {
        let placeholder = d("2999-01-01");
        assert_eq!(
            bare_text_coverage(
                Some(d("2007-11-01")),
                Some(d("2007-10-02")),
                "",
                &[],
                &jorf()
            ),
            Some(vec![interval(None, Some("JORF"), "2007-11-01", None)])
        );
        assert_eq!(
            bare_text_coverage(
                Some(placeholder),
                Some(d("1988-08-25")),
                "VIGUEUR",
                &[],
                &jorf()
            ),
            Some(vec![interval(None, Some("JORF"), "1988-08-25", None)])
        );
        assert_eq!(
            bare_text_coverage(
                Some(d("2001-02-28")),
                None,
                "ABROGE",
                &[
                    d("1997-03-13"),
                    placeholder,
                    d("2012-01-01"),
                    d("2010-02-25")
                ],
                &jorf()
            ),
            Some(vec![interval(
                None,
                Some("JORF"),
                "2001-02-28",
                Some("2010-02-25")
            )])
        );
        let alsace = Base::of("JORF", None, Some("geo:FR-ALS"));
        assert_eq!(
            bare_text_coverage(Some(d("2001-02-14")), None, "", &[], &alsace),
            Some(vec![interval(Some("geo:FR-ALS"), None, "2001-02-14", None)])
        );
    }

    #[test]
    fn a_text_gone_without_a_date_is_a_doubt_and_an_older_date_reads_the_other_way() {
        let start = Some(d("2009-04-30"));
        assert_eq!(
            bare_text_coverage(start, None, "ABROGE", &[], &jorf()),
            None
        );
        assert_eq!(
            bare_text_coverage(start, None, "", &[d("2999-01-01")], &jorf()),
            None
        );
        assert_eq!(
            bare_text_coverage(
                start,
                None,
                "",
                &[d("2009-04-30"), d("1995-01-01")],
                &jorf()
            ),
            Some(vec![interval(None, Some("JORF"), "2009-04-30", None)])
        );
    }

    #[test]
    fn fingerprint_folds_whitespace_only() {
        assert_eq!(
            wording_fingerprint("a  b\n c"),
            wording_fingerprint("a b c")
        );
        assert_ne!(
            wording_fingerprint("arrivé, à le réparer"),
            wording_fingerprint("arrivé à le réparer")
        );
        assert_eq!(wording_fingerprint("abc").1, 3);
    }
}
