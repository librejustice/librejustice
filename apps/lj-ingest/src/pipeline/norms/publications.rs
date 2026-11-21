//! The journal publications of the acts a consolidation is read for: the RO acts of a Swiss
//! federal act, the Moniteur publication of a Belgian act Justel lists.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_sources::fedlex::CompilationAct;
use lj_store::repository::{Edition, PublicationRow};
use serde_json::{json, Value};

use super::super::legal_corpus::CorpusLink;

pub(super) const RO: Edition = Edition {
    language: "fr",
    status: "authentic",
};
const ERRATUM: [&str; 2] = ["6", "13"];

/// The visit an act's RO acts are kept under.
pub(super) fn compilation_item(key: &str) -> String {
    format!("{key}#ro")
}

fn day(d: Option<&str>) -> Result<Option<NaiveDate>> {
    d.map(|d| {
        NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d")
            .map_err(|e| anyhow!("day {d:?}: {e}"))
    })
    .transpose()
}

/// `RO 2021 747`, the volume and first page alone for an act of the old volumes.
fn citation(path: &str) -> String {
    let (year, number) = path.split_once('/').unwrap_or((path, ""));
    format!("RO {year} {}", number.split('_').next().unwrap_or(number))
}

/// What the act publishing a text, or changing it, is to it: `publie`, `rectifie` for errata alone,
/// else `modifie`, a partial repeal included, since a text-level `abroge` repeals the whole text.
fn verb(kinds: &[String]) -> &'static str {
    if kinds.iter().any(|k| k == "basic") {
        "publie"
    } else if kinds.iter().all(|k| ERRATUM.contains(&k.as_str())) {
        "rectifie"
    } else {
        "modifie"
    }
}

/// Each RO act's publication and facts, and the incoming link the text takes from it.
pub(super) fn compilation(
    acts: &[CompilationAct],
) -> Result<(Vec<PublicationRow>, Vec<Value>, Vec<CorpusLink>)> {
    let mut rows = Vec::with_capacity(acts.len());
    let mut facts = Vec::with_capacity(acts.len());
    let mut links = Vec::with_capacity(acts.len());
    for act in acts {
        let uid = format!("RO/{}", act.path);
        let citation = citation(&act.path);
        let title = act.title.clone().unwrap_or_else(|| citation.clone());
        let adopted = day(act.adopted.as_deref())?;
        let published = day(act.published.as_deref())?;
        rows.push(PublicationRow {
            text_uid: uid.clone(),
            journal_code: "RO".to_string(),
            nature: None,
            nor: None,
            title: title.clone(),
            date_texte: adopted,
            date_publi: published,
            url: format!("https://www.fedlex.admin.ch/eli/oc/{}/fr", act.path),
            body: None,
        });
        facts.push(json!({
            "text_uid": uid,
            "num_parution": citation,
            "date_publi": published,
            "eli": format!("https://fedlex.data.admin.ch/eli/oc/{}", act.path),
            "title": title,
            "keywords": [],
            "blocks": {},
        }));
        links.push(CorpusLink {
            verb: verb(&act.kinds).to_string(),
            direction: "incoming".to_string(),
            target_text_uid: uid,
            target_label: match &act.title {
                Some(t) => format!("{t}, {citation}"),
                None => citation,
            },
            date: adopted.or(published).map(|d| d.to_string()),
        });
    }
    Ok((rows, facts, links))
}

/// The Moniteur's edition a source reads: the Codex's Dutch one, else the French one, a
/// translation for the German-speaking Community's acts.
pub(super) fn moniteur_edition(journal: &str) -> Edition<'static> {
    match journal {
        "MB-VLG" => Edition {
            language: "nl",
            status: "authentic",
        },
        "MB-CDE" => Edition {
            language: "fr",
            status: "translation",
        },
        _ => Edition {
            language: "fr",
            status: "authentic",
        },
    }
}

/// The visit a Flemish or Walloon act's Moniteur publication is kept under.
pub(super) fn moniteur_item(key: &str) -> String {
    format!("{key}#mb")
}

/// The Moniteur publication of the Belgian act read as `text_uid`.
pub(super) fn moniteur(
    text_uid: &str,
    title: &str,
    nature: &str,
    url: &str,
    adopted: Option<&str>,
    published: Option<&str>,
) -> Result<PublicationRow> {
    Ok(PublicationRow {
        text_uid: text_uid.to_string(),
        journal_code: lj_norm::applicability::journal_code(text_uid, "BE"),
        nature: Some(nature.to_string()),
        nor: None,
        title: title.to_string(),
        date_texte: day(adopted)?,
        date_publi: day(published)?,
        url: url.to_string(),
        body: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(k: &[&str]) -> Vec<String> {
        k.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn an_ro_act_publishes_rectifies_or_modifies_and_never_repeals_the_text() {
        assert_eq!(verb(&kinds(&["basic"])), "publie");
        assert_eq!(verb(&kinds(&["1", "basic"])), "publie");
        assert_eq!(verb(&kinds(&["13", "6"])), "rectifie");
        assert_eq!(verb(&kinds(&["2"])), "modifie");
        assert_eq!(verb(&kinds(&["1", "6"])), "modifie");
        assert_eq!(citation("2021/747"), "RO 2021 747");
        assert_eq!(citation("24/233_245_233"), "RO 24 233");
    }
}
