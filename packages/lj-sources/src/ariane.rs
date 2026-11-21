//! ArianeWeb (Conseil d'État, moteur Sinequa `engine2`) — commentaires
//! institutionnels des décisions CE (analyses AJCE, existence des conclusions
//! CRP) et fond des décisions elles-mêmes.
//!
//! Deux canaux, sans auth :
//! - `xsearch?type=json` filtered by `SourceStr4=<FOND>&SourceInt1=<année>` and
//!   paged by `PageNumber`, the server setting the page size (`PageSize` and
//!   `SkipCount` are ignored). A hit's year is the reading date of its parent
//!   decision: the analyses and conclusions of one decision fall in the same year.
//!   Without a year filter, the response carries the `anneeDateDeLecture` facet
//!   counted over the whole selection: the fond is counted in one request.
//! - `downloadFilePagePlugin` for the full HTML of an AJCE analysis (the JSON hit
//!   carries only the analytic titles, truncated; the full summaries are only in
//!   the HTML). The `Id` keeps its pipe un-URL-encoded.
//!
//! Encoding boundary: the HTML is decoded here, latin-1 or UTF-8 depending on
//! what the bytes carry, plus the `0x19` cleanup (mis-mapped Windows-1252
//! apostrophe). A CRP's PDF is read through the egress gateway by [`CrpClient`].

use std::collections::BTreeMap;
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

/// Fonds ArianeWeb énumérés.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArianeFond {
    /// Analyses of the CE (HTML).
    Ajce,
    /// Conclusions of the rapporteur public (PDF, never downloaded).
    Crp,
    /// The decisions themselves (HTML).
    Dce,
}

impl ArianeFond {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ajce => "AW_AJCE",
            Self::Crp => "AW_CRP",
            Self::Dce => "AW_DCE",
        }
    }
}

/// A page of `xsearch` results, its size set by the server.
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
    /// Facettes comptées par le serveur sur la sélection entière, pas sur la
    /// page servie.
    #[serde(rename = "Boxes", default)]
    pub boxes: Vec<ArianeBox>,
}

/// Une facette `xsearch` et ses nœuds comptés.
#[derive(Debug, Deserialize)]
pub struct ArianeBox {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Nodes", default)]
    pub nodes: Vec<ArianeNode>,
}

/// Nœud de facette. Sur `anneeDateDeLecture`, `level` 1 est la décennie et 2
/// l'année.
#[derive(Debug, Deserialize)]
pub struct ArianeNode {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Level")]
    pub level: u32,
    #[serde(rename = "Count")]
    pub count: u32,
}

/// Hit `xsearch` — sous-ensemble des champs `SourceStrN`/`SourceCsvN` utilisés.
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
    /// jamais une clé de jointure.
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
    /// `kind: "conclusions"`, jamais de lien aveugle.
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

/// Census of a fond, year by year, in **one** request: the server counts the
/// `anneeDateDeLecture` facet over the whole selection, not over the hits served.
/// Nodes out of range are faulty years of the fond (`211341`), dropped here.
pub fn census_by_year(
    client: &reqwest::blocking::Client,
    fond: ArianeFond,
) -> Result<BTreeMap<u16, u32>> {
    let url = format!(
        "{BASE_URL}/xsearch?type=json&SourceStr4={}&PageNumber=1",
        fond.as_str()
    );
    let (status, _, body) = get_with_body_retrying(&url, || client.get(&url).send())?;
    if status != 200 {
        return Err(SourceError::Invalid(format!(
            "recensement {}: statut {status}",
            fond.as_str()
        )));
    }
    let body = body.expect("statut 200 → corps lu");
    let page: ArianeSearchPage = serde_json::from_slice(&body)
        .map_err(|e| SourceError::Invalid(format!("recensement {}: {e}", fond.as_str())))?;
    let box_ = page
        .boxes
        .iter()
        .find(|b| b.name == "anneeDateDeLecture")
        .ok_or_else(|| {
            SourceError::Invalid(format!(
                "recensement {}: facette anneeDateDeLecture absente",
                fond.as_str()
            ))
        })?;
    Ok(box_
        .nodes
        .iter()
        .filter(|n| n.level == 2)
        .filter_map(|n| n.name.parse::<u16>().ok().map(|y| (y, n.count)))
        .filter(|(y, _)| (1780..=2100).contains(y))
        .collect())
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

/// Decoded HTML of an AJCE analysis, through a sharded disk cache
/// (`<dir>/<millier>/<num>.html`). Returns `(html, fetched)`, `fetched` saying
/// whether the network was touched (the caller throttles only then).
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
/// [`ajce_html_cached`]). Backfill borné, pas un canal de flux.
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

/// Sinequa serves its recent documents in UTF-8 and the older ones in latin-1,
/// under the same `Content-Type`, so the encoding is read off the bytes and not
/// off the header: accented French latin-1 is not valid UTF-8. The `0x19` is a
/// typographic apostrophe Sinequa mis-maps, in both encodings.
pub fn decode_ariane_html(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(text) => text.replace('\u{19}', "\u{2019}"),
        Err(_) => raw
            .iter()
            .map(|&b| match b {
                0x19 => '\u{2019}',
                b => b as char,
            })
            .collect(),
    }
}

/// The rapporteur public's conclusions as ArianeWeb serves them, a PDF per decision.
pub struct CrpClient {
    http: reqwest::Client,
}

impl CrpClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "ArianeWeb's conclusions are read through LIBREJUSTICE_EGRESS_PROXY, which is empty"
                    .into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { http })
    }

    /// The PDF of the conclusions read on `date` in case `dossier`, `None` when the site
    /// answers anything else.
    pub async fn pdf(&self, date: &str, dossier: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("{BASE_URL}/fr/arianeweb/CRP/conclusion/{date}/{dossier}?download_pdf");
        let resp = self.http.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        let bytes = resp.bytes().await?;
        Ok(bytes.starts_with(b"%PDF").then(|| bytes.to_vec()))
    }
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
        assert_eq!(hit.crp_num(), Some(7628));
        assert_eq!(hit.date_iso(), Some("2022-06-20"));
        assert_eq!(hit.pcja(), vec!["54-06-05-11", "60-04-02-01"]);
    }

    #[test]
    fn decode_latin1_et_0x19() {
        let raw = b"Conseil d\x19\xc9tat"; // 0x19 + É latin-1
        assert_eq!(decode_ariane_html(raw), "Conseil d\u{2019}État");
    }

    #[test]
    fn decode_utf8_et_0x19() {
        let raw = "Expulsion prononc\u{e9}e \u{e0} l\u{19}encontre".as_bytes();
        assert_eq!(
            decode_ariane_html(raw),
            "Expulsion prononcée à l\u{2019}encontre"
        );
    }
}
