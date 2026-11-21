//! Senegal's and OHADA's texts on SenegalLII: a work its page renders in Akoma Ntoso has each
//! dated expression for a version, a work shown as a PDF is read from its text layer or its scan.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::{anyhow, Result};
use lj_bucket::{key, Bucket};
use lj_norm::text_aliases::{self, TextAlias};
use lj_parse::{ConsolidatedArticle, PeachjamListed, PeachjamPage};
use lj_sources::senlii::SenliiClient;
use lj_store::repository::DecisionRepository;
use regex::Regex;
use serde_json::Value;

use super::super::legal_corpus::{
    article_status, load_doc, write_aliases, CorpusArticle, CorpusDoc, CorpusVersion,
};
use super::{refused, Run, Tally};
use crate::ocr::{self, Ocr};

pub(super) const SOURCE: &str = "senlii";
/// Fewer characters a page than this, and a PDF is a scan.
const NATIVE_CHARS_PER_PAGE: usize = 200;
static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^code\b|\bportant (?:le |nouveau )?code\b").unwrap());

/// The day of an expression's path, `…/fra@1965-09-06`.
fn day(path: &str) -> &str {
    path.rsplit_once('@').map_or("", |(_, d)| d)
}

/// The uid a work's URI gives and the number it is cited by: `65-60` for a law of 1965,
/// `2016-32` for one of 2016, none for an arrêté nor for a work numbered by words.
fn identity(jurisdiction: &str, work: &str) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = work.trim_start_matches("/akn/").split('/').collect();
    let (subtype, year, number) = match parts[..] {
        [_, "act", year, number] => (None, year, number),
        [_, "act", subtype, year, number] => (Some(subtype), year, number),
        _ => return None,
    };
    let year = year.get(..4)?;
    let number = number
        .trim_start_matches("acte-uniforme-")
        .trim_start_matches("acte-");
    let code = match (jurisdiction, subtype) {
        ("OHADA", None) => "ACTE-UNIFORME".to_string(),
        (_, None | Some("law" | "ol")) => "LOI".to_string(),
        (_, Some("decree")) => "DECRET".to_string(),
        (_, Some("mo" | "order")) => "ARRETE".to_string(),
        (_, Some("rules")) => "REGLEMENT".to_string(),
        (_, Some("treaty")) => "TRAITE".to_string(),
        (_, Some(other)) => lj_parse::journal_number_key(other),
    };
    let cited = if number.chars().any(char::is_alphabetic) || code == "ARRETE" {
        None
    } else if number.contains('-') {
        Some(number.to_string())
    } else if year < "2000" {
        Some(format!("{}-{number}", &year[2..]))
    } else {
        Some(format!("{year}-{number}"))
    };
    let key = match &cited {
        Some(cited) => lj_parse::journal_number_key(cited),
        None => lj_parse::journal_number_key(&format!("{year}-{number}")),
    };
    Some((format!("{jurisdiction}/{code}-{key}"), cited))
}

/// The nature a title opens on, else the uid's.
fn nature(title: &str, uid: &str) -> String {
    const OPENINGS: &[(&str, &str)] = &[
        ("loi organique", "LOI_ORGANIQUE"),
        ("loi constitutionnelle", "LOI_CONSTITUTIONNELLE"),
        ("loi", "LOI"),
        ("decret", "DECRET"),
        ("arrete", "ARRETE"),
        ("ordonnance", "ORDONNANCE"),
        ("acte uniforme", "ACTE_UNIFORME"),
        ("traite", "TRAITE"),
        ("reglement", "REGLEMENT"),
        ("arbitration rules", "REGLEMENT"),
        ("decision", "DECISION"),
        ("circulaire", "CIRCULAIRE"),
        ("constitution", "CONSTITUTION"),
    ];
    let folded = lj_fold::fold(title).to_lowercase();
    OPENINGS
        .iter()
        .find(|(opening, _)| {
            folded
                .strip_prefix(opening)
                .is_some_and(|rest| !rest.starts_with(char::is_alphanumeric))
        })
        .map(|(_, nature)| nature.to_string())
        .unwrap_or_else(|| {
            let code = uid.split_once('/').map_or(uid, |(_, rest)| rest);
            code.split('-').next().unwrap_or(code).to_string()
        })
}

static IN_FORCE_ON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)entr\w* en vigueur le (\d{1,2}(?:er)? \p{L}+ \d{4})").unwrap()
});

/// The day an expression comes into force: an OHADA act replacing another under its name on the
/// successions table's day, else on the day its articles name, not before SenegalLII's date,
/// else a uniform act's 90 days after that date, its publication in the OHADA Official Journal.
fn in_force(uid: &str, day: &str, articles: &[ConsolidatedArticle]) -> String {
    if !uid.starts_with("OHADA/") {
        return day.to_string();
    }
    if let Some(since) = lj_norm::text_successions::in_force_since(uid) {
        return since.to_string();
    }
    let named = articles
        .iter()
        .rev()
        .find_map(|a| IN_FORCE_ON.captures(&a.texte))
        .and_then(|c| lj_core::dates::parse_date_fr(&c[1]))
        .map(|d| d.to_string())
        .filter(|d| d.as_str() >= day);
    match named {
        Some(d) => d,
        None if uid.starts_with("OHADA/ACTE-UNIFORME-") => {
            let published = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .expect("an expression URI is dated `@YYYY-MM-DD`");
            (published + chrono::Days::new(90)).to_string()
        }
        None => day.to_string(),
    }
}

/// A scan's kind: `CODE` for a work whose title enacts a code, else its nature.
fn kind(title: &str, uid: &str) -> String {
    match CODE.is_match(&lj_fold::fold(title).to_lowercase()) {
        true => "CODE".to_string(),
        false => nature(title, uid),
    }
}

/// The order the OCR reads scans in, codes first; `None` for a kind it leaves for later.
fn ocr_rank(kind: &str) -> Option<u8> {
    match kind {
        "CODE" => Some(0),
        "LOI" | "LOI_ORGANIQUE" | "LOI_CONSTITUTIONNELLE" | "ORDONNANCE" | "CONSTITUTION" => {
            Some(1)
        }
        "DECRET" => Some(2),
        _ => None,
    }
}

/// An expression the page lists after its own: the sitemap can name an older one, whose page
/// renders no text.
fn later(page: &PeachjamPage) -> Option<&str> {
    page.expressions
        .iter()
        .filter(|(day, _)| *day > page.expression_date)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path.as_str())
}

fn checksum(listed: &PeachjamListed) -> u64 {
    xxhash_rust::xxh3::xxh3_64(format!("{}|{:?}", listed.path, listed.modified).as_bytes())
}

/// What a run does with a listed work.
enum Role {
    Read {
        uid: String,
        cited: Option<String>,
    },
    /// Its uid belongs to a work with a later expression.
    Twice,
    Unkeyed,
}

fn roles<'a>(jurisdiction: &str, listed: &'a [PeachjamListed]) -> HashMap<&'a str, Role> {
    let mut latest: HashMap<String, &str> = HashMap::new();
    for l in listed {
        if let Some((uid, _)) = identity(jurisdiction, &l.work) {
            let owner = latest.entry(uid).or_insert(&l.path);
            if day(&l.path) > day(owner) {
                *owner = &l.path;
            }
        }
    }
    listed
        .iter()
        .map(|l| {
            let role = match identity(jurisdiction, &l.work) {
                None => Role::Unkeyed,
                Some((uid, _)) if latest.get(&uid) != Some(&l.path.as_str()) => Role::Twice,
                Some((uid, cited)) => Role::Read { uid, cited },
            };
            (l.path.as_str(), role)
        })
        .collect()
}

/// Each article over the expressions read, oldest first: a wording holds from the day of the
/// expression that first shows it to that of the next one that changes or drops it.
fn articles(expressions: &[(String, Vec<ConsolidatedArticle>)]) -> Vec<CorpusArticle> {
    let keyed: Vec<HashMap<String, &ConsolidatedArticle>> = expressions
        .iter()
        .map(|(_, arts)| {
            let mut by_key = HashMap::new();
            for a in arts {
                by_key
                    .entry(lj_norm::article_key::identity_key(&a.num))
                    .or_insert(a);
            }
            by_key
        })
        .collect();
    let mut order: Vec<String> = Vec::new();
    for (_, arts) in expressions.iter().rev() {
        for a in arts {
            let key = lj_norm::article_key::identity_key(&a.num);
            if !order.contains(&key) {
                order.push(key);
            }
        }
    }
    order
        .into_iter()
        .map(|key| {
            let mut versions: Vec<CorpusVersion> = Vec::new();
            let mut latest: Option<&ConsolidatedArticle> = None;
            for ((day, _), by_key) in expressions.iter().zip(&keyed) {
                let found = by_key.get(&key).copied();
                let open = versions.last_mut().filter(|v| v.date_fin.is_none());
                if let (Some(v), Some(a)) = (&open, found) {
                    if v.texte == a.texte {
                        latest = Some(a);
                        continue;
                    }
                }
                if let Some(v) = open {
                    v.date_fin = Some(day.clone());
                }
                if let Some(a) = found {
                    versions.push(CorpusVersion {
                        date_debut: Some(day.clone()),
                        date_fin: None,
                        status: article_status(&a.texte).to_string(),
                        texte: a.texte.clone(),
                        source: None,
                        source_url: None,
                        source_asof: None,
                        source_upstream_url: None,
                    });
                    latest = Some(a);
                }
            }
            let latest = latest.expect("an ordered key comes from an expression");
            CorpusArticle {
                num: latest.num.clone(),
                num_key: None,
                texte: None,
                texte_original: None,
                lang_original: None,
                translation: None,
                nota: None,
                versions: Some(versions),
                title_path: latest.title_path.clone(),
            }
        })
        .collect()
}

fn single(
    num: String,
    num_key: Option<String>,
    texte: &str,
    path: Option<String>,
) -> CorpusArticle {
    CorpusArticle {
        num,
        num_key,
        texte: Some(lj_core::normalizer::normalize_chars(texte)),
        texte_original: None,
        lang_original: None,
        translation: None,
        nota: None,
        versions: None,
        title_path: path,
    }
}

/// The body of a text that numbers no article, as one piece.
fn body(texte: &str) -> CorpusArticle {
    single(String::new(), Some("body".to_string()), texte, None)
}

/// The articles of a text read from a PDF, else its body as one piece.
fn text_articles(text: &str) -> Vec<CorpusArticle> {
    let articles: Vec<CorpusArticle> = lj_parse::journal_text_articles(text)
        .into_iter()
        .map(|a| single(a.num, None, &a.texte, a.title_path))
        .collect();
    match articles.is_empty() {
        false => articles,
        true => vec![body(&lj_parse::journal_text_body(text))],
    }
}

/// A work served as a scan, read once the text layers are.
struct Scanned {
    work: String,
    sum: u64,
    uid: String,
    cited: Option<String>,
    page: PeachjamPage,
    pages: u32,
    kind: String,
}

fn normalised(mut articles: Vec<ConsolidatedArticle>) -> Vec<ConsolidatedArticle> {
    for a in &mut articles {
        a.texte = lj_core::normalizer::normalize_chars(&a.texte);
    }
    articles.retain(|a| !a.texte.trim().is_empty());
    articles
}

impl Run<'_> {
    /// The run's budget is spent: a batch of modified works waits for the next run too, its
    /// visits unchanged.
    fn spent(&self) -> bool {
        self.deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
    }

    fn senlii_doc(
        &self,
        uid: &str,
        page: &PeachjamPage,
        articles: Vec<CorpusArticle>,
    ) -> CorpusDoc {
        CorpusDoc {
            text_uid: uid.to_string(),
            source: SOURCE.to_string(),
            jurisdiction: self.jurisdiction.to_string(),
            title: page.title.clone(),
            title_key: None,
            nature: nature(&page.title, uid),
            translation: "officiel".to_string(),
            source_url: Some(SenliiClient::url(&format!(
                "/fr{}/{}@{}",
                page.work, page.language, page.expression_date
            ))),
            source_asof: None,
            source_upstream_url: None,
            date_texte: page.assent.clone(),
            effect_date: Some(in_force(uid, &page.expression_date, &page.articles)),
            detect_abrogation: false,
            num_prefix_agnostic: false,
            articles,
            links: Vec::new(),
        }
    }

    /// Writes `doc` and its names, true once both are.
    async fn senlii_write(
        &self,
        repo: &DecisionRepository<'_>,
        doc: &CorpusDoc,
        cited: Option<&str>,
        tally: &mut Tally,
    ) -> bool {
        let aliases = [
            text_aliases::of(&doc.text_uid),
            cited.into_iter().map(TextAlias::number).collect(),
        ]
        .concat();
        let load = async {
            load_doc(
                repo,
                doc,
                self.read_on,
                self.source,
                &mut tally.publie_targets,
            )
            .await?;
            write_aliases(repo, &doc.text_uid, &aliases).await
        };
        match load.await {
            Ok(_) => {
                tally.written += 1;
                true
            }
            Err(e) => {
                tracing::error!(publisher = self.source, key = doc.text_uid, error = %e, "followed text failed");
                tally.failed.push(doc.text_uid.clone());
                false
            }
        }
    }

    /// A scan's text, from Mistral's answer kept in the bucket, else read by the OCR, its answer kept.
    async fn senlii_ocr(
        &self,
        client: &SenliiClient,
        ocr: &Ocr,
        bucket: &Bucket,
        pdf: &str,
        label: &str,
    ) -> Result<String> {
        let answer = key::ocr(&format!("senlii{pdf}"));
        let pages = match ocr::kept::<Vec<Value>>(bucket, &answer).await? {
            Some(pages) => pages,
            None => {
                let bytes = client.pdf(pdf).await?;
                ocr.document_kept(bucket, &answer, &bytes, label)
                    .await?
                    .pages
            }
        };
        Ok(lj_parse::clean_ocr_markdown(&ocr::text(&pages)))
    }

    /// The articles of a work its page renders in Akoma Ntoso, from every expression it lists.
    async fn senlii_expressions(
        &self,
        client: &SenliiClient,
        uid: &str,
        page: &PeachjamPage,
    ) -> Result<Vec<CorpusArticle>> {
        let numbers: HashSet<String> = page
            .articles
            .iter()
            .map(|a| lj_norm::article_key::identity_key(&a.num))
            .collect();
        if numbers.len() < page.articles.len() {
            tracing::warn!(
                publisher = self.source,
                work = page.work,
                repeated = page.articles.len() - numbers.len(),
                "an article numbered twice keeps its first"
            );
        }
        let mut read: Vec<(String, Vec<ConsolidatedArticle>)> = Vec::new();
        let mut bodies: Vec<String> = vec![page.body.clone()];
        for (expression, path) in &page.expressions {
            if expression == &page.expression_date {
                continue;
            }
            let older = client.page(path).await?;
            bodies.push(older.body);
            read.push((expression.clone(), normalised(older.articles)));
        }
        read.push((
            page.expression_date.clone(),
            normalised(page.articles.clone()),
        ));
        for (day, arts) in &mut read {
            *day = in_force(uid, day, arts);
        }
        read.sort_by(|a, b| a.0.cmp(&b.0));
        read.retain(|(_, arts)| !arts.is_empty());
        Ok(match read.is_empty() {
            false => articles(&read),
            true => bodies
                .iter()
                .find(|b| !b.trim().is_empty())
                .map(|b| body(b))
                .into_iter()
                .collect(),
        })
    }

    /// Reads the sitemap, then the works of `place` never visited and those it lists as modified.
    pub(super) async fn senlii_lane(
        &self,
        client: &SenliiClient,
        place: &str,
        limit: Option<usize>,
    ) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.senlii(client, place, limit, &mut tally).await {
            tracing::error!(publisher = self.source, place, error = %e, "SenegalLII run stopped");
            tally.failed.push(SOURCE.to_string());
        }
        tally
    }

    async fn senlii(
        &self,
        client: &SenliiClient,
        place: &str,
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let mut listed = client.listing().await?;
        listed.retain(|l| l.place == place);
        listed.sort_by(|a, b| day(&b.path).cmp(day(&a.path)));
        let roles = roles(self.jurisdiction, &listed);
        let mut visited: Vec<(String, u64)> = Vec::new();
        let mut to_read = Vec::new();
        let (mut twice, mut unkeyed) = (0usize, 0usize);
        for l in &listed {
            let role = &roles[l.path.as_str()];
            let sum = checksum(l);
            if self
                .visits
                .get(&l.work)
                .is_some_and(|v| v.content_checksum == sum)
            {
                tally.unchanged += 1;
                continue;
            }
            match role {
                Role::Read { uid, cited } => to_read.push((l, uid.clone(), cited.clone(), sum)),
                Role::Unkeyed => {
                    tracing::warn!(
                        publisher = self.source,
                        work = l.work,
                        "work keyed by no uid"
                    );
                    unkeyed += 1;
                    visited.push((l.work.clone(), sum));
                }
                Role::Twice => {
                    twice += 1;
                    visited.push((l.work.clone(), sum));
                }
            }
        }
        if let Some(limit) = limit {
            to_read.truncate(limit);
        }
        tracing::info!(
            publisher = self.source,
            place,
            listed = listed.len(),
            to_read = to_read.len(),
            "SenegalLII works listed"
        );
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        let reader = match self.dry_run {
            true => None,
            false => match Ocr::from_pool(&repo).await? {
                Some(ocr) => Some((ocr, crate::config::Settings::from_env()?.bucket()?)),
                None => None,
            },
        };
        let mut scanned: Vec<Scanned> = Vec::new();
        let (mut native, mut akn, mut stopped, mut left) = (0usize, 0usize, false, 0usize);
        for (l, uid, cited, sum) in to_read {
            if self.spent() {
                left += 1;
                continue;
            }
            tally.listed += 1;
            let read = async {
                let listed = client.page(&l.path).await?;
                let page = match later(&listed) {
                    Some(path) => client.page(path).await?,
                    None => listed,
                };
                let Some(pdf) = &page.pdf else {
                    let articles = self.senlii_expressions(client, &uid, &page).await?;
                    return anyhow::Ok((page, Some(articles), 0));
                };
                let bytes = client.pdf(pdf).await?;
                let pages =
                    lj_sources::pdf::page_count(&bytes).map_err(|e| anyhow!("{pdf}: {e}"))?;
                let text = lj_sources::pdf::pdftotext_extract(&bytes)
                    .map_err(|e| anyhow!("{pdf}: {e}"))?;
                let chars: usize = text.split_whitespace().map(str::len).sum();
                if chars < NATIVE_CHARS_PER_PAGE * pages.max(1) as usize {
                    return Ok((page, None, pages));
                }
                Ok((page, Some(text_articles(&text)), pages))
            };
            let (page, articles) = match read.await {
                Ok((page, Some(articles), _)) => (page, articles),
                Ok((page, None, pages)) => {
                    let kind = kind(&page.title, &uid);
                    scanned.push(Scanned {
                        work: l.work.clone(),
                        sum,
                        uid,
                        cited,
                        page,
                        pages,
                        kind,
                    });
                    continue;
                }
                Err(e) => {
                    tracing::error!(publisher = self.source, key = uid, error = %e, "followed text failed");
                    tally.failed.push(uid);
                    if refused(&e) {
                        stopped = true;
                        break;
                    }
                    continue;
                }
            };
            match page.pdf {
                Some(_) => native += 1,
                None => akn += 1,
            }
            let doc = self.senlii_doc(&uid, &page, articles);
            if self.dry_run {
                tracing::info!(
                    publisher = self.source,
                    key = uid,
                    title = doc.title,
                    pdf = page.pdf.is_some(),
                    expressions = page.expressions.len().max(1),
                    articles = doc.articles.len(),
                    versions = doc
                        .articles
                        .iter()
                        .map(|a| a.versions.as_ref().map_or(1, Vec::len))
                        .sum::<usize>(),
                    "SenegalLII text read"
                );
                continue;
            }
            if self
                .senlii_write(&repo, &doc, cited.as_deref(), tally)
                .await
            {
                visited.push((l.work.clone(), sum));
            }
            if visited.len() >= 100 {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        let mut by_kind: BTreeMap<String, (usize, u32)> = BTreeMap::new();
        for s in &scanned {
            let (works, pages) = by_kind.entry(s.kind.clone()).or_default();
            *works += 1;
            *pages += s.pages;
        }
        if !by_kind.is_empty() {
            tracing::info!(publisher = self.source, place, scans = ?by_kind, "SenegalLII scans, works and pages by kind");
        }
        scanned.sort_by_key(|s| ocr_rank(&s.kind).unwrap_or(u8::MAX));
        let (mut ocr_read, mut later, mut unread) = (0usize, 0usize, 0usize);
        for s in scanned {
            if ocr_rank(&s.kind).is_none() {
                later += 1;
                visited.push((s.work, s.sum));
                continue;
            }
            let Some((ocr, bucket)) = reader
                .as_ref()
                .filter(|(ocr, _)| !stopped && !ocr.spent() && !self.spent())
            else {
                unread += 1;
                continue;
            };
            let pdf = s.page.pdf.as_deref().expect("a scan is served as a PDF");
            let text = match self.senlii_ocr(client, ocr, bucket, pdf, &s.uid).await {
                Ok(text) => text,
                Err(e) => {
                    tracing::error!(publisher = self.source, key = s.uid, error = %e, "the OCR of a scan failed");
                    stopped |= refused(&e);
                    tally.failed.push(s.uid);
                    continue;
                }
            };
            ocr_read += 1;
            let doc = self.senlii_doc(&s.uid, &s.page, text_articles(&text));
            if self
                .senlii_write(&repo, &doc, s.cited.as_deref(), tally)
                .await
            {
                visited.push((s.work, s.sum));
            }
            if visited.len() >= 100 {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        if let Some((ocr, _)) = &reader {
            ocr.park_spent(&repo).await?;
        }
        if !self.dry_run {
            repo.put_norm_visits(self.source, &visited)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        self.budget_spent(self.jurisdiction, left);
        tracing::info!(
            publisher = self.source,
            place,
            akn,
            native,
            ocr_read,
            "SenegalLII texts read"
        );
        if later + twice + unkeyed > 0 {
            tracing::warn!(
                publisher = self.source,
                place,
                later,
                twice,
                unkeyed,
                "works visited and not written"
            );
        }
        if unread > 0 {
            tracing::warn!(
                publisher = self.source,
                place,
                unread,
                "scans left unvisited for an OCR"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_is_keyed_by_the_number_it_is_cited_by() {
        let id = |j: &str, w: &str| identity(j, w).unwrap();
        assert_eq!(
            id("SN", "/akn/sn/act/1965/60"),
            ("SN/LOI-65-60".to_string(), Some("65-60".to_string()))
        );
        assert_eq!(
            id("SN", "/akn/sn/act/law/2016/32"),
            ("SN/LOI-2016-32".to_string(), Some("2016-32".to_string()))
        );
        assert_eq!(
            id("SN", "/akn/sn/act/decree/1996/96-154"),
            ("SN/DECRET-96-154".to_string(), Some("96-154".to_string()))
        );
        assert_eq!(
            id("SN", "/akn/sn/act/mo/2025/020586"),
            ("SN/ARRETE-2025-020586".to_string(), None)
        );
        assert_eq!(
            id("OHADA", "/akn/aa-ohada/act/2010/droit-commercial-général"),
            (
                "OHADA/ACTE-UNIFORME-2010-DROIT-COMMERCIAL-GENERAL".to_string(),
                None
            )
        );
        assert_eq!(
            id(
                "OHADA",
                "/akn/aa-ohada/act/2010/acte-uniforme-relatif-au-droit-des-sociétés-coopératives"
            )
            .0,
            "OHADA/ACTE-UNIFORME-2010-RELATIF-AU-DROIT-DES-SOCIETES-COOPERATIVES"
        );
        assert_eq!(
            id("OHADA", "/akn/aa-ohada/act/treaty/1993/ohada").0,
            "OHADA/TRAITE-1993-OHADA"
        );
    }

    #[test]
    fn the_ocr_reads_codes_first_then_laws_then_decrees_and_leaves_arretes() {
        let rank = |title: &str, uid: &str| ocr_rank(&kind(title, uid));
        assert_eq!(
            rank(
                "Loi Portant Code Pénal (Crimes et Délits), 1965",
                "SN/LOI-65-60"
            ),
            Some(0)
        );
        assert_eq!(
            rank(
                "Loi modifiant certaines dispositions du Code pénal",
                "SN/LOI-2020-05"
            ),
            Some(1)
        );
        assert_eq!(
            rank(
                "Décret portant organisation du ministère",
                "SN/DECRET-2023-1703"
            ),
            Some(2)
        );
        assert_eq!(
            rank(
                "Arrêté ministériel portant création",
                "SN/ARRETE-2025-020586"
            ),
            None
        );
    }

    fn art(num: &str, texte: &str) -> ConsolidatedArticle {
        ConsolidatedArticle {
            num: num.to_string(),
            title_path: None,
            texte: texte.to_string(),
            nota: None,
        }
    }

    #[test]
    fn an_ohada_act_comes_into_force_on_the_day_it_names_else_90_days_after_its_publication() {
        let named = [art("289", "Il entrera en vigueur le 1er janvier 1998.")];
        let silent = [art("1", "Le présent Acte uniforme s'applique.")];
        let audcg = "OHADA/ACTE-UNIFORME-1997-DROIT-COMMERCIAL-GENERAL";
        assert_eq!(in_force(audcg, "1997-04-17", &named), "1998-01-01");
        assert_eq!(in_force(audcg, "1999-03-01", &named), "1999-05-30");
        assert_eq!(in_force(audcg, "2011-02-15", &silent), "2011-05-16");
        let aupsrve = "OHADA/ACTE-UNIFORME-2023-ORGANISATION-DES-PROCEDURES-SIMPLIFIEES-DE-RECOUVREMENT-ET-DES-VOIES-D-EXECUTION";
        assert_eq!(in_force(aupsrve, "2024-07-02", &silent), "2024-02-16");
        assert_eq!(
            in_force("OHADA/TRAITE-1993-OHADA", "2008-10-17", &silent),
            "2008-10-17"
        );
        assert_eq!(in_force("SN/LOI-65-60", "1965-09-06", &named), "1965-09-06");
    }

    #[test]
    fn a_wording_holds_until_an_expression_changes_or_drops_it() {
        let expressions = vec![
            ("1992-12-22".to_string(), vec![art("1", "A"), art("2", "X")]),
            ("1998-07-26".to_string(), vec![art("1", "A"), art("2", "Y")]),
            (
                "2002-06-23".to_string(),
                vec![art("1", "B"), art("1-1", "N")],
            ),
        ];
        let windows: Vec<_> = articles(&expressions)
            .into_iter()
            .map(|a| {
                let versions: Vec<_> = a
                    .versions
                    .unwrap()
                    .into_iter()
                    .map(|v| (v.date_debut.unwrap(), v.date_fin, v.texte))
                    .collect();
                (a.num, versions)
            })
            .collect();
        let v = |s: &str, e: Option<&str>, t: &str| {
            (s.to_string(), e.map(str::to_string), t.to_string())
        };
        assert_eq!(
            windows,
            vec![
                (
                    "1".to_string(),
                    vec![
                        v("1992-12-22", Some("2002-06-23"), "A"),
                        v("2002-06-23", None, "B")
                    ]
                ),
                ("1-1".to_string(), vec![v("2002-06-23", None, "N")]),
                (
                    "2".to_string(),
                    vec![
                        v("1992-12-22", Some("1998-07-26"), "X"),
                        v("1998-07-26", Some("2002-06-23"), "Y")
                    ]
                ),
            ]
        );
    }
}
