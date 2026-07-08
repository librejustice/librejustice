//! Logique SEO pure des pages /loi (titre, meta description, JSON-LD). Calqué
//! sur [`crate::seo::decision`] : module pur (aucun accès DOM/réseau), la donnée
//! vient des DTO `lj-dtos`. Rust pur, PAS de `lj-core` (règle wasm).

use lj_dtos::LawArticleResponse;

use super::generic::CANONICAL_BASE;

/// URL canonique d'un article LEGI (`/loi/{code}/{num}`). La version-à-date
/// (`…/{date}`) n'est jamais canonique : toutes les versions canonicalisent vers
/// l'article courant pour ne pas fragmenter le SEO.
pub fn article_canonical_url(code: &str, num: &str) -> String {
    format!("{CANONICAL_BASE}/loi/{code}/{num}")
}

/// URL canonique d'un code (`/loi/{code}`).
pub fn code_canonical_url(code: &str) -> String {
    format!("{CANONICAL_BASE}/loi/{code}")
}

/// `<meta description>` d'un article : début du texte de l'article (ou son
/// intitulé / fil d'Ariane à défaut de texte), tronqué sur une frontière de mot.
pub fn article_meta_description(article: &LawArticleResponse, title: &str) -> String {
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
    // et résolue en lookup exact côté serve (ADR 0123 §2).
    let url = article_canonical_url(&article.code, &article.num_key);
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
    // `sameAs` = lien source canonique (ADR 0131) : Légifrance pour le natif, page du
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
