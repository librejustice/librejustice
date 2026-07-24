//! Corps des fiches JORF sans articles (`TRAITE`, `AVIS`, `DECISION`,
//! `ACCORD_FONCTION_PUBLIQUE`) et accords interprofessionnels (`TI`, KALI)
//! depuis les blocs sans numéro des stocks bulk locaux (ADR 0223).
//!
//! Cibles : fiches sans `legal_article` ni corps réel (`empty_legal_text_uids`).
//! Le contenu est déjà sur le disque (stocks `Freemium_*_global` du cache) : la
//! passe streame les stocks, assemble chaque texte dans l'ordre du document et
//! pose `legal_text.body`. UPDATE ciblé, jamais de création (même contrat que
//! la passe circulaires, ADR 0222).
//!
//! - **JORF** : ordre = `STRUCT/LIEN_ART` du `texte/struct` (les ids `JORFARTI`
//!   ne suivent pas l'ordre de lecture) ; blocs = `BLOC_TEXTUEL` des articles
//!   du cid. Les labels (« Art. 1er. - », « A N N E X E I ») vivent dans le
//!   texte même. Un texte à STRUCT vide est pré-numérisation (contenu en
//!   fac-similé JO seulement) : compté `no_content`, track différé (ADR 0223 §4).
//! - **KALI** : ordre = `LIEN_TXT` du conteneur (texte de base puis attachés)
//!   puis id `KALIARTI` croissant — l'ordre exact vivrait dans les `KALISCTA`,
//!   une indirection de plus pour un gain nul (les ids d'un même texte suivent
//!   le document). Le titre de section (`TM`) est préposé à chaque changement :
//!   c'est le seul endroit où vit le libellé (« Préambule », « Chapitre Ier »…).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};

use lj_sources::dila::DilaFond;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Compteurs d'un fond : corps posés / cibles sans contenu dans le stock.
#[derive(Debug, Default, Clone, Copy)]
struct BodyCounts {
    updated: usize,
    no_content: usize,
}

/// Passe complète : cibles en base → stocks du cache → `legal_text.body`.
pub async fn backfill_treaty_bodies() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_store::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // TRAITE + les natures JORF dont le corps vit dans un article
    // `TYPE=AUTONOME` sans `NUM` que l'ingest saute (audit fiches nues du
    // 2026-07-19 : AVIS, DECISION, ACCORD_FONCTION_PUBLIQUE — 47 fiches,
    // toutes `JORFTEXT*`). Même assemblage : le parseur JORF rend tous les
    // blocs, numérotés ou non.
    let mut jorf_targets: HashSet<String> = HashSet::new();
    for nature in ["TRAITE", "AVIS", "DECISION", "ACCORD_FONCTION_PUBLIQUE"] {
        jorf_targets.extend(
            repo.empty_legal_text_uids(nature)
                .await
                .map_err(|e| anyhow!("empty_legal_text_uids {nature}: {e}"))?,
        );
    }
    let kali_targets: HashSet<String> = repo
        .empty_legal_text_uids("TI")
        .await
        .map_err(|e| anyhow!("empty_legal_text_uids TI: {e}"))?
        .into_iter()
        .collect();

    let cache = settings.cache_dir();
    let jorf = backfill_jorf(&repo, &latest_stock(&cache, DilaFond::Jorf)?, &jorf_targets).await?;
    let kali = backfill_kali(&repo, &latest_stock(&cache, DilaFond::Kali)?, &kali_targets).await?;

    tracing::info!(
        jorf_targets = jorf_targets.len(),
        jorf_updated = jorf.updated,
        jorf_no_content = jorf.no_content,
        ti_targets = kali_targets.len(),
        ti_updated = kali.updated,
        ti_no_content = kali.no_content,
        "backfill_treaty_bodies"
    );
    Ok(())
}

/// Dernier stock global du fond dans le cache
/// (`Freemium_<fond>_global_*.tar.gz`). Erreur franche si absent : la passe
/// suppose les stocks déjà téléchargés (bootstrap des fonds JORF/KALI).
fn latest_stock(cache_dir: &Path, fond: DilaFond) -> Result<PathBuf> {
    let dir = lj_sources::dila::tarballs_dir(cache_dir, fond);
    let prefix = format!("Freemium_{}_global_", fond.stock_infix());
    let mut stocks: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!("lecture {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".tar.gz"))
        })
        .collect();
    stocks.sort();
    stocks
        .pop()
        .ok_or_else(|| anyhow!("aucun stock {prefix}*.tar.gz sous {}", dir.display()))
}

/// Valeur du premier attribut `pat` (`… pat="valeur"`) d'un payload XML brut —
/// pré-filtre d'appartenance avant le parse complet (le stock JORF a des
/// millions d'articles, on ne bâtit l'arbre XML que pour les cibles).
fn attr_value<'a>(raw: &'a [u8], pat: &[u8]) -> Option<&'a str> {
    let start = raw.windows(pat.len()).position(|w| w == pat)? + pat.len();
    let len = raw[start..].iter().position(|&b| b == b'"')?;
    std::str::from_utf8(&raw[start..start + len]).ok()
}

/// Moisson JORF : ordres de lecture (`texte/struct`) et blocs (`article`) des
/// cibles, en un seul stream du stock.
#[derive(Default)]
struct JorfHarvest {
    /// text_uid → ids `JORFARTI` dans l'ordre du document.
    orders: HashMap<String, Vec<String>>,
    /// text_uid → blocs `(jorfarti, texte)` (ordre du tar, réordonnés ensuite).
    blocks: HashMap<String, Vec<(String, String)>>,
}

async fn backfill_jorf(
    repo: &DecisionRepository<'_>,
    stock: &Path,
    targets: &HashSet<String>,
) -> Result<BodyCounts> {
    if targets.is_empty() {
        return Ok(BodyCounts::default());
    }
    let shared = Arc::new(targets.clone());
    let stock_path = stock.to_path_buf();
    let harvest = tokio::task::spawn_blocking(move || -> Result<JorfHarvest> {
        let mut h = JorfHarvest::default();
        lj_sources::tar_reader::for_each_member(&stock_path, |name, raw| {
            let stem = name.rsplit('/').next().unwrap_or(&name);
            if name.contains("/texte/struct/") && stem.starts_with("JORFTEXT") {
                if !shared.contains(stem.trim_end_matches(".xml")) {
                    return Ok(());
                }
                match lj_extract::jorf::parse_jorf_struct(&raw) {
                    Ok(s) => {
                        h.orders.insert(s.jorftext, s.article_ids);
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "jorf struct: parse échec");
                    }
                }
            } else if name.contains("/article/") && stem.starts_with("JORFARTI") {
                // Pré-filtre sur le cid du CONTEXTE (premier `<TEXTE cid=`).
                let cid = attr_value(&raw, b"<TEXTE cid=\"");
                if !cid.is_some_and(|c| shared.contains(c)) {
                    return Ok(());
                }
                match lj_extract::jorf::parse_jorf_article(&raw) {
                    Ok(a) => {
                        if let Some(t) = a.texte {
                            h.blocks
                                .entry(a.jorftext)
                                .or_default()
                                .push((a.jorfarti, t));
                        }
                    }
                    Err(e) => {
                        tracing::error!(member = %name, error = %e, "jorf article: parse échec");
                    }
                }
            }
            Ok(())
        })?;
        Ok(h)
    })
    .await
    .map_err(|e| anyhow!("tâche lecture stock JORF: {e}"))??;

    let mut counts = BodyCounts::default();
    for uid in targets {
        let Some(blocks) = harvest.blocks.get(uid) else {
            counts.no_content += 1;
            continue;
        };
        let body = assemble_ordered(harvest.orders.get(uid).map(Vec::as_slice), blocks);
        if body.is_empty() {
            counts.no_content += 1;
            continue;
        }
        if repo
            .set_legal_text_body(uid, &body)
            .await
            .map_err(|e| anyhow!("set_legal_text_body {uid}: {e}"))?
        {
            counts.updated += 1;
        }
    }
    Ok(counts)
}

/// Concatène les blocs dans l'ordre du `struct` ; les blocs hors struct suivent,
/// par id croissant (stable).
fn assemble_ordered(order: Option<&[String]>, blocks: &[(String, String)]) -> String {
    let by_id: HashMap<&str, &str> = blocks
        .iter()
        .map(|(id, t)| (id.as_str(), t.as_str()))
        .collect();
    let mut used: HashSet<&str> = HashSet::new();
    let mut parts: Vec<&str> = Vec::new();
    for id in order.unwrap_or_default() {
        if let Some(t) = by_id.get(id.as_str()) {
            if used.insert(id) {
                parts.push(t);
            }
        }
    }
    let mut rest: Vec<&(String, String)> = blocks
        .iter()
        .filter(|(id, _)| !used.contains(id.as_str()))
        .collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    parts.extend(rest.iter().map(|(_, t)| t.as_str()));
    parts.join("\n\n")
}

/// Un bloc TI : rang du texte porteur dans le sommaire du conteneur, id
/// d'article (l'ordre de tri), titre de section, texte.
struct TiBlock {
    rank: usize,
    kaliarti: String,
    titre: Option<String>,
    texte: String,
}

async fn backfill_kali(
    repo: &DecisionRepository<'_>,
    stock: &Path,
    targets: &HashSet<String>,
) -> Result<BodyCounts> {
    if targets.is_empty() {
        return Ok(BodyCounts::default());
    }
    let shared = Arc::new(targets.clone());
    let stock_path = stock.to_path_buf();
    let blocks = tokio::task::spawn_blocking(move || -> Result<HashMap<String, Vec<TiBlock>>> {
        // Passe 1 — sommaires des conteneurs cibles : kalitext → rang.
        let mut ranks: HashMap<String, usize> = HashMap::new();
        lj_sources::tar_reader::for_each_member(&stock_path, |name, raw| {
            let stem = name.rsplit('/').next().unwrap_or(&name);
            if !name.contains("/conteneur/")
                || !stem.starts_with("KALICONT")
                || !shared.contains(stem.trim_end_matches(".xml"))
            {
                return Ok(());
            }
            match lj_extract::kali::parse_kali_conteneur(&raw) {
                Ok(cont) => {
                    for (rank, kalitext) in cont.textes.into_iter().enumerate() {
                        ranks.insert(kalitext, rank);
                    }
                }
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "kali conteneur: parse échec");
                }
            }
            Ok(())
        })?;

        // Passe 2 — blocs des articles ancrés sur un conteneur cible.
        let mut blocks: HashMap<String, Vec<TiBlock>> = HashMap::new();
        lj_sources::tar_reader::for_each_member(&stock_path, |name, raw| {
            let stem = name.rsplit('/').next().unwrap_or(&name);
            if !name.contains("/article/") || !stem.starts_with("KALIARTI") {
                return Ok(());
            }
            let cid = attr_value(&raw, b"<CONTENEUR cid=\"");
            if !cid.is_some_and(|c| shared.contains(c)) {
                return Ok(());
            }
            match lj_extract::kali::parse_kali_article(&raw) {
                Ok(a) => {
                    if let Some(texte) = a.texte {
                        let rank = a
                            .kalitext
                            .as_deref()
                            .and_then(|k| ranks.get(k).copied())
                            .unwrap_or(usize::MAX);
                        blocks.entry(a.kalicont).or_default().push(TiBlock {
                            rank,
                            kaliarti: a.kaliarti,
                            titre: a.titre_text,
                            texte,
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(member = %name, error = %e, "kali article: parse échec");
                }
            }
            Ok(())
        })?;
        Ok(blocks)
    })
    .await
    .map_err(|e| anyhow!("tâche lecture stock KALI: {e}"))??;

    let mut counts = BodyCounts::default();
    for uid in targets {
        let Some(list) = blocks.get(uid) else {
            counts.no_content += 1;
            continue;
        };
        let body = assemble_ti(list);
        if body.is_empty() {
            counts.no_content += 1;
            continue;
        }
        if repo
            .set_legal_text_body(uid, &body)
            .await
            .map_err(|e| anyhow!("set_legal_text_body {uid}: {e}"))?
        {
            counts.updated += 1;
        }
    }
    Ok(counts)
}

/// Assemble les blocs d'un TI : tri `(rang du texte, id d'article)`, titre de
/// section préposé à chaque changement.
fn assemble_ti(blocks: &[TiBlock]) -> String {
    let mut sorted: Vec<&TiBlock> = blocks.iter().collect();
    sorted.sort_by(|a, b| (a.rank, &a.kaliarti).cmp(&(b.rank, &b.kaliarti)));
    let mut parts: Vec<String> = Vec::new();
    let mut last_titre: Option<&str> = None;
    for b in sorted {
        if let Some(t) = b.titre.as_deref() {
            if last_titre != Some(t) {
                parts.push(t.to_string());
                last_titre = Some(t);
            }
        }
        parts.push(b.texte.clone());
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec ADR 0223 : l'ordre du corps est celui du struct, pas celui des ids
    // (les JORFARTI ne suivent pas l'ordre de lecture) ; les blocs hors struct
    // suivent, par id croissant.
    #[test]
    fn assemble_follows_struct_order_then_leftover_ids() {
        let order = vec![
            "JORFARTI2".to_string(),
            "JORFARTI9".to_string(),
            "JORFARTI1".to_string(),
        ];
        let blocks = vec![
            ("JORFARTI1".to_string(), "annexe".to_string()),
            ("JORFARTI5".to_string(), "hors struct".to_string()),
            ("JORFARTI2".to_string(), "art 1er".to_string()),
            ("JORFARTI9".to_string(), "art 2".to_string()),
        ];
        assert_eq!(
            assemble_ordered(Some(&order), &blocks),
            "art 1er\n\nart 2\n\nannexe\n\nhors struct"
        );
    }

    #[test]
    fn assemble_without_struct_sorts_by_id() {
        let blocks = vec![
            ("KALIARTI9".to_string(), "b".to_string()),
            ("KALIARTI1".to_string(), "a".to_string()),
        ];
        assert_eq!(assemble_ordered(None, &blocks), "a\n\nb");
    }

    // Spec ADR 0223 : le titre de section KALI est préposé une fois par
    // section (c'est le seul endroit où vit le libellé), l'ordre est
    // (rang du texte au sommaire, id d'article).
    #[test]
    fn assemble_ti_prepends_section_titles_once() {
        let blocks = vec![
            TiBlock {
                rank: 0,
                kaliarti: "KALIARTI2".into(),
                titre: Some("Chapitre Ier".into()),
                texte: "champ".into(),
            },
            TiBlock {
                rank: 0,
                kaliarti: "KALIARTI1".into(),
                titre: Some("Préambule".into()),
                texte: "les parties".into(),
            },
            TiBlock {
                rank: 0,
                kaliarti: "KALIARTI3".into(),
                titre: Some("Chapitre Ier".into()),
                texte: "suite".into(),
            },
        ];
        assert_eq!(
            assemble_ti(&blocks),
            "Préambule\n\nles parties\n\nChapitre Ier\n\nchamp\n\nsuite"
        );
    }

    #[test]
    fn attr_value_reads_first_match() {
        let raw = br#"<CONTEXTE><TEXTE cid="JORFTEXT000000209787" nature="DECRET"/></CONTEXTE>"#;
        assert_eq!(
            attr_value(raw, b"<TEXTE cid=\""),
            Some("JORFTEXT000000209787")
        );
        assert_eq!(attr_value(raw, b"<CONTENEUR cid=\""), None);
    }
}
