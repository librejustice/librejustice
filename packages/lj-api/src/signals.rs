//! Pondération RRF dynamique — port de `signals.py` (ADR 0032).
//!
//! La fusion RRF a 3 jambes (BM25 body, ANN, BM25 titre). On part d'une base à
//! parité `BASE = (title=1.0, bm25=1.0, ann=1.0)` puis on applique des
//! multiplicateurs cumulatifs selon des signaux détectés sur la query (regex) ou
//! la géométrie des embeddings (centroïdes BM25 vs ANN).
//!
//! - article-in-query (`L 531-1`, `R. 222-1`…)  : title ×0.2, bm25 ×1.5
//! - docket-in-query (numéro de rôle, tous formats) : title ×6.0
//! - date-in-query (FR/ISO)                          : title ×1.5
//! - `d_centroids > 0.06` ET `pair_mu < 0.18`        : bm25 ×0.4, ann ×2.5
//!
//! Article et boost_ann sont mutuellement exclusifs ; docket/date se cumulent.

use std::sync::LazyLock;

use regex::Regex;

/// Longueur considérée pour les centroïdes / la matrice pairwise.
pub const TOP_K: usize = 50;

/// Seuils issus de la calibration empirique (FINDINGS.md).
const D_CENTROIDS_THRESHOLD: f64 = 0.06;
const PAIR_MU_THRESHOLD: f64 = 0.18;

/// `\b[LRD]\.?\s*\d+(?:[-]\d+)+` matche L 531-1, R. 222-1, D.521-2… (IGNORECASE).
///
/// `regex` n'a pas de `\b` côté Unicode identique à Python re, mais sur ces
/// motifs ASCII (lettre + chiffres) le comportement est équivalent.
static ARTICLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[LRD]\.?\s*\d+(?:-\d+)+").unwrap());

/// Numéro de rôle, toutes juridictions. Faux positifs tolérés (boost titre seul).
///   - CC ancien    `07-11.687` / `74-11.869`
///   - CC nouveau   `19-23664` (pourvoi)
///   - CA / TJ      `23/01001`
///   - TA / CAA / CE `2202166` (6-8 chiffres contigus)
static DOCKET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b\d{2}-\d{2}\.\d{3,4}\b", // CC ancien : 07-11.687, 19-23.664
        r"|\b\d{2}-\d{4,5}\b",       // CC nouveau : 19-23664
        r"|\b\d{2}/\d{5}\b",         // CA / TJ : 23/01001
        r"|\b\d{6,8}\b",             // TA / CAA / CE : 2202166
    ))
    .unwrap()
});

/// Date FR (« 13 février 2024 », « février 2024 ») ou ISO (« 2024-02-13 »).
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let months = "janvier|f[ée]vrier|mars|avril|mai|juin|juillet|ao[uû]t|septembre|octobre|novembre|d[ée]cembre";
    let pat = format!(
        r"(?i)\b(?:\d{{1,2}}\s+(?:{m})(?:\s+\d{{4}})?|(?:{m})\s+\d{{4}}|\d{{4}}-\d{{2}}-\d{{2}})\b",
        m = months
    );
    Regex::new(&pat).unwrap()
});

/// Poids RRF par jambe. Tous strictement positifs (sauf cas extrême).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub title: f64,
    pub bm25: f64,
    pub ann: f64,
}

impl Weights {
    fn scale(self, title: f64, bm25: f64, ann: f64) -> Weights {
        Weights {
            title: self.title * title,
            bm25: self.bm25 * bm25,
            ann: self.ann * ann,
        }
    }
}

/// Base par défaut : 3 jambes à parité.
pub const BASE_WEIGHTS: Weights = Weights {
    title: 1.0,
    bm25: 1.0,
    ann: 1.0,
};

pub fn has_article_reference(query: &str) -> bool {
    ARTICLE_RE.is_match(query)
}

pub fn has_docket_reference(query: &str) -> bool {
    DOCKET_RE.is_match(query)
}

pub fn has_date_reference(query: &str) -> bool {
    DATE_RE.is_match(query)
}

/// Distance cosinus (1 − cos) entre les centroïdes des deux jeux d'embeddings.
///
/// `ann_emb` / `bm25_emb` sont des lignes (un vecteur par chunk). Vide → 0.0,
/// fidèle au `ann_emb.size == 0` Python.
pub fn compute_d_centroids(ann_emb: &[Vec<f32>], bm25_emb: &[Vec<f32>]) -> f64 {
    if ann_emb.is_empty() || bm25_emb.is_empty() {
        return 0.0;
    }
    let a = mean_axis0(ann_emb);
    let b = mean_axis0(bm25_emb);
    let a_norm = l2_norm(&a);
    let b_norm = l2_norm(&b);
    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(&b).map(|(x, y)| *x * *y).sum();
    1.0 - dot / (a_norm * b_norm)
}

/// 1 − moyenne des similarités cosinus pairwise (triangle supérieur, k=1).
pub fn compute_pair_mu(ann_emb: &[Vec<f32>]) -> f64 {
    let n = ann_emb.len();
    if n < 2 {
        return 0.0;
    }
    let normed: Vec<Vec<f64>> = ann_emb
        .iter()
        .map(|row| {
            let norm = l2_norm_f32(row);
            let denom = if norm == 0.0 { 1.0 } else { norm };
            row.iter().map(|x| (*x as f64) / denom).collect()
        })
        .collect();
    let mut sum = 0.0;
    let mut count = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let sim: f64 = normed[i].iter().zip(&normed[j]).map(|(x, y)| x * y).sum();
            sum += sim;
            count += 1;
        }
    }
    1.0 - sum / count as f64
}

fn mean_axis0(rows: &[Vec<f32>]) -> Vec<f64> {
    let dim = rows[0].len();
    let mut acc = vec![0.0f64; dim];
    for row in rows {
        for (a, x) in acc.iter_mut().zip(row) {
            *a += *x as f64;
        }
    }
    let n = rows.len() as f64;
    for a in &mut acc {
        *a /= n;
    }
    acc
}

fn l2_norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn l2_norm_f32(v: &[f32]) -> f64 {
    v.iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt()
}

/// Composition multiplicative des signaux à partir de [`BASE_WEIGHTS`].
pub fn compute_weights(query: &str, ann_emb: &[Vec<f32>], bm25_emb: &[Vec<f32>]) -> Weights {
    let mut w = BASE_WEIGHTS;

    if has_article_reference(query) {
        w = w.scale(0.2, 1.5, 1.0);
    } else {
        let d_cent = compute_d_centroids(ann_emb, bm25_emb);
        let p_mu = compute_pair_mu(ann_emb);
        if d_cent > D_CENTROIDS_THRESHOLD && p_mu < PAIR_MU_THRESHOLD {
            w = w.scale(1.0, 0.4, 2.5);
        }
    }

    if has_docket_reference(query) {
        // 6.0 plutôt que 3.0 : le body BM25 noyait sinon le titre.
        w = w.scale(6.0, 1.0, 1.0);
    }

    if has_date_reference(query) {
        w = w.scale(1.5, 1.0, 1.0);
    }

    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_signal_detected() {
        assert!(has_article_reference("article L 531-1 du CESEDA"));
        assert!(has_article_reference("R. 222-1"));
        assert!(has_article_reference("D.521-2"));
        assert!(!has_article_reference("congés payés"));
    }

    #[test]
    fn docket_signal_all_formats() {
        assert!(has_docket_reference("pourvoi 07-11.687"));
        assert!(has_docket_reference("19-23.664"));
        assert!(has_docket_reference("19-23664"));
        assert!(has_docket_reference("23/01001"));
        assert!(has_docket_reference("requête 2202166"));
        assert!(!has_docket_reference("juste du texte"));
    }

    #[test]
    fn date_signal_fr_and_iso() {
        assert!(has_date_reference("13 février 2024"));
        assert!(has_date_reference("février 2024"));
        assert!(has_date_reference("2024-02-13"));
        assert!(has_date_reference("Cour de cassation 22 novembre 2000"));
        assert!(!has_date_reference("pas de date"));
    }

    #[test]
    fn article_scales_title_down_bm25_up() {
        let w = compute_weights("article L 531-1", &[], &[]);
        assert_eq!(w.title, 0.2);
        assert_eq!(w.bm25, 1.5);
        assert_eq!(w.ann, 1.0);
    }

    #[test]
    fn docket_and_date_cumulate() {
        // « Cour de cassation 22 novembre 2000 » → date ×1.5 ; pas de docket ici.
        let w = compute_weights("Cour de cassation 22 novembre 2000", &[], &[]);
        assert_eq!(w.title, 1.5);
    }

    #[test]
    fn base_weights_when_no_signal_no_embeddings() {
        let w = compute_weights("congés payés licenciement", &[], &[]);
        assert_eq!(w, BASE_WEIGHTS);
    }

    #[test]
    fn boost_ann_when_centroids_diverge_and_ann_tight() {
        // ann tight (vecteurs quasi colinéaires → pair_mu petit), bm25 ailleurs
        // (centroïdes éloignés → d_cent grand).
        let ann = vec![vec![1.0f32, 0.0, 0.0], vec![1.0, 0.01, 0.0]];
        let bm25 = vec![vec![0.0f32, 1.0, 0.0], vec![0.0, 1.0, 0.0]];
        let w = compute_weights("question naturelle", &ann, &bm25);
        assert_eq!(w.bm25, 0.4);
        assert_eq!(w.ann, 2.5);
    }

    // Parité détection de signaux ↔ oracle Python (apps/api signals.py). GT figée
    // dans tests/fixtures/oracle/signals_detect.json depuis `has_article_reference`/
    // `has_docket_reference`/`has_date_reference`. Batterie large des formats gap-prone
    // (articles L/R/D ± point ± espace, dockets CC ancien/nouveau + CA/TJ + admin,
    // dates FR jour/mois/an + mois/an + ISO, négatifs).
    #[derive(serde::Deserialize)]
    struct SigCase {
        query: String,
        article: bool,
        docket: bool,
        date: bool,
    }
    #[derive(serde::Deserialize)]
    struct SigFixture {
        cases: Vec<SigCase>,
    }

    #[test]
    fn detection_signal_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/signals_detect.json");
        let fix: SigFixture = serde_json::from_str(raw).expect("fixture signals_detect");
        for c in &fix.cases {
            assert_eq!(
                has_article_reference(&c.query),
                c.article,
                "article {:?}",
                c.query
            );
            assert_eq!(
                has_docket_reference(&c.query),
                c.docket,
                "docket {:?}",
                c.query
            );
            assert_eq!(has_date_reference(&c.query), c.date, "date {:?}", c.query);
        }
    }

    #[test]
    fn centroid_and_pair_mu_match_numpy_oracle() {
        // Valeurs figées depuis numpy (oracle) sur des embeddings exactement
        // représentables → verrouille la formule (1−cos des centroïdes ; 1−moyenne
        // des sim cosinus du triangle supérieur). Tolérance 1e-6 : numpy calcule en
        // float32 (les embeddings le sont), Rust upcaste en f64 → micro-écart ~2e-9,
        // très en deçà de la résolution qui influerait sur les seuils de pondération.
        let ann = vec![
            vec![1.0f32, 0.0, 0.0, 0.0],
            vec![0.5, 0.5, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
        ];
        let bm = vec![vec![0.0f32, 0.0, 1.0, 0.0], vec![0.0, 0.0, 0.5, 0.5]];
        assert!((compute_d_centroids(&ann, &bm) - 1.0).abs() < 1e-6);
        assert!((compute_pair_mu(&ann) - 0.5285954773426056).abs() < 1e-6);
    }

    // Parité de la composition complète des poids ↔ oracle Python (apps/api
    // signals.compute_weights). Couvre toute la matrice : aucun signal, article
    // (×0.2/×1.5, exclut boost_ann), docket (×6.0), date (×1.5), cumuls
    // docket+date (×9.0) et article+docket+date, plus la géométrie embeddings
    // (boost_ann ann×2.5/bm25×0.4 quand centroïdes divergent ET ann resserré ;
    // ignoré si article présent OU centroïdes confondus). GT figée dans
    // tests/fixtures/oracle/signals_weights.json.
    #[derive(serde::Deserialize)]
    struct WeightCase {
        name: String,
        query: String,
        ann_emb: Vec<Vec<f32>>,
        bm25_emb: Vec<Vec<f32>>,
        title: f64,
        bm25: f64,
        ann: f64,
    }
    #[derive(serde::Deserialize)]
    struct WeightFixture {
        cases: Vec<WeightCase>,
    }

    #[test]
    fn compute_weights_full_matrix_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/signals_weights.json");
        let fix: WeightFixture = serde_json::from_str(raw).expect("fixture signals_weights");
        for c in &fix.cases {
            let w = compute_weights(&c.query, &c.ann_emb, &c.bm25_emb);
            // Mêmes opérations IEEE f64 dans le même ordre des deux côtés : on
            // attend l'égalité au bit près, ε de garde contre toute dérive.
            assert!(
                (w.title - c.title).abs() < 1e-12,
                "{} title {} != {}",
                c.name,
                w.title,
                c.title
            );
            assert!(
                (w.bm25 - c.bm25).abs() < 1e-12,
                "{} bm25 {} != {}",
                c.name,
                w.bm25,
                c.bm25
            );
            assert!(
                (w.ann - c.ann).abs() < 1e-12,
                "{} ann {} != {}",
                c.name,
                w.ann,
                c.ann
            );
        }
    }
}
