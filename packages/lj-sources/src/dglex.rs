//! DGLex, the German-speaking Community's base of the texts in force on its territory: its search
//! by kind of act, a document's PDF.

use std::sync::LazyLock;

use lj_parse::DglexListed;
use regex::Regex;

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://ostbelgienlive.be";
const SEARCH: &str = "/desktopdefault.aspx/tabid-6055/";
const CONTROL: &str = "ctl00$ctl12$";

/// The kinds of act the search lists, by their value in its form.
pub const KINDS: [(&str, &str); 6] = [
    ("569", "DG - Dekrete"),
    ("570", "DG - Erlasse"),
    ("571", "Gesetze"),
    ("572", "Andere - Erlasse"),
    ("573", "RW - Dekrete"),
    ("577", "RW - Erlasse"),
];

static HIDDEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<input type="hidden" name="(__VIEWSTATE[A-Z]*)" id="[^"]*" value="([^"]*)""#)
        .unwrap()
});

pub struct DglexClient {
    paced: PacedClient,
}

impl DglexClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::with_cookies(proxy, "dglex")?,
        })
    }

    /// The form `page` posts back when `event` fires on a search for `kind`.
    fn form(page: &str, kind: &str, event: &str) -> Vec<(String, String)> {
        let mut form: Vec<(String, String)> = HIDDEN
            .captures_iter(page)
            .map(|c| {
                (
                    c[1].to_string(),
                    html_escape::decode_html_entities(&c[2]).into_owned(),
                )
            })
            .collect();
        form.push(("__EVENTTARGET".into(), format!("{CONTROL}{event}")));
        form.push(("__EVENTARGUMENT".into(), String::new()));
        form.push((format!("{CONTROL}ddlArt"), kind.into()));
        for field in [
            "txtSuchbegriff",
            "txtDokNr",
            "txtDatumvon",
            "txtDatumbis",
            "txtParlamentarier",
            "ddlHauptkategorien",
            "ddlUnterkategorien",
            "ddlLegislatur",
        ] {
            form.push((format!("{CONTROL}{field}"), String::new()));
        }
        form.push((format!("{CONTROL}searchMode"), "rbTitleSearchOnly".into()));
        form
    }

    async fn post(&self, form: &[(String, String)]) -> Result<String> {
        let url = format!("{SITE}{SEARCH}");
        let body = self
            .paced
            .send(|| self.paced.http.post(&url).form(form))
            .await?;
        String::from_utf8(body).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// Every document of one kind, page after page of the search.
    pub async fn listing(&self, kind: &str) -> Result<Vec<DglexListed>> {
        let blank = self.paced.text(&format!("{SITE}{SEARCH}")).await?;
        let mut page = self.post(&Self::form(&blank, kind, "lbtnsuchen")).await?;
        let mut documents = Vec::new();
        loop {
            let list = lj_parse::dglex_listing(&page)
                .map_err(|e| SourceError::Invalid(format!("dglex kind {kind}: {e}")))?;
            if list.documents.is_empty() || documents.len() + list.documents.len() > list.total {
                return Err(SourceError::Invalid(format!(
                    "dglex kind {kind}: page of {} after {} of {}",
                    list.documents.len(),
                    documents.len(),
                    list.total
                )));
            }
            documents.extend(list.documents);
            if documents.len() == list.total {
                return Ok(documents);
            }
            page = self.post(&Self::form(&page, kind, "lnkbtnnexttop")).await?;
        }
    }

    /// A document's PDF as `pdftohtml -xml` renders it, `None` when DGLex serves something else:
    /// the page saying it could not read its own file, a spreadsheet.
    pub async fn xml(&self, listed: &DglexListed) -> Result<Option<String>> {
        let url = format!(
            "{SITE}/addons/SharepointDokumentsuche/desktop/SharepointDokDetails.aspx?DokID={}&FileID={}",
            listed.document, listed.file
        );
        let pdf = self.paced.send(|| self.paced.http.get(&url)).await?;
        if !pdf.starts_with(b"%PDF") {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || crate::pdf::pdftohtml_xml_extract(&pdf))
            .await
            .map_err(|e| SourceError::Invalid(format!("{url}: pdftohtml: {e}")))?
            .map(Some)
    }
}
