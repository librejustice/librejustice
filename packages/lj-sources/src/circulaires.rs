//! Fond DILA CIRCULAIRES (ADR 0196) : circulaires et instructions applicables
//! signalées par les ministères (`OPENDATA/CIRCULAIRES/`, ≥ 2009).
//!
//! **Layout non-Freemium** (recon 2026-07-10) — trois familles de fichiers :
//! - stocks `xml/<année>_xml.tar.gz` (2009→2014, figés) — arbo interne
//!   `MM/cir_<ID>.xml` ;
//! - `txt/abroge_txt.tar` (2013→2015-01, figé) — listes d'abrogation/suppression
//!   par chemins ;
//! - flux `FLUX/<année>/circulaire_<jjmmaa><hh>h<mm>.tar.gz` +
//!   `FLUX/Annee_en_cours/` — créations ET mises à jour **last-write-wins par
//!   `ID_CIRCULAIRE`** (une abrogation moderne = re-export `ETAT=A`), arbo
//!   interne `xml/YYYY/MM/cir_<ID>.xml` (+ `pdf/…`).
//!
//! Le XML est de la **métadonnée pure** (titre, NOR, dates, état V/A, résumé,
//! textes de référence) — le corps n'existe qu'en PDF (`pdf/`, ~19 Go), track
//! ultérieur. Le pivot d'identité est `ID_CIRCULAIRE` (le NOR est vide sur
//! ~14 % du stock 2009).
//!
//! Chausse-trappes portées par ce module : UTF-8 tronqué/mojibake du stock
//! 2009 (lecture lossy), dérive de schéma silencieuse (`OPPOSABLE` absent du
//! XSD, listes auto-fermées) → arbre `XmlNode` tolérant, pas de serde strict.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::dila::{http_client, with_stall_watchdog};
use crate::downloader::http::{get_to_file_retrying, get_with_body_retrying};
use crate::error::{Result, SourceError};

const CIRC_BASE_URL: &str = "https://echanges.dila.gouv.fr/OPENDATA/CIRCULAIRES";

/// Un fichier du fond à ingérer, dans l'ordre de rejeu.
#[derive(Debug)]
pub struct CircTarball {
    /// Nom relatif au fond (`xml/2009_xml.tar.gz`, `FLUX/2025/circulaire_….tar.gz`,
    /// `txt/abroge_txt.tar`) — clé du manifest.
    pub name: String,
    /// Copie locale téléchargée.
    pub path: PathBuf,
    pub kind: CircKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircKind {
    /// Stock annuel de métadonnées (`MM/cir_<ID>.xml`).
    Stock,
    /// Listes d'abrogation/suppression historiques (fichiers `.txt` dans un tar).
    Abroge,
    /// Tarball de flux (`xml/YYYY/MM/cir_<ID>.xml`, upsert last-write-wins).
    Flux,
}

/// Un tarball PDF du fond à rejouer pour la passe corps (ADR 0222) : stock
/// `pdf/<année>_pdf.tar.gz` ou tarball de flux (PDF compagnons du XML).
#[derive(Debug)]
pub struct CircPdfTarball {
    /// Nom relatif au fond — clé du manifest `pdf-manifest.json`.
    pub name: String,
    /// Copie locale téléchargée.
    pub path: PathBuf,
}

/// Planifie un sync : liste le fond, télécharge ce qui manque localement, et
/// renvoie les fichiers **pas encore ingérés** (absents du manifest) dans
/// l'ordre de rejeu correct : stocks (par année) → abrogations historiques →
/// flux (ordre chronologique du nom). L'appelant ingère puis appelle
/// [`mark_circulaire_done`] par fichier — un crash au milieu reprend où il
/// s'était arrêté (les téléchargements présents sont skippés).
pub fn plan_circulaires_sync(dir: &Path) -> Result<Vec<CircTarball>> {
    fs::create_dir_all(dir)?;
    let done = read_manifest(dir, MANIFEST)?;
    let client = http_client()?;

    let mut plan: Vec<(String, CircKind)> = Vec::new();

    // 1. Stocks annuels de métadonnées (xml/), triés par année (ordre lexical).
    let mut stocks = list_dir(&client, "xml/")?
        .into_iter()
        .filter(|n| stock_re().is_match(n))
        .collect::<Vec<_>>();
    stocks.sort();
    plan.extend(
        stocks
            .into_iter()
            .map(|n| (format!("xml/{n}"), CircKind::Stock)),
    );

    // 2. Abrogations/suppressions historiques (fond figé 2013→2015-01).
    plan.push(("txt/abroge_txt.tar".to_string(), CircKind::Abroge));

    // 3. Flux : toutes les années listées + l'année en cours, chaque dossier
    //    trié par la date encodée dans le nom (jjmmaa → clé aammjj).
    plan.extend(
        list_flux_names(&client)?
            .into_iter()
            .map(|n| (n, CircKind::Flux)),
    );

    // Téléchargement de ce qui n'est pas déjà sur disque, retour du non-ingéré.
    let mut out = Vec::new();
    for (name, kind) in plan {
        if done.contains(&name) {
            continue;
        }
        let local = dir.join(name.replace('/', "_"));
        if !local.exists() {
            download(&client, &name, &local)?;
        }
        out.push(CircTarball {
            name,
            path: local,
            kind,
        });
    }
    Ok(out)
}

/// Planifie la passe corps (ADR 0222) : stocks PDF (`pdf/<année>_pdf.tar.gz`,
/// par année) puis flux (les tarballs de flux, partagés avec le sync
/// métadonnées, sont réutilisés depuis le cache ; téléchargés au besoin).
/// Renvoie les fichiers absents du manifest `pdf-manifest.json` dans l'ordre
/// de rejeu last-write-wins par `ID_CIRCULAIRE`.
pub fn plan_circulaires_pdf_sync(dir: &Path) -> Result<Vec<CircPdfTarball>> {
    fs::create_dir_all(dir)?;
    let done = read_manifest(dir, PDF_MANIFEST)?;
    let client = http_client()?;

    let mut stocks = list_dir(&client, "pdf/")?
        .into_iter()
        .filter(|n| pdf_stock_re().is_match(n))
        .collect::<Vec<_>>();
    stocks.sort();
    let mut plan: Vec<String> = stocks.into_iter().map(|n| format!("pdf/{n}")).collect();
    plan.extend(list_flux_names(&client)?);

    let mut out = Vec::new();
    for name in plan {
        if done.contains(&name) {
            continue;
        }
        let local = dir.join(name.replace('/', "_"));
        if !local.exists() {
            download(&client, &name, &local)?;
        }
        out.push(CircPdfTarball { name, path: local });
    }
    Ok(out)
}

/// Noms de flux ordonnés (`FLUX/<année>/<nom>`) : années croissantes,
/// `Annee_en_cours/` en dernier (avant les chiffres en ASCII), chaque dossier
/// trié par la clé chronologique du nom.
fn list_flux_names(client: &reqwest::blocking::Client) -> Result<Vec<String>> {
    let mut year_dirs = list_dir(client, "FLUX/")?
        .into_iter()
        .filter(|n| n.ends_with('/'))
        .collect::<Vec<_>>();
    year_dirs.sort();
    if let Some(pos) = year_dirs.iter().position(|d| d == "Annee_en_cours/") {
        let cur = year_dirs.remove(pos);
        year_dirs.push(cur);
    }
    let mut out = Vec::new();
    for ydir in year_dirs {
        let mut flux: Vec<String> = list_dir(client, &format!("FLUX/{ydir}"))?
            .into_iter()
            .filter(|n| flux_key(n).is_some())
            .collect();
        flux.sort_by_key(|n| flux_key(n).expect("filtré sur flux_key"));
        out.extend(flux.into_iter().map(|n| format!("FLUX/{ydir}{n}")));
    }
    Ok(out)
}

/// Manifest du sync métadonnées (XML) / de la passe corps (PDF, ADR 0222).
const MANIFEST: &str = "manifest.json";
const PDF_MANIFEST: &str = "pdf-manifest.json";

/// Marque un fichier comme ingéré par le sync métadonnées (idempotent).
pub fn mark_circulaire_done(dir: &Path, name: &str) -> Result<()> {
    mark_done(dir, MANIFEST, name)
}

/// Marque un tarball comme rejoué par la passe corps (idempotent).
pub fn mark_circulaire_pdf_done(dir: &Path, name: &str) -> Result<()> {
    mark_done(dir, PDF_MANIFEST, name)
}

fn mark_done(dir: &Path, manifest: &str, name: &str) -> Result<()> {
    let mut done = read_manifest(dir, manifest)?;
    done.insert(name.to_string());
    let path = dir.join(manifest);
    let json = serde_json::to_string_pretty(&done.iter().collect::<Vec<_>>())
        .map_err(|e| SourceError::Invalid(format!("manifest circulaires: {e}")))?;
    fs::write(path, json)?;
    Ok(())
}

fn read_manifest(dir: &Path, manifest: &str) -> Result<BTreeSet<String>> {
    let path = dir.join(manifest);
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let raw = fs::read_to_string(&path)?;
    let names: Vec<String> = serde_json::from_str(&raw)
        .map_err(|e| SourceError::Invalid(format!("manifest circulaires illisible: {e}")))?;
    Ok(names.into_iter().collect())
}

fn stock_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{4}_xml\.tar\.gz$").expect("regex stock valide"))
}

fn pdf_stock_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{4}_pdf\.tar\.gz$").expect("regex stock pdf valide"))
}

/// Clé chronologique d'un nom de flux `circulaire_<jj><mm><aa><hh>h<mm>.tar.gz`
/// (jour-premier dans le nom → clé `(aa, mm, jj, hh, min)`). `None` = pas un flux.
fn flux_key(name: &str) -> Option<(u16, u8, u8, u8, u8)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^circulaire_(\d{2})(\d{2})(\d{2})(\d{2})h(\d{2})\.tar\.gz$")
            .expect("regex flux valide")
    });
    let c = re.captures(name)?;
    let p = |i: usize| c[i].parse::<u8>().ok();
    Some((c[3].parse::<u16>().ok()? + 2000, p(2)?, p(1)?, p(4)?, p(5)?))
}

/// Liste les `href` d'un répertoire du fond (listing Apache), basenames only.
fn list_dir(client: &reqwest::blocking::Client, rel: &str) -> Result<Vec<String>> {
    static HREF_RE: OnceLock<Regex> = OnceLock::new();
    let re = HREF_RE
        .get_or_init(|| Regex::new(r#"href="(?P<href>[^"?]+)""#).expect("regex href valide"));
    let url = format!("{CIRC_BASE_URL}/{rel}");
    let (status, _, body) = with_stall_watchdog("GET listing", Duration::from_secs(60), || {
        get_with_body_retrying(&url, || client.get(&url).send())
    })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "listing CIRCULAIRES {rel} statut {status}"
        )));
    }
    let bytes = body.expect("statut 200 → corps lu");
    let html = String::from_utf8_lossy(&bytes);
    Ok(re
        .captures_iter(&html)
        .filter_map(|c| {
            let href = &c["href"];
            // Basenames only : liens de tri/parent exclus.
            if href.starts_with('/') || href.starts_with("http") {
                return None;
            }
            Some(href.to_string())
        })
        .collect())
}

fn download(client: &reqwest::blocking::Client, name: &str, dst: &Path) -> Result<()> {
    let url = format!("{CIRC_BASE_URL}/{name}");
    let tmp = dst.with_extension("part");
    let (status, _, octets) =
        with_stall_watchdog(&format!("GET {name}"), Duration::from_secs(600), || {
            get_to_file_retrying(&url, &tmp, || client.get(&url).send())
        })?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "téléchargement CIRCULAIRES {name} statut {status}"
        )));
    }
    fs::rename(&tmp, dst)?;
    tracing::info!(name, octets, "circulaires_tarball");
    Ok(())
}

// ── Parsing des membres XML ───────────────────────────────────────────────────

/// Métadonnées d'une circulaire (le corps est en PDF, hors v1).
#[derive(Debug, PartialEq, Eq)]
pub struct Circulaire {
    /// Pivot d'identité : `cir_<ID_CIRCULAIRE>` (= stem des fichiers du fond).
    pub id: String,
    /// `V` (vigueur) ou `A` (abrogée/retirée de la diffusion).
    pub etat: String,
    pub titre: String,
    /// NOR — vide sur ~14 % du stock 2009 → `None`.
    pub nor: Option<String>,
    /// `DATE_SIGNATURE` ISO brute (jamais vide ; valeurs aberrantes possibles —
    /// stockée telle quelle, la validation vit à la frontière du pipeline).
    pub date_signature: Option<String>,
    /// Mise en ligne : `DATE_EXPORT`, repli `DATE_DEPOT`.
    pub date_publi: Option<String>,
    pub auteur: Option<String>,
    pub signataire: Option<String>,
    /// Résumé (≤ 5 000 caractères) — seul « corps » disponible hors PDF.
    pub resume: Option<String>,
    /// Chemin du PDF compagnon (`/pdf/YYYY/MM/cir_<ID>.pdf`) — corps, track futur.
    pub pdf_path: Option<String>,
    /// `TEXTE_REF` (textes cités par la circulaire — candidats `legal_link`).
    pub textes_ref: Vec<String>,
}

/// Parse un membre `cir_<ID>.xml` (stock ou flux). Lecture **lossy** (UTF-8
/// tronqué/mojibake du stock 2009). Champ d'identité manquant = erreur franche.
pub fn parse_circulaire_xml(raw: &[u8]) -> Result<Circulaire> {
    // Lossy systématique : 5 fichiers 2009 portent un RESUME tronqué en pleine
    // séquence multi-octets ; le reste passe inchangé.
    let cleaned = String::from_utf8_lossy(raw);
    let root = lj_core::parsing::build_tree(cleaned.as_bytes())
        .ok_or_else(|| SourceError::Invalid("circulaire: XML illisible".into()))?;

    let text = |tag: &str| -> Option<String> {
        root.find(tag)
            .and_then(|n| n.text())
            .map(|s| normalize_ws(&s))
            .filter(|s| !s.is_empty())
    };

    let id_num = text("ID_CIRCULAIRE")
        .ok_or_else(|| SourceError::Invalid("circulaire sans ID_CIRCULAIRE".into()))?;
    let titre = text("TITRE")
        .ok_or_else(|| SourceError::Invalid(format!("circulaire {id_num} sans TITRE")))?;
    let etat = text("ETAT").unwrap_or_else(|| "V".to_string());

    let textes_ref = root
        .find("TEXTES_DE_REFERENCE")
        .map(|n| {
            n.children
                .iter()
                .filter(|c| c.tag == "TEXTE_REF")
                .filter_map(|c| c.text())
                .map(|s| normalize_ws(&s))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Ok(Circulaire {
        id: format!("cir_{id_num}"),
        etat,
        titre,
        nor: text("NUMERO_NOR"),
        date_signature: text("DATE_SIGNATURE"),
        date_publi: text("DATE_EXPORT").or_else(|| text("DATE_DEPOT")),
        auteur: text("AUTEUR"),
        signataire: text("SIGNATAIRE"),
        resume: text("RESUME"),
        pdf_path: text("NOM_FICHIER_PDF"),
        textes_ref,
    })
}

/// Entrée d'une liste d'abrogation/suppression historique (`abroge_txt.tar`).
/// Chaque ligne = chemin relatif (`2011/07/cir_33411.pdf` ou `.xml`, parfois
/// préfixé `A_`) → id `cir_<ID>` dédupliqué.
pub fn parse_abroge_list(raw: &[u8]) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"cir_(\d+)").expect("regex abroge valide"));
    let text = String::from_utf8_lossy(raw);
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        if let Some(c) = re.captures(line) {
            seen.insert(format!("cir_{}", &c[1]));
        }
    }
    seen.into_iter().collect()
}

/// Membre XML d'un tarball du fond (`MM/cir_N.xml` en stock,
/// `xml/YYYY/MM/cir_N.xml` en flux) ? Les PDF compagnons sont ignorés en v1.
pub fn is_circulaire_xml_member(name: &str) -> bool {
    let stem = name.rsplit('/').next().unwrap_or(name);
    stem.starts_with("cir_") && stem.ends_with(".xml")
}

/// Id `cir_<N>` d'un membre PDF du fond (`MM/cir_N.pdf` en stock,
/// `pdf/YYYY/MM/cir_N.pdf` en flux). `None` = pas un PDF de circulaire.
pub fn circulaire_pdf_member_id(name: &str) -> Option<String> {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let id = stem.strip_suffix(".pdf")?;
    id.starts_with("cir_").then(|| id.to_string())
}

/// Relit le markdown OCR caché d'une circulaire scannée
/// (`<dir>/ocr/<id>.md`, ADR 0222). `None` si absent.
pub fn load_cached_circulaire_ocr(dir: &Path, id: &str) -> Result<Option<String>> {
    let path = circ_ocr_path(dir, id);
    if path.exists() {
        Ok(Some(fs::read_to_string(&path)?))
    } else {
        Ok(None)
    }
}

/// Écrit le markdown OCR au cache (atomique : fichier temporaire + rename).
pub fn save_cached_circulaire_ocr(dir: &Path, id: &str, markdown: &str) -> Result<()> {
    let path = circ_ocr_path(dir, id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, markdown)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn circ_ocr_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("ocr").join(format!("{id}.md"))
}

/// Dépose un PDF scanné en file d'attente OCR (`<dir>/ocr-pending/<id>.pdf`,
/// atomique). Écrit quand le repli OCR échoue (pas de clé / pool épuisé) : le
/// retry ne coûte que ce fichier, jamais un re-stream du tarball d'origine.
pub fn save_pending_circulaire_pdf(dir: &Path, id: &str, pdf: &[u8]) -> Result<()> {
    let path = circ_pending_path(dir, id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pdf.tmp");
    fs::write(&tmp, pdf)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Liste la file d'attente OCR : `(id, chemin)` triés par id.
pub fn list_pending_circulaire_pdfs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let pending = dir.join("ocr-pending");
    if !pending.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&pending)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(id) = circulaire_pdf_member_id(name) {
            out.push((id, path));
        }
    }
    out.sort();
    Ok(out)
}

/// Retire une entrée résolue de la file d'attente OCR.
pub fn remove_pending_circulaire_pdf(dir: &Path, id: &str) -> Result<()> {
    fs::remove_file(circ_pending_path(dir, id))?;
    Ok(())
}

fn circ_pending_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("ocr-pending").join(format!("{id}.pdf"))
}

/// CR encodés (`&#13;`) et blancs multiples → un espace simple.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<CIRCULAIRE><UTILISATEUR_DEPOSANT_MINISTERE>13</UTILISATEUR_DEPOSANT_MINISTERE><NOM_FICHIER_PDF>/pdf/2025/12/cir_45634.pdf</NOM_FICHIER_PDF><ID_CIRCULAIRE>45634</ID_CIRCULAIRE><TAILLE_FICHIER_PDF>196883</TAILLE_FICHIER_PDF><ETAT>V</ETAT><DATE_DEPOT>2025-12-22</DATE_DEPOT><DATE_EXPORT>2025-12-22</DATE_EXPORT><LISTE_MINISTERES_DEPOSANTS/><LISTE_DOMAINES><DOMAINE>Budget, fiscalité</DOMAINE></LISTE_DOMAINES><TITRE>Instruction n° 6514/SG du 15 décembre 2025 relative à la communication</TITRE><NUMERO_NOR>PRMX2536323J</NUMERO_NOR><NUMERO_INTERNE></NUMERO_INTERNE><DATE_SIGNATURE>2025-12-15</DATE_SIGNATURE><AUTEUR>Premier ministre</AUTEUR><RESUME>Stratégie de communication.</RESUME><TEXTES_DE_REFERENCE><TEXTE_REF>Circulaire du 12 mai 1998</TEXTE_REF><URL_TEXTE_REF></URL_TEXTE_REF><TEXTE_REF>Décret n° 2000-1027</TEXTE_REF><URL_TEXTE_REF></URL_TEXTE_REF></TEXTES_DE_REFERENCE><REMPLACE></REMPLACE><SIGNATAIRE>S. L.</SIGNATAIRE><OPPOSABLE>n</OPPOSABLE></CIRCULAIRE>"#;

    #[test]
    fn parse_flux_moderne() {
        let c = parse_circulaire_xml(SAMPLE.as_bytes()).unwrap();
        assert_eq!(c.id, "cir_45634");
        assert_eq!(c.etat, "V");
        assert_eq!(c.nor.as_deref(), Some("PRMX2536323J"));
        assert_eq!(c.date_signature.as_deref(), Some("2025-12-15"));
        assert_eq!(c.date_publi.as_deref(), Some("2025-12-22"));
        assert_eq!(c.pdf_path.as_deref(), Some("/pdf/2025/12/cir_45634.pdf"));
        assert_eq!(
            c.textes_ref,
            vec!["Circulaire du 12 mai 1998", "Décret n° 2000-1027"]
        );
    }

    #[test]
    fn champs_optionnels_absents_toleres() {
        // Schéma minimal (dérive : balises absentes, listes vides).
        let xml = r#"<CIRCULAIRE><ID_CIRCULAIRE>26000</ID_CIRCULAIRE><ETAT>A</ETAT><TITRE>Vieille circulaire</TITRE><NUMERO_NOR></NUMERO_NOR></CIRCULAIRE>"#;
        let c = parse_circulaire_xml(xml.as_bytes()).unwrap();
        assert_eq!(c.id, "cir_26000");
        assert_eq!(c.etat, "A");
        assert_eq!(c.nor, None);
        assert!(c.textes_ref.is_empty());
    }

    #[test]
    fn utf8_tronque_lossy() {
        // RESUME coupé en pleine séquence multi-octets (é = 0xC3 0xA9) : le
        // parse ne panique pas et l'identité survit.
        let mut raw =
            br#"<CIRCULAIRE><ID_CIRCULAIRE>26042</ID_CIRCULAIRE><TITRE>T</TITRE><RESUME>caf"#
                .to_vec();
        raw.push(0xC3); // séquence tronquée
        raw.extend_from_slice("</RESUME></CIRCULAIRE>".as_bytes());
        let c = parse_circulaire_xml(&raw).unwrap();
        assert_eq!(c.id, "cir_26042");
    }

    #[test]
    fn ordre_chronologique_des_flux() {
        // jj-mm-aa dans le nom : l'ordre lexical est FAUX, la clé le corrige.
        let mut names = vec![
            "circulaire_02012618h00.tar.gz", // 2 janv. 2026
            "circulaire_22122518h00.tar.gz", // 22 déc. 2025
            "circulaire_05062615h30.tar.gz", // 5 juin 2026
        ];
        names.sort_by_key(|n| flux_key(n).unwrap());
        assert_eq!(
            names,
            vec![
                "circulaire_22122518h00.tar.gz",
                "circulaire_02012618h00.tar.gz",
                "circulaire_05062615h30.tar.gz",
            ]
        );
    }

    #[test]
    fn abroge_list_dedup() {
        let raw = b"2011/07/cir_33411.pdf\n2011/07/cir_33411.xml\n2012/01/A_cir_100.xml\n";
        assert_eq!(parse_abroge_list(raw), vec!["cir_100", "cir_33411"]);
    }
}
