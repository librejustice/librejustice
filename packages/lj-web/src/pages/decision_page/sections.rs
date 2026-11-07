//! Sections de rendu d'une décision (renderer pur).

use lj_dtos::{CitationSpan, DecisionDetail, UiAction};

/// Bloc de rendu : `id` (ancre DOM), `title` (libellé), `paragraphs`. Distinct du
/// DTO `DecisionSection` (`kind`/`anchor`/`label`/`paragraphs`).
///
/// `paragraph_spans` : mentions de citation cliquables alignées index-à-index sur
/// `paragraphs`. Vide ⇒ paragraphes en texte brut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSection {
    pub id: String,
    pub title: String,
    pub paragraphs: Vec<String>,
    pub paragraph_spans: Vec<Vec<CitationSpan>>,
}

/// Spans du paragraphe d'indice `i` : `spans[i]` s'il existe, sinon vide (l'API
/// omet le tableau quand aucun paragraphe ne porte de citation).
fn spans_at(spans: &[Vec<CitationSpan>], i: usize) -> Vec<CitationSpan> {
    spans.get(i).cloned().unwrap_or_default()
}

/// Sections de rendu d'une décision :
/// - `detail.sections` non-vide ⇒ map `{id: anchor, title: label, paragraphs}` ;
/// - sinon `detail.paragraphs` vide ⇒ `[]` ;
/// - sinon un bloc unique « Texte intégral » (ancre `texte-integral`).
pub fn resolve_decision_sections(detail: &DecisionDetail) -> Vec<RenderSection> {
    if let Some(sections) = detail.sections.as_ref().filter(|s| !s.is_empty()) {
        return sections
            .iter()
            .map(|s| RenderSection {
                id: s.anchor.clone(),
                title: s.label.clone(),
                paragraphs: s.paragraphs.clone(),
                paragraph_spans: (0..s.paragraphs.len())
                    .map(|i| spans_at(&s.paragraph_spans, i))
                    .collect(),
            })
            .collect();
    }
    if detail.paragraphs.is_empty() {
        return Vec::new();
    }
    vec![RenderSection {
        id: "texte-integral".to_string(),
        title: "Texte intégral".to_string(),
        paragraphs: detail.paragraphs.clone(),
        paragraph_spans: (0..detail.paragraphs.len())
            .map(|i| spans_at(&detail.paragraph_spans, i))
            .collect(),
    }]
}

/// A sommaire entry: its anchor, its title, the action a click on it counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub id: String,
    pub title: String,
    pub usage: UiAction,
}

fn toc_usage(kind: &str) -> UiAction {
    match kind {
        "preamble" => UiAction::DecisionTocPreamble,
        "procedure" => UiAction::DecisionTocProcedure,
        "visa" => UiAction::DecisionTocVisa,
        "expose" => UiAction::DecisionTocExpose,
        "moyens" => UiAction::DecisionTocMoyens,
        "chapeau" => UiAction::DecisionTocChapeau,
        "motivations" => UiAction::DecisionTocMotivations,
        "dispositif" => UiAction::DecisionTocDispositif,
        "annexes" => UiAction::DecisionTocAnnexes,
        _ => UiAction::DecisionTocOther,
    }
}

/// Sommaire de la décision : entrée `Synthèse` en tête, puis une entrée par
/// `kind` canonique (première occurrence, ancrée sur `anchor`), avec repli sur
/// les sections rendues quand l'API n'en fournit pas.
pub fn toc_sections(detail: &DecisionDetail, body_sections: &[RenderSection]) -> Vec<TocEntry> {
    let mut toc_body: Vec<TocEntry> = Vec::new();
    let mut seen_kinds: Vec<String> = Vec::new();
    for section in detail.sections.iter().flatten() {
        if seen_kinds.contains(&section.kind) {
            continue;
        }
        seen_kinds.push(section.kind.clone());
        toc_body.push(TocEntry {
            id: section.anchor.clone(),
            title: section.label.clone(),
            usage: toc_usage(&section.kind),
        });
    }
    let mut out = vec![TocEntry {
        id: "synthese".to_string(),
        title: "Synthèse".to_string(),
        usage: UiAction::DecisionTocSynthesis,
    }];
    if toc_body.is_empty() {
        out.extend(body_sections.iter().map(|s| TocEntry {
            id: s.id.clone(),
            title: s.title.clone(),
            usage: UiAction::DecisionTocOther,
        }));
    } else {
        out.extend(toc_body);
    }
    out
}

/// Occurrences under each sommaire entry: a body section counts toward the last
/// entry anchored at or before it, the stretch the entry leads to and lights.
pub fn toc_hit_counts(
    toc: &[TocEntry],
    section_ids: &[String],
    section_hits: &[usize],
) -> Vec<usize> {
    let mut out = vec![0; toc.len()];
    let mut entry = None;
    for (id, n) in section_ids.iter().zip(section_hits) {
        if let Some(i) = toc.iter().position(|e| &e.id == id) {
            entry = Some(i);
        }
        if let Some(i) = entry {
            out[i] += n;
        }
    }
    out
}
