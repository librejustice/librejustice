//! Ministerial bulletins, the sites art. D. 312-11 CRPA names: one connector per
//! site lists its notices, each body is fetched from the notice's own link.

pub mod agri;
pub mod armees;
pub mod culture;
pub mod dgafp;
pub mod economie;
pub mod education;
pub mod interieur;
mod issue;
pub mod justice;
pub mod mte;
pub mod social;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::NaiveDate;
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;

use crate::error::{Result, SourceError};

/// The sites are sized for browsing.
const THROTTLE: Duration = Duration::from_millis(700);

/// Hosts read through the page gateway, at its own pace.
const GATEWAY_HOSTS: &[&str] = &[
    "www.education.gouv.fr",
    "www.interieur.gouv.fr",
    "www.economie.gouv.fr",
    "sante.gouv.fr",
    "travail-emploi.gouv.fr",
];
const GATEWAY_THROTTLE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BulletinSite {
    /// Ecological transition, territorial cohesion, energy and sea.
    Mte,
    Justice,
    Armees,
    Agri,
    /// Health and labour, one site since July 2025, each ministry's own before.
    Social,
    /// The civil service directorate, which publishes no bulletin.
    Dgafp,
    Culture,
    /// National education, youth and sports.
    Education,
    Interieur,
    /// Economy and finance, the central administration's bulletin.
    Economie,
}

impl BulletinSite {
    pub const ALL: &'static [BulletinSite] = &[
        BulletinSite::Mte,
        BulletinSite::Justice,
        BulletinSite::Armees,
        BulletinSite::Agri,
        BulletinSite::Social,
        BulletinSite::Dgafp,
        BulletinSite::Culture,
        BulletinSite::Education,
        BulletinSite::Interieur,
        BulletinSite::Economie,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            BulletinSite::Mte => "mte",
            BulletinSite::Justice => "justice",
            BulletinSite::Armees => "armees",
            BulletinSite::Agri => "agri",
            BulletinSite::Social => "social",
            BulletinSite::Dgafp => "dgafp",
            BulletinSite::Culture => "culture",
            BulletinSite::Education => "education",
            BulletinSite::Interieur => "interieur",
            BulletinSite::Economie => "economie",
        }
    }

    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|site| site.slug() == s)
            .ok_or_else(|| {
                let known: Vec<&str> = Self::ALL.iter().map(|s| s.slug()).collect();
                format!("{s}: one of {}", known.join(", "))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulletinNotice {
    pub site: BulletinSite,
    pub native_id: String,
    pub nor: Option<String>,
    /// The site's type label (« Circulaire », « Note »…).
    pub kind: String,
    pub title: String,
    pub signed: Option<NaiveDate>,
    pub published: Option<NaiveDate>,
    pub notice_url: String,
    pub body_url: Option<String>,
    /// The text itself, cut from an issue that gathers several.
    pub body: Option<String>,
    /// The DILA circular the notice links to instead of a document of its own.
    pub legifrance_uid: Option<String>,
    /// A circular, an instruction, a note or a directive, not an individual
    /// measure, an arrêté or a convention.
    pub doctrine: bool,
    /// The site states the text abrogated or lapsed.
    pub abrogated: bool,
}

impl BulletinNotice {
    pub fn text_uid(&self) -> String {
        format!("bo_{}_{}", self.site.slug(), self.native_id)
    }

    /// A doctrine text is a circular; any other nature is read off the type label,
    /// or the title when the site gives none.
    pub fn nature(&self) -> Option<&'static str> {
        const NATURES: [(&str, &str); 11] = [
            ("arrete", "ARRETE"),
            ("decision", "DECISION"),
            ("convention", "CONVENTION"),
            ("avis", "AVIS"),
            ("decret", "DECRET"),
            ("deliberation", "DELIBERATION"),
            ("liste", "LISTE"),
            ("charte", "CHARTE"),
            ("protocole", "PROTOCOLE"),
            ("avenant", "AVENANT"),
            ("recommandation", "RECOMMANDATION"),
        ];
        if self.doctrine {
            return Some("CIRCULAIRE");
        }
        let label = if self.kind.is_empty() {
            &self.title
        } else {
            &self.kind
        };
        let word: String = label
            .split_whitespace()
            .next()?
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphabetic())
            .map(|c| match c {
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'à' | 'â' => 'a',
                'î' | 'ï' => 'i',
                'ô' => 'o',
                'ù' | 'û' => 'u',
                c => c,
            })
            .collect();
        NATURES
            .iter()
            .find(|(stem, _)| word == *stem || word.strip_suffix('s') == Some(stem))
            .map(|(_, nature)| *nature)
    }
}

pub struct BulletinClient {
    client: ClientWithMiddleware,
    page_gateway: Option<String>,
    cache: PathBuf,
}

impl BulletinClient {
    pub fn new(page_gateway: Option<String>, cache: PathBuf) -> Self {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) librejustice-bulletins/0.1")
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self {
            client,
            page_gateway,
            cache,
        }
    }

    /// Every notice the site published from `since_year` on.
    pub async fn list(&self, site: BulletinSite, since_year: i32) -> Result<Vec<BulletinNotice>> {
        match site {
            BulletinSite::Mte => mte::list(self, since_year).await,
            BulletinSite::Justice => justice::list(self, since_year).await,
            BulletinSite::Armees => armees::list(self, since_year).await,
            BulletinSite::Agri => agri::list(self, since_year).await,
            BulletinSite::Social => social::list(self, since_year).await,
            BulletinSite::Dgafp => dgafp::list(self, since_year).await,
            BulletinSite::Culture => culture::list(self, since_year).await,
            BulletinSite::Education => education::list(self, since_year).await,
            BulletinSite::Interieur => interieur::list(self, since_year).await,
            BulletinSite::Economie => economie::list(self, since_year).await,
        }
    }

    /// What only the notice's own page gives, for the sites whose listing lacks it.
    pub async fn enrich(&self, notice: &mut BulletinNotice) -> Result<()> {
        match notice.site {
            BulletinSite::Agri => agri::enrich(self, notice).await,
            BulletinSite::Social => social::enrich(self, notice).await,
            BulletinSite::Education => education::enrich(self, notice).await,
            BulletinSite::Mte
            | BulletinSite::Justice
            | BulletinSite::Armees
            | BulletinSite::Dgafp
            | BulletinSite::Culture
            | BulletinSite::Interieur
            | BulletinSite::Economie => Ok(()),
        }
    }

    fn gated(url: &str) -> bool {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| GATEWAY_HOSTS.contains(&h)))
            .unwrap_or(false)
    }

    /// The gateway answers the origin's status; a timeout or a 5xx is retried, four
    /// attempts at most.
    async fn through_gateway(&self, url: &str) -> Result<(u16, Vec<u8>)> {
        let Some(gateway) = &self.page_gateway else {
            return Err(SourceError::Invalid(format!(
                "LIBREJUSTICE_PAGE_GATEWAY is empty: {url} is read through the page gateway"
            )));
        };
        let target = reqwest::Url::parse_with_params(&format!("{gateway}/fetch"), [("url", url)])
            .map_err(|e| SourceError::Invalid(format!("page gateway {gateway}: {e}")))?;
        let mut attempt = 1;
        loop {
            tokio::time::sleep(GATEWAY_THROTTLE).await;
            let resp = match self.client.get(target.clone()).send().await {
                Err(e) if e.is_timeout() && attempt < 4 => {
                    tracing::warn!(url, attempt, "bulletin: page gateway busy, retried");
                    attempt += 1;
                    continue;
                }
                resp => resp.map_err(|e| SourceError::Invalid(format!("bulletin {url}: {e}")))?,
            };
            let status = resp.status().as_u16();
            if status >= 500 && attempt < 4 {
                tracing::warn!(url, status, attempt, "bulletin: origin error, retried");
                tokio::time::sleep(Duration::from_secs(20 * attempt)).await;
                attempt += 1;
                continue;
            }
            return Ok((status, resp.bytes().await?.to_vec()));
        }
    }

    pub(crate) async fn get_page(&self, url: &str) -> Result<(u16, String)> {
        if Self::gated(url) {
            let (status, bytes) = self.through_gateway(url).await?;
            return Ok((status, String::from_utf8_lossy(&bytes).into_owned()));
        }
        tokio::time::sleep(THROTTLE).await;
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("bulletin {url}: {e}")))?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().await?))
    }

    pub(crate) async fn get_text(&self, url: &str) -> Result<String> {
        let (status, text) = self.get_page(url).await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "bulletin status {status} {url}"
            )));
        }
        Ok(text)
    }

    /// `None` on 404, a notice whose document the site lost.
    pub async fn get_bytes(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let (status, bytes) = if Self::gated(url) {
            self.through_gateway(url).await?
        } else {
            tokio::time::sleep(THROTTLE).await;
            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| SourceError::Invalid(format!("bulletin {url}: {e}")))?;
            (resp.status().as_u16(), resp.bytes().await?.to_vec())
        };
        match status {
            200 => Ok(Some(bytes)),
            404 => Ok(None),
            status => Err(SourceError::Invalid(format!(
                "bulletin status {status} {url}"
            ))),
        }
    }
}

pub fn site_dir(cache: &Path, site: BulletinSite) -> PathBuf {
    cache.join("bulletins").join(site.slug())
}

/// Native ids of the notices whose publication is written.
pub fn load_published(dir: &Path) -> Result<BTreeSet<String>> {
    let path = dir.join("published.json");
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))
}

pub fn save_published(dir: &Path, published: &BTreeSet<String>) -> Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join("published.json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(published).map_err(|e| SourceError::Invalid(e.to_string()))?,
    )?;
    fs::rename(tmp, dir.join("published.json"))?;
    Ok(())
}

/// The day the OCR last missed each scanned notice, by native id.
pub fn load_ocr_missed(dir: &Path) -> Result<BTreeMap<String, NaiveDate>> {
    let path = dir.join("ocr_missed.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))
}

pub fn save_ocr_missed(dir: &Path, missed: &BTreeMap<String, NaiveDate>) -> Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join("ocr_missed.json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(missed).map_err(|e| SourceError::Invalid(e.to_string()))?,
    )?;
    fs::rename(tmp, dir.join("ocr_missed.json"))?;
    Ok(())
}

pub fn load_cached_ocr(dir: &Path, native_id: &str) -> Result<Option<String>> {
    let path = dir.join("ocr").join(format!("{native_id}.md"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path)?))
}

pub fn save_cached_ocr(dir: &Path, native_id: &str, markdown: &str) -> Result<()> {
    let ocr = dir.join("ocr");
    fs::create_dir_all(&ocr)?;
    fs::write(ocr.join(format!("{native_id}.md")), markdown)?;
    Ok(())
}

/// A circular, an instruction, a note, a directive or guidelines, by the type
/// a title or a label opens with.
pub(crate) fn is_doctrine(label: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?i:circulaire|instruction|note|directive|lignes directrices)s?\b")
            .expect("valid regex")
    })
    .is_match(label)
}

/// `dd-mm-yyyy` or `dd/mm/yyyy`.
pub(crate) fn french_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s, "%d-%m-%Y")
        .or_else(|_| NaiveDate::parse_from_str(s, "%d/%m/%Y"))
        .ok()
}

/// « 1er octobre 2020 », « 26 octobre 2026 ».
pub(crate) fn french_long_date(s: &str) -> Option<NaiveDate> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)\b(\d{1,2})(?:er)?\s+(janvier|février|fevrier|mars|avril|mai|juin|juillet|août|aout|septembre|octobre|novembre|décembre|decembre)\s+(\d{4})")
            .expect("valid regex")
    });
    let c = re.captures(s)?;
    let month = match c[2].to_lowercase().as_str() {
        "janvier" => 1,
        "février" | "fevrier" => 2,
        "mars" => 3,
        "avril" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" => 7,
        "août" | "aout" => 8,
        "septembre" => 9,
        "octobre" => 10,
        "novembre" => 11,
        _ => 12,
    };
    NaiveDate::from_ymd_opt(c[3].parse().ok()?, month, c[1].parse().ok()?)
}

/// A well-formed NOR, the sites' field carrying stray values.
pub(crate) fn nor(s: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[A-Z]{4}\d{7}[A-Z]$").expect("valid regex"));
    let s = s.trim().replace(' ', "").to_uppercase();
    re.is_match(&s).then_some(s)
}

/// The NOR a document's head states, letter-spaced or not.
pub fn nor_in_text(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r"NOR\s*:?\s*((?:[A-Z0-9] ?){12})").expect("valid regex"));
    let head: String = text.chars().take(4000).collect();
    re.captures_iter(&head).find_map(|c| nor(&c[1]))
}

/// « 27 décembre 2019 ».
pub(crate) fn french_long(d: NaiveDate) -> String {
    use chrono::Datelike;
    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let day = match d.day() {
        1 => "1er".to_string(),
        n => n.to_string(),
    };
    format!("{day} {} {}", MONTHS[d.month0() as usize], d.year())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nor_is_read_letter_spaced_or_not() {
        assert_eq!(
            nor_in_text("Texte 8\nNOR A R M B 2 1 0 1 5 4 4 J\nRéférence(s)").as_deref(),
            Some("ARMB2101544J")
        );
        assert_eq!(
            nor_in_text("Circulaire du 4 septembre 2026\n\nNOR : TRAT2533039C\n\n(Texte")
                .as_deref(),
            Some("TRAT2533039C")
        );
        assert_eq!(nor_in_text("NOR : néant"), None);
    }

    #[test]
    fn a_nature_is_read_off_the_label_or_else_the_title() {
        let notice = |kind: &str, title: &str, doctrine: bool| BulletinNotice {
            site: BulletinSite::Interieur,
            native_id: "x".to_string(),
            nor: None,
            kind: kind.to_string(),
            title: title.to_string(),
            signed: None,
            published: None,
            notice_url: String::new(),
            body_url: None,
            body: None,
            legifrance_uid: None,
            doctrine,
            abrogated: false,
        };
        assert_eq!(
            notice("Note de service", "Note de service n° 2026-012", true).nature(),
            Some("CIRCULAIRE")
        );
        assert_eq!(
            notice("ARRÊTÉS", "Arrêtés de délégation", false).nature(),
            Some("ARRETE")
        );
        assert_eq!(
            notice("", "Décision du 1er avril 2019 portant délégation", false).nature(),
            Some("DECISION")
        );
        assert_eq!(
            notice("", "Avis de vacance d'emploi", false).nature(),
            Some("AVIS")
        );
        assert_eq!(notice("Rectificatif", "Rectificatif", false).nature(), None);
    }
}
