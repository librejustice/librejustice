//! The catalogue of the Défenseur des droits (PMB, `juridique.defenseurdesdroits.fr`): the list of
//! its decisions newest first, a notice's fields, its files. Every request leaves through the egress
//! proxy, 30 seconds after the previous one, the delay its `robots.txt` asks.
//!
//! The cache holds `notices.jsonl`, one notice per line, the last line of an id winning, and the
//! files under `pdf/<explnum id>.pdf`, the OCR of a scan under `ocr/<explnum id>.md`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{Result, SourceError};
use crate::html_strip::decode_entities;

pub const BASE_URL: &str = "https://juridique.defenseurdesdroits.fr";
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
pub const PACE: Duration = Duration::from_secs(30);
const ATTEMPTS: u32 = 5;
/// The catalogue's view of the decisions, the former authorities' included.
const DECISIONS_VIEW: u32 = 9;
const PER_PAGE: usize = 200;

/// A notice of the catalogue: its fields by label, and the ids of its files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DddNotice {
    pub id: u64,
    pub fields: BTreeMap<String, String>,
    pub explnum: Vec<u64>,
}

impl DddNotice {
    pub fn is_decision(&self) -> bool {
        self.fields.get("Type de document").map(|t| t.trim()) == Some("Décisions")
    }
}

pub struct DddClient {
    http: reqwest::Client,
    last: Mutex<Option<Instant>>,
}

impl DddClient {
    pub fn new(proxy: &str) -> Result<Self> {
        if proxy.is_empty() {
            return Err(SourceError::Invalid(
                "the Défenseur des droits catalogue is read through LIBREJUSTICE_EGRESS_PROXY, which is empty".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .proxy(reqwest::Proxy::all(proxy)?)
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(240))
            .build()?;
        Ok(Self {
            http,
            last: Mutex::new(None),
        })
    }

    /// Sends `request` at the catalogue's pace, again after a network failure or a 5xx, up to
    /// [`ATTEMPTS`] times, each wait one pace longer.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut last = self.last.lock().await;
        for attempt in 1.. {
            if let Some(at) = *last {
                tokio::time::sleep((PACE * attempt).saturating_sub(at.elapsed())).await;
            }
            let sent = request
                .try_clone()
                .expect("a catalogue request has no streamed body")
                .send()
                .await;
            *last = Some(Instant::now());
            match sent.and_then(reqwest::Response::error_for_status) {
                Err(e)
                    if attempt < ATTEMPTS
                        && (e.is_connect()
                            || e.is_timeout()
                            || e.is_request()
                            || e.status().is_some_and(|s| s.is_server_error())) =>
                {
                    tracing::warn!(attempt, error = %e, "ddd: request failed, sent again");
                }
                sent => return Ok(sent?),
            }
        }
        unreachable!("the attempts loop returns")
    }

    async fn text(&self, request: reqwest::RequestBuilder) -> Result<String> {
        Ok(self.send(request).await?.text().await?)
    }

    /// The notice ids of the decisions list, newest first, over its first `pages` pages of 200;
    /// every page with `None`.
    pub async fn decision_ids(&self, pages: Option<usize>) -> Result<Vec<u64>> {
        self.text(self.http.get(format!(
            "{BASE_URL}/index.php?lvl=cmspage&pageid=12&opac_view={DECISIONS_VIEW}"
        )))
        .await?;
        let mut page = self
            .text(
                self.http
                    .post(format!(
                        "{BASE_URL}/index.php?lvl=more_results&autolevel1=1"
                    ))
                    .form(&[
                        ("user_query", "*"),
                        ("look_TITLE", "1"),
                        ("look_ALL", "1"),
                        ("look_AUTHPERSO_1#", "1"),
                        ("look_AUTHPERSO_2#", "1"),
                    ]),
            )
            .await?;
        let total = result_count(&page)
            .ok_or_else(|| SourceError::Invalid("the decisions list gives no count".into()))?;
        let last = total.div_ceil(PER_PAGE);
        let mut ids: Vec<u64> = Vec::new();
        for n in 1..=pages.map_or(last, |p| p.min(last)) {
            let mut form: Vec<(String, String)> = form_values(&page)
                .into_iter()
                .filter(|(k, _)| k != "page" && k != "nb_per_page_custom")
                .collect();
            form.push(("page".into(), n.to_string()));
            form.push(("nb_per_page_custom".into(), PER_PAGE.to_string()));
            page = self
                .text(
                    self.http
                        .post(format!("{BASE_URL}/index.php?lvl=more_results"))
                        .form(&form),
                )
                .await?;
            for id in notice_ids(&page) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        if pages.is_none() && ids.len() != total {
            return Err(SourceError::Invalid(format!(
                "the decisions list counts {total} and lists {}",
                ids.len()
            )));
        }
        Ok(ids)
    }

    /// The notice `id`, `None` when the catalogue has none under it.
    pub async fn notice(&self, id: u64) -> Result<Option<DddNotice>> {
        let page = self
            .text(
                self.http
                    .get(format!("{BASE_URL}/index.php?lvl=notice_display&id={id}")),
            )
            .await?;
        Ok(parse_notice(&page, id))
    }

    pub async fn file(&self, explnum: u64) -> Result<Vec<u8>> {
        let response = self
            .send(
                self.http
                    .get(format!("{BASE_URL}/doc_num.php?explnum_id={explnum}")),
            )
            .await?;
        Ok(response.bytes().await?.to_vec())
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn result_count(page: &str) -> Option<usize> {
    static COUNT: OnceLock<Regex> = OnceLock::new();
    let text = cell_text(page);
    let c = regex(&COUNT, r"(\d[\d ]*) titres? trouvés?").captures(&text)?;
    c[1].replace(' ', "").parse().ok()
}

fn form_values(page: &str) -> Vec<(String, String)> {
    static FORM: OnceLock<Regex> = OnceLock::new();
    static INPUT: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    static VALUE: OnceLock<Regex> = OnceLock::new();
    let Some(form) = regex(&FORM, r#"(?is)<form name="form_values".*?</form>"#).find(page) else {
        return Vec::new();
    };
    regex(&INPUT, r"(?is)<input[^>]*>")
        .find_iter(form.as_str())
        .filter_map(|tag| {
            let tag = tag.as_str();
            let name = regex(&NAME, r#"(?s)name=["']([^"']*)["']"#).captures(tag)?;
            let value = regex(&VALUE, r#"(?s)value=["']([^"']*)["']"#)
                .captures(tag)
                .map_or(String::new(), |v| decode_entities(&v[1]));
            Some((decode_entities(&name[1]), value))
        })
        .collect()
}

fn notice_ids(page: &str) -> Vec<u64> {
    static ID: OnceLock<Regex> = OnceLock::new();
    let mut ids: Vec<u64> = Vec::new();
    for c in regex(&ID, r"notice_display(?:&amp;|&)id=(\d+)").captures_iter(page) {
        let id = c[1].parse().unwrap();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The text of an HTML fragment, its pieces between tags joined by a space.
fn cell_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    regex(&TAG, r"(?s)<[^>]*>")
        .split(html)
        .map(|piece| {
            decode_entities(piece)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|piece| !piece.is_empty() && piece != ",")
        .collect::<Vec<_>>()
        .join(" ")
}

/// The fields of a notice page, by label, the first row of a label winning; `None` for a page
/// with no title, which the catalogue serves for an id it has no notice under.
pub fn parse_notice(page: &str, id: u64) -> Option<DddNotice> {
    static ROW: OnceLock<Regex> = OnceLock::new();
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static FILE: OnceLock<Regex> = OnceLock::new();
    let page = regex(&SCRIPT, r"(?is)<script.*?</script>|<style.*?</style>").replace_all(page, "");
    let mut fields = BTreeMap::new();
    for row in regex(
        &ROW,
        r#"(?s)<td class="labelNot">(.*?)</td>\s*<td class="labelContent">(.*?)</td>\s*</tr>"#,
    )
    .captures_iter(&page)
    {
        let label = cell_text(&row[1]).trim_end_matches(':').trim().to_string();
        if !label.is_empty() {
            fields.entry(label).or_insert_with(|| cell_text(&row[2]));
        }
    }
    fields.contains_key("Titre").then(|| {
        let mut explnum: Vec<u64> = regex(&FILE, r"doc_num\.php\?explnum_id=(\d+)")
            .captures_iter(&page)
            .map(|c| c[1].parse().unwrap())
            .collect();
        explnum.sort_unstable();
        explnum.dedup();
        DddNotice {
            id,
            fields,
            explnum,
        }
    })
}

pub fn read_notices(dir: &Path) -> Result<HashMap<u64, DddNotice>> {
    let path = dir.join("notices.jsonl");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let mut notices = HashMap::new();
    for line in fs::read_to_string(&path)?.lines() {
        let notice: DddNotice = serde_json::from_str(line)
            .map_err(|e| SourceError::Invalid(format!("{}: {e}", path.display())))?;
        notices.insert(notice.id, notice);
    }
    Ok(notices)
}

pub fn append_notice(dir: &Path, notice: &DddNotice) -> Result<()> {
    fs::create_dir_all(dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("notices.jsonl"))?;
    let line = serde_json::to_string(notice)
        .map_err(|e| SourceError::Invalid(format!("notice {}: {e}", notice.id)))?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn ocr_path(dir: &Path, explnum: u64) -> PathBuf {
    dir.join("ocr").join(format!("{explnum}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_page_gives_its_fields_and_files() {
        let page = r#"<table class="descr_notice"><tbody>
            <tr class="record_tit1"><td class="labelNot"><span>Titre : </span></td>
            <td class="labelContent"><span id="exempl_titleNot">Décision 2026-195 du 11 septembre 2026 &ndash; Accès à la préfecture</span></td></tr>
            <tr class="record_responsabilites"><td class="labelNot"><span>Auteurs : </span></td>
            <td class="labelContent"><span><a href="./index.php?lvl=author_see&amp;id=39" title="Défenseur des droits">Défenseur des droits</a>, Auteur ;
            <a href="./index.php?lvl=author_see&amp;id=6247">Droits fondamentaux des étrangers</a>, Auteur</span></td></tr>
            <tr class="record_tdoc"><td class="labelNot"><span>Type de document : </span></td>
            <td class="labelContent"><span>
                Décisions
            </span></td></tr>
            </tbody></table>
            <a href="./doc_num.php?explnum_id=23420">JP_2026_195</a>"#;
        let notice = parse_notice(page, 61013).unwrap();
        assert_eq!(
            notice.fields["Auteurs"],
            "Défenseur des droits , Auteur ; Droits fondamentaux des étrangers , Auteur"
        );
        assert!(notice.fields["Titre"].starts_with("Décision 2026-195 du 11 septembre 2026"));
        assert!(notice.is_decision());
        assert_eq!(notice.explnum, vec![23420]);
        assert!(parse_notice("<html><body>Aucune notice</body></html>", 1).is_none());
    }
}
