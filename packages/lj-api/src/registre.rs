//! The registry side of the entity page: SIRENE identity, RNE officers and
//! finances from recherche-entreprises.api.gouv.fr, and the BODACC notices of a
//! SIREN or the JOAFE notices of an RNA from the base, as the pot's `JournalHit`.
//! Cached 24 h in process (`AppState::registre_cache`); an API down leaves its
//! section empty, never an error. The JOAFE PDFs stay outgoing links.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lj_dtos::{
    EntityJournalGroupDto, EntityRegistreResponse, JournalHit, RegistreDirigeantDto,
    RegistreEntrepriseDto, RegistreFinanceDto, RegistreLienDto,
};
use lj_vocab::{JOURNAL_FOND_BODACC, JOURNAL_FOND_JOAFE};
use serde::Deserialize;
use tracing::instrument;

use crate::error::Result;
use crate::state::AppState;

/// Timeout par appel amont — au-delà, la section dégrade (absente du rendu).
pub(crate) const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(3);
/// Past this, a SUCCESSFUL upstream call leaves a line: far above the usual latency,
/// so nothing in normal operation, and an upstream drifting toward the budget shows
/// before it fails.
const UPSTREAM_LENT: Duration = Duration::from_millis(1000);
/// Annonces affichées sur la fiche (le total réel est renvoyé à part).
const JOURNALS_LIMIT: usize = 10;

const RECHERCHE_ENTREPRISES_HOST: &str = "recherche-entreprises.api.gouv.fr";
const RECHERCHE_ENTREPRISES: &str = "https://recherche-entreprises.api.gouv.fr/search";

/// recherche-entreprises resets about one call in 200 before answering it: the GET
/// goes again once, on a fresh connection, inside the same [`UPSTREAM_TIMEOUT`].
pub(crate) fn retry_policy() -> reqwest::retry::Builder {
    reqwest::retry::for_host(RECHERCHE_ENTREPRISES_HOST)
        .max_retries_per_request(1)
        .classify_fn(|attempt| {
            if attempt.error().is_some_and(is_reset) {
                attempt.retryable()
            } else {
                attempt.success()
            }
        })
}

fn is_reset(e: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(e), |e| e.source()).any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionReset)
    })
}

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
    let mut response = match state.registre_cache.get(&key).await {
        Some(hit) => (*hit).clone(),
        None => {
            let (response, complete) = match ns {
                "siren" => registre_entreprise(state, id).await,
                "rna" => registre_association(state, id).await,
                _ => (empty(), true),
            };
            if complete {
                state
                    .registre_cache
                    .insert(key.clone(), Arc::new(response.clone()))
                    .await;
            }
            response
        }
    };
    // The contracts join the list outside the 24 h cache: one closed or new
    // shows at the next harvest. They come first.
    let mut groupes = crate::journals::entity_groups(state, &key).await?;
    groupes.append(&mut response.journals);
    response.journals = groupes;
    Ok(response)
}

fn empty() -> EntityRegistreResponse {
    EntityRegistreResponse {
        entreprise: None,
        journals: Vec::new(),
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
    let liens = vec![RegistreLienDto {
        label: "Annuaire des entreprises".to_string(),
        url: format!("https://annuaire-entreprises.data.gouv.fr/entreprise/{siren}"),
    }];
    let (entreprise, bodacc) = tokio::join!(
        fetch_recherche_entreprises(state, &siren),
        crate::journals_bodacc::pour_entite(state, &siren, JOURNALS_LIMIT)
    );
    let complete = entreprise.is_ok() && bodacc.is_ok();
    (
        EntityRegistreResponse {
            entreprise: entreprise.unwrap_or(None),
            journals: groupe_live(JOURNAL_FOND_BODACC, bodacc.unwrap_or_default()),
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

// ── Associations (`rna:`) ─────────────────────────────────────────────────────

/// Annonces JOAFE d'un RNA (création, modification, dissolution, comptes),
/// avec le PDF officiel DILA en lien direct.
async fn registre_association(state: &AppState, id: &str) -> (EntityRegistreResponse, bool) {
    // An RNA is `W` and alphanumerics, upper case as the base writes it.
    let rna: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_uppercase();
    if rna.is_empty() {
        return (empty(), true);
    }
    let fetched = crate::journals_joafe::pour_entite(state, &rna, JOURNALS_LIMIT).await;
    let complete = fetched.is_ok();
    (
        EntityRegistreResponse {
            entreprise: None,
            journals: groupe_live(JOURNAL_FOND_JOAFE, fetched.unwrap_or_default()),
            liens: Vec::new(),
        },
        complete,
    )
}

/// Un fond live en groupe de la fiche : sa clé est le fond, son libellé celui
/// du pot. Aucune ligne → aucun groupe (une section vide n'est pas une info).
fn groupe_live(fond: &str, (items, total): (Vec<JournalHit>, i64)) -> Vec<EntityJournalGroupDto> {
    if items.is_empty() {
        return Vec::new();
    }
    vec![EntityJournalGroupDto {
        key: fond.to_string(),
        label: lj_dtos::journal_fond_label(fond)
            .unwrap_or(fond)
            .to_string(),
        total,
        items,
    }]
}

// ── Transport ────────────────────────────────────────────────────────────────

fn upstream_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_status() {
        "status"
    } else if e.is_decode() {
        "decode"
    } else if e.is_body() {
        "body"
    } else {
        "other"
    }
}

/// Chaîne d'erreur complète. Le `Display` de `reqwest::Error` s'arrête à
/// « error sending request for url (…) » et range la cause réelle dans
/// `source()` : sans la dérouler, un DNS mort, un TCP refusé et une poignée de
/// main TLS ratée s'écrivent pareil dans le log.
fn chain(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(&format!(" -> {s}"));
        src = s.source();
    }
    out
}

pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(
    state: &AppState,
    url: &str,
    query: &[(&str, &str)],
) -> std::result::Result<T, ()> {
    // Sans la durée, un budget épuisé et un TCP refusé s'écrivent pareil, et
    // l'épisode suivant se rediscute sans chiffre.
    let t = std::time::Instant::now();
    let response = state
        .registre_http
        .get(url)
        .query(query)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| {
            tracing::warn!(
                url,
                kind = upstream_kind(&e),
                elapsed_ms = t.elapsed().as_millis() as u64,
                error = %chain(&e),
                "registre upstream failed"
            )
        })?;
    let elapsed = t.elapsed();
    if elapsed >= UPSTREAM_LENT {
        tracing::warn!(
            url,
            elapsed_ms = elapsed.as_millis() as u64,
            "registre upstream lent"
        );
    }
    response
        .json::<T>()
        .await
        .map_err(|e| tracing::warn!(url, error = %e, "registre payload invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(dto.dirigeants.len(), 2);
        assert_eq!(dto.dirigeants[0].nom, "JEAN MADELEINE");
        assert_eq!(dto.dirigeants[1].nom, "HOLDING X");
        assert_eq!(dto.finances[0].annee, "2023");
        assert_eq!(dto.finances[0].resultat_net, Some(-123_456));
    }

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
