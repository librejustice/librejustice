//! Source ADDE (`adde-association.org`) — API REST WordPress.
//!
//! L'ADDE (Avocats pour la défense des droits des étrangers) publie des
//! analyses de décisions dans la catégorie *jurisprudence* (id 48). Le titre
//! du post est la citation de la décision commentée ; on n'en tire que des
//! **liens sortants** (jamais le corps). Volume faible (dizaines de posts) :
//! un seul appel REST paginé, pas de cache disque.
//!
//! User-Agent identifiable (`LibreJusticeBot`) et débit poli entre pages.

use crate::error::{Result, SourceError};
use std::time::Duration;

/// Racine du site ADDE.
pub const BASE_URL: &str = "https://adde-association.org";
/// User-Agent identifiable (contact + finalité).
pub const USER_AGENT: &str = "LibreJusticeBot/0.1 (+https://librejustice.fr)";
/// Catégorie WordPress « Jurisprudence » (analyses de décisions).
pub const JURISPRUDENCE_CATEGORY: u32 = 48;
/// Débit poli entre pages REST.
pub const THROTTLE: Duration = Duration::from_millis(500);

/// Un post ADDE de la catégorie jurisprudence (métadonnées seules).
#[derive(Debug, Clone, PartialEq)]
pub struct AddePost {
    /// Titre rendu (= citation de la décision commentée).
    pub title: String,
    /// URL publique de l'analyse.
    pub link: String,
    /// Date de publication ISO (`2026-02-03`).
    pub date: String,
}

/// Client HTTP async pour l'API REST WordPress d'ADDE.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("reqwest client build")
}

/// Énumère les posts de la catégorie jurisprudence via `/wp-json/wp/v2/posts`
/// (paginé `per_page=100`). S'arrête quand une page revient vide ou courte.
pub async fn fetch_jurisprudence_posts(client: &reqwest::Client) -> Result<Vec<AddePost>> {
    let mut posts = Vec::new();
    for page in 1..=20 {
        let url = format!(
            "{BASE_URL}/wp-json/wp/v2/posts?categories={JURISPRUDENCE_CATEGORY}\
             &per_page=100&page={page}&_fields=title,link,date"
        );
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| SourceError::Invalid(format!("requête adde {url}: {e}")))?;
        // WordPress renvoie 400 `rest_post_invalid_page_number` au-delà de la
        // dernière page — fin d'énumération, pas une erreur.
        if resp.status().as_u16() == 400 {
            break;
        }
        if resp.status().as_u16() != 200 {
            return Err(SourceError::Invalid(format!(
                "adde {url}: statut {}",
                resp.status()
            )));
        }
        let text = resp.text().await?;
        let items: Vec<serde_json::Value> = sonic_rs::from_str(&text)?;
        if items.is_empty() {
            break;
        }
        let n = items.len();
        for it in items {
            let (Some(title), Some(link), Some(date)) = (
                it["title"]["rendered"].as_str(),
                it["link"].as_str(),
                it["date"].as_str(),
            ) else {
                continue;
            };
            posts.push(AddePost {
                title: decode_entities(title),
                link: link.to_string(),
                // On ne garde que la date (`YYYY-MM-DD`), pas l'heure.
                date: date.split('T').next().unwrap_or(date).to_string(),
            });
        }
        if n < 100 {
            break;
        }
        tokio::time::sleep(THROTTLE).await;
    }
    Ok(posts)
}

/// Décode les quelques entités HTML numériques/nommées que WordPress insère
/// dans les titres rendus (apostrophes typographiques, tirets). Suffisant pour
/// les titres de citation — pas un décodeur HTML général.
fn decode_entities(s: &str) -> String {
    s.replace("&#8217;", "\u{2019}")
        .replace("&rsquo;", "\u{2019}")
        .replace("&#8211;", "\u{2013}")
        .replace("&#8230;", "\u{2026}")
        .replace("&amp;", "&")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_entities_basic() {
        assert_eq!(
            decode_entities("L&#8217;ADDE &amp; co"),
            "L\u{2019}ADDE & co"
        );
    }
}
