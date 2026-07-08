//! Downloader CNDA (Cour nationale du droit d'asile) — I/O réseau au bord
//! lj-sources (ADR 0096).
//!
//! La CNDA est **absente** de Judilibre/JADE/opendata : seule source = scraping
//! du site `cnda.fr`. Deux niveaux fusionnés côté pipeline (une ligne `decisions`
//! par numéro, grounding §Décisions de conception 2) :
//! - **fiche HTML éditoriale** (liste paginée `?page=1..N`, 30 fiches/page,
//!   N≈17 ⇒ ~500 décisions « jurisprudentielles », sous-ensemble curé) : titre
//!   éditorial, `content_type`, abstract/analyse rédigé par la Cour (1300–2700
//!   car.), lien « Voir la décision » (slug PDF **sans extension** sous
//!   `/Media/mediatheque-cnda/images/<année>/`), date de mise en ligne.
//! - **PDF Word lié** (texte intégral) : octets bruts → OCR Mistral
//!   (`mistral-ocr-latest`, markdown par page) côté pipeline `lj-ingest`, mis en
//!   cache local (`ocr/<numero>.md`) car payant et non déterministe.
//!
//! Le parsing métier (sections, ECLI fabriqué, `source_fields`) vit dans
//! `lj-core::parse_cnda`, qui reçoit le markdown OCR nettoyé (`clean_ocr_markdown`)
//! et les métadonnées fiche désérialisées. Throttle de courtoisie + User-Agent de
//! contact. Le watermark (dernière page drainée / dernier numéro vu) est persisté
//! façon [`crate::dila::DilaManifest`].

use crate::error::{Result, SourceError};
use crate::html_strip::strip_html;
use chrono::Utc;
use regex::Regex;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// Base du site CNDA.
pub const BASE_URL: &str = "https://www.cnda.fr";
/// User-Agent de contact (scraping courtois d'un site public sans API).
pub const USER_AGENT: &str = "librejustice-cnda/0.1 (+https://github.com/)";
/// Throttle de courtoisie entre fetchs (site public, scraping respectueux).
pub const THROTTLE: Duration = Duration::from_millis(500);
/// Chemin de la liste jurisprudentielle paginée.
pub const LIST_PATH: &str = "/decisions-de-justice/jurisprudence/decisions-jurisprudentielles";

const CNDA_SOURCE_DIR: &str = "cnda";

// ----------------------------------------------------------------------------
// Client CNDA (async reqwest-middleware, comme cedh.rs / judilibre.rs)
// ----------------------------------------------------------------------------

/// Client CNDA (reqwest async + `TracingMiddleware`). Crawl HTML de la liste
/// paginée, fiches HTML, PDF liés.
pub struct CndaClient {
    base_url: String,
    client: ClientWithMiddleware,
}

impl CndaClient {
    /// Client de production (base [`BASE_URL`]).
    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    /// Client sur une base donnée (`base_url` normalisée, trailing `/` retiré).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .expect("reqwest client build"),
        )
        .with(TracingMiddleware::default())
        .build();
        Self { base_url, client }
    }

    /// HTML d'une page de la liste jurisprudentielle (`?page=<page>`). La page 1
    /// est la racine (`?page=1` est accepté côté serveur). `None` sur **404** :
    /// au-delà de la dernière page, le site CNDA renvoie un 404 (et non une liste
    /// vide) — c'est le signal de fin de pagination, pas une erreur. Erreur franche
    /// sur tout autre statut ≠ 200.
    pub async fn list_page(&self, page: u32) -> Result<Option<String>> {
        let url = format!("{}{}?page={page}", self.base_url, LIST_PATH);
        let resp = self
            .client
            .get(&url)
            .header("accept", "text/html")
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête cnda {url}: {e}")))?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        let text = resp.text().await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "cnda statut {status} {url}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(Some(text))
    }

    /// HTML de la rubrique « dernières décisions » (canal d'incrément).
    pub async fn latest_html(&self) -> Result<String> {
        // Refonte du site (vérifié 2026-06-15) : la rubrique est sous
        // `/decisions-de-justice/dernieres-decisions` (l'ancienne, sous
        // `/jurisprudence/`, renvoie 404).
        let url = format!("{}/decisions-de-justice/dernieres-decisions", self.base_url);
        self.get_html(&url).await
    }

    /// HTML d'une fiche éditoriale. `fiche_url` est une URL absolue (issue de
    /// [`enumerate_fiche_urls`]) ou un chemin relatif au site.
    pub async fn fiche_html(&self, fiche_url: &str) -> Result<String> {
        let url = self.absolutize(fiche_url);
        self.get_html(&url).await
    }

    /// Octets bruts du PDF lié. `pdf_url` est le slug « Voir la décision »
    /// (**sans extension `.pdf`**). Renvoie `None` si le PDF est introuvable
    /// (404 / lien mort) ⇒ la décision sera traitée fiche-only côté pipeline
    /// (grounding §Décisions 2). Erreur franche sur tout autre statut ≠ 200.
    pub async fn fetch_pdf(&self, pdf_url: &str) -> Result<Option<Vec<u8>>> {
        let url = self.absolutize(pdf_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête cnda pdf {url}: {e}")))?;
        let status = resp.status().as_u16();
        match status {
            200 => Ok(Some(resp.bytes().await?.to_vec())),
            404 => Ok(None),
            other => Err(SourceError::Invalid(format!(
                "cnda pdf statut {other} {url}"
            ))),
        }
    }

    async fn get_html(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .header("accept", "text/html")
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête cnda {url}: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if status != 200 {
            return Err(SourceError::Invalid(format!(
                "cnda statut {status} {url}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        Ok(text)
    }

    /// Rend une URL absolue : si `path` commence déjà par `http`, telle quelle ;
    /// sinon préfixe par `base_url` (en garantissant un seul `/` de jonction).
    fn absolutize(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}/{}", self.base_url, path.trim_start_matches('/'))
        }
    }
}

impl Default for CndaClient {
    fn default() -> Self {
        Self::new()
    }
}

// ----------------------------------------------------------------------------
// Énumération des fiches (crawl pagination)
// ----------------------------------------------------------------------------

/// Énumère les URL de fiches éditoriales présentes dans le HTML d'une page de
/// liste. Les fiches vivent sous
/// `/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/<slug>`
/// (≠ l'index nu de la sous-rubrique et ≠ `dernieres-decisions` ; cf.
/// [`is_fiche_path`]). Dédoublonne en préservant l'ordre. Chemins relatifs au site.
pub fn enumerate_fiche_urls(list_html: &str) -> Vec<String> {
    static HREF_RE: OnceLock<Regex> = OnceLock::new();
    let re = HREF_RE
        .get_or_init(|| Regex::new(r#"href\s*=\s*["']([^"']+)["']"#).expect("regex href valide"));

    let mut seen = Vec::new();
    for caps in re.captures_iter(list_html) {
        let href = &caps[1];
        // Normalise vers un chemin relatif au site : on garde tout à partir de
        // `/decisions-de-justice` (qu'`href` soit absolu ou déjà relatif).
        let Some(idx) = href.find("/decisions-de-justice") else {
            continue;
        };
        let path = &href[idx..];
        if !is_fiche_path(path) {
            continue;
        }
        if !seen.iter().any(|p| p == path) {
            seen.push(path.to_string());
        }
    }
    seen
}

/// Vrai si `path` désigne une fiche de décision (sous la rubrique jurisprudence)
/// et non une page de navigation (rubrique racine, `dernieres-decisions`,
/// pagination, ancres internes).
fn is_fiche_path(path: &str) -> bool {
    // Depuis la refonte du site (vérifié 2026-06-15), les fiches vivent sous la
    // sous-rubrique `decisions-jurisprudentielles/<slug>` (et non plus directement
    // sous `jurisprudence/<slug>`). Le préfixe inclut le `/` final : l'index nu
    // `…/decisions-jurisprudentielles` (et sa pagination `?page=`) ne le matche pas
    // → exclu comme page de navigation.
    let prefix = "/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/";
    let Some(slug) = path.strip_prefix(prefix) else {
        return false;
    };
    if slug.is_empty() {
        return false;
    }
    // Ancre interne (`#chapitre`) ou query résiduelle : pas une fiche propre.
    if slug.contains('#') || slug.contains('?') {
        return false;
    }
    // Un slug de fiche n'a pas de sous-segment (une seule composante).
    !slug.trim_end_matches('/').contains('/')
}

// ----------------------------------------------------------------------------
// Parse de la fiche HTML éditoriale
// ----------------------------------------------------------------------------

/// Métadonnées extraites d'une fiche HTML éditoriale CNDA. Sérialisée en `Value`
/// par [`CndaFiche::to_value`] pour être consommée par `lj-core::parse_cnda`
/// (qui reçoit ces champs + le texte PDF déjà recollé, jamais d'octets bruts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CndaFiche {
    /// URL (relative) de la fiche elle-même.
    pub fiche_url: String,
    /// Titre éditorial (h1) — descriptif (« La Cour reconnait… »), pas un
    /// intitulé d'arrêt.
    pub title: Option<String>,
    /// Type de contenu (1re ligne du body : `Jurisprudence` / `Décision de justice`).
    pub content_type: Option<String>,
    /// Abstract / analyse éditoriale (1300–2700 car., « En premier lieu… »).
    pub editorial_abstract: Option<String>,
    /// Slug PDF « Voir la décision » (sans extension `.pdf`), si présent.
    pub pdf_url: Option<String>,
    /// Numéro de dossier lu dans le **corps** de la fiche (`n°NNNNNNNN`), repli
    /// quand le slug (PDF ou fiche) ne le porte pas : les fiches modernes ont un
    /// slug descriptif (`la-cnda-evalue-…`) + un lien `/documents/…` sans numéro,
    /// mais citent `n°25013796` dans l'analyse. Numéro propre = le plus fréquent.
    pub numero: Option<String>,
    /// Date de lecture lue dans la **référence de décision** du corps
    /// (`CNDA 27 juillet 2016 M. A. n° 16012935 C`), distincte de la mise en ligne
    /// (`publication_date`). Repli pour les fiche-only sans PDF (donc sans
    /// marqueur OCR) ni date de slug. ≠ date de mise en ligne (ADR 0096 §43/85).
    pub lecture_date: Option<String>,
    /// Date de mise en ligne de la fiche (texte brut, ex. `1 juin 2026`).
    /// **≠ date de lecture** (piège audit §43/85) — informatif seulement.
    pub publication_date: Option<String>,
}

impl CndaFiche {
    /// Sérialise en `Value` (contrat consommé par `lj-core::parse_cnda`).
    pub fn to_value(&self) -> Value {
        json!({
            "fiche_url": self.fiche_url,
            "title": self.title,
            "content_type": self.content_type,
            "editorial_abstract": self.editorial_abstract,
            "pdf_url": self.pdf_url,
            "numero": self.numero,
            "lecture_date": self.lecture_date,
            "publication_date": self.publication_date,
        })
    }
}

/// Parse une fiche HTML éditoriale en [`CndaFiche`]. Frontière de validation
/// (#12) : on **exige** un lien PDF candidat OU un abstract éditorial — une fiche
/// sans ni l'un ni l'autre est une page vidée de sa substance (DOM cassé) et lève
/// une erreur franche plutôt que de produire une décision creuse. L'absence du
/// *seul* PDF (lien mort) reste tolérée (fiche-only) ; c'est l'absence des deux
/// qui signale un gabarit cassé.
pub fn parse_fiche(html: &str, fiche_url: &str) -> Result<CndaFiche> {
    let title = extract_h1(html).or_else(|| extract_meta(html, "og:title"));
    let pdf_url = extract_pdf_slug(html);

    // Texte plat du body pour content_type + abstract.
    let body_text = strip_html(html);
    let content_type = content_type_line(&body_text, title.as_deref());
    let editorial_abstract =
        extract_abstract(&body_text).or_else(|| extract_meta(html, "og:description"));
    let publication_date = extract_publication_date(&body_text);
    let numero = extract_numero_from_body(&body_text);
    let lecture_date = extract_lecture_date_from_body(&body_text);

    if pdf_url.is_none() && editorial_abstract.is_none() {
        return Err(SourceError::Invalid(format!(
            "fiche cnda sans lien PDF ni abstract (gabarit cassé ?): {fiche_url}"
        )));
    }

    Ok(CndaFiche {
        fiche_url: fiche_url.to_string(),
        title,
        content_type,
        editorial_abstract,
        pdf_url,
        numero,
        lecture_date,
        publication_date,
    })
}

/// Date de lecture lue dans la référence de décision du corps
/// (`CNDA 27 juillet 2016 M. A. n° 16012935 C` / `CRR 18 avril 2005 …`) : la date
/// FR qui suit immédiatement le sigle de la juridiction. Distincte de la date de
/// mise en ligne (`publication_date`, qui suit le `content_type`). Repli pour les
/// fiche-only sans PDF. `None` si la référence est absente.
fn extract_lecture_date_from_body(body_text: &str) -> Option<String> {
    static REF_RE: OnceLock<Regex> = OnceLock::new();
    let re = REF_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:CNDA|CRR)\s+((?:1er|\d{1,2})\s+(?:janvier|février|fevrier|mars|avril|mai|juin|juillet|ao\u{fb}t|aout|septembre|octobre|novembre|décembre|decembre)\s+\d{4})\b",
        )
        .expect("regex réf décision corps")
    });
    re.captures(body_text).map(|c| c[1].trim().to_string())
}

/// Numéro de dossier (`n°NNNNNNNN`, ≥ 6 chiffres) lu dans le corps de la fiche.
/// Le **plus fréquent** : la décision sujet recurre dans l'analyse tandis qu'une
/// décision citée n'apparaît qu'une fois (départage : 1re occurrence). Repli quand
/// le slug ne porte pas le numéro (fiches modernes au slug descriptif). `None` si
/// aucun.
fn extract_numero_from_body(body_text: &str) -> Option<String> {
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let re = NUM_RE.get_or_init(|| Regex::new(r"(?i)n[°o]\s*(\d{6,})").expect("regex n° body"));
    let mut counts: Vec<(String, usize, usize)> = Vec::new(); // (numero, count, first_pos)
    for m in re.captures_iter(body_text) {
        let num = m[1].to_string();
        let pos = m.get(1).map(|g| g.start()).unwrap_or(0);
        if let Some(e) = counts.iter_mut().find(|(n, _, _)| *n == num) {
            e.1 += 1;
        } else {
            counts.push((num, 1, pos));
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2))) // + fréquent, puis + tôt
        .map(|(n, _, _)| n)
}

fn extract_h1(html: &str) -> Option<String> {
    static H1_RE: OnceLock<Regex> = OnceLock::new();
    let re = H1_RE.get_or_init(|| Regex::new(r"(?is)<h1\b[^>]*>(.*?)</h1>").expect("regex h1"));
    re.captures(html).and_then(|c| {
        let t = strip_html(&c[1]);
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_string())
    })
}

/// Extrait un `<meta property|name="<key>" content="...">` (og:title, og:description).
fn extract_meta(html: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?is)<meta[^>]*(?:property|name)\s*=\s*["']{}["'][^>]*content\s*=\s*["']([^"']*)["']"#,
        regex::escape(key)
    ))
    .ok()?;
    re.captures(html).and_then(|c| {
        let t = c[1].trim();
        (!t.is_empty()).then(|| decode_basic_entities(t))
    })
}

/// Slug PDF du lien « Voir la décision » : un `href` sous
/// `/Media/mediatheque-cnda/<segment>/…/<slug>` (sans extension `.pdf`). La
/// refonte du site sert les PDF sous `…/documents/…` (et non plus seulement
/// `…/images/…`) — on ne fige donc pas le segment. Parmi tous les hrefs média, on
/// retient le **premier lien décision**, identifié par l'un des deux signaux :
/// - slug portant un numéro (`…-n-NNNNNNNN-c`, cf. [`numero_from_slug`]) — fiches
///   anciennes, slug = intitulé d'arrêt ;
/// - chemin sous `…/documents/…` — fiches modernes au slug **descriptif**
///   (`la-cnda-evalue-…`, sans numéro) où le PDF vit sous `documents/` (≠ les
///   illustrations/logos sous `images/`). Le lien 302-redirige vers le fichier réel.
fn extract_pdf_slug(html: &str) -> Option<String> {
    static PDF_RE: OnceLock<Regex> = OnceLock::new();
    let re = PDF_RE.get_or_init(|| {
        Regex::new(r#"href\s*=\s*["']([^"']*?/Media/mediatheque-cnda/[^"']+)["']"#)
            .expect("regex pdf href")
    });
    re.captures_iter(html)
        .map(|c| {
            let raw = c[1].trim();
            // Le slug est sans extension ; si une `.pdf` traîne, on la retire (audit).
            raw.strip_suffix(".pdf").unwrap_or(raw).to_string()
        })
        .find(|slug| numero_from_slug(slug).is_some() || slug.contains("/documents/"))
}

/// `content_type` = première ligne non vide du body **après** le titre éditorial
/// (audit §42 : `Jurisprudence` / `Décision de justice`). Le strip plat fait
/// remonter le h1 en tête, donc on saute la ligne qui répète le titre.
fn content_type_line(body_text: &str, title: Option<&str>) -> Option<String> {
    body_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .find(|l| Some(*l) != title)
        .map(str::to_string)
}

/// Abstract éditorial : la plus longue ligne dans la fenêtre 1300–2700 car.
/// (audit §45 : le bloc d'analyse est nettement plus long que toute autre ligne
/// du body). Heuristique best-effort ; `None` si rien dans la fenêtre.
fn extract_abstract(body_text: &str) -> Option<String> {
    body_text
        .lines()
        .map(str::trim)
        .filter(|l| {
            let n = l.chars().count();
            (1300..=2700).contains(&n)
        })
        .max_by_key(|l| l.chars().count())
        .map(str::to_string)
}

/// Date de mise en ligne dans le body (`<jour> <mois> <année>` en français).
/// Informatif seulement (la date indexée = date de lecture du PDF, audit §85).
fn extract_publication_date(body_text: &str) -> Option<String> {
    static DATE_RE: OnceLock<Regex> = OnceLock::new();
    let re = DATE_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d{1,2}\s+(?:janvier|février|fevrier|mars|avril|mai|juin|juillet|ao\u{fb}t|aout|septembre|octobre|novembre|décembre|decembre)\s+\d{4})\b",
        )
        .expect("regex date fr")
    });
    re.captures(body_text).map(|c| c[1].trim().to_string())
}

/// Décode les entités HTML de base laissées dans un attribut `content`
/// (les balises sont déjà absentes d'un attribut, mais `&amp;`/`&#39;` peuvent
/// traîner). Best-effort minimal — le strip complet vit dans `html_strip`.
fn decode_basic_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

// ----------------------------------------------------------------------------
// Numéro de décision (slug PDF / /Title PDF)
// ----------------------------------------------------------------------------

/// Extrait le numéro de décision d'un slug PDF. Le slug encode `…-n-<numéro>-<classement>`
/// (audit §30 : `cnda-12-mai-2026-m.y.-n-26006334-c`). On cherche le segment
/// numérique le plus long (≥ 6 chiffres), le numéro étant un identifiant long
/// (`26006334`) tandis que les dates sont des nombres courts. `None` si aucun.
pub fn numero_from_slug(slug: &str) -> Option<String> {
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let re = NUM_RE.get_or_init(|| Regex::new(r"\d{6,}").expect("regex numéro slug"));
    re.find_iter(slug)
        .map(|m| m.as_str().to_string())
        .max_by_key(String::len)
}

/// Extrait le numéro de décision d'un `/Title` PDF (audit §56 : le `/Title` vaut
/// exactement le numéro, ex. `26006334`). Tolère un titre bruité : on prend la
/// première suite de ≥ 6 chiffres. `None` si aucune.
pub fn numero_from_pdf_title(title: &str) -> Option<String> {
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let re = NUM_RE.get_or_init(|| Regex::new(r"\d{6,}").expect("regex numéro title"));
    re.find(title).map(|m| m.as_str().to_string())
}

// ----------------------------------------------------------------------------
// Manifeste CNDA (watermark)
// ----------------------------------------------------------------------------

/// Watermark CNDA : dernière page de liste drainée + dernier numéro vu.
/// Variante du [`crate::dila::DilaManifest`] (un fichier JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CndaManifest {
    #[serde(default)]
    pub last_page_done: Option<u32>,
    #[serde(default)]
    pub last_numero: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

impl CndaManifest {
    fn now_iso_seconds() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| SourceError::Invalid(format!("manifest CNDA illisible: {e}")))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| SourceError::Invalid(format!("manifest CNDA non sérialisable: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn mark(&mut self, page: u32, numero: impl Into<String>) {
        self.last_page_done = Some(page);
        self.last_numero = Some(numero.into());
        self.fetched_at = Some(Self::now_iso_seconds());
    }

    /// Pose le watermark du sync incrémental : `last_numero` = la décision **la
    /// plus récente** chargée (tête de la liste antichrono). Sert d'early-exit au
    /// run suivant (on s'arrête dès qu'on la retombe). Ne touche pas
    /// `last_page_done` (la pagination n'est pas un point de reprise en sync).
    pub fn mark_newest(&mut self, numero: impl Into<String>) {
        self.last_numero = Some(numero.into());
        self.fetched_at = Some(Self::now_iso_seconds());
    }
}

/// Chemin du manifeste CNDA sous `data_dir` (`<data_dir>/cnda/manifest.json`).
pub fn manifest_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(CNDA_SOURCE_DIR).join("manifest.json")
}

// ----------------------------------------------------------------------------
// Cache local des payloads PDF
// ----------------------------------------------------------------------------

/// Chemin du PDF caché pour `numero` (`<data_dir>/cnda/payloads/<numero>.pdf`).
/// La CNDA scrape des PDF un par un ; on les persiste comme les autres sources
/// persistent leurs archives (`zips/` opendata, tarballs judilibre) pour qu'un
/// re-run — ou une ré-extraction (changement d'extracteur/OCR) — relise le PDF
/// localement au lieu de re-crawler cnda.fr (lent, throttlé, liens qui meurent).
pub fn payload_path(data_dir: &Path, numero: &str) -> std::path::PathBuf {
    data_dir
        .join(CNDA_SOURCE_DIR)
        .join("payloads")
        .join(format!("{numero}.pdf"))
}

/// Lit le PDF caché pour `numero`, ou `None` si absent du cache.
pub fn load_cached_payload(data_dir: &Path, numero: &str) -> Result<Option<Vec<u8>>> {
    let path = payload_path(data_dir, numero);
    if path.exists() {
        Ok(Some(fs::read(&path)?))
    } else {
        Ok(None)
    }
}

/// Écrit le PDF dans le cache (write atomique via fichier temporaire + rename).
pub fn save_cached_payload(data_dir: &Path, numero: &str, bytes: &[u8]) -> Result<()> {
    let path = payload_path(data_dir, numero);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pdf.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Chemin du markdown OCR caché pour `numero` (`<data_dir>/cnda/ocr/<numero>.md`).
/// 2ᵉ couche de cache au-dessus du PDF : l'OCR Mistral est **non déterministe,
/// payant et rate-limité** ; on le fige par décision pour ne le rejouer **qu'une
/// fois par PDF**. Tout l'aval (strip markdown + `clean_texte` + sections) devient
/// alors un transform local pur, itérable sans re-OCR. Re-OCR seulement si le PDF
/// source change (ADR 0108, même principe que jafbase).
pub fn ocr_path(data_dir: &Path, numero: &str) -> std::path::PathBuf {
    data_dir
        .join(CNDA_SOURCE_DIR)
        .join("ocr")
        .join(format!("{numero}.md"))
}

/// Lit le markdown OCR caché pour `numero`, ou `None` si absent.
pub fn load_cached_ocr(data_dir: &Path, numero: &str) -> Result<Option<String>> {
    let path = ocr_path(data_dir, numero);
    if path.exists() {
        Ok(Some(fs::read_to_string(&path)?))
    } else {
        Ok(None)
    }
}

/// Écrit le markdown OCR dans le cache (write atomique via fichier temporaire + rename).
pub fn save_cached_ocr(data_dir: &Path, numero: &str, markdown: &str) -> Result<()> {
    let path = ocr_path(data_dir, numero);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, markdown)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trailing_slash_trimmed() {
        let c = CndaClient::with_base_url("https://x/");
        assert_eq!(c.base_url, "https://x");
        assert_eq!(c.absolutize("/foo/bar"), "https://x/foo/bar");
        assert_eq!(c.absolutize("https://y/z"), "https://y/z");
    }

    #[test]
    fn enumerate_keeps_only_fiche_paths_dedup_ordered() {
        // Structure du site post-refonte (vérifiée 2026-06-15) : fiches sous
        // `decisions-jurisprudentielles/<slug>` ; l'index nu + sa pagination, les
        // `dernieres-decisions`, ancres et sous-rubriques sœurs sont exclus.
        let html = r##"
            <a href="/decisions-de-justice/jurisprudence/decisions-jurisprudentielles?page=2">suiv</a>
            <a href="/decisions-de-justice/jurisprudence/decisions-jurisprudentielles">index</a>
            <a href="/decisions-de-justice/jurisprudence/dernieres-decisions">latest</a>
            <a href="/decisions-de-justice/jurisprudence/bulletins-et-notes-juridiques-mensuels">nav</a>
            <a href="/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/cour-reconnait-refugie-soudanais-four">f1</a>
            <a href="https://www.cnda.fr/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/revision-ofpra-exclusion">f2</a>
            <a href="/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/cour-reconnait-refugie-soudanais-four">f1 again</a>
            <a href="#chapitre-1">ancre</a>
        "##;
        let urls = enumerate_fiche_urls(html);
        assert_eq!(
            urls,
            vec![
                "/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/cour-reconnait-refugie-soudanais-four"
                    .to_string(),
                "/decisions-de-justice/jurisprudence/decisions-jurisprudentielles/revision-ofpra-exclusion".to_string(),
            ]
        );
    }

    #[test]
    fn numero_from_slug_picks_long_number() {
        // Le slug porte la date (nombres courts) + le numéro (≥ 6 chiffres).
        assert_eq!(
            numero_from_slug("cnda-12-mai-2026-m.y.-n-26006334-c").as_deref(),
            Some("26006334")
        );
        assert_eq!(
            numero_from_slug(
                "/Media/mediatheque-cnda/images/2024/cnda-3-juin-2024-m.t.-n-24031657-c"
            )
            .as_deref(),
            Some("24031657")
        );
        assert_eq!(numero_from_slug("aucun-numero-ici-2026").as_deref(), None);
    }

    #[test]
    fn numero_from_pdf_title_takes_digits() {
        assert_eq!(
            numero_from_pdf_title("26006334").as_deref(),
            Some("26006334")
        );
        assert_eq!(
            numero_from_pdf_title("N° 24031657 ").as_deref(),
            Some("24031657")
        );
        assert_eq!(numero_from_pdf_title("sans numero").as_deref(), None);
    }

    #[test]
    fn parse_fiche_extracts_title_abstract_pdf_and_date() {
        let abstract_txt = "En premier lieu, ".to_string()
            + &"la Cour relève que le requérant établit le bien-fondé de ses craintes. ".repeat(20);
        assert!((1300..=2700).contains(&abstract_txt.chars().count()));
        let html = format!(
            r##"<html><head>
            <meta property="og:title" content="Titre OG tronqué">
            </head><body>
            <h1>La Cour reconnait la qualité de réfugié</h1>
            <p>Jurisprudence</p>
            <p>Publié le 1 juin 2026</p>
            <p>{abstract_txt}</p>
            <a href="/Media/mediatheque-cnda/images/2026/cnda-12-mai-2026-m.y.-n-26006334-c">Voir la décision</a>
            </body></html>"##
        );
        let fiche = parse_fiche(&html, "/decisions-de-justice/jurisprudence/f1").unwrap();
        assert_eq!(
            fiche.title.as_deref(),
            Some("La Cour reconnait la qualité de réfugié")
        );
        assert_eq!(fiche.content_type.as_deref(), Some("Jurisprudence"));
        assert_eq!(
            fiche.pdf_url.as_deref(),
            Some("/Media/mediatheque-cnda/images/2026/cnda-12-mai-2026-m.y.-n-26006334-c")
        );
        assert_eq!(fiche.publication_date.as_deref(), Some("1 juin 2026"));
        assert!(fiche
            .editorial_abstract
            .as_deref()
            .unwrap()
            .starts_with("En premier lieu,"));
        // Le numéro se relit du slug PDF (triple source robuste).
        assert_eq!(
            numero_from_slug(fiche.pdf_url.as_deref().unwrap()).as_deref(),
            Some("26006334")
        );
    }

    #[test]
    fn parse_fiche_extracts_pdf_under_documents_segment_ignoring_asset_links() {
        // Refonte du site : le PDF « Voir la décision » est servi sous
        // `…/documents/…` (et non plus `…/images/…`). On ne fige pas le segment ;
        // un autre href média sans numéro (logo) ne doit pas être pris pour le PDF.
        let abstract_txt = "En premier lieu, ".to_string()
            + &"la Cour relève le bien-fondé des craintes du requérant. ".repeat(25);
        assert!((1300..=2700).contains(&abstract_txt.chars().count()));
        let html = format!(
            r##"<html><body>
            <a href="/Media/mediatheque-cnda/images/logo/banniere-cnda"><img></a>
            <h1>Soudan : violence aveugle d'intensité exceptionnelle</h1>
            <p>Jurisprudence</p>
            <p>{abstract_txt}</p>
            <a href="/Media/mediatheque-cnda/documents/2024/decembre/cnda-17-juillet-2024-m.-j.-n-24009379-c">&gt; Voir la décision</a>
            </body></html>"##
        );
        let fiche = parse_fiche(&html, "/x").unwrap();
        assert_eq!(
            fiche.pdf_url.as_deref(),
            Some("/Media/mediatheque-cnda/documents/2024/decembre/cnda-17-juillet-2024-m.-j.-n-24009379-c")
        );
        assert_eq!(
            numero_from_slug(fiche.pdf_url.as_deref().unwrap()).as_deref(),
            Some("24009379")
        );
    }

    #[test]
    fn parse_fiche_picks_documents_pdf_with_descriptive_slug_no_numero() {
        // Fiche moderne : lien PDF sous `documents/` au slug **descriptif** (sans
        // `-n-NNNNNNNN-`). Doit être retenu (signal `/documents/`), le numéro venant
        // alors du corps (`n°25013796`), pas du slug ; le logo `images/` est ignoré.
        let abstract_txt = "En premier lieu, ".to_string()
            + &"la Cour évalue la crédibilité de la demande de protection. ".repeat(25);
        let html = format!(
            r##"<html><body>
            <a href="/Media/mediatheque-cnda/images/logo/banniere-cnda"><img></a>
            <h1>Guinée : risque d'excision</h1>
            <p>Jurisprudence</p>
            <p>Dans l'affaire n°25013796, la Cour … (n°25013796 rappelé).</p>
            <p>{abstract_txt}</p>
            <a href="/Media/mediatheque-cnda/documents/20257/juillet/la-cnda-evalue-la-credibilite-en-guinee">&gt; Voir la décision</a>
            </body></html>"##
        );
        let fiche = parse_fiche(&html, "/x").unwrap();
        assert_eq!(
            fiche.pdf_url.as_deref(),
            Some("/Media/mediatheque-cnda/documents/20257/juillet/la-cnda-evalue-la-credibilite-en-guinee")
        );
        // Slug sans numéro → repli sur le corps.
        assert_eq!(numero_from_slug(fiche.pdf_url.as_deref().unwrap()), None);
        assert_eq!(fiche.numero.as_deref(), Some("25013796"));
    }

    #[test]
    fn parse_fiche_without_pdf_keeps_abstract_fiche_only() {
        let abstract_txt = "En premier lieu, ".to_string()
            + &"motivation résumée par les juristes de la Cour. ".repeat(30);
        assert!((1300..=2700).contains(&abstract_txt.chars().count()));
        let html = format!(
            "<html><body><h1>Titre</h1><p>Jurisprudence</p><p>{abstract_txt}</p></body></html>"
        );
        let fiche = parse_fiche(&html, "/x").unwrap();
        assert!(fiche.pdf_url.is_none());
        assert!(fiche.editorial_abstract.is_some());
    }

    #[test]
    fn parse_fiche_franc_error_when_no_pdf_no_abstract() {
        // Gabarit cassé : ni lien PDF, ni bloc d'analyse → erreur franche (#12).
        let html = "<html><body><h1>Titre seul</h1><p>Jurisprudence</p></body></html>";
        let err = parse_fiche(html, "/broken").unwrap_err();
        assert!(matches!(err, SourceError::Invalid(_)));
    }

    #[test]
    fn numero_from_body_picks_most_frequent() {
        // La décision sujet recurre ; une décision citée n'apparaît qu'une fois.
        let body = "Par la décision n°21059246 citée, la Cour a statué. Dans la \
                    présente affaire n°25013796, … le n°25013796 est rejeté. Voir aussi \
                    n°25013796 pour le raisonnement.";
        assert_eq!(extract_numero_from_body(body).as_deref(), Some("25013796"));
        // Variante casse/espace `N° 24031657`.
        assert_eq!(
            extract_numero_from_body("Décision N° 24031657 du jour").as_deref(),
            Some("24031657")
        );
        // Aucun numéro à 6+ chiffres → None.
        assert_eq!(
            extract_numero_from_body("aucun numéro ici").as_deref(),
            None
        );
    }

    #[test]
    fn fiche_to_value_roundtrips_fields() {
        let fiche = CndaFiche {
            fiche_url: "/x".to_string(),
            title: Some("T".to_string()),
            content_type: Some("Jurisprudence".to_string()),
            editorial_abstract: Some("A".to_string()),
            pdf_url: Some("/Media/…/slug".to_string()),
            numero: Some("26006334".to_string()),
            lecture_date: Some("12 mai 2026".to_string()),
            publication_date: Some("1 juin 2026".to_string()),
        };
        let v = fiche.to_value();
        assert_eq!(v["title"].as_str(), Some("T"));
        assert_eq!(v["pdf_url"].as_str(), Some("/Media/…/slug"));
        assert_eq!(v["numero"].as_str(), Some("26006334"));
        assert_eq!(v["lecture_date"].as_str(), Some("12 mai 2026"));
        assert_eq!(v["content_type"].as_str(), Some("Jurisprudence"));
    }

    #[test]
    fn lecture_date_from_body_reads_decision_ref_not_publication() {
        // Réf de décision `CNDA <date> … n°…` = date de lecture ; le `28 juillet
        // 2016` qui suit `Jurisprudence` (content_type) est la mise en ligne.
        let body = "Jurisprudence 28 juillet 2016 Accueil … la Cour (CNDA 27 juillet \
                    2016 M. A. n° 16012935 C) juge que …";
        assert_eq!(
            extract_lecture_date_from_body(body).as_deref(),
            Some("27 juillet 2016")
        );
        // CRR ancien.
        assert_eq!(
            extract_lecture_date_from_body("réf CRR 18 avril 2005 M. O. n° 455708").as_deref(),
            Some("18 avril 2005")
        );
        assert_eq!(
            extract_lecture_date_from_body("aucune référence").as_deref(),
            None
        );
    }

    #[test]
    fn cached_payload_roundtrip_and_path() {
        let dir = tempfile::tempdir().unwrap();
        // Format de chemin = contrat (la ré-extraction s'appuie dessus).
        assert_eq!(
            payload_path(dir.path(), "25043827"),
            dir.path().join("cnda/payloads/25043827.pdf")
        );
        // Cache miss avant écriture.
        assert_eq!(load_cached_payload(dir.path(), "25043827").unwrap(), None);
        // Round-trip : écrit (création du répertoire) puis relit à l'identique.
        let bytes = b"%PDF-1.7 fake".to_vec();
        save_cached_payload(dir.path(), "25043827", &bytes).unwrap();
        assert_eq!(
            load_cached_payload(dir.path(), "25043827").unwrap(),
            Some(bytes)
        );
    }

    #[test]
    fn manifest_roundtrip_and_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = manifest_path(dir.path());
        let mut m = CndaManifest::load(&path).unwrap();
        assert_eq!(m.last_page_done, None);
        m.mark(3, "26006334");
        m.save(&path).unwrap();
        let reloaded = CndaManifest::load(&path).unwrap();
        assert_eq!(reloaded.last_page_done, Some(3));
        assert_eq!(reloaded.last_numero.as_deref(), Some("26006334"));
        assert!(reloaded.fetched_at.is_some());
    }
}
