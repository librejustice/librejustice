//! Cache référentiel process-local (ADR 0146 §4) : `facet_value` +
//! `jurisdiction` chargés depuis Postgres via `lj-store`, servis aux handlers
//! pour résoudre labels de facettes, tags des hits et arbre juridiction.
//!
//! Cache moka mono-entrée, TTL 1 h (aligné sur les caches search/rerank) : le
//! référentiel ne bouge qu'aux migrations/backfills ; un UPDATE de label est
//! visible au pire une heure plus tard, sans redéploiement.

use std::collections::HashMap;
use std::sync::Arc;

use lj_dtos::FacetTag;
use lj_store::repository::{DecisionRepository, FacetValueRow, JurisdictionRow};

use crate::error::{ApiError, Result};
use crate::state::AppState;

/// Une valeur de facette du référentiel (`facet_value`), indexée par uid.
#[derive(Debug, Clone)]
pub struct FacetValueEntry {
    pub facet: String,
    pub label: String,
    pub abbr: Option<String>,
    pub parent_uid: Option<String>,
}

/// Une unité juridictionnelle (`jurisdiction`), indexée par code.
#[derive(Debug, Clone)]
pub struct JurisdictionEntry {
    pub juridiction_type: String,
    pub label: String,
}

/// Vue en mémoire des référentiels de facettes (ADR 0146). Immutable après
/// chargement, partagée en `Arc` via le cache de l'[`AppState`].
#[derive(Debug, Default)]
pub struct Referential {
    values: HashMap<String, FacetValueEntry>,
    jurisdictions: HashMap<String, JurisdictionEntry>,
}

impl Referential {
    /// Construit la vue depuis les lignes DB (ou des fixtures de test).
    pub fn new(values: Vec<FacetValueRow>, jurisdictions: Vec<JurisdictionRow>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|r| {
                    (
                        r.uid,
                        FacetValueEntry {
                            facet: r.facet,
                            label: r.label,
                            abbr: r.abbr,
                            parent_uid: r.parent_uid,
                        },
                    )
                })
                .collect(),
            jurisdictions: jurisdictions
                .into_iter()
                .map(|r| {
                    (
                        r.code,
                        JurisdictionEntry {
                            juridiction_type: r.juridiction_type,
                            label: r.label,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Entrée `facet_value` d'un uid complet (`solution:REJET`).
    pub fn value(&self, uid: &str) -> Option<&FacetValueEntry> {
        self.values.get(uid)
    }

    /// Label FR d'un uid complet ; repli transparent sur le **suffixe** d'uid
    /// si l'entrée manque (cache d'une heure vs seed frais — pas de panique
    /// pour un affichage).
    pub fn label<'a>(&'a self, uid: &'a str) -> &'a str {
        self.values
            .get(uid)
            .map(|e| e.label.as_str())
            .unwrap_or_else(|| uid_suffix(uid))
    }

    /// Tag référentiel d'un uid complet : `key` = suffixe d'uid, `label` résolu.
    pub fn tag(&self, uid: &str) -> FacetTag {
        FacetTag {
            key: uid_suffix(uid).to_string(),
            label: self.label(uid).to_string(),
        }
    }

    /// Label d'un type de juridiction (`TJ` → « Tribunal judiciaire »), depuis
    /// les lignes `juridiction:*` (migration 0102).
    pub fn juridiction_type_label(&self, code: &str) -> Option<&str> {
        self.values
            .get(&format!("juridiction:{code}"))
            .map(|e| e.label.as_str())
    }

    /// Entrée `jurisdiction` d'un code (`tj76351`).
    pub fn jurisdiction(&self, code: &str) -> Option<&JurisdictionEntry> {
        self.jurisdictions.get(code)
    }
}

/// Suffixe d'un uid namespacé (`solution:REJET` → `REJET`).
pub fn uid_suffix(uid: &str) -> &str {
    uid.split_once(':').map(|(_, s)| s).unwrap_or(uid)
}

/// Référentiel depuis le cache de l'état (TTL 1 h), chargé au premier accès.
pub async fn referential(state: &AppState) -> Result<Arc<Referential>> {
    state
        .referential_cache
        .try_get_with((), load(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("referential load: {e}")))
}

async fn load(state: &AppState) -> Result<Arc<Referential>> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);
    let values = repo.load_facet_values().await.map_err(ApiError::Store)?;
    let jurisdictions = repo.load_jurisdictions().await.map_err(ApiError::Store)?;
    Ok(Arc::new(Referential::new(values, jurisdictions)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fv(uid: &str, label: &str, parent: Option<&str>) -> FacetValueRow {
        FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: parent.map(str::to_string),
            sort: 0,
        }
    }

    #[test]
    fn tag_resolves_label_and_falls_back_to_suffix() {
        let refs = Referential::new(
            vec![
                fv("solution:REJET", "Rejet", None),
                fv("juridiction:TJ", "Tribunal judiciaire", None),
            ],
            vec![JurisdictionRow {
                code: "tj76351".to_string(),
                juridiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
            }],
        );
        let tag = refs.tag("solution:REJET");
        assert_eq!((tag.key.as_str(), tag.label.as_str()), ("REJET", "Rejet"));
        // Uid inconnu : repli transparent sur le suffixe.
        let tag = refs.tag("solution:INCONNU");
        assert_eq!(tag.label, "INCONNU");
        assert_eq!(
            refs.juridiction_type_label("TJ"),
            Some("Tribunal judiciaire")
        );
        assert_eq!(
            refs.jurisdiction("tj76351").map(|j| j.label.as_str()),
            Some("Tribunal judiciaire du Havre")
        );
    }
}
