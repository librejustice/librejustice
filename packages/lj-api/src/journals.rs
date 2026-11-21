//! Pot « Annonces » : le moteur facetté multi-fonds, la fiche d'une
//! consultation, le catalogue de crawl par département, la section de la fiche
//! entité, et les flux d'alerte RSS/ICS.
//!
//! The `marche` fond is served here, `bodacc`, `joafe`, `jo` and `raa` by their own
//! module over their tables.
//!
//! Facets first, and no embeddings over one-line objects: `q` is a BM25 match of
//! every word on the title or the buyer. A page and its axes are served from
//! `journals_cache` (TTL 15 min).

use std::collections::HashSet;
use std::sync::Arc;

use lj_dtos::{
    journal_acteur_role, journal_fond_label, AnnonceCanalDto, AnnonceCatalogueResponse,
    AnnonceDetailResponse, AnnonceEntiteDto, AnnonceLienDto, AnnonceLotDto, AnnonceLotOuvertDto,
    AnnonceMontantDto, AnnonceResultatDto, AnnonceZoneEntry, EntityJournalGroupDto, FacetChoice,
    FacetTag, JournalFacetAxis, JournalFondGroup, JournalHit, JournalSearchRequest,
    JournalSearchResponse, JournalSort,
};
use lj_norm::referential_labels::departement_label;
use lj_store::repository::{
    AnnonceAxe, AnnonceFiltre, AnnonceOrdre, AnnonceRow, DecisionRepository, EntityAnnonceRow,
};
use lj_vocab::{
    zone_est_departement, JOURNAL_FOND_BODACC, JOURNAL_FOND_JO, JOURNAL_FOND_JOAFE,
    JOURNAL_FOND_MARCHE, JOURNAL_FOND_RAA, ZONE_EU, ZONE_FRANCE, ZONE_SANS,
};
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::journals_bodacc;
use crate::journals_jo;
use crate::journals_joafe;
use crate::journals_raa;
use crate::state::{AppState, PEAK_CONNS_PER_SEARCH};

/// Annonces par rôle sur une fiche entité.
const PAR_ROLE: i64 = 5;
/// Plafond d'items d'un flux.
pub const FEED_MAX_ITEMS: u32 = 100;

#[instrument(
    name = "annonces_search",
    skip(state, req),
    fields(fonds = %req.fond.join(","), total = tracing::field::Empty)
)]
pub async fn search(state: &AppState, req: &JournalSearchRequest) -> Result<JournalSearchResponse> {
    let key = cache_key(req);
    if let Some(hit) = state.journals_cache.get(&key).await {
        return Ok((*hit).clone());
    }
    let response = load_search(state, req).await?;
    tracing::Span::current().record("total", response.total);
    state
        .journals_cache
        .insert(key, Arc::new(response.clone()))
        .await;
    Ok(response)
}

/// Clé de cache : le filtre entier plus la fenêtre de pagination — deux pages du
/// même filtre sont deux entrées.
fn cache_key(req: &JournalSearchRequest) -> String {
    serde_json::to_string(req).unwrap_or_else(|_| format!("{req:?}"))
}

/// Les fonds du pot, dans l'ordre d'affichage — et la liste des jetons `fond`
/// que la frontière HTTP accepte.
pub const FONDS: [&str; 5] = [
    JOURNAL_FOND_MARCHE,
    JOURNAL_FOND_BODACC,
    JOURNAL_FOND_JOAFE,
    JOURNAL_FOND_JO,
    JOURNAL_FOND_RAA,
];

/// Lignes montrées par fond quand plusieurs fonds répondent.
const APERCU_PAR_FOND: u32 = 5;

/// The fonds this request reaches: the official journals always, the registers, France's,
/// under France alone and never under one journal named.
fn fonds_ouverts(req: &JournalSearchRequest) -> Vec<&'static str> {
    let france = req.home.as_deref().is_none_or(|h| h == "fr")
        && req
            .jurisdiction
            .as_deref()
            .is_none_or(|j| j.eq_ignore_ascii_case("FR"));
    FONDS
        .into_iter()
        .filter(|f| france || *f == JOURNAL_FOND_JO)
        .collect()
}

/// The fonds asked for this filter state: those the user ticked, else all of them, the JO
/// and the recueils only on a question, their acts naming persons the bare page would list.
fn fonds_actifs(req: &JournalSearchRequest) -> Vec<&'static str> {
    let ouverts = fonds_ouverts(req).into_iter();
    if req.fond.is_empty() {
        let question = req.q.as_deref().is_some_and(|q| !q.trim().is_empty());
        return ouverts
            .filter(|f| question || !matches!(*f, JOURNAL_FOND_JO | JOURNAL_FOND_RAA))
            .collect();
    }
    ouverts
        .filter(|f| req.fond.iter().any(|choisi| choisi == f))
        .collect()
}

/// Dispatch by fond. Each backend gives its page and its total, never
/// interleaved with another's; a backend down removes its group alone. The fonds
/// and the active fond's axes are asked at once, as many at a time as one search
/// holds connections.
async fn load_search(
    state: &AppState,
    req: &JournalSearchRequest,
) -> Result<JournalSearchResponse> {
    let actifs = fonds_actifs(req);
    let journals = journals_jo::journals(state, req).await?;
    let journals = journals.as_deref();
    // Plusieurs fonds ouverts = une vue d'aperçu : chaque groupe montre sa tête
    // de liste et son total, le « tout voir » du groupe coche le fond et rend
    // la page pleine.
    let apercu = actifs.len() > 1;
    let fenetre = JournalSearchRequest {
        limit: APERCU_PAR_FOND,
        offset: 0,
        ..req.clone()
    };
    let req_fond = if apercu { &fenetre } else { req };

    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    let conns = tokio::sync::Semaphore::new(PEAK_CONNS_PER_SEARCH);
    let conns = &conns;
    let actifs = &actifs;
    // An inactive fond is asked for its count alone, which fills the `Type` axis.
    let pages = futures::future::join_all(fonds_ouverts(req).into_iter().map(|fond| async move {
        let _conn = conns.acquire().await.expect("never closed");
        (
            fond,
            page_fond(state, req_fond, fond, journals, actifs.contains(&fond)).await,
        )
    }));
    let propres = async {
        let _conn = conns.acquire().await.expect("never closed");
        axes_propres(state, req, actifs).await
    };
    let (pages, propres) = tokio::join!(pages, propres);

    let mut groupes: Vec<JournalFondGroup> = Vec::new();
    let mut comptes: Vec<(&'static str, i64)> = Vec::new();
    for (fond, page) in pages {
        let (hits, total) = match page {
            Ok(page) => page,
            Err(e) => {
                tracing::warn!(fond, error = %e, "fond unavailable, group removed");
                continue;
            }
        };
        comptes.push((fond, total));
        if actifs.contains(&fond) {
            groupes.push(JournalFondGroup {
                fond: fond.to_string(),
                label: journal_fond_label(fond).unwrap_or(fond).to_string(),
                total,
                hits,
            });
        }
    }

    let total = groupes.iter().map(|g| g.total).sum();
    let mut axes = vec![axe_fond(&comptes)];
    axes.extend(propres);
    Ok(JournalSearchResponse {
        total,
        groupes,
        sort: req.sort,
        axes,
    })
}

/// A page of one fond and its total. `avec_hits` false asks for its total alone.
async fn page_fond(
    state: &AppState,
    req: &JournalSearchRequest,
    fond: &str,
    journals: Option<&[String]>,
    avec_hits: bool,
) -> Result<(Vec<JournalHit>, i64)> {
    if fond == JOURNAL_FOND_BODACC {
        return journals_bodacc::page(state, req, avec_hits).await;
    }
    if fond == JOURNAL_FOND_JOAFE {
        return journals_joafe::page(state, req, avec_hits).await;
    }
    if fond == JOURNAL_FOND_JO {
        return journals_jo::page(state, req, journals, avec_hits).await;
    }
    if fond == JOURNAL_FOND_RAA {
        return journals_raa::page(state, req, avec_hits).await;
    }
    let filtre = filtre_from(req, fond);
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let total = repo.annonces_total(&filtre).await?;
    if !avec_hits {
        return Ok((Vec::new(), total));
    }
    let rows = repo.annonces(&filtre).await?;
    Ok((rows.iter().map(hit_from).collect(), total))
}

/// The `Type` axis, always there: the way into the pot.
fn axe_fond(comptes: &[(&'static str, i64)]) -> JournalFacetAxis {
    JournalFacetAxis {
        key: "fond".to_string(),
        label: "Type".to_string(),
        choices: comptes
            .iter()
            .map(|(fond, count)| FacetChoice {
                value: (*fond).to_string(),
                label: journal_fond_label(fond).unwrap_or(fond).to_string(),
                count: *count,
                parent: None,
                hint: None,
            })
            .collect(),
    }
}

/// The axes of a fond selected alone, declared by its backend: a BODACC family
/// is no market nature.
async fn axes_propres(
    state: &AppState,
    req: &JournalSearchRequest,
    actifs: &[&'static str],
) -> Vec<JournalFacetAxis> {
    let [fond] = actifs else {
        return Vec::new();
    };
    let propres = match *fond {
        JOURNAL_FOND_BODACC => journals_bodacc::axes(state, req).await,
        JOURNAL_FOND_JOAFE => journals_joafe::axes(state, req).await,
        JOURNAL_FOND_JO => Ok(Vec::new()),
        JOURNAL_FOND_RAA => journals_raa::axes(state, req).await,
        stocke => axes_stockes(state, req, stocke).await,
    };
    propres.unwrap_or_else(|e| {
        tracing::warn!(fond, error = %e, "fond facets unavailable");
        Vec::new()
    })
}

async fn axes_stockes(
    state: &AppState,
    req: &JournalSearchRequest,
    fond: &'static str,
) -> Result<Vec<JournalFacetAxis>> {
    let filtre = filtre_from(req, fond);
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let (zone, famille, cpv, metier) = tokio::try_join!(
        repo.annonces_facette(&filtre, AnnonceAxe::Zone),
        repo.annonces_facette(&filtre, AnnonceAxe::Famille),
        repo.annonces_facette(&filtre, AnnonceAxe::Cpv),
        repo.annonces_facette(&filtre, AnnonceAxe::Metier),
    )?;
    let refs = crate::referential::referential(state).await?;
    let mut zone = JournalFacetAxis {
        key: "zone".to_string(),
        label: "Zone".to_string(),
        choices: par_poids(
            zone.into_iter()
                .map(|(code, _, count)| FacetChoice {
                    label: zone_nom(&code),
                    value: code,
                    count,
                    parent: None,
                    hint: None,
                })
                .collect(),
        ),
    };
    completer_selection(&mut zone, &req.zone, zone_nom);
    let mut cpv = JournalFacetAxis {
        key: "cpv".to_string(),
        label: "Domaine".to_string(),
        choices: sous_les_natures(famille, cpv),
    };
    completer_selection(&mut cpv, &req.cpv, |v| {
        let uid = match v.starts_with(NATURE_PREFIXE) {
            true => v.to_string(),
            false => format!("cpv:{v}"),
        };
        refs.label(&uid).to_string()
    });
    let mut metier = JournalFacetAxis {
        key: "metier".to_string(),
        label: "Métier".to_string(),
        choices: par_poids(choices_referentiel(metier)),
    };
    completer_selection(&mut metier, &req.metier, |v| {
        lj_annonces::metiers::libelle(v).to_string()
    });
    Ok(vec![zone, cpv, metier])
}

/// Rend à l'axe les valeurs cochées que son agrégat ne compte plus. Une facette
/// se calcule son propre filtre retiré, mais **pas** ceux des autres axes : le
/// CPV 71 coché sous « Guyane + Travaux » ne pèse aucune ligne, donc il sort du
/// `GROUP BY` et son libellé avec. Comptée pour zéro, la ligne se relit et se
/// décoche.
fn completer_selection(
    axe: &mut JournalFacetAxis,
    selection: &[String],
    label: impl Fn(&str) -> String,
) {
    for value in selection {
        if axe.choices.iter().any(|c| &c.value == value) {
            continue;
        }
        axe.choices.push(FacetChoice {
            label: label(value),
            value: value.clone(),
            count: 0,
            parent: None,
            hint: None,
        });
    }
}

/// Long axes read by weight: the rail shows their first lines alone.
fn par_poids(mut choices: Vec<FacetChoice>) -> Vec<FacetChoice> {
    choices.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    choices
}

/// Choix d'un axe référentiel : la **valeur de filtre est le suffixe d'uid**
/// (`TRAVAUX`, `45`), comme partout ailleurs dans l'API.
fn choices_referentiel(rows: Vec<(String, Option<String>, i64)>) -> Vec<FacetChoice> {
    choices_sous(rows, |_| None)
}

/// The domain facet, two levels in one axis, same shape as `juridiction`: the
/// root carries the full uid (`nature_marche:TRAVAUX`), the leaf the bare
/// division code with its root in `parent`. One key means one selection list,
/// and a division opens without emptying the natures beside it.
const NATURE_PREFIXE: &str = "nature_marche:";

fn sous_les_natures(
    natures: Vec<(String, Option<String>, i64)>,
    divisions: Vec<(String, Option<String>, i64)>,
) -> Vec<FacetChoice> {
    let racines: Vec<FacetChoice> = natures
        .into_iter()
        .map(|(uid, label, count)| FacetChoice {
            label: label.unwrap_or_else(|| uid.clone()),
            value: uid,
            count,
            parent: None,
            hint: None,
        })
        .collect();
    let feuilles = par_poids(choices_sous(divisions, |uid| {
        lj_annonces::cpv::nature_probable(uid).map(|n| n.facet_uid().to_string())
    }));
    let mut out = Vec::with_capacity(racines.len() + feuilles.len());
    for racine in racines {
        let sous: Vec<FacetChoice> = feuilles
            .iter()
            .filter(|f| f.parent.as_deref() == Some(racine.value.as_str()))
            .cloned()
            .collect();
        out.push(racine);
        out.extend(sous);
    }
    let orphelines: Vec<FacetChoice> = feuilles
        .into_iter()
        .filter(|f| !out.iter().any(|c| c.value == f.value))
        .collect();
    out.extend(orphelines);
    out
}

/// Same, with the parent each uid hangs from, as the parent axis's own value.
fn choices_sous(
    rows: Vec<(String, Option<String>, i64)>,
    parent: impl Fn(&str) -> Option<String>,
) -> Vec<FacetChoice> {
    rows.into_iter()
        .map(|(uid, label, count)| FacetChoice {
            label: label.unwrap_or_else(|| uid.clone()),
            parent: parent(&uid),
            value: suffixe(&uid).to_string(),
            count,
            hint: None,
        })
        .collect()
}

/// A tender's page (`/notice/{juris}/{uid}`): the notice, its deep links by use,
/// its channels and its edges into the graph. Closed rows are still served: they
/// were indexed, and they are the archive.
#[instrument(skip(state))]
pub async fn detail(state: &AppState, uid: &str) -> Result<AnnonceDetailResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let row = match repo.annonce(uid).await? {
        Some(row) => row,
        // Un uid absorbé par une fusion sert la fiche de l'élu ;
        // la page SSR lit l'écart d'uid dans la réponse et répond 301.
        None => {
            let cible = repo
                .redirection_annonce(uid)
                .await?
                .ok_or(ApiError::NotFound)?;
            repo.annonce(&cible).await?.ok_or(ApiError::NotFound)?
        }
    };
    let (canaux, entites) = tokio::try_join!(
        repo.annonce_canaux(&row.uid),
        repo.annonce_entites(&row.uid)
    )?;
    let lot_sirens: Vec<String> = row
        .details
        .pointer("/resultat/lots")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|l| l.get("siren")?.as_str().map(str::to_string))
        .collect();
    let registered: HashSet<String> = if lot_sirens.is_empty() {
        HashSet::new()
    } else {
        repo.denominations_par_siren(&lot_sirens)
            .await?
            .into_iter()
            .map(|(siren, _)| siren)
            .collect()
    };
    Ok(AnnonceDetailResponse {
        liens: liens_from(&row),
        lots_ouverts: lots_ouverts_from(&row),
        criteres: row
            .details
            .get("criteres")
            .and_then(|v| v.as_array())
            .map(|c| {
                c.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        montant_estime: row.details.get("montant_estime").and_then(|m| {
            Some(AnnonceMontantDto {
                valeur: m.get("valeur")?.as_f64()?,
                devise: m.get("devise").and_then(|d| d.as_str()).map(str::to_string),
            })
        }),
        resultat: resultat_from(&row, &registered),
        annonce: hit_from(&row),
        canaux_detail: canaux
            .into_iter()
            .map(|c| AnnonceCanalDto {
                moteur: moteur_de(&c.source_uid).to_string(),
                source_uid: c.source_uid,
                url_avis: c.url_avis,
                first_seen_at: c.first_seen_at.to_rfc3339(),
            })
            .collect(),
        entites: entites
            .into_iter()
            .filter_map(|e| {
                let (ns, id) = e.entity_uid.split_once(':')?;
                Some(AnnonceEntiteDto {
                    role: e.role,
                    href: e
                        .denomination
                        .is_some()
                        .then(|| lj_vocab::entity_path(ns, id)),
                    label: e.denomination.unwrap_or_else(|| e.entity_uid.clone()),
                })
            })
            .collect(),
    })
}

/// Open stock broken down by département and by nature: the crawl paths to
/// `/annonces/departement/{code}`. One cache entry, TTL 1 h.
pub async fn catalogue(state: &AppState) -> Result<Arc<AnnonceCatalogueResponse>> {
    state
        .journals_catalogue_cache
        .try_get_with(String::new(), load_catalogue(state))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue annonces: {e}")))
}

async fn load_catalogue(state: &AppState) -> Result<Arc<AnnonceCatalogueResponse>> {
    let filtre = AnnonceFiltre::nouveau(JOURNAL_FOND_MARCHE);
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let (total, zones, familles) = tokio::try_join!(
        repo.annonces_total(&filtre),
        repo.annonces_facette(&filtre, AnnonceAxe::Zone),
        repo.annonces_facette(&filtre, AnnonceAxe::Famille),
    )?;
    Ok(Arc::new(AnnonceCatalogueResponse {
        fond: JOURNAL_FOND_MARCHE.to_string(),
        total,
        zones: zones
            .into_iter()
            // The catalogue is the crawl surface, one page per département:
            // nationwide and Europe-wide are facets, not pages.
            .filter(|(code, _, _)| zone_est_departement(code))
            .map(|(code, _, count)| AnnonceZoneEntry {
                label: zone_nom(&code),
                code,
                count,
            })
            .collect(),
        familles: choices_referentiel(familles),
    }))
}

/// Les marchés d'une entité, groupés par rôle — la couture du graphe sur la
/// fiche entité. Lu à chaque rendu : la requête est indexée sur
/// `annonce_entity(entity_uid)` et le fond bouge tous les jours.
#[instrument(skip(state))]
pub async fn entity_groups(state: &AppState, uid: &str) -> Result<Vec<EntityJournalGroupDto>> {
    let conn = state.pool.get().await?;
    let rows = DecisionRepository::new(&conn)
        .annonces_entite(uid, PAR_ROLE)
        .await?;
    Ok(grouper_par_role(rows))
}

fn grouper_par_role(rows: Vec<EntityAnnonceRow>) -> Vec<EntityJournalGroupDto> {
    let mut groups: Vec<EntityJournalGroupDto> = Vec::new();
    for row in rows {
        let hit = hit_from(&row.annonce);
        match groups.last_mut() {
            Some(g) if g.key == row.role => g.items.push(hit),
            _ => groups.push(EntityJournalGroupDto {
                label: role_label(&row.role).to_string(),
                key: row.role,
                total: row.total_role,
                items: vec![hit],
            }),
        }
    }
    groups
}

fn role_label(role: &str) -> &str {
    match role {
        "acheteur" => "Marchés publics en cours",
        "titulaire" => "Marchés remportés",
        other => other,
    }
}

// ── Projection ligne → contrat ──────────────────────────────────────────────

/// Whether a source published a code. It lands in two places, the listing's
/// `cpv` and the notice page's, and a market may carry it in the second alone.
fn code_cpv_publie(details: &serde_json::Value) -> bool {
    let pose = |v: Option<&serde_json::Value>| v.is_some_and(|v| !v.is_null());
    pose(details.get("cpv")) || pose(details.get("detail").and_then(|d| d.get("cpv")))
}

/// A trade neither the descriptors nor the works lots publish was read off the title.
fn metiers_deduits(row: &AnnonceRow) -> bool {
    let tableau = |chemin: &str| {
        row.details
            .pointer(chemin)
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let descripteurs: Vec<&str> = tableau("/descripteurs")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let codes_des_lots: Vec<&str> = tableau("/lots")
        .iter()
        .chain(tableau("/detail/lots"))
        .filter_map(|l| l.get("cpv").and_then(|c| c.as_str()))
        .collect();
    let publies = lj_annonces::metiers::metiers_publies(&descripteurs, &codes_des_lots);
    row.metiers.iter().any(|m| !publies.contains(m))
}

fn hit_from(row: &AnnonceRow) -> JournalHit {
    let texte = |cle: &str| {
        row.details
            .get(cle)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    JournalHit {
        uid: row.uid.clone(),
        fond_label: journal_fond_label(&row.fond)
            .unwrap_or(&row.fond)
            .to_string(),
        acteur_role: journal_acteur_role(&row.fond).to_string(),
        jurisdiction: "FR".to_string(),
        journal: None,
        fond: row.fond.clone(),
        titre: row.titre.clone(),
        description: row.description.clone(),
        acteur: row.acteur_libelle.clone(),
        zones: row
            .zones
            .iter()
            .map(|c| FacetTag {
                label: zone_nom(c),
                key: c.clone(),
            })
            .collect(),
        famille: tag(&row.famille_uid, &row.famille_label),
        cpv: tag(&row.cpv_uid, &row.cpv_label),
        cpv_deduit: row.cpv_uid.is_some() && !code_cpv_publie(&row.details),
        metiers: row
            .metiers
            .iter()
            .map(|u| FacetTag {
                label: lj_annonces::metiers::libelle(u).to_string(),
                key: u.clone(),
            })
            .collect(),
        metiers_deduits: metiers_deduits(row),
        date_publication: row.date_publication.to_string(),
        echeance: row.echeance.map(|t| t.to_rfc3339()),
        // Même définition que la fenêtre servie : l'échéance passée, rien
        // d'autre.
        closed: row.echeance.is_some_and(|e| e < chrono::Utc::now()),
        // Le statut se dérive, il n'est pas stocké : un marché est
        // attribué dès qu'un avis de résultat a posé son bloc.
        attribue: row.details.get("resultat").is_some_and(|r| !r.is_null()),
        url_action: row.url_action.clone(),
        reference: texte("reference"),
        // La procédure manque à la plupart des lignes sans canal BOAMP : elle vient
        // alors de la page de détail, sous `details.detail`.
        procedure: texte("procedure").or_else(|| {
            row.details
                .get("detail")
                .and_then(|d| d.get("procedure"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }),
        first_seen_at: row.premiere_vue.map(|t| t.to_rfc3339()),
        canaux: row.canaux.clone(),
    }
}

/// The allotment the notice publishes, before any award.
fn lots_ouverts_from(row: &AnnonceRow) -> Vec<AnnonceLotOuvertDto> {
    let texte =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    row.details
        .get("lots")
        .and_then(|v| v.as_array())
        .map(|lots| {
            lots.iter()
                .map(|l| AnnonceLotOuvertDto {
                    nom: texte(l, "nom"),
                    description: texte(l, "description"),
                    cpv: texte(l, "cpv"),
                })
                .filter(|l| l.nom.is_some() || l.description.is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// Le bloc attribution d'une fiche de marché — ce qui rend `attribue` vrai. Les
/// lots viennent de `details.resultat`, posé par l'ingest sur la ligne de la
/// consultation qu'ils clôturent.
fn resultat_from(row: &AnnonceRow, registered: &HashSet<String>) -> Option<AnnonceResultatDto> {
    let r = row.details.get("resultat")?;
    let texte =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let lots = r
        .get("lots")
        .and_then(|v| v.as_array())
        .map(|lots| {
            lots.iter()
                .filter_map(|l| {
                    Some(AnnonceLotDto {
                        titulaire: texte(l, "titulaire")?,
                        href: texte(l, "siren")
                            .filter(|s| registered.contains(s))
                            .map(|s| lj_vocab::entity_path("siren", &s)),
                        description: texte(l, "description"),
                        montant: l.get("montant").and_then(|v| v.as_f64()),
                        devise: texte(l, "devise"),
                        nb_offres: l.get("nb_offres").and_then(|v| v.as_i64()),
                        date_attribution: texte(l, "date_attribution"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(AnnonceResultatDto {
        lots,
        texte: texte(r, "texte"),
        avis_url: texte(r, "avis_url"),
    })
}

/// Tag référentiel résolu : clé = **suffixe** d'uid (le jeton de filtre).
fn tag(uid: &Option<String>, label: &Option<String>) -> Option<FacetTag> {
    let uid = uid.as_deref()?;
    Some(FacetTag {
        key: suffixe(uid).to_string(),
        label: label.clone().unwrap_or_else(|| uid.to_string()),
    })
}

/// Liens profonds par usage, dédupliqués : plusieurs canaux publient souvent le
/// même avis, et l'utilisateur veut un lien par chose à faire.
fn liens_from(row: &AnnonceRow) -> Vec<AnnonceLienDto> {
    let mut out: Vec<AnnonceLienDto> = Vec::new();
    let canaux = row.details.get("liens").and_then(|v| v.as_array());
    for canal in canaux.into_iter().flatten() {
        let moteur = canal
            .get("moteur")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        for (kind, label) in [
            ("avis", "Avis de publicité"),
            ("rc", "Règlement de la consultation"),
            ("dce", "Dossier de consultation"),
        ] {
            let Some(url) = canal.get(kind).and_then(|v| v.as_str()) else {
                continue;
            };
            if out.iter().any(|l| l.url == url) {
                continue;
            }
            out.push(AnnonceLienDto {
                kind: kind.to_string(),
                label: label.to_string(),
                url: url.to_string(),
                moteur: moteur.to_string(),
            });
        }
    }
    // La plateforme de dépôt déclarée par l'avis (details.depot) : servie
    // quand aucun canal n'offre de lien profond vers le dossier.
    if !out.iter().any(|l| l.kind == "rc" || l.kind == "dce") {
        if let Some(url) = row.details.get("depot").and_then(|v| v.as_str()) {
            let hote = url
                .split('/')
                .nth(2)
                .unwrap_or_default()
                .trim_start_matches("www.");
            // A deposit URL carrying a consultation identifier is that
            // consultation's page, and not a platform root.
            let designe = !lj_annonces::cles_jointure_depuis_urls(std::iter::once(url)).is_empty();
            if !out.iter().any(|l| l.url == url) {
                out.push(AnnonceLienDto {
                    kind: if designe { "consultation" } else { "depot" }.to_string(),
                    label: if designe {
                        "Page de la consultation"
                    } else {
                        "Plateforme de dépôt"
                    }
                    .to_string(),
                    url: url.to_string(),
                    moteur: hote.to_string(),
                });
            }
        }
    }
    out
}

/// A département, the two scopes that are not places, and the name of what no
/// source located.
fn zone_nom(code: &str) -> String {
    match code {
        ZONE_FRANCE => "France entière".to_string(),
        ZONE_EU => "Union européenne".to_string(),
        ZONE_SANS => "Zone indéterminée".to_string(),
        d => departement_label(d).unwrap_or(d).to_string(),
    }
}

/// Suffixe d'un uid de référentiel (`cpv:45` → `45`).
fn suffixe(uid: &str) -> &str {
    uid.split_once(':').map(|(_, s)| s).unwrap_or(uid)
}

/// Moteur d'un canal, lu dans le préfixe du `source_uid` (`atexo:<hôte>:…`).
fn moteur_de(source_uid: &str) -> &str {
    source_uid.split(':').next().unwrap_or(source_uid)
}

/// Traduit le filtre HTTP (suffixes d'uid, jours) en filtre de repository (uids
/// complets) — la frontière de traduction, une seule fois.
pub fn filtre_from(req: &JournalSearchRequest, fond: &str) -> AnnonceFiltre {
    AnnonceFiltre {
        // Le repository ne sert qu'un fond : le dispatch a déjà choisi.
        fond: fond.to_string(),
        q: req.q.clone(),
        zones: req.zone.clone(),
        familles: req
            .famille
            .iter()
            .map(|s| format!("nature_marche:{}", s.to_uppercase()))
            .chain(
                req.cpv
                    .iter()
                    .filter(|s| s.starts_with(NATURE_PREFIXE))
                    .cloned(),
            )
            .collect(),
        cpv: req
            .cpv
            .iter()
            .filter(|s| !s.starts_with(NATURE_PREFIXE))
            .map(|s| format!("cpv:{s}"))
            .collect(),
        metiers: req.metier.iter().map(|s| format!("metier:{s}")).collect(),
        echeance_jours: req.echeance_jours.map(|j| j as i32),
        closes: req.closes,
        ordre: match req.sort {
            JournalSort::Echeance => AnnonceOrdre::Echeance,
            JournalSort::Publication => AnnonceOrdre::Publication,
            JournalSort::Nouveautes => AnnonceOrdre::Nouveautes,
        },
        limit: i64::from(req.limit),
        offset: i64::from(req.offset),
    }
}

/// Les lignes d'un flux : la fenêtre **pleine** de chaque fond actif, à plat.
/// Pas d'aperçu ni d'axes : un agenda ne facette pas.
pub async fn hits_flux(state: &AppState, req: &JournalSearchRequest) -> Result<Vec<JournalHit>> {
    let actifs = fonds_actifs(req);
    let journals = journals_jo::journals(state, req).await?;
    // Le plafond du flux est un total, pas un plafond par fond : on le partage
    // entre les fonds actifs.
    let part = JournalSearchRequest {
        limit: req.limit / actifs.len().max(1) as u32,
        ..req.clone()
    };
    let mut hits: Vec<JournalHit> = Vec::new();
    for fond in actifs {
        match page_fond(state, &part, fond, journals.as_deref(), true).await {
            Ok((page, _)) => hits.extend(page),
            Err(e) => tracing::warn!(fond, error = %e, "fond unavailable, absent from the feed"),
        }
    }
    Ok(hits)
}

// ── Flux d'alerte : une alerte est un état de filtres ───────────────────────

/// Where a feed line leads: the notice's page.
fn lien_hit(hit: &JournalHit, base: &str) -> String {
    format!(
        "{base}{}",
        lj_vocab::notice_path(&hit.jurisdiction, &hit.uid)
    )
}

/// Titre humain d'un filtre — celui du flux et de la page.
pub fn titre_filtre(req: &JournalSearchRequest) -> String {
    let mut bouts: Vec<String> = Vec::new();
    if let Some(q) = req.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        bouts.push(format!("« {q} »"));
    }
    if !req.zone.is_empty() {
        bouts.push(
            req.zone
                .iter()
                .map(|c| zone_nom(c))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !req.famille.is_empty() {
        bouts.push(req.famille.join(", ").to_lowercase());
    }
    if let Some(j) = req.echeance_jours {
        bouts.push(format!("clôture sous {j} jours"));
    }
    // Le sujet du flux est le fond quand l'utilisateur en a coché un seul,
    // sinon le pot.
    let sujet = match fonds_actifs(req).as_slice() {
        [fond] => journal_fond_label(fond).unwrap_or("Journaux"),
        _ => "Journaux",
    };
    if bouts.is_empty() {
        format!("{sujet} — en cours")
    } else {
        format!("{sujet} — {}", bouts.join(" · "))
    }
}

/// Flux RSS 2.0 du filtre, ordonné par première apparition dans nos canaux.
pub fn rss(req: &JournalSearchRequest, hits: &[JournalHit], self_url: &str, base: &str) -> String {
    let titre = titre_filtre(req);
    let mut items = String::new();
    for hit in hits {
        let lien = lien_hit(hit, base);
        // `pubDate` = première vue chez nous, à défaut la parution.
        let date = hit
            .first_seen_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.to_rfc2822())
            .or_else(|| date_publication_rfc2822(&hit.date_publication))
            .unwrap_or_default();
        items.push_str(&format!(
            "<item><title>{}</title>{}\
             <guid isPermaLink=\"false\">{}</guid>{}<description>{}</description></item>",
            xml_escape(&titre_hit(hit)),
            if lien.is_empty() {
                String::new()
            } else {
                format!("<link>{}</link>", xml_escape(&lien))
            },
            xml_escape(&hit.uid),
            if date.is_empty() {
                String::new()
            } else {
                format!("<pubDate>{}</pubDate>", xml_escape(&date))
            },
            xml_escape(&description(hit)),
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\"><channel>\
         <title>{}</title><link>{}</link>\
         <description>{}</description>\
         <language>fr</language>\
         <atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>{items}</channel></rss>",
        xml_escape(&format!("LibreJustice — {titre}")),
        xml_escape(&self_url.replacen("/journal-search.rss", "/journal-search", 1)),
        xml_escape(&format!(
            "{titre}. Une alerte est un état de filtres : cette URL est l'abonnement."
        )),
        xml_escape(self_url),
    )
}

/// Calendrier ICS du filtre : un VEVENT par **échéance**, l'heure limite de
/// remise des offres. Les annonces sans échéance connue en sortent.
pub fn ics(req: &JournalSearchRequest, hits: &[JournalHit], base: &str) -> String {
    let stamp = ics_stamp(chrono::Utc::now());
    let mut out = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//LibreJustice//Journaux//FR\r\n\
         CALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\n",
    );
    out.push_str(&ics_line("X-WR-CALNAME", &titre_filtre(req)));
    for hit in hits {
        let Some(echeance) = hit
            .echeance
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let debut = ics_stamp(echeance.with_timezone(&chrono::Utc));
        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str(&format!("UID:{}@librejustice.fr\r\n", hit.uid));
        out.push_str(&format!("DTSTAMP:{stamp}\r\n"));
        out.push_str(&format!("DTSTART:{debut}\r\n"));
        // RFC 5545 3.8.2.2 requires DTEND strictly later than DTSTART.
        out.push_str(&format!(
            "DTEND:{}\r\n",
            ics_stamp(echeance.with_timezone(&chrono::Utc) + chrono::Duration::minutes(30))
        ));
        out.push_str(&ics_line(
            "SUMMARY",
            &format!("Clôture — {} ({})", hit.titre, hit.acteur),
        ));
        out.push_str(&ics_line("DESCRIPTION", &description(hit)));
        out.push_str(&ics_line("URL", &lien_hit(hit, base)));
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

const SEPARATEUR: &str = "\u{2014}";

/// Proxied fonds already build their title on the actor's name, so prefixing it
/// would repeat it.
fn titre_hit(hit: &JournalHit) -> String {
    if hit.acteur.is_empty() || hit.titre.contains(&hit.acteur) {
        hit.titre.clone()
    } else {
        format!("{} {SEPARATEUR} {}", hit.acteur, hit.titre)
    }
}

fn date_publication_rfc2822(date: &str) -> Option<String> {
    Some(
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()?
            .and_hms_opt(0, 0, 0)?
            .and_utc()
            .to_rfc2822(),
    )
}

/// Corps commun aux deux flux : ce qu'il faut savoir pour décider d'y aller.
fn description(hit: &JournalHit) -> String {
    let mut bouts = vec![hit.acteur.clone()];
    if !hit.zones.is_empty() {
        bouts.push(
            hit.zones
                .iter()
                .map(|z| z.label.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(f) = hit.famille.as_ref() {
        bouts.push(f.label.clone());
    }
    if let Some(r) = hit.reference.as_deref() {
        bouts.push(format!("réf. {r}"));
    }
    bouts.push(format!("canaux : {}", hit.canaux.join(", ")));
    bouts.join(" — ")
}

/// `20260812T160000Z` — les instants ICS sont en UTC (suffixe `Z`).
fn ics_stamp(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Une ligne de propriété ICS, échappée puis **pliée** à 75 octets (RFC 5545
/// §3.1). Le pli se fait sur une frontière de caractère UTF-8, la continuation
/// commence par une espace.
fn ics_line(prop: &str, value: &str) -> String {
    let ligne = format!("{prop}:{}", ics_escape(value));
    let mut out = String::new();
    let mut reste: &str = &ligne;
    let mut limite = 75;
    while reste.len() > limite {
        let mut coupe = limite;
        while !reste.is_char_boundary(coupe) {
            coupe -= 1;
        }
        out.push_str(&reste[..coupe]);
        out.push_str("\r\n ");
        reste = &reste[coupe..];
        // Les lignes de continuation portent l'espace de pli en plus.
        limite = 74;
    }
    out.push_str(reste);
    out.push_str("\r\n");
    out
}

/// Échappement ICS (RFC 5545 §3.3.11) : `\`, `;`, `,` et les retours à la ligne.
fn ics_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\r', "")
        .replace('\n', "\\n")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bare_page_lists_no_jo_act_nor_recueil() {
        let bare = JournalSearchRequest::default();
        assert!(!fonds_actifs(&bare).contains(&JOURNAL_FOND_JO));
        assert!(!fonds_actifs(&bare).contains(&JOURNAL_FOND_RAA));
        let blank = JournalSearchRequest {
            q: Some("  ".into()),
            ..Default::default()
        };
        assert!(!fonds_actifs(&blank).contains(&JOURNAL_FOND_JO));
        assert!(!fonds_actifs(&blank).contains(&JOURNAL_FOND_RAA));
        let question = JournalSearchRequest {
            q: Some("sous-préfet".into()),
            ..Default::default()
        };
        assert_eq!(fonds_actifs(&question), FONDS);
        let chosen = JournalSearchRequest {
            fond: vec![JOURNAL_FOND_JO.into()],
            ..Default::default()
        };
        assert_eq!(fonds_actifs(&chosen), [JOURNAL_FOND_JO]);
    }

    /// One key holds both levels, as `juridiction` does: the root is told from
    /// the leaf by its prefix, and each goes to the column it filters.
    #[test]
    fn la_cle_du_domaine_route_la_nature_et_la_division_vers_leur_colonne() {
        let req = JournalSearchRequest {
            cpv: vec!["45".into(), "nature_marche:SERVICES".into()],
            ..Default::default()
        };
        let f = filtre_from(&req, JOURNAL_FOND_MARCHE);
        assert_eq!(f.cpv, ["cpv:45"]);
        assert_eq!(f.familles, ["nature_marche:SERVICES"]);
    }

    #[test]
    fn chaque_division_pend_sous_sa_nature_et_la_suit_dans_la_liste() {
        let natures = vec![
            (
                "nature_marche:TRAVAUX".to_string(),
                Some("Travaux".into()),
                9,
            ),
            (
                "nature_marche:SERVICES".to_string(),
                Some("Services".into()),
                7,
            ),
        ];
        let divisions = vec![
            (
                "cpv:45".to_string(),
                Some("Travaux de construction".into()),
                9,
            ),
            (
                "cpv:79".to_string(),
                Some("Services aux entreprises".into()),
                7,
            ),
        ];
        let out = sous_les_natures(natures, divisions);
        let vus: Vec<(&str, Option<&str>)> = out
            .iter()
            .map(|c| (c.value.as_str(), c.parent.as_deref()))
            .collect();
        assert_eq!(
            vus,
            [
                ("nature_marche:TRAVAUX", None),
                ("45", Some("nature_marche:TRAVAUX")),
                ("nature_marche:SERVICES", None),
                ("79", Some("nature_marche:SERVICES")),
            ]
        );
    }

    fn hit() -> JournalHit {
        JournalHit {
            metiers: Vec::new(),
            metiers_deduits: false,
            uid: "marche:abc".into(),
            fond: JOURNAL_FOND_MARCHE.into(),
            fond_label: "Marchés publics".into(),
            titre: "Travaux de voirie, réfection".into(),
            description: None,
            acteur: "Commune d'Ax".into(),
            acteur_role: "Acheteur".into(),
            jurisdiction: "FR".to_string(),
            journal: None,
            zones: vec![FacetTag {
                key: "09".into(),
                label: "Ariège".into(),
            }],
            famille: Some(FacetTag {
                key: "TRAVAUX".into(),
                label: "Travaux".into(),
            }),
            cpv: None,
            cpv_deduit: false,
            date_publication: "2026-08-01".into(),
            echeance: Some("2026-09-12T15:00:00+00:00".into()),
            closed: false,
            attribue: false,
            url_action: "https://exemple.fr/c/1".into(),
            reference: Some("PA-2026-3".into()),
            procedure: None,
            first_seen_at: Some("2026-08-02T04:10:00+00:00".into()),
            canaux: vec!["boamp".into(), "atexo".into()],
        }
    }

    fn row(details: serde_json::Value) -> AnnonceRow {
        AnnonceRow {
            metiers: Vec::new(),
            uid: "marche:abc".into(),
            fond: JOURNAL_FOND_MARCHE.into(),
            famille_uid: None,
            famille_label: None,
            titre: "Travaux de voirie".into(),
            acteur_libelle: "Commune d'Ax".into(),
            zones: Vec::new(),
            cpv_uid: None,
            cpv_label: None,
            date_publication: chrono::NaiveDate::from_ymd_opt(2026, 8, 1).expect("date valide"),
            echeance: None,
            url_action: "https://exemple.fr/c/1".into(),
            description: None,
            details,
            canaux: vec!["aws".into()],
            premiere_vue: None,
        }
    }

    #[test]
    fn la_procedure_tombe_en_repli_sur_le_bloc_detail() {
        let flux = hit_from(&row(serde_json::json!({
            "procedure": "Appel d'offres ouvert",
            "detail": {"procedure": "Procédure adaptée"},
        })));
        assert_eq!(flux.procedure.as_deref(), Some("Appel d'offres ouvert"));

        let repli = hit_from(&row(serde_json::json!({
            "procedure": null,
            "detail": {"procedure": "Procédure adaptée"},
        })));
        assert_eq!(repli.procedure.as_deref(), Some("Procédure adaptée"));

        // Page visitée, muette sur la procédure : le bloc est là, le champ non.
        let muet = hit_from(&row(serde_json::json!({"procedure": null, "detail": {}})));
        assert_eq!(muet.procedure, None);
    }

    #[test]
    fn a_trade_is_declared_deduced_unless_what_the_market_publishes_folds_to_it() {
        let deduit = |descripteurs: serde_json::Value, metiers: &[&str]| {
            let mut r = row(serde_json::json!({ "descripteurs": descripteurs }));
            r.metiers = metiers.iter().map(|m| m.to_string()).collect();
            hit_from(&r).metiers_deduits
        };
        assert!(!deduit(
            serde_json::json!(["Couverture"]),
            &["metier:charpente-couverture"]
        ));
        assert!(deduit(
            serde_json::json!(["Prestations de services"]),
            &["metier:vrd"]
        ));
        assert!(deduit(serde_json::Value::Null, &["metier:vrd"]));
        assert!(!deduit(serde_json::json!(["Prestations de services"]), &[]));

        let mut lots = row(serde_json::json!({
            "descripteurs": ["Prestations de services"],
            "lots": [{"nom": "Menuiseries", "cpv": "45421000"}, {"nom": "Divers"}],
        }));
        lots.metiers = vec!["metier:menuiserie".to_string()];
        assert!(!hit_from(&lots).metiers_deduits);
        let mut page = row(serde_json::json!({"detail": {"lots": [{"cpv": "45330000"}]}}));
        page.metiers = vec!["metier:genie-climatique".to_string()];
        assert!(!hit_from(&page).metiers_deduits);
        page.metiers.insert(0, "metier:vrd".to_string());
        assert!(hit_from(&page).metiers_deduits);
    }

    #[test]
    fn la_selection_hors_facette_garde_son_libelle() {
        let mut axe = JournalFacetAxis {
            key: "cpv".to_string(),
            label: "Domaine (CPV)".to_string(),
            choices: vec![FacetChoice {
                value: "45".to_string(),
                label: "Travaux de construction".to_string(),
                count: 18,
                parent: None,
                hint: None,
            }],
        };
        completer_selection(&mut axe, &["45".to_string(), "71".to_string()], |v| {
            format!("CPV {v}")
        });
        // Le 45 compté n'est pas dupliqué ; le 71 entre à zéro.
        assert_eq!(axe.choices.len(), 2);
        let ajout = axe.choices.last().unwrap();
        assert_eq!(ajout.value, "71");
        assert_eq!(ajout.label, "CPV 71");
        assert_eq!(ajout.count, 0);
    }

    #[test]
    fn ics_echappe_les_separateurs_et_plie_a_75_octets() {
        let ics = ics(&JournalSearchRequest::default(), &[hit()], "https://lj.fr");
        // Virgule du titre échappée.
        assert!(ics.contains("Travaux de voirie\\, réfection"));
        // L'échéance part en UTC, jamais en heure locale.
        assert!(ics.contains("DTSTART:20260912T150000Z"));
        for ligne in ics.split("\r\n") {
            assert!(ligne.len() <= 75, "ligne non pliée : {ligne:?}");
        }
        // Chaque continuation commence par une espace.
        assert!(ics.split("\r\n").skip(1).all(|l| !l.starts_with('\t')));
    }

    #[test]
    fn ics_saute_les_annonces_sans_echeance() {
        let mut sans = hit();
        sans.echeance = None;
        let ics = ics(&JournalSearchRequest::default(), &[sans], "https://lj.fr");
        assert!(!ics.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn rss_porte_la_premiere_vue_en_pubdate() {
        let rss = rss(
            &JournalSearchRequest::default(),
            &[hit()],
            "https://lj.fr/journal-search.rss",
            "https://lj.fr",
        );
        // Le curseur d'alerte est la première vue chez nous, pas la parution.
        assert!(rss.contains("<pubDate>Sun, 2 Aug 2026 04:10:00 +0000</pubDate>"));
        assert!(rss.contains("<link>https://lj.fr/notice/fr/marche:abc</link>"));
        let filtre = super::rss(
            &JournalSearchRequest::default(),
            &[],
            "https://lj.fr/journal-search.rss?fond=marche&zone=973",
            "https://lj.fr",
        );
        assert!(
            filtre.contains("<link>https://lj.fr/journal-search?fond=marche&amp;zone=973</link>")
        );
    }

    /// A notice with no link keeps its own `guid` and writes no empty `<link>`.
    #[test]
    fn sans_lien_chaque_entree_garde_son_identite() {
        let sans_cible = |uid: &str| JournalHit {
            uid: uid.into(),
            fond: JOURNAL_FOND_JOAFE.into(),
            url_action: String::new(),
            first_seen_at: None,
            ..hit()
        };
        let rss = rss(
            &JournalSearchRequest::default(),
            &[sans_cible("joafe:1"), sans_cible("joafe:2")],
            "https://lj.fr/journal-search.rss",
            "https://lj.fr",
        );
        assert!(rss.contains("<guid isPermaLink=\"false\">joafe:1</guid>"));
        assert!(rss.contains("<guid isPermaLink=\"false\">joafe:2</guid>"));
        assert!(!rss.contains("<link></link>"));
        assert!(rss.contains("<pubDate>Sat, 1 Aug 2026 00:00:00 +0000</pubDate>"));
    }

    #[test]
    fn le_filtre_traduit_les_suffixes_en_uids_de_referentiel() {
        let req = JournalSearchRequest {
            famille: vec!["travaux".into()],
            cpv: vec!["45".into()],
            ..JournalSearchRequest::default()
        };
        let f = filtre_from(&req, JOURNAL_FOND_MARCHE);
        assert_eq!(f.familles, ["nature_marche:TRAVAUX"]);
        assert_eq!(f.cpv, ["cpv:45"]);
        assert!(!f.closes);
        assert_eq!(f.ordre, AnnonceOrdre::Echeance);
    }

    #[test]
    fn le_titre_du_flux_nomme_le_fond_et_le_filtre() {
        let req = JournalSearchRequest {
            fond: vec![JOURNAL_FOND_MARCHE.into()],
            zone: vec!["09".into()],
            echeance_jours: Some(7),
            ..JournalSearchRequest::default()
        };
        assert_eq!(
            titre_filtre(&req),
            "Marchés publics — Ariège · clôture sous 7 jours"
        );
        assert_eq!(
            titre_filtre(&JournalSearchRequest {
                fond: vec![JOURNAL_FOND_BODACC.into()],
                ..JournalSearchRequest::default()
            }),
            "Vie des entreprises — en cours"
        );
        assert_eq!(
            titre_filtre(&JournalSearchRequest::default()),
            "Journaux \u{2014} en cours"
        );
    }
}
