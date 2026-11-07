//! Harnais SEO.
//!
//! Verrouille le `<head>` SEO du front Rust (Leptos SSR). Deux niveaux :
//!
//! - Shell générique : `<title>` posé par `App()` ; `<meta description>` posée par
//!   `AppShell` depuis `seo::site_default()`, supprimée sur `/decision/*` où la
//!   page émet la sienne.
//! - Routes dynamiques : `/decision/:id` (title/desc/OG/canonical/JSON-LD dans le
//!   HTML initial) et `/decision-search` (hérite du shell).
//!
//! Ces tests exercent les fonctions SEO pures et l'assemblage de balises, pas le
//! rendu de l'arbre `App`→`AppShell`→`DecisionPage` complet.
//!
//! Gate `ssr` : `seo` n'est compilé que côté serveur.
#![cfg(feature = "ssr")]

use lj_dtos::{DecisionDetail, JurisdictionType};
use lj_web::seo::decision::{build_json_ld, meta_description, seo_title};
use lj_web::seo::{canonical_url, site_default, OG_IMAGE};

fn extract_title(head_html: &str) -> Option<&str> {
    let start = head_html.find("<title>")? + "<title>".len();
    let rest = &head_html[start..];
    let end = rest.find("</title>")?;
    Some(rest[..end].trim())
}

fn extract_meta_description(head_html: &str) -> Option<&str> {
    let tag_start = head_html.find(r#"name="description""#)?;
    let rest = &head_html[tag_start..];
    let attr = rest.find("content=\"")? + "content=\"".len();
    let after = &rest[attr..];
    let end = after.find('"')?;
    Some(&after[..end])
}

fn fixture_detail() -> DecisionDetail {
    DecisionDetail {
        id: "ce-2024-470537".to_string(),
        jurisdiction_type: JurisdictionType::Ce,
        title: "Conseil d'État, 12 mars 2024, n° 470537".to_string(),
        case_name: None,
        paragraphs: vec![],
        paragraph_spans: Vec::new(),
        sections: None,
        summary: Some(
            "Le Conseil d'État annule la décision attaquée pour erreur de droit. \
             La haute juridiction renvoie l'affaire."
                .to_string(),
        ),
        summary_quote: None,
        summary_body: None,
        jurisdiction_code: None,
        jurisdiction_name: Some("Conseil d'État".to_string()),
        court_place: None,
        date_lecture: Some("2024-03-12".to_string()),
        solution: None,
        formation: Vec::new(),
        legal_domain: None,
        publication_codes: vec![],
        date_audience: None,
        docket_numbers: None,
        seat: None,
        seat_path: None,
        legal_references: None,
        source_xml: None,
        themes: Vec::new(),
        nac: None,
        ecli: None,
        official_url: None,
        source: None,
        chronology: Vec::new(),
        commentaires: vec![],
        citantes: vec![],
        citantes_total: None,
    }
}

// ── Shell générique ──────────────────────────────────────────────────────────

#[test]
fn site_default_locks_generic_title_and_description() {
    let meta = site_default();
    assert_eq!(meta.title, "LibreJustice");
    assert_eq!(
        meta.description,
        "Moteur de recherche libre sur le droit français : jurisprudence (Conseil \
         d'État, Cour de cassation, cours d'appel, tribunaux) et textes (codes, \
         lois, traités), mis à jour quotidiennement."
    );
    assert!(meta.robots.is_none());
}

#[test]
fn head_extraction_matches_site_default() {
    let meta = site_default();
    let head = format!(
        "<head><meta charset=\"utf-8\"/><title>{}</title>\
         <meta name=\"description\" content=\"{}\"/></head>",
        meta.title, meta.description
    );

    assert_eq!(extract_title(&head), Some(meta.title.as_str()));
    assert_eq!(
        extract_meta_description(&head),
        Some(meta.description.as_str())
    );
}

// ── Route /decision/:id : SEO-critique ───────────────────────────────────────

#[test]
fn decision_route_head_matches_react_meta_contract() {
    let detail = fixture_detail();
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(
        detail.jurisdiction_type,
        detail.jurisdiction_code.as_deref(),
        &detail.id,
    );

    let page_title = format!("{} - LibreJustice", seo_title(&detail));
    assert_eq!(
        page_title,
        "Conseil d'État, 12 mars 2024, n° 470537 - LibreJustice"
    );

    assert_eq!(
        description,
        "Le Conseil d'État annule la décision attaquée pour erreur de droit. \
         La haute juridiction renvoie l'affaire."
    );

    assert_eq!(url, "https://librejustice.fr/decision/fr/ce-2024-470537");

    let head = format!(
        "<title>{page_title}</title>\
         <meta name=\"description\" content=\"{description}\"/>\
         <meta property=\"og:type\" content=\"article\"/>\
         <meta property=\"og:site_name\" content=\"LibreJustice\"/>\
         <meta property=\"og:title\" content=\"{title}\"/>\
         <meta property=\"og:description\" content=\"{description}\"/>\
         <meta property=\"og:url\" content=\"{url}\"/>\
         <meta property=\"og:locale\" content=\"fr_FR\"/>\
         <meta property=\"og:image\" content=\"{OG_IMAGE}\"/>\
         <meta property=\"og:image:width\" content=\"1200\"/>\
         <meta property=\"og:image:height\" content=\"630\"/>\
         <meta name=\"twitter:card\" content=\"summary_large_image\"/>\
         <link rel=\"canonical\" href=\"{url}\"/>"
    );

    assert_eq!(extract_title(&head), Some(page_title.as_str()));
    assert_eq!(extract_meta_description(&head), Some(description.as_str()));
    assert!(head.contains(r#"<meta property="og:type" content="article"/>"#));
    assert!(head.contains(
        r#"<meta property="og:title" content="Conseil d'État, 12 mars 2024, n° 470537"/>"#
    ));
    assert!(head.contains(r#"<meta property="og:locale" content="fr_FR"/>"#));
    assert!(head.contains(r#"<meta name="twitter:card" content="summary_large_image"/>"#));
    assert!(head.contains(&format!(
        r#"<meta property="og:image" content="{OG_IMAGE}"/>"#
    )));
    assert!(head.contains(&format!(r#"<link rel="canonical" href="{url}"/>"#)));
}

/// JSON-LD : comparé en `Value`, l'ordre des clés d'un objet n'étant pas
/// significatif.
#[test]
fn decision_jsonld_matches_react_graph() {
    let detail = fixture_detail();
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(
        detail.jurisdiction_type,
        detail.jurisdiction_code.as_deref(),
        &detail.id,
    );

    let got = build_json_ld(&detail, &title, &description);

    let expected: serde_json::Value = serde_json::json!({
        "@context": "https://schema.org",
        "@graph": [
            {
                "@type": "LegalCase",
                "name": title,
                "headline": title,
                "url": url,
                "inLanguage": "fr",
                "abstract": description,
                "description": description,
                "datePublished": "2024-03-12",
                "courtName": "Conseil d'État",
            },
            {
                "@type": "Article",
                "name": title,
                "headline": title,
                "url": url,
                "inLanguage": "fr",
                "abstract": description,
                "description": description,
                "datePublished": "2024-03-12",
                "mainEntityOfPage": url,
            },
        ],
    });

    assert_eq!(got, expected);
}

/// La description remplit le budget SERP : elle déborde de la phrase 1 et coupe
/// en milieu de phrase sur une frontière de mot, ellipse comprise.
#[test]
fn decision_description_spans_beyond_the_first_sentence() {
    let mut detail = fixture_detail();
    detail.summary = Some(
        "M. et Mme N recherchent la responsabilité de leur voisin M. R pour troubles \
         anormaux de voisinage. La cour d'appel d'Amiens retient l'anormalité du trouble \
         causé par l'utilisation fréquente du barbecue et condamne M. R à le déplacer, \
         ainsi qu'à verser 1500 euros de dommages-intérêts."
            .to_string(),
    );
    let description = meta_description(&detail, &detail.title);

    assert_eq!(
        description,
        "M. et Mme N recherchent la responsabilité de leur voisin M. R pour troubles \
         anormaux de voisinage. La cour d'appel d'Amiens retient l'anormalité du trouble…"
    );
    assert!(description.chars().count() <= 160);
}

/// Un paragraphe suivant ne rejoint jamais la description : le résumé s'arrête
/// au premier `\n\n`.
#[test]
fn decision_description_stops_at_the_first_paragraph() {
    let mut detail = fixture_detail();
    detail.summary = Some("Première phrase courte.\n\nSecond paragraphe.".to_string());
    let description = meta_description(&detail, &detail.title);
    assert_eq!(description, "Première phrase courte.");
}

#[test]
fn decision_description_falls_back_to_title_without_summary() {
    let mut detail = fixture_detail();
    detail.summary = None;
    let description = meta_description(&detail, &detail.title);
    assert_eq!(description, detail.title);
}

#[test]
fn seo_title_drops_a_seat_that_carries_its_own_commas() {
    let mut detail = fixture_detail();
    detail.seat = Some("pôle 5, 3e chambre · formation à trois".to_string());
    detail.title =
        "Cour d'appel de Paris, pôle 5, 3e chambre · formation à trois, 12 mars 2024, 22/01234"
            .to_string();

    assert_eq!(
        seo_title(&detail),
        "Cour d'appel de Paris, 12 mars 2024, 22/01234"
    );
}

#[test]
fn seo_title_is_the_canonical_title_without_a_seat() {
    let detail = fixture_detail();
    assert!(detail.seat.is_none());
    assert_eq!(seo_title(&detail), detail.title);
}
