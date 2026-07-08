//! Quantisation / sérialisation de vecteurs (port de `embedding/quantize.py`).
//!
//! Format cible : littéral JSON `[v1,…,vN]` à 6 décimales, vecteurs
//! L2-normalisés. Sert aux casts SQL `::vector` / `quantize_to_rabitq8`.

use crate::error::{EmbedError, Result};
use ndarray::Array1;

pub const EMBEDDING_DIM: usize = 1024;

/// `v / ‖v‖₂` en f32 ; renvoie `v` inchangé si la norme est sous `eps`.
///
/// Port fidèle de `l2_normalize` (Python) : la norme est calculée en f32
/// (comme `numpy.linalg.norm` sur un array float32) puis le vecteur est divisé
/// élément par élément. Vecteur nul (norme `< eps`) renvoyé inchangé.
pub fn l2_normalize(v: &Array1<f32>, eps: f32) -> Array1<f32> {
    let norm = norm_f32(v);
    if norm < eps {
        return v.clone();
    }
    v.mapv(|x| x / norm)
}

/// `‖v‖₂` calculée en f32 (sum of squares puis racine), comme numpy float32.
fn norm_f32(v: &Array1<f32>) -> f32 {
    let sumsq: f32 = v.iter().map(|x| x * x).sum();
    sumsq.sqrt()
}

/// Sérialise un vecteur 1D en littéral JSON (`[v1,…,vN]`, 6 décimales) pour les
/// casts SQL `::vector` / `quantize_to_rabitq8`. Contrôle la dimension.
///
/// Port de `to_vector_json` : `f"{x:.6f}"` côté Python ↔ `{:.6}` formaté à
/// 6 décimales fixes côté Rust.
pub fn to_vector_json(v: &Array1<f32>, dim: usize) -> Result<String> {
    if v.len() != dim {
        return Err(EmbedError::Invalid(format!(
            "dimension attendue {dim}, reçu {}",
            v.len()
        )));
    }
    let mut out = String::with_capacity(dim * 10 + 2);
    out.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("{x:.6}"));
    }
    out.push(']');
    Ok(out)
}

/// Compose `l2_normalize` + `to_vector_json` — helper d'usage courant.
pub fn prepare(v: &Array1<f32>, dim: usize) -> Result<String> {
    to_vector_json(&l2_normalize(v, 1e-12), dim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::array;

    #[test]
    fn l2_normalize_unit_vector() {
        let v = array![3.0_f32, 4.0];
        let n = l2_normalize(&v, 1e-12);
        // 3-4-5 triangle → [0.6, 0.8].
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!((n[1] - 0.8).abs() < 1e-6);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_unchanged() {
        let v = array![0.0_f32, 0.0, 0.0];
        let n = l2_normalize(&v, 1e-12);
        assert_eq!(n, v);
    }

    #[test]
    fn to_vector_json_format() {
        // 6 décimales fixes, séparées par des virgules, entre crochets.
        let v = array![0.5_f32, -0.25, 1.0];
        let s = to_vector_json(&v, 3).unwrap();
        assert_eq!(s, "[0.500000,-0.250000,1.000000]");
    }

    #[test]
    fn to_vector_json_dim_mismatch() {
        let v = array![1.0_f32, 2.0];
        assert!(to_vector_json(&v, 3).is_err());
    }

    #[test]
    fn prepare_normalizes_and_serializes() {
        let v = array![3.0_f32, 4.0];
        let s = prepare(&v, 2).unwrap();
        assert_eq!(s, "[0.600000,0.800000]");
    }

    // Parité du format `to_vector_json` ↔ oracle Python (`f"{x:.6f}"`). Ce littéral
    // alimente directement les casts SQL `::vector` / `quantize_to_rabitq8` : un
    // écart d'arrondi décale la quantisation. Cas gap-prone figés depuis l'oracle :
    // demi-pas (round-half-even), zéro négatif, minuscules arrondis à ±0.000000,
    // troncature f32 visible (123.456789 → 123.456787). GT figée dans
    // tests/fixtures/oracle/quantize_vector_json.json.
    #[derive(serde::Deserialize)]
    struct VecCase {
        name: String,
        values: Vec<f32>,
        dim: usize,
        expected: String,
    }
    #[derive(serde::Deserialize)]
    struct VecFixture {
        cases: Vec<VecCase>,
    }

    #[test]
    fn to_vector_json_format_parity_oracle() {
        let raw = include_str!("../tests/fixtures/oracle/quantize_vector_json.json");
        let fix: VecFixture = serde_json::from_str(raw).expect("fixture quantize_vector_json");
        for c in &fix.cases {
            let v = Array1::from(c.values.clone());
            let got = to_vector_json(&v, c.dim).expect("to_vector_json");
            assert_eq!(got, c.expected, "case {}", c.name);
        }
    }
}
