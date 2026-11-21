//! Ministerial bulletins, the sites art. D. 312-11 CRPA names: one connector per
//! site lists its notices, each body is fetched from the notice's own link.

pub mod acpr;
pub mod agri;
pub mod armees;
pub mod culture;
pub mod dgafp;
pub mod douane;
pub mod economie;
pub mod education;
pub mod esr;
pub mod interieur;
mod issue;
pub mod justice;
pub mod meae;
pub mod mte;
pub mod social;
pub mod sports;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{Datelike, NaiveDate};
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
    "www.enseignementsup-recherche.gouv.fr",
    "boss.gouv.fr",
];
const GATEWAY_THROTTLE: Duration = Duration::from_secs(5);
/// Hosts read one request every two seconds.
const SLOW_HOSTS: &[&str] = &["www.douane.gouv.fr"];
const SLOW_THROTTLE: Duration = Duration::from_secs(2);
/// A gated host's files, which its challenge leaves open, are read through the
/// egress gateway: the page gateway stalls on a body over 100 MB.
const GATED_FILES: &str = "/sites/default/files/";

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
    /// Europe and foreign affairs.
    Meae,
    /// National education, youth and sports.
    Education,
    Interieur,
    /// Economy and finance, the central administration's bulletin.
    Economie,
    /// Higher education and research.
    Esr,
    /// Sports, youth and community life, until 2020.
    Sports,
    /// The prudential supervision and resolution authority's official register.
    Acpr,
    /// Customs, the administrative decisions of its bulletin.
    Douane,
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
        BulletinSite::Meae,
        BulletinSite::Education,
        BulletinSite::Interieur,
        BulletinSite::Economie,
        BulletinSite::Esr,
        BulletinSite::Sports,
        BulletinSite::Acpr,
        BulletinSite::Douane,
    ];

    /// The first year `--full` lists: 1970 for the customs listing, 1998, the
    /// health bulletin's first year, for the others.
    pub fn first_year(self) -> i32 {
        match self {
            BulletinSite::Douane => 1970,
            _ => 1998,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            BulletinSite::Mte => "mte",
            BulletinSite::Justice => "justice",
            BulletinSite::Armees => "armees",
            BulletinSite::Agri => "agri",
            BulletinSite::Social => "social",
            BulletinSite::Dgafp => "dgafp",
            BulletinSite::Culture => "culture",
            BulletinSite::Meae => "meae",
            BulletinSite::Education => "education",
            BulletinSite::Interieur => "interieur",
            BulletinSite::Economie => "economie",
            BulletinSite::Esr => "esr",
            BulletinSite::Sports => "sports",
            BulletinSite::Acpr => "acpr",
            BulletinSite::Douane => "douane",
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
    files: Option<ClientWithMiddleware>,
    page_gateway: Option<String>,
    cache: PathBuf,
}

impl BulletinClient {
    pub fn new(
        page_gateway: Option<String>,
        egress_proxy: Option<String>,
        cache: PathBuf,
    ) -> Result<Self> {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) librejustice-bulletins/0.1")
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                // Under the 5 s a Node server keeps an idle connection, which the
                // gateway's 5 s throttle would otherwise race.
                .pool_idle_timeout(Duration::from_secs(2))
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        let files = match egress_proxy {
            Some(proxy) => Some(
                ClientBuilder::new(
                    reqwest::Client::builder()
                        .user_agent("LibreJusticeBot/0.1 (+https://librejustice.fr)")
                        .proxy(reqwest::Proxy::all(proxy)?)
                        .connect_timeout(Duration::from_secs(15))
                        .timeout(Duration::from_secs(600))
                        .build()?,
                )
                .with(TracingMiddleware::default())
                .build(),
            ),
            None => None,
        };
        Ok(Self {
            client,
            files,
            page_gateway,
            cache,
        })
    }

    /// Every notice the site published from `since` on, the sites listed by year
    /// from the first day of its year.
    pub async fn list(&self, site: BulletinSite, since: NaiveDate) -> Result<Vec<BulletinNotice>> {
        let since_year = since.year();
        match site {
            BulletinSite::Mte => mte::list(self, since_year).await,
            BulletinSite::Justice => justice::list(self, since_year).await,
            BulletinSite::Armees => armees::list(self, since_year).await,
            BulletinSite::Agri => agri::list(self, since_year).await,
            BulletinSite::Social => social::list(self, since).await,
            BulletinSite::Dgafp => dgafp::list(self, since_year).await,
            BulletinSite::Culture => culture::list(self, since_year).await,
            BulletinSite::Meae => meae::list(self, since).await,
            BulletinSite::Education => education::list(self, since_year).await,
            BulletinSite::Interieur => interieur::list(self, since_year).await,
            BulletinSite::Economie => economie::list(self, since).await,
            BulletinSite::Esr => esr::list(self, since).await,
            BulletinSite::Sports => sports::list(self, since_year).await,
            BulletinSite::Acpr => acpr::list(self, since).await,
            BulletinSite::Douane => douane::list(self, since).await,
        }
    }

    /// What only the notice's own page gives, for the sites whose listing lacks it.
    pub async fn enrich(&self, notice: &mut BulletinNotice) -> Result<()> {
        match notice.site {
            BulletinSite::Agri => agri::enrich(self, notice).await,
            BulletinSite::Social => social::enrich(self, notice).await,
            BulletinSite::Education => education::enrich(self, notice).await,
            BulletinSite::Acpr => acpr::enrich(self, notice).await,
            BulletinSite::Douane => douane::enrich(self, notice).await,
            BulletinSite::Mte
            | BulletinSite::Justice
            | BulletinSite::Armees
            | BulletinSite::Dgafp
            | BulletinSite::Culture
            | BulletinSite::Meae
            | BulletinSite::Interieur
            | BulletinSite::Economie
            | BulletinSite::Esr
            | BulletinSite::Sports => Ok(()),
        }
    }

    fn gated(url: &str) -> bool {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|h| GATEWAY_HOSTS.contains(&h)))
            .unwrap_or(false)
    }

    fn gated_file(url: &str) -> bool {
        Self::gated(url)
            && reqwest::Url::parse(url).is_ok_and(|u| u.path().starts_with(GATED_FILES))
    }

    /// A gated host's page is asked of the gateway, which answers the origin's
    /// status. A request that fails to send or a 5xx is retried, four attempts
    /// at most.
    async fn send(&self, url: &str) -> Result<reqwest::Response> {
        let (client, target, throttle) = if Self::gated_file(url) {
            let Some(files) = &self.files else {
                return Err(SourceError::Invalid(format!(
                    "LIBREJUSTICE_EGRESS_PROXY is empty: {url} is read through the egress gateway"
                )));
            };
            let target = reqwest::Url::parse(url)
                .map_err(|e| SourceError::Invalid(format!("bulletin {url}: {e}")))?;
            (files, target, GATEWAY_THROTTLE)
        } else if Self::gated(url) {
            let Some(gateway) = &self.page_gateway else {
                return Err(SourceError::Invalid(format!(
                    "LIBREJUSTICE_PAGE_GATEWAY is empty: {url} is read through the page gateway"
                )));
            };
            let target =
                reqwest::Url::parse_with_params(&format!("{gateway}/fetch"), [("url", url)])
                    .map_err(|e| SourceError::Invalid(format!("page gateway {gateway}: {e}")))?;
            (&self.client, target, GATEWAY_THROTTLE)
        } else {
            let target = reqwest::Url::parse(url)
                .map_err(|e| SourceError::Invalid(format!("bulletin {url}: {e}")))?;
            let slow = target.host_str().is_some_and(|h| SLOW_HOSTS.contains(&h));
            (
                &self.client,
                target,
                if slow { SLOW_THROTTLE } else { THROTTLE },
            )
        };
        let mut attempt = 1;
        loop {
            tokio::time::sleep(throttle).await;
            match client.get(target.clone()).send().await {
                Err(e) if attempt < 4 => {
                    tracing::warn!(url, attempt, error = %e, "bulletin: request failed, retried");
                }
                Err(e) => return Err(SourceError::Invalid(format!("bulletin {url}: {e}"))),
                Ok(resp) if resp.status().is_server_error() && attempt < 4 => {
                    tracing::warn!(
                        url,
                        status = resp.status().as_u16(),
                        attempt,
                        "bulletin: origin error, retried"
                    );
                }
                Ok(resp) => return Ok(resp),
            }
            tokio::time::sleep(Duration::from_secs(20 * attempt)).await;
            attempt += 1;
        }
    }

    pub(crate) async fn get_page(&self, url: &str) -> Result<(u16, String)> {
        let resp = self.send(url).await?;
        let status = resp.status().as_u16();
        let text = if Self::gated(url) {
            String::from_utf8_lossy(&resp.bytes().await?).into_owned()
        } else {
            resp.text().await?
        };
        Ok((status, text))
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

    /// An issue does not change once published: one read once is kept beside
    /// its site's state, so an interrupted run resumes where it stopped.
    pub(crate) async fn frozen_issue(
        &self,
        site: BulletinSite,
        url: &str,
    ) -> Result<Option<Vec<u8>>> {
        let dir = site_dir(&self.cache, site).join("issues");
        let path = dir.join(url.trim_start_matches("https://").replace('/', "_"));
        match tokio::fs::read(&path).await {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let Some(bytes) = self.get_bytes(url).await? else {
            return Ok(None);
        };
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&path, &bytes).await?;
        Ok(Some(bytes))
    }

    /// `None` on 404, a notice whose document the site lost. A body cut short
    /// is asked again, four attempts at most.
    pub async fn get_bytes(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let mut attempt = 1;
        loop {
            let resp = self.send(url).await?;
            match resp.status().as_u16() {
                200 => match resp.bytes().await {
                    Ok(bytes) => return Ok(Some(bytes.to_vec())),
                    Err(e) if attempt < 4 => {
                        tracing::warn!(url, attempt, error = %e, "bulletin: body cut short, retried");
                    }
                    Err(e) => return Err(e.into()),
                },
                404 => return Ok(None),
                status => {
                    return Err(SourceError::Invalid(format!(
                        "bulletin status {status} {url}"
                    )))
                }
            }
            tokio::time::sleep(Duration::from_secs(20 * attempt)).await;
            attempt += 1;
        }
    }
}

pub fn site_dir(cache: &Path, site: BulletinSite) -> PathBuf {
    cache.join("bulletins").join(site.slug())
}

/// Native ids of the notices whose publication is written.
pub fn load_published(dir: &Path) -> Result<BTreeSet<String>> {
    load_ids(dir, "published")
}

pub fn save_published(dir: &Path, published: &BTreeSet<String>) -> Result<()> {
    save_ids(dir, "published", published)
}

/// Native ids of the written notices whose file was asked for the bucket.
pub fn load_filed(dir: &Path) -> Result<BTreeSet<String>> {
    load_ids(dir, "filed")
}

pub fn save_filed(dir: &Path, filed: &BTreeSet<String>) -> Result<()> {
    save_ids(dir, "filed", filed)
}

fn load_ids(dir: &Path, name: &str) -> Result<BTreeSet<String>> {
    let path = dir.join(format!("{name}.json"));
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))
}

fn save_ids(dir: &Path, name: &str, ids: &BTreeSet<String>) -> Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{name}.json.tmp"));
    fs::write(
        &tmp,
        serde_json::to_vec(ids).map_err(|e| SourceError::Invalid(e.to_string()))?,
    )?;
    fs::rename(tmp, dir.join(format!("{name}.json")))?;
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

/// A listing title cut short by « … », completed from the document, which
/// prints it whole above its NOR.
pub fn complete_title(title: &str, body: &str) -> Option<String> {
    let stem = title.strip_suffix('…')?.trim_end();
    let head = body
        .chars()
        .take(4000)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let rest = &head[head.find(stem)?..];
    let end = rest.find(" NOR")?;
    Some(rest[..end].trim_end_matches([' ', '.']).to_string())
}

/// « 27 décembre 2019 ».
pub(crate) fn french_long(d: NaiveDate) -> String {
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
