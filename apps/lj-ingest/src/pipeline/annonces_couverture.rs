//! Complétude de la moisson des profils d'acheteurs, mesurée par les canaux de
//! publication.
//!
//! La moisson lit les plateformes ; trois canaux indépendants lisent ce que les
//! acheteurs publient — BOAMP (au-dessus des seuils), TED (JOUE), presse
//! (annonces légales). Une consultation ouverte que seuls ces canaux portent est
//! un trou : soit le tenant n'est pas recensé, soit son moteur n'est pas
//! connecté, soit le tenant est bien moissonné et la ligne vit quand même
//! seule — et ce dernier cas est celui qu'il faut aller regarder de près.
//!
//! Rapport pur : rien n'est écrit, tout est lu depuis le stock.

use std::collections::{BTreeMap, HashMap};

use anyhow::{anyhow, Result};

use lj_sources_annonces::discover::{canon_host, MOTEURS_CONNECTES};
use lj_store::repository::DecisionRepository;
use lj_vocab::JOURNAL_FOND_MARCHE;

use crate::config::Settings;

/// L'état d'un tenant vis-à-vis de la moisson — l'ordre est celui du rapport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Etat {
    /// Tenant qu'un connecteur énumère : la ligne aurait dû se fondre dans une
    /// ligne de plateforme. Deux causes, que ce comptage ne départage pas — la
    /// moisson a manqué la consultation, ou elle l'a bien mais le jumelage n'a
    /// pas conclu.
    TenantMoissonne,
    /// Hôte recensé, moteur identifié mais sans connecteur (agysoft,
    /// centraledesmarches…).
    MoteurNonConnecte,
    /// Hôte recensé, moteur jamais identifié par la sonde.
    MoteurInconnu,
    /// Hôte absent du recensement : `annonces-discover` ne l'a jamais classé.
    TenantInconnu,
    /// L'avis ne nomme aucune plateforme : rien à moissonner, rien à conclure.
    SansDepot,
}

impl Etat {
    fn nom(self) -> &'static str {
        match self {
            Etat::TenantMoissonne => "tenant_moissonne",
            Etat::MoteurNonConnecte => "moteur_non_connecte",
            Etat::MoteurInconnu => "moteur_inconnu",
            Etat::TenantInconnu => "tenant_inconnu",
            Etat::SansDepot => "sans_depot",
        }
    }
}

/// L'hôte d'une URL de dépôt, canonisé comme le recensement l'écrit.
fn hote(depot: &str) -> Option<String> {
    let brut = depot
        .split_once("://")
        .map_or(depot, |(_, r)| r)
        .split(['/', '?', '#'])
        .next()?
        .split(':')
        .next()?
        .trim_end_matches(',');
    (brut.contains('.')).then(|| canon_host(brut))
}

fn etat(depot: &str, moteurs: &HashMap<String, String>) -> (Etat, String) {
    let Some(h) = hote(depot) else {
        return (Etat::SansDepot, String::new());
    };
    match moteurs.get(&h) {
        None => (Etat::TenantInconnu, h),
        Some(m) if m.is_empty() => (Etat::MoteurInconnu, h),
        Some(m) if MOTEURS_CONNECTES.contains(&m.as_str()) => (Etat::TenantMoissonne, h),
        Some(_) => (Etat::MoteurNonConnecte, h),
    }
}

pub async fn annonces_couverture(limite: usize) -> Result<()> {
    let debut = std::time::Instant::now();
    let settings = Settings::from_env()?;
    let pool =
        lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let repo = DecisionRepository::new(&conn);

    // Le recensement porte les hôtes classés ; un hôte recensé sans moteur n'y
    // figure pas, d'où la chaîne vide qui distingue « pas de moteur » de
    // « pas recensé ».
    let mut moteurs: HashMap<String, String> = repo
        .moteurs_par_hote()
        .await
        .map_err(|e| anyhow!("moteurs par hôte: {e}"))?
        .into_iter()
        .collect();
    for h in repo
        .hotes_plateformes_classees()
        .await
        .map_err(|e| anyhow!("hôtes classés: {e}"))?
    {
        moteurs.entry(h).or_default();
    }

    let trous = repo
        .annonces_sans_plateforme(JOURNAL_FOND_MARCHE, &MOTEURS_CONNECTES)
        .await
        .map_err(|e| anyhow!("trous de couverture: {e}"))?;

    let mut par_etat: BTreeMap<Etat, i64> = BTreeMap::new();
    let mut par_tenant: BTreeMap<(Etat, String), (i64, String)> = BTreeMap::new();
    let mut par_canal: BTreeMap<String, i64> = BTreeMap::new();
    let mut total = 0;
    for t in &trous {
        let (etat, hote) = etat(&t.depot, &moteurs);
        total += t.lignes;
        *par_etat.entry(etat).or_default() += t.lignes;
        *par_canal.entry(t.canaux.clone()).or_default() += t.lignes;
        let e = par_tenant.entry((etat, hote)).or_default();
        e.0 += t.lignes;
        if e.1.is_empty() {
            e.1 = t.canaux.clone();
        }
    }

    // Les tenants du haut du tas : c'est là que se répare la couverture.
    let mut classement: Vec<((Etat, String), (i64, String))> = par_tenant.into_iter().collect();
    classement.sort_by_key(|t| std::cmp::Reverse(t.1 .0));
    for ((etat, hote), (lignes, canaux)) in classement.iter().take(limite) {
        tracing::info!(
            etat = etat.nom(),
            hote = %hote,
            lignes,
            canaux = %canaux,
            "annonces_couverture: tenant"
        );
    }

    tracing::info!(
        ouvertes_hors_plateforme = total,
        etats = par_etat
            .iter()
            .map(|(e, n)| format!("{}={n}", e.nom()))
            .collect::<Vec<_>>()
            .join(","),
        canaux = par_canal
            .iter()
            .map(|(c, n)| format!("{c}={n}"))
            .collect::<Vec<_>>()
            .join(","),
        tenants = classement.len(),
        elapsed_s = debut.elapsed().as_secs(),
        "annonces_couverture"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{etat, hote, Etat};
    use std::collections::HashMap;

    #[test]
    fn le_depot_se_reduit_a_l_hote_du_recensement() {
        assert_eq!(
            hote("https://WWW.Marches-Securises.fr/entreprise/?module=x").as_deref(),
            Some("marches-securises.fr")
        );
        assert_eq!(
            hote("place.gouv.fr").as_deref(),
            Some("marches-publics.gouv.fr")
        );
        assert_eq!(hote(""), None);
    }

    /// Les verdicts qui décident quoi réparer : recenser l'hôte, connecter le
    /// moteur, ou aller voir pourquoi une ligne d'un tenant moissonné vit seule.
    #[test]
    fn l_etat_d_un_tenant_dit_quoi_reparer() {
        let moteurs: HashMap<String, String> = [
            ("marches-securises.fr", "atline"),
            ("agysoft.marches-publics.info", "agysoft"),
            ("mystere.fr", ""),
        ]
        .into_iter()
        .map(|(h, m)| (h.to_string(), m.to_string()))
        .collect();
        assert_eq!(
            etat("https://marches-securises.fr/x", &moteurs).0,
            Etat::TenantMoissonne
        );
        assert_eq!(
            etat("https://agysoft.marches-publics.info/", &moteurs).0,
            Etat::MoteurNonConnecte
        );
        assert_eq!(etat("https://mystere.fr/", &moteurs).0, Etat::MoteurInconnu);
        assert_eq!(etat("https://inconnu.fr/", &moteurs).0, Etat::TenantInconnu);
        assert_eq!(etat("", &moteurs).0, Etat::SansDepot);
    }
}
