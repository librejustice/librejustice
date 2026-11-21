//! The Vaud systematic collection (BLV), from the API its publication site reads: the
//! consolidated acts in force, each act's versions, and each version's Akoma Ntoso rendering,
//! through the egress proxy.

use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::lexwork::Listing;
use crate::publisher::{Consolidation, PacedClient};

pub const HOST: &str = "prestations.vd.ch";
const SITE: &str = "https://prestations.vd.ch/pub/blv-publication/";

/// A consolidated act in force, as the collection's search lists it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VdAct {
    pub id: String,
    /// Its number in the collection, shared by an act and the acts that execute it.
    pub cote: String,
    /// Its title with its date and abbreviation, `LOI du 28.10.2008 sur la procédure
    /// administrative (LPA-VD)`.
    pub titre_complet_sans_cote: String,
    /// `MAJEUR` for an act of its own, `MINEUR` for an order of entry into force, a credit, a vote.
    pub importance: String,
    /// The day its current version came into force, `01.09.2025`.
    pub date_mise_en_vigueur_version: Option<String>,
}

impl VdAct {
    /// Its title with the leading words the collection capitalises in sentence case: `LOI du …`
    /// reads `Loi du …`, `ACCORD INTERCANTONAL du …` reads `Accord intercantonal du …`.
    pub fn title(&self) -> String {
        let mut leading = true;
        let words: Vec<String> = self
            .titre_complet_sans_cote
            .split_whitespace()
            .enumerate()
            .map(|(i, word)| {
                leading &=
                    word.chars().any(char::is_alphabetic) && !word.chars().any(char::is_lowercase);
                match (leading, i) {
                    (false, _) => word.to_string(),
                    (true, 0) => {
                        let lower = word.to_lowercase();
                        let mut chars = lower.chars();
                        chars
                            .next()
                            .map(|c| c.to_uppercase().chain(chars).collect())
                            .unwrap_or_default()
                    }
                    (true, _) => word.to_lowercase(),
                }
            })
            .collect();
        words.join(" ")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    html_id: String,
    version_date_mise_en_vigueur: Option<String>,
    version_type: String,
    date_adoption: Option<String>,
}

/// `28.10.2008` as `2008-10-28`.
fn iso_day(day: &str) -> Option<String> {
    let mut parts = day.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(d), Some(m), Some(y), None) if d.len() == 2 && m.len() == 2 && y.len() == 4 => {
            Some(format!("{y}-{m}-{d}"))
        }
        _ => None,
    }
}

pub struct VdClient {
    paced: PacedClient,
}

impl VdClient {
    pub fn new(proxy: &str) -> Result<Self> {
        Ok(Self {
            paced: PacedClient::new(proxy, HOST)?,
        })
    }

    /// The act's page on the collection's site.
    pub fn page_url(id: &str, cote: &str) -> String {
        format!("{SITE}actes/consolide/{cote}?id={id}")
    }

    /// The consolidated acts in force, major and minor, in one answer.
    pub async fn acts(&self) -> Result<Vec<VdAct>> {
        let url = format!(
            "{SITE}api/actes/rechercher?textes=ABREVIATION,COTE,TITRE&statuts=EN_VIGUEUR\
             &importances=MAJEUR,MINEUR&categories=CONSOLIDE&sortBy=COTE"
        );
        let body = self.paced.text(&url).await?;
        serde_json::from_str(&body).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))
    }

    /// The versions of the act `id` numbered `cote`, oldest first, the one the collection lists
    /// first when it keeps several of a day, each named by its rendering.
    pub async fn listing(&self, id: &str, cote: &str) -> Result<Listing> {
        let url = format!("{SITE}api/actes/CONSOLIDE?id={id}&cote={cote}");
        let body = self.paced.text(&url).await?;
        let versions: Vec<Version> =
            serde_json::from_str(&body).map_err(|e| SourceError::Invalid(format!("{url}: {e}")))?;
        let adopted = versions
            .first()
            .and_then(|v| v.date_adoption.as_deref())
            .and_then(iso_day);
        let mut consolidations: Vec<Consolidation> = Vec::new();
        for v in versions {
            if v.version_type == "FUTURE" && v.version_date_mise_en_vigueur.is_none() {
                continue;
            }
            let start = v
                .version_date_mise_en_vigueur
                .as_deref()
                .unwrap_or_default();
            let applies_from = iso_day(start).ok_or_else(|| {
                SourceError::Invalid(format!(
                    "{url}: version {} without a start day: {:?}",
                    v.html_id, v.version_date_mise_en_vigueur
                ))
            })?;
            if consolidations
                .iter()
                .any(|c| c.applies_from == applies_from)
            {
                continue;
            }
            let token = v
                .html_id
                .replace('-', "")
                .get(..16)
                .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                .or_else(|| v.html_id.parse().ok())
                .ok_or_else(|| SourceError::Invalid(format!("{url}: version id {}", v.html_id)))?;
            consolidations.push(Consolidation {
                xml_url: format!("{SITE}api/actes/{}/html", v.html_id),
                applies_from,
                modified: None,
                token: Some(token),
            });
        }
        consolidations.sort_by(|a, b| a.applies_from.cmp(&b.applies_from));
        Ok(Listing {
            adopted,
            consolidations,
            abbreviation: None,
        })
    }

    pub async fn html(&self, url: &str) -> Result<String> {
        self.paced.text(url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(title: &str) -> VdAct {
        VdAct {
            id: String::new(),
            cote: String::new(),
            titre_complet_sans_cote: title.to_string(),
            importance: String::new(),
            date_mise_en_vigueur_version: None,
        }
    }

    #[test]
    fn a_title_s_capitalised_leading_words_read_in_sentence_case() {
        assert_eq!(
            act("LOI du 28.10.2008 sur la procédure administrative (LPA-VD)").title(),
            "Loi du 28.10.2008 sur la procédure administrative (LPA-VD)"
        );
        assert_eq!(
            act("ACCORD INTERCANTONAL du 15.11.2019 sur les marchés publics (A-IMP)").title(),
            "Accord intercantonal du 15.11.2019 sur les marchés publics (A-IMP)"
        );
        assert_eq!(
            act("RÈGLEMENT d'application").title(),
            "Règlement d'application"
        );
    }
}
