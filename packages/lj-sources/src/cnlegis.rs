//! Madagascar's CNLEGIS, read through its search by type of text, which it holds in a session.

use std::time::Duration;

use lj_parse::{CnlegisPage, CnlegisText};

use crate::error::{Result, SourceError};
use crate::publisher::PacedClient;

const SITE: &str = "https://cnlegis.gov.mg";
/// A thousand records a request: 9.9 s and 27 MB for the laws on 19/11/2026, ten times fewer
/// requests than pages of a hundred.
const PAGE: usize = 1000;
/// Two seconds apart, CNLEGIS answered 403 after about 700 requests.
const PACE: Duration = Duration::from_secs(10);

pub struct CnlegisClient {
    paced: PacedClient,
}

impl CnlegisClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::with_cookies(proxy, "cnlegis.gov.mg")?.paced(PACE),
        })
    }

    async fn search(&self, kind: &str) -> Result<()> {
        let url = format!("{SITE}/page_find_type_texte/");
        self.paced
            .send(|| {
                self.paced.http.post(&url).form(&[
                    ("input_type[]", kind),
                    ("val_selected_type", kind),
                    ("txt_recherche", ""),
                ])
            })
            .await?;
        Ok(())
    }

    async fn page(&self, offset: usize, size: usize) -> Result<CnlegisPage> {
        let url = format!("{SITE}/page_type_data_find_direct");
        let limit = format!("{offset},{size}");
        let body = self
            .paced
            .send(|| {
                self.paced
                    .http
                    .post(&url)
                    .form(&[("var_pg_limit", limit.as_str())])
            })
            .await?;
        let json = String::from_utf8(body)
            .map_err(|e| SourceError::Invalid(format!("{url} is not UTF-8: {e}")))?;
        lj_parse::cnlegis_page(&json).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// The texts filed under `kind`, newest first, `most` of them if given.
    pub async fn listing(&self, kind: &str, most: Option<usize>) -> Result<Vec<CnlegisText>> {
        self.search(kind).await?;
        let mut out: Vec<CnlegisText> = Vec::new();
        while most.is_none_or(|m| out.len() < m) {
            let size = most.map_or(PAGE, |m| PAGE.min(m - out.len()));
            let mut page = self.page(out.len(), size).await?;
            if page.searched != kind {
                self.search(kind).await?;
                page = self.page(out.len(), size).await?;
                if page.searched != kind {
                    return Err(SourceError::Invalid(format!(
                        "cnlegis.gov.mg searches {:?} instead of {kind:?}",
                        page.searched
                    )));
                }
            }
            let done = page.texts.len() < size;
            out.extend(page.texts);
            if done {
                break;
            }
        }
        Ok(out)
    }

    /// The home page's status, one request: a 200 after a refused file says the refusal is
    /// that file's.
    pub async fn home(&self) -> Result<u16> {
        self.paced.status(SITE).await
    }

    pub async fn pdf(&self, url: &str) -> Result<Vec<u8>> {
        if let Some(first) = first_copy(url) {
            match self.paced.send(|| self.paced.http.get(&first)).await {
                Ok(bytes) if bytes.starts_with(b"%PDF") => return Ok(bytes),
                _ => {}
            }
        }
        self.paced.send(|| self.paced.http.get(url)).await
    }
}

/// CNLEGIS answers 403 for a copy's number, ` (1)`, and serves the same act under the name
/// without it: four files of four on 21/11/2026.
fn first_copy(url: &str) -> Option<String> {
    let (stem, n) = url.strip_suffix(").pdf")?.rsplit_once("%20(")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| format!("{stem}.pdf"))
}

#[cfg(test)]
mod tests {
    use super::first_copy;

    #[test]
    fn a_numbered_copy_is_read_under_the_name_without_its_number() {
        let at = |name: &str| first_copy(&format!("https://cnlegis.gov.mg/uploads/{name}"));
        assert_eq!(
            at("D2021-786(VF)%20(1).pdf").as_deref(),
            Some("https://cnlegis.gov.mg/uploads/D2021-786(VF).pdf")
        );
        assert_eq!(
            at("D2022-586%20(VF)%20(1).pdf").as_deref(),
            Some("https://cnlegis.gov.mg/uploads/D2022-586%20(VF).pdf")
        );
        assert_eq!(at("D2022-586%20(VF).pdf"), None);
        assert_eq!(at("D2022-586%20().pdf"), None);
    }
}
