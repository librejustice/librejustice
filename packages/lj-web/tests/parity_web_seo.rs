//! Harnais parity-web SEO.
//!
//! Verrouille la parité du `<head>` SEO entre le front Rust (Leptos SSR) et
//! l'oracle Node `apps/web` (React Router framework mode). Deux niveaux :
//!
//! - Shell générique : `<title>` posé par `App()` ; `<meta description>` posé par
//!   `AppShell` depuis `seo::site_default()`, SUPPRIMÉE sur `/decision/*` (la page
//!   y émet sa propre description) — parité RR : la route remplace la meta racine,
//!   jamais deux balises `description`.
//! - Routes dynamiques : la route `/decision/:id` (SEO-critique : title/desc/OG/
//!   canonical/JSON-LD dans le HTML initial) et `/decisions` (hérite du shell —
//!   pas de meta propre côté React, cf. `seo/search.rs`).
//!
//! Limite connue : ces tests exercent les fonctions SEO pures + l'assemblage de
//! balises, PAS le rendu de l'arbre `App`→`AppShell`→`DecisionPage` complet. La
//! non-duplication de `<meta description>` (générique vs décision) relève de la
//! composition des composants : vérifiée en live (curl sur serveur SSR + A/B
//! Node, cf. working-notes), hors de portée d'un test fixture sans DB.
//!
//! Comparaison : FIXTURE pour l'instant — on exerce les fonctions SEO pures
//! (`seo::decision`, identiques byte-à-byte au port de `lib/decision-seo.ts`) et
//! on assemble les balises exactement comme la page / l'export `meta` React les
//! émet, puis on assert les valeurs attendues. L'oracle Node live (`apps/web`
//! SSR ou le service `ssr`) n'était pas joignable hors-ligne au moment d'écrire
//! ce harnais (ports 3000/5174 fermés) ; la comparaison live byte-exact contre
//! le HTML rendu arrivera quand l'oracle tournera en CI/local.
//!
//! Divergence connue, NON sémantique : l'ordre des clés du JSON-LD diffère
//! (`{"@type", ...common}` côté TS vs `@type` inséré après `common` côté Rust).
//! Les crawlers parsent le JSON-LD en graphe — on compare donc le JSON-LD en
//! `serde_json::Value` (indépendant de l'ordre des clés des objets), pas en
//! octets.
//!
//! Gate `ssr` : `seo` n'est compilé que côté serveur dans le flux courant ; la
//! cible wasm hydrate n'exécute pas `cargo test`.
#![cfg(feature = "ssr")]

use lj_dtos::{DecisionDetail, JurisdictionType};
use lj_web::seo::decision::{build_json_ld, meta_description};
use lj_web::seo::{canonical_url, site_default, OG_IMAGE};

/// Extrait le contenu textuel d'une balise `<title>…</title>` d'un `<head>`.
fn extract_title(head_html: &str) -> Option<&str> {
    let start = head_html.find("<title>")? + "<title>".len();
    let rest = &head_html[start..];
    let end = rest.find("</title>")?;
    Some(rest[..end].trim())
}

/// Extrait le `content` de la `<meta name="description" content="…">`.
fn extract_meta_description(head_html: &str) -> Option<&str> {
    let tag_start = head_html.find(r#"name="description""#)?;
    let rest = &head_html[tag_start..];
    let attr = rest.find("content=\"")? + "content=\"".len();
    let after = &rest[attr..];
    let end = after.find('"')?;
    Some(&after[..end])
}

/// Fixture de décision : champs minimaux pour exercer le SEO décision.
fn fixture_detail() -> DecisionDetail {
    DecisionDetail {
        id: "ce-2024-470537".to_string(),
        jurisdiction_type: JurisdictionType::Ce,
        title: "Conseil d'État, 12 mars 2024, n° 470537".to_string(),
        paragraphs: vec![],
        paragraph_spans: Vec::new(),
        sections: None,
        summary: Some(
            "Le Conseil d'État annule la décision attaquée pour erreur de droit. \
             La haute juridiction renvoie l'affaire."
                .to_string(),
        ),
        jurisdiction_code: None,
        jurisdiction_name: Some("Conseil d'État".to_string()),
        date_lecture: Some("2024-03-12".to_string()),
        solution: None,
        procedure: None,
        office: None,
        legal_domain: None,
        publication: None,
        publication_codes: vec![],
        date_audience: None,
        docket_numbers: None,
        seat: None,
        chamber: None,
        formation: None,
        legal_references: None,
        source_xml: None,
        themes: Vec::new(),
        nac: None,
        ecli: None,
        source: None,
        chronology: Vec::new(),
        commentaires: vec![],
    }
}

// ── Shell générique (T0, conservé) ───────────────────────────────────────────

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

/// Assemble le `<head>` décision EXACTEMENT comme `DecisionLoaded` (Rust) et
/// l'export `meta` de `decision-page.tsx` (React) le font, puis verrouille
/// chaque balise. Parité title/description/OG/twitter/canonical.
#[test]
fn decision_route_head_matches_react_meta_contract() {
    let detail = fixture_detail();
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(&detail.id);

    // Title : `{title} — LibreJustice` (parité ligne 50 de decision-page.tsx).
    let page_title = format!("{title} — LibreJustice");
    assert_eq!(
        page_title,
        "Conseil d'État, 12 mars 2024, n° 470537 — LibreJustice"
    );

    // Description : phrase 1 du summary, tronquée au mot (≤ 160 c).
    assert_eq!(
        description,
        "Le Conseil d'État annule la décision attaquée pour erreur de droit."
    );

    // URL canonique apex.
    assert_eq!(url, "https://librejustice.fr/decision/ce-2024-470537");

    // Bloc OG/twitter rendu par `DecisionLoaded` (ordre + valeurs).
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

/// JSON-LD : comparé en `Value` (ordre des clés objet non significatif). On
/// reconstruit le graphe attendu et on assert l'égalité sémantique.
#[test]
fn decision_jsonld_matches_react_graph() {
    let detail = fixture_detail();
    let title = detail.title.clone();
    let description = meta_description(&detail, &title);
    let url = canonical_url(&detail.id);

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

/// Cas `summary == None` : la description tombe sur le titre (parité fallback
/// `detail.summary ? … : title`).
#[test]
fn decision_description_falls_back_to_title_without_summary() {
    let mut detail = fixture_detail();
    detail.summary = None;
    let description = meta_description(&detail, &detail.title);
    assert_eq!(description, detail.title);
}
