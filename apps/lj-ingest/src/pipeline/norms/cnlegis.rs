//! Madagascar's texts as CNLEGIS files them: one version each from its Journal officiel's day,
//! its issue and ministry, its status, and the act its object amends.

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, Result};
use lj_parse::{CnlegisText, ConsolidatedArticle};
use lj_sources::cnlegis::CnlegisClient;
use lj_sources::error::SourceError;
use lj_sources::publisher::Refusal;
use lj_store::repository::{
    DecisionRepository, Edition, IssuePlacement, JournalIssueRow, PublicationRow,
};
use serde_json::json;

use super::super::legal_corpus::{load_doc, CorpusArticle, CorpusDoc, CorpusLink};
use super::{refused, Run, Tally};

pub(super) const SOURCE: &str = "cnlegis";
const JOURNAL: &str = "JO-MG";
const EDITION: Edition = Edition {
    language: "fr",
    status: "authentic",
};
/// The types a run lists, each newest first.
const KINDS: [&str; 13] = [
    "Constitution",
    "Loi constitutionnelle",
    "Loi organique",
    "Loi",
    "Ordonnance",
    "Décret",
    "Arrêté",
    "Circulaire",
    "Décision",
    "Instruction",
    "Note",
    "Délibération",
    "Déclaration",
];
/// A daily run lists the newest hundred of each type: on 18/11/2026 they reach back seven months
/// for the decrees, the busiest type.
const DAILY_TEXTS: usize = 100;
/// Fewer characters a page than this, and a PDF is a scan.
const NATIVE_CHARS_PER_PAGE: usize = 200;
/// A text whose content holds fewer characters than this and no article holds only its header.
const HEADER_ONLY: usize = 200;

/// The uid head and nature of a text CNLEGIS files under a type of norm, its number known.
fn followed(t: &CnlegisText) -> Option<(&'static str, &'static str)> {
    let upper = t.number.to_uppercase();
    if t.number.is_empty() || lj_parse::cnlegis_court(&t.number) || upper.contains("EXPOSE") {
        return None;
    }
    lj_parse::cnlegis_nature(&t.kind)
}

fn base_uid(code: &str, number: &str) -> String {
    format!("MG/{code}-{}", lj_parse::journal_number_key(number))
}

fn title_key(object: &str) -> String {
    lj_fold::fold(object)
        .trim_end_matches(['.', ' '])
        .to_string()
}

/// Each text's uid, `None` for a text entered twice: of a uid's texts, one with the same object
/// keeps its latest entry, and a different text under the same number takes the uid suffixed with
/// its own id.
fn identities(listed: &[&CnlegisText]) -> HashMap<u64, Option<String>> {
    let mut groups: BTreeMap<String, Vec<&CnlegisText>> = BTreeMap::new();
    for t in listed {
        if let Some((code, _)) = followed(t) {
            groups.entry(base_uid(code, &t.number)).or_default().push(t);
        }
    }
    let mut out = HashMap::new();
    for (base, mut texts) in groups {
        texts.sort_by_key(|t| t.id);
        let mut distinct: Vec<(String, Vec<u64>)> = Vec::new();
        for t in texts {
            let key = title_key(&t.object);
            match distinct.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ids)) => ids.push(t.id),
                None => distinct.push((key, vec![t.id])),
            }
        }
        for (i, (_, ids)) in distinct.into_iter().enumerate() {
            let latest = *ids.last().expect("an entry");
            for id in ids {
                let uid = (id == latest).then(|| match i {
                    0 => base.clone(),
                    _ => format!("{base}-{latest}"),
                });
                out.insert(id, uid);
            }
        }
    }
    out
}

fn checksum(t: &CnlegisText) -> u64 {
    xxhash_rust::xxh3::xxh3_64(
        format!(
            "{}|{}|{:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}",
            t.kind,
            t.number,
            t.day,
            t.object,
            t.status,
            t.notes,
            t.ministry,
            t.issue,
            t.issue_day,
            t.issue_page,
            t.html
        )
        .as_bytes(),
    )
}

fn day(d: Option<&str>) -> Option<chrono::NaiveDate> {
    d.and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// `Loi n° 2016-021 du 22 août 2016 sur les Pôles Anti-Corruption`: the object's opening
/// participle lowered, a proper noun kept.
fn title(t: &CnlegisText) -> String {
    let mut out = format!("{} n° {}", t.kind, t.number);
    if let Some(d) = &t.day {
        out.push_str(" du ");
        out.push_str(&lj_core::titles::format_fr_date(d));
    }
    let object = t.object.trim().trim_end_matches(['.', ' ']);
    if !object.is_empty() {
        let first = object.split_whitespace().next().unwrap_or_default();
        let folded = lj_fold::fold(first);
        let lowered = first.chars().skip(1).all(char::is_lowercase)
            && (folded.ends_with("ant")
                || matches!(
                    folded.as_str(),
                    "relatif" | "relative" | "relatifs" | "relatives" | "sur"
                ));
        out.push(' ');
        if lowered {
            let mut chars = object.chars();
            let head = chars.next().expect("a first word");
            out.extend(head.to_lowercase());
            out.push_str(chars.as_str());
        } else {
            out.push_str(object);
        }
    }
    out
}

fn issue_title(issue: &str, day: Option<&str>) -> String {
    match day {
        Some(d) => format!(
            "Journal officiel de la République de Madagascar n° {issue} du {}",
            lj_core::titles::format_fr_date(d)
        ),
        None => format!("Journal officiel de la République de Madagascar n° {issue}"),
    }
}

/// A file its site serves and qpdf does not open.
#[derive(Debug)]
struct Unreadable(String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

/// What a text's content gives: its articles, else one piece, else nothing.
enum Content {
    Articles(Vec<ConsolidatedArticle>),
    Body(String),
    Empty,
}

fn content(articles: Vec<ConsolidatedArticle>, body: String) -> Content {
    match (articles.is_empty(), body.chars().count() < HEADER_ONLY) {
        (false, _) => Content::Articles(articles),
        (true, false) => Content::Body(body),
        (true, true) => Content::Empty,
    }
}

fn article(
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

/// What one text writes: its text and status, its publication, and its place in its issue.
struct Read {
    doc: CorpusDoc,
    nature: &'static str,
    gone: Option<&'static str>,
    publication: PublicationRow,
    facts: serde_json::Value,
    issue: Option<(JournalIssueRow, IssuePlacement)>,
}

fn read(t: &CnlegisText, uid: &str, nature: &'static str, content: Content) -> Option<Read> {
    let articles: Vec<CorpusArticle> = match content {
        Content::Articles(a) => a
            .into_iter()
            .map(|a| article(a.num, None, &a.texte, a.title_path))
            .collect(),
        Content::Body(body) => vec![article(
            String::new(),
            Some("body".to_string()),
            &body,
            None,
        )],
        Content::Empty => return None,
    };
    let title = title(t);
    let links = lj_parse::ejo_effects(&t.object)
        .into_iter()
        .map(|e| CorpusLink {
            verb: e.verb.to_string(),
            direction: "outgoing".to_string(),
            target_text_uid: base_uid(e.nature.code(), &e.reference),
            target_label: e.label,
            date: None,
        })
        .filter(|l| l.target_text_uid != uid)
        .collect();
    let published = t.issue_day.clone().or_else(|| t.day.clone());
    let url = t
        .pdf
        .clone()
        .unwrap_or_else(|| "https://cnlegis.gov.mg/page_lois_reglements_cnL/".to_string());
    let doc = CorpusDoc {
        text_uid: uid.to_string(),
        source: SOURCE.to_string(),
        jurisdiction: "MG".to_string(),
        title: title.clone(),
        title_key: None,
        nature: nature.to_string(),
        translation: "officiel".to_string(),
        source_url: Some(url.clone()),
        source_asof: None,
        source_upstream_url: None,
        date_texte: t.day.clone(),
        effect_date: published.clone(),
        detect_abrogation: false,
        num_prefix_agnostic: false,
        articles,
        links,
    };
    let publication = PublicationRow {
        text_uid: uid.to_string(),
        journal_code: JOURNAL.to_string(),
        nature: Some(nature.to_string()),
        nor: None,
        title: title.clone(),
        date_texte: day(t.day.as_deref()),
        date_publi: day(published.as_deref()),
        url,
        body: None,
    };
    let mut blocks = serde_json::Map::new();
    if !t.notes.is_empty() {
        blocks.insert("notes".to_string(), json!(t.notes.join("\n")));
    }
    let facts = json!({
        "text_uid": uid,
        "date_publi": published,
        "num_parution": t.issue,
        "page_deb": t.issue_page,
        "ministere": t.ministry,
        "title": title,
        "keywords": [],
        "blocks": blocks,
    });
    let issue = t.issue.as_ref().map(|number| {
        let issue_uid = format!("{JOURNAL}/{number}");
        (
            JournalIssueRow {
                issue_uid: issue_uid.clone(),
                journal_code: JOURNAL.to_string(),
                num: Some(number.clone()),
                date_publi: day(t.issue_day.as_deref()),
                eli: None,
                title: issue_title(number, t.issue_day.as_deref()),
                toc: json!([]),
            },
            IssuePlacement {
                text_uid: uid.to_string(),
                issue_uid,
                issue_rank: t.issue_page.unwrap_or(0),
            },
        )
    });
    let gone = match t.status.as_str() {
        "Abrogé" | "Abrogée" => Some("ABROGE"),
        "Périmé" | "Périmée" => Some("PERIME"),
        _ => None,
    };
    Some(Read {
        doc,
        nature,
        gone,
        publication,
        facts,
        issue,
    })
}

/// What a run met besides the texts it wrote.
#[derive(Default)]
struct Skipped {
    empty: usize,
    scans: usize,
    scan_pages: u32,
}

impl Run<'_> {
    /// Lists each type, then reads the texts never visited and those changed since, or the `ids`
    /// asked alone, whatever their visit.
    pub(super) async fn cnlegis_lane(
        &self,
        client: &CnlegisClient,
        ids: &[u64],
        limit: Option<usize>,
    ) -> Tally {
        let mut tally = Tally::default();
        if let Err(e) = self.cnlegis(client, ids, limit, &mut tally).await {
            tracing::error!(publisher = self.source, error = %e, "CNLEGIS run stopped");
            tally.failed.push("cnlegis".to_string());
        }
        tally
    }

    async fn cnlegis(
        &self,
        client: &CnlegisClient,
        ids: &[u64],
        limit: Option<usize>,
        tally: &mut Tally,
    ) -> Result<()> {
        let most = match (self.relist_before, self.visits.is_empty()) {
            _ if !ids.is_empty() => None,
            (Some(_), _) => None,
            (None, false) => Some(DAILY_TEXTS),
            (None, true) => limit,
        };
        let mut listed: Vec<CnlegisText> = Vec::new();
        for kind in KINDS {
            let texts = client.listing(kind, most).await?;
            tracing::info!(
                publisher = self.source,
                kind,
                listed = texts.len(),
                "CNLEGIS type listed"
            );
            listed.extend(texts);
        }
        let kept: Vec<&CnlegisText> = listed.iter().filter(|t| followed(t).is_some()).collect();
        let identities = identities(&kept);
        let mut twice: Vec<(String, u64)> = Vec::new();
        let mut to_read: Vec<(&CnlegisText, String)> = Vec::new();
        for t in &kept {
            let item = t.id.to_string();
            let sum = checksum(t);
            let skipped = if ids.is_empty() {
                self.visits
                    .get(&item)
                    .is_some_and(|v| v.content_checksum == sum)
            } else {
                !ids.contains(&t.id)
            };
            if skipped {
                tally.unchanged += 1;
                continue;
            }
            match &identities[&t.id] {
                None => twice.push((item, sum)),
                Some(uid) => to_read.push((t, uid.clone())),
            }
        }
        if let Some(limit) = limit {
            to_read.truncate(limit);
        }
        tracing::info!(
            publisher = self.source,
            listed = listed.len(),
            kept = kept.len(),
            to_read = to_read.len(),
            twice = twice.len(),
            "CNLEGIS texts listed"
        );
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| anyhow!("pool.get: {e}"))?;
        let repo = DecisionRepository::new(&conn);
        if !self.dry_run && !twice.is_empty() {
            repo.put_norm_visits(self.source, &twice)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        let mut skipped = Skipped::default();
        let mut visited: Vec<(String, u64)> = Vec::new();
        let mut left = 0usize;
        for (t, uid) in to_read {
            if self.left_for_later(&t.id.to_string()) {
                left += 1;
                continue;
            }
            tally.listed += 1;
            let (_, nature) = followed(t).expect("a followed text");
            let content = match self.cnlegis_content(client, t, &mut skipped).await {
                Ok(c) => c,
                Err(e) => {
                    let home = match e.downcast_ref() {
                        Some(
                            SourceError::Refused {
                                refusal: Refusal::Status(403),
                                ..
                            }
                            | SourceError::Invalid(_),
                        ) => Some(client.home().await),
                        _ => None,
                    };
                    let answers = matches!(home, Some(Ok(200)));
                    if answers && refused(&e) {
                        tracing::warn!(publisher = self.source, key = uid, id = t.id, error = %e, "file refused while the site answers, visited and not written");
                        visited.push((t.id.to_string(), checksum(t)));
                        continue;
                    }
                    if answers || e.is::<Unreadable>() {
                        tracing::error!(publisher = self.source, key = uid, id = t.id, url = t.pdf.as_deref(), error = %e, "file unreadable, visited and not written");
                        tally.failed.push(uid);
                        visited.push((t.id.to_string(), checksum(t)));
                        continue;
                    }
                    tracing::error!(publisher = self.source, key = uid, id = t.id, error = %e, ?home, "followed text failed");
                    tally.failed.push(uid);
                    if refused(&e) {
                        break;
                    }
                    continue;
                }
            };
            let Some(r) = read(t, &uid, nature, content) else {
                visited.push((t.id.to_string(), checksum(t)));
                continue;
            };
            if self.dry_run {
                continue;
            }
            match self.cnlegis_write(&repo, &r, tally).await {
                Ok(()) => visited.push((t.id.to_string(), checksum(t))),
                Err(e) => {
                    tracing::error!(publisher = self.source, key = uid, id = t.id, error = %e, "followed text failed");
                    tally.failed.push(uid);
                }
            }
            if visited.len() >= 100 {
                repo.put_norm_visits(self.source, &visited)
                    .await
                    .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
                visited.clear();
            }
        }
        if !self.dry_run {
            repo.put_norm_visits(self.source, &visited)
                .await
                .map_err(|e| anyhow!("put_norm_visits {}: {e}", self.source))?;
        }
        self.budget_spent(self.jurisdiction, left);
        if skipped.empty > 0 || skipped.scans > 0 {
            tracing::warn!(
                publisher = self.source,
                empty = skipped.empty,
                scans = skipped.scans,
                scan_pages = skipped.scan_pages,
                "texts without a text layer are visited and not written"
            );
        }
        Ok(())
    }

    /// A text's content: its HTML, else the PDF it links to when that PDF has a text layer.
    async fn cnlegis_content(
        &self,
        client: &CnlegisClient,
        t: &CnlegisText,
        skipped: &mut Skipped,
    ) -> Result<Content> {
        let html = content(
            lj_parse::journal_articles(&t.html),
            lj_parse::journal_body(&t.html),
        );
        let Some(url) = t
            .pdf
            .as_deref()
            .filter(|_| !matches!(html, Content::Articles(_)))
        else {
            if matches!(html, Content::Empty) {
                skipped.empty += 1;
            }
            return Ok(html);
        };
        let bytes = client.pdf(url).await?;
        let pages = lj_sources::pdf::page_count(&bytes).map_err(|e| match e {
            SourceError::Invalid(why) => anyhow::Error::new(Unreadable(why)),
            e => anyhow!("{url}: {e}"),
        })?;
        let text = lj_sources::pdf::pdftotext_extract(&bytes).map_err(|e| anyhow!("{url}: {e}"))?;
        let chars: usize = text.split_whitespace().map(str::len).sum();
        if chars < NATIVE_CHARS_PER_PAGE * pages.max(1) as usize {
            skipped.scans += 1;
            skipped.scan_pages += pages;
            return Ok(Content::Empty);
        }
        Ok(content(
            lj_parse::journal_text_articles(&text),
            lj_parse::journal_text_body(&text),
        ))
    }

    async fn cnlegis_write(
        &self,
        repo: &DecisionRepository<'_>,
        r: &Read,
        tally: &mut Tally,
    ) -> Result<()> {
        load_doc(
            repo,
            &r.doc,
            self.read_on,
            self.source,
            &mut tally.publie_targets,
        )
        .await?;
        if let Some(status) = r.gone {
            repo.set_legal_texts_status(r.nature, std::slice::from_ref(&r.doc.text_uid), status)
                .await
                .map_err(|e| anyhow!("set_legal_texts_status {}: {e}", r.doc.text_uid))?;
        }
        repo.upsert_publications(EDITION, std::slice::from_ref(&r.publication))
            .await
            .map_err(|e| anyhow!("upsert_publications {}: {e}", r.doc.text_uid))?;
        repo.upsert_publication_facts(EDITION, std::slice::from_ref(&r.facts))
            .await
            .map_err(|e| anyhow!("upsert_publication_facts {}: {e}", r.doc.text_uid))?;
        if let Some((issue, placement)) = &r.issue {
            repo.upsert_journal_issues(EDITION, std::slice::from_ref(issue))
                .await
                .map_err(|e| anyhow!("upsert_journal_issues {}: {e}", issue.issue_uid))?;
            repo.place_publications(EDITION, std::slice::from_ref(placement))
                .await
                .map_err(|e| anyhow!("place_publications {}: {e}", r.doc.text_uid))?;
        }
        tally.written += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(id: u64, kind: &str, number: &str, object: &str) -> CnlegisText {
        CnlegisText {
            id,
            kind: kind.to_string(),
            number: number.to_string(),
            day: Some("2016-08-22".to_string()),
            object: object.to_string(),
            status: "En vigueur".to_string(),
            notes: vec![],
            ministry: None,
            issue: None,
            issue_day: None,
            issue_page: None,
            html: String::new(),
            pdf: None,
        }
    }

    #[test]
    fn a_text_entered_twice_keeps_its_latest_entry_and_another_text_takes_its_id() {
        let texts = [
            text(
                37319,
                "Loi",
                "2003-035",
                "Autorisant la ratification de l'accord",
            ),
            text(
                37430,
                "Loi",
                "2003-035",
                "Autorisant la ratification de l'Accord.",
            ),
            text(
                47014,
                "Loi organique",
                "2019-006",
                "Portant loi organique modifiant",
            ),
            text(
                47096,
                "Loi",
                "2019-006",
                "Autorisant la ratification de l'Accord Prêt",
            ),
            text(58376, "Loi", "2026-009-EXPOSEDESMOTIFS", "Modifiant"),
            text(5, "Décision", "13-HCC/D3-27-07-2026", "Concernant la loi"),
        ];
        let ids = identities(&texts.iter().collect::<Vec<_>>());
        assert_eq!(ids[&37319], None);
        assert_eq!(ids[&37430].as_deref(), Some("MG/LOI-2003-035"));
        assert_eq!(ids[&47014].as_deref(), Some("MG/LOI-2019-006"));
        assert_eq!(ids[&47096].as_deref(), Some("MG/LOI-2019-006-47096"));
        assert!(!ids.contains_key(&58376));
        assert!(!ids.contains_key(&5));
    }

    #[test]
    fn a_title_reads_type_number_day_and_object() {
        assert_eq!(
            title(&text(
                1,
                "Loi",
                "2016-021",
                "Sur les Pôles Anti-Corruption."
            )),
            "Loi n° 2016-021 du 22 août 2016 sur les Pôles Anti-Corruption"
        );
        assert_eq!(
            title(&text(2, "Décret", "2002-234", "Portant création du Comité")),
            "Décret n° 2002-234 du 22 août 2016 portant création du Comité"
        );
        assert_eq!(
            title(&text(
                3,
                "Arrêté",
                "609/2002",
                "Madagascar Institut National"
            )),
            "Arrêté n° 609/2002 du 22 août 2016 Madagascar Institut National"
        );
    }
}
