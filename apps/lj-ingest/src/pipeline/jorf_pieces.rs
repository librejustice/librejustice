//! What a JO text prints beside its numbered articles, its unnumbered articles
//! and the blocks around them, copied from its French publication row into
//! `legal_article` as pieces of the text, every row in printed order.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use lj_store::repository::{DecisionRepository, JorfPiece, PieceSource};
use serde_json::Value;

use crate::config::Settings;

const BATCH: i64 = 2000;

#[derive(Default, Debug)]
pub(super) struct PieceCounts {
    texts: usize,
    pieces: u64,
    positioned: u64,
    dropped: u64,
    undated: usize,
}

struct Placed {
    id: String,
    num: Option<String>,
    title_path: Option<String>,
}

fn walk(
    items: &Value,
    sections: &Value,
    path: &mut Vec<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<Placed>,
) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(id) = item["article"].as_str() {
            if seen.insert(id.to_string()) {
                out.push(Placed {
                    id: id.to_string(),
                    num: item["num"]
                        .as_str()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string),
                    title_path: (!path.is_empty()).then(|| path.join(" > ")),
                });
            }
        } else if let Some(id) = item["section"].as_str() {
            if seen.insert(id.to_string()) {
                let section = &sections[id];
                let title = section["title"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty());
                path.extend(title.map(str::to_string));
                walk(&section["items"], sections, path, seen, out);
                if title.is_some() {
                    path.pop();
                }
            }
        }
    }
}

/// A publication's articles in printed order, each with the number it prints, its
/// struct's or else the label its text opens on, unless the text publishes an
/// instrument.
fn printed(s: &PieceSource) -> Vec<Placed> {
    let Some(entries) = s.articles.as_object() else {
        return Vec::new();
    };
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    walk(
        &s.structure,
        &s.sections,
        &mut Vec::new(),
        &mut seen,
        &mut order,
    );
    let mut rest: Vec<&String> = entries.keys().filter(|id| !seen.contains(*id)).collect();
    rest.sort();
    order.extend(rest.into_iter().map(|id| Placed {
        id: id.clone(),
        num: None,
        title_path: None,
    }));
    for p in &mut order {
        if p.num.is_none() && !s.publishes {
            p.num = entries
                .get(&p.id)
                .and_then(|e| e["text"].as_str())
                .and_then(lj_norm::article_key::inline_num);
        }
    }
    order
}

/// The articles of `order` printing a number an article printed before them in
/// the text already gives, under the key the article pass gives it.
fn repeated(s: &PieceSource, order: &[Placed]) -> HashSet<String> {
    let dispositif: HashSet<String> = s
        .structure
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["article"].is_string())
        .filter_map(|i| i["num"].as_str())
        .map(lj_norm::article_key::identity_key)
        .collect();
    let mut first = HashSet::new();
    order
        .iter()
        .filter_map(|p| {
            let key = lj_norm::article_key::identity_key(p.num.as_deref()?);
            let key = match &p.title_path {
                Some(path) if dispositif.contains(&key) => {
                    format!("{}-{key}", lj_norm::article_key::article_key(path))
                }
                _ => key,
            };
            (!first.insert(key)).then(|| p.id.clone())
        })
        .collect()
}

/// The articles of the French JO publications of `only` that print a number an
/// earlier article of their text gives: the article pass keys them on their id.
pub(super) async fn repeated_articles(
    repo: &DecisionRepository<'_>,
    only: &[String],
) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    let mut after = String::new();
    loop {
        let batch = repo
            .jorf_piece_sources(&after, BATCH, Some(only))
            .await
            .map_err(|e| anyhow!("jorf_piece_sources: {e}"))?;
        let Some(last) = batch.last() else {
            return Ok(out);
        };
        after = last.text_uid.clone();
        for s in &batch {
            out.extend(repeated(s, &printed(s)));
        }
    }
}

/// A publication's pieces, the printed place of its numbered `JORFARTI`, and those
/// with their number as `(text_uid, JORFARTI, num_key)`, whose old piece goes.
struct Moved {
    pieces: Vec<JorfPiece>,
    positions: Vec<(String, String, i32)>,
    numbered: Vec<(String, String, String)>,
}

fn moved(s: &PieceSource, date_debut: chrono::NaiveDate, held: &HashSet<String>) -> Moved {
    let mut out = Moved {
        pieces: Vec::new(),
        positions: Vec::new(),
        numbered: Vec::new(),
    };
    let block = |key: &str| {
        let position = lj_norm::article_order::block_position(key)?;
        let texte = s.blocks[key]
            .as_str()
            .map(str::trim)
            .filter(|t| !t.is_empty())?;
        Some(JorfPiece {
            text_uid: s.text_uid.clone(),
            num_key: key.to_string(),
            source_uid: s.text_uid.clone(),
            position,
            title_path: None,
            date_debut,
            texte: Some(texte.to_string()),
            nota: None,
            content_checksum: xxhash_rust::xxh3::xxh3_64(texte.as_bytes()),
        })
    };
    out.pieces.extend(
        lj_norm::article_order::BLOCKS
            .iter()
            .filter_map(|k| block(k)),
    );
    let Some(entries) = s.articles.as_object() else {
        return out;
    };
    let order = printed(s);
    let repeats = repeated(s, &order);
    for (rank, p) in order.into_iter().enumerate() {
        let position = rank as i32 + 1;
        let Some(entry) = entries.get(&p.id) else {
            continue;
        };
        if let Some(num) = &p.num {
            let key = if repeats.contains(&p.id) {
                p.id.to_lowercase()
            } else {
                lj_norm::article_key::identity_key(num)
            };
            out.numbered.push((s.text_uid.clone(), p.id.clone(), key));
            out.positions.push((s.text_uid.clone(), p.id, position));
            continue;
        }
        if held.contains(&p.id) || entry.get("held").is_some() {
            continue;
        }
        out.pieces.push(JorfPiece {
            text_uid: s.text_uid.clone(),
            num_key: p.id.to_lowercase(),
            source_uid: p.id.clone(),
            position,
            title_path: p.title_path,
            date_debut,
            texte: entry["text"].as_str().map(str::to_string),
            nota: entry["nota"].as_str().map(str::to_string),
            content_checksum: xxhash_rust::xxh3::xxh3_64(entry.to_string().as_bytes()),
        });
    }
    out
}

/// Writes the pieces of the French JO publications, every one or those of
/// `only`, and the printed place of their numbered rows.
pub(super) async fn move_pieces(
    repo: &DecisionRepository<'_>,
    only: Option<&[String]>,
) -> Result<PieceCounts> {
    let mut counts = PieceCounts::default();
    let mut after = String::new();
    loop {
        let batch = repo
            .jorf_piece_sources(&after, BATCH, only)
            .await
            .map_err(|e| anyhow!("jorf_piece_sources: {e}"))?;
        let Some(last) = batch.last() else {
            counts.dropped += repo
                .drop_twinned_jorf_pieces(only)
                .await
                .map_err(|e| anyhow!("drop_twinned_jorf_pieces: {e}"))?;
            return Ok(counts);
        };
        after = last.text_uid.clone();
        let ids: Vec<&str> = batch
            .iter()
            .filter_map(|s| s.articles.as_object())
            .flat_map(|m| m.keys().map(String::as_str))
            .collect();
        let held = repo
            .jorf_articles_held(&ids)
            .await
            .map_err(|e| anyhow!("jorf_articles_held: {e}"))?;

        let (mut pieces, mut positions, mut numbered, mut texts) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for s in &batch {
            let Some(date_debut) = s.date_publi else {
                counts.undated += 1;
                continue;
            };
            let m = moved(s, date_debut, &held);
            if !s.held_out && !m.pieces.is_empty() {
                texts.push(s.text_uid.clone());
                pieces.extend(m.pieces);
            }
            positions.extend(m.positions);
            numbered.extend(m.numbered);
        }
        counts.pieces += repo
            .move_jorf_pieces(&pieces)
            .await
            .map_err(|e| anyhow!("move_jorf_pieces: {e}"))?;
        counts.positioned += repo
            .set_jorf_positions(&positions)
            .await
            .map_err(|e| anyhow!("set_jorf_positions: {e}"))?;
        let batch_texts: Vec<String> = batch.iter().map(|s| s.text_uid.clone()).collect();
        counts.dropped += repo
            .drop_consolidated_jorf_pieces(&batch_texts, &numbered)
            .await
            .map_err(|e| anyhow!("drop_consolidated_jorf_pieces: {e}"))?;
        counts.texts += texts.len();
        if only.is_none() {
            tracing::info!(after = %after, ?counts, "jorf pieces: batch written");
        }
    }
}

/// Writes what every French JO publication prints beside its numbered articles
/// as pieces, then ranks the texts that got them. Replayable.
pub async fn backfill_jorf_pieces() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0")
        .await
        .map_err(|e| anyhow!("set statement_timeout: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    let started = std::time::Instant::now();
    let counts = move_pieces(&repo, None).await?;
    let (ranked_texts, ranked_rows) = repo
        .refresh_reading_ranks()
        .await
        .map_err(|e| anyhow!("refresh_reading_ranks: {e}"))?;
    tracing::warn!(
        ?counts,
        ranked_texts,
        ranked_rows,
        seconds = started.elapsed().as_secs(),
        "jorf pieces: written"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source(structure: Value, sections: Value, articles: Value) -> PieceSource {
        PieceSource {
            text_uid: "JORFTEXT000000000001".to_string(),
            date_publi: None,
            structure,
            sections,
            articles,
            blocks: json!({}),
            held_out: false,
            publishes: false,
        }
    }

    fn repeats(s: &PieceSource) -> Vec<String> {
        let mut ids: Vec<String> = repeated(s, &printed(s)).into_iter().collect();
        ids.sort();
        ids
    }

    #[test]
    fn a_later_article_printing_a_number_its_text_gave_is_repeated() {
        let annexed = source(
            json!([{"article": "A3"}, {"article": "A1"}, {"article": "A4"}, {"article": "A2"}]),
            json!({}),
            json!({
                "A3": {"text": "Art. 1er. - Le présent arrêté fixe le cahier des charges."},
                "A1": {"text": "Art. 2. - Le directeur est chargé de son exécution."},
                "A4": {"text": "Article 1er\nLe concessionnaire entretient l'ouvrage."},
                "A2": {"text": "Art. 2. - Il en répond devant l'Etat."},
            }),
        );
        assert_eq!(repeats(&annexed), ["A2", "A4"]);

        let sectioned = source(
            json!([{"article": "B1", "num": "1"}, {"section": "S1"}]),
            json!({"S1": {"title": "Annexe", "items": [
                {"article": "B2", "num": "1"},
                {"article": "B3", "num": "1"},
            ]}}),
            json!({"B1": {"text": "x"}, "B2": {"text": "y"}, "B3": {"text": "z"}}),
        );
        assert_eq!(repeats(&sectioned), ["B3"]);
    }

    #[test]
    fn a_decree_publishing_a_treaty_leaves_its_unstructured_labels_unnumbered() {
        let mut decree = source(
            json!([{"article": "A1", "num": "1"}, {"article": "A2"}]),
            json!({}),
            json!({
                "A1": {"text": "La convention sera publiée au Journal officiel."},
                "A2": {"text": "Article 3\nCommission mixte\nIl est créé une commission mixte."},
            }),
        );
        let nums = |s: &PieceSource| -> Vec<Option<String>> {
            printed(s).into_iter().map(|p| p.num).collect()
        };
        assert_eq!(
            nums(&decree),
            [Some("1".to_string()), Some("3".to_string())]
        );
        decree.publishes = true;
        assert_eq!(nums(&decree), [Some("1".to_string()), None]);
    }
}
