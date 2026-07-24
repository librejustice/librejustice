//! Référentiel de droit positif — law-at-date, timeline, citations, sommaire de
//! code (ADR 0097, généralise ADR 0092).
//!
//! Handlers purs `(&AppState, …) -> Result<DTO>` (même forme que
//! [`crate::decisions`]) ; l'adaptation axum (path/query, headers de cache) vit
//! dans [`crate::routes`]. Le `code` d'URL est résolu en `text_uid` par lookup
//! **exact** sur le slug ([`DecisionRepository::resolve_referential_code`]) ; rien
//! ne matche ⇒ 404 ([`ApiError::NotFound`]). Le `num` d'URL **est déjà** la clé
//! canonique (`num_key`) : nos liens internes la portent (DTO `numKey`, ADR 0123
//! §2), aucune normalisation au runtime serve — c'est ce qui sort la pile
//! `lj-extract` du chemin serve.

use chrono::{Datelike, NaiveDate};
use lj_core::source_authority::{is_live_authoritative, source_authority};
use lj_dtos::{
    ArticleModification, ArticleNeighbor, ArticleSearchFacets, ArticleSearchHit,
    ArticleSearchResponse, CitationSpan, CitationTarget, CitingDecisionHit, CoCitedArticle,
    CodeCatalogueEntry, CodeCatalogueResponse, CodeTocResponse, Commentaire, FacetChoice,
    JurisdictionType, LawArticleResponse, LawArticleVersion, LawCodeSummary, LawCompareOp,
    LawCompareResponse, LawCompareSegment, LawSectionItem, LawSectionRef, LawSectionResponse,
    LinkedTextRef, ModificationItem, Significance, TocEntry, TocNode,
};
use lj_store::repository::{
    ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, CitingDecisionRow,
    DecisionRepository, FacetCount, LawVersionRow, LegalArticleRow, LegalTextCatalogRow,
    ResolvedLegalLink, TocArticleRow, TocReadingRow, TocTreeRow,
};
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::state::AppState;
use crate::titles::decision_title;

/// URL Légifrance versionnée d'un article (ADR 0092) :
/// `/codes/article_lc/{LEGIARTI}/{date}`. `date` ISO `YYYY-MM-DD` est la date de
/// consultation (date demandée pour la version-à-date, sinon `date_debut` de la
/// version servie).
/// URL Légifrance absolue et versionnée d'un article **natif** Légifrance
/// (`source_uid` = LEGIARTI…). `None` pour une source curée (droit étranger, traités :
/// `source_uid` synthétique `text_uid#…`) qui porte déjà sa propre `source_url`
/// (ADR 0131). Sert à *remplir* `source_url` quand elle est absente — pas de champ
/// `legifrance_url` dédié (lien « source » unique et générique).
fn legifrance_source_url(source_uid: &str, date: &str) -> Option<String> {
    if source_uid.contains('#') {
        return None;
    }
    Some(format!(
        "https://www.legifrance.gouv.fr/codes/article_lc/{source_uid}/{date}"
    ))
}

/// Rend une `date_debut` en ISO `YYYY-MM-DD`, en mappant la sentinelle borne
/// ouverte (`0001-01-01`, posée par l'upsert pour les versions sans début connu —
/// la colonne est NOT NULL depuis ADR 0112) vers la chaîne vide, comme une borne
/// ouverte l'était avant (DTO `dateDebut` vide = pas de début). Un `None` (qui ne
/// devrait plus survenir, la colonne étant NOT NULL) est traité de même.
fn date_debut_display(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 => d.to_string(),
        _ => String::new(),
    }
}

/// Date de début pour l'AFFICHAGE (DTO `dateDebut`) : comme [`date_debut_display`]
/// mais absorbe aussi la sentinelle LEGI `2999-01-01`, posée sur `date_debut` des
/// articles **modificatifs** sans date propre (sinon « en vigueur depuis le 1
/// janvier 2999 »). L'URL Légifrance garde 2999 (elle la résout) via la
/// `consult_date`, calculée à part par [`date_debut_display`].
fn display_start_date(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 && d.year() < 2999 => d.to_string(),
        _ => String::new(),
    }
}

/// Partie « nom du texte » d'un libellé de lien DILA (« Code civil - art. 1302
/// (V) » → « Code civil »).
fn label_prefix(label: &str) -> &str {
    label.split(" - ").next().unwrap_or(label).trim()
}

/// Partie « cible » d'un libellé de lien DILA (« Code civil - Chapitre III :
/// Les autres sources » → « Chapitre III : Les autres sources »).
fn label_suffix(label: &str) -> &str {
    label
        .split_once(" - ")
        .map(|(_, rest)| rest.trim())
        .unwrap_or(label)
}

/// Une arête du graphe en référence liée : lien article garanti quand la cible
/// est résolue, ancre de section au sommaire (`#{cid}`, ADR 0207) pour une
/// cible section, sinon sommaire du texte porteur, sinon libellé nu.
fn linked_text_ref(l: ResolvedLegalLink) -> LinkedTextRef {
    let href = match (l.resolved_slug, l.resolved_num_key) {
        (Some(slug), Some(num_key)) => Some(format!("/texte/{slug}/{num_key}")),
        _ => match (&l.target_text_slug, &l.resolved_section_cid) {
            (Some(slug), Some(cid)) => Some(format!("/texte/{slug}#{cid}")),
            _ => l.target_text_slug.map(|s| format!("/texte/{s}")),
        },
    };
    LinkedTextRef {
        label: l.target_label,
        href,
        date: l.target_date.map(|d| d.to_string()),
    }
}

/// Dérive les blocs de la page article depuis ses arêtes `legal_link`
/// (ADR 0174) : dispositions modifiées (sortant modifie/crée/abroge, groupé
/// **globalement** par action × texte cible dans l'ordre de première
/// apparition — le XML entrelace les actions section par section, un
/// regroupement seulement consécutif émietterait le rendu ADR 0173 §3),
/// « Modifié par » (entrant modificatoire, trié par date), « Cite » / « Cité
/// par » (CITATION, dédupliqués — la réciprocité DILA peut répéter une cible).
fn link_blocks(
    links: Vec<ResolvedLegalLink>,
) -> (
    Vec<ArticleModification>,
    Vec<LinkedTextRef>,
    Vec<LinkedTextRef>,
    Vec<LinkedTextRef>,
) {
    let mut modifications: Vec<ArticleModification> = Vec::new();
    let mut group_index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut modified_by: Vec<LinkedTextRef> = Vec::new();
    let mut cites: Vec<LinkedTextRef> = Vec::new();
    let mut cited_by: Vec<LinkedTextRef> = Vec::new();
    for l in links {
        if l.target_label.is_empty() {
            continue;
        }
        let incoming = l.direction == "incoming";
        match l.verb.as_str() {
            "modifie" | "cree" | "abroge" if !incoming => {
                let action = l.verb.clone();
                let code = l
                    .target_text_title
                    .clone()
                    .unwrap_or_else(|| label_prefix(&l.target_label).to_string());
                let code_href = l.target_text_slug.clone().map(|s| format!("/texte/{s}"));
                let item = match l.target_kind.as_str() {
                    "article" => ModificationItem {
                        href: match (&l.resolved_slug, &l.resolved_num_key) {
                            (Some(slug), Some(num_key)) => Some(format!("/texte/{slug}/{num_key}")),
                            _ => None,
                        },
                        kind: "article".to_string(),
                        label: l
                            .target_num
                            .unwrap_or_else(|| label_suffix(&l.target_label).to_string()),
                    },
                    // Cible section : ancre au sommaire du texte porteur quand
                    // le cid est résolu via l'arbre structurel (ADR 0207).
                    "section" => ModificationItem {
                        kind: "section".to_string(),
                        label: label_suffix(&l.target_label).to_string(),
                        href: match (&l.target_text_slug, &l.resolved_section_cid) {
                            (Some(slug), Some(cid)) => Some(format!("/texte/{slug}#{cid}")),
                            _ => None,
                        },
                    },
                    _ => ModificationItem {
                        kind: "texte".to_string(),
                        label: l.target_label.clone(),
                        href: l.target_text_slug.map(|s| format!("/texte/{s}")),
                    },
                };
                match group_index.get(&(action.clone(), code.clone())) {
                    Some(&i) => modifications[i].items.push(item),
                    None => {
                        group_index.insert((action.clone(), code.clone()), modifications.len());
                        modifications.push(ArticleModification {
                            action,
                            code,
                            code_href,
                            items: vec![item],
                        });
                    }
                }
            }
            "modifie" | "cree" | "abroge" => modified_by.push(linked_text_ref(l)),
            "cite" if !incoming => cites.push(linked_text_ref(l)),
            "cite" => cited_by.push(linked_text_ref(l)),
            // codification, concordance, transfert… : stockés, exposés quand un
            // usage produit les réclamera (frises Phase C/D).
            _ => {}
        }
    }
    // « Modifié par » chronologique (ISO trie lexicalement) ; dates absentes en fin.
    modified_by.sort_by(|a, b| match (&a.date, &b.date) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    for list in [&mut modified_by, &mut cites, &mut cited_by] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|r| seen.insert((r.label.clone(), r.href.clone())));
    }
    (modifications, modified_by, cites, cited_by)
}

/// Convertit une ligne de timeline du repo en DTO version (dates ISO telles
/// quelles, `dateDebut` borne ouverte ⇒ chaîne vide, `dateFin` absente ⇒ pas de
/// fin). `dateDebut` du DTO est requise : une borne ouverte (sentinelle
/// `0001-01-01` posée par l'upsert, ou source non versionnée) est rendue en
/// chaîne vide. `source_uid` = identifiant natif de la version (LEGIARTI…).
fn version_dto(row: LawVersionRow) -> LawArticleVersion {
    // Sentinelles LEGI de borne ouverte / date absente : `0001-01-01` (borne
    // ouverte upsert) et `2999-01-01` (article modificatif sans date propre).
    let date_debut = match row.date_debut {
        Some(d) if d != "0001-01-01" && d != "2999-01-01" => d,
        _ => String::new(),
    };
    // Version future = pas encore entrée en vigueur (ADR 0178). Comparaison
    // lexicale ISO ; la sentinelle 2999 est déjà absorbée ci-dessus, 2222
    // (date inconnue) reste marquée « à venir ».
    let upcoming =
        !date_debut.is_empty() && date_debut > chrono::Utc::now().date_naive().to_string();
    LawArticleVersion {
        legiarti: row.source_uid,
        etat: row.status,
        date_debut,
        date_fin: row.date_fin,
        upcoming,
    }
}

/// Assemble la réponse article (version servie + timeline) à partir d'une ligne
/// `legal_article` et de la timeline. `consult_date` est la date de consultation
/// pour l'URL Légifrance (date demandée, sinon `date_debut` de la version, sinon
/// chaîne vide pour une borne ouverte). `legiarti` = identifiant natif de la
/// version (`source_uid`, LEGIARTI…) ; `legitext` = identité du texte
/// (`text_uid`).
async fn article_response(
    repo: &DecisionRepository<'_>,
    code: &str,
    article: LegalArticleRow,
    consult_date: &str,
    href_date: Option<NaiveDate>,
    versions: Vec<LawVersionRow>,
    context: Vec<ArticleNeighborRow>,
) -> Result<LawArticleResponse> {
    let links = repo
        .article_links(&article.text_uid, &article.num_key, article.date_debut)
        .await?;
    // Renvois du corps (ADR 0217) : spans de la version servie, hrefs internes
    // — datés quand la lecture l'est (`href_date` = date demandée de la route),
    // pour que le renvoi navigue dans le même temps que la lecture. Même
    // doctrine que les décisions : article ciblé → `/texte/{slug}/{numKey}`,
    // mention nue → sommaire seulement si le texte a des articles, sinon pas
    // de span du tout (mort du pointillé).
    let texte_spans: Vec<CitationSpan> = repo
        .article_citation_spans(
            &article.text_uid,
            &article.num_key,
            // La borne ouverte est `None` dans la ligne lue mais stockée
            // sentinelle `0001-01-01` côté `text_legal_citation` (clé owner).
            article
                .date_debut
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(1, 1, 1).unwrap()),
        )
        .await?
        .into_iter()
        .filter_map(|s| {
            let slug = s.ref_slug?;
            let href = match s.ref_num_key.as_deref().filter(|k| !k.is_empty()) {
                Some(k) => match href_date {
                    Some(d) => format!("/texte/{slug}/{k}/{d}"),
                    None => format!("/texte/{slug}/{k}"),
                },
                None if s.ref_has_articles => match href_date {
                    Some(d) => format!("/texte/{slug}?date={d}"),
                    None => format!("/texte/{slug}"),
                },
                None => return None,
            };
            Some(CitationSpan {
                start: s.char_start.max(0) as usize,
                end: s.char_end.max(0) as usize,
                targets: vec![CitationTarget {
                    href: Some(href),
                    label: s.ref_title,
                }],
            })
        })
        .collect();
    let travaux_parlementaires = travaux_refs(&links);
    let (modifications, modified_by, cites, cited_by) = link_blocks(links);
    // Commentaires de norme (ADR 0212) : bundles ancrés sur (texte, article)
    // et sur le texte entier, forme `commentaires[]` partagée avec la décision.
    let commentaires = match repo
        .article_commentaires(&article.text_uid, &article.num_key)
        .await?
    {
        Some(v) => serde_json::from_value::<Vec<Commentaire>>(v)
            .map_err(|e| ApiError::Internal(format!("commentaires invalides: {e}")))?,
        None => Vec::new(),
    };
    let date_debut = display_start_date(article.date_debut);
    let date_fin = article.date_fin.map(|d| d.to_string());
    // Lien « source » unique (ADR 0131) : l'URL curée si présente (droit étranger,
    // traités), sinon — pour un article natif Légifrance — la page Légifrance versionnée.
    let source_url = article
        .source_url
        .clone()
        .or_else(|| legifrance_source_url(&article.source_uid, consult_date));
    // Fraîcheur effective (ADR 0129) : `source_asof` par ligne (curé / bulk / jafbase),
    // sinon — pour les sources live re-synchronisées (legifrance/kali) — la date du
    // dernier sync (`ingest_freshness`). Axe distinct de `translation`.
    let source_asof = match article.source_asof {
        Some(d) => Some(d.to_string()),
        None if is_live_authoritative(&article.source) => repo
            .get_ingest_freshness(&article.source)
            .await?
            .map(|d| d.to_string()),
        None => None,
    };
    let source_authority = source_authority(&article.source).label().to_string();
    // Titre humain du texte pour l'en-tête « Article N du … » : le `code` d'URL est un
    // slug (le front l'humaniserait laidement, ex. « code de la famille senegal sn »).
    let code_title = repo.referential_title(code).await?;
    // Frise + bandeau « sera modifié le … » (ADR 0178) : versions convertie une
    // fois, première date future extraite.
    let versions: Vec<LawArticleVersion> = versions.into_iter().map(version_dto).collect();
    let upcoming_version_date = versions
        .iter()
        .filter(|v| v.upcoming)
        .map(|v| v.date_debut.clone())
        .min();
    // Fil d'Ariane TOC cliquable : divisions enclosantes de la version servie,
    // hrefs vers la vue-lecture de section (ADR 0207). Vide hors TOC (JORF,
    // étranger) — le front retombe sur `titre_text`.
    let breadcrumb = repo
        .article_toc_breadcrumb(&article.text_uid, &article.source_uid)
        .await?
        .into_iter()
        .map(|(label, cid)| LinkedTextRef {
            label,
            href: cid.map(|c| format!("/texte/{code}/section/{c}")),
            date: None,
        })
        .collect();
    Ok(LawArticleResponse {
        legiarti: article.source_uid,
        legitext: article.text_uid,
        code: code.to_string(),
        code_title,
        num: article.num,
        num_key: article.num_key,
        etat: article.status,
        date_debut,
        source: article.source,
        titre_text: article.title_path,
        breadcrumb,
        date_fin,
        texte: article.texte,
        texte_spans,
        texte_original: article.texte_original,
        lang_original: article.lang_original,
        translation: article.translation,
        source_asof,
        source_authority,
        source_url,
        source_upstream_url: article.source_upstream_url,
        nota: article.nota,
        upcoming_version_date,
        versions,
        context: context.into_iter().map(neighbor_dto).collect(),
        modifications,
        modified_by,
        cites,
        cited_by,
        commentaires,
        travaux_parlementaires,
    })
}

/// Travaux parlementaires (ADR 0215, zéro ingest) : une ligne par **loi**
/// modificatrice de l'article (arêtes entrantes modifie/crée/abroge, nature
/// LOI, texte JORFTEXT), dédupliquée par texte, triée par date décroissante.
/// `href` composé vers la page Légifrance de la loi au JO, qui porte le bloc
/// « Travaux préparatoires » et les liens de dossiers législatifs.
fn travaux_refs(links: &[ResolvedLegalLink]) -> Vec<LinkedTextRef> {
    let mut seen = std::collections::HashSet::new();
    let mut refs: Vec<LinkedTextRef> = links
        .iter()
        .filter(|l| {
            l.direction == "incoming"
                && matches!(l.verb.as_str(), "modifie" | "cree" | "abroge")
                && l.target_nature.as_deref() == Some("LOI")
        })
        .filter_map(|l| {
            let uid = l.target_text_uid.as_deref()?;
            if !uid.starts_with("JORFTEXT") || !seen.insert(uid.to_string()) {
                return None;
            }
            Some(LinkedTextRef {
                // Le libellé DILA pointe l'article de la loi (« LOI n°2018-287
                // du 20 avril 2018 - art. 16 ») ; la ligne vise la loi.
                label: label_prefix(&l.target_label).to_string(),
                href: Some(format!("https://www.legifrance.gouv.fr/jorf/id/{uid}")),
                date: l.target_date.map(|d| d.to_string()),
            })
        })
        .collect();
    refs.sort_by(|a, b| b.date.cmp(&a.date));
    refs
}

/// Mappe un voisin du repo vers le DTO (numéro + état + flag courant, ADR 0114).
fn neighbor_dto(row: ArticleNeighborRow) -> ArticleNeighbor {
    ArticleNeighbor {
        num: row.num,
        num_key: row.num_key,
        etat: row.status,
        current: row.current,
    }
}

/// Résout le `code` d'URL en `text_uid` du texte de référentiel par lookup
/// **exact** du slug (ADR 0112 §6 / ADR 0123 §2). 404 si le slug est inconnu :
/// nos liens internes portent le slug canonique (DTO), une forme legacy/tapée-main
/// ne bénéficie plus du rattrapage flou (assumé, ADR 0123).
async fn resolve_text(repo: &DecisionRepository<'_>, code: &str) -> Result<String> {
    repo.resolve_referential_code(code)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Version en vigueur d'un article (`GET /api/texte/{code}/{num}`), avec sa
/// timeline. 404 si le code ou l'article n'est pas en vigueur.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_in_force(
    state: &AppState,
    code: &str,
    num: &str,
) -> Result<LawArticleResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL replié en clé d'identité (ADR 0236) : nos liens portent
    // `numKey`, mais l'HTML caché/externe porte encore des formes citables
    // (« L. 761-1 », « 1er ») — même clé, même page.
    let num_key = &lj_core::article_key::identity_key(num);

    let article = repo
        .law_article_at_date(&text_uid, num_key, None)
        .await?
        .ok_or(ApiError::NotFound)?;
    let consult_date = date_debut_display(article.date_debut);
    let versions = repo.law_article_versions(&text_uid, num_key).await?;
    let context = repo.article_context(&text_uid, num_key, None).await?;
    article_response(&repo, code, article, &consult_date, None, versions, context).await
}

/// Version d'un article à une date (`GET /api/texte/{code}/{num}/{date}`), avec sa
/// timeline. 404 si le code est inconnu ou si aucune version ne couvre la date.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date = %date))]
pub async fn article_at_date(
    state: &AppState,
    code: &str,
    num: &str,
    date: NaiveDate,
) -> Result<LawArticleResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL replié en clé d'identité (ADR 0236) : nos liens portent
    // `numKey`, mais l'HTML caché/externe porte encore des formes citables
    // (« L. 761-1 », « 1er ») — même clé, même page.
    let num_key = &lj_core::article_key::identity_key(num);

    let article = repo
        .law_article_at_date(&text_uid, num_key, Some(date))
        .await?
        .ok_or(ApiError::NotFound)?;
    let consult_date = date.to_string();
    let versions = repo.law_article_versions(&text_uid, num_key).await?;
    let context = repo.article_context(&text_uid, num_key, Some(date)).await?;
    article_response(
        &repo,
        code,
        article,
        &consult_date,
        Some(date),
        versions,
        context,
    )
    .await
}

/// Variante de [`article_at_date`] acceptant une date ISO `YYYY-MM-DD` en
/// chaîne, pour les appelants in-process (client SSR `lj-web`) qui ne
/// manipulent pas `chrono`. Format invalide ⇒ 422
/// ([`ApiError::Unprocessable`]) — même frontière de validation que la route
/// HTTP (#12).
pub async fn article_at_date_str(
    state: &AppState,
    code: &str,
    num: &str,
    date: &str,
) -> Result<LawArticleResponse> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", "date"],
            date,
            "invalid date",
        ))
    })?;
    article_at_date(state, code, num, parsed).await
}

/// Parse la date Chronolégi optionnelle des appelants in-process (client SSR,
/// qui ne manipule pas `chrono`). Format invalide ⇒ 422, même frontière que la
/// route HTTP (#12).
fn parse_opt_date(date: Option<&str>) -> Result<Option<NaiveDate>> {
    date.map(|d| {
        NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
            ApiError::Unprocessable(crate::error::validation::date_parsing(
                &["query", "date"],
                d,
                "invalid date",
            ))
        })
    })
    .transpose()
}

/// Variante de [`code_toc`] à date ISO chaîne optionnelle (client SSR).
pub async fn code_toc_str(
    state: &AppState,
    code: &str,
    date: Option<&str>,
) -> Result<CodeTocResponse> {
    code_toc(state, code, parse_opt_date(date)?).await
}

/// Variante de [`law_section`] à date ISO chaîne optionnelle (client SSR).
pub async fn law_section_str(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<&str>,
) -> Result<LawSectionResponse> {
    law_section(state, code, cid, parse_opt_date(date)?).await
}

/// Borne du comparateur de versions (ADR 0193) : date ISO tombant dans la
/// fenêtre de la version visée, ou `initiale` pour la version à borne ouverte
/// (sentinelle `0001-01-01`). Format invalide ⇒ 422, même frontière que la
/// route à date (#12).
fn parse_compare_bound(raw: &str, segment: &'static str) -> Result<NaiveDate> {
    if raw == "initiale" {
        return Ok(NaiveDate::from_ymd_opt(1, 1, 1).expect("date sentinelle"));
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", segment],
            raw,
            "invalid date",
        ))
    })
}

/// Métadonnées de version d'une ligne `legal_article` servie, avec la même
/// normalisation de sentinelles que la timeline ([`version_dto`]).
fn row_version_dto(row: &LegalArticleRow) -> LawArticleVersion {
    version_dto(LawVersionRow {
        source_uid: row.source_uid.clone(),
        status: row.status.clone(),
        date_debut: row.date_debut.map(|d| d.to_string()),
        date_fin: row.date_fin.map(|d| d.to_string()),
    })
}

/// Comparaison de deux rédactions d'un article (ADR 0193) :
/// `GET /api/texte/{code}/{num}/compare/{de}/{a}`. Chaque borne résout sa version
/// par la fenêtre de dates (law-at-date) ; 404 si le code est inconnu ou si une
/// borne ne couvre aucune version. Le diff est calculé côté serveur
/// (`lj_core::compare`, mot Unicode dans les blocs remplacés).
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, de = %de, a = %a))]
pub async fn article_compare(
    state: &AppState,
    code: &str,
    num: &str,
    de: &str,
    a: &str,
) -> Result<LawCompareResponse> {
    let from_date = parse_compare_bound(de, "de")?;
    let to_date = parse_compare_bound(a, "a")?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL replié en clé d'identité (ADR 0236) : nos liens portent
    // `numKey`, mais l'HTML caché/externe porte encore des formes citables
    // (« L. 761-1 », « 1er ») — même clé, même page.
    let num_key = &lj_core::article_key::identity_key(num);

    let from_row = repo
        .law_article_at_date(&text_uid, num_key, Some(from_date))
        .await?
        .ok_or(ApiError::NotFound)?;
    let to_row = repo
        .law_article_at_date(&text_uid, num_key, Some(to_date))
        .await?
        .ok_or(ApiError::NotFound)?;

    let segments = lj_core::compare::compare_texts(
        from_row.texte.as_deref().unwrap_or(""),
        to_row.texte.as_deref().unwrap_or(""),
    )
    .into_iter()
    .map(|s| LawCompareSegment {
        op: match s.op {
            lj_core::compare::CompareOp::Equal => LawCompareOp::Equal,
            lj_core::compare::CompareOp::Insert => LawCompareOp::Insert,
            lj_core::compare::CompareOp::Delete => LawCompareOp::Delete,
        },
        text: s.text,
    })
    .collect();

    let versions = repo.law_article_versions(&text_uid, num_key).await?;
    let code_title = repo.referential_title(code).await?;
    Ok(LawCompareResponse {
        code: code.to_string(),
        code_title,
        num: to_row.num.clone(),
        num_key: to_row.num_key.clone(),
        from: row_version_dto(&from_row),
        to: row_version_dto(&to_row),
        segments,
        versions: versions.into_iter().map(version_dto).collect(),
    })
}

/// Convertit une ligne `law_decisions_citing` en DTO citant : le `title`
/// machine est dérivé des métadonnées (même source que le détail/MCP), le
/// libellé de repli du type venant du référentiel (ADR 0146), la portée du
/// groupe de `publication_codes` (ADR 0167).
fn citing_dto(row: CitingDecisionRow, refs: &crate::referential::Referential) -> CitingDecisionHit {
    let dockets = row.docket_numbers.filter(|d| !d.is_empty());
    let title = decision_title(
        refs.jurisdiction_type_label(&row.jurisdiction_type)
            .unwrap_or(&row.jurisdiction_type),
        row.jurisdiction_name.as_deref(),
        None,
        row.date_lecture.as_deref(),
        dockets.as_deref(),
    );
    let jurisdiction_type = parse_jur_type(&row.jurisdiction_type).unwrap_or(JurisdictionType::Ta);
    let significance =
        match lj_core::publication::significance_key(&row.publication_codes.unwrap_or_default()) {
            "MAJEURE" => Significance::Majeure,
            "IMPORTANTE" => Significance::Importante,
            "LIMITEE" => Significance::Limitee,
            _ => Significance::Indeterminee,
        };
    CitingDecisionHit {
        id: row.public_id,
        title,
        jurisdiction_type,
        date_lecture: row.date_lecture,
        significance,
        summary: row.summary,
    }
}

/// Désérialise un code `jurisdiction_type` issu de la DB en [`JurisdictionType`]
/// (mapping serde des variantes). `None` si le code est inattendu.
fn parse_jur_type(raw: &str) -> Option<JurisdictionType> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Décisions citant un article (`GET /api/texte/{code}/{num}/citing`), paginées.
/// `date` (ISO `YYYY-MM-DD`, `None` = en vigueur) sélectionne la version servie ;
/// les citantes sont bornées à sa fenêtre de validité (une décision cite la
/// version en vigueur à sa date, cf. renumérotations). 404 si code/article
/// inconnu ; 422 si `date` mal formée.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date, limit, offset))]
pub async fn article_citing(
    state: &AppState,
    code: &str,
    num: &str,
    date: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CitingDecisionHit>> {
    let date = date
        .map(|d| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| {
                ApiError::Unprocessable(crate::error::validation::date_parsing(
                    &["query", "date"],
                    d,
                    "invalid date",
                ))
            })
        })
        .transpose()?;

    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL replié en clé d'identité (ADR 0236) : nos liens portent
    // `numKey`, mais l'HTML caché/externe porte encore des formes citables
    // (« L. 761-1 », « 1er ») — même clé, même page.
    let num_key = &lj_core::article_key::identity_key(num);

    // Version servie (même résolution que l'en-tête d'article) → sa fenêtre de
    // validité borne les citantes. `date_debut` NOT NULL en base (#12).
    let article = repo
        .law_article_at_date(&text_uid, num_key, date)
        .await?
        .ok_or(ApiError::NotFound)?;
    let date_debut = article
        .date_debut
        .ok_or_else(|| ApiError::Internal("version d'article sans date_debut".to_string()))?;

    let rows = repo
        .law_decisions_citing(
            &text_uid,
            num_key,
            date_debut,
            article.date_fin,
            limit,
            offset,
        )
        .await?;
    let refs = crate::referential::referential(state).await?;
    Ok(rows.into_iter().map(|r| citing_dto(r, &refs)).collect())
}

/// Articles co-cités (`GET /api/texte/{code}/{num}/related`) : croisement
/// `legal_citation` (« souvent cité avec », plan graphe Phase D), pondéré IDF
/// côté repo (ADR 0250 — le boilerplate procédural coule de lui-même). 404 si
/// le code est inconnu ; liste vide si l'article n'a pas de co-citations.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_co_cited(
    state: &AppState,
    code: &str,
    num: &str,
) -> Result<Vec<CoCitedArticle>> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // `num` d'URL replié en clé d'identité (ADR 0236), comme tous les autres
    // endpoints article — « L761-1 » externe et « l761-1 » interne = même page.
    let num_key = &lj_core::article_key::identity_key(num);
    let rows = repo.law_co_cited_articles(&text_uid, num_key, 8).await?;
    Ok(rows
        .into_iter()
        .map(|r| CoCitedArticle {
            href: r
                .text_slug
                .as_deref()
                .map(|slug| format!("/texte/{slug}/{}", r.num_key)),
            num_key: r.num_key,
            text_title: r.text_title,
            count: r.count,
        })
        .collect())
}

/// Sommaire d'un code (`GET /api/texte/{code}`) : métadonnées + nombre d'articles
/// en vigueur. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_summary(state: &AppState, code: &str) -> Result<LawCodeSummary> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let row = repo
        .law_code_summary(code)
        .await?
        .ok_or(ApiError::NotFound)?;
    // `slug` est non-NULL pour un code résolu par son slug (la requête filtre
    // `WHERE slug = $1`).
    let slug = row.slug.unwrap_or_else(|| code.to_string());
    let mut upcoming_versions: Vec<String> = row
        .upcoming_versions
        .iter()
        .map(|d| d.to_string())
        .collect();
    upcoming_versions.sort();
    // Sur-facette portée (ADR 0196) : libellé exposé seulement pour la
    // doctrine administrative — les normes n'ont pas de badge.
    let scope = lj_core::referential_labels::nature_scope(&row.nature);
    let scope = (scope != "norme").then(|| {
        lj_core::referential_labels::scope_label(scope)
            .unwrap_or(scope)
            .to_string()
    });
    // Maillage retour vers le hub fond×année du catalogue des normes
    // (ADR 0255) ; `None` pour un acte individuel.
    let fond_link = repo.norm_fond_of_text(&row.text_uid).await?;
    let (fond, fond_label, fond_year) = match fond_link {
        Some((fond, year)) => {
            let label = lj_core::referential_labels::norm_fond_label(&fond)
                .unwrap_or(&fond)
                .to_string();
            (Some(fond), Some(label), year)
        }
        None => (None, None, None),
    };
    Ok(LawCodeSummary {
        legitext: row.text_uid,
        code: slug,
        titre: row.title,
        nature: row.nature,
        derniere_modification: row.last_modified,
        article_count: row.article_count,
        upcoming_versions,
        body: row.body,
        status: row.status,
        nor: row.nor,
        date_texte: row.date_texte,
        scope,
        fond,
        fond_label,
        fond_year,
    })
}

/// Longueur cible d'un extrait d'article dans les résultats `/recherche-textes`.
const ARTICLE_SNIPPET_MAX: usize = 280;

/// Recherche plein-texte d'articles (`GET /api/search-textes`, ADR 0114),
/// **titre-primaire** : jambe titre (`search_title` formé, boostée + expansions
/// d'alias) > jambe corps. `code` optionnel borne la recherche à un texte (résolu
/// comme les pages `/texte`) ; absent ⇒ tout le référentiel navigable. Les facettes
/// `jurisdiction`/`nature`/`source` et le `total` sont comptés sous le même
/// prédicat BM25 + mêmes filtres que la page de hits, en une requête `GROUPING
/// SETS` lancée **en parallèle** des hits sur sa propre connexion — le prédicat
/// BM25 (~1 s plein corpus, ~430 k articles scorés sur une requête à tokens
/// fréquents) est le poste dominant, on ne le paie qu'une fois
/// par jambe. Chaque hit reçoit un extrait surligné via
/// [`crate::snippets::highlight`] (fallback corps tronqué).
#[instrument(skip(state), fields(db.system = "postgresql", limit, offset))]
#[allow(clippy::too_many_arguments)]
pub async fn search_textes(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    jurisdiction: Option<&str>,
    nature: Option<&str>,
    source: Option<&str>,
    scope: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<ArticleSearchResponse> {
    let checkout = || async {
        state
            .pool
            .get()
            .await
            .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))
    };
    let (conn_hits, conn_stats) = tokio::try_join!(checkout(), checkout())?;
    let repo_hits = DecisionRepository::new(&conn_hits);
    let repo_stats = DecisionRepository::new(&conn_stats);

    let text_uid = match code {
        Some(c) if !c.is_empty() => Some(resolve_text(&repo_hits, c).await?),
        _ => None,
    };
    // Expansion d'alias (acronymes/noms usuels → forme développée) OR-ée dans la
    // jambe titre — substitut lexical au sémantique (ADR 0114).
    let expansions = lj_core::aliases::expand_query(q);
    // Sur-facette « portée » (ADR 0196) → ensemble de natures (mapping code) :
    // doctrine administrative = liste fermée, norme = son complément ouvert.
    let doctrine_natures: Vec<String> = lj_core::referential_labels::DOCTRINE_ADMIN_NATURES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let nature_set: Option<(&[String], bool)> = match scope {
        None => None,
        Some("doctrine_administrative") => Some((&doctrine_natures, true)),
        Some("norme") => Some((&doctrine_natures, false)),
        Some(other) => {
            return Err(ApiError::Unprocessable(
                crate::error::validation::enum_error(
                    &["query", "scope"],
                    other,
                    &["norme", "doctrine_administrative"],
                ),
            ))
        }
    };
    let (rows, stats) = tokio::try_join!(
        repo_hits.search_articles(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            nature_set,
            limit,
            offset,
        ),
        repo_stats.article_search_stats(
            q,
            &expansions,
            text_uid.as_deref(),
            jurisdiction,
            nature,
            source,
            nature_set,
        ),
    )?;
    Ok(ArticleSearchResponse {
        hits: hits_with_snippets(rows, q),
        total: stats.total,
        facets: facets_dto(stats),
    })
}

/// Mappe les facettes du repo vers le DTO (chaque `FacetCount` devient un
/// `FacetChoice`, tri préservé : count décroissant puis valeur croissante),
/// libellés humanisés via `lj_core::referential_labels` (pays FR pour
/// `jurisdiction`, natures LEGI/curées pour `nature`, diffuseurs à jeton court
/// pour `source` — valeur brute en repli).
fn facets_dto(stats: ArticleSearchStats) -> ArticleSearchFacets {
    use lj_core::referential_labels::{jurisdiction_label, nature_label, source_label};
    let map = |rows: Vec<FacetCount>, label: fn(&str) -> Option<&'static str>| {
        rows.into_iter()
            .map(|row| FacetChoice {
                label: label(&row.value)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.value.clone()),
                value: row.value,
                count: row.count,
                parent: None,
            })
            .collect()
    };
    // Sur-facette portée (ADR 0196) : agrégat des buckets nature par mapping code.
    let mut scope_counts: std::collections::BTreeMap<&'static str, i64> =
        std::collections::BTreeMap::new();
    for row in &stats.nature {
        *scope_counts
            .entry(lj_core::referential_labels::nature_scope(&row.value))
            .or_default() += row.count;
    }
    let mut scope: Vec<FacetChoice> = scope_counts
        .into_iter()
        .map(|(value, count)| FacetChoice {
            label: lj_core::referential_labels::scope_label(value)
                .unwrap_or(value)
                .to_string(),
            value: value.to_string(),
            count,
            parent: None,
        })
        .collect();
    scope.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    ArticleSearchFacets {
        // Slugs auto-descriptifs (ADR 0209) : le label est la valeur.
        code: map(stats.code, |_| None),
        jurisdiction: map(stats.jurisdiction, jurisdiction_label),
        nature: map(stats.nature, nature_label),
        source: map(stats.source, source_label),
        scope,
    }
}

/// Catalogue des codes navigables (`GET /api/codes`) : un texte de référentiel
/// par entrée, avec son nombre d'articles. `slug` du texte = `code` du DTO (lien
/// `/texte/{code}`).
#[instrument(skip(state), fields(db.system = "postgresql"))]
pub async fn code_catalogue(state: &AppState, head_only: bool) -> Result<CodeCatalogueResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let rows = repo.list_legal_texts().await?;
    let total = rows.len() as u64;
    let entries: Vec<_> = rows
        .into_iter()
        .map(catalogue_entry)
        .filter(|e| !head_only || e.is_head())
        .collect();
    Ok(CodeCatalogueResponse { entries, total })
}

/// Mappe une ligne de catalogue du repo vers le DTO (slug → `code`).
fn catalogue_entry(row: LegalTextCatalogRow) -> CodeCatalogueEntry {
    CodeCatalogueEntry {
        code: row.slug,
        title: row.title,
        nature: row.nature,
        jurisdiction: row.jurisdiction,
        article_count: row.article_count,
    }
}

// Au-delà de ce nombre d'articles, le sommaire renvoie la table des matières
// (chips) ; en deçà, la vue-lecture intégrale — un texte court (BOFiP, décret,
// arrêté, loi ordinaire) se lit sur sa page. Le seuil couvre p95 des natures
// non-code (LOI 124, BOFIP 50) et laisse codes et CCN au sommaire. Partagé
// avec le front (éligibilité d'une division à l'accordéon, ADR 0214).
use lj_dtos::INLINE_READING_MAX;

/// Table des matières d'un code (`GET /api/texte/{code}/sommaire`) : articles
/// ordonnés, chacun avec son fil d'Ariane et son état. Textes courts
/// (≤ [`INLINE_READING_MAX`] articles) : `reading` porte la vue-lecture
/// intégrale (corps joints) à la place des entrées. 404 si le slug est inconnu
/// (même résolution que les pages `/texte`).
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_toc(
    state: &AppState,
    code: &str,
    date: Option<NaiveDate>,
) -> Result<CodeTocResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let text_uid = resolve_text(&repo, code).await?;
    // Arbre structurel réel daté (ADR 0207) à la date demandée (Chronolégi,
    // ADR 0193 §5) sinon en vigueur aujourd'hui ; repli sur le sommaire à plat
    // (title_path) pour les textes sans structure ingérée.
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let tree = repo.toc_tree(&text_uid, at).await?;
    if !tree.is_empty() {
        let n_articles = tree.iter().filter(|r| r.child_kind == "article").count();
        let reading = if n_articles <= INLINE_READING_MAX {
            repo.toc_text_reading(&text_uid, at)
                .await?
                .into_iter()
                .map(section_item)
                .collect()
        } else {
            Vec::new()
        };
        return Ok(CodeTocResponse {
            entries: Vec::new(),
            tree: tree.into_iter().map(toc_node).collect(),
            reading,
        });
    }
    let rows = repo.code_table_of_contents(&text_uid).await?;
    if rows.len() <= INLINE_READING_MAX {
        let reading = repo
            .flat_text_reading(&text_uid)
            .await?
            .into_iter()
            .map(section_item)
            .collect();
        return Ok(CodeTocResponse {
            entries: Vec::new(),
            tree: Vec::new(),
            reading,
        });
    }
    Ok(CodeTocResponse {
        entries: rows.into_iter().map(toc_entry).collect(),
        tree: Vec::new(),
        reading: Vec::new(),
    })
}

/// Mappe un nœud d'arbre structurel du repo vers le DTO (ADR 0207).
fn toc_node(row: TocTreeRow) -> TocNode {
    TocNode {
        kind: row.child_kind,
        depth: row.depth,
        label: row.label,
        cid: row.child_cid,
        num_key: row.child_num_key,
        etat: row.etat,
    }
}

/// Vue-lecture d'une section (`GET /api/texte/{code}/section/{cid}`, ADR 0207) :
/// le sous-arbre de la section en vigueur aujourd'hui, corps des articles
/// joints. 404 si le slug ou le `cid` est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, cid = %cid))]
pub async fn law_section(
    state: &AppState,
    code: &str,
    cid: &str,
    date: Option<NaiveDate>,
) -> Result<LawSectionResponse> {
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;
    let repo = DecisionRepository::new(&conn);

    let summary = repo
        .law_code_summary(code)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Chronolégi (ADR 0193 §5) : sous-arbre et corps à la date demandée,
    // sinon en vigueur aujourd'hui.
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let (title, items) = repo
        .toc_section_reading(&summary.text_uid, cid, at)
        .await?
        .ok_or(ApiError::NotFound)?;
    let tree = repo.toc_tree(&summary.text_uid, at).await?;
    let (ancestors, prev, next) = section_nav(&tree, cid);
    Ok(LawSectionResponse {
        code: code.to_string(),
        code_title: Some(summary.title),
        cid: cid.to_string(),
        title,
        ancestors,
        prev,
        next,
        items: items.into_iter().map(section_item).collect(),
    })
}

/// Situe une section dans l'arbre aplati en ordre de lecture : divisions
/// englobantes (racine → parent), bloc précédent et suivant — les voisins en
/// ordre de lecture hors sous-arbre (le suivant d'un Livre est le Livre
/// d'après, pas son premier Titre).
fn section_nav(
    tree: &[TocTreeRow],
    cid: &str,
) -> (
    Vec<LawSectionRef>,
    Option<LawSectionRef>,
    Option<LawSectionRef>,
) {
    let sref = |row: &TocTreeRow| LawSectionRef {
        cid: row.child_cid.clone().unwrap_or_default(),
        label: row.label.clone(),
    };
    let Some(at) = tree
        .iter()
        .position(|r| r.child_kind == "section" && r.child_cid.as_deref() == Some(cid))
    else {
        return (Vec::new(), None, None);
    };
    let depth = tree[at].depth;

    // Pile des divisions ouvertes jusqu'à la section : ses ancêtres.
    let mut stack: Vec<&TocTreeRow> = Vec::new();
    for row in &tree[..at] {
        if row.child_kind == "section" {
            stack.truncate((row.depth - 1) as usize);
            stack.push(row);
        }
    }
    stack.truncate((depth - 1) as usize);
    let ancestor_cids: Vec<Option<&str>> = stack.iter().map(|r| r.child_cid.as_deref()).collect();
    let ancestors = stack.iter().map(|r| sref(r)).collect();

    let next = tree[at + 1..]
        .iter()
        .find(|r| r.child_kind == "section" && r.depth <= depth)
        .map(sref);
    // Inverse du suivant : le dernier bloc en ordre de lecture avant la
    // section, quelle que soit sa profondeur (hors ancêtres).
    let prev = tree[..at]
        .iter()
        .rev()
        .find(|r| r.child_kind == "section" && !ancestor_cids.contains(&r.child_cid.as_deref()))
        .map(sref);
    (ancestors, prev, next)
}

/// Mappe un item de vue-lecture du repo vers le DTO (ADR 0207).
fn section_item(row: TocReadingRow) -> LawSectionItem {
    LawSectionItem {
        kind: row.child_kind,
        depth: row.depth,
        label: row.label,
        cid: row.child_cid,
        num_key: row.child_num_key,
        etat: row.etat,
        texte: row.texte,
        nota: row.nota,
    }
}

/// Mappe une ligne de sommaire du repo vers le DTO (`position` ignorée — l'ordre
/// est porté par la séquence de la requête).
fn toc_entry(row: TocArticleRow) -> TocEntry {
    TocEntry {
        num: row.num,
        num_key: row.num_key,
        title_path: row.title_path,
        status: row.status,
    }
}

/// Construit les hits DTO en surlignant le corps (`snippets::highlight`, mêmes
/// tokenizers que l'index). Les docs sont indexés par position (id synthétique) ;
/// un doc qui ne matche pas (absent du retour `highlight`) retombe sur un corps
/// tronqué. `slug` est non-NULL (la requête filtre `t.slug IS NOT NULL`).
fn hits_with_snippets(rows: Vec<ArticleSearchRow>, q: &str) -> Vec<ArticleSearchHit> {
    let docs: Vec<(i64, String)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i as i64, r.texte.clone().unwrap_or_default()))
        .collect();
    let snippets = crate::snippets::highlight(&docs, q, ARTICLE_SNIPPET_MAX);
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let snippet = snippets.get(&(i as i64)).cloned().unwrap_or_else(|| {
                truncate_plain(r.texte.as_deref().unwrap_or(""), ARTICLE_SNIPPET_MAX)
            });
            ArticleSearchHit {
                code: r.slug.unwrap_or_default(),
                code_title: r.code_title,
                num: r.num,
                num_key: r.num_key,
                titre_path: r.title_path,
                snippet,
                source: r.source,
            }
        })
        .collect()
}

/// Tronque un texte brut à `max` caractères (frontière char), suffixe `…` si
/// coupé. Fallback quand le corps ne matche pas la requête de surlignage.
fn truncate_plain(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(max) {
        Some((byte_idx, _)) => format!("{}…", &trimmed[..byte_idx]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arbre de test : L1 (T1 (C1, C2), T2), L2 — profondeurs 1/2/3.
    fn nav_tree() -> Vec<TocTreeRow> {
        let section = |cid: &str, depth: i32| TocTreeRow {
            depth,
            child_kind: "section".to_string(),
            child_uid: format!("uid-{cid}"),
            child_cid: Some(cid.to_string()),
            child_num_key: None,
            label: cid.to_uppercase(),
            etat: "VIGUEUR".to_string(),
        };
        let article = |num: &str, depth: i32| TocTreeRow {
            depth,
            child_kind: "article".to_string(),
            child_uid: format!("uid-a{num}"),
            child_cid: None,
            child_num_key: Some(num.to_string()),
            label: num.to_string(),
            etat: "VIGUEUR".to_string(),
        };
        vec![
            section("l1", 1),
            section("t1", 2),
            section("c1", 3),
            article("1", 4),
            section("c2", 3),
            article("2", 4),
            section("t2", 2),
            article("3", 3),
            section("l2", 1),
            article("4", 2),
        ]
    }

    #[test]
    fn section_nav_ancetres_et_voisins() {
        let tree = nav_tree();

        // c2 : ancêtres l1 > t1, précédent c1, suivant t2 (hors sous-arbre).
        let (anc, prev, next) = section_nav(&tree, "c2");
        assert_eq!(
            anc.iter().map(|r| r.cid.as_str()).collect::<Vec<_>>(),
            ["l1", "t1"]
        );
        assert_eq!(prev.unwrap().cid, "c1");
        assert_eq!(next.unwrap().cid, "t2");

        // t2 : le suivant saute au Livre 2 ; le précédent est le dernier bloc
        // lu (c2), pas son parent t1.
        let (anc, prev, next) = section_nav(&tree, "t2");
        assert_eq!(anc.len(), 1);
        assert_eq!(prev.unwrap().cid, "c2");
        assert_eq!(next.unwrap().cid, "l2");

        // Bornes : l1 n'a pas de précédent, l2 pas de suivant, racine sans ancêtre.
        let (anc, prev, next) = section_nav(&tree, "l1");
        assert!(anc.is_empty() && prev.is_none());
        assert_eq!(next.unwrap().cid, "l2");
        let (_, _, next) = section_nav(&tree, "l2");
        assert!(next.is_none());

        // cid inconnu (texte sans arbre) : rien.
        let (anc, prev, next) = section_nav(&tree, "zzz");
        assert!(anc.is_empty() && prev.is_none() && next.is_none());
    }

    #[test]
    fn legifrance_source_url_native_is_versioned() {
        assert_eq!(
            legifrance_source_url("LEGIARTI000006419292", "1992-05-15").as_deref(),
            Some("https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006419292/1992-05-15")
        );
    }

    #[test]
    fn legifrance_source_url_none_for_curated() {
        // Source curée (`source_uid` synthétique) : pas de page Légifrance ; elle porte
        // sa propre `source_url` (ADR 0131) — pas de lien Légifrance fabriqué.
        assert_eq!(
            legifrance_source_url("JORFTEXT000000694290#6@1968-12-27", "2002-12-26"),
            None
        );
    }

    #[test]
    fn parse_jur_type_maps_known_codes() {
        assert_eq!(parse_jur_type("CC"), Some(JurisdictionType::Cc));
        assert_eq!(parse_jur_type("TA"), Some(JurisdictionType::Ta));
        assert_eq!(parse_jur_type("ZZZ"), None);
    }

    #[test]
    fn citing_dto_builds_title_and_falls_back_jur_type() {
        let row = CitingDecisionRow {
            id: 42,
            public_id: "CETATEXT000012345678".to_string(),
            jurisdiction_type: "CE".to_string(),
            jurisdiction_name: Some("Conseil d'État".to_string()),
            date_lecture: Some("2024-02-13".to_string()),
            docket_numbers: Some(vec!["123456".to_string()]),
            publication_codes: Some(vec!["A".to_string()]),
            summary: None,
        };
        let refs = crate::referential::Referential::new(Vec::new(), Vec::new());
        let hit = citing_dto(row, &refs);
        assert_eq!(hit.id, "CETATEXT000012345678");
        assert_eq!(hit.jurisdiction_type, JurisdictionType::Ce);
        assert_eq!(hit.title, "Conseil d'État, 13 février 2024, 123456");
        assert_eq!(hit.date_lecture.as_deref(), Some("2024-02-13"));
        // « A » = Recueil Lebon → portée majeure (groupes ADR 0167).
        assert_eq!(hit.significance, Significance::Majeure);
    }
}
