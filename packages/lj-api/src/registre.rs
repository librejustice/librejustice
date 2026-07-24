//! Volet registre de la fiche entité (ADR 0199) : proxy mince vers les APIs
//! publiques — recherche-entreprises.api.gouv.fr (identité SIRENE + dirigeants
//! RNE + finances) et Opendatasoft DILA (chronologie BODACC par SIREN, annonces
//! JOAFE par RNA). Aucun stock local : cache in-process 24 h
//! (`AppState::registre_cache`) et dégradation propre — API indisponible →
//! section vide, jamais d'erreur. Les PDF (JOAFE, INPI) restent des liens
//! sortants, jamais proxifiés.
//!
//! Sondes, quotas et cartographie section ↔ API : working-note
//! `2026-07-11_annuaire-registres-apis-affichage.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lj_dtos::{
    EntityRegistreResponse, RegistreAnnonceDto, RegistreDirigeantDto, RegistreEntrepriseDto,
    RegistreFinanceDto, RegistreLienDto,
};
use serde::Deserialize;
use tracing::instrument;

use crate::error::Result;
use crate::state::AppState;

/// Timeout par appel amont — au-delà, la section dégrade (absente du rendu).
pub(crate) const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(3);
/// Annonces affichées sur la fiche (le total réel est renvoyé à part).
const ANNONCES_LIMIT: usize = 10;

const RECHERCHE_ENTREPRISES: &str = "https://recherche-entreprises.api.gouv.fr/search";
const BODACC_RECORDS: &str =
    "https://bodacc-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/annonces-commerciales/records";
const JOAFE_RECORDS: &str =
    "https://journal-officiel-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/jo_associations/records";

/// Volet registre d'une entité (`GET /entity/{ns}/{id}/registre`). Namespaces
/// sans volet (`cnb:`, `oacc:`…) → réponse vide. Seules les réponses dont tous
/// les appels amont ont réussi sont mises en cache : un upstream en panne est
/// retenté au rendu suivant, pas figé 24 h.
#[instrument(skip(state), fields(uid = %format!("{ns}:{id}")))]
pub async fn entity_registre(
    state: &AppState,
    ns: &str,
    id: &str,
) -> Result<EntityRegistreResponse> {
    let key = format!("{ns}:{id}");
    if let Some(hit) = state.registre_cache.get(&key).await {
        return Ok((*hit).clone());
    }
    let (response, complete) = match ns {
        "siren" => registre_entreprise(state, id).await,
        "rna" => registre_association(state, id).await,
        _ => (empty(), true),
    };
    if complete {
        state
            .registre_cache
            .insert(key, Arc::new(response.clone()))
            .await;
    }
    Ok(response)
}

fn empty() -> EntityRegistreResponse {
    EntityRegistreResponse {
        entreprise: None,
        annonces: Vec::new(),
        annonces_total: 0,
        liens: Vec::new(),
    }
}

// ── Entreprises (`siren:`) ────────────────────────────────────────────────────

/// Fiche entreprise : identité/dirigeants/finances (recherche-entreprises) et
/// chronologie BODACC, interrogés en parallèle. Retourne `(réponse, complet)` —
/// `complet = false` si un amont a échoué (réponse partielle, non cachée).
async fn registre_entreprise(state: &AppState, id: &str) -> (EntityRegistreResponse, bool) {
    // Frontière de validation : un SIREN est 9 chiffres, tout autre id ne
    // correspond à rien dans les APIs amont.
    let siren: String = id.chars().filter(char::is_ascii_digit).collect();
    if siren.len() != 9 {
        return (empty(), true);
    }
    let liens = vec![
        RegistreLienDto {
            label: "Annuaire des entreprises".to_string(),
            url: format!("https://annuaire-entreprises.data.gouv.fr/entreprise/{siren}"),
        },
        RegistreLienDto {
            label: "Documents et actes (INPI)".to_string(),
            url: format!("https://data.inpi.fr/entreprises/{siren}#documents"),
        },
    ];
    let (entreprise, bodacc) = tokio::join!(
        fetch_recherche_entreprises(state, &siren),
        fetch_bodacc(state, &siren)
    );
    let complete = entreprise.is_ok() && bodacc.is_ok();
    let (annonces, annonces_total) = bodacc.unwrap_or_default();
    (
        EntityRegistreResponse {
            entreprise: entreprise.unwrap_or(None),
            annonces,
            annonces_total,
            liens,
        },
        complete,
    )
}

/// Payload recherche-entreprises (champs consommés uniquement — l'API en
/// renvoie des dizaines d'autres, ignorés par serde).
#[derive(Deserialize)]
struct ReSearch {
    #[serde(default)]
    results: Vec<ReResult>,
}

#[derive(Deserialize)]
struct ReResult {
    siren: Option<String>,
    siege: Option<ReSiege>,
    activite_principale: Option<String>,
    date_creation: Option<String>,
    tranche_effectif_salarie: Option<String>,
    // L'API renvoie `null` (pas une valeur vide) quand la donnée RNE manque —
    // `serde(default)` ne couvre que le champ absent, d'où l'Option.
    dirigeants: Option<Vec<ReDirigeant>>,
    finances: Option<HashMap<String, ReFinance>>,
}

#[derive(Deserialize)]
struct ReSiege {
    adresse: Option<String>,
}

#[derive(Deserialize)]
struct ReDirigeant {
    nom: Option<String>,
    prenoms: Option<String>,
    qualite: Option<String>,
    /// Personne morale dirigeante (holding…) : dénomination à la place de
    /// nom/prénoms.
    denomination: Option<String>,
}

#[derive(Deserialize)]
struct ReFinance {
    ca: Option<serde_json::Number>,
    resultat_net: Option<serde_json::Number>,
}

/// Identité + dirigeants + finances. `Ok(None)` = SIREN inconnu de l'API
/// (diffusion protégée, entité radiée avant SIRENE…) — dégradation normale.
async fn fetch_recherche_entreprises(
    state: &AppState,
    siren: &str,
) -> std::result::Result<Option<RegistreEntrepriseDto>, ()> {
    let payload: ReSearch = get_json(
        state,
        RECHERCHE_ENTREPRISES,
        &[("q", siren), ("page", "1"), ("per_page", "1")],
    )
    .await?;
    // `q` est un moteur de recherche : ne garder que la correspondance exacte.
    let Some(result) = payload
        .results
        .into_iter()
        .find(|r| r.siren.as_deref() == Some(siren))
    else {
        return Ok(None);
    };
    Ok(Some(present_entreprise(result)))
}

fn present_entreprise(result: ReResult) -> RegistreEntrepriseDto {
    let dirigeants = result
        .dirigeants
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| {
            let nom = match (d.denomination, d.prenoms, d.nom) {
                (Some(denomination), _, _) => denomination,
                (None, Some(prenoms), Some(nom)) => format!("{prenoms} {nom}"),
                (None, None, Some(nom)) => nom,
                _ => return None,
            };
            Some(RegistreDirigeantDto {
                nom,
                qualite: d.qualite,
            })
        })
        .collect();
    let mut finances: Vec<RegistreFinanceDto> = result
        .finances
        .unwrap_or_default()
        .into_iter()
        .map(|(annee, f)| RegistreFinanceDto {
            annee,
            chiffre_affaires: f.ca.and_then(|n| n.as_i64()),
            resultat_net: f.resultat_net.and_then(|n| n.as_i64()),
        })
        .collect();
    finances.sort_by(|a, b| b.annee.cmp(&a.annee));
    RegistreEntrepriseDto {
        siege_adresse: result.siege.and_then(|s| s.adresse),
        activite_naf: result.activite_principale,
        date_creation: result.date_creation,
        effectif: result
            .tranche_effectif_salarie
            .as_deref()
            .and_then(effectif_label)
            .map(str::to_string),
        dirigeants,
        finances,
    }
}

/// Libellé d'une tranche d'effectif salarié INSEE (nomenclature
/// `tranche_effectif_salarie` SIRENE). `NN`/inconnue → `None` (non affichée).
fn effectif_label(code: &str) -> Option<&'static str> {
    Some(match code {
        "00" => "0 salarié",
        "01" => "1 ou 2 salariés",
        "02" => "3 à 5 salariés",
        "03" => "6 à 9 salariés",
        "11" => "10 à 19 salariés",
        "12" => "20 à 49 salariés",
        "21" => "50 à 99 salariés",
        "22" => "100 à 199 salariés",
        "31" => "200 à 249 salariés",
        "32" => "250 à 499 salariés",
        "41" => "500 à 999 salariés",
        "42" => "1 000 à 1 999 salariés",
        "51" => "2 000 à 4 999 salariés",
        "52" => "5 000 à 9 999 salariés",
        "53" => "10 000 salariés et plus",
        _ => return None,
    })
}

/// Payload Opendatasoft (BODACC et JOAFE partagent la même enveloppe v2.1 ;
/// `total_count` et `results` sont toujours présents).
#[derive(Deserialize)]
struct OdsRecords<T> {
    total_count: i64,
    results: Vec<T>,
}

#[derive(Deserialize)]
struct BodaccRecord {
    dateparution: Option<String>,
    familleavis_lib: Option<String>,
}

/// Chronologie BODACC d'un SIREN (procédures collectives, modifications,
/// radiations, dépôts de comptes), plus récente d'abord. Le champ `registre`
/// est composite (`siren,greffe`) → `like`.
async fn fetch_bodacc(
    state: &AppState,
    siren: &str,
) -> std::result::Result<(Vec<RegistreAnnonceDto>, i64), ()> {
    let where_clause = format!("registre like \"{siren}\"");
    let limit = ANNONCES_LIMIT.to_string();
    let payload: OdsRecords<BodaccRecord> = get_json(
        state,
        BODACC_RECORDS,
        &[
            ("where", where_clause.as_str()),
            ("order_by", "dateparution desc"),
            ("select", "dateparution,familleavis_lib"),
            ("limit", limit.as_str()),
        ],
    )
    .await?;
    let annonces = payload
        .results
        .into_iter()
        .filter_map(|r| {
            Some(RegistreAnnonceDto {
                date: r.dateparution,
                famille: r.familleavis_lib?,
                url_pdf: None,
            })
        })
        .collect();
    Ok((annonces, payload.total_count))
}

// ── Associations (`rna:`) ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JoafeRecord {
    dateparution: Option<String>,
    typeavis: Option<String>,
    url_pdf: Option<String>,
}

/// Annonces JOAFE d'un RNA (création, modification, dissolution, comptes),
/// avec le PDF officiel DILA en lien direct.
async fn registre_association(state: &AppState, id: &str) -> (EntityRegistreResponse, bool) {
    // Frontière de validation : un RNA est `W` + chiffres (parfois un code
    // préfectoral alphanumérique) — on ne laisse passer que l'alphanumérique,
    // ce qui neutralise aussi toute injection dans la clause `where` ODS.
    let rna: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_uppercase();
    if rna.is_empty() {
        return (empty(), true);
    }
    let where_clause = format!("numero_rna=\"{rna}\"");
    let limit = ANNONCES_LIMIT.to_string();
    let fetched: std::result::Result<OdsRecords<JoafeRecord>, ()> = get_json(
        state,
        JOAFE_RECORDS,
        &[
            ("where", where_clause.as_str()),
            ("order_by", "dateparution desc"),
            ("select", "dateparution,typeavis,url_pdf"),
            ("limit", limit.as_str()),
        ],
    )
    .await;
    let complete = fetched.is_ok();
    let payload = fetched.unwrap_or(OdsRecords {
        total_count: 0,
        results: Vec::new(),
    });
    let annonces = payload
        .results
        .into_iter()
        .filter_map(|r| {
            Some(RegistreAnnonceDto {
                date: r.dateparution,
                famille: r.typeavis?,
                url_pdf: r.url_pdf,
            })
        })
        .collect();
    (
        EntityRegistreResponse {
            entreprise: None,
            annonces,
            annonces_total: payload.total_count,
            liens: Vec::new(),
        },
        complete,
    )
}

// ── Transport ────────────────────────────────────────────────────────────────

/// GET JSON avec l'échec réduit à `Err(())` : l'appelant dégrade, le détail
/// part en `warn` (une fiche ne doit jamais échouer parce qu'un registre
/// public est indisponible).
async fn get_json<T: serde::de::DeserializeOwned>(
    state: &AppState,
    url: &str,
    query: &[(&str, &str)],
) -> std::result::Result<T, ()> {
    let response = state
        .registre_http
        .get(url)
        .query(query)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| tracing::warn!(url, error = %e, "registre upstream failed"))?;
    response
        .json::<T>()
        .await
        .map_err(|e| tracing::warn!(url, error = %e, "registre payload invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verrouille le mapping du payload recherche-entreprises (forme réelle
    /// observée le 2026-07-11, valeurs fictives, réduit aux champs consommés) :
    /// dirigeants personne physique/morale, finances triées, tranche
    /// d'effectif résolue.
    #[test]
    fn presente_une_entreprise_depuis_le_payload_recherche_entreprises() {
        let raw = serde_json::json!({
            "siren": "123456780",
            "champ_inconnu": {"ignore": true},
            "siege": {"adresse": "55 RUE PLUMET 75007 PARIS", "autre": 1},
            "activite_principale": "62.01Z",
            "date_creation": "2018-08-01",
            "tranche_effectif_salarie": "12",
            "dirigeants": [
                {"nom": "MADELEINE", "prenoms": "JEAN", "qualite": "Président de SAS"},
                {"denomination": "HOLDING X", "qualite": "Président", "siren": "123456789"},
                {"qualite": "orphelin sans nom"}
            ],
            "finances": {
                "2022": {"ca": 100, "resultat_net": -5},
                "2023": {"ca": 0, "resultat_net": -123456}
            }
        });
        let result: ReResult = serde_json::from_value(raw).unwrap();
        let dto = present_entreprise(result);
        assert_eq!(
            dto.siege_adresse.as_deref(),
            Some("55 RUE PLUMET 75007 PARIS")
        );
        assert_eq!(dto.activite_naf.as_deref(), Some("62.01Z"));
        assert_eq!(dto.effectif.as_deref(), Some("20 à 49 salariés"));
        // Le dirigeant sans nom ni dénomination est écarté, pas paniqué.
        assert_eq!(dto.dirigeants.len(), 2);
        assert_eq!(dto.dirigeants[0].nom, "JEAN MADELEINE");
        assert_eq!(dto.dirigeants[1].nom, "HOLDING X");
        // Finances plus récentes d'abord.
        assert_eq!(dto.finances[0].annee, "2023");
        assert_eq!(dto.finances[0].resultat_net, Some(-123_456));
    }

    /// L'API renvoie `null` (pas un champ absent) quand la donnée RNE manque —
    /// observé en prod le 2026-07-11 : le décodage doit passer et produire
    /// une fiche sans dirigeants ni finances.
    #[test]
    fn decode_les_champs_rne_null() {
        let raw = serde_json::json!({
            "siren": "123456782",
            "siege": {"adresse": "1 RUE X 75001 PARIS"},
            "activite_principale": "70.10Z",
            "date_creation": "1988-01-01",
            "tranche_effectif_salarie": null,
            "dirigeants": null,
            "finances": null
        });
        let result: ReResult = serde_json::from_value(raw).unwrap();
        let dto = present_entreprise(result);
        assert!(dto.dirigeants.is_empty());
        assert!(dto.finances.is_empty());
        assert_eq!(dto.siege_adresse.as_deref(), Some("1 RUE X 75001 PARIS"));
    }
}
