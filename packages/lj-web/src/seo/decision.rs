//! Logique SEO pure des pages decision (titre, meta description, JSON-LD).
//!
//! Module pur (aucun acces DOM/reseau) ; la donnee vient des DTO
//! `lj-dtos::DecisionDetail`. Rust pur, PAS de `lj-core` (regle wasm).

use lj_dtos::DecisionDetail;

use super::generic::canonical_url;
use crate::components::ui::shown_summary;

/// Abreviations a NE PAS considerer comme fin de phrase.
const ABBREVIATIONS: &[&str] = &[
    "M.", "MM.", "Mme.", "Mlle.", "Dr.", "Pr.", "Me.", "Bd.", "art.", "al.", "ord.", "n.", "Inc.",
    "Ltd.", "Co.", "Corp.",
];

/// Cap meta description (limite SERP Google).
const META_DESCRIPTION_MAX: usize = 160;

/// Retourne la premiere phrase d'un `summary` Mistral.
///
/// Privilegie le separateur paragraphe `\n\n` ; sinon cherche le premier `. `
/// dont le token precedent n'est PAS une abreviation / initiale isolee.
/// Travaille en indices de `char` (pas d'octets) pour rester sur des frontieres
/// valides.
pub fn first_sentence(summary: &str) -> String {
    let chars: Vec<char> = summary.chars().collect();
    // Separateur paragraphe explicite `\n\n`.
    if let Some(para) = find_char_seq(&chars, &['\n', '\n'], 0) {
        return chars[..para].iter().collect();
    }
    let mut pos = 0;
    while pos < chars.len() {
        let next_sp = find_char_seq(&chars, &['.', ' '], pos);
        let next_nl = find_char_seq(&chars, &['.', '\n'], pos);
        let dot_idx = match (next_sp, next_nl) {
            (None, None) => break,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (Some(a), Some(b)) => a.min(b),
        };
        // Point d'une ellipse `..`/`...` : pas une fin de phrase.
        if dot_idx > 0 && chars[dot_idx - 1] == '.' {
            pos = dot_idx + 1;
            continue;
        }
        // Token alphabetique terminant pile avant le point.
        let mut i = dot_idx as isize - 1;
        while i >= 0 && is_alpha(chars[i as usize]) {
            i -= 1;
        }
        let token: String = chars[(i + 1) as usize..dot_idx].iter().collect();
        let is_initial =
            token.chars().count() == 1 && token == token.to_uppercase() && is_alpha_str(&token);
        if is_initial || ABBREVIATIONS.contains(&format!("{token}.").as_str()) {
            pos = dot_idx + 2;
            continue;
        }
        return chars[..=dot_idx].iter().collect();
    }
    // Pas de separateur interne : retire un point final orphelin.
    if summary.ends_with('.') {
        chars[..chars.len() - 1].iter().collect()
    } else {
        summary.to_string()
    }
}

/// Cherche la sous-sequence `needle` dans `hay` a partir de `from` ; renvoie
/// l'indice (char) du premier element, ou `None`.
fn find_char_seq(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn is_alpha(ch: char) -> bool {
    ch.is_alphabetic()
}

fn is_alpha_str(s: &str) -> bool {
    s.chars().all(is_alpha) && !s.is_empty()
}

/// Tronque a `max` `char`s sur une frontiere de mot, ellipse `…` incluse.
fn truncate_at_word(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let hard_chars: Vec<char> = {
        let sliced: String = text.chars().take(max - 1).collect();
        sliced.trim_end().chars().collect()
    };
    let last_space = hard_chars.iter().rposition(|&c| c == ' ');
    let cut: String = match last_space {
        Some(idx) if idx as f64 > max as f64 * 0.6 => hard_chars[..idx].iter().collect(),
        _ => hard_chars.iter().collect(),
    };
    format!("{}…", cut.trim_end())
}

/// The `<title>` of a decision page: the canonical title minus its seat segment,
/// which pushes the date and the docket number past the SERP display bound.
pub fn seo_title(detail: &DecisionDetail) -> String {
    match detail.seat.as_deref() {
        Some(seat) => detail.title.replacen(&format!(", {seat}"), "", 1),
        None => detail.title.clone(),
    }
}

/// `<meta description>` : premier paragraphe du summary (ou fallback titre),
/// tronque sur une frontiere de mot.
///
/// Cuts mid-sentence rather than at the first sentence, which is short and
/// leaves much of the SERP budget unused.
pub fn meta_description(detail: &DecisionDetail, title: &str) -> String {
    let shown = detail.summary.as_deref().map(|summary| {
        shown_summary(
            detail.jurisdiction_type,
            summary,
            detail.summary_quote.as_ref(),
            detail.summary_body.as_ref(),
        )
        .text
    });
    let raw = match shown.as_deref() {
        Some(summary) => first_paragraph(summary),
        None => title,
    };
    truncate_at_word(raw, META_DESCRIPTION_MAX)
}

/// Texte jusqu'au premier separateur paragraphe `\n\n`.
fn first_paragraph(summary: &str) -> &str {
    match summary.find("\n\n") {
        Some(idx) => summary[..idx].trim_end(),
        None => summary.trim_end(),
    }
}

/// Construit le JSON-LD `@graph` LegalCase + Article.
pub fn build_json_ld(detail: &DecisionDetail, title: &str, description: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let url = canonical_url(
        detail.jurisdiction_type,
        detail.jurisdiction_code.as_deref(),
        &detail.id,
    );
    let mut common = Map::new();
    common.insert("name".into(), json!(title));
    common.insert("headline".into(), json!(title));
    common.insert("url".into(), json!(url));
    common.insert("inLanguage".into(), json!("fr"));
    if !description.is_empty() {
        common.insert("abstract".into(), json!(description));
        common.insert("description".into(), json!(description));
    }
    if let Some(date) = &detail.date_lecture {
        common.insert("datePublished".into(), json!(date));
    }

    let mut legal_case = common.clone();
    legal_case.insert("@type".into(), json!("LegalCase"));
    if let Some(court) = &detail.jurisdiction_name {
        legal_case.insert("courtName".into(), json!(court));
    }

    let mut article = common;
    article.insert("@type".into(), json!("Article"));
    article.insert("mainEntityOfPage".into(), json!(url));

    json!({
        "@context": "https://schema.org",
        "@graph": [Value::Object(legal_case), Value::Object(article)],
    })
}
