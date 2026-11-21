//! What a journal printed, read as a notice: one page whatever the journal.

use std::collections::HashSet;

use lj_dtos::{LinkedTextRef, PublicationNotice, PublishedArticle, TextPublication};
use lj_store::repository::DecisionRepository;
use serde_json::Value;

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// The notice a uid `<journal>:<native id>` names, read from its journal's store.
pub async fn notice(state: &AppState, uid: &str) -> Result<PublicationNotice> {
    match uid.split_once(':').ok_or(ApiError::NotFound)? {
        ("raa", id) => raa(state, id.parse().map_err(|_| ApiError::NotFound)?).await,
        ("bodacc", reference) => crate::notices_gazette::bodacc(state, reference).await,
        ("joafe", id) => crate::notices_gazette::joafe(state, id).await,
        (journal, text_uid) => publication(state, journal, text_uid).await,
    }
}

/// A prefecture's recueil: one notice, the prefecture its issuer.
async fn raa(state: &AppState, id: i64) -> Result<PublicationNotice> {
    let conn = state.pool.get().await?;
    let row = DecisionRepository::new(&conn)
        .raa_notice(id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let journal = lj_norm::applicability::journal("RAA").expect("journals.tsv names the RAA");
    Ok(PublicationNotice {
        uid: format!("raa:{}", row.id),
        jurisdiction: "FR".to_string(),
        journal: journal.code.to_string(),
        journal_label: Some(journal.label.to_string()),
        language: "fr".to_string(),
        other_languages: Vec::new(),
        title: row.title,
        nature: None,
        nor: None,
        date_texte: None,
        date_publi: row.published.map(|d| d.to_string()),
        issuer: prefecture(&row.publisher),
        source_url: Some(row.url),
        publication: TextPublication::default(),
        articles: Vec::new(),
        body: row.body,
        norm: None,
        fields: Vec::new(),
        entity: None,
        // A recueil gathers acts naming their addressees, and is not cut act by act.
        individual: true,
    })
}

/// The administration an Attrap publisher id names: `pref67`, `prefidf`, `hcnc`.
pub(crate) fn prefecture(publisher: &str) -> Option<String> {
    let region = |name: &str| format!("Préfecture de la région {name}");
    Some(match publisher {
        "ppparis" => "Préfecture de police".to_string(),
        "prefidf" => "Préfecture de la région d'Île-de-France".to_string(),
        "prefara" => region("Auvergne-Rhône-Alpes"),
        "prefbfc" => region("Bourgogne-Franche-Comté"),
        "prefbretagne" => region("Bretagne"),
        "prefcvl" => region("Centre-Val de Loire"),
        "prefcorse" => "Préfecture de Corse".to_string(),
        "prefge" => region("Grand Est"),
        "prefhdf" => region("Hauts-de-France"),
        "prefnormandie" => region("Normandie"),
        "prefna" => region("Nouvelle-Aquitaine"),
        "prefoccitanie" => region("Occitanie"),
        "prefpl" => region("Pays de la Loire"),
        "prefpaca" => region("Provence-Alpes-Côte d'Azur"),
        "hcnc" => "Haut-commissariat de la République en Nouvelle-Calédonie".to_string(),
        "hcpf" => "Haut-commissariat de la République en Polynésie française".to_string(),
        other => {
            let code = other.strip_prefix("pref")?.to_ascii_uppercase();
            let name = lj_norm::referential_labels::departement_label(&code)?;
            let of = match code.as_str() {
                "01" | "02" | "03" | "07" | "09" | "10" | "11" | "12" | "27" | "34" | "36"
                | "38" | "60" | "61" | "89" | "91" => "de l'",
                "28" | "35" | "37" => "d'",
                "41" | "47" | "49" | "54" | "71" | "75" | "77" | "82" | "974" | "976" => "de ",
                "04" | "05" | "06" | "08" | "13" | "22" | "40" | "64" | "65" | "66" | "78"
                | "79" | "88" | "92" => "des ",
                "14" | "15" | "18" | "25" | "29" | "30" | "32" | "39" | "45" | "46" | "56"
                | "59" | "62" | "63" | "67" | "68" | "69" | "81" | "83" | "84" | "90" | "94"
                | "95" => "du ",
                _ => "de la ",
            };
            format!("Préfecture {of}{name}")
        }
    })
}

/// The publication `<journal>:<text_uid>` names.
async fn publication(state: &AppState, journal: &str, text_uid: &str) -> Result<PublicationNotice> {
    let conn = state.pool.get().await?;
    let row = DecisionRepository::new(&conn)
        .notice_publication(text_uid)
        .await?
        .filter(|r| r.journal_code.eq_ignore_ascii_case(journal))
        .ok_or(ApiError::NotFound)?;
    // CRPA R. 221-15 and R. 221-16: a protected act is read by its title and its issue only.
    let protected = lj_norm::individual::is_protected_act(&row.title);
    let individual = protected || lj_norm::individual::is_individual_measure(&row.title);
    let articles = match protected {
        true => Vec::new(),
        false => printed_articles(&row.structure, &row.sections, &row.articles),
    };
    let publication = crate::legi::publication_blocks(&row.blocks, row.issue);
    let publication = match protected {
        true => TextPublication {
            issue: publication.issue,
            ..TextPublication::default()
        },
        false => publication,
    };
    let source_url = row.url.clone().or_else(|| {
        text_uid
            .starts_with("JORFTEXT")
            .then(|| format!("https://www.legifrance.gouv.fr/jorf/id/{text_uid}"))
    });
    Ok(PublicationNotice {
        uid: format!("{}:{}", row.journal_code.to_lowercase(), row.text_uid),
        jurisdiction: lj_norm::applicability::journal_place(&row.journal_code).to_string(),
        other_languages: row
            .languages
            .iter()
            .filter(|l| **l != row.language)
            .cloned()
            .collect(),
        publication,
        norm: row
            .norm
            .filter(|_| !protected)
            .map(|(slug, title, jurisdiction)| LinkedTextRef {
                href: Some(lj_vocab::norm_path(&jurisdiction, &slug)),
                label: title,
                date: None,
            }),
        journal: row.journal_code,
        journal_label: row.journal_label,
        language: row.language,
        title: row.title,
        nature: row
            .nature
            .map(|n| lj_norm::referential_labels::nature_label(&n).map_or(n, str::to_string)),
        nor: row.nor,
        date_texte: row.date_texte.map(|d| d.to_string()),
        date_publi: row.date_publi.map(|d| d.to_string()),
        issuer: row.ministere.or(row.autorite),
        source_url,
        articles,
        body: row.body.filter(|_| !protected),
        fields: Vec::new(),
        entity: None,
        individual,
    })
}

/// `(id, num)` of the articles a structure places, sections walked in order.
fn placed(items: &Value, sections: &Value, out: &mut Vec<(String, Option<String>)>) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(id) = item["article"].as_str() {
            let num = item["num"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty());
            out.push((id.to_string(), num.map(str::to_string)));
        } else if let Some(id) = item["section"].as_str() {
            placed(&sections[id]["items"], sections, out);
        }
    }
}

/// The entries in the structure's order, then those it does not place in id order.
fn printed_articles(structure: &Value, sections: &Value, entries: &Value) -> Vec<PublishedArticle> {
    let Some(entries) = entries.as_object() else {
        return Vec::new();
    };
    let mut order = Vec::new();
    placed(structure, sections, &mut order);
    let mut seen = HashSet::new();
    order.retain(|(id, _)| seen.insert(id.clone()));
    let mut rest: Vec<&String> = entries.keys().filter(|id| !seen.contains(*id)).collect();
    rest.sort();
    order.extend(rest.into_iter().map(|id| (id.clone(), None)));
    order
        .into_iter()
        .filter_map(|(id, num)| {
            let entry = entries.get(&id)?;
            Some(PublishedArticle {
                num: num.or_else(|| entry["num"].as_str().map(str::to_string)),
                text: entry["text"].as_str()?.to_string(),
            })
        })
        .collect()
}

/// The page a slug of a text that left the norms answers with: its notice, or the decision
/// a court's ruling leads to.
pub async fn moved_text(state: &AppState, slug: &str) -> Result<Option<String>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    if let Some(uid) = repo.notice_of_slug(slug).await? {
        return Ok(Some(path(&uid)));
    }
    Ok(repo
        .decision_of_moved_slug(slug)
        .await?
        .and_then(|(jt, code, public_id)| {
            let jt = lj_dtos::JurisdictionType::from_code(&jt)?;
            Some(lj_dtos::decision_path(jt, code.as_deref(), &public_id))
        }))
}

/// The page of the notice a uid names, under the place of the journal its prefix names.
pub fn path(uid: &str) -> String {
    let journal = uid.split_once(':').map_or("", |(journal, _)| journal);
    let place = lj_norm::applicability::journal_place(&journal.to_ascii_uppercase());
    lj_vocab::notice_path(place, uid)
}
