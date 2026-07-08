//! Passe d'analyse sur le corpus local (port de
//! `librejustice-store/pipelines/analyze.py`).
//!
//! Lit toutes les décisions sous `data/zips/**/*.zip` (+ dossiers `--extra-dir`
//! de `*.xml`), parse chacune via `lj-core`, et produit un rapport JSON :
//! totaux, distribution de longueurs (tokens ≈ mots), juridictions vues, top 50
//! tokens de refs normalisées, et N décisions d'échantillon pour revue manuelle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lj_core::normalizer::extract_refs;
use lj_core::parsing::parse_xml;
use lj_sources::zip_reader::iter_decisions;
use serde_json::{json, Map, Value};

/// Compteur ordonné façon `collections.Counter` : suit l'ordre d'insertion
/// (première occurrence) pour casser les égalités comme CPython.
#[derive(Default)]
struct Counter {
    counts: HashMap<String, i64>,
    order: Vec<String>,
}

impl Counter {
    fn bump(&mut self, key: &str) {
        match self.counts.get_mut(key) {
            Some(c) => *c += 1,
            None => {
                self.counts.insert(key.to_string(), 1);
                self.order.push(key.to_string());
            }
        }
    }

    fn total(&self) -> i64 {
        self.counts.values().sum()
    }

    fn unique(&self) -> usize {
        self.counts.len()
    }

    /// Port de `Counter.most_common(n)` : tri par count desc, stable sur
    /// l'ordre d'insertion. `n=None` → tout.
    fn most_common(&self, n: Option<usize>) -> Vec<(String, i64)> {
        let mut items: Vec<(String, i64)> = self
            .order
            .iter()
            .map(|k| (k.clone(), self.counts[k]))
            .collect();
        // stable : préserve l'ordre d'insertion à égalité
        items.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        if let Some(limit) = n {
            items.truncate(limit);
        }
        items
    }

    /// `dict(most_common(n))` → objet JSON ordonné.
    fn to_json(&self, n: Option<usize>) -> Value {
        let mut map = Map::new();
        for (key, count) in self.most_common(n) {
            map.insert(key, Value::from(count));
        }
        Value::Object(map)
    }
}

/// Approximation rapide : mots séparés par whitespace (suffisant pour p50/p99).
fn token_count(txt: &str) -> usize {
    txt.split_whitespace().count()
}

/// Itère toutes les paires `(member_name, raw_xml)` du corpus local.
///
/// Port de `_iter_corpus` : zips sous `data_dir/zips/**/*.zip` préfixés par le
/// nom du zip, puis fichiers `*.xml` des dossiers `extra_dirs`.
fn iter_corpus(data_dir: &Path, extra_dirs: &[PathBuf]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let zips_root = data_dir.join("zips");
    if zips_root.exists() {
        for zip_path in sorted_rglob(&zips_root, "zip")? {
            let zip_name = zip_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            for (member, raw) in iter_decisions(&zip_path)
                .with_context(|| format!("analyze: lecture zip {}", zip_path.display()))?
            {
                // On préfixe le nom de membre par le zip pour tracer l'origine.
                out.push((format!("{zip_name}::{member}"), raw));
            }
        }
    }
    for extra in extra_dirs {
        if !extra.exists() {
            continue;
        }
        for xml_path in sorted_rglob(extra, "xml")? {
            let raw = std::fs::read(&xml_path)
                .with_context(|| format!("analyze: read {}", xml_path.display()))?;
            out.push((xml_path.to_string_lossy().into_owned(), raw));
        }
    }
    Ok(out)
}

/// `sorted(root.rglob("*.<ext>"))` — chemins triés, récursif.
fn sorted_rglob(root: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    walk(root, ext, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("analyze: read_dir {}", dir.display()))?
    {
        let entry = entry.context("analyze: read_dir entry")?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, ext, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    Ok(())
}

/// Percentile (port de `_percentile`) : `values[round((n-1)*p)]` sur tri.
fn percentile(values: &[usize], p: f64) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    // `round()` Python = arrondi banquier ; ici les indices sont entiers donc
    // l'arrondi half-to-even tombe rarement à .5 — on suit Python via round_ties_even.
    let k = (((sorted.len() - 1) as f64) * p).round_ties_even() as usize;
    sorted[k.min(sorted.len() - 1)]
}

/// Construit l'entrée d'échantillon JSON pour une décision.
fn sample_entry(decision: &lj_core::decision::Decision, texte: &str, n_tokens: usize) -> Value {
    let refs: Vec<Value> = extract_refs(texte)
        .iter()
        .map(|r| {
            json!({
                "prefix": r.prefix,
                "num": r.num,
                "code": r.code,
                "article_token": r.article_token(),
                "compound_token": r.compound_token(),
            })
        })
        .collect();
    // texte[:600] côté Python = 600 *caractères* (codepoints), pas octets.
    let texte_debut: String = texte.chars().take(600).collect();
    json!({
        "source_uid": decision.source_uid,
        "juridiction": decision.juridiction_code,
        "date": decision.date_lecture,
        "type_recours": decision.type_recours,
        "solution": decision.solution,
        "n_tokens": n_tokens,
        "texte_debut": texte_debut,
        "refs": refs,
    })
}

/// Lance l'analyse et retourne le rapport JSON (port de `analyze.run`).
///
/// `seed` est conservé pour le sampling par réservoir ; la parité bit-à-bit du
/// RNG avec CPython (`random.Random`) n'est pas garantie (Mersenne Twister ≠
/// PRNG Rust), mais l'algorithme et le format de sortie sont identiques.
pub fn run(data_dir: &Path, extra_dirs: &[PathBuf], sample: usize, seed: u64) -> Result<Value> {
    let mut rng = SmallRng::new(seed);

    let mut juridictions = Counter::default();
    let mut jur_types = Counter::default();
    let mut types_recours = Counter::default();
    let mut solutions = Counter::default();
    let mut ref_tokens = Counter::default();
    let mut warnings = Counter::default();
    let mut token_counts: Vec<usize> = Vec::new();

    let mut sampled: Vec<Value> = Vec::new();
    let mut total: u64 = 0;
    let mut missing_text: u64 = 0;

    for (member, raw) in iter_corpus(data_dir, extra_dirs)? {
        total += 1;
        let decision = parse_xml(&raw, &member, None);
        for w in &decision.parse_warnings {
            warnings.bump(w);
        }
        juridictions.bump(decision.juridiction_code.as_deref().unwrap_or("UNKNOWN"));
        jur_types.bump(decision.juridiction_type.as_deref().unwrap_or("UNKNOWN"));
        if let Some(tr) = &decision.type_recours {
            types_recours.bump(tr);
        }
        if let Some(sol) = &decision.solution {
            solutions.bump(sol);
        }

        if decision.texte_integral_raw.is_empty() {
            missing_text += 1;
            continue;
        }

        let texte = decision.texte_integral_clean.clone();
        let n_tokens = token_count(&texte);
        token_counts.push(n_tokens);
        for r in extract_refs(&texte) {
            let token = r.compound_token().unwrap_or_else(|| r.article_token());
            ref_tokens.bump(&token);
        }

        // Reservoir sampling : échantillon uniforme sans tout garder.
        if sampled.len() < sample {
            sampled.push(sample_entry(&decision, &texte, n_tokens));
        } else {
            // Python : j = rng.randint(0, total - 1), inclusif des deux bornes.
            let j = rng.randint(total - 1);
            if (j as usize) < sample {
                sampled[j as usize] = sample_entry(&decision, &texte, n_tokens);
            }
        }
    }

    let mean = if token_counts.is_empty() {
        Value::from(0)
    } else {
        let sum: usize = token_counts.iter().sum();
        let mean = sum as f64 / token_counts.len() as f64;
        // round(x, 1) Python.
        Value::from((mean * 10.0).round_ties_even() / 10.0)
    };

    Ok(json!({
        "total_decisions": total,
        "missing_texte_integral": missing_text,
        "juridiction_types": jur_types.to_json(None),
        "juridictions_top_30": juridictions.to_json(Some(30)),
        "types_recours": types_recours.to_json(None),
        "solutions_top_20": solutions.to_json(Some(20)),
        "length_tokens": {
            "count": token_counts.len(),
            "mean": mean,
            "p50": percentile(&token_counts, 0.50),
            "p90": percentile(&token_counts, 0.90),
            "p99": percentile(&token_counts, 0.99),
            "max": token_counts.iter().copied().max().unwrap_or(0),
        },
        "refs_tokens_top_50": ref_tokens.to_json(Some(50)),
        "refs_tokens_total": ref_tokens.total(),
        "refs_tokens_unique": ref_tokens.unique(),
        "parse_warnings": warnings.to_json(None),
        "sample": Value::Array(sampled),
    }))
}

/// PRNG déterministe minimal (SplitMix64) pour le reservoir sampling.
///
/// NB : ce n'est PAS le Mersenne Twister de CPython ; le *contenu* de
/// l'échantillon différera de Python à seed égal, mais le tirage est
/// déterministe et reproductible côté Rust.
struct SmallRng {
    state: u64,
}

impl SmallRng {
    fn new(seed: u64) -> Self {
        // Évite l'état 0 (qui produirait une suite dégénérée).
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Entier uniforme dans `[0, hi]` (inclusif), comme `random.randint`.
    fn randint(&mut self, hi: u64) -> u64 {
        self.next_u64() % (hi + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec : token_count = mots séparés par whitespace.
    #[test]
    fn token_count_words() {
        assert_eq!(token_count("le tribunal administratif de Paris"), 5);
        assert_eq!(token_count("  espaces   multiples  "), 2);
        assert_eq!(token_count(""), 0);
    }

    // Spec : percentile = values[round((n-1)*p)] sur tri.
    #[test]
    fn percentile_indices() {
        let v: Vec<usize> = (1..=100).collect(); // 1..100
        assert_eq!(percentile(&v, 0.50), 51); // round(99*0.5)=round(49.5)=50 (banker's) → v[50]=51
        assert_eq!(percentile(&v, 0.90), 90); // round(99*0.9)=round(89.1)=89 → v[89]=90
        assert_eq!(percentile(&v, 0.99), 99); // round(99*0.99)=round(98.01)=98 → v[98]=99
        assert_eq!(percentile(&[], 0.5), 0);
    }

    // Spec : Counter.most_common = tri count desc, stable insertion à égalité.
    #[test]
    fn counter_most_common_order() {
        let mut c = Counter::default();
        for k in ["a", "b", "a", "c", "b", "a"] {
            c.bump(k);
        }
        // a:3, b:2, c:1.
        assert_eq!(
            c.most_common(None),
            vec![
                ("a".to_string(), 3),
                ("b".to_string(), 2),
                ("c".to_string(), 1)
            ]
        );
        assert_eq!(c.total(), 6);
        assert_eq!(c.unique(), 3);
    }

    // Spec : à égalité de count, l'ordre d'insertion est préservé.
    #[test]
    fn counter_ties_keep_insertion_order() {
        let mut c = Counter::default();
        for k in ["z", "y", "x"] {
            c.bump(k);
        }
        assert_eq!(
            c.most_common(None),
            vec![
                ("z".to_string(), 1),
                ("y".to_string(), 1),
                ("x".to_string(), 1)
            ]
        );
    }
}
