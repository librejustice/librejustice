//! Service-Public.fr « vos droits » sheets into `legal_text` `FICHE_PRATIQUE`, and
//! one outgoing `cite` `legal_link` per Légifrance reference of the sheet.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use lj_norm::article_key::identity_key;
use lj_norm::article_order::num_key_sort_key;
use lj_norm::text_identity::{legifrance_id, IdKind};
use lj_parse::{ArticleSpan, ServicePublicSheet, SheetReference};
use lj_store::repository::{
    DecisionRepository, LegalLinkOwner, LegalLinkRow, LegalTextRow, SectionArticle, SectionArticles,
};

use crate::config::Settings;

const NATURE: &str = "FICHE_PRATIQUE";

/// Downloads the feeds, writes every sheet and its edges, turns `ABROGE` the
/// sheets gone from the feeds, then slugs the new ones.
pub async fn sync_service_public() -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.cache_dir().join("service-public");
    let (sheets, errors) = tokio::task::spawn_blocking(move || -> Result<_> {
        let zips = lj_sources::service_public::fetch_service_public(&dir)
            .map_err(|e| anyhow!("fetch service-public: {e}"))?;
        let mut seen = HashSet::new();
        let (mut sheets, mut errors) = (Vec::new(), 0);
        for zip in &zips {
            let (read, failed) = lj_sources::service_public::read_sheets(zip)
                .map_err(|e| anyhow!("read {}: {e}", zip.display()))?;
            errors += failed;
            sheets.extend(read.into_iter().filter(|s| seen.insert(s.id.clone())));
        }
        Ok((sheets, errors))
    })
    .await
    .map_err(|e| anyhow!("service-public read task: {e}"))??;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    let article_ids: Vec<String> = sheets
        .iter()
        .flat_map(|s| &s.references)
        .filter_map(|r| match legifrance_id(&r.url) {
            Some((IdKind::Article, id)) => Some(id.to_string()),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let articles = repo
        .articles_by_source_uids(&article_ids)
        .await
        .map_err(|e| anyhow!("articles_by_source_uids: {e}"))?;
    let section_ids: Vec<String> = sheets
        .iter()
        .flat_map(|s| &s.references)
        .filter_map(|r| match legifrance_id(&r.url) {
            Some((IdKind::Section, id)) => Some(id.to_string()),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let sections = repo
        .section_articles(&section_ids, chrono::Utc::now().date_naive())
        .await
        .map_err(|e| anyhow!("section_articles: {e}"))?;

    let mut edges = Vec::with_capacity(sheets.len());
    for sheet in &sheets {
        let row = sheet_row(sheet)?;
        repo.upsert_legal_text(&row)
            .await
            .map_err(|e| anyhow!("upsert_legal_text {}: {e}", row.text_uid))?;
        edges.push((
            LegalLinkOwner {
                text_uid: row.text_uid,
                num_key: String::new(),
                date_debut: None,
            },
            sheet
                .references
                .iter()
                .flat_map(|r| reference_edges(r, &articles, &sections))
                .collect::<Vec<_>>(),
        ));
    }
    let resolved = edges
        .iter()
        .flat_map(|(_, links)| links)
        .filter(|l| l.target_kind == "article" && l.target_num_key.is_some())
        .count();
    let written = repo
        .replace_legal_links(&edges)
        .await
        .map_err(|e| anyhow!("replace_legal_links: {e}"))?;

    let kept: Vec<String> = edges.into_iter().map(|(o, _)| o.text_uid).collect();
    let retired = repo
        .retire_legal_texts_except(NATURE, &kept)
        .await
        .map_err(|e| anyhow!("retire_legal_texts_except: {e}"))?;
    if retired > 0 {
        tracing::warn!(retired, "service_public: sheets gone from the feeds");
    }
    let slugged = super::slugs::assign_text_slugs(&repo).await?;

    tracing::info!(
        sheets = kept.len(),
        errors,
        links = written,
        article_links = resolved,
        retired,
        slugged,
        "sync_service_public"
    );
    Ok(())
}

fn sheet_row(sheet: &ServicePublicSheet) -> Result<LegalTextRow> {
    let updated = chrono::NaiveDate::from_ymd_opt(
        sheet.updated.year().into(),
        sheet.updated.month() as u32,
        sheet.updated.day() as u32,
    )
    .ok_or_else(|| anyhow!("{}: date {}", sheet.id, sheet.updated))?;
    Ok(LegalTextRow {
        text_uid: format!("sp_{}", sheet.id),
        jurisdiction: "FR".to_string(),
        title: sheet.title.clone(),
        title_key: lj_extract::extract::normalize_instrument(&sheet.title),
        nature: NATURE.to_string(),
        last_modified: Some(updated),
        date_texte: None,
        date_publi: Some(updated),
        eli: None,
        nor: None,
        instrument_key: None,
        body: Some(sheet.body.clone()),
        status: Some("VIGUEUR".to_string()),
    })
}

/// The edges of one reference, none when its URL names no Légifrance id. A
/// section becomes the articles its title names; one naming none stays whole.
fn reference_edges(
    reference: &SheetReference,
    articles: &HashMap<String, (String, String, String)>,
    sections: &HashMap<String, SectionArticles>,
) -> Vec<LegalLinkRow> {
    let Some((kind, id)) = legifrance_id(&reference.url) else {
        return Vec::new();
    };
    if let (IdKind::Section, Some(section)) = (kind, sections.get(id)) {
        let named = named_articles(&reference.articles(), &section.articles);
        if !named.is_empty() {
            return named
                .into_iter()
                .map(|a| {
                    cite(
                        reference,
                        "article",
                        &a.uid,
                        Some(section.text_uid.clone()),
                        Some((a.num.clone(), a.num_key.clone())),
                    )
                })
                .collect();
        }
    }
    let edge = match kind {
        IdKind::Article => cite(
            reference,
            "article",
            id,
            articles.get(id).map(|(text_uid, _, _)| text_uid.clone()),
            articles
                .get(id)
                .map(|(_, num, num_key)| (num.clone(), num_key.clone())),
        ),
        // `/codes/section_lc/LEGITEXT…/LEGISCTA…` names the code before the section.
        IdKind::Section => cite(
            reference,
            "section",
            id,
            reference
                .url
                .split(id)
                .next()
                .and_then(legifrance_id)
                .filter(|(k, _)| *k == IdKind::Text)
                .map(|(_, t)| t.to_string()),
            None,
        ),
        IdKind::Text => cite(reference, "texte", id, Some(id.to_string()), None),
    };
    vec![edge]
}

fn cite(
    reference: &SheetReference,
    target_kind: &str,
    target_uid: &str,
    target_text_uid: Option<String>,
    num: Option<(String, String)>,
) -> LegalLinkRow {
    let (target_num, target_num_key) = num.unzip();
    LegalLinkRow {
        typelien: "CITATION".to_string(),
        verb: "cite".to_string(),
        direction: "outgoing".to_string(),
        target_kind: target_kind.to_string(),
        target_uid: Some(target_uid.to_string()),
        target_text_uid,
        target_num,
        target_num_key,
        target_nature: None,
        target_label: reference.title.clone(),
        target_date: None,
        target_nor: None,
    }
}

/// The section's articles the spans name, in reading order. A range bound the
/// section does not hold still bounds it, a single number must be held.
fn named_articles<'a>(
    spans: &[ArticleSpan],
    articles: &'a [SectionArticle],
) -> Vec<&'a SectionArticle> {
    let orders: Vec<_> = articles.iter().map(|a| order(&a.num_key)).collect();
    let mut kept = vec![false; articles.len()];
    for span in spans {
        match span {
            ArticleSpan::One(a) => {
                let a = order(a);
                for (k, o) in kept.iter_mut().zip(&orders) {
                    *k |= *o == a;
                }
            }
            ArticleSpan::Range(a, b) => {
                // « R424-1 à 424-6 »: the second bound takes the first's prefix.
                let b = match b.starts_with(|c: char| c.is_ascii_digit()) {
                    true => format!(
                        "{}{b}",
                        a.split(|c: char| c.is_ascii_digit())
                            .next()
                            .unwrap_or_default()
                    ),
                    false => b.clone(),
                };
                let (a, b) = (order(a), order(&b));
                let (low, high) = if a <= b { (a, b) } else { (b, a) };
                for (k, o) in kept.iter_mut().zip(&orders) {
                    *k |= low <= *o && *o <= high;
                }
            }
            ArticleSpan::From(a) => {
                let a = order(a);
                if let Some(i) = orders.iter().position(|o| *o == a) {
                    kept[i..].fill(true);
                }
            }
        }
    }
    articles
        .iter()
        .zip(kept)
        .filter_map(|(a, k)| k.then_some(a))
        .collect()
}

/// An article number's natural order, dashes dropped so that « 4A » meets « 4 A ».
fn order(num: &str) -> (u8, Vec<u64>, String) {
    let (rank, nums, key) = num_key_sort_key(&identity_key(num));
    (rank, nums, key.replace('-', ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(nums: &[&str]) -> Vec<SectionArticle> {
        nums.iter()
            .map(|n| SectionArticle {
                uid: format!("LEGIARTI-{n}"),
                num: n.to_string(),
                num_key: identity_key(n),
            })
            .collect()
    }

    fn named(title: &str, articles: &[SectionArticle]) -> Vec<String> {
        let reference = SheetReference {
            url: String::new(),
            title: title.into(),
            complement: None,
        };
        named_articles(&reference.articles(), articles)
            .into_iter()
            .map(|a| a.num.clone())
            .collect()
    }

    #[test]
    fn a_section_reference_keeps_the_articles_its_title_names_in_reading_order() {
        let chapter = section(&[
            "L1132-1",
            "L1132-2",
            "L1132-3",
            "L1132-3-1",
            "L1132-4",
            "L1133-1",
        ]);
        assert_eq!(
            named("Code du travail : articles L1132-1 à L1132-4", &chapter),
            ["L1132-1", "L1132-2", "L1132-3", "L1132-3-1", "L1132-4"]
        );
        assert_eq!(
            named("Code du travail : articles L1132-4 et L1132-2", &chapter),
            ["L1132-2", "L1132-4"]
        );
        assert_eq!(
            named("Code du travail : article L1132-4 et suivants", &chapter),
            ["L1132-4", "L1133-1"]
        );
        assert_eq!(
            named("Code du travail : articles L. 1132-3-1 à L1132-9", &chapter),
            ["L1132-3-1", "L1132-4"]
        );
        assert_eq!(
            named("Code du travail : articles L1134-1 à L1134-5", &chapter),
            Vec::<String>::new()
        );
        assert_eq!(
            named("Code du travail : article L1132-5", &chapter),
            Vec::<String>::new()
        );
        assert_eq!(named("Code du travail", &chapter), Vec::<String>::new());

        let asylum = section(&["R424-1", "R424-2", "R424-6", "R424-7"]);
        assert_eq!(
            named(
                "Code de l'entrée et du séjour des étrangers : article R424-1 à 424-6",
                &asylum
            ),
            ["R424-1", "R424-2", "R424-6"]
        );
        let taxes = section(&["4 A", "4 B", "8 quinquies", "9"]);
        assert_eq!(
            named(
                "Code général des impôts : articles 4A à 8 quinquies",
                &taxes
            ),
            ["4 A", "4 B", "8 quinquies"]
        );
    }
}
