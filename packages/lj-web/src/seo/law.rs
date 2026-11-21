//! Logique SEO pure des pages /texte (titre, meta description, JSON-LD). Calqué
//! sur [`crate::seo::decision`] : module pur (aucun accès DOM/réseau), la donnée
//! vient des DTO `lj-dtos`. Rust pur, PAS de `lj-core` (règle wasm).

use lj_dtos::LawArticleResponse;

use super::generic::CANONICAL_BASE;

/// The canonical URL of a LEGI article (`/norm/{juris}/{code}/{num}`).
/// A version at a date (`…/{date}`) is never canonical: every version
/// canonicalises to the current article, so the SEO does not fragment.
pub fn article_canonical_url(juris: &str, code: &str, num: &str) -> String {
    format!("{CANONICAL_BASE}{}/{num}", lj_vocab::norm_path(juris, code))
}

/// URL canonique d'un code (`/norm/{juris}/{code}`).
pub fn code_canonical_url(juris: &str, code: &str) -> String {
    format!("{CANONICAL_BASE}{}", lj_vocab::norm_path(juris, code))
}

/// `<meta description>` d'un article : début du texte de l'article (ou son
/// intitulé / fil d'Ariane à défaut de texte), tronqué sur une frontière de mot.
pub fn article_meta_description(
    ui: &crate::i18n::Ui,
    article: &LawArticleResponse,
    title: &str,
) -> String {
    // Article modificatif : le `texte` est le résumé de liens brut (illisible en
    // snippet SERP). On résume proprement depuis `modifications`.
    if !article.modifications.is_empty() {
        return truncate_at_word(
            &modifying_description(ui, article, title),
            META_DESCRIPTION_MAX,
        );
    }
    let raw = match article
        .texte
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(texte) => texte,
        None => title,
    };
    truncate_at_word(raw, META_DESCRIPTION_MAX)
}

/// Description SERP d'un article modificatif : verbes présents (modifie/crée/
/// abroge) + codes cibles distincts, dérivés de `modifications`.
fn modifying_description(
    ui: &crate::i18n::Ui,
    article: &LawArticleResponse,
    title: &str,
) -> String {
    let mut verbs: Vec<&str> = Vec::new();
    for m in &article.modifications {
        let Some(v) = (ui.norm.action_verb)(&m.action) else {
            continue;
        };
        if !verbs.contains(&v) {
            verbs.push(v);
        }
    }
    let mut codes: Vec<&str> = Vec::new();
    for m in &article.modifications {
        if !codes.contains(&m.code.as_str()) {
            codes.push(&m.code);
        }
    }
    let verbs = (ui.list)(&verbs);
    let codes = (ui.list)(&codes);
    if verbs.is_empty() || codes.is_empty() {
        return title.to_string();
    }
    (ui.norm.amends)(title, &verbs, &codes)
}

/// Cap meta description (limite SERP Google). Aligné sur `seo::decision`.
const META_DESCRIPTION_MAX: usize = 160;

/// Tronque à `max` `char`s sur une frontière de mot, ellipse `…` incluse.
/// Calqué sur `truncate_at_word` de `seo::decision`.
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

/// Construit le JSON-LD `Legislation` (schema.org) d'un article LEGI. Calqué sur
/// `seo::decision::build_json_ld` (un seul `@type`, pas de `@graph`).
pub fn build_article_json_ld(
    article: &LawArticleResponse,
    title: &str,
    description: &str,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    // URL sur la clé canonique (`numKey`), cohérente avec le `<link rel=canonical>`
    // et résolue en lookup exact côté serve.
    let url = article_canonical_url(&article.jurisdiction, &article.code, &article.num_key);
    let mut node = Map::new();
    node.insert("@type".into(), json!("Legislation"));
    node.insert("name".into(), json!(title));
    node.insert("url".into(), json!(url));
    node.insert("inLanguage".into(), json!("fr"));
    node.insert("legislationType".into(), json!("Code"));
    node.insert("jurisdiction".into(), json!("FR"));
    node.insert(
        "legislationIdentifier".into(),
        json!(article.legiarti.clone()),
    );
    if !description.is_empty() {
        node.insert("description".into(), json!(description));
    }
    node.insert("dateModified".into(), json!(article.date_debut.clone()));
    node.insert("legislationDate".into(), json!(article.date_debut.clone()));
    // `sameAs` = lien source canonique : Légifrance pour le natif, page du
    // diffuseur pour le curé. Omis si aucune source n'est connue.
    if let Some(url) = &article.source_url {
        node.insert("sameAs".into(), json!(url.clone()));
    }

    Value::Object({
        let mut root = Map::new();
        root.insert("@context".into(), json!("https://schema.org"));
        root.extend(node);
        root
    })
}
