//! The Bulletin officiel de la sécurité sociale: one page per rubric, its paragraphs numbered.

use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;

use crate::bofip::{split_numbered, Paragraph};
use crate::bulletins::{french_date, BulletinClient};
use crate::error::{Result, SourceError};
use crate::html_strip::{decode_entities, strip_html};

pub const BASE_URL: &str = "https://boss.gouv.fr";
const THEMES: &[&str] = &[
    "regles-dassujettissement",
    "autres-elements-de-remuneration",
    "bulletin-de-paie",
    "exonerations",
    "mesures-exceptionnelles",
];

/// A rubric's page path under `/portail/accueil/`, `exonerations/exonerations-zonees`.
pub async fn rubrics(client: &BulletinClient) -> Result<Vec<String>> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let link = RE.get_or_init(|| {
        Regex::new(
            r#"href="(?:https://boss\.gouv\.fr)?/portail/accueil/([a-z-]+/[a-z0-9-]+)\.html""#,
        )
        .expect("valid regex")
    });
    let mut out = Vec::new();
    for theme in THEMES {
        let html = client
            .get_text(&format!("{BASE_URL}/portail/accueil/{theme}.html"))
            .await?;
        for c in link.captures_iter(&html) {
            if c[1].starts_with(&format!("{theme}/")) && !out.contains(&c[1].to_string()) {
                out.push(c[1].to_string());
            }
        }
    }
    Ok(out)
}

pub fn rubric_url(path: &str) -> String {
    format!("{BASE_URL}/portail/accueil/{path}.html")
}

pub async fn rubric(client: &BulletinClient, url: &str) -> Result<Rubric> {
    parse_rubric(&client.get_text(url).await?)
}

/// An earlier version's fragment as served, for [`parse_version`].
pub async fn fragment(client: &BulletinClient, url: &str) -> Result<String> {
    client.get_text(url).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub from: NaiveDate,
    /// The day the next version applies from; `None` for the version in force.
    pub until: Option<NaiveDate>,
    /// The fragment that serves an earlier version; the rubric's page serves the one in force.
    pub url: Option<String>,
}

#[derive(Debug)]
pub struct Rubric {
    pub title: String,
    /// The version in force first, then the earlier ones, newest first.
    pub versions: Vec<Version>,
    pub paragraphs: Vec<Paragraph>,
    /// The text of a rubric with no numbered paragraph, a page of prose.
    pub body: Option<String>,
}

pub fn parse_rubric(html: &str) -> Result<Rubric> {
    static RE: OnceLock<[Regex; 3]> = OnceLock::new();
    let [in_force, earlier, title] = RE.get_or_init(|| {
        [
            Regex::new(r"Version en vigueur au (\d{2}/\d{2}/\d{4})").expect("valid regex"),
            Regex::new(r#"(?s)<option value="\d+"\s+data-url="([^"]+)">\s*Version du (\d{2}/\d{2}/\d{4})(?:&nbsp;|\s)*au (\d{2}/\d{2}/\d{4})"#)
                .expect("valid regex"),
            Regex::new(r"(?s)<h1>(.*?)</h1>").expect("valid regex"),
        ]
    });
    let from = in_force
        .captures(html)
        .and_then(|c| french_date(&c[1]))
        .ok_or_else(|| SourceError::Invalid("boss: rubric without a version in force".into()))?;
    let mut versions = vec![Version {
        from,
        until: None,
        url: None,
    }];
    versions.extend(earlier.captures_iter(html).filter_map(|c| {
        Some(Version {
            from: french_date(&c[2])?,
            until: french_date(&c[3]),
            url: Some(format!("{BASE_URL}{}", decode_entities(&c[1]))),
        })
    }));
    let content = content(html);
    let title = title
        .captures(content)
        .map(|c| clean(&c[1]))
        .ok_or_else(|| SourceError::Invalid("boss: rubric without a title".into()))?;
    let paragraphs = paragraphs(content);
    let body = paragraphs
        .is_empty()
        .then(|| strip_html(&noise().replace_all(content, "")))
        .filter(|text| !text.is_empty());
    Ok(Rubric {
        title,
        versions,
        paragraphs,
        body,
    })
}

/// An earlier version's fragment, the article alone.
pub fn parse_version(html: &str) -> Vec<Paragraph> {
    paragraphs(content(html))
}

/// Inside `<article class="colonne_droite_article">`.
fn content(html: &str) -> &str {
    let start = html
        .find("colonne_droite_article")
        .and_then(|at| html[at..].find('>').map(|end| at + end + 1))
        .unwrap_or(0);
    let end = html[start..]
        .find("</article>")
        .map_or(html.len(), |e| start + e);
    &html[start..end]
}

/// The heading, the update notice above the first chapter and the date of the last update.
fn noise() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<h1>.*?</h1>|<div class="(?:bloc_important|mise_a_jour)"[^>]*>.*?</div>"#)
            .expect("valid regex")
    })
}

fn paragraphs(article: &str) -> Vec<Paragraph> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let marker = RE.get_or_init(|| {
        Regex::new(r#"(?is)<span\b[^>]*class="paragraphe"[^>]*>(.*?)</span>"#).expect("valid regex")
    });
    let article = noise().replace_all(article, "");
    let (_, paragraphs) = split_numbered(&article, marker);
    paragraphs
}

fn clean(s: &str) -> String {
    decode_entities(&strip_html(s))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<select id="choix_version">
<option value="" selected="">
    Version en vigueur au 01/06/2026
</option>
<option value="11"  data-url="/portail/cms/render/live/fr/sites/siteboss/accueil/regles-dassujettissement/effectif/pageContent/thema-12/version-11.1.2.html.ajax">
    Version du 01/01/2026&nbsp;au 01/06/2026
</option>
<option value="v1">Version du 07/01/2003 au 22/01/2019</option>
</select>
<article  class="colonne_droite_article">
<h1>Comptabilisation des effectifs</h1>
<div class="bloc_important" tabindex="-1"><p>La rubrique est opposable à compter du 1er juin 2026.</p></div>
<h2 id="titre-chapitre-1">Chapitre 1 - Champ d&rsquo;application</h2>
<h3 id="titre-chapitre-1-section-1">Section 1 - Les seuils</h3>
<span id="10" class="paragraphe" tabindex="-1">10</span><p>L&rsquo;assujettissement des employeurs.</p>
<p class="texte_reference">Textes de r&eacute;f&eacute;rence&nbsp;: <a href="x">Article L. 130-1</a></p>
<span id="20" class="paragraphe" tabindex="-1">20</span><p>Les dispositions s&rsquo;appliquent.</p>
<div class="mise_a_jour"> Mis à jour le 08/06/2026 </div>
</article>"#;

    #[test]
    fn a_rubric_gives_its_versions_and_its_numbered_paragraphs_under_their_sections() {
        let r = parse_rubric(PAGE).expect("rubric");
        assert_eq!(r.title, "Comptabilisation des effectifs");
        assert_eq!(
            r.versions,
            vec![
                Version {
                    from: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                    until: None,
                    url: None,
                },
                Version {
                    from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                    until: NaiveDate::from_ymd_opt(2026, 6, 1),
                    url: Some("https://boss.gouv.fr/portail/cms/render/live/fr/sites/siteboss/accueil/regles-dassujettissement/effectif/pageContent/thema-12/version-11.1.2.html.ajax".into()),
                },
            ]
        );
        assert_eq!(r.paragraphs.len(), 2);
        assert_eq!(r.paragraphs[0].num, "10");
        assert_eq!(
            r.paragraphs[0].section_path,
            vec!["Chapitre 1 - Champ d’application", "Section 1 - Les seuils"]
        );
        assert!(r.paragraphs[0]
            .texte
            .starts_with("L’assujettissement des employeurs."));
        assert!(r.paragraphs[0].texte.contains("Article L. 130-1"));
        assert_eq!(r.paragraphs[1].texte, "Les dispositions s’appliquent.");
        assert_eq!(r.body, None);
    }

    #[test]
    fn a_rubric_without_numbered_paragraphs_gives_its_text() {
        let page = r#"<p>Version en vigueur au 01/01/2024</p>
<article class="colonne_droite_article">
<h1>Montant net social</h1>
<p>Le montant net social correspond aux revenus d&eacute;clar&eacute;s.</p>
<h2>Calcul</h2><p>Il est constitu&eacute; des sommes brutes.</p>
<div class="mise_a_jour"> Mis à jour le 08/06/2026 </div>
</article>"#;
        let r = parse_rubric(page).expect("rubric");
        assert!(r.paragraphs.is_empty());
        let body = r.body.expect("a body");
        assert!(body.starts_with("Le montant net social correspond aux revenus déclarés."));
        assert!(body.contains("Calcul"));
        assert!(!body.contains("Mis à jour"));
    }
}
