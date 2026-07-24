//! Ingestion catalogue EUR-Lex (droit dérivé UE) — ADR 0138.
//!
//! Peuple `legal_text` avec les règlements/directives UE **cités mais absents** du
//! catalogue, en **entrées catalogue-seul** (métadonnées, `articles: []`) — la règle
//! UE slashnum du linker (ADR 0145) les ré-apparie à la passe intégrale suivante.
//! Piloté PAR LA DEMANDE : on n'ingère que les slashnums réellement cités (spans
//! non liés de `legal_citation`, retranchés de `full_text`), pas tout EUR-Lex.
//!
//! Ce module est un GÉNÉRATEUR de datasets : il écrit un JSON par acte sous
//! `<state_dir>/ingest/corpus/` (règle #17), consommé ensuite par le loader
//! `load_legal_corpus` (frontière d'écriture DB unique, règle #2). Idempotent : un
//! dataset déjà présent n'est pas re-fetché (reprise après interruption).
//!
//! Robustesse : le CELEX est reconstruit du couple (nature, slashnum) — la numérotation
//! UE est incohérente (règlements pré-2015 = `seq/année`, directives = `année/seq`,
//! unifiée `année/seq` post-2015). On teste les interprétations plausibles et on
//! **AUTO-VALIDE** : l'entrée n'est écrite que si le titre FR récupéré porte EXACTEMENT
//! le slashnum cité (un CELEX mal deviné rend un autre numéro → rejeté, zéro mislink).

use anyhow::{anyhow, Result};

use lj_sources::cjue::CjueClient;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Premier numéro année/séquence `a/b` (a,b ≤ 4 chiffres) d'un libellé — MÊME extraction
/// que le test d'absence catalogue (`regexp_match(title, '(\d{1,4}/\d{1,4})')`).
/// Sert à l'auto-validation.
fn first_slashnum(s: &str) -> Option<String> {
    let ch: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < ch.len() {
        if !ch[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let l0 = i;
        while i < ch.len() && ch[i].is_ascii_digit() {
            i += 1;
        }
        let left_len = i - l0;
        if i < ch.len() && ch[i] == '/' {
            let r0 = i + 1;
            let mut j = r0;
            while j < ch.len() && ch[j].is_ascii_digit() {
                j += 1;
            }
            if j > r0 && left_len <= 4 && (j - r0) <= 4 {
                return Some(ch[l0..j].iter().collect());
            }
        }
        // sinon : la run de gauche n'ouvre pas un slashnum, on reprend après elle.
    }
    None
}

/// CELEX candidats pour `(nature, slashnum)`. `nature ∈ {reglement, directive}`.
/// Rend 1-2 CELEX (interprétations année/séquence), le plus probable d'abord selon la
/// convention historique (règlement = `seq/année`, directive = `année/seq`), l'autre en
/// repli. L'auto-validation aval tranche définitivement.
fn celex_candidates(nature: &str, slashnum: &str) -> Vec<String> {
    let (a, b) = match slashnum.split_once('/') {
        Some((a, b)) => (a, b),
        None => return Vec::new(),
    };
    let letter = if nature == "reglement" { 'R' } else { 'L' };
    let full_year = |y: i64| -> Option<i64> {
        match y {
            1950..=2035 => Some(y),   // année 4 chiffres
            0..=99 => Some(1900 + y), // année 2 chiffres → 19YY (UE pré-2000)
            _ => None,
        }
    };
    let mk = |year: i64, seq: i64| format!("3{year:04}{letter}{seq:04}");
    let (Ok(na), Ok(nb)) = (a.parse::<i64>(), b.parse::<i64>()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut push = |year: Option<i64>, seq: i64| {
        if let Some(y) = year {
            let c = mk(y, seq);
            if !out.contains(&c) {
                out.push(c);
            }
        }
    };
    // Un token 4-chiffres-année épingle l'interprétation sans ambiguïté.
    if (1950..=2035).contains(&na) {
        push(full_year(na), nb); // année/seq
    } else if (1950..=2035).contains(&nb) {
        push(full_year(nb), na); // seq/année
    } else if nature == "reglement" {
        push(full_year(nb), na); // règlement pré-2000 : seq/YY d'abord
        push(full_year(na), nb);
    } else {
        push(full_year(na), nb); // directive pré-2000 : YY/seq d'abord
        push(full_year(nb), na);
    }
    out
}

/// text_uid canonique depuis un CELEX validé : `32016R0679` → `EU/REG/679-2016`
/// (format des entrées REGLEMENT existantes), `31977L0388` → `EU/DIR/388-1977`.
fn text_uid_from_celex(celex: &str) -> Option<String> {
    let ch: Vec<char> = celex.chars().collect();
    if ch.len() < 7 || ch[0] != '3' {
        return None;
    }
    let year: String = ch[1..5].iter().collect();
    let letter = ch[5];
    let seq: String = ch[6..].iter().collect();
    let seq_trim = seq.trim_start_matches('0');
    let seq_trim = if seq_trim.is_empty() { "0" } else { seq_trim };
    let kind = match letter {
        'R' => "REG",
        'L' => "DIR",
        _ => return None,
    };
    Some(format!("EU/{kind}/{seq_trim}-{year}"))
}

/// Génère les datasets EUR-Lex pour les slashnums cités-absents (ADR 0138).
/// `limit` borne le nombre de slashnums traités (les plus cités d'abord) ; `None` = tous.
pub async fn ingest_eu_catalog(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {e}", dir.display()))?;

    let pool =
        lj_store::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    // Scan lourd (retranche les spans non liés de `full_text`) : pas de timeout.
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);

    let mut missing = repo.cited_eu_slashnums_missing().await?;
    let total_edges: i64 = missing.iter().map(|(_, _, e)| *e).sum();
    if let Some(n) = limit {
        missing.truncate(n);
    }
    let target_edges: i64 = missing.iter().map(|(_, _, e)| *e).sum();
    tracing::info!(
        slashnums = missing.len(),
        target_edges,
        total_edges,
        "ingestion EUR-Lex : slashnums cités-absents à récupérer"
    );

    let client = CjueClient::new();
    let (mut written, mut skipped_existing, mut not_found, mut mismatch) = (0usize, 0, 0, 0);

    for (nature, slashnum, edges) in &missing {
        let candidates = celex_candidates(nature, slashnum);
        let mut done = false;
        for celex in &candidates {
            let Some(text_uid) = text_uid_from_celex(celex) else {
                continue;
            };
            let path = dir.join(format!(
                "eu-{}.json",
                text_uid.replace('/', "-").to_lowercase()
            ));
            if path.exists() {
                skipped_existing += 1;
                done = true;
                break;
            }
            let meta = client.legislation_meta_fr(celex).await?;
            let Some((title, date)) = meta else {
                continue; // CELEX inexistant / sans titre FR → interprétation suivante
            };
            // AUTO-VALIDATION : le titre récupéré porte-t-il EXACTEMENT le slashnum cité ?
            if first_slashnum(&title).as_deref() != Some(slashnum.as_str()) {
                continue;
            }
            let nature_db = if nature == "reglement" {
                "REGLEMENT"
            } else {
                "DIRECTIVE_EURO"
            };
            let doc = serde_json::json!({
                "text_uid": text_uid,
                "source": "eur-lex",
                "jurisdiction": "UE",
                "title": title,
                "nature": nature_db,
                "translation": "officiel",
                "source_url": format!("http://publications.europa.eu/resource/celex/{celex}"),
                "date_texte": date,
                "articles": [],
            });
            std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)
                .map_err(|e| anyhow!("write {}: {e}", path.display()))?;
            tracing::debug!(celex, %text_uid, edges, "acte UE récupéré");
            written += 1;
            done = true;
            break;
        }
        if !done {
            if candidates.is_empty() {
                mismatch += 1;
            } else {
                not_found += 1;
            }
        }
    }

    tracing::info!(
        written,
        skipped_existing,
        not_found,
        mismatch,
        "ingestion EUR-Lex terminée : datasets générés (charger via load-legal-corpus)"
    );
    println!(
        "ingest-eu-catalog : {written} écrits, {skipped_existing} déjà présents, \
         {not_found} introuvables/non-validés, {mismatch} slashnum illisible ; \
         puis `lj-ingest load-legal-corpus` + `resolve-citations`."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_slashnum_extracts_head_number() {
        assert_eq!(
            first_slashnum("Règlement (UE) 2016/679 du Parlement"),
            Some("2016/679".to_string())
        );
        assert_eq!(
            first_slashnum("Règlement (CE) n° 44/2001 du Conseil"),
            Some("44/2001".to_string())
        );
        // Directive : le premier slashnum est le numéro, pas le suffixe /CEE.
        assert_eq!(
            first_slashnum("Sixième directive 77/388/CEE du Conseil"),
            Some("77/388".to_string())
        );
        assert_eq!(first_slashnum("aucun numéro ici"), None);
    }

    #[test]
    fn celex_candidates_covers_conventions() {
        // Règlement moderne année/seq.
        assert_eq!(
            celex_candidates("reglement", "2016/679"),
            vec!["32016R0679"]
        );
        // Règlement classique seq/année (4-chiffres année épingle).
        assert_eq!(celex_candidates("reglement", "44/2001"), vec!["32001R0044"]);
        // Directive classique année-2chiffres/seq (année d'abord).
        assert_eq!(celex_candidates("directive", "77/388"), vec!["31977L0388"]);
        // Directive moderne année/seq.
        assert_eq!(celex_candidates("directive", "2003/86"), vec!["32003L0086"]);
    }

    #[test]
    fn text_uid_matches_existing_format() {
        assert_eq!(
            text_uid_from_celex("32016R0679").as_deref(),
            Some("EU/REG/679-2016")
        );
        assert_eq!(
            text_uid_from_celex("31977L0388").as_deref(),
            Some("EU/DIR/388-1977")
        );
        assert_eq!(
            text_uid_from_celex("32001R0044").as_deref(),
            Some("EU/REG/44-2001")
        );
    }
}
