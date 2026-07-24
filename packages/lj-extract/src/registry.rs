//! Registre de parties (ADR 0175 V0, ontologie ADR 0180) : le point de
//! convergence des moissons NER. Une entité = (valeur, côté, qualité) ; les
//! champs plats (`companies`, `counsel`, `intervenors`) sont des projections
//! [`PartyRegistry::view`]. V0 réindexe les moissons par gabarit telles
//! quelles ; les vagues V1+ (fusion de coréférences, côté par force,
//! résolution vers les référentiels ADR 0179) réécrivent la construction
//! sans toucher les consommateurs.

/// Côté procédural, normalisé par degré (appelant→demandeur…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Applicant,
    Defendant,
}

/// Qualité de l'entité dans l'instance (axe Rôle de l'ontologie 0180,
/// restreint aux cellules émises aujourd'hui).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Partie personne morale.
    Party,
    /// Structure d'exercice du conseil (SCP, SELARL…).
    LawFirm,
    /// Avocat nommé.
    CounselName,
    /// Intervenant (côté optionnel, structurellement `None` en V0).
    Intervenor,
}

/// Une entité du document : tranche verbatim + coordonnées (côté, qualité).
#[derive(Debug, Clone)]
pub struct Entity {
    pub value: String,
    pub side: Option<Side>,
    pub quality: Quality,
}

/// Registre par document, mémoïsé sur `DocScan` — construit UNE fois, servi
/// à tous les champs.
#[derive(Debug, Clone, Default)]
pub struct PartyRegistry {
    pub entities: Vec<Entity>,
}

impl PartyRegistry {
    pub(crate) fn push(&mut self, values: Vec<String>, side: Option<Side>, quality: Quality) {
        self.entities.extend(values.into_iter().map(|value| Entity {
            value,
            side,
            quality,
        }));
    }

    /// Projection plate d'une cellule (côté, qualité), en ordre d'insertion.
    pub fn view(&self, side: Option<Side>, quality: Quality) -> Vec<String> {
        self.entities
            .iter()
            .filter(|e| e.side == side && e.quality == quality)
            .map(|e| e.value.clone())
            .collect()
    }
}
