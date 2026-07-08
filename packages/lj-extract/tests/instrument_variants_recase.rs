//! Oracle de régression normalisation (ADR 0077) : 311 variantes d'instruments
//! réellement vues en prod (≥10 refs), mal orthographiées / tronquées / à prose
//! collée, classées par subagents puis auditées Opus. Chacune DOIT se recoller
//! à son code canonique via `normalize_instrument` (règle squelette + table
//! d'alias embarquée). Fige les ~96 k références prod ainsi récupérées.

use serde::Deserialize;

#[derive(Deserialize)]
struct Variant {
    instrument: String,
    code: String,
}

#[test]
fn audited_variants_normalize_to_canonical_code() {
    let raw = include_str!("fixtures/instrument_variants.json");
    let variants: Vec<Variant> = serde_json::from_str(raw).expect("fixture valide");
    assert_eq!(
        variants.len(),
        311,
        "fixture attendue : 311 variantes auditées"
    );
    let mut fails = Vec::new();
    for v in &variants {
        let got = lj_extract::extract::normalize_instrument(&v.instrument);
        if got != v.code {
            fails.push(format!(
                "{:?} → {:?} (attendu {:?})",
                v.instrument, got, v.code
            ));
        }
    }
    assert!(
        fails.is_empty(),
        "{} variantes non recollées :\n{}",
        fails.len(),
        fails.join("\n")
    );
}
