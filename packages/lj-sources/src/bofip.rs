//! BOFiP-Impôts (ADR 0196) : doctrine fiscale DGFiP opposable (LPF, art. L. 80 A).
//!
//! Source : dataset open data **`bofip-vigueur`** (data.economie.gouv.fr,
//! licence ouverte) — un record par publication doctrinale **en vigueur**
//! (~9 k), champs `identifiant_juridique` (`BOI-TVA-DECLA-20-30-20-30`),
//! `serie`/`division`, `titre` (fil d'Ariane complet), `debut_de_validite`,
//! `contenu_html`. Export JSONL streamé sur disque puis parsé ligne à ligne.
//!
//! Le corps HTML porte les **paragraphes numérotés** (`<p class=
//! "numero-de-paragraphe-western">130</p>` puis le contenu jusqu'au § suivant)
//! — l'unité que citent les décisions (« paragraphe n° 130 du BOI-… ») — et
//! des **intertitres** `<h1>`–`<h6>` (« I. », « A. », « 1. ») qui portent le
//! plan du document. Le parse découpe donc : préambule (avant le premier §) +
//! liste de §, chaque § portant son chemin d'intertitres englobants
//! (`section_path`) ; les intertitres sont retirés des textes. Les records
//! `type=Actualité` (annonces de mise à jour, non citables) sont ignorés.
//!
//! Chausse-trappe source : les attributs HTML arrivent avec des guillemets
//! doublés (`class=""paragraphe-western""`, artefact d'échappement CSV côté
//! Opendatasoft) — normalisés avant découpe.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;

use crate::downloader::http::get_to_file_retrying;
use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;

/// Export JSONL intégral du dataset (un record par ligne).
const BOFIP_EXPORT_URL: &str =
    "https://data.economie.gouv.fr/api/explore/v2.1/catalog/datasets/bofip-vigueur/exports/jsonl";

const BOFIP_USER_AGENT: &str = "librejustice-ingest/1.0 (+https://librejustice.fr)";

/// Télécharge l'export JSONL `bofip-vigueur` sous `dir` (streamé, RAM
/// ~constante). Toujours re-téléchargé : le dataset est un snapshot vivant
/// (publications en vigueur), pas un fond incrémental — l'idempotence vit dans
/// l'upsert par `content_checksum` (#7).
pub fn fetch_bofip_vigueur(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let dst = dir.join("bofip-vigueur.jsonl");
    let tmp = dst.with_extension("jsonl.part");
    let client = reqwest::blocking::Client::builder()
        .user_agent(BOFIP_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(3600))
        .build()?;
    let (status, _, octets) =
        crate::dila::with_stall_watchdog("GET bofip-vigueur", Duration::from_secs(600), || {
            get_to_file_retrying(BOFIP_EXPORT_URL, &tmp, || {
                client.get(BOFIP_EXPORT_URL).send()
            })
        })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "export bofip-vigueur statut {status}"
        )));
    }
    fs::rename(&tmp, &dst)?;
    tracing::info!(octets, "bofip_vigueur téléchargé");
    Ok(dst)
}

/// Record brut du dataset (frontière serde, #12).
#[derive(Debug, Deserialize)]
pub struct BofipRecord {
    #[serde(rename = "type")]
    pub kind: String,
    pub titre: Option<String>,
    pub debut_de_validite: Option<String>,
    pub serie: Option<String>,
    pub division: Option<String>,
    pub identifiant_juridique: Option<String>,
    pub permalien: Option<String>,
    pub contenu_html: Option<String>,
}

/// Un paragraphe numéroté (`§ 130`) d'un document BOFiP.
#[derive(Debug, PartialEq, Eq)]
pub struct BofipParagraph {
    /// Numéro du § tel que publié (« 1 », « 130 »).
    pub num: String,
    pub texte: String,
    /// Intertitres englobants au point du § (du plus large au plus fin,
    /// « I. … » puis « A. … ») — le plan du document, hors texte des §.
    pub section_path: Vec<String>,
}

/// Un document doctrinal BOFiP en vigueur, prêt pour l'ingest.
#[derive(Debug)]
pub struct BofipDoc {
    /// Identifiant juridique = identité (`BOI-TVA-DECLA-20-30-20-30`).
    pub identifiant: String,
    /// Fil d'Ariane complet (« TVA - Régimes d'imposition - … »).
    pub titre: String,
    pub serie: String,
    pub division: Option<String>,
    /// Date ISO de début de validité de la version en vigueur.
    pub debut_de_validite: String,
    /// URL officielle versionnée (`bofip.impots.gouv.fr/bofip/…-PGP.html/…`) —
    /// non dérivable de l'identifiant (numéro PGP opaque), donc portée en source.
    pub permalien: Option<String>,
    /// Texte avant le premier § numéroté (introduction non citable).
    pub preambule: Option<String>,
    pub paragraphs: Vec<BofipParagraph>,
}

/// Parse une ligne JSONL de l'export. `Ok(None)` = record hors périmètre
/// (`type=Actualité`). Champ d'identité manquant = erreur franche (#12).
pub fn parse_bofip_record(line: &str) -> Result<Option<BofipDoc>> {
    let rec: BofipRecord = serde_json::from_str(line)
        .map_err(|e| SourceError::Invalid(format!("record bofip illisible: {e}")))?;
    if rec.kind != "Contenu" {
        return Ok(None);
    }
    let identifiant = rec
        .identifiant_juridique
        .ok_or_else(|| SourceError::Invalid("record bofip Contenu sans identifiant".into()))?;
    let titre = rec
        .titre
        .ok_or_else(|| SourceError::Invalid(format!("bofip {identifiant}: titre absent")))?;
    let debut = rec.debut_de_validite.ok_or_else(|| {
        SourceError::Invalid(format!("bofip {identifiant}: debut_de_validite absent"))
    })?;
    let serie = rec
        .serie
        .ok_or_else(|| SourceError::Invalid(format!("bofip {identifiant}: serie absente")))?;
    let (preambule, paragraphs) = split_paragraphs(rec.contenu_html.as_deref().unwrap_or(""));
    Ok(Some(BofipDoc {
        identifiant,
        titre,
        serie,
        division: rec.division,
        debut_de_validite: debut,
        permalien: rec.permalien,
        preambule,
        paragraphs,
    }))
}

/// Découpe le corps HTML en (préambule, §). Deux types de coupes : marqueur de
/// § (`<p class="numero-de-paragraphe-western">N</p>`) et intertitre
/// (`<h1>`–`<h6>`). Le texte entre deux coupes va au § ouvert (au préambule
/// avant le premier) ; les intertitres tiennent la pile de sections courante
/// et sortent des textes. Sans aucun marqueur de §, tout le corps devient le
/// préambule (documents-plans, corps courts).
fn split_paragraphs(html: &str) -> (Option<String>, Vec<BofipParagraph>) {
    static MARKER_RE: OnceLock<Regex> = OnceLock::new();
    static HEADING_RE: OnceLock<Regex> = OnceLock::new();
    let re = MARKER_RE.get_or_init(|| {
        Regex::new(r#"(?is)<p\b[^>]*numero-de-paragraphe-western[^>]*>(.*?)</p>"#)
            .expect("regex marqueur § valide")
    });
    let hre = HEADING_RE.get_or_init(|| {
        Regex::new(r#"(?is)<h([1-6])\b[^>]*>(.*?)</h[1-6]\s*>"#).expect("regex intertitre valide")
    });
    // Guillemets doublés (artefact CSV Opendatasoft) → HTML propre.
    let html = html.replace("\"\"", "\"");

    enum Cut {
        Para(String),
        Heading(u8, String),
    }
    let mut cuts: Vec<(usize, usize, Cut)> = re
        .captures_iter(&html)
        .filter_map(|c| {
            let m = c.get(0).expect("groupe 0 toujours présent");
            let num: String = strip_html(&c[1]).trim().to_string();
            // Un marqueur vide/non numérique n'ouvre pas de § (bruit de mise en page).
            if num.is_empty() || !num.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            Some((m.start(), m.end(), Cut::Para(num)))
        })
        .collect();
    cuts.extend(hre.captures_iter(&html).filter_map(|c| {
        let m = c.get(0).expect("groupe 0 toujours présent");
        let level: u8 = c[1].parse().expect("niveau h1-h6 à un chiffre");
        let title = strip_html(&c[2]).trim().to_string();
        // Un intertitre vide n'est pas une section (bruit de mise en page).
        (!title.is_empty()).then(|| (m.start(), m.end(), Cut::Heading(level, title)))
    }));
    cuts.sort_by_key(|(start, _, _)| *start);

    let mut stack: Vec<(u8, String)> = Vec::new();
    let mut preambule = String::new();
    let mut paragraphs: Vec<BofipParagraph> = Vec::new();
    let mut cursor = 0usize;
    let mut push_text = |chunk: &str, paragraphs: &mut Vec<BofipParagraph>| {
        let text = strip_html(chunk);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let dst = match paragraphs.last_mut() {
            Some(p) => &mut p.texte,
            None => &mut preambule,
        };
        if !dst.is_empty() {
            dst.push('\n');
        }
        dst.push_str(text);
    };
    for (start, end, cut) in cuts {
        push_text(&html[cursor..start], &mut paragraphs);
        cursor = end;
        match cut {
            Cut::Heading(level, title) => {
                while stack.last().is_some_and(|(l, _)| *l >= level) {
                    stack.pop();
                }
                stack.push((level, title));
                // Un § encore vide devant l'intertitre en est le porte-numéro
                // (layout BOFiP : « 20 » étiquette le titre « II. … ») : il
                // adopte la section qu'il ouvre.
                if let Some(p) = paragraphs.last_mut() {
                    if p.texte.is_empty() {
                        p.section_path = stack.iter().map(|(_, t)| t.clone()).collect();
                    }
                }
            }
            Cut::Para(num) => paragraphs.push(BofipParagraph {
                num,
                texte: String::new(),
                section_path: stack.iter().map(|(_, t)| t.clone()).collect(),
            }),
        }
    }
    push_text(&html[cursor..], &mut paragraphs);

    (non_empty(preambule), paragraphs)
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_preambule_puis_paragraphes() {
        let html = r#"<p class=""paragraphe-western"">Intro du document.</p>
            <h1 id=""x"">I. Section</h1>
            <p class=""numero-de-paragraphe-western"" id=""1_00"">1</p>
            <p class=""paragraphe-western"">Contenu du § 1.</p>
            <p class=""numero-de-paragraphe-western"" id=""10_0"">10</p>
            <p class=""paragraphe-western"">Contenu du § 10, phrase A.</p>
            <p class=""paragraphe-western"">Phrase B.</p>"#;
        let (pre, paras) = split_paragraphs(html);
        // Le préambule = le texte avant le premier §, intertitres exclus
        // (« I. Section » devient le chemin de section des §).
        assert_eq!(pre.as_deref(), Some("Intro du document."));
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].num, "1");
        assert_eq!(paras[0].texte, "Contenu du § 1.");
        assert_eq!(paras[0].section_path, vec!["I. Section"]);
        assert_eq!(paras[1].num, "10");
        assert!(paras[1].texte.contains("phrase A"));
        assert!(paras[1].texte.contains("Phrase B"));
        assert_eq!(paras[1].section_path, vec!["I. Section"]);
    }

    #[test]
    fn intertitres_hierarchie_et_hors_texte() {
        // h2 entre le § 30 et le § 40 : il sort du texte du § 30 et entre dans
        // le chemin du § 40 ; un h1 suivant vide la pile (dépile les niveaux ≥).
        let html = r#"<h1>I. Premier</h1>
            <p class=""numero-de-paragraphe-western"">30</p>
            <p>Fin du § 30.</p>
            <h2>A. Sous-section</h2>
            <p class=""numero-de-paragraphe-western"">40</p>
            <p>Corps du § 40.</p>
            <h1>II. Second</h1>
            <p class=""numero-de-paragraphe-western"">50</p>
            <p>Corps du § 50.</p>"#;
        let (pre, paras) = split_paragraphs(html);
        assert_eq!(pre, None);
        assert_eq!(paras[0].texte, "Fin du § 30.");
        assert_eq!(paras[0].section_path, vec!["I. Premier"]);
        assert_eq!(paras[1].section_path, vec!["I. Premier", "A. Sous-section"]);
        assert_eq!(paras[1].texte, "Corps du § 40.");
        assert_eq!(paras[2].section_path, vec!["II. Second"]);
    }

    #[test]
    fn paragraphe_porte_numero_conserve_et_rattache_a_sa_section() {
        // Layout BOFiP fréquent : le marqueur de § précède l'intertitre qu'il
        // étiquette (« 20 » devant « II. … »). Le § reste (ancre citable,
        // texte vide) et adopte la section qu'il ouvre.
        let html = r#"<p class=""numero-de-paragraphe-western"">10</p>
            <p>Corps du § 10.</p>
            <p class=""numero-de-paragraphe-western"">20</p>
            <h1>II. Section ouverte par le § 20</h1>
            <p class=""numero-de-paragraphe-western"">30</p>
            <p>Corps du § 30.</p>"#;
        let (_, paras) = split_paragraphs(html);
        assert_eq!(paras.len(), 3);
        assert_eq!(paras[1].num, "20");
        assert_eq!(paras[1].texte, "");
        assert_eq!(
            paras[1].section_path,
            vec!["II. Section ouverte par le § 20"]
        );
        assert_eq!(
            paras[2].section_path,
            vec!["II. Section ouverte par le § 20"]
        );
    }

    #[test]
    fn corps_sans_marqueur_devient_preambule() {
        let (pre, paras) = split_paragraphs("<p>Plan de la série.</p>");
        assert_eq!(pre.as_deref(), Some("Plan de la série."));
        assert!(paras.is_empty());
    }

    #[test]
    fn record_actualite_ignore() {
        let line = r#"{"type":"Actualité","titre":"Mise à jour","debut_de_validite":"2026-01-01","serie":null,"division":null,"identifiant_juridique":null,"permalien":null,"contenu":null,"contenu_html":null}"#;
        assert!(parse_bofip_record(line).unwrap().is_none());
    }
}
