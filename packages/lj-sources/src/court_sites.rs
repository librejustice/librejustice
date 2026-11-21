//! The sites African courts publish their decisions on: a listing page, then each decision's
//! PDF, one request at a time through the egress gateway.

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::{Result, SourceError};

pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// A refusal waits this long before the request is asked again, twice at most.
const REFUSAL_PAUSE: Duration = Duration::from_secs(120);
/// The pauses before a request whose transport failed is sent again.
const TRANSPORT_PAUSES: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

pub struct CourtSiteClient {
    http: reqwest::Client,
    pace: Duration,
    last: Mutex<Option<Instant>>,
}

impl CourtSiteClient {
    /// `pace` is the least time between two requests.
    pub fn new(proxy: &str, pace: Duration) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "a court site is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self {
            http,
            pace,
            last: Mutex::new(None),
        })
    }

    /// The body `read` takes from the answer to `url`, a GET or, with `form`, a POST of that
    /// urlencoded form; `None` when the site answers one of `ends`. A transport failure, the body
    /// cut included, sends the request again.
    async fn get<T, F, Fut>(
        &self,
        url: &str,
        form: Option<&str>,
        ends: &[u16],
        read: F,
    ) -> Result<Option<T>>
    where
        F: Fn(reqwest::Response) -> Fut,
        Fut: Future<Output = reqwest::Result<T>>,
    {
        let mut last = self.last.lock().await;
        let mut pauses = TRANSPORT_PAUSES.iter();
        let mut refusals = 0;
        loop {
            if let Some(at) = *last {
                tokio::time::sleep(self.pace.saturating_sub(at.elapsed())).await;
            }
            let request = match form {
                Some(form) => self
                    .http
                    .post(url)
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(form.to_string()),
                None => self.http.get(url),
            };
            let response = request.send().await;
            *last = Some(Instant::now());
            let failure = match response {
                Err(e) => e,
                Ok(response) => {
                    let status = response.status().as_u16();
                    if matches!(status, 403 | 429 | 503) {
                        refusals += 1;
                        if refusals == 3 {
                            return Err(SourceError::Invalid(format!(
                                "court site answers {status} on {url}"
                            )));
                        }
                        tracing::warn!(status, url, "court site refuses, the request waits");
                        tokio::time::sleep(REFUSAL_PAUSE).await;
                        continue;
                    }
                    if ends.contains(&status) {
                        return Ok(None);
                    }
                    let body = read(response.error_for_status()?).await;
                    *last = Some(Instant::now());
                    match body {
                        Ok(body) => return Ok(Some(body)),
                        Err(e) => e,
                    }
                }
            };
            let Some(pause) = pauses.next() else {
                return Err(failure.into());
            };
            tracing::warn!(url, error = %failure, "court site: the transport failed, the request waits");
            tokio::time::sleep(*pause).await;
        }
    }

    /// A page's text.
    pub async fn page(&self, url: &str) -> Result<String> {
        Ok(self
            .get(url, None, &[], reqwest::Response::text)
            .await?
            .expect("no status ends a page"))
    }

    /// A page's text, `None` when the site answers 400 or 404: a WordPress API asked for a page
    /// past its last.
    pub async fn page_or_end(&self, url: &str) -> Result<Option<String>> {
        self.get(url, None, &[400, 404], reqwest::Response::text)
            .await
    }

    /// The text the site answers to `form` posted to `url`, `None` when it answers 400 or 404.
    pub async fn form_or_end(&self, url: &str, form: &str) -> Result<Option<String>> {
        self.get(url, Some(form), &[400, 404], reqwest::Response::text)
            .await
    }

    /// A file's bytes, `None` when the site answers 404.
    pub async fn file(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.get(url, None, &[404], |r| async move {
            r.bytes().await.map(|b| b.to_vec())
        })
        .await
    }
}

/// The URL of a page of the Cour constitutionnelle du Bénin's listing, newest first.
pub fn cc_bj_listing_url(page: usize) -> String {
    format!("https://courconstitutionnelle.bj/fr/decisions?page={page}")
}

/// The URL of the `page`-th page, from 1, of the African Court on Human and Peoples' Rights'
/// decisions API, newest first, 150 decisions a page counted from the first one shown.
pub fn african_court_listing_url(page: usize) -> String {
    format!(
        "https://www.african-court.org/ajudata/apidata/decisions?start={}",
        (page - 1) * 150
    )
}

/// The African Court's finalised advisory proceedings, all on one page.
pub const AFRICAN_COURT_ADVISORY_URL: &str =
    "https://www.african-court.org/ajudata/advisory-finalised";

/// The URL of the `page`-th page, from 1, of the Conseil constitutionnel de Côte d'Ivoire's
/// listing, newest first; the site counts its pages from 0.
pub fn cc_ci_listing_url(page: usize) -> String {
    format!(
        "https://www.conseil-constitutionnel.ci/decisions?page={}",
        page - 1
    )
}

/// A page of the Conseil constitutionnel de Djibouti's PDF uploads, a hundred at a time, newest
/// first.
pub fn cc_dj_media_url(page: usize) -> String {
    format!(
        "https://www.conseil-constitutionnel.dj/wp-json/wp/v2/media?mime_type=application/pdf\
         &per_page=100&page={page}&_fields=id,link,title,source_url"
    )
}

/// A page of the Haute Cour constitutionnelle's posts, a hundred at a time, newest first, in the
/// categories of its arrêts, opinions, decisions, deliberations and elections.
pub fn hcc_mg_posts_url(page: usize) -> String {
    format!(
        "https://www.hcc.gov.mg/index.php?rest_route=/wp/v2/posts&categories=3,20,4,7,8,9,24,6\
         &per_page=100&page={page}&_fields=id,date,link,title,content"
    )
}

/// The three natures `jurisprudence.justice.gov.mg` lists its arrêts by.
pub const JURISPRUDENCE_MG_NATURES: [&str; 3] = ["civile", "commerciale", "sociale"];

/// A page of the Cour de cassation de Madagascar's arrêts of one nature, twenty at a time.
pub fn jurisprudence_mg_listing_url(nature: &str, page: usize) -> String {
    format!(
        "https://jurisprudence.justice.gov.mg/decision/{nature}?nature={}&page={page}",
        nature.to_uppercase()
    )
}

/// The matters the Cour de cassation de Côte d'Ivoire lists its arrêts by.
pub const CASSATION_CI_MATTERS: [&str; 3] = ["civ_com", "pen", "soc"];

/// A page, from 1, of the Cour de cassation de Côte d'Ivoire's arrêts of one matter, ten a page.
pub fn cassation_ci_listing_url(matter: &str, page: usize) -> String {
    format!("https://eservices.courdecassation.ci/search/stoppages?matter={matter}&nb_page={page}")
}

/// The free collections of the CNDJ de Côte d'Ivoire, one a judicial year: 2017-2018, 2018-2019.
pub const CNDJ_CI_COLLECTIONS: [&str; 2] = ["4", "5"];

/// A page, from 1, of a free CNDJ collection, ten decisions a page.
pub fn cndj_ci_listing_url(collection: &str, page: usize) -> String {
    format!("https://www.cndj.ci/jurisprudences/{collection}?page={page}")
}

/// The categories `biblio.ohada.org` files the CCJA's decisions under, one a year, newest first.
pub const CCJA_YEAR_CATEGORIES: [&str; 28] = [
    "7330", "7328", "7327", "7326", "7324", "7320", "7309", "7288", "7232", "7231", "7230", "7229",
    "7197", "7214", "7213", "7212", "7211", "7210", "7209", "7208", "7207", "7206", "7205", "7204",
    "7203", "7306", "7201", "7200",
];

/// A page, from 1, of one of [`CCJA_YEAR_CATEGORIES`], [`lj_parse::CCJA_PAGE`] notices a page.
pub fn ccja_listing_url(category: &str, page: usize) -> String {
    format!(
        "https://biblio.ohada.org/index.php?lvl=categ_see&id={category}&page={page}\
         &nb_per_page_custom={}",
        lj_parse::CCJA_PAGE
    )
}

/// The Cour constitutionnelle du Maroc's decisions, read by posting [`cc_ma_listing_form`].
pub const CC_MA_LISTING_URL: &str = "https://www.cour-constitutionnelle.ma/Decision/GetDecisions";

/// The form asking a page, from 1, of the Cour constitutionnelle du Maroc's decisions of every
/// year and every body, [`lj_parse::CC_MA_PAGE`] a page, the last posted first.
pub fn cc_ma_listing_form(page: usize) -> String {
    format!(
        "draw={page}&start={}&length={}&order%5B0%5D%5Bcolumn%5D=0&order%5B0%5D%5Bdir%5D=desc\
         &columns%5B0%5D%5Bdata%5D=iddecision&columns%5B0%5D%5Bname%5D=Iddecision\
         &Annee=&spec=&NumDec=&TxtLibre=&Inst=",
        (page - 1) * lj_parse::CC_MA_PAGE,
        lj_parse::CC_MA_PAGE
    )
}
