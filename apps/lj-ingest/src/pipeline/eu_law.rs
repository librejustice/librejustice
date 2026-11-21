//! `sync-eu-law`: the EU fund carries the WHOLE consolidated series of each act,
//! sourced as Formex 4 through CELLAR, so an article reads at the date it is
//! asked for and not only today.
//!
//! Act registry = the `eu-*.json` datasets of `<state_dir>/ingest/corpus/`. Per
//! act: its consolidated versions, asked by SPARQL for the whole sweep at once →
//! the applicable snapshots (base act, then every consolidated version dated
//! today or earlier) → fetch of those the dataset has not read → pure parse
//! (`lj_parse::formex`) → merge into dated versions → the legal-corpus loader,
//! single DB write path.
//!
//! A version is born from a CHANGE OF BODY, never from a consolidation: a
//! snapshot repeating an article extends its version, a changed body closes the
//! previous one the day before, a vanished article closes its own. The dataset
//! holding the whole history is what makes the pass incremental: its
//! `snapshots` list says which are already read, and only the new one is
//! fetched. Lost (fresh machine), the history rebuilds from CELLAR.
//!
//! No watermark: comparing available against ingested is self-repairing. The
//! skip has two stages, dataset at target and DB carrying its last version (a
//! run interrupted between write and load).
//!
//! Acts with no Formex (pre-format ~1983, purely amending directives) keep their
//! dataset and their stock: counted `no_formex`, never rewritten blind. Network
//! failures are counted per act; past 5 % the pass exits in error after loading
//! what succeeded.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, NaiveDate};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use lj_parse::formex::{parse_formex_articles, FormexArticle};
use lj_sources::cjue::CjueClient;
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

/// Acts never visited each pass probes on top of the bodied ones, newest first.
/// A stock catch-up goes through `--backlog`, run by hand.
const UNVISITED_PER_NET: usize = 2_000;

/// CELLAR resource base a dataset URL is built on.
const CELEX_BASE_URL: &str = "http://publications.europa.eu/resource/celex";

/// Identity fields read back from the existing dataset: the sync never mints a
/// fiche, it refreshes the body of an act already in the registry.
#[derive(Deserialize)]
struct SeedDoc {
    text_uid: String,
    title: String,
    #[serde(default)]
    title_key: Option<String>,
    nature: String,
    jurisdiction: String,
    #[serde(default)]
    date_texte: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    /// Snapshots already merged into this dataset, oldest first. Empty (fiche
    /// seeded from the DB, or dataset predating the series) rebuilds the history.
    #[serde(default)]
    snapshots: Vec<SnapshotRef>,
    #[serde(default)]
    articles: Vec<SeedArticle>,
}

#[derive(Deserialize)]
struct SeedArticle {
    num: String,
    #[serde(default)]
    title_path: Option<String>,
    #[serde(default)]
    versions: Vec<SeedVersion>,
}

#[derive(Deserialize)]
struct SeedVersion {
    #[serde(default)]
    date_debut: Option<String>,
    #[serde(default)]
    date_fin: Option<String>,
    status: String,
    texte: String,
    #[serde(default)]
    source_url: Option<String>,
}

/// One form of the act: the CELEX serving it, and the date it takes effect.
/// `date` is absent only for a base act of unknown adoption date, whose version
/// then carries an open bound.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
struct SnapshotRef {
    celex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    date: Option<String>,
}

/// Un acte du registre, prêt à sonder.
struct SeedAct {
    path: PathBuf,
    doc: SeedDoc,
    /// CELEX de base (`32006L0112`), dérivé de `source_url`.
    celex_base: String,
}

/// Issue d'un acte de la passe.
enum Outcome {
    /// Dataset régénéré (nouvelle version consolidée ou premier passage).
    Updated(PathBuf),
    /// Déjà à la dernière version applicable.
    Unchanged,
    /// Aucune manifestation Formex FR pour la cible : stock conservé.
    NoFormex,
    /// Erreur réseau/parse : stock conservé, compté.
    Failed,
}

/// CELEX porté par l'URL du dataset (`…/resource/celex/{celex}`), ramené à sa
/// forme de base : une CELEX consolidée `0{corps}-{AAAAMMJJ}` (bootstrap REACH)
/// redevient `3{corps}`.
fn celex_base_from_url(url: &str) -> Option<String> {
    let celex = url.rsplit("/celex/").next().filter(|c| !c.is_empty())?;
    if let Some((head, tail)) = celex.split_once('-') {
        if celex.starts_with('0') && tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            return Some(format!("3{}", &head[1..]));
        }
    }
    Some(celex.to_string())
}

/// `20070101` to `2007-01-01`.
fn iso_date(compact: &str) -> String {
    format!("{}-{}-{}", &compact[0..4], &compact[4..6], &compact[6..8])
}

fn parse_iso(iso: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").map_err(|e| anyhow!("date {iso:?} invalide: {e}"))
}

/// The snapshots to read for an act: the base act, then each applicable
/// consolidated version. The base act is dropped when a consolidation shares its
/// date, two forms taking effect the same day leaving one unreachable.
fn snapshot_targets(
    celex_base: &str,
    date_texte: Option<&str>,
    applicable: &[&str],
) -> Vec<SnapshotRef> {
    let mut out = Vec::with_capacity(applicable.len() + 1);
    let premiere = applicable.first().map(|v| iso_date(v));
    let base_avant = match (date_texte, premiere.as_deref()) {
        (Some(d), Some(p)) => d < p,
        _ => true,
    };
    if base_avant {
        out.push(SnapshotRef {
            celex: celex_base.to_string(),
            date: date_texte.map(str::to_string),
        });
    }
    for v in applicable {
        out.push(SnapshotRef {
            celex: format!("0{}-{v}", &celex_base[1..]),
            date: Some(iso_date(v)),
        });
    }
    out
}

/// A dated version of an article, as it will land in the dataset.
struct VersionAcc {
    date_debut: Option<String>,
    date_fin: Option<String>,
    status: String,
    texte: String,
    celex: String,
}

struct ArticleAcc {
    num: String,
    title_path: Option<String>,
    versions: Vec<VersionAcc>,
}

/// The act's history under construction, one entry per article identity.
#[derive(Default)]
struct History {
    articles: HashMap<String, ArticleAcc>,
    /// Reading order of the last snapshot applied.
    order: Vec<String>,
}

impl History {
    /// Rebuilds the accumulator from a dataset that already carries its series.
    /// `None` when the file predates it, or lost the CELEX a version came from:
    /// the history is then refetched whole rather than extended on a guess.
    fn from_seed(doc: &SeedDoc) -> Option<Self> {
        if doc.snapshots.is_empty() || doc.articles.is_empty() {
            return None;
        }
        let mut articles = HashMap::with_capacity(doc.articles.len());
        let mut order = Vec::with_capacity(doc.articles.len());
        for a in &doc.articles {
            let mut versions = Vec::with_capacity(a.versions.len());
            for v in &a.versions {
                let celex = celex_from_url(v.source_url.as_deref()?)?;
                versions.push(VersionAcc {
                    date_debut: v.date_debut.clone(),
                    date_fin: v.date_fin.clone(),
                    status: v.status.clone(),
                    texte: v.texte.clone(),
                    celex,
                });
            }
            if versions.is_empty() {
                return None;
            }
            let key = lj_norm::article_key::identity_key(&a.num);
            order.push(key.clone());
            articles.insert(
                key,
                ArticleAcc {
                    num: a.num.clone(),
                    title_path: a.title_path.clone(),
                    versions,
                },
            );
        }
        Some(History { articles, order })
    }

    /// Merges one snapshot: an unchanged body extends its version, a changed one
    /// opens the next and closes the previous the day before, and an article the
    /// snapshot no longer carries closes its own.
    fn apply(&mut self, celex: &str, date: Option<NaiveDate>, articles: &[FormexArticle]) {
        let debut = date.map(|d| d.to_string());
        let veille = date.map(|d| (d - chrono::Duration::days(1)).to_string());
        let mut order = Vec::with_capacity(articles.len());
        let mut present: HashSet<String> = HashSet::with_capacity(articles.len());
        for a in articles {
            let key = lj_norm::article_key::identity_key(&a.num);
            if !present.insert(key.clone()) {
                continue;
            }
            order.push(key.clone());
            // The Formex subtitle (« Définitions ») opens the body: LEGI has no
            // per-article title, the body is its only carrier.
            let texte = match &a.subtitle {
                Some(s) => format!("{s}\n{}", a.texte),
                None => a.texte.clone(),
            };
            let acc = self.articles.entry(key).or_insert_with(|| ArticleAcc {
                num: a.num.clone(),
                title_path: a.title_path.clone(),
                versions: Vec::new(),
            });
            acc.num.clone_from(&a.num);
            acc.title_path.clone_from(&a.title_path);
            match acc.versions.last_mut() {
                Some(last) if last.date_fin.is_none() && last.texte == texte => continue,
                Some(last) if last.date_fin.is_none() => last.date_fin.clone_from(&veille),
                _ => {}
            }
            acc.versions.push(VersionAcc {
                status: super::legal_corpus::article_status(&texte).to_string(),
                date_debut: debut.clone(),
                date_fin: None,
                texte,
                celex: celex.to_string(),
            });
        }
        for (key, acc) in self.articles.iter_mut() {
            if present.contains(key) {
                continue;
            }
            if let Some(last) = acc.versions.last_mut() {
                if last.date_fin.is_none() {
                    last.date_fin.clone_from(&veille);
                }
            }
        }
        self.order = order;
    }

    fn is_empty(&self) -> bool {
        self.articles.is_empty()
    }

    /// Version rows the loader will write, the count the DB must match.
    fn total_versions(&self) -> usize {
        self.articles.values().map(|a| a.versions.len()).sum()
    }

    /// CELEX of the most recent version written: what the DB carries once the
    /// act is loaded. A consolidation that changed no article leaves it behind,
    /// which is why the skip reads it here and not from the snapshot list.
    fn last_celex(&self) -> Option<&str> {
        self.articles
            .values()
            .filter_map(|a| a.versions.last())
            .max_by(|x, y| x.date_debut.cmp(&y.date_debut))
            .map(|v| v.celex.as_str())
    }

    /// Dataset articles, in the reading order of the last snapshot; the ones it
    /// no longer carries follow, by identity key.
    fn into_json(self) -> Vec<serde_json::Value> {
        let History {
            mut articles,
            mut order,
        } = self;
        let vus: HashSet<&String> = order.iter().collect();
        let mut restes: Vec<String> = articles
            .keys()
            .filter(|k| !vus.contains(k))
            .cloned()
            .collect();
        restes.sort();
        drop(vus);
        order.extend(restes);
        order
            .iter()
            .filter_map(|k| articles.remove(k))
            .map(|a| {
                let versions: Vec<serde_json::Value> = a
                    .versions
                    .into_iter()
                    .map(|v| {
                        serde_json::json!({
                            "date_debut": v.date_debut,
                            "date_fin": v.date_fin,
                            "status": v.status,
                            "texte": v.texte,
                            "source_url": format!("{CELEX_BASE_URL}/{}", v.celex),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "num": a.num,
                    "title_path": a.title_path,
                    "versions": versions,
                })
            })
            .collect()
    }
}

/// CELEX carried by a dataset URL (`…/resource/celex/{celex}`).
fn celex_from_url(url: &str) -> Option<String> {
    url.rsplit("/celex/")
        .next()
        .filter(|c| !c.is_empty() && *c != url)
        .map(str::to_string)
}

/// Articles of one snapshot. A consolidated act carries its annexes inline, an
/// original act publishes them as separate volumes of the same manifestation;
/// reading both sources emits every annex twice, under the key the loader takes
/// for a primary key.
fn snapshot_articles(documents: &[String], celex: &str, text_uid: &str) -> Vec<FormexArticle> {
    let Some(acte) = documents.first() else {
        return Vec::new();
    };
    let mut articles = parse_formex_articles(acte);
    if !articles.iter().any(|a| a.num.starts_with("Annexe")) {
        articles.extend(
            documents[1..]
                .iter()
                .flat_map(|xml| parse_formex_articles(xml)),
        );
    }
    let mut seen = HashSet::new();
    let mut collisions: Vec<String> = Vec::new();
    articles.retain(|a| {
        let kept = seen.insert(lj_norm::article_key::identity_key(&a.num));
        if !kept {
            collisions.push(a.num.clone());
        }
        kept
    });
    if !collisions.is_empty() {
        tracing::warn!(celex = %celex, text_uid = %text_uid, numeros = ?collisions, "duplicate article key, content dropped");
    }
    articles
}

/// What the DB holds of an act, which says whether its series is loaded.
#[derive(Clone, Copy, Default)]
struct EtatBase<'a> {
    celex: Option<&'a str>,
    versions: Option<i64>,
}

/// Probes an act and extends its dataset with the snapshots it has not read.
/// Two skips, in this order: the dataset is at target and the DB holds its
/// series, nothing to do; the dataset is at target and the DB is behind (crash
/// between write and load), reload without refetching. `force` refetches the
/// whole series. Also returns the future versions read in the notice.
async fn refresh_act(
    client: &CjueClient,
    seed: &SeedAct,
    versions: &[String],
    db: EtatBase<'_>,
    today_compact: &str,
    force: bool,
) -> Result<Outcome> {
    // A version dated after today is pre-published: the pass of its day reads it.
    let applicable: Vec<&str> = versions
        .iter()
        .map(String::as_str)
        .filter(|v| *v <= today_compact)
        .collect();
    let targets = snapshot_targets(
        &seed.celex_base,
        seed.doc.date_texte.as_deref(),
        &applicable,
    );

    // An intermediate consolidation published after the fact breaks the prefix:
    // the series is then rebuilt whole rather than grafted onto the wrong past.
    let repris = match History::from_seed(&seed.doc) {
        Some(h) if !force && targets.starts_with(&seed.doc.snapshots) => Some(h),
        _ => None,
    };
    let (mut history, lus) = match repris {
        Some(h) => {
            if seed.doc.snapshots.len() == targets.len() {
                let charge =
                    db.celex == h.last_celex() && db.versions == Some(h.total_versions() as i64);
                let outcome = if charge {
                    Outcome::Unchanged
                } else {
                    Outcome::Updated(seed.path.clone())
                };
                return Ok(outcome);
            }
            let lus = seed.doc.snapshots.len();
            (h, lus)
        }
        None => (History::default(), 0),
    };

    for target in &targets[lus..] {
        let documents = client
            .formex_act_fr(&target.celex)
            .await
            .map_err(|e| anyhow!("formex {}: {e}", target.celex))?;
        let articles = snapshot_articles(&documents, &target.celex, &seed.doc.text_uid);
        if articles.is_empty() {
            tracing::warn!(celex = %target.celex, text_uid = %seed.doc.text_uid, "Formex without an article, snapshot skipped");
            continue;
        }
        let date = target.date.as_deref().map(parse_iso).transpose()?;
        history.apply(&target.celex, date, &articles);
    }
    if history.is_empty() {
        // Regressing to nothing would erase a possibly richer stock (XHTML
        // bootstrap, act older than Formex).
        return Ok(Outcome::NoFormex);
    }

    let today_iso = iso_date(today_compact);
    let derniere = targets
        .last()
        .ok_or_else(|| anyhow!("acte sans instantané: {}", seed.celex_base))?;
    let articles = history.into_json();
    let versions_ecrites: usize = articles
        .iter()
        .filter_map(|a| a["versions"].as_array().map(Vec::len))
        .sum();
    let doc = serde_json::json!({
        "text_uid": seed.doc.text_uid,
        "source": "eur-lex",
        "jurisdiction": seed.doc.jurisdiction,
        "title": seed.doc.title,
        "title_key": seed.doc.title_key,
        "nature": seed.doc.nature,
        "translation": "officiel",
        "source_url": format!("{CELEX_BASE_URL}/{}", derniere.celex),
        "source_asof": today_iso,
        "date_texte": seed.doc.date_texte,
        "snapshots": targets,
        "articles": articles,
    });
    std::fs::write(&seed.path, serde_json::to_vec_pretty(&doc)?)
        .map_err(|e| anyhow!("write {}: {e}", seed.path.display()))?;
    tracing::info!(
        text_uid = %seed.doc.text_uid,
        celex = %derniere.celex,
        instantanes = targets.len(),
        articles = articles.len(),
        versions = versions_ecrites,
        "eu-law dataset regenerated"
    );
    Ok(Outcome::Updated(seed.path.clone()))
}

/// Pose la borne de validité de chaque acte du registre (`(CELEX de base,
/// text_uid)`). Un acte abrogé ne fait paraître aucune consolidation : le delta
/// ne le voit jamais, d'où un registre **entier** à chaque passe.
async fn poser_bornes(
    repo: &DecisionRepository<'_>,
    ends: &HashMap<String, NaiveDate>,
    registre: &[(String, String)],
) -> Result<()> {
    let closures: Vec<(String, chrono::NaiveDate)> = registre
        .iter()
        .filter_map(|(base, uid)| ends.get(base).map(|fin| (uid.clone(), *fin)))
        .collect();
    let articles = repo
        .close_legal_articles(&closures)
        .await
        .map_err(|e| anyhow!("close_legal_articles: {e}"))?;
    tracing::info!(
        actes = closures.len(),
        articles,
        "sync-eu-law: validity bounds set"
    );
    Ok(())
}

/// Dates the acts of the registry that carry no date with CELLAR's date of the
/// document, written to the dataset on disk and to the text in the database.
async fn date_undated_acts(
    client: &CjueClient,
    repo: &DecisionRepository<'_>,
    seeds: &mut [SeedAct],
) -> Result<()> {
    let undated: Vec<String> = seeds
        .iter()
        .filter(|s| s.doc.date_texte.is_none())
        .map(|s| s.celex_base.clone())
        .collect();
    if undated.is_empty() {
        return Ok(());
    }
    let dates = client
        .acts_document_dates(&undated)
        .await
        .map_err(|e| anyhow!("document dates: {e}"))?;
    let mut fills = Vec::new();
    for seed in seeds.iter_mut().filter(|s| s.doc.date_texte.is_none()) {
        let Some(date) = dates.get(&seed.celex_base) else {
            continue;
        };
        seed.doc.date_texte = Some(date.to_string());
        if seed.path.exists() {
            let raw = std::fs::read_to_string(&seed.path)
                .map_err(|e| anyhow!("lecture {}: {e}", seed.path.display()))?;
            let mut doc: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow!("dataset invalide {}: {e}", seed.path.display()))?;
            doc["date_texte"] = serde_json::Value::String(date.to_string());
            std::fs::write(&seed.path, serde_json::to_vec_pretty(&doc)?)
                .map_err(|e| anyhow!("write {}: {e}", seed.path.display()))?;
        }
        fills.push((seed.doc.text_uid.clone(), *date));
    }
    let dated = repo
        .fill_text_dates(&fills)
        .await
        .map_err(|e| anyhow!("fill_text_dates: {e}"))?;
    if dated.is_empty() {
        tracing::info!(
            undated = undated.len(),
            "sync-eu-law: no undated act dated by CELLAR"
        );
    } else {
        tracing::warn!(
            undated = undated.len(),
            texts = dated.len(),
            "sync-eu-law: undated acts dated from CELLAR"
        );
    }
    Ok(())
}

/// Registre des actes : les datasets `eu-*.json` du corpus dir.
fn seed_acts(dir: &Path, only: Option<&str>) -> Result<Vec<SeedAct>> {
    let mut seeds = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| anyhow!("lecture {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| anyhow!("readdir: {e}"))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.starts_with("eu-") || !name.ends_with(".json") {
            continue;
        }
        if let Some(pat) = only {
            if !name.contains(pat) {
                continue;
            }
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow!("lecture {}: {e}", path.display()))?;
        let doc: SeedDoc = serde_json::from_str(&raw)
            .map_err(|e| anyhow!("dataset invalide {}: {e}", path.display()))?;
        let Some(celex_base) = doc
            .source_url
            .as_deref()
            .and_then(celex_base_from_url)
            .filter(|c| c.len() >= 2)
        else {
            tracing::warn!(dataset = %path.display(), "eu-law dataset without a usable CELEX, ignored");
            continue;
        };
        seeds.push(SeedAct {
            path,
            doc,
            celex_base,
        });
    }
    seeds.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(seeds)
}

/// `sync-eu-law` pass: regenerates the datasets a newer consolidated version
/// applies to, and reloads those alone. Every pass probes each act with a body
/// and a batch of those never visited, the consolidated versions asked by
/// SPARQL for all of them at once; `backlog` probes every act never visited,
/// the stock catch-up. A partial run (`limit`/`only`, debugging) sweeps its
/// subset without touching the manifest.
pub async fn sync_eu_law(
    limit: Option<usize>,
    only: Option<&str>,
    force: bool,
    backlog: bool,
    concurrency: usize,
) -> Result<()> {
    let settings = Settings::from_env()?;
    let dir = settings.legal_corpus_dir();
    // The registry is refreshed before being followed: without this pass, an act
    // nobody cited never enters. A partial run is debugging and has no business
    // growing the corpus.
    if limit.is_none() && only.is_none() {
        super::eu_catalog::sync_eu_catalog(false, None).await?;
    }
    let mut seeds = seed_acts(&dir, only)?;

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 4).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    lj_store::migrator::apply_migrations(&conn, lj_store::migrator::MigrationIntent::Verify)
        .await
        .map_err(|e| anyhow!("migrations: {e}"))?;
    let repo = DecisionRepository::new(&conn);
    // Fond UE en base : autorité du skip, ET complément du registre disque —
    // `state_dir` est local à la machine, un acte chargé ailleurs (bootstrap sur
    // la machine de dev) n'a pas son dataset ici.
    let registry = repo
        .eu_law_registry()
        .await
        .map_err(|e| anyhow!("eu_law_registry: {e}"))?;
    let etat_base: std::collections::HashMap<String, (String, i64)> = registry
        .iter()
        .map(|(uid, celex, versions, ..)| (uid.clone(), (celex.clone(), *versions)))
        .collect();
    let on_disk: HashSet<String> = seeds.iter().map(|s| s.doc.text_uid.clone()).collect();
    for (uid, celex, _versions, title, nature, jurisdiction, date_texte) in registry {
        if on_disk.contains(&uid) {
            continue;
        }
        let filename = format!("eu-{}.json", uid.replace('/', "-").to_lowercase());
        if let Some(pat) = only {
            if !filename.contains(pat) {
                continue;
            }
        }
        let url = format!("{CELEX_BASE_URL}/{celex}");
        let Some(celex_base) = celex_base_from_url(&url).filter(|c| c.len() >= 2) else {
            tracing::warn!(text_uid = %uid, celex = %celex, "act in the database without a usable CELEX, ignored");
            continue;
        };
        seeds.push(SeedAct {
            path: dir.join(filename),
            doc: SeedDoc {
                text_uid: uid,
                title,
                title_key: None,
                nature,
                jurisdiction,
                date_texte: date_texte.map(|d| d.to_string()),
                source_url: Some(url),
                snapshots: Vec::new(),
                articles: Vec::new(),
            },
            celex_base,
        });
    }
    seeds.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(n) = limit {
        seeds.truncate(n);
    }
    if seeds.is_empty() {
        tracing::warn!(dir = %dir.display(), "sync-eu-law: no act in the registry");
        return Ok(());
    }
    let client = Arc::new(CjueClient::new());
    date_undated_acts(&client, &repo, &mut seeds).await?;
    let today_compact = chrono::Utc::now().format("%Y%m%d").to_string();
    let today_iso = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // A partial run (debugging) leaves the manifest alone.
    let partial = limit.is_some() || only.is_some();
    let manifest_path = lj_sources::cjue::eu_law_manifest_path(&settings.cache_dir());
    let mut manifest = lj_sources::cjue::EuLawManifest::load(&manifest_path)
        .map_err(|e| anyhow!("manifest eu-law: {e}"))?;
    let all_bases: HashSet<String> = seeds.iter().map(|s| s.celex_base.clone()).collect();
    // A catalogue-only entry has no article to close.
    let registre: Vec<(String, String)> = seeds
        .iter()
        .filter(|s| !s.doc.snapshots.is_empty() || etat_base.contains_key(&s.doc.text_uid))
        .map(|s| (s.celex_base.clone(), s.doc.text_uid.clone()))
        .collect();

    let ends = client
        .acts_end_of_validity(
            &registre
                .iter()
                .map(|(base, _)| base.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| anyhow!("bornes de validité: {e}"))?;
    if !force && !partial {
        let bodied: HashSet<&str> = registre.iter().map(|(b, _)| b.as_str()).collect();
        let (keep, mut unvisited): (Vec<SeedAct>, Vec<SeedAct>) = seeds
            .into_iter()
            .filter(|s| {
                bodied.contains(s.celex_base.as_str())
                    || !manifest.visited.contains_key(&s.celex_base)
            })
            .partition(|s| bodied.contains(s.celex_base.as_str()));
        // Most acts have no Formex to find, nearly every one before 2000: one
        // listing marks them visited without a request.
        let mut sans_formex = 0usize;
        if !unvisited.is_empty() {
            let today = chrono::Utc::now().date_naive();
            let with_formex = client
                .acts_with_formex(super::eu_catalog::PREMIERE_ANNEE..=today.year())
                .await
                .map_err(|e| anyhow!("actes avec Formex: {e}"))?;
            unvisited.retain(|s| {
                let found = with_formex.contains(&s.celex_base);
                if !found {
                    manifest
                        .visited
                        .insert(s.celex_base.clone(), today_iso.clone());
                    sans_formex += 1;
                }
                found
            });
        }
        let backlog_total = unvisited.len();
        if !backlog {
            unvisited.sort_by(|a, b| b.doc.date_texte.cmp(&a.doc.date_texte));
            unvisited.truncate(UNVISITED_PER_NET);
        }
        tracing::info!(
            bodied = keep.len(),
            unvisited = unvisited.len(),
            backlog_total,
            sans_formex,
            "sync-eu-law: selection"
        );
        seeds = keep;
        seeds.extend(unvisited);
    }

    let total = seeds.len();
    if total == 0 {
        tracing::warn!("sync-eu-law: nothing to probe");
        return Ok(());
    }
    tracing::info!(total, force, "sync-eu-law: CELLAR sweep");
    let consolidated = client
        .acts_consolidated_versions(
            &seeds
                .iter()
                .map(|s| s.celex_base.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(|e| anyhow!("consolidated versions: {e}"))?;

    struct Probe {
        celex_base: String,
        outcome: Outcome,
    }
    let probes: Vec<Probe> = futures::stream::iter(seeds.iter().map(|seed| {
        let client = Arc::clone(&client);
        let today = today_compact.clone();
        let versions = consolidated
            .get(&seed.celex_base)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let db = match etat_base.get(&seed.doc.text_uid) {
            Some((celex, versions)) => EtatBase {
                celex: Some(celex.as_str()),
                versions: Some(*versions),
            },
            None => EtatBase::default(),
        };
        async move {
            match refresh_act(&client, seed, versions, db, &today, force).await {
                Ok(outcome) => Probe {
                    celex_base: seed.celex_base.clone(),
                    outcome,
                },
                Err(e) => {
                    tracing::warn!(
                        text_uid = %seed.doc.text_uid,
                        celex = %seed.celex_base,
                        error = %e,
                        "sync-eu-law: act failed, stock kept"
                    );
                    Probe {
                        celex_base: seed.celex_base.clone(),
                        outcome: Outcome::Failed,
                    }
                }
            }
        }
    }))
    .buffer_unordered(concurrency)
    .collect()
    .await;

    let mut updated: Vec<PathBuf> = Vec::new();
    let (mut unchanged, mut no_formex, mut failed) = (0usize, 0usize, 0usize);
    for p in &probes {
        match &p.outcome {
            Outcome::Updated(path) => updated.push(path.clone()),
            Outcome::Unchanged => unchanged += 1,
            Outcome::NoFormex => no_formex += 1,
            Outcome::Failed => failed += 1,
        }
    }
    updated.sort();

    if !partial {
        manifest.visited.retain(|base, _| all_bases.contains(base));
        for p in &probes {
            if !matches!(p.outcome, Outcome::Failed) {
                manifest
                    .visited
                    .insert(p.celex_base.clone(), today_iso.clone());
            }
        }
    }

    if !updated.is_empty() {
        let mut publie_targets: HashSet<String> = HashSet::new();
        for path in &updated {
            super::legal_corpus::load_dataset_file(&repo, path, &mut publie_targets).await?;
        }
        super::legal_corpus::run_corpus_finishers(&repo, publie_targets).await?;
    }

    // APRÈS le loader : une régénération réécrirait la fermeture.
    poser_bornes(&repo, &ends, &registre).await?;

    tracing::info!(
        total,
        updated = updated.len(),
        unchanged,
        no_formex,
        failed,
        "sync-eu-law done"
    );
    println!(
        "sync-eu-law : {} rechargés, {unchanged} à jour, {no_formex} sans Formex, \
         {failed} échecs (sur {total}).",
        updated.len()
    );
    if !partial {
        manifest
            .save(&manifest_path)
            .map_err(|e| anyhow!("manifest save: {e}"))?;
    }
    // Trop d'échecs = CELLAR malade ou recette cassée : sortie franche, après
    // avoir chargé ce qui a réussi (idempotent, le prochain run reprend).
    if failed * 20 > total {
        bail!("sync-eu-law : {failed} échecs sur {total} actes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{celex_base_from_url, snapshot_targets, History};
    use lj_parse::formex::FormexArticle;

    fn art(num: &str, texte: &str) -> FormexArticle {
        FormexArticle {
            num: num.to_string(),
            subtitle: None,
            title_path: None,
            texte: texte.to_string(),
        }
    }

    fn jour(iso: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").expect("date de test")
    }

    /// Spec: every applicable version makes a snapshot, the pre-published ones
    /// wait in the manifest.
    /// Spec: the base act opens the series, unless a consolidation takes effect
    /// on the day it was adopted.
    #[test]
    fn snapshot_targets_ouvre_sur_l_acte_de_base() {
        let cibles = snapshot_targets("32006L0112", Some("2006-11-28"), &["20070101", "20100415"]);
        let vus: Vec<(&str, Option<&str>)> = cibles
            .iter()
            .map(|s| (s.celex.as_str(), s.date.as_deref()))
            .collect();
        assert_eq!(
            vus,
            vec![
                ("32006L0112", Some("2006-11-28")),
                ("02006L0112-20070101", Some("2007-01-01")),
                ("02006L0112-20100415", Some("2010-04-15")),
            ]
        );
        // Never consolidated: the base act is the whole series.
        let cibles = snapshot_targets("32006L0112", Some("2006-11-28"), &[]);
        assert_eq!(cibles.len(), 1);
        // Consolidated on the day of adoption: it replaces the base act, two
        // forms opening the same day leaving one unreachable.
        let cibles = snapshot_targets("32006L0112", Some("2007-01-01"), &["20070101"]);
        assert_eq!(cibles.len(), 1);
        assert_eq!(cibles[0].celex, "02006L0112-20070101");
    }

    /// Spec: a version is born from a change of body, not from a consolidation.
    /// A repeated body extends its version, a changed one closes the previous
    /// the day before.
    #[test]
    fn apply_ouvre_une_version_au_changement_de_corps() {
        let mut h = History::default();
        h.apply(
            "32006L0112",
            Some(jour("2006-11-28")),
            &[art("1", "corps d'origine"), art("2", "stable")],
        );
        h.apply(
            "02006L0112-20070101",
            Some(jour("2007-01-01")),
            &[art("1", "corps d'origine"), art("2", "stable")],
        );
        h.apply(
            "02006L0112-20100415",
            Some(jour("2010-04-15")),
            &[art("1", "corps amendé"), art("2", "stable")],
        );
        let json = h.into_json();
        assert_eq!(json.len(), 2);
        let un = &json[0]["versions"];
        assert_eq!(un.as_array().map(Vec::len), Some(2));
        assert_eq!(un[0]["date_debut"], "2006-11-28");
        assert_eq!(un[0]["date_fin"], "2010-04-14");
        assert_eq!(un[1]["date_debut"], "2010-04-15");
        assert!(un[1]["date_fin"].is_null());
        // Two consolidations, an unchanged body: one open version.
        assert_eq!(json[1]["versions"].as_array().map(Vec::len), Some(1));
        assert!(json[1]["versions"][0]["date_fin"].is_null());
    }

    /// Spec: an article a snapshot no longer carries closes its version and
    /// keeps its history; an empty shell reads as `ABROGE`.
    #[test]
    fn apply_ferme_l_article_disparu_et_date_l_abrogation() {
        let mut h = History::default();
        h.apply(
            "32006L0112",
            Some(jour("2006-11-28")),
            &[art("1", "corps"), art("2", "corps")],
        );
        h.apply(
            "02006L0112-20070101",
            Some(jour("2007-01-01")),
            &[art("1", "corps"), art("2", "")],
        );
        h.apply(
            "02006L0112-20100415",
            Some(jour("2010-04-15")),
            &[art("1", "corps")],
        );
        let json = h.into_json();
        let deux = json
            .iter()
            .find(|a| a["num"] == "2")
            .expect("l'article disparu garde son historique");
        assert_eq!(deux["versions"].as_array().map(Vec::len), Some(2));
        assert_eq!(deux["versions"][0]["date_fin"], "2006-12-31");
        assert_eq!(deux["versions"][1]["status"], "ABROGE");
        assert_eq!(deux["versions"][1]["date_fin"], "2010-04-14");
    }

    #[test]
    fn celex_base_couvre_les_formes_du_registre() {
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/32006L0112")
                .as_deref(),
            Some("32006L0112")
        );
        // Bootstrap REACH : URL consolidée datée → CELEX de base.
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/02006R1907-20221217")
                .as_deref(),
            Some("32006R1907")
        );
        // CAAS (secteur 4, parenthèses) : inchangé.
        assert_eq!(
            celex_base_from_url("http://publications.europa.eu/resource/celex/42000A0922(02)")
                .as_deref(),
            Some("42000A0922(02)")
        );
    }
}
