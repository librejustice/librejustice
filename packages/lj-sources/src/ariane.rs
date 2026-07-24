//! ArianeWeb (Conseil d'État, moteur Sinequa `engine2`) — commentaires
//! institutionnels des décisions CE (ADR 0204 ; savoir source ADR 0095) :
//! analyses AJCE (sommaires doctrinaux) + existence des conclusions CRP.
//!
//! Deux canaux, sans auth :
//! - `xsearch?type=json` filtré `SourceStr4=<FOND>&SourceInt1=<année>` et paginé
//!   par `PageNumber` (20 hits/page servis par le serveur — vérifié live
//!   2026-07-11 ; `PageSize`/`SkipCount` sont ignorés). L'année d'un hit est
//!   celle de la **date de lecture** de la décision parente : analyses et
//!   conclusions d'une même décision tombent dans la même tranche annuelle.
//! - `downloadFilePagePlugin` pour l'HTML plein d'une analyse AJCE (le hit JSON
//!   ne porte que les titres analytiques tronqués à 1 544 car. ; les sommaires
//!   complets ne sont que dans l'HTML). L'`Id` garde son pipe **non URL-encodé**.
//!
//! Frontière d'encodage (#12) : HTML décodé ici en latin-1 (le header HTTP dit
//! vrai pour l'HTML) + nettoyage du `0x19` (apostrophe Windows-1252 mal mappée).
//! Le PDF CRP n'est jamais téléchargé : l'ADR 0204 ne stocke que l'existence.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::downloader::http::get_with_body_retrying;
use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://www.conseil-etat.fr";
/// User-Agent de contact (scraping courtois d'un site public sans API).
pub const USER_AGENT: &str = "librejustice-ariane/0.1 (+https://librejustice.fr)";
/// Throttle de courtoisie entre requêtes réseau (appliqué par l'appelant).
pub const THROTTLE: Duration = Duration::from_millis(500);

/// Fonds ArianeWeb énumérés. `AW_DCE` (décisions) n'est pas un fond d'ingest :
/// son id ne sert que de pivot de fratrie (`source_uid = ariane-web/<DCE>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArianeFond {
    /// Analyses du CE (~108 k, HTML).
    Ajce,
    /// Conclusions du rapporteur public (~9 k, PDF — jamais téléchargé).
    Crp,
}

impl ArianeFond {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ajce => "AW_AJCE",
            Self::Crp => "AW_CRP",
        }
    }
}

/// Une page de résultats `xsearch` (pagination serveur : 20 hits/page).
#[derive(Debug, Deserialize)]
pub struct ArianeSearchPage {
    #[serde(rename = "TotalCount")]
    pub total_count: u32,
    #[serde(rename = "PageCount")]
    pub page_count: u32,
    #[serde(rename = "CurrentPage")]
    pub current_page: u32,
    #[serde(rename = "Documents")]
    pub documents: Vec<ArianeHit>,
}

/// Hit `xsearch` — sous-ensemble des champs `SourceStrN`/`SourceCsvN` utiles au
/// bundle ADR 0204 (l'inventaire complet vit dans l'audit
/// `docs/working-notes/data-audit/arianeweb.md`).
#[derive(Debug, Clone, Deserialize)]
pub struct ArianeHit {
    /// `/Ariane_Web/<FOND>/|<NNNNNN>` — id interne ArianeWeb du document.
    #[serde(rename = "Id")]
    pub id: String,
    /// N° de dossier CE public — **multi-valué `;`** sur les affaires jointes
    /// (`412849;412895`), le premier compose le lien public (vérifié live).
    #[serde(rename = "SourceCsv1", default)]
    pub dossier: Option<String>,
    /// Date de lecture de la décision parente (`2022-06-20 02:00:00`).
    #[serde(rename = "SourceDateTime1", default)]
    pub date_lecture: Option<String>,
    /// Lien vers la décision parente (`/Ariane_Web/AW_DCE/|240377`).
    #[serde(rename = "SourceCsv5", default)]
    pub parent: Option<String>,
    /// Tous les documents frères (`DCE:240377;CRP:7628`).
    #[serde(rename = "SourceCsv2", default)]
    pub siblings: Option<String>,
    /// Lien direct vers les conclusions sœurs (`/Ariane_Web/AW_CRP/|7628`).
    #[serde(rename = "SourceCsv7", default)]
    pub crp: Option<String>,
    /// Codes du plan de classement (PCJA/Lebon), multi-valués `;`.
    #[serde(rename = "SourceCsv3", default)]
    pub codes_pcja: Option<String>,
    /// Niveau de publication (A/B/C).
    #[serde(rename = "SourceStr8", default)]
    pub niveau: Option<String>,
    /// ECLI de la décision parente — segment formation variable selon l'époque,
    /// jamais une clé de jointure (ADR 0095).
    #[serde(rename = "SourceStr30", default)]
    pub ecli_parent: Option<String>,
}

impl ArianeHit {
    /// Numéro interne du document (`/Ariane_Web/AW_AJCE/|172708` → `172708`).
    pub fn num(&self) -> Result<u32> {
        id_num(&self.id)
            .ok_or_else(|| SourceError::Invalid(format!("id ArianeWeb illisible: {}", self.id)))
    }

    /// Numéro `AW_DCE` de la décision parente — pivot du bundle. `SourceCsv5`
    /// d'abord, repli sur le graphe de fratrie `SourceCsv2` (`DCE:240377;…`).
    pub fn parent_dce_num(&self) -> Option<u32> {
        if let Some(n) = self.parent.as_deref().and_then(id_num) {
            return Some(n);
        }
        sibling_num(self.siblings.as_deref()?, "DCE")
    }

    /// Numéro `AW_CRP` des conclusions sœurs, si le graphe en confirme une
    /// (`SourceCsv7`, repli `SourceCsv2`) — condition d'émission de l'entrée
    /// `kind: "conclusions"` (ADR 0204 : jamais de lien aveugle).
    pub fn crp_num(&self) -> Option<u32> {
        if let Some(n) = self.crp.as_deref().and_then(id_num) {
            return Some(n);
        }
        sibling_num(self.siblings.as_deref()?, "CRP")
    }

    /// Date de lecture ISO (`2022-06-20`), partie date de `SourceDateTime1`.
    pub fn date_iso(&self) -> Option<&str> {
        let d = self.date_lecture.as_deref()?.get(..10)?;
        (d.len() == 10).then_some(d)
    }

    /// N°s de dossier éclatés (`412849;412895` → vec), ordre conservé — le
    /// premier est le numéro principal (lien public), les suivants servent
    /// au rattachement des affaires jointes. `SourceCsv1` manque sur une
    /// poignée de hits CRP : repli sur le n° embarqué dans l'ECLI parent
    /// (`ECLI:FR:CECHR:2022:457616.20220720` → `457616`).
    pub fn dossiers(&self) -> Vec<String> {
        let out: Vec<String> = self
            .dossier
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !out.is_empty() {
            return out;
        }
        self.ecli_parent
            .as_deref()
            .and_then(|e| e.rsplit(':').next()?.split('.').next())
            .filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
            .map(|d| vec![d.to_string()])
            .unwrap_or_default()
    }

    /// Codes PCJA éclatés (`54-06-05-11;60-04` → vec), ordre conservé.
    pub fn pcja(&self) -> Vec<String> {
        self.codes_pcja
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// `/Ariane_Web/<FOND>/|<NNNNNN>` → `NNNNNN`.
fn id_num(id: &str) -> Option<u32> {
    id.rsplit('|').next()?.trim().parse().ok()
}

/// `DCE:240377;CRP:7628` → numéro du membre `kind`.
fn sibling_num(siblings: &str, kind: &str) -> Option<u32> {
    siblings.split(';').find_map(|part| {
        let (k, v) = part.split_once(':')?;
        (k.trim() == kind).then(|| v.trim().parse().ok())?
    })
}

/// Client HTTP du fond (blocking, comme les autres downloaders de stocks).
pub fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(SourceError::from)
}

/// Une page d'énumération d'un fond pour une année (`PageNumber` 1-based).
/// Une année sans document renvoie `total_count = 0`, `documents` vide.
pub fn search_page(
    client: &reqwest::blocking::Client,
    fond: ArianeFond,
    year: u16,
    page: u32,
) -> Result<ArianeSearchPage> {
    let url = format!(
        "{BASE_URL}/xsearch?type=json&SourceStr4={}&SourceInt1={year}&PageNumber={page}",
        fond.as_str()
    );
    let (status, _, body) = get_with_body_retrying(&url, || client.get(&url).send())?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "xsearch {} {year} p{page}: statut {status}",
            fond.as_str()
        )));
    }
    let body = body.expect("statut 200 → corps lu");
    serde_json::from_slice(&body)
        .map_err(|e| SourceError::Invalid(format!("xsearch {} {year} p{page}: {e}", fond.as_str())))
}

/// HTML décodé d'une analyse AJCE, via cache disque (`<dir>/<millier>/<num>.html`,
/// shardé — ~108 k fichiers). Renvoie `(html, fetched)` où `fetched` dit si le
/// réseau a été touché (l'appelant throttle dans ce cas seulement).
pub fn ajce_html_cached(
    client: &reqwest::blocking::Client,
    cache_dir: &Path,
    num: u32,
) -> Result<(String, bool)> {
    let path = ajce_cache_path(cache_dir, num);
    if path.exists() {
        return Ok((decode_ariane_html(&fs::read(&path)?), false));
    }
    let url = format!(
        "{BASE_URL}/plugin?plugin=Service.downloadFilePagePlugin&Index=Ariane_Web&Id=/Ariane_Web/AW_AJCE/|{num}"
    );
    let (status, _, body) = get_with_body_retrying(&url, || client.get(&url).send())?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "download AJCE {num}: statut {status}"
        )));
    }
    let bytes = body.expect("statut 200 → corps lu");
    if !bytes.windows(5).any(|w| w.eq_ignore_ascii_case(b"<html")) {
        return Err(SourceError::Invalid(format!(
            "download AJCE {num}: corps non-HTML ({} octets)",
            bytes.len()
        )));
    }
    fs::create_dir_all(path.parent().expect("chemin shardé a un parent"))?;
    fs::write(&path, &bytes)?;
    Ok((decode_ariane_html(&bytes), true))
}

fn ajce_cache_path(cache_dir: &Path, num: u32) -> PathBuf {
    cache_dir.join(format!("ajce/{}/{num}.html", num / 1000))
}

/// HTML plein d'une décision `AW_DCE` (cache disque shardé, comme
/// [`ajce_html_cached`]). Backfill borné ADR 0219 — pas un canal de flux.
pub fn dce_html_cached(
    client: &reqwest::blocking::Client,
    cache_dir: &Path,
    num: u32,
) -> Result<(String, bool)> {
    let path = cache_dir.join(format!("dce/{}/{num}.html", num / 1000));
    if path.exists() {
        return Ok((decode_ariane_html(&fs::read(&path)?), false));
    }
    let url = format!(
        "{BASE_URL}/plugin?plugin=Service.downloadFilePagePlugin&Index=Ariane_Web&Id=/Ariane_Web/AW_DCE/|{num}"
    );
    let (status, _, body) = get_with_body_retrying(&url, || client.get(&url).send())?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "download DCE {num}: statut {status}"
        )));
    }
    let bytes = body.expect("statut 200 → corps lu");
    if !bytes.windows(5).any(|w| w.eq_ignore_ascii_case(b"<html")) {
        return Err(SourceError::Invalid(format!(
            "download DCE {num}: corps non-HTML ({} octets)",
            bytes.len()
        )));
    }
    fs::create_dir_all(path.parent().expect("chemin shardé a un parent"))?;
    fs::write(&path, &bytes)?;
    Ok((decode_ariane_html(&bytes), true))
}

/// Décodage latin-1 + réparation du `0x19` (apostrophe typographique
/// Windows-1252 mal mappée par Sinequa sur les documents récents).
pub fn decode_ariane_html(raw: &[u8]) -> String {
    raw.iter()
        .map(|&b| match b {
            0x19 => '\u{2019}',
            b => b as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_pivots() {
        let hit: ArianeHit = serde_json::from_str(
            r#"{
                "Id": "/Ariane_Web/AW_AJCE/|172708",
                "SourceCsv1": "438885",
                "SourceDateTime1": "2022-06-20 02:00:00",
                "SourceCsv5": "/Ariane_Web/AW_DCE/|240377",
                "SourceCsv2": "DCE:240377;CRP:7628",
                "SourceCsv3": "54-06-05-11;60-04-02-01",
                "SourceStr8": "B"
            }"#,
        )
        .unwrap();
        assert_eq!(hit.num().unwrap(), 172708);
        assert_eq!(hit.parent_dce_num(), Some(240377));
        // CRP confirmée par le graphe de fratrie même sans `SourceCsv7`.
        assert_eq!(hit.crp_num(), Some(7628));
        assert_eq!(hit.date_iso(), Some("2022-06-20"));
        assert_eq!(hit.pcja(), vec!["54-06-05-11", "60-04-02-01"]);
    }

    #[test]
    fn decode_latin1_et_0x19() {
        let raw = b"Conseil d\x19\xc9tat"; // 0x19 + É latin-1
        assert_eq!(decode_ariane_html(raw), "Conseil d\u{2019}État");
    }
}
