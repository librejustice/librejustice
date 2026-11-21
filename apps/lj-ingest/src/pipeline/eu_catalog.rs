//! EUR-Lex catalogue ingest (EU secondary law).
//!
//! Fills `legal_text` with EU regulations and directives, as catalogue-only
//! entries (metadata, `articles: []`), through three doors:
//!
//! - BY DEMAND ([`ingest_eu_catalog`]): the slashnums our decisions actually
//!   cite and the corpus lacks (unlinked `legal_citation` spans, cut out of
//!   `full_text`). Catches the old or repealed acts a decision still invokes.
//! - BY THE REGISTRY ([`sync_eu_catalog`]): the whole registry of the six
//!   secondary-law types, cited or not, then its daily delta. The only door for
//!   an act nobody has cited yet.
//! - BY THE FRENCH JO ([`sync_eu_jorf`]): the DILA fiches of European nature,
//!   which name an act and never carry its body. Each is paired with its
//!   EUR-Lex act, then collapses into it, leaving one identity per act.
//!
//! This module is a dataset GENERATOR: one JSON per act under
//! `<state_dir>/ingest/corpus/`, read back by the `load_legal_corpus` loader.
//! Idempotent: a dataset already there is not re-fetched.
//!
//! The CELEX is rebuilt from the (nature, slashnum) pair, EU numbering being
//! inconsistent (pre-2015 regulations = `seq/year`, directives = `year/seq`,
//! unified to `year/seq` after 2015). Plausible readings are tried and the entry
//! is written only if the FR title fetched carries EXACTLY the cited slashnum.

use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDate};

use lj_norm::text_identity::celex_text_uid;
use lj_sources::cjue::CjueClient;
use lj_store::repository::{DecisionRepository, EuTwin};

use crate::config::Settings;

/// Premier numéro année/séquence `a/b` (a, b ≤ 4 chiffres) d'un libellé. Sert à
/// l'auto-validation.
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
    }
    None
}

/// CELEX candidats pour `(nature, slashnum)`. `nature ∈ {reglement,
/// directive}`. Rend 1-2 CELEX (interprétations année/séquence), le plus
/// probable d'abord selon la convention historique (règlement = `seq/année`,
/// directive = `année/seq`), l'autre en repli.
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

/// Dataset path of an act: one file per `text_uid`, stable name, the only shape
/// the on-disk registry of `sync-eu-law` can read back.
fn dataset_path(dir: &std::path::Path, text_uid: &str) -> std::path::PathBuf {
    dir.join(format!(
        "eu-{}.json",
        text_uid.replace('/', "-").to_lowercase()
    ))
}

/// Writes a catalogue-only entry: metadata and `articles: []`, the body coming
/// later from `sync-eu-law` through the consolidated Formex.
fn ecrire_dataset(
    path: &std::path::Path,
    text_uid: &str,
    title: &str,
    date: Option<&str>,
    nature_db: &str,
    celex: &str,
) -> Result<()> {
    let doc = serde_json::json!({
        "text_uid": text_uid,
        "source": "eur-lex",
        "jurisdiction": "EU",
        "title": title,
        "nature": nature_db,
        "translation": "officiel",
        "source_url": format!("http://publications.europa.eu/resource/celex/{celex}"),
        "date_texte": date,
        "articles": [],
    });
    std::fs::write(path, serde_json::to_vec_pretty(&doc)?)
        .map_err(|e| anyhow!("write {}: {e}", path.display()))
}

/// Génère les datasets EUR-Lex pour les slashnums cités-absents. `limit`
/// borne le nombre de slashnums traités (les plus cités d'abord) ; `None` = tous.
pub async fn ingest_eu_catalog(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {e}", dir.display()))?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
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
        "EUR-Lex ingest: cited-but-missing slashnums to fetch"
    );

    let client = CjueClient::new();
    let (mut written, mut skipped_existing, mut not_found, mut mismatch) = (0usize, 0, 0, 0);

    for (nature, slashnum, edges) in &missing {
        let candidates = celex_candidates(nature, slashnum);
        let mut done = false;
        for celex in &candidates {
            let Some(text_uid) = celex_text_uid(celex) else {
                continue;
            };
            let path = dataset_path(&dir, &text_uid);
            if path.exists() {
                skipped_existing += 1;
                done = true;
                break;
            }
            let meta = client.legislation_meta_fr(celex).await?;
            let Some((title, date)) = meta else {
                continue; // CELEX inexistant / sans titre FR → interprétation suivante
            };
            if first_slashnum(&title).as_deref() != Some(slashnum.as_str()) {
                continue;
            }
            let nature_db = if nature == "reglement" {
                "REGLEMENT"
            } else {
                "DIRECTIVE_EURO"
            };
            ecrire_dataset(&path, &text_uid, &title, date.as_deref(), nature_db, celex)?;
            tracing::debug!(celex, %text_uid, edges, "EU act fetched");
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
        "EUR-Lex ingest done: datasets generated (load them with load-legal-corpus)"
    );
    println!(
        "ingest-eu-catalog : {written} écrits, {skipped_existing} déjà présents, \
         {not_found} introuvables/non-validés, {mismatch} slashnum illisible ; \
         puis `lj-ingest load-legal-corpus` + `resolve-citations`."
    );
    Ok(())
}

/// First year of the registry: the ECSC acts of 1952.
pub(crate) const PREMIERE_ANNEE: i32 = 1952;

/// Days the registry delta reaches back before its watermark. CELLAR can register
/// an act after the day it dates the registration with; the overlap costs a few
/// hundred rows, the weekly full sweep catches what it still misses.
const RECOUVREMENT_JOURS: i64 = 30;

/// Brings in every act of the EUR-Lex registry the corpus lacks. The only door
/// through which an uncited act arrives, since [`ingest_eu_catalog`] sees only
/// what our decisions name. Two cadences: the delta asks the acts CELLAR
/// registered since the watermark of the eu-law manifest, the full pass sweeps
/// the registry year by year, and is the bootstrap when the watermark is absent.
/// Loads right away, one file per transaction, otherwise nothing exists in the
/// database. The full pass then stamps CELLAR's end of validity on the acts with
/// no article.
pub async fn sync_eu_catalog(full: bool, limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {e}", dir.display()))?;
    let manifest_path = lj_sources::cjue::eu_law_manifest_path(&settings.cache_dir());
    let mut manifest = lj_sources::cjue::EuLawManifest::load(&manifest_path)
        .map_err(|e| anyhow!("manifest eu-law: {e}"))?;
    let aujourdhui = chrono::Utc::now().date_naive();
    let since = match (&manifest.catalog_since, full) {
        (Some(watermark), false) => {
            let watermark = NaiveDate::parse_from_str(watermark, "%Y-%m-%d")
                .map_err(|e| anyhow!("catalog_since {watermark:?}: {e}"))?;
            Some(watermark - chrono::Duration::days(RECOUVREMENT_JOURS))
        }
        _ => None,
    };

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    // Disk alone would lie: an act loaded from another machine has no dataset
    // here, and would be rewritten then reloaded on every pass. The CELEX
    // counts as much as the uid: `EU/REG/1408-71` holds 31971R1408, and
    // `EU/REG/1408-1971` would open it a second time.
    let registre = repo
        .eu_law_registry()
        .await
        .map_err(|e| anyhow!("eu_law_registry: {e}"))?;
    let mut en_base: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (uid, celex, ..) in registre {
        en_base.insert(uid);
        if let Some(canonical) = canonical_uid_from_notice(&celex) {
            en_base.insert(canonical);
        }
    }

    let client = CjueClient::new();
    let (mut vus, mut deja) = (0usize, 0usize);
    let mut ecrits: Vec<(std::path::PathBuf, String)> = Vec::new();
    let partitions: Vec<Option<i32>> = match since {
        Some(_) => vec![None],
        None => (PREMIERE_ANNEE..=aujourdhui.year()).map(Some).collect(),
    };
    'balayage: for annee in partitions {
        let mut offset = 0usize;
        loop {
            let page = match (annee, since) {
                (Some(annee), _) => client.legislation_page(annee, offset).await?,
                (None, Some(since)) => {
                    client
                        .legislation_since_page(&since.to_string(), offset)
                        .await?
                }
                (None, None) => unreachable!("a delta pass always has its bound"),
            };
            let pleine = page.len() >= lj_sources::cjue::PAGE_SIZE;
            for (celex, date, title) in &page {
                let Some(text_uid) = celex_text_uid(celex) else {
                    continue; // corrigendum, unnumbered act: not a corpus text
                };
                vus += 1;
                let path = dataset_path(&dir, &text_uid);
                if path.exists() || en_base.contains(&text_uid) {
                    deja += 1;
                    continue;
                }
                let nature_db = if text_uid.starts_with("EU/REG/") {
                    "REGLEMENT"
                } else {
                    "DIRECTIVE_EURO"
                };
                ecrire_dataset(&path, &text_uid, title, Some(date), nature_db, celex)?;
                tracing::debug!(celex, %text_uid, "EU act discovered in the registry");
                ecrits.push((path, text_uid));
                if limit.is_some_and(|n| ecrits.len() >= n) {
                    break 'balayage;
                }
            }
            if !pleine {
                break;
            }
            offset += lj_sources::cjue::PAGE_SIZE;
        }
    }

    let mut publie_targets = std::collections::HashSet::new();
    for (path, _) in &ecrits {
        super::legal_corpus::load_dataset_file(&repo, path, &mut publie_targets).await?;
    }
    if !ecrits.is_empty() {
        super::legal_corpus::run_corpus_finishers(&repo, publie_targets).await?;
    }
    // A pass cut by `--limit` has not seen the whole window.
    if limit.is_none() {
        manifest.catalog_since = Some(aujourdhui.to_string());
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
    }

    let full = since.is_none();
    if full {
        stamp_eu_validity(&repo, &client).await?;
    }
    tracing::info!(
        full,
        since = ?since,
        vus,
        deja,
        ecrits = ecrits.len(),
        "sync-eu-catalog: EUR-Lex registry swept"
    );
    println!(
        "sync-eu-catalog ({}) : {vus} actes au registre, {deja} déjà au fonds, {n} entrés et chargés.",
        match since {
            Some(since) => format!("delta depuis le {since}"),
            None => "registre entier".to_string(),
        },
        n = ecrits.len()
    );
    Ok(())
}

/// Stamps CELLAR's end of validity on every European text with no article.
async fn stamp_eu_validity(repo: &DecisionRepository<'_>, client: &CjueClient) -> Result<()> {
    let texts = repo
        .eu_texts_without_articles()
        .await
        .map_err(|e| anyhow!("eu_texts_without_articles: {e}"))?;
    let by_celex: std::collections::HashMap<String, (String, Option<String>)> = texts
        .into_iter()
        .filter_map(|(uid, status)| {
            Some((lj_norm::text_identity::text_uid_celex(&uid)?, (uid, status)))
        })
        .collect();
    let bases: Vec<String> = by_celex.keys().cloned().collect();
    let ends = client
        .acts_end_of_validity(&bases)
        .await
        .map_err(|e| anyhow!("acts_end_of_validity: {e}"))?;
    let today = chrono::Utc::now().date_naive();
    let (mut expired, mut restored) = (Vec::new(), Vec::new());
    for (celex, end) in &ends {
        let Some((uid, status)) = by_celex.get(celex) else {
            continue;
        };
        match (*end < today, status.as_deref() == Some("ABROGE")) {
            (true, false) => expired.push(uid.clone()),
            (false, true) => restored.push(uid.clone()),
            _ => {}
        }
    }
    let abrogated = repo
        .set_texts_status(&expired, Some("ABROGE"))
        .await
        .map_err(|e| anyhow!("set_texts_status: {e}"))?;
    let in_force = repo
        .set_texts_status(&restored, None)
        .await
        .map_err(|e| anyhow!("set_texts_status: {e}"))?;
    tracing::info!(
        texts = by_celex.len(),
        ends = ends.len(),
        abrogated,
        in_force,
        "sync-eu-catalog: CELLAR's ends of validity stamped"
    );
    Ok(())
}

/// Nature the DILA title announces, in the vocabulary [`celex_candidates`]
/// reads. The column says `DIRECTIVE_EURO` on regulations too, the title alone
/// is right.
fn nature_of_title(title: &str) -> Option<&'static str> {
    let head = title.trim_start();
    if head.starts_with("Directive") {
        Some("directive")
    } else if head.starts_with("Règlement") {
        Some("reglement")
    } else {
        None
    }
}

/// Pairs the JO fiches of European nature with their EUR-Lex act and collapses
/// them into it: the act is minted at the catalogue when the corpus lacks it,
/// which is the only way its body ever arrives.
///
/// Self-repairing rather than watermarked: a fiche already collapsed is gone
/// from the source query, and one DILA republishes is paired again next pass.
pub async fn sync_eu_jorf(limit: Option<usize>) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {e}", dir.display()))?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);

    let mut shells = repo.jorf_eu_shells().await?;
    let total = shells.len();
    if let Some(n) = limit {
        shells.truncate(n);
    }
    let mut en_base: std::collections::HashSet<String> =
        repo.eu_text_uids().await?.into_iter().collect();

    let client = CjueClient::new();
    let (mut fusionnees, mut frappees, mut sans_acte) = (0usize, 0usize, 0usize);
    let mut publie_targets = std::collections::HashSet::new();
    for (jorf_uid, title, _nature) in &shells {
        let (Some(nature), Some(slashnum)) = (nature_of_title(title), first_slashnum(title)) else {
            sans_acte += 1;
            continue;
        };
        let candidates = celex_candidates(nature, &slashnum);
        let uids: Vec<(String, String)> = candidates
            .iter()
            .filter_map(|c| celex_text_uid(c).map(|u| (c.clone(), u)))
            .collect();
        // The corpus has both numbering conventions on `EU/…`: an act already
        // there under either form is the target, minting would open a second.
        let mut target = uids.iter().find(|(_, u)| en_base.contains(u)).cloned();
        if target.is_none() {
            for (celex, uid) in &uids {
                let Some((titre_ue, date)) = client.legislation_meta_fr(celex).await? else {
                    continue;
                };
                if first_slashnum(&titre_ue).as_deref() != Some(slashnum.as_str()) {
                    continue;
                }
                let nature_db = if nature == "reglement" {
                    "REGLEMENT"
                } else {
                    "DIRECTIVE_EURO"
                };
                let path = dataset_path(&dir, uid);
                if !path.exists() {
                    ecrire_dataset(&path, uid, &titre_ue, date.as_deref(), nature_db, celex)?;
                }
                super::legal_corpus::load_dataset_file(&repo, &path, &mut publie_targets).await?;
                en_base.insert(uid.clone());
                frappees += 1;
                target = Some((celex.clone(), uid.clone()));
                break;
            }
        }
        let Some((_, uid)) = target else {
            sans_acte += 1;
            continue;
        };
        if repo.collapse_legal_text(jorf_uid, &uid).await?.is_some() {
            fusionnees += 1;
            tracing::debug!(from = jorf_uid, to = uid, "OJ typo merged");
        }
    }
    // On `frappees`, not on `publie_targets`: a catalogue entry has no article,
    // so it opens no `publie` edge, and the finishers are what gives it a slug.
    // A redirect posed towards a fiche with none makes the read crash.
    if frappees > 0 {
        super::legal_corpus::run_corpus_finishers(&repo, publie_targets).await?;
    }
    let (ranked_texts, _) = repo.refresh_reading_ranks().await?;

    tracing::info!(
        total,
        fusionnees,
        frappees,
        sans_acte,
        ranked_texts,
        "sync-eu-jorf: OJ typos matched to EUR-Lex"
    );
    println!(
        "sync-eu-jorf : {total} coquilles du JO, {fusionnees} fusionnées dans leur acte EUR-Lex \
         (dont {frappees} actes frappés au catalogue), {sans_acte} sans acte identifiable.\n\
         Suite : sync-eu-law pour les corps, puis reextract-fields --full --field legal_references."
    );
    Ok(())
}

/// Base CELEX of a notice: a consolidated series is `0{corps}-{AAAAMMJJ}` and
/// only the sector 3 form names an act, which is what `celex_text_uid` reads.
fn canonical_uid_from_notice(celex: &str) -> Option<String> {
    match lj_sources::cjue::parse_consolidated_celex(celex) {
        Some(r) => celex_text_uid(&format!("3{}", r.body)),
        None => celex_text_uid(celex),
    }
}

/// Of two uids differing only by the order of number and year, the one the CELEX
/// names, and `None` unless the pair really holds one act twice. Two four-digit
/// numbers make a legitimate pair of **distinct** acts, 2015/2016 and 2016/2015
/// both existing: proof of duplication is that both sides answer the same CELEX,
/// or that the side carrying none is empty and is exactly what the other's CELEX
/// should have named.
pub(crate) fn canonical_of(twins: &(EuTwin, EuTwin)) -> Option<String> {
    let (a, b) = twins;
    let named = |t: &EuTwin| t.celex.as_deref().and_then(canonical_uid_from_notice);
    match (named(a), named(b)) {
        (Some(ca), Some(cb)) => (ca == cb && (ca == a.text_uid || ca == b.text_uid)).then_some(ca),
        (Some(c), None) => (c == b.text_uid && b.articles == 0).then_some(c),
        (None, Some(c)) => (c == a.text_uid && a.articles == 0).then_some(c),
        (None, None) => None,
    }
}

/// Datasets that would re-seed a uid the base no longer holds, while their own
/// CELEX names a fiche it does hold. The loader keys on the `text_uid` inside the
/// file and not on its name, 342 of the EU datasets being named after the
/// JORFTEXT they came from: a deletion keyed on the name misses those, and the
/// next sweep reopens the identity that was just closed.
pub(crate) fn datasets_a_retirer(
    dir: &std::path::Path,
    en_base: &std::collections::HashSet<String>,
) -> Result<Vec<(std::path::PathBuf, String, String)>> {
    #[derive(serde::Deserialize)]
    struct SeedUid {
        text_uid: String,
        source_url: Option<String>,
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| anyhow!("lecture {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| anyhow!("readdir: {e}"))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("eu-") || !name.ends_with(".json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("lecture {}: {e}", path.display()))?;
        let doc: SeedUid = serde_json::from_str(&raw)
            .map_err(|e| anyhow!("dataset invalide {}: {e}", path.display()))?;
        if en_base.contains(&doc.text_uid) {
            continue;
        }
        let Some(nomme) = doc
            .source_url
            .as_deref()
            .and_then(|u| u.rsplit_once("/celex/").map(|(_, c)| c.to_string()))
            .and_then(|c| canonical_uid_from_notice(&c))
        else {
            continue;
        };
        if nomme != doc.text_uid && en_base.contains(&nomme) {
            out.push((path, doc.text_uid, nomme));
        }
    }
    Ok(out)
}

/// Merges the `EU/` fiches held twice under swapped number and year, keeping the
/// uid the CELEX names. Legacy of an ingest no longer in the code, so a one-off
/// on the base; the disk pass, itself a net, runs on every call. Dry run unless
/// `apply`.
pub async fn merge_eu_swapped_uids(apply: bool) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    conn.batch_execute("SET statement_timeout = 0").await?;
    let repo = DecisionRepository::new(&conn);

    let twins = repo.eu_uid_twins().await?;
    let mut plan: Vec<(&EuTwin, &EuTwin, bool)> = Vec::new();
    let mut laissees = 0usize;
    for pair in &twins {
        let Some(canonical) = canonical_of(pair) else {
            laissees += 1;
            tracing::info!(
                gauche = %pair.0.text_uid,
                gauche_celex = ?pair.0.celex,
                droite = %pair.1.text_uid,
                droite_celex = ?pair.1.celex,
                "swapped uids not proven duplicates, left in place"
            );
            continue;
        };
        let (garde, meurt) = if canonical == pair.0.text_uid {
            (&pair.0, &pair.1)
        } else {
            (&pair.1, &pair.0)
        };
        // The body is not always on the uid the CELEX names. When it is not, the
        // empty canonical dies into the fiche that has it, which then takes the
        // canonical uid: two moves rather than a migration of every child row.
        let corps_a_deplacer = garde.articles == 0 && meurt.articles > 0;
        plan.push((garde, meurt, corps_a_deplacer));
    }

    let (mut fusionnees, mut deplacees) = (0usize, 0usize);
    for (garde, meurt, corps_a_deplacer) in &plan {
        if !apply {
            println!(
                "{} <- {} ({} articles{})",
                garde.text_uid,
                meurt.text_uid,
                meurt.articles,
                if *corps_a_deplacer {
                    ", body to move"
                } else {
                    ""
                }
            );
            continue;
        }
        if *corps_a_deplacer {
            repo.collapse_legal_text(&garde.text_uid, &meurt.text_uid)
                .await?;
            repo.rename_legal_text(&meurt.text_uid, &garde.text_uid, None)
                .await?;
            deplacees += 1;
        } else {
            repo.collapse_legal_text(&meurt.text_uid, &garde.text_uid)
                .await?;
            fusionnees += 1;
        }
        tracing::info!(garde = %garde.text_uid, meurt = %meurt.text_uid, corps_a_deplacer, "swapped uid merged");
    }

    // Read after the merges, so the disk pass sees the state a regeneration
    // would start from. In a dry run the uids about to die are subtracted, which
    // is the same state without writing it.
    let mut en_base: std::collections::HashSet<String> =
        repo.eu_text_uids().await?.into_iter().collect();
    if !apply {
        for (_, meurt, _) in &plan {
            en_base.remove(&meurt.text_uid);
        }
    }
    let orphelins = datasets_a_retirer(&dir, &en_base)?;
    for (path, uid, nomme) in &orphelins {
        if apply {
            std::fs::remove_file(path).map_err(|e| anyhow!("rm {}: {e}", path.display()))?;
        }
        tracing::info!(dataset = %path.display(), uid, nomme, apply, "dataset reseeding a closed uid");
    }

    let (n, s) = (orphelins.len(), if orphelins.len() > 1 { "s" } else { "" });
    if !apply {
        println!(
            "merge-eu-swapped-uids : {} paires, {laissees} non prouvées doublons, {n} dataset{s} à \
             retirer. Rien écrit, --apply pour fusionner.",
            twins.len()
        );
        return Ok(());
    }
    println!(
        "merge-eu-swapped-uids : {} paires, {fusionnees} fusionnées, {deplacees} dont le corps a \
         changé d'uid, {laissees} laissées en place, {n} dataset{s} retiré{s}.",
        twins.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nature_vient_du_titre_pas_de_la_colonne() {
        assert_eq!(
            nature_of_title("Directive 2011/65/UE du Parlement"),
            Some("directive")
        );
        assert_eq!(
            nature_of_title("Règlement (UE) 2016/679"),
            Some("reglement")
        );
        assert_eq!(nature_of_title("Décision 2010/87/UE"), None);
    }

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
        assert_eq!(
            first_slashnum("Sixième directive 77/388/CEE du Conseil"),
            Some("77/388".to_string())
        );
        assert_eq!(first_slashnum("aucun numéro ici"), None);
    }

    #[test]
    fn celex_candidates_covers_conventions() {
        assert_eq!(
            celex_candidates("reglement", "2016/679"),
            vec!["32016R0679"]
        );
        assert_eq!(celex_candidates("reglement", "44/2001"), vec!["32001R0044"]);
        assert_eq!(celex_candidates("directive", "77/388"), vec!["31977L0388"]);
        assert_eq!(celex_candidates("directive", "2003/86"), vec!["32003L0086"]);
    }

    fn twin(uid: &str, celex: Option<&str>, articles: i64) -> EuTwin {
        EuTwin {
            text_uid: uid.into(),
            celex: celex.map(Into::into),
            articles,
        }
    }

    #[test]
    fn celex_settles_the_swap_even_when_consolidated() {
        assert_eq!(
            canonical_of(&(
                twin("EU/REG/2016-679", Some("02016R0679-20160504"), 99),
                twin("EU/REG/679-2016", Some("02016R0679-20160504"), 99),
            ))
            .as_deref(),
            Some("EU/REG/679-2016")
        );
        assert_eq!(
            canonical_of(&(
                twin("EU/DIR/115-2008", None, 0),
                twin("EU/DIR/2008-115", Some("32008L0115"), 23),
            ))
            .as_deref(),
            Some("EU/DIR/115-2008")
        );
        assert_eq!(
            canonical_of(&(
                twin("EU/DIR/1-2000", None, 0),
                twin("EU/DIR/2000-1", None, 3)
            )),
            None
        );
    }

    #[test]
    fn two_real_acts_are_not_a_swap() {
        // 2016/2017 and 2017/2016 both exist. The two CELEX differ, so the pair
        // holds two acts and not one act twice.
        assert_eq!(
            canonical_of(&(
                twin("EU/REG/2016-2017", Some("32017R2016"), 15),
                twin("EU/REG/2017-2016", Some("32016R2017"), 3),
            )),
            None
        );
        // A single CELEX, but it names its own side: nothing says the empty twin
        // is the same act, it may simply not be loaded yet.
        assert_eq!(
            canonical_of(&(
                twin("EU/REG/2015-2016", Some("32016R2015"), 4),
                twin("EU/REG/2016-2015", None, 0),
            )),
            None
        );
        assert_eq!(
            canonical_of(&(
                twin("EU/REG/2019-2024", None, 0),
                twin("EU/REG/2024-2019", Some("02019R2024-20210501"), 14),
            )),
            None
        );
    }
}
