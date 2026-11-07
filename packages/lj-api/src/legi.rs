//! Référentiel de droit positif — law-at-date, timeline, citations, sommaire de
//! code.
//!
//! Handlers purs `(&AppState, …) -> Result<DTO>` (même forme que
//! [`crate::decisions`]) ; l'adaptation axum (path/query, headers de cache) vit
//! dans [`crate::routes`]. Le `code` d'URL est résolu en `text_uid` par lookup
//! **exact** sur le slug ([`DecisionRepository::referential_ref`], caché in-process) ; rien
//! ne matche ⇒ 404 ([`ApiError::NotFound`]). Le `num` d'URL **est déjà** la clé
//! canonique (`num_key`) : nos liens internes la portent (DTO `numKey`), aucune
//! normalisation au runtime serve.

use chrono::{Datelike, NaiveDate};
use lj_dtos::{
    Applicability, ApplicabilityStatus, ArticleModification, ArticleNeighbor, ArticleSearchFacets,
    ArticleSearchHit, ArticleSearchResponse, CitationSpan, CitationTarget, CitingDecisionHit,
    CoCitedArticle, CodeCatalogueEntry, CodeCatalogueResponse, CodeTocResponse, Commentaire,
    CorpusGroup, CorpusJournal, CorpusText, FacetChoice, JurisdictionType, LawArticleResponse,
    LawArticleVersion, LawCodeSummary, LawCompareOp, LawCompareResponse, LawCompareRow,
    LawCompareRowKind, LawCompareSegment, LawSectionItem, LawSectionRef, LawSectionResponse,
    LinkedTextRef, ModificationItem, PlaceCorpusResponse, PlaceGroup, PlaceNode, PlaceRef,
    PlacesResponse, ReadablePlace, SectionCitingText, StateElsewhere, TocEntry, TocNode,
    TopCitedArticle, UndeterminedReason, DEFAULT_READER_PLACE,
};
use lj_norm::journal_rules::{self, matter_of, Regime, Rule};
use lj_norm::source_authority::{is_live_authoritative, source_authority};
use lj_norm::toc_path::{toc_paths, TocKind, TocNodePath, TocPaths};
use lj_store::repository::{
    covering_states, ArticleNeighborRow, ArticleSearchRow, ArticleSearchStats, AssertionRow,
    CitingDecisionRow, DecisionRepository, FacetCount, LawVersionRow, LegalArticleRow,
    LegalTextCatalogRow, PlaceFilter, PlaceReading, PlacedElsewhere, ResolvedLegalLink,
    TocArticleRow, TocReadingRow, TocTreeRow,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::state::AppState;
use crate::titles::decision_title;

/// The jurisdiction a URL segment names, for the in-process caller that has no router seam to convert it.
pub use lj_norm::jurisdictions::from_url_segment;

/// URL Légifrance absolue et versionnée d'un article **natif** Légifrance
/// (`source_uid` = LEGIARTI…) : `/codes/article_lc/{LEGIARTI}/{date}`, `date` ISO
/// `YYYY-MM-DD` étant la date de consultation (date demandée pour la
/// version-à-date, sinon `date_debut` de la version servie). `None` pour une
/// source curée (droit étranger, traités : `source_uid` synthétique
/// `text_uid#…`) qui porte déjà sa propre `source_url`. Sert à *remplir*
/// `source_url` quand elle est absente.
fn legifrance_source_url(source_uid: &str, date: &str) -> Option<String> {
    if source_uid.contains('#') {
        return None;
    }
    Some(format!(
        "https://www.legifrance.gouv.fr/codes/article_lc/{source_uid}/{date}"
    ))
}

/// Rend une `date_debut` en ISO `YYYY-MM-DD`, en mappant la sentinelle de borne
/// ouverte (`0001-01-01`, posée par l'upsert pour les versions sans début connu)
/// vers la chaîne vide (DTO `dateDebut` vide = pas de début). Un `None` est traité
/// de même.
fn date_debut_display(date_debut: Option<NaiveDate>) -> String {
    match date_debut {
        Some(d) if d.year() != 1 => d.to_string(),
        _ => String::new(),
    }
}

/// Date de début pour l'AFFICHAGE (DTO `dateDebut`) : comme [`date_debut_display`]
/// mais absorbe aussi la sentinelle LEGI `2999-01-01`, posée sur `date_debut` des
/// articles **modificatifs** sans date propre. L'URL Légifrance garde 2999 (elle
/// la résout) via la `consult_date`, calculée à part par [`date_debut_display`].
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

/// A DILA label carries the line breaks of its XML, collapsed by HTML but literal in
/// the MCP JSON.
fn tidy_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Le chemin d'un texte quand les deux côtés de sa jointure ont répondu.
fn norm_path_of(jurisdiction: Option<&str>, slug: Option<&str>) -> Option<String> {
    Some(lj_vocab::norm_path(jurisdiction?, slug?))
}

/// A graph edge as a linked reference: a guaranteed article link when the target
/// is resolved, the cid address (`…/section/{cid}`, redirected to the path) for a
/// section target, else the carrying text's table of contents, else a bare label.
fn linked_text_ref(l: ResolvedLegalLink) -> LinkedTextRef {
    let resolved = norm_path_of(
        l.resolved_jurisdiction.as_deref(),
        l.resolved_slug.as_deref(),
    );
    let target = norm_path_of(
        l.target_text_jurisdiction.as_deref(),
        l.target_text_slug.as_deref(),
    );
    let href = match (resolved, l.resolved_num_key) {
        (Some(path), Some(num_key)) => Some(format!("{path}/{num_key}")),
        _ => match (&target, &l.resolved_section_cid) {
            (Some(path), Some(cid)) => Some(format!("{path}/section/{cid}")),
            _ => target,
        },
    };
    LinkedTextRef {
        label: tidy_label(&l.target_label),
        href,
        date: l.target_date.map(|d| d.to_string()),
    }
}

/// Dérive les blocs de la page article depuis ses arêtes `legal_link` :
/// dispositions modifiées (sortant modifie/crée/abroge, groupé **globalement**
/// par action × texte cible dans l'ordre de première apparition — le XML
/// entrelace les actions section par section), « Modifié par » (entrant
/// modificatoire, trié par date), « Cité par » (CITATION entrant, dédupliqué —
/// la réciprocité DILA peut répéter une cible). Le sortant CITATION n'est pas
/// rendu : la citation se lit dans le texte, sur son span.
fn link_blocks(
    links: Vec<ResolvedLegalLink>,
) -> (
    Vec<ArticleModification>,
    Vec<LinkedTextRef>,
    Vec<LinkedTextRef>,
) {
    let mut modifications: Vec<ArticleModification> = Vec::new();
    let mut group_index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut modified_by: Vec<LinkedTextRef> = Vec::new();
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
                let target = norm_path_of(
                    l.target_text_jurisdiction.as_deref(),
                    l.target_text_slug.as_deref(),
                );
                let resolved = norm_path_of(
                    l.resolved_jurisdiction.as_deref(),
                    l.resolved_slug.as_deref(),
                );
                let code_href = target.clone();
                let item = match l.target_kind.as_str() {
                    "article" => ModificationItem {
                        href: match (&resolved, &l.resolved_num_key) {
                            (Some(path), Some(num_key)) => Some(format!("{path}/{num_key}")),
                            _ => None,
                        },
                        kind: "article".to_string(),
                        label: l
                            .target_num
                            .unwrap_or_else(|| label_suffix(&l.target_label).to_string()),
                    },
                    // Cible section : l'adresse de son cid, que le serveur
                    // redirige vers le chemin de la division.
                    "section" => ModificationItem {
                        kind: "section".to_string(),
                        label: label_suffix(&l.target_label).to_string(),
                        href: match (&target, &l.resolved_section_cid) {
                            (Some(path), Some(cid)) => Some(format!("{path}/section/{cid}")),
                            _ => None,
                        },
                    },
                    _ => ModificationItem {
                        kind: "texte".to_string(),
                        label: l.target_label.clone(),
                        href: target,
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
            "cite" if !incoming => {}
            "cite" => cited_by.push(linked_text_ref(l)),
            // codification, concordance, transfert… : stockés, non exposés.
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
    for list in [&mut modified_by, &mut cited_by] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|r| seen.insert((r.label.clone(), r.href.clone())));
    }
    (modifications, modified_by, cited_by)
}

/// Convertit une ligne de timeline du repo en DTO version (dates ISO telles
/// quelles). `dateDebut` du DTO est requise : une borne ouverte (sentinelle
/// `0001-01-01` posée par l'upsert, ou source non versionnée) est rendue en
/// chaîne vide ; `dateFin` absente ⇒ pas de fin. `source_uid` = identifiant natif
/// de la version (LEGIARTI…).
fn version_dto(row: LawVersionRow) -> LawArticleVersion {
    // Sentinelles LEGI de borne ouverte / date absente : `0001-01-01` (borne
    // ouverte upsert) et `2999-01-01` (article modificatif sans date propre).
    let date_debut = match row.date_debut {
        Some(d) if d != "0001-01-01" && d != "2999-01-01" => d,
        _ => String::new(),
    };
    // Version future = pas encore entrée en vigueur. Comparaison lexicale ISO ;
    // la sentinelle 2999 est déjà absorbée ci-dessus, 2222 (date inconnue) reste
    // marquée « à venir ».
    let upcoming =
        !date_debut.is_empty() && date_debut > chrono::Utc::now().date_naive().to_string();
    LawArticleVersion {
        legiarti: row.source_uid,
        etat: row.status,
        date_debut,
        // The JORF closes a version it never ends on `2998-12-31`.
        date_fin: row.date_fin.filter(|d| d.as_str() < "2998-12-31"),
        upcoming,
    }
}

/// A place as the vocabulary keys it. The web and the connector write `PF` for
/// `geo:PF`, and `FR`, the Republic rather than a place one reads from, for
/// metropolitan France.
pub fn place_uid(code: &str) -> String {
    match code {
        "FR" => "geo:FR-M".to_string(),
        code => format!("geo:{code}"),
    }
}

/// A place as the web and the connector write it.
pub fn place_code(uid: &str) -> &str {
    uid.strip_prefix("geo:")
        .expect("a place of the vocabulary is keyed geo:")
}

/// Where and when the article is read: the place the states are matched
/// against, resolved to its ancestors, and the date, `None` meaning today.
pub struct Reader {
    place: String,
    path: Vec<String>,
    date: Option<NaiveDate>,
}

impl Reader {
    fn day(&self) -> NaiveDate {
        self.date.unwrap_or_else(|| chrono::Utc::now().date_naive())
    }

    /// A division's href at the reader's date: its path is the one the tree held
    /// that day, which today's may not.
    fn section_href(&self, href: String) -> String {
        match self.date {
            Some(day) => self.href(format!("{href}?date={day}")),
            None => self.href(href),
        }
    }

    /// The reader's place carried on an internal href, when it is not the default.
    fn href(&self, href: String) -> String {
        let place = place_code(&self.place);
        if place == DEFAULT_READER_PLACE {
            return href;
        }
        let (path, fragment) = match href.split_once('#') {
            Some((p, f)) => (p.to_string(), format!("#{f}")),
            None => (href, String::new()),
        };
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{path}{sep}place={place}{fragment}")
    }
}

/// Resolves the reader's place on the vocabulary; an unknown place is a 422.
async fn reader(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    place: Option<&str>,
    date: Option<NaiveDate>,
) -> Result<Reader> {
    let default = place_uid(DEFAULT_READER_PLACE);
    let place = place.unwrap_or(&default);
    let path = match state.place_path_cache.get(place).await {
        Some(hit) => hit,
        None => {
            let path = Arc::new(repo.place_path(place).await?);
            if !path.is_empty() {
                state
                    .place_path_cache
                    .insert(place.to_string(), path.clone())
                    .await;
            }
            path
        }
    };
    if path.is_empty() {
        return Err(ApiError::Unprocessable(
            crate::error::validation::value_error(
                &["query", "place"],
                place_code(place),
                "unknown place",
            ),
        ));
    }
    Ok(Reader {
        place: place.to_string(),
        path: (*path).clone(),
        date,
    })
}

/// Where a reader is read from, as `legal_text.jurisdiction` codes: the country
/// of their place, the territories on their path whose own journal reaches it,
/// the supranational orders that reach it, and INTL, which every place shares.
/// A reader who has named no place is read from France, so the answer is the
/// one every caller had before the place became a setting.
async fn reader_home(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    place: Option<&str>,
) -> Result<Vec<String>> {
    let domestic = || {
        lj_norm::jurisdictions::DOMESTIC_REGIONS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
    };
    let Some(place) = place.filter(|p| place_code(p) != DEFAULT_READER_PLACE) else {
        return Ok(domestic());
    };
    if let Some(hit) = state.reader_home_cache.get(place).await {
        return Ok((*hit).clone());
    }
    let rd = reader(state, repo, Some(place), None).await?;
    let root = rd.path.last().expect("a reader path is never empty");
    let country = root.strip_prefix("geo:").unwrap_or(root);
    let mut home = vec![country.to_string()];
    for uid in &rd.path[..rd.path.len() - 1] {
        let local = uid.strip_prefix("geo:").unwrap_or(uid);
        let journal = lj_norm::applicability::journal_code("", local);
        if journal_rules::reaches(&journal, &rd.path) {
            home.push(local.to_string());
        }
    }
    if journal_rules::reaches("JOUE", &rd.path) {
        home.push("EU".to_string());
    }
    home.push("INTL".to_string());
    state
        .reader_home_cache
        .insert(place.to_string(), Arc::new(home.clone()))
        .await;
    Ok(home)
}

/// The texts `place` rules out on `day`, per jurisdiction it belongs to, INTL
/// left out, a treaty being read from every place: those its page says do not
/// apply there. A text placed elsewhere by name stays when its own key reaches
/// the place on mention, a doubt.
async fn place_texts(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    place: &str,
    day: NaiveDate,
) -> Result<Arc<BTreeMap<String, PlaceFilter>>> {
    let key = format!("{place}@{day}");
    if let Some(hit) = state.place_texts_cache.get(&key).await {
        return Ok(hit);
    }
    let rd = reader(state, repo, Some(place), Some(day)).await?;
    let mut keeping = journal_rules::keys_reaching(&rd.path, to_jiff(day));
    keeping.extend(journal_rules::keys_on_mention(&rd.path, to_jiff(day)));
    let dropping: Vec<String> = journal_rules::all_key_spans()
        .map(|(key, _)| key.to_string())
        .filter(|key| !keeping.contains(key))
        .collect();
    let reading = PlaceReading {
        path: &rd.path,
        day,
        keeping: &keeping,
        dropping: &dropping,
    };
    let home: Vec<String> = reader_home(state, repo, Some(place))
        .await?
        .into_iter()
        .filter(|j| j != "INTL")
        .collect();
    let journals: Vec<String> = home
        .iter()
        .flat_map(|j| {
            let own = lj_norm::applicability::journal_code("", j);
            lj_norm::applicability::journals()
                .iter()
                .filter(|r| r.defaults_from == Some(own.as_str()))
                .map(|r| r.code.to_string())
                .chain([own.clone()])
                .collect::<Vec<_>>()
        })
        .collect();
    let mut texts: BTreeMap<String, PlaceFilter> = home
        .iter()
        .map(|j| (j.clone(), PlaceFilter::default()))
        .collect();
    for text in repo
        .texts_placed_elsewhere(&reading, &home, &journals)
        .await?
    {
        if is_doubt(&text, &rd.path, day) {
            continue;
        }
        let filter = texts.entry(text.jurisdiction).or_default();
        if text.articles {
            filter.articles.push(text.text_uid.clone());
        }
        if text.body {
            filter.bodies.push(text.text_uid);
        }
    }
    let texts = Arc::new(texts);
    state.place_texts_cache.insert(key, texts.clone()).await;
    Ok(texts)
}

/// Whether a text placed elsewhere is a doubt where `path` reads: placed there
/// by name only, under a key of its own that reaches the place on mention.
fn is_doubt(text: &PlacedElsewhere, path: &[String], day: NaiveDate) -> bool {
    !text.keyed
        && Reception::new(&text.text_uid, &text.jurisdiction, &text.title, "")
            .rule(path, day)
            .is_some_and(|r| r.regime == Regime::Express)
}

/// The states of an article as a reader meets them: the one covering them,
/// the others covering them too, and those covering the day somewhere else.
struct ReaderStates<'a> {
    served: Option<&'a AssertionRow>,
    competing: Vec<&'a AssertionRow>,
    elsewhere: Vec<&'a AssertionRow>,
}

fn reader_states<'a>(rows: &'a [AssertionRow], rd: &Reader) -> ReaderStates<'a> {
    let day = rd.day();
    let covering = covering_states(rows, &rd.path, day);
    let served = covering.first().copied();
    let elsewhere = match served {
        Some(_) => Vec::new(),
        None => rows
            .iter()
            .filter(|a| a.starts_on <= day && a.ends_on.is_none_or(|e| e > day))
            .collect(),
    };
    ReaderStates {
        served,
        competing: covering.into_iter().skip(1).collect(),
        elsewhere,
    }
}

fn place_ref(uid: &str, labels: &std::collections::HashMap<String, String>) -> PlaceRef {
    PlaceRef {
        uid: place_code(uid).to_string(),
        label: labels
            .get(uid)
            .cloned()
            .unwrap_or_else(|| place_code(uid).to_string()),
    }
}

/// A start the source does not know is stored at the sentinel and shown absent.
fn bound_display(d: NaiveDate) -> Option<String> {
    (d.year() != 1).then(|| d.to_string())
}

fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8).expect("valid date")
}

fn to_chrono(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32).expect("valid date")
}

/// How an article's text is received: the journal it resolves on and its
/// matter, read as the compiler reads them.
struct Reception {
    journal: String,
    matter: Option<&'static str>,
}

impl Reception {
    fn of(text: &TextRef, num_key: &str) -> Self {
        Reception::new(
            &text.text_uid,
            &text.jurisdiction,
            text.title.as_deref().unwrap_or(""),
            num_key,
        )
    }

    fn new(text_uid: &str, jurisdiction: &str, title: &str, num_key: &str) -> Self {
        let journal = lj_norm::applicability::journal_code(text_uid, jurisdiction);
        let matter = matter_of(&journal, text_uid, num_key, title);
        Reception { journal, matter }
    }

    /// The line of the most specific place of `path` on `day`.
    fn rule(&self, path: &[String], day: NaiveDate) -> Option<&'static Rule> {
        path.iter()
            .find_map(|p| journal_rules::resolve(&self.journal, p, self.matter, to_jiff(day)))
    }

    /// The day of the transfer whose wording the reader's place keeps.
    fn frozen_on(&self, rd: &Reader) -> Option<NaiveDate> {
        self.rule(&rd.path, rd.day())
            .filter(|r| r.regime == Regime::Frozen)
            .and_then(|r| r.since)
            .map(to_chrono)
    }
}

/// The heading a place is picked under: the metropole, then the overseas
/// places the JORF reaches by default or on express mention, then the States.
fn place_group(place: &str, day: NaiveDate) -> PlaceGroup {
    if matches!(place, "geo:FR-M" | "geo:FR-ALS") {
        return PlaceGroup::Metropole;
    }
    match journal_rules::resolve("JORF", place, None, to_jiff(day)).map(|r| r.regime) {
        Some(Regime::Default) => PlaceGroup::OverseasIdentity,
        Some(_) => PlaceGroup::OverseasSpecialty,
        None => PlaceGroup::OtherStates,
    }
}

/// Whether a version was in force on `day`, its end being inclusive.
fn in_force_on(start: Option<NaiveDate>, end: Option<NaiveDate>, day: NaiveDate) -> bool {
    start.is_none_or(|s| s <= day) && end.is_none_or(|e| e >= day)
}

/// What an article is where no state covers the reader: the rule of the most
/// specific place of their path decides, a frozen matter keeping only an
/// article in force on the day of the transfer.
fn uncovered_status(
    rule: Option<&Rule>,
    elsewhere: bool,
    in_force_at: impl Fn(NaiveDate) -> bool,
) -> ApplicabilityStatus {
    match rule.map(|r| (r.regime, r.since)) {
        None | Some((Regime::Excluded, _)) => ApplicabilityStatus::NotApplicable,
        Some((Regime::Frozen, since)) => {
            if in_force_at(to_chrono(since.expect("a frozen line names its day"))) {
                ApplicabilityStatus::Undetermined(UndeterminedReason::Frozen)
            } else {
                ApplicabilityStatus::NotApplicable
            }
        }
        Some(_) if !elsewhere => ApplicabilityStatus::OutsideWindow,
        Some((Regime::Express, _)) => {
            ApplicabilityStatus::Undetermined(UndeterminedReason::NoMention)
        }
        Some((Regime::Default, _)) => ApplicabilityStatus::NotApplicable,
    }
}

/// An article a decision cites, with what its reception is read from.
pub(crate) struct CitedArticle {
    pub text_uid: String,
    pub jurisdiction: String,
    pub title: String,
    pub num_key: String,
}

/// The articles of `cited` that do not apply at `place` on `day`: a decision
/// of a court of that place links none of them. An article with no state is
/// not among them, nor an undetermined one.
pub(crate) async fn not_applicable_at(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    place: &str,
    day: NaiveDate,
    cited: &[CitedArticle],
) -> Result<std::collections::HashSet<(String, String)>> {
    let rd = reader(state, repo, Some(place), Some(day)).await?;
    let key = |c: &CitedArticle| (c.text_uid.clone(), c.num_key.clone());
    let articles: Vec<(String, String)> = cited.iter().map(key).collect();
    let places = repo.places_on(&articles, day).await?;
    let uncovered: Vec<(&CitedArticle, Option<&'static Rule>, bool)> = cited
        .iter()
        .filter_map(|c| {
            let on = places.get(&key(c))?;
            let covered = on.iter().any(|p| rd.path.contains(p));
            let rule = Reception::new(&c.text_uid, &c.jurisdiction, &c.title, &c.num_key)
                .rule(&rd.path, day);
            (!covered).then_some((c, rule, !on.is_empty()))
        })
        .collect();
    let frozen: Vec<(String, String)> = uncovered
        .iter()
        .filter(|(_, rule, _)| rule.is_some_and(|r| r.regime == Regime::Frozen))
        .map(|(c, _, _)| key(c))
        .collect();
    let windows = if frozen.is_empty() {
        Default::default()
    } else {
        repo.version_windows(&frozen).await?
    };
    Ok(uncovered
        .into_iter()
        .filter(|(c, rule, elsewhere)| {
            let in_force_at = |since| {
                windows
                    .get(&key(c))
                    .is_some_and(|ws| ws.iter().any(|&(s, e)| in_force_on(s, e, since)))
            };
            uncovered_status(*rule, *elsewhere, in_force_at) == ApplicabilityStatus::NotApplicable
        })
        .map(|(c, _, _)| key(c))
        .collect())
}

/// What the article says for the reader: the state covering them, the others
/// covering them when the source left two, else what the rule of reception of
/// their place decides, and where the article can be read from. `None` while
/// the text has no state.
async fn applicability(
    repo: &DecisionRepository<'_>,
    text: &TextRef,
    article: &LegalArticleRow,
    rows: &[AssertionRow],
    versions: &[LawVersionRow],
    rd: &Reader,
) -> Result<Option<Applicability>> {
    if rows.is_empty() {
        return Ok(None);
    }
    let day = rd.day();
    let states = reader_states(rows, rd);
    let served = states.served;
    let reception = Reception::of(text, &article.num_key);
    let rule = reception.rule(&rd.path, day);
    let frozen_on = reception.frozen_on(rd);
    let status = match served {
        Some(_) => ApplicabilityStatus::Applicable,
        None => uncovered_status(rule, !states.elsewhere.is_empty(), |since| {
            in_force_on(article.date_debut, article.date_fin, since)
        }),
    };
    let mut readable: std::collections::BTreeMap<String, Option<String>> = rows
        .iter()
        .filter(|a| a.starts_on <= day && a.ends_on.is_none_or(|e| e > day))
        .flat_map(|a| {
            let version = a.version_start.to_string();
            a.places
                .iter()
                .map(move |p| (p.clone(), Some(version.clone())))
        })
        .collect();
    let parse = |d: &Option<String>| {
        d.as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    };
    for r in journal_rules::rules_of(&reception.journal) {
        let frozen =
            journal_rules::resolve(&reception.journal, r.place, reception.matter, to_jiff(day))
                .filter(|line| line.regime == Regime::Frozen)
                .and_then(|line| line.since)
                .map(to_chrono);
        if let Some(since) = frozen {
            if let Some(v) = versions
                .iter()
                .find(|v| in_force_on(parse(&v.date_debut), parse(&v.date_fin), since))
            {
                readable
                    .entry(r.place.to_string())
                    .or_insert(v.date_debut.clone());
            }
        }
    }
    readable.entry(rd.place.clone()).or_insert(None);
    let mut uids: Vec<String> = readable.keys().cloned().collect();
    for a in served
        .iter()
        .chain(&states.competing)
        .chain(&states.elsewhere)
    {
        uids.extend(a.places.iter().cloned());
    }
    let labels = repo.place_labels(&uids).await?;
    let state_ref = |a: &&AssertionRow| StateElsewhere {
        places: a.places.iter().map(|p| place_ref(p, &labels)).collect(),
        starts_on: bound_display(a.starts_on).unwrap_or_default(),
        version: a.version_start.to_string(),
    };
    let mut readable: Vec<ReadablePlace> = readable
        .into_iter()
        .map(|(p, version)| ReadablePlace {
            place: place_ref(&p, &labels),
            group: place_group(&p, day),
            version,
        })
        .collect();
    readable.sort_by(|a, b| (a.group, &a.place.label).cmp(&(b.group, &b.place.label)));
    Ok(Some(Applicability {
        reader: place_ref(&rd.place, &labels),
        status,
        places: served
            .map(|a| a.places.iter().map(|p| place_ref(p, &labels)).collect())
            .unwrap_or_default(),
        starts_on: served.and_then(|a| bound_display(a.starts_on)),
        starts_on_source: served.and_then(|a| a.start_source.clone()),
        ends_on: served.and_then(|a| a.ends_on.map(|d| d.to_string())),
        ends_on_origin: served.and_then(|a| a.end_origin.clone()),
        ends_on_source: served.and_then(|a| a.end_source.clone()),
        competing: states.competing.iter().map(state_ref).collect(),
        elsewhere: states.elsewhere.iter().map(state_ref).collect(),
        rule_source: rule
            .filter(|r| {
                r.regime != Regime::Default
                    && matches!(
                        status,
                        ApplicabilityStatus::NotApplicable | ApplicabilityStatus::Undetermined(_)
                    )
            })
            .map(|r| r.source.to_string()),
        frozen_on: frozen_on
            .filter(|_| served.is_none())
            .map(|d| d.to_string()),
        readable,
    }))
}

/// How many titles a group of the corpus carries: enough to recognise what a
/// journal covers, not the whole fund.
const CORPUS_PER_GROUP: i64 = 12;

/// The nature of a group as the page shows it. The source leaves the field
/// empty on some texts of the fund, which are a group like any other: they are
/// named for what they are rather than by a blank heading.
fn corpus_nature_label(nature: &str) -> String {
    if nature.is_empty() {
        return "Nature non précisée".to_string();
    }
    lj_norm::referential_labels::nature_label(nature)
        .unwrap_or(nature)
        .to_string()
}

/// What applies in a place at a date (`GET /internal/corpus`), grouped by
/// journal then nature. The whole metropolitan fund is one of the answers, so
/// the result is memoized on the pair asked.
pub async fn place_corpus(
    state: &AppState,
    place: Option<&str>,
    date: Option<NaiveDate>,
) -> Result<PlaceCorpusResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let rd = reader(state, &repo, place.map(place_uid).as_deref(), date).await?;
    let day = rd.day();
    let key = format!("{}@{day}", rd.place);
    if let Some(hit) = state.place_corpus_cache.get(&key).await {
        return Ok((*hit).clone());
    }
    let keys = journal_rules::keys_reaching(&rd.path, to_jiff(day));
    let rows = repo
        .place_corpus(&rd.path, &keys, day, CORPUS_PER_GROUP)
        .await?;
    let labels = repo.place_labels(std::slice::from_ref(&rd.place)).await?;
    let mut journals: Vec<CorpusJournal> = Vec::new();
    for row in rows {
        let Some(slug) = row.slug.clone() else {
            continue;
        };
        if journals.last().is_none_or(|j| j.code != row.journal_code) {
            let label = lj_norm::applicability::journal(&row.journal_code)
                .map(|j| j.label.to_string())
                .unwrap_or_else(|| row.journal_code.clone());
            journals.push(CorpusJournal {
                code: row.journal_code.clone(),
                label,
                total: 0,
                groups: Vec::new(),
            });
        }
        let journal = journals.last_mut().expect("a journal was just pushed");
        if journal.groups.last().is_none_or(|g| g.nature != row.nature) {
            journal.total += row.total;
            journal.groups.push(CorpusGroup {
                nature: row.nature.clone(),
                label: corpus_nature_label(&row.nature),
                total: row.total,
                texts: Vec::new(),
            });
        }
        journal
            .groups
            .last_mut()
            .expect("a group was just pushed")
            .texts
            .push(CorpusText {
                title: row.title,
                slug,
                jurisdiction: row.jurisdiction,
            });
    }
    // The fold walks the SQL order, `(journal, nature)`, and the page reads by
    // weight: the JORF before a bulletin of two texts, a decree before a
    // memorandum.
    for journal in &mut journals {
        journal
            .groups
            .sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.label.cmp(&b.label)));
    }
    journals.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.label.cmp(&b.label)));
    let body = PlaceCorpusResponse {
        place: place_ref(&rd.place, &labels),
        date: day.to_string(),
        total: journals.iter().map(|j| j.total).sum(),
        journals,
    };
    state
        .place_corpus_cache
        .insert(key, Arc::new(body.clone()))
        .await;
    Ok(body)
}

/// The corpus of a place read on a day given as an ISO string, for the SSR
/// client which holds the URL segment and not a date.
pub async fn place_corpus_str(
    state: &AppState,
    place: Option<&str>,
    date: Option<&str>,
) -> Result<PlaceCorpusResponse> {
    let parsed = match date {
        Some(raw) => Some(NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
            ApiError::Unprocessable(crate::error::validation::date_parsing(
                &["path", "date"],
                raw,
                "invalid date",
            ))
        })?),
        None => None,
    };
    place_corpus(state, place, parsed).await
}

/// The place vocabulary (`GET /internal/places`), roots first, each place
/// under its heading of today.
pub async fn places(state: &AppState) -> Result<PlacesResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let today = chrono::Utc::now().date_naive();
    let places = repo
        .places()
        .await?
        .into_iter()
        .map(|(uid, label, parent)| PlaceNode {
            group: place_group(&uid, today),
            uid: place_code(&uid).to_string(),
            label,
            parent: parent.as_deref().map(|p| place_code(p).to_string()),
        })
        .collect();
    Ok(PlacesResponse { places })
}

/// Assemble la réponse article (version servie + timeline) à partir d'une ligne
/// `legal_article` et de la timeline. `consult_date` est la date de consultation
/// pour l'URL Légifrance (date demandée, sinon `date_debut` de la version, sinon
/// chaîne vide pour une borne ouverte). `legiarti` = identifiant natif de la
/// version (`source_uid`, LEGIARTI…) ; `legitext` = identité du texte
/// (`text_uid`).
#[allow(clippy::too_many_arguments)]
async fn article_response(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    text: &TextRef,
    article: LegalArticleRow,
    consult_date: &str,
    rd: &Reader,
    states: Vec<AssertionRow>,
    versions: Vec<LawVersionRow>,
    context: Vec<ArticleNeighborRow>,
) -> Result<LawArticleResponse> {
    let href_date = rd.date;
    let applicability = applicability(repo, text, &article, &states, &versions, rd).await?;
    let links = repo
        .article_links(&article.text_uid, &article.num_key, article.date_debut)
        .await?;
    // Renvois du corps : spans de la version servie, hrefs internes — datés quand
    // la lecture l'est (`href_date` = date demandée de la route). Article ciblé →
    // `/norm/{slug}/{numKey}`, mention nue → sommaire seulement si le texte a des
    // articles, sinon pas de span du tout.
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
        .filter_map(|s| citation_span(s, href_date, |h| rd.href(h)))
        .collect();
    let travaux_parlementaires = travaux_refs(&links);
    let (modifications, modified_by, cited_by) = link_blocks(links);
    let with_place = |r: LinkedTextRef| LinkedTextRef {
        href: r.href.map(|h| rd.href(h)),
        ..r
    };
    let modified_by: Vec<LinkedTextRef> = modified_by.into_iter().map(with_place).collect();
    let public_info: Vec<String> = lj_norm::referential_labels::PUBLIC_INFO_NATURES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let sheets = repo
        .texts_citing_article(&article.text_uid, &article.num_key, &public_info)
        .await?
        .into_iter()
        .map(|t| LinkedTextRef {
            label: match lj_norm::referential_labels::nature_label(&t.nature) {
                Some(kind) => format!("{kind} : {}", t.title),
                None => t.title,
            },
            href: Some(lj_vocab::norm_path(&t.jurisdiction, &t.slug)),
            date: t.last_modified.map(|d| d.to_string()),
        });
    let cited_by: Vec<LinkedTextRef> = cited_by.into_iter().chain(sheets).map(with_place).collect();
    let modifications: Vec<ArticleModification> = modifications
        .into_iter()
        .map(|m| ArticleModification {
            code_href: m.code_href.map(|h| rd.href(h)),
            items: m
                .items
                .into_iter()
                .map(|i| ModificationItem {
                    href: i.href.map(|h| rd.href(h)),
                    ..i
                })
                .collect(),
            ..m
        })
        .collect();
    // Commentaires de norme : bundles ancrés sur (texte, article) et sur le texte
    // entier, forme `commentaires[]` partagée avec la décision. Ancre = le slug
    // canonique : la clé que la page porte, pas l'identité physique du texte —
    // celle-ci bouge en source.
    let commentaires = match repo
        .article_commentaires(&text.slug, &article.num_key)
        .await?
    {
        Some(v) => serde_json::from_value::<Vec<Commentaire>>(v)
            .map_err(|e| ApiError::Internal(format!("commentaires invalides: {e}")))?,
        None => Vec::new(),
    };
    let date_debut = display_start_date(article.date_debut);
    let expired = article
        .date_fin
        .is_some_and(|f| f < chrono::Utc::now().date_naive());
    let date_fin = article
        .date_fin
        .filter(|d| d.year() < 2998)
        .map(|d| d.to_string());
    // Lien « source » unique : l'URL curée si présente (droit étranger, traités),
    // sinon — pour un article natif Légifrance — la page Légifrance versionnée.
    let source_url = article
        .source_url
        .clone()
        .or_else(|| legifrance_source_url(&article.source_uid, consult_date));
    // Fraîcheur effective : `source_asof` par ligne (curé / bulk / jafbase), sinon
    // — pour les sources live re-synchronisées (legifrance/kali) — la date du
    // dernier sync (`ingest_freshness`). Axe distinct de `translation`.
    let source_asof = match article.source_asof {
        Some(d) => Some(d.to_string()),
        None if is_live_authoritative(&article.source) => {
            ingest_freshness(state, repo, &article.source)
                .await?
                .map(|d| d.to_string())
        }
        None => None,
    };
    let authority = source_authority(&article.source);
    // Titre humain du texte pour l'en-tête « Article N du … » : le `code` d'URL est un
    // slug. Porté par la résolution du slug — même ligne `legal_text`, déjà lue.
    let code_title = text.title.clone();
    // Frise + bandeau « sera modifié le … » : versions converties une fois,
    // première date future extraite.
    let versions: Vec<LawArticleVersion> = versions.into_iter().map(version_dto).collect();
    let upcoming_version_date = versions
        .iter()
        .filter(|v| v.upcoming)
        .map(|v| v.date_debut.clone())
        .min();
    // Fil d'Ariane TOC cliquable : divisions enclosantes de la version servie,
    // hrefs vers la vue-lecture de section. Vide hors TOC (JORF, étranger) — le
    // front retombe sur `titre_text`.
    let crumbs = repo
        .article_toc_breadcrumb(&article.text_uid, &article.source_uid)
        .await?;
    let toc = if crumbs.is_empty() {
        None
    } else {
        Some(toc_addresses(state, repo, &article.text_uid, rd.day()).await?)
    };
    let text_path = lj_vocab::norm_path(&text.jurisdiction, &text.slug);
    let breadcrumb = crumbs
        .into_iter()
        .map(|(label, cid)| LinkedTextRef {
            label,
            href: cid
                .zip(toc.as_ref())
                .and_then(|(c, toc)| toc.cid_href(&text_path, &c))
                .map(|href| rd.section_href(href)),
            date: None,
        })
        .collect();
    Ok(LawArticleResponse {
        legiarti: article.source_uid,
        legitext: article.text_uid,
        code: text.slug.clone(),
        jurisdiction: text.jurisdiction.clone(),
        jurisdiction_label: jurisdiction_name(&text.jurisdiction),
        code_title,
        num: article.num,
        num_key: article.num_key,
        etat: article.status,
        date_debut,
        source: article.source,
        titre_text: article.title_path,
        breadcrumb,
        date_fin,
        expired,
        texte: article.texte,
        texte_spans,
        texte_original: article.texte_original,
        lang_original: article.lang_original,
        translation: article.translation,
        source_asof,
        source_authority: authority.label().to_string(),
        source_official: authority.is_official(),
        source_url,
        source_upstream_url: article.source_upstream_url,
        nota: article.nota,
        upcoming_version_date,
        versions,
        context: context.into_iter().map(neighbor_dto).collect(),
        modifications,
        modified_by,
        cited_by,
        commentaires,
        travaux_parlementaires,
        applicability,
    })
}

/// Travaux parlementaires : une ligne par **loi** modificatrice de l'article
/// (arêtes entrantes modifie/crée/abroge, nature LOI, texte JORFTEXT),
/// dédupliquée par texte, triée par date décroissante. `href` composé vers la
/// page Légifrance de la loi au JO, qui porte le bloc « Travaux préparatoires »
/// et les liens de dossiers législatifs.
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
                label: tidy_label(label_prefix(&l.target_label)),
                href: Some(format!("https://www.legifrance.gouv.fr/jorf/id/{uid}")),
                date: l.target_date.map(|d| d.to_string()),
            })
        })
        .collect();
    refs.sort_by(|a, b| b.date.cmp(&a.date));
    refs
}

fn neighbor_dto(row: ArticleNeighborRow) -> ArticleNeighbor {
    ArticleNeighbor {
        num: row.num,
        num_key: row.num_key,
        etat: row.status,
        current: row.current,
    }
}

/// Identité d'un texte de référentiel résolue depuis son slug d'URL : le
/// `text_uid` qui clé toutes les requêtes en aval, le **slug canonique** que
/// portent les DTO, et le titre humain de l'en-tête. Valeur du cache
/// [`AppState::text_ref_cache`], partagée par `Arc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRef {
    pub text_uid: String,
    pub slug: String,
    pub title: Option<String>,
    pub jurisdiction: String,
}

/// Résout le `code` d'URL en identité de texte de référentiel. Le lookup suit
/// les renommages de slug : le DTO rend alors le slug d'aujourd'hui,
/// et la page SSR, qui lit l'écart avec l'URL demandée, répond 301. 404 si le
/// slug n'est ni vivant ni redirigé.
///
/// Servi depuis le cache in-process (TTL 1 h) : le référentiel ne bouge qu'à
/// l'ingest.
async fn resolve_text(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    code: &str,
    juris: Option<&str>,
) -> Result<Arc<TextRef>> {
    let key = match juris {
        Some(j) => format!("{j}/{code}"),
        None => code.to_string(),
    };
    if let Some(hit) = state.text_ref_cache.get(&key).await {
        return hit.ok_or(ApiError::NotFound);
    }
    let found =
        repo.referential_ref(code, juris)
            .await?
            .map(|(text_uid, slug, title, jurisdiction)| {
                Arc::new(TextRef {
                    text_uid,
                    slug,
                    title,
                    jurisdiction,
                })
            });
    state.text_ref_cache.insert(key, found.clone()).await;
    found.ok_or(ApiError::NotFound)
}

/// Fraîcheur « as-of » d'une source live, servie depuis le cache in-process
/// (TTL 1 h).
async fn ingest_freshness(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    source: &str,
) -> Result<Option<NaiveDate>> {
    if let Some(hit) = state.ingest_freshness_cache.get(source).await {
        return Ok(hit);
    }
    let asof = repo.get_ingest_freshness(source).await?;
    state
        .ingest_freshness_cache
        .insert(source.to_string(), asof)
        .await;
    Ok(asof)
}

/// Version d'un article à une date, `None` valant « en vigueur », clé compacte
/// rattrapée (`l80a` pour `l80-a`) et motif à la place du `not_found` muet.
///
/// Rend la clé servie : timeline, contexte et URL doivent porter la même, sinon
/// la réponse mélange deux articles.
///
/// A compiled state covering the reader names the version first; a matter
/// frozen at the reader's place names the version of the day of its transfer;
/// the window of `legal_article` answers otherwise, which keeps the paths equal
/// wherever the states only repeat the source.
async fn served_version(
    repo: &DecisionRepository<'_>,
    text: &TextRef,
    num_key: &str,
    rd: &Reader,
) -> Result<(String, LegalArticleRow, Vec<AssertionRow>)> {
    let text_uid = text.text_uid.as_str();
    let date = rd.date;
    let separee = lj_norm::article_key::letter_suffix_separated(num_key);
    for (i, cle) in lj_norm::article_key::candidate_keys(num_key)
        .iter()
        .enumerate()
    {
        let states = repo.assertions_of_article(text_uid, cle).await?;
        let mut served = None;
        if let Some(a) = covering_states(&states, &rd.path, rd.day()).first() {
            served = repo
                .law_article_version(text_uid, cle, a.version_start)
                .await?;
            if served.is_none() {
                tracing::warn!(
                    text_uid,
                    cle,
                    version = %a.version_start,
                    "article: the state names a missing version, falling back to the source window"
                );
            }
        } else if let Some(since) = Reception::of(text, cle).frozen_on(rd) {
            served = repo.law_article_at_date(text_uid, cle, Some(since)).await?;
        }
        let served = match served {
            Some(row) => Some(row),
            None => repo.law_article_at_date(text_uid, cle, date).await?,
        };
        if let Some(article) = served {
            if i > 0 {
                tracing::info!(demande = num_key, servi = %cle, "article: key recovered");
            }
            return Ok((cle.to_string(), article, states));
        }
    }
    let mut cle = num_key.to_string();
    let mut versions = repo.law_article_versions(text_uid, num_key).await?;
    if versions.is_empty() {
        if let Some(c) = &separee {
            versions = repo.law_article_versions(text_uid, c).await?;
            cle = c.clone();
        }
    }
    // Asymétrie voulue : après la dernière fenêtre, le texte tel qu'il a cessé de
    // s'appliquer répond. Avant la première, servir la plus ancienne version
    // l'appliquerait à des faits qu'elle ne régissait pas, donc erreur.
    if let (Some(d), Some(fin)) = (date, derniere_fin(&versions)) {
        if d > fin {
            if let Some(article) = repo
                .law_article_last_version_before(text_uid, &cle, d)
                .await?
            {
                let states = repo.assertions_of_article(text_uid, &cle).await?;
                return Ok((cle, article, states));
            }
        }
    }
    Err(no_version_error(&versions, date))
}

/// Fin de la dernière fenêtre de validité, `None` si une version court toujours
/// ou si aucune n'est bornée.
fn derniere_fin(versions: &[LawVersionRow]) -> Option<NaiveDate> {
    if versions.iter().any(|v| v.date_fin.is_none()) {
        return None;
    }
    versions
        .iter()
        .filter_map(|v| v.date_fin.as_deref())
        .filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .max()
}

/// The versions closing before `date` and opening after it, the reasoning living
/// in `lj_dtos::version_gap`: the front decides the same question over its own
/// version type.
fn encadrement(
    versions: &[LawVersionRow],
    date: NaiveDate,
) -> (Option<&LawVersionRow>, Option<&LawVersionRow>) {
    let jour = date.to_string();
    let bornes: Vec<_> = versions
        .iter()
        .map(|v| (v.date_debut.as_deref(), v.date_fin.as_deref()))
        .collect();
    match lj_dtos::version_gap(&bornes, &jour) {
        Some((i, j)) => (versions.get(i), versions.get(j)),
        None => (None, None),
    }
}

/// English wording of a status that closes a gap, `None` when it says nothing and
/// the version missing from it is ours to hold.
fn gap_motif(status: &str) -> Option<&'static str> {
    if !lj_dtos::gap_explique(status) {
        return None;
    }
    Some(match status {
        "ANNULE" => "annulled",
        "PERIME" => "lapsed",
        "TRANSFERE" => "moved to another number",
        _ => "repealed",
    })
}

/// Why a read returns nothing, from the set of versions alone.
///
/// A `VIGUEUR*` version that does not cover the date is no repeal: the message
/// then asserts nothing about the law.
fn no_version_error(versions: &[LawVersionRow], date: Option<NaiveDate>) -> ApiError {
    if versions.is_empty() {
        return ApiError::NotFound;
    }
    if versions
        .iter()
        .all(|v| v.date_debut.as_deref() == Some(lj_dtos::DATE_A_DETERMINER))
    {
        return ApiError::NoVersionAtDate(
            "law article adopted but not yet in force: its entry into force is deferred to \
             a date the publisher has not fixed"
                .to_string(),
        );
    }
    let debut = versions
        .iter()
        .filter_map(|v| v.date_debut.as_deref())
        .filter(|d| *d != lj_dtos::DATE_A_DETERMINER)
        .min();
    // An unbounded `date_fin` still runs, so the span ends at now.
    let ouverte = versions.iter().any(|v| v.date_fin.is_none());
    let fin = versions.iter().filter_map(|v| v.date_fin.as_deref()).max();
    let borne_haute = if ouverte { "now" } else { fin.unwrap_or("now") };
    if versions.iter().any(|v| v.status.starts_with("VIGUEUR")) {
        // A gap between two live versions is read at its own edge: the status
        // that closes it says why nothing covers the date.
        if let Some((d, av, ap)) = date.and_then(|d| match encadrement(versions, d) {
            (Some(av), Some(ap)) => Some((d, av, ap)),
            _ => None,
        }) {
            let ferme = av.date_fin.as_deref().unwrap_or("?");
            let reprise = ap.date_debut.as_deref().unwrap_or("?");
            return ApiError::NoVersionAtDate(match gap_motif(&av.status) {
                Some(motif) => format!(
                    "law article {motif} on {ferme}, held again from {reprise}: \
                     no version covers that date"
                ),
                None => {
                    tracing::warn!(
                        date = %d,
                        ferme,
                        reprise,
                        statut = %av.status,
                        "article: version gap between two live versions"
                    );
                    format!(
                        "law article runs on both sides of that date but no version \
                         is held between {ferme} and {reprise}"
                    )
                }
            });
        }
        return ApiError::NoVersionAtDate(match debut {
            Some(a) => format!(
                "law article exists but no version covers that date \
                 (known versions span {a} to {borne_haute})"
            ),
            None => "law article exists but no version covers that date".to_string(),
        });
    }
    ApiError::NoVersionAtDate(match fin {
        Some(b) => format!(
            "law article repealed: no version in force at that date, the last one ran to {b}"
        ),
        None => "law article repealed: no version in force at that date".to_string(),
    })
}

/// Version en vigueur d'un article (`GET /api/norm/{code}/{num}`), avec sa
/// timeline. 404 si le code ou l'article n'est pas en vigueur.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_in_force(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    place: Option<&str>,
) -> Result<LawArticleResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_norm::article_key::identity_key(num);
    let rd = reader(state, &repo, place.map(place_uid).as_deref(), None).await?;

    let (num_key, article, states) = served_version(&repo, &text, num_key, &rd).await?;
    let consult_date = date_debut_display(article.date_debut);
    let versions = repo.law_article_versions(text_uid, &num_key).await?;
    let context = repo.article_context(text_uid, &num_key, None).await?;
    article_response(
        state,
        &repo,
        &text,
        article,
        &consult_date,
        &rd,
        states,
        versions,
        context,
    )
    .await
}

/// Version d'un article à une date (`GET /api/norm/{code}/{num}/{date}`), avec sa
/// timeline. 404 si le code est inconnu ou si aucune version ne couvre la date.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date = %date))]
pub async fn article_at_date(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    date: NaiveDate,
    place: Option<&str>,
) -> Result<LawArticleResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_norm::article_key::identity_key(num);
    let rd = reader(state, &repo, place.map(place_uid).as_deref(), Some(date)).await?;

    let (num_key, article, states) = served_version(&repo, &text, num_key, &rd).await?;
    let consult_date = date.to_string();
    let versions = repo.law_article_versions(text_uid, &num_key).await?;
    let context = repo.article_context(text_uid, &num_key, Some(date)).await?;
    article_response(
        state,
        &repo,
        &text,
        article,
        &consult_date,
        &rd,
        states,
        versions,
        context,
    )
    .await
}

/// Variante de [`article_at_date`] acceptant une date ISO `YYYY-MM-DD` en
/// chaîne, pour les appelants in-process (client SSR `lj-web`) qui ne
/// manipulent pas `chrono`. Format invalide ⇒ 422
/// ([`ApiError::Unprocessable`]).
pub async fn article_at_date_str(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    date: &str,
    place: Option<&str>,
) -> Result<LawArticleResponse> {
    let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["path", "date"],
            date,
            "invalid date",
        ))
    })?;
    article_at_date(state, code, juris, num, parsed, place).await
}

/// Parse la date Chronolégi optionnelle des appelants in-process (client SSR,
/// qui ne manipule pas `chrono`). Format invalide ⇒ 422.
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
    juris: Option<&str>,
    date: Option<&str>,
) -> Result<CodeTocResponse> {
    code_toc(state, code, juris, parse_opt_date(date)?).await
}

/// Variante de [`law_section`] à date ISO chaîne optionnelle (client SSR).
pub async fn law_section_str(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    path: &str,
    date: Option<&str>,
) -> Result<LawSectionResponse> {
    law_section(state, code, juris, path, parse_opt_date(date)?).await
}

/// Borne du comparateur de versions : date ISO tombant dans la fenêtre de la
/// version visée, ou `initiale` pour la version à borne ouverte (sentinelle
/// `0001-01-01`). Format invalide ⇒ 422.
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

/// Comparaison de deux rédactions d'un article :
/// `GET /api/norm/{code}/{num}/compare/{de}/{a}`. Chaque borne résout sa version
/// par la fenêtre de dates (law-at-date) ; 404 si le code est inconnu ou si une
/// borne ne couvre aucune version. Le diff est calculé côté serveur
/// (`lj_norm::compare`, mot Unicode dans les blocs remplacés).
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, de = %de, a = %a))]
pub async fn article_compare(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    de: &str,
    a: &str,
) -> Result<LawCompareResponse> {
    let from_date = parse_compare_bound(de, "de")?;
    let to_date = parse_compare_bound(a, "a")?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_norm::article_key::identity_key(num);

    let from_row = repo
        .law_article_at_date(text_uid, num_key, Some(from_date))
        .await?
        .ok_or(ApiError::NotFound)?;
    let to_row = repo
        .law_article_at_date(text_uid, num_key, Some(to_date))
        .await?
        .ok_or(ApiError::NotFound)?;

    let rows = lj_norm::compare::align_texts(
        from_row.texte.as_deref().unwrap_or(""),
        to_row.texte.as_deref().unwrap_or(""),
    )
    .into_iter()
    .map(|r| LawCompareRow {
        kind: match r.kind {
            lj_norm::compare::RowKind::Equal => LawCompareRowKind::Equal,
            lj_norm::compare::RowKind::Changed => LawCompareRowKind::Changed,
            lj_norm::compare::RowKind::Added => LawCompareRowKind::Added,
            lj_norm::compare::RowKind::Removed => LawCompareRowKind::Removed,
        },
        left: r.left.into_iter().map(compare_segment_dto).collect(),
        right: r.right.into_iter().map(compare_segment_dto).collect(),
    })
    .collect();

    let versions = repo.law_article_versions(text_uid, num_key).await?;
    Ok(LawCompareResponse {
        code: text.slug.clone(),
        jurisdiction: text.jurisdiction.clone(),
        code_title: text.title.clone(),
        num: to_row.num.clone(),
        num_key: to_row.num_key.clone(),
        from: row_version_dto(&from_row),
        to: row_version_dto(&to_row),
        rows,
        versions: versions.into_iter().map(version_dto).collect(),
    })
}

fn compare_segment_dto(segment: lj_norm::compare::CompareSegment) -> LawCompareSegment {
    LawCompareSegment {
        op: match segment.op {
            lj_norm::compare::CompareOp::Equal => LawCompareOp::Equal,
            lj_norm::compare::CompareOp::Insert => LawCompareOp::Insert,
            lj_norm::compare::CompareOp::Delete => LawCompareOp::Delete,
        },
        text: segment.text,
    }
}

/// Convertit une ligne `law_decisions_citing` en DTO citant : le `title`
/// machine est dérivé des métadonnées (même source que le détail/MCP), le
/// libellé de repli du type venant du référentiel, la portée du groupe de
/// `publication_codes`.
pub(crate) fn citing_dto(
    row: CitingDecisionRow,
    refs: &crate::referential::Referential,
) -> CitingDecisionHit {
    let dockets = row.docket_numbers.filter(|d| !d.is_empty());
    let title = decision_title(
        refs.jurisdiction_type_label(&row.jurisdiction_type)
            .unwrap_or(&row.jurisdiction_type),
        row.jurisdiction_name.as_deref(),
        None,
        row.date_lecture.as_deref(),
        None,
        dockets.as_deref(),
    );
    let jurisdiction_type = parse_jur_type(&row.jurisdiction_type).unwrap_or(JurisdictionType::Ta);
    CitingDecisionHit {
        id: row.public_id,
        title,
        jurisdiction_type,
        jurisdiction_code: row.jurisdiction_code,
        date_lecture: row.date_lecture,
        publication_codes: row.publication_codes.unwrap_or_default(),
        summary: row.summary.as_deref().map(lj_summary::summary_teaser),
        cited_articles: row.cited_num_keys,
    }
}

/// Désérialise un code `jurisdiction_type` issu de la DB en [`JurisdictionType`]
/// (mapping serde des variantes). `None` si le code est inattendu.
fn parse_jur_type(raw: &str) -> Option<JurisdictionType> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()
}

/// Décisions citant un article (`GET /api/norm/{code}/{num}/citing`), paginées.
/// `date` (ISO `YYYY-MM-DD`, `None` = en vigueur) sélectionne la version servie ;
/// les citantes sont bornées à sa fenêtre de validité (une décision cite la
/// version en vigueur à sa date, cf. renumérotations). 404 si code/article
/// inconnu ; 422 si `date` mal formée.
#[allow(clippy::too_many_arguments)]
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num, date, limit, offset))]
pub async fn article_citing(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
    date: Option<&str>,
    place: Option<&str>,
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

    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let text_uid = &text.text_uid;
    let num_key = &lj_norm::article_key::identity_key(num);

    // Version servie (même résolution que l'en-tête d'article) → sa fenêtre de
    // validité borne les citantes. `date_debut` NOT NULL en base.
    let rd = reader(state, &repo, place.map(place_uid).as_deref(), date).await?;
    let (num_key, article, _) = served_version(&repo, &text, num_key, &rd).await?;
    let num_key = num_key.as_str();
    let date_debut = article
        .date_debut
        .ok_or_else(|| ApiError::Internal("version d'article sans date_debut".to_string()))?;

    let rows = repo
        .law_decisions_citing(
            text_uid,
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

/// Articles interrogés pour les citantes d'une division. La liste des `num_key`
/// d'un Livre en compte des milliers, et le coût d'une union de citantes croît
/// avec le nombre de termes : on classe d'abord le sous-arbre par
/// `citing_decision_counts`, puis on ne lit les décisions que pour les articles
/// de tête. Même borne que le « voir tout » du front, qui compose une facette à
/// une valeur par article.
const SECTION_CITING_ARTICLES: usize = 20;

/// Articles du palmarès « les plus cités » d'une division ou d'un code (colonne
/// de rebond).
const SECTION_TOP_CITED: usize = 5;

/// Vivier du palmarès d'un texte entier : les termes les plus cités ne sont pas
/// tous des articles encore présents dans le texte (renumérotations, transferts).
/// On en tire quatre fois le plafond avant de joindre le catalogue.
const TOP_CITED_CANDIDATES: usize = SECTION_TOP_CITED * 4;

/// Textes citants d'une division : même plafond que les décisions du bloc.
const SECTION_CITING_TEXTS: usize = 5;

/// Seuil de débruitage des textes citants d'une division : un texte qui ne vise
/// qu'un seul de ses articles n'est pas un texte de la division. Deux articles
/// visés = une application de la division.
const SECTION_CITING_TEXT_MIN_ARTICLES: i64 = 2;

/// Decisions citing a DIVISION (`GET /api/norm/{code}/section-citing/{path}`),
/// paginated: union of the citing decisions of the division's **whole subtree** at
/// the requested date, deduplicated by decision and sorted like an article's citing
/// decisions. `date` (ISO `YYYY-MM-DD`, `None` = in force) picks the subtree's
/// composition. 404 if the code or the path is unknown; 422 if `date` is malformed.
///
/// Cost bounded by [`SECTION_CITING_ARTICLES`]: only the division's most cited
/// articles are queried, the ranking coming from the counter table.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, path = %path, date, limit, offset))]
pub async fn section_citing(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    path: &str,
    date: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CitingDecisionHit>> {
    let date = parse_opt_date(date)?;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let toc = toc_addresses(state, &repo, &text.text_uid, at).await?;
    let index = toc.paths.find(path).ok_or(ApiError::NotFound)?;
    let mut keys: Vec<String> = toc
        .paths
        .subtree(index)
        .iter()
        .filter(|n| n.kind == TocKind::Article)
        .filter_map(|n| toc.row(n).child_num_key.clone())
        .collect();
    keys.sort();
    keys.dedup();
    let top: Vec<String> = repo
        .citing_counts_for_articles(&text.text_uid, &keys)
        .await?
        .into_iter()
        .take(SECTION_CITING_ARTICLES)
        .map(|(num_key, _)| num_key)
        .collect();
    let rows = repo
        .law_decisions_citing_any(&text.text_uid, &top, limit, offset)
        .await?;
    let refs = crate::referential::referential(state).await?;
    Ok(rows.into_iter().map(|r| citing_dto(r, &refs)).collect())
}

/// Articles co-cités (`GET /api/norm/{code}/{num}/related`) : croisement
/// `legal_citation` (« souvent cité avec »), pondéré IDF côté repo. 404 si le
/// code est inconnu ; liste vide si l'article n'a pas de co-citations.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, num = %num))]
pub async fn article_co_cited(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    num: &str,
) -> Result<Vec<CoCitedArticle>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let num_key = &lj_norm::article_key::identity_key(num);
    let rows = repo
        .law_co_cited_articles(&text.text_uid, num_key, 8)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| CoCitedArticle {
            href: r.text_slug.as_deref().map(|slug| {
                format!(
                    "{}/{}",
                    lj_vocab::norm_path(&r.text_jurisdiction, slug),
                    r.num_key
                )
            }),
            num_key: r.num_key,
            text_title: r.text_title,
            count: r.count,
        })
        .collect())
}

/// The canonical address of a text named by its slug alone: today's slug under
/// the segment of the jurisdiction that enacts it. `None` when no text answers.
pub async fn text_address(state: &AppState, code: &str) -> Option<String> {
    let conn = state.pool.get().await.ok()?;
    let repo = DecisionRepository::new(&conn);
    let text = resolve_text(state, &repo, code, None).await.ok()?;
    Some(lj_vocab::norm_path(&text.jurisdiction, &text.slug))
}

/// Relaxed resolution of an URL `code`: slug, text name or acronym. The browser
/// only ever emits slugs and keeps the exact lookup of [`resolve_text`]; `mcp.rs`
/// resolves through the same repository call but keeps the candidates, of which
/// it makes a "did you mean".
pub async fn resolve_code(
    state: &AppState,
    raw: &str,
    jurisdiction: Option<&str>,
) -> Result<String> {
    use lj_store::repository::InstrumentResolution as R;
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    match repo.resolve_instrument(raw, jurisdiction).await? {
        R::Direct { slug, .. } | R::Rattrape { slug, .. } => Ok(slug),
        R::Inconnu { .. } => Err(ApiError::NotFound),
    }
}

/// What `/norm/{code}` answers with no article segment.
pub enum LawRoot {
    /// A text with no articles *is* its body (circulaire, publication decree,
    /// guidelines), readable no other way.
    Body(LawCodeSummary),
    Toc(LawCodeSummary, CodeTocResponse),
}

pub async fn law_root(state: &AppState, code: &str, juris: Option<&str>) -> Result<LawRoot> {
    let summary = code_summary(state, code, juris).await?;
    if summary.article_count == 0 && summary.body.is_some() {
        return Ok(LawRoot::Body(summary));
    }
    let toc = code_toc(state, code, juris, None).await?;
    Ok(LawRoot::Toc(summary, toc))
}

pub async fn article_keys_near(
    state: &AppState,
    text_uid: &str,
    key: &str,
    limit: i64,
) -> Result<(Vec<(String, String)>, i64)> {
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn)
        .article_keys_near(text_uid, key, limit)
        .await?)
}

/// `(code, num_key)` targets of a `LEGIARTI…`. Every version of one article
/// shares its id, so the id almost always designates a single target.
pub async fn article_refs_by_source_uid(
    state: &AppState,
    uid: &str,
) -> Result<Vec<(String, String)>> {
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn)
        .article_refs_by_source_uid(uid)
        .await?)
}

/// What the JO printed around a text's articles, `None` for a text it did not
/// publish.
async fn text_publication(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
) -> Result<Option<lj_dtos::TextPublication>> {
    let Some(row) = repo.text_publication(text_uid).await? else {
        return Ok(None);
    };
    let block = |key: &str| {
        row.blocks
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let publication = lj_dtos::TextPublication {
        issue: row.issue.map(|i| LinkedTextRef {
            href: i.eli,
            label: i.title,
            date: None,
        }),
        notice: block("notice"),
        visas: block("visas"),
        signatories: block("signataires"),
        preparatory_work: block("tp"),
        summary: block("sm"),
        repeal: block("abro"),
        corrigendum: block("rect"),
    };
    Ok((publication != lj_dtos::TextPublication::default()).then_some(publication))
}

/// `(slug, jurisdiction)` of a Légifrance text id.
pub async fn text_ref_by_uid(state: &AppState, uid: &str) -> Result<Option<(String, String)>> {
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn).text_ref_by_uid(uid).await?)
}

/// `(slug, jurisdiction)` of the texts holding the section `cid`.
pub async fn section_texts_by_cid(state: &AppState, cid: &str) -> Result<Vec<(String, String)>> {
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn)
        .section_texts_by_cid(cid)
        .await?)
}

/// Sommaire d'un code (`GET /api/norm/{code}`) : métadonnées + nombre d'articles
/// en vigueur. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_summary(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
) -> Result<LawCodeSummary> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    // Through `resolve_text`, to follow a slug rename and to hold the uid: under
    // several jurisdictions the slug alone no longer identifies.
    let text = resolve_text(state, &repo, code, juris).await?;
    let slug = text.slug.clone();
    let row = repo
        .law_code_summary(&text.text_uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut upcoming_versions: Vec<String> = row
        .upcoming_versions
        .iter()
        .map(|d| d.to_string())
        .collect();
    upcoming_versions.sort();
    // The scope label, shown outside the norms, which carry no badge.
    let scope = lj_norm::referential_labels::nature_scope(&row.nature);
    let scope = (scope != "norme").then(|| {
        lj_norm::referential_labels::scope_label(scope)
            .unwrap_or(scope)
            .to_string()
    });
    // Maillage retour vers le hub fond×année du catalogue des normes ; `None`
    // pour un acte individuel.
    let fond_link = repo.norm_fond_of_text(&row.text_uid).await?;
    let (fond, fond_label, fond_year) = match fond_link {
        Some((fond, year)) => {
            let label = lj_norm::referential_labels::norm_fond_label(&fond)
                .unwrap_or(&fond)
                .to_string();
            (Some(fond), Some(label), year)
        }
        None => (None, None, None),
    };
    // Palmarès du texte entier : la colonne droite de la page code ouvre sur ses
    // articles les plus jugés. Lecture de la table de compteurs, constante en taille
    // de code.
    let text_path = lj_vocab::norm_path(&row.jurisdiction, &slug);
    let top_cited = top_cited_articles(&repo, &row.text_uid, &text_path).await?;
    let commentaires = match repo.text_commentaires(&slug).await? {
        None => Vec::new(),
        Some(v) => serde_json::from_value(v)
            .map_err(|e| ApiError::Internal(format!("commentaires invalides: {e}")))?,
    };
    let publication = text_publication(&repo, &row.text_uid).await?;
    Ok(LawCodeSummary {
        legitext: row.text_uid,
        code: slug,
        jurisdiction: row.jurisdiction,
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
        top_cited,
        commentaires,
        publication,
    })
}

/// Longueur cible d'un extrait d'article dans les résultats `/recherche-textes`.
const ARTICLE_SNIPPET_MAX: usize = 280;

/// An ISO date of the norms search's query, `field` naming it in the refusal.
pub fn query_date(field: &str, raw: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        ApiError::Unprocessable(crate::error::validation::date_parsing(
            &["query", field],
            raw,
            "invalid date",
        ))
    })
}

/// Recherche plein-texte d'articles (`GET /api/search-textes`),
/// **titre-primaire** : jambe titre (`search_title` formé, boostée + expansions
/// d'alias) > jambe corps. `code` optionnel borne la recherche à un texte (résolu
/// comme les pages `/texte`) ; absent ⇒ tout le référentiel navigable. Les facettes
/// `jurisdiction`/`nature`/`source` et le `total` sont comptés sous le même
/// prédicat BM25 + mêmes filtres que la page de hits, en une requête `GROUPING
/// SETS` lancée **en parallèle** des hits sur sa propre connexion — le prédicat
/// BM25 est le poste dominant, on ne le paie qu'une fois par jambe. Chaque hit
/// reçoit un extrait surligné via [`crate::snippets::highlight`] (fallback corps
/// tronqué).
///
/// Émet le signal d'usage du moteur `textes` sous le nom de span `search`, même
/// vocabulaire `librejustice.search.*` que les décisions ([`crate::usage`]) :
/// `{name="search"}` couvre donc les deux moteurs, ventilables par `.engine`.
#[instrument(
    name = "search",
    skip(state, caller, budget_key),
    fields(
        db.system = "postgresql",
        limit,
        offset,
        librejustice.search.engine = tracing::field::Empty,
        librejustice.search.source = tracing::field::Empty,
        librejustice.search.query = tracing::field::Empty,
        librejustice.search.identity = tracing::field::Empty,
            librejustice.search.context = tracing::field::Empty,
        librejustice.search.origin = tracing::field::Empty,
        librejustice.search.caller = tracing::field::Empty,
        user_agent.original = tracing::field::Empty,
        librejustice.request.headers = tracing::field::Empty,
        librejustice.request.worker = tracing::field::Empty,
        librejustice.search.visit = tracing::field::Empty,
        librejustice.search.page = tracing::field::Empty,
        librejustice.search.filters = tracing::field::Empty,
        librejustice.search.results_count = tracing::field::Empty,
        librejustice.search.mode = tracing::field::Empty,
        librejustice.search.resolved = tracing::field::Empty,
        librejustice.search.pinned = tracing::field::Empty,
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn search_textes(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    places: &[String],
    jurisdictions: &[String],
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    valid_from: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
    mode: lj_dtos::SearchMode,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<ArticleSearchResponse> {
    const ENGINE: lj_dtos::SearchEngine = lj_dtos::SearchEngine::Textes;
    if places.len() > lj_dtos::MAX_READER_PLACES {
        return Err(ApiError::Unprocessable(
            crate::error::validation::value_error(
                &["query", "place"],
                &places.len().to_string(),
                &format!("at most {} places", lj_dtos::MAX_READER_PLACES),
            ),
        ));
    }
    let filters =
        textes_filters(code, places, jurisdictions, source, scope, at, valid_from).to_string();
    let shape = crate::usage::Shape {
        page: crate::usage::page_number(offset, limit).unwrap_or(1),
        filters: &filters,
    };
    caller.record_span(ENGINE, q, shape);
    tracing::Span::current().record("librejustice.search.mode", tracing::field::debug(mode));
    if offset + limit > TEXTES_CACHED_WINDOW {
        return Err(ApiError::Internal(format!(
            "textes window offset={offset} limit={limit} exceeds {TEXTES_CACHED_WINDOW}"
        )));
    }
    let snippet_chars = crate::snippets::budget(caller.context, ARTICLE_SNIPPET_MAX);
    let uncached = || {
        search_textes_uncached(
            state,
            q,
            code,
            places,
            jurisdictions,
            source,
            scope,
            at,
            valid_from,
            TEXTES_CACHED_WINDOW,
            0,
            mode,
            snippet_chars,
            budget_key,
        )
    };
    let key = textes_cache_key(
        q,
        code,
        places,
        jurisdictions,
        source,
        scope,
        at,
        valid_from,
        mode,
        snippet_chars,
    );
    let mut own_err: Option<ApiError> = None;
    let mut paye = false;
    let hit = state
        .textes_cache
        .optionally_get_with(key, async {
            paye = true;
            match uncached().await {
                Ok(resp) => Some(std::sync::Arc::new(resp)),
                Err(err) => {
                    own_err = Some(err);
                    None
                }
            }
        })
        .await;
    crate::cache_metrics::record_access("textes", !paye);
    let resp = match (hit, own_err) {
        (Some(resp), _) => resp,
        (None, Some(err)) => return Err(err),
        (None, None) => std::sync::Arc::new(uncached().await?),
    };
    let span = tracing::Span::current();
    span.record(
        "librejustice.search.resolved",
        tracing::field::debug(resp.query_mode),
    );
    span.record("librejustice.search.pinned", resp.pinned.len() as u64);
    caller.record_outcome(ENGINE, q, shape, resp.total);
    Ok(ArticleSearchResponse {
        hits: resp
            .hits
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect(),
        total: resp.total,
        facets: resp.facets.clone(),
        query_mode: resp.query_mode,
        pinned: resp.pinned.clone(),
        pinned_more: resp.pinned_more,
        jurisdictions: resp.jurisdictions.clone(),
        reader_place: resp.reader_place.clone(),
    })
}

/// The deepest page the route admits, `offset` 99 + `limit` 100: fetching 200 rows
/// on a miss costs little more than fetching 8.
const TEXTES_CACHED_WINDOW: i64 = 200;

/// Built from the raw arguments, never from [`textes_filters`]: that display
/// serialisation dropping a field would let two queries share an entry.
#[allow(clippy::too_many_arguments)]
fn textes_cache_key(
    q: &str,
    code: Option<&str>,
    places: &[String],
    jurisdictions: &[String],
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    valid_from: Option<chrono::NaiveDate>,
    mode: lj_dtos::SearchMode,
    snippet_chars: usize,
) -> String {
    serde_json::json!([
        q,
        code,
        places,
        jurisdictions,
        source,
        scope,
        at.map(|d| d.to_string()),
        valid_from.map(|d| d.to_string()),
        mode,
        snippet_chars,
    ])
    .to_string()
}

/// Poids d'une réponse pour le plafond mémoire du cache.
pub(crate) fn response_weight(resp: &ArticleSearchResponse) -> usize {
    let hits: usize = resp
        .hits
        .iter()
        .map(|h| {
            std::mem::size_of::<lj_dtos::ArticleSearchHit>()
                + h.code.len()
                + h.code_title.len()
                + h.num.len()
                + h.num_key.len()
                + h.titre_path.as_ref().map_or(0, String::len)
                + h.snippet.len()
                + h.source.len()
                + h.status.as_ref().map_or(0, String::len)
                + h.date_debut.as_ref().map_or(0, String::len)
        })
        .sum();
    let f = &resp.facets;
    let facets: usize = [&f.code, &f.jurisdiction, &f.source, &f.scope, &f.path]
        .into_iter()
        .flatten()
        .map(|c| {
            std::mem::size_of::<lj_dtos::FacetChoice>()
                + c.value.len()
                + c.label.len()
                + c.parent.as_ref().map_or(0, String::len)
        })
        .sum();
    hits + facets
}

/// Séparé de [`search_textes`] pour que single-flight, jeton et permit ne portent
/// que sur le MISS.
#[allow(clippy::too_many_arguments)]
async fn search_textes_uncached(
    state: &AppState,
    q: &str,
    code: Option<&str>,
    places: &[String],
    jurisdictions: &[String],
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    valid_from: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
    mode: lj_dtos::SearchMode,
    snippet_chars: usize,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<ArticleSearchResponse> {
    if state
        .budgets
        .take(crate::ratelimit::Cost::Search, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
        && state.settings.search_budget_enforce
    {
        return Err(ApiError::TooManyRequests);
    }
    // Le sémaphore des décisions, pas un second : deux bornes sur un pool unique
    // demanderaient 62 connexions sur 32.
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    // One connection per member, taken where it is used: the reference mode holds
    // one where the page holds three, on a pool of 32.
    let checkout = || async { state.pool.get().await.map_err(ApiError::from) };
    let conn_hits = checkout().await?;
    let repo_hits = DecisionRepository::new(&conn_hits);
    let places: Vec<String> = places.iter().map(|p| place_uid(p)).collect();
    let read_from: Vec<String> = match places.as_slice() {
        [] => vec![place_uid(DEFAULT_READER_PLACE)],
        named => named.to_vec(),
    };
    let day = at.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let mut home: Vec<String> = Vec::new();
    let mut kept = Vec::with_capacity(read_from.len());
    for p in &read_from {
        for j in reader_home(state, &repo_hits, Some(p.as_str())).await? {
            if !home.contains(&j) {
                home.push(j);
            }
        }
        kept.push(place_texts(state, &repo_hits, p, day).await?);
    }
    let place_filter = PlaceFilter::union(&kept.iter().map(|k| k.as_ref()).collect::<Vec<_>>());
    let home: Vec<&str> = home.iter().map(String::as_str).collect();
    // The paths go to the store as stated; the orders they sit in place the
    // facet and resolve names.
    let stated: Vec<&str> = jurisdictions.iter().map(String::as_str).collect();
    let orders = path_orders(&stated);
    let stated_orders: Vec<&str> = orders.iter().map(String::as_str).collect();
    // A single place named is what the hits are read from; several have no one
    // place to open an article at.
    let place = match places.as_slice() {
        [one] => Some(one.as_str()),
        _ => None,
    };
    let answers_place = places.len() <= 1;

    let text_uid = match code {
        Some(c) if !c.is_empty() => Some(
            resolve_text(state, &repo_hits, c, None)
                .await?
                .text_uid
                .clone(),
        ),
        _ => None,
    };
    // Expansion d'alias (acronymes/noms usuels → forme développée) OR-ée dans la
    // jambe titre.
    let expansions = lj_norm::aliases::expand_query(q);
    // The scope as a set of natures: doctrine and public information are closed
    // lists, a norm is any nature outside both.
    use lj_norm::referential_labels::{DOCTRINE_ADMIN_NATURES, PUBLIC_INFO_NATURES};
    let owned =
        |natures: &[&str]| -> Vec<String> { natures.iter().map(|s| s.to_string()).collect() };
    let doctrine_natures = owned(DOCTRINE_ADMIN_NATURES);
    let public_info_natures = owned(PUBLIC_INFO_NATURES);
    let non_norm_natures = owned(&[DOCTRINE_ADMIN_NATURES, PUBLIC_INFO_NATURES].concat());
    let nature_set: Option<(&[String], bool)> = match scope {
        None => None,
        Some("doctrine_administrative") => Some((&doctrine_natures, true)),
        Some("information_publique") => Some((&public_info_natures, true)),
        Some("norme") => Some((&non_norm_natures, false)),
        Some(other) => {
            return Err(ApiError::Unprocessable(
                crate::error::validation::enum_error(
                    &["query", "scope"],
                    other,
                    &["norme", "doctrine_administrative", "information_publique"],
                ),
            ))
        }
    };
    // No query: the texts of the corpora, the most recently changed first. The
    // places read from rule none out: a listing says what was enacted.
    if q.trim().is_empty() {
        let listed = lj_store::repository::ArticleBrowseFilters {
            home: &home,
            text_uid: text_uid.as_deref(),
            jurisdictions: &stated,
            source,
            nature_set,
            at,
            valid_from,
        };
        let conn_stats = checkout().await?;
        let repo_stats = DecisionRepository::new(&conn_stats);
        let (rows, stats) = tokio::try_join!(
            repo_hits.list_texts(listed, limit, offset),
            repo_stats.text_list_stats(listed),
        )?;
        return Ok(ArticleSearchResponse {
            hits: listed_hits(rows),
            total: stats.total,
            facets: facets_dto(stats, &home, &orders),
            query_mode: lj_dtos::QueryMode::Browse,
            pinned: Vec::new(),
            pinned_more: false,
            jurisdictions: jurisdictions.to_vec(),
            reader_place: match answers_place {
                true => answered_place(state, &repo_hits, place, &[]).await?,
                false => None,
            },
        });
    }
    // A stated content mode is served as stated, without a card.
    let cites = matches!(
        mode,
        lj_dtos::SearchMode::Auto | lj_dtos::SearchMode::Reference
    );
    let jurisdictions = searched_jurisdictions(q, &stated_orders, &home);
    // The citation lands on the index before any name is resolved, and a landing
    // consumes the query by construction: nothing is left to search.
    let cited = match cites {
        true => repo_hits.cited_articles(q, &home, at).await?,
        false => Vec::new(),
    };
    let identified = match cites && cited.is_empty() {
        true => identified_texts(&repo_hits, q, &jurisdictions).await?,
        false => IdentifiedTexts::default(),
    };
    let named = named_text(state, &repo_hits, q).await?;
    let designation = match cites && cited.is_empty() {
        true => {
            let filters = lj_store::repository::ArticleBrowseFilters {
                home: &home,
                text_uid: text_uid.as_deref(),
                jurisdictions: &stated,
                source,
                nature_set,
                at,
                valid_from,
            };
            repo_hits
                .designated_articles(q, named.as_ref(), filters)
                .await?
        }
        false => None,
    };
    let (pinned, pinned_more) = match route_articles(mode, cited, designation, identified) {
        ArticleRoute::Reference(rows) => {
            let read_from = match answers_place {
                true => answered_place(state, &repo_hits, place, &jurisdictions).await?,
                false => None,
            };
            return Ok(reference_response(
                rows,
                q,
                &home,
                jurisdictions,
                read_from,
                limit,
                offset,
                snippet_chars,
            ));
        }
        ArticleRoute::Content {
            pinned,
            pinned_more,
        } => (pinned, pinned_more),
    };
    let (conn_art, conn_txt) = tokio::try_join!(checkout(), checkout())?;
    let repo_art = DecisionRepository::new(&conn_art);
    let repo_txt = DecisionRepository::new(&conn_txt);
    let (rows, member_art, member_txt) = tokio::try_join!(
        repo_hits.search_articles(
            q,
            &expansions,
            &home,
            &place_filter,
            text_uid.as_deref(),
            &stated,
            source,
            nature_set,
            named.as_ref(),
            at,
            valid_from,
            limit,
            offset,
        ),
        repo_art.article_facets_articles(
            q,
            &expansions,
            &home,
            &place_filter,
            text_uid.as_deref(),
            &stated,
            source,
            nature_set,
            at,
            valid_from,
        ),
        repo_txt.article_facets_texts(
            q,
            &expansions,
            &home,
            &place_filter,
            text_uid.as_deref(),
            &stated,
            source,
            nature_set,
            at,
            valid_from,
        ),
    )?;
    let stats = lj_store::repository::merge_article_facets([member_art, member_txt]);
    Ok(ArticleSearchResponse {
        hits: hits_with_snippets(rows, q, snippet_chars),
        total: stats.total,
        facets: facets_dto(stats, &home, &jurisdictions),
        query_mode: lj_dtos::QueryMode::Lexical,
        pinned: pinned.into_iter().map(pinned_article).collect(),
        pinned_more,
        reader_place: match answers_place {
            true => answered_place(state, &repo_hits, place, &jurisdictions).await?,
            false => None,
        },
        jurisdictions,
    })
}

/// Articles a card shows before handing the rest to `mode=reference`.
const PINNED_ARTICLES: usize = 3;

/// What the mode makes of what the query designates.
pub enum ArticleRoute {
    /// The list is the designation, empty when the citation lands nowhere.
    Reference(Vec<lj_store::repository::ArticleSearchRow>),
    /// A content list, the designated texts and articles in a card above it.
    Content {
        pinned: Vec<lj_store::repository::ArticleSearchRow>,
        pinned_more: bool,
    },
}

/// The texts a query names by their identity, whole, and whether the name is
/// all the query says.
#[derive(Default)]
pub struct IdentifiedTexts {
    pub texts: Vec<lj_store::repository::ArticleSearchRow>,
    pub whole: bool,
}

/// The identity `q` names, resolved in `jurisdictions` for a usual name.
pub async fn identified_texts(
    repo: &DecisionRepository<'_>,
    q: &str,
    jurisdictions: &[String],
) -> lj_store::error::Result<IdentifiedTexts> {
    let Some(named) = lj_norm::text_identity::text_identity(q) else {
        return Ok(IdentifiedTexts::default());
    };
    Ok(IdentifiedTexts {
        texts: repo
            .texts_by_identity(&named.identity, jurisdictions)
            .await?,
        whole: named.whole,
    })
}

/// The one place the mode is decided. `cited` is what the title alone answered,
/// `designation` what the named text or the bare keys answered when it did not,
/// `identified` the texts the query names; a stated content mode ignores all
/// three, `reference` serves whatever landed. A text named and nothing else is
/// the reference; named among other words, it waits in the card.
pub fn route_articles(
    mode: lj_dtos::SearchMode,
    cited: Vec<lj_store::repository::ArticleSearchRow>,
    designation: Option<lj_store::repository::ArticleDesignation>,
    identified: IdentifiedTexts,
) -> ArticleRoute {
    let cites = matches!(
        mode,
        lj_dtos::SearchMode::Auto | lj_dtos::SearchMode::Reference
    );
    if cites && !cited.is_empty() {
        return ArticleRoute::Reference(cited);
    }
    let consumes = designation.as_ref().is_some_and(|d| d.consumes);
    let mut articles = designation.map(|d| d.articles).unwrap_or_default();
    if cites && consumes {
        return ArticleRoute::Reference(articles);
    }
    if cites && identified.whole && !identified.texts.is_empty() {
        return ArticleRoute::Reference(identified.texts);
    }
    if mode == lj_dtos::SearchMode::Reference {
        return ArticleRoute::Reference(articles);
    }
    let pinned_more = articles.len() > PINNED_ARTICLES;
    articles.truncate(PINNED_ARTICLES);
    ArticleRoute::Content {
        pinned: identified.texts.into_iter().chain(articles).collect(),
        pinned_more,
    }
}

/// The place results are read from, when there is one. The reader's own if they
/// set it; else the country a single-country answer designates, and only if the
/// place vocabulary knows it, a link to a place nobody holds being a dead end.
/// `FR` is never one: it has children, so no reader stands there.
async fn answered_place(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    place: Option<&str>,
    jurisdictions: &[String],
) -> Result<Option<PlaceRef>> {
    let uid = match place {
        Some(p) => p.to_string(),
        None => {
            let [only] = jurisdictions else {
                return Ok(None);
            };
            if lj_norm::jurisdictions::DOMESTIC_REGIONS.contains(&only.as_str()) {
                return Ok(None);
            }
            let uid = format!("geo:{only}");
            let known = match state.place_path_cache.get(&uid).await {
                Some(hit) => !hit.is_empty(),
                None => {
                    let path = repo.place_path(&uid).await?;
                    let known = !path.is_empty();
                    state
                        .place_path_cache
                        .insert(uid.clone(), Arc::new(path))
                        .await;
                    known
                }
            };
            if !known {
                return Ok(None);
            }
            uid
        }
    };
    let labels = repo.place_labels(std::slice::from_ref(&uid)).await?;
    Ok(Some(place_ref(&uid, &labels)))
}

/// The corpora a search covers: those stated, else what the query resolves to.
fn searched_jurisdictions(q: &str, stated: &[&str], home: &[&str]) -> Vec<String> {
    match stated {
        [] => lj_norm::jurisdictions::query_scope(q, home).jurisdictions,
        stated => stated.iter().map(|s| s.to_string()).collect(),
    }
}

/// The orders the stated paths sit in, once each: `FR.LOI` and `FR` read `FR`.
fn path_orders(stated: &[&str]) -> Vec<String> {
    let mut orders: Vec<String> = Vec::new();
    for path in stated {
        let order = path.split_once('.').map_or(*path, |(order, _)| order);
        if !orders.iter().any(|o| o == order) {
            orders.push(order.to_string());
        }
    }
    orders
}

/// The texts of the searched corpora, the most recently changed first: the norm
/// search with no query, one hit per text, on its root. Read from the default place, as the
/// connector reads, and with no place ruling a text out: a listing says what was
/// enacted, not where it applies.
#[allow(clippy::too_many_arguments)]
pub async fn list_textes(
    state: &AppState,
    code: Option<&str>,
    jurisdictions: &[String],
    valid_from: Option<chrono::NaiveDate>,
    at: Option<chrono::NaiveDate>,
    limit: i64,
    offset: i64,
    caller: crate::usage::Caller,
    budget_key: Option<&crate::ratelimit::CallerKey>,
) -> Result<ArticleSearchResponse> {
    const ENGINE: lj_dtos::SearchEngine = lj_dtos::SearchEngine::Textes;
    let filters = textes_filters(code, &[], jurisdictions, None, None, at, valid_from).to_string();
    let shape = crate::usage::Shape {
        page: crate::usage::page_number(offset, limit).unwrap_or(1),
        filters: &filters,
    };
    caller.record_span(ENGINE, "", shape);
    if state
        .budgets
        .take(crate::ratelimit::Cost::Search, budget_key)
        .await
        == crate::ratelimit::Verdict::Exhausted
        && state.settings.search_budget_enforce
    {
        return Err(ApiError::TooManyRequests);
    }
    let _db_permit = state
        .search_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(format!("search semaphore: {e}")))?;
    let (conn_rows, conn_stats) = tokio::try_join!(state.pool.get(), state.pool.get())?;
    let (repo_rows, repo_stats) = (
        DecisionRepository::new(&conn_rows),
        DecisionRepository::new(&conn_stats),
    );
    let text_uid = match code {
        Some(c) => Some(
            resolve_text(state, &repo_rows, c, None)
                .await?
                .text_uid
                .clone(),
        ),
        None => None,
    };
    let home = lj_norm::jurisdictions::DOMESTIC_REGIONS;
    let stated: Vec<&str> = jurisdictions.iter().map(String::as_str).collect();
    let listed = lj_store::repository::ArticleBrowseFilters {
        home: &home,
        text_uid: text_uid.as_deref(),
        jurisdictions: &stated,
        source: None,
        nature_set: None,
        at,
        valid_from,
    };
    let (rows, stats) = tokio::try_join!(
        repo_rows.list_texts(listed, limit, offset),
        repo_stats.text_list_stats(listed),
    )?;
    caller.record_outcome(ENGINE, "", shape, stats.total);
    Ok(ArticleSearchResponse {
        hits: listed_hits(rows),
        total: stats.total,
        facets: facets_dto(stats, &home, &path_orders(&stated)),
        query_mode: lj_dtos::QueryMode::Browse,
        pinned: Vec::new(),
        pinned_more: false,
        jurisdictions: jurisdictions.to_vec(),
        reader_place: None,
    })
}

/// The hits of a listing, one per text, on its root.
fn listed_hits(rows: Vec<lj_store::repository::TextListRow>) -> Vec<ArticleSearchHit> {
    rows.into_iter()
        .map(|r| ArticleSearchHit {
            code: r.slug,
            jurisdiction: r.jurisdiction,
            code_title: r.title,
            num: String::new(),
            num_key: String::new(),
            titre_path: None,
            snippet: String::new(),
            source: r.source,
            status: None,
            date_debut: None,
            valid_from: Some(r.valid_from.to_string()),
            valid_to: None,
        })
        .collect()
}

/// The reference list counts itself: total and the code facet come off the
/// rows, and the page is a window on them.
#[allow(clippy::too_many_arguments)]
fn reference_response(
    rows: Vec<lj_store::repository::ArticleSearchRow>,
    q: &str,
    home: &[&str],
    jurisdictions: Vec<String>,
    reader_place: Option<PlaceRef>,
    limit: i64,
    offset: i64,
    snippet_chars: usize,
) -> ArticleSearchResponse {
    let total = rows.len() as i64;
    let mut by_code: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for r in &rows {
        *by_code
            .entry(r.slug.clone().unwrap_or_default())
            .or_default() += 1;
    }
    let mut code: Vec<FacetCount> = by_code
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect();
    code.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    let page: Vec<_> = rows
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit.max(0) as usize)
        .collect();
    ArticleSearchResponse {
        total,
        hits: hits_with_snippets(page, q, snippet_chars),
        facets: facets_dto(
            ArticleSearchStats {
                total,
                code,
                jurisdiction: Vec::new(),
                nature: Vec::new(),
                source: Vec::new(),
                path: Vec::new(),
            },
            home,
            &jurisdictions,
        ),
        query_mode: lj_dtos::QueryMode::Reference,
        pinned: Vec::new(),
        pinned_more: false,
        jurisdictions,
        reader_place,
    }
}

fn pinned_article(r: lj_store::repository::ArticleSearchRow) -> lj_dtos::PinnedArticle {
    lj_dtos::PinnedArticle {
        code: r.slug.unwrap_or_default(),
        jurisdiction: r.jurisdiction,
        code_title: r.code_title,
        num: r.num,
        num_key: r.num_key,
        titre_path: r.title_path,
        status: (!r.status.is_empty()).then_some(r.status),
        date_debut: r.date_debut.map(|d| d.to_string()),
    }
}

/// Filters of a text search, under the URL keys of the `/norm-search` page
/// (`origine`, not `source`): without the query or the window. Serialisation
/// shared by the history in base and by the span attribute
/// `librejustice.search.filters`; the depth goes in `page`, which the history
/// adds here and the span carries apart.
#[allow(clippy::too_many_arguments)]
pub(crate) fn textes_filters(
    code: Option<&str>,
    places: &[String],
    jurisdictions: &[String],
    source: Option<&str>,
    scope: Option<&str>,
    at: Option<chrono::NaiveDate>,
    valid_from: Option<chrono::NaiveDate>,
) -> serde_json::Value {
    let date = at.map(|d| d.to_string());
    let from = valid_from.map(|d| d.to_string());
    let mut filters = serde_json::Map::new();
    for (key, values) in [("place", places), ("jurisdiction", jurisdictions)] {
        if !values.is_empty() {
            filters.insert(key.to_string(), serde_json::json!(values));
        }
    }
    for (key, value) in [
        ("code", code),
        ("origine", source),
        ("scope", scope),
        ("date", date.as_deref()),
        ("valid_from", from.as_deref()),
    ] {
        if let Some(v) = value {
            filters.insert(key.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    serde_json::Value::Object(filters)
}

/// Every position of the jurisdiction filter: the orders the reader's places
/// search, states and territories first, then the Union, then treaties, and
/// after them those the query names elsewhere. A zero is a position outside
/// the searched corpora, `jurisdictions` says which.
fn jurisdiction_positions(
    rows: Vec<FacetCount>,
    home: &[&str],
    searched: &[String],
) -> Vec<FacetCount> {
    let mut order: Vec<&str> = home.to_vec();
    order.sort_by_key(|j| match *j {
        "EU" => 1,
        "INTL" => 2,
        _ => 0,
    });
    for j in searched {
        if !order.contains(&j.as_str()) {
            order.push(j);
        }
    }
    order
        .into_iter()
        .map(|j| FacetCount {
            value: j.to_string(),
            count: rows.iter().find(|r| r.value == j).map_or(0, |r| r.count),
        })
        .collect()
}

/// Maps the repository facets onto the DTO, each row a choice in the same
/// order, labelled by `lj_norm::referential_labels`, the raw value failing one.
fn facets_dto(
    stats: ArticleSearchStats,
    home: &[&str],
    searched: &[String],
) -> ArticleSearchFacets {
    use lj_norm::referential_labels::{jurisdiction_label, source_label};
    let map = |rows: Vec<FacetCount>, label: fn(&str) -> Option<&'static str>| {
        rows.into_iter()
            .map(|row| FacetChoice {
                label: label(&row.value)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.value.clone()),
                value: row.value,
                count: row.count,
                parent: None,
                hint: None,
            })
            .collect()
    };
    // Sur-facette portée : agrégat des buckets nature par mapping code.
    let mut scope_counts: std::collections::BTreeMap<&'static str, i64> =
        std::collections::BTreeMap::new();
    for row in &stats.nature {
        *scope_counts
            .entry(lj_norm::referential_labels::nature_scope(&row.value))
            .or_default() += row.count;
    }
    let mut scope: Vec<FacetChoice> = scope_counts
        .into_iter()
        .map(|(value, count)| FacetChoice {
            label: lj_norm::referential_labels::scope_label(value)
                .unwrap_or(value)
                .to_string(),
            value: value.to_string(),
            count,
            parent: None,
            hint: None,
        })
        .collect();
    scope.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    ArticleSearchFacets {
        // Slugs auto-descriptifs : le label est la valeur.
        code: map(stats.code, |_| None),
        jurisdiction: map(
            jurisdiction_positions(stats.jurisdiction, home, searched),
            jurisdiction_label,
        ),
        source: map(stats.source, source_label),
        scope,
        path: stats
            .path
            .into_iter()
            .map(|row| FacetChoice {
                label: lj_norm::jurisdiction_path::path_label(&row.value)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.value.clone()),
                parent: row
                    .value
                    .rsplit_once('.')
                    .map(|(parent, _)| parent.to_string()),
                value: row.value,
                count: row.count,
                hint: None,
            })
            .collect(),
    }
}

/// The text `q` names, resolved once for every leg that wants it. Both lookups
/// scan `legal_text` whole, so both are cached: see `AppState`.
async fn named_text(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    q: &str,
) -> Result<Option<lj_store::repository::NamedText>> {
    // An instrument cited by number resolves off slug patterns alone, so the
    // catalogue gates itself and never the cut.
    let catalog = match lj_norm::aliases::query_names_code(q) {
        true => state
            .named_code_catalog_cache
            .try_get_with(String::new(), async {
                repo.named_code_catalog()
                    .await
                    .map(Arc::new)
                    .map_err(|e| ApiError::Internal(format!("catalogue des codes nommables: {e}")))
            })
            .await
            .map_err(|e: Arc<ApiError>| ApiError::Internal(e.to_string()))?,
        false => Arc::new(Vec::new()),
    };
    let Some((target, stripped)) = lj_norm::aliases::strip_query_named_text(q, &catalog) else {
        return Ok(None);
    };
    let uid = state
        .named_text_cache
        .try_get_with(format!("{target:?}"), async {
            repo.resolve_named_text(target)
                .await
                .map(|u| u.map(Arc::from))
                .map_err(|e| ApiError::Internal(format!("résolution du texte nommé: {e}")))
        })
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(e.to_string()))?;
    Ok(uid.map(|text_uid| lj_store::repository::NamedText {
        text_uid: text_uid.to_string(),
        stripped,
    }))
}

/// Catalogue of the browsable codes (`GET /api/codes`): one referential text per
/// entry, with its article count. The text's `slug` is the DTO's `code` (link
/// `/norm/{code}`).
/// One cache entry per `head_only`, TTL 12 h: counting the articles in force,
/// correlated per text, reads all of `legal_article` and is never run per request.
pub async fn code_catalogue(
    state: &AppState,
    head_only: bool,
) -> Result<Arc<CodeCatalogueResponse>> {
    state
        .code_catalogue_cache
        .try_get_with(head_only.to_string(), load_code_catalogue(state, head_only))
        .await
        .map_err(|e: Arc<ApiError>| ApiError::Internal(format!("catalogue codes: {e}")))
}

#[instrument(skip(state), fields(db.system = "postgresql"))]
async fn load_code_catalogue(
    state: &AppState,
    head_only: bool,
) -> Result<Arc<CodeCatalogueResponse>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let entries = repo
        .list_legal_texts(head_only)
        .await?
        .into_iter()
        .map(catalogue_entry)
        .collect();
    Ok(Arc::new(CodeCatalogueResponse { entries }))
}

/// The jurisdiction's name, « France » for `FR`, the code itself when the
/// referential does not name it.
fn jurisdiction_name(code: &str) -> String {
    lj_norm::referential_labels::jurisdiction_label(code)
        .map_or_else(|| code.to_string(), str::to_string)
}

fn catalogue_entry(row: LegalTextCatalogRow) -> CodeCatalogueEntry {
    CodeCatalogueEntry {
        jurisdiction_label: jurisdiction_name(&row.jurisdiction),
        code: row.slug,
        title: row.title,
        nature: row.nature,
        jurisdiction: row.jurisdiction,
        article_count: row.article_count,
    }
}

// Au-delà de ce nombre d'articles, le sommaire renvoie la table des matières
// (chips) ; en deçà, la vue-lecture intégrale — un texte court (BOFiP, décret,
// arrêté, loi ordinaire) se lit sur sa page. Partagé avec le front (éligibilité
// d'une division à l'accordéon).
use lj_dtos::{INLINE_READING_MAX, INLINE_READING_MAX_BYTES};

/// Table des matières d'un code (`GET /api/norm/{code}/sommaire`) : articles
/// ordonnés, chacun avec son fil d'Ariane et son état. Textes courts
/// (≤ [`INLINE_READING_MAX`] articles) : `reading` porte la vue-lecture
/// intégrale (corps joints) à la place des entrées. 404 si le slug est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code))]
pub async fn code_toc(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    date: Option<NaiveDate>,
) -> Result<CodeTocResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let text_uid = &text.text_uid;
    // Arbre structurel réel daté à la date demandée (Chronolégi) sinon en vigueur
    // aujourd'hui ; repli sur le sommaire à plat (title_path) pour les textes sans
    // structure ingérée.
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let toc = toc_addresses(state, &repo, text_uid, at).await?;
    if !toc.tree.is_empty() {
        let n_articles = toc
            .tree
            .iter()
            .filter(|r| r.child_kind == "article")
            .count();
        let reading = if n_articles <= INLINE_READING_MAX {
            match reading_items(&repo, text_uid, &toc, &toc.paths.nodes, 0, date).await? {
                (items, None) => items,
                (_, Some(_)) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        return Ok(CodeTocResponse {
            entries: Vec::new(),
            tree: toc.paths.nodes.iter().map(|n| toc.toc_node(n)).collect(),
            reading,
        });
    }
    let rows = repo.code_table_of_contents(text_uid).await?;
    if rows.len() <= INLINE_READING_MAX {
        let rows = repo.flat_text_reading(text_uid).await?;
        let versions: Vec<Option<(String, NaiveDate)>> = rows
            .iter()
            .map(|r| Some((r.child_num_key.clone()?, r.date_debut)))
            .collect();
        let mut reading = lecture_bornee(rows);
        link_reading(&repo, text_uid, &mut reading, &versions, date).await?;
        if !reading.is_empty() {
            return Ok(CodeTocResponse {
                entries: Vec::new(),
                tree: Vec::new(),
                reading,
            });
        }
    }
    Ok(CodeTocResponse {
        entries: rows.into_iter().map(toc_entry).collect(),
        tree: Vec::new(),
        reading: Vec::new(),
    })
}

/// A text's tree at one date, and the address of each of its nodes.
pub struct TocAddresses {
    pub tree: Vec<TocTreeRow>,
    pub paths: TocPaths,
}

impl TocAddresses {
    pub fn weight(&self) -> usize {
        let rows: usize = self
            .tree
            .iter()
            .map(|r| r.label.len() + r.child_uid.len() + 96)
            .sum();
        let nodes: usize = self
            .paths
            .nodes
            .iter()
            .map(|n| n.path.as_ref().map_or(0, String::len) + 8 * n.rows.len() + 64)
            .sum();
        rows + nodes
    }

    fn row(&self, n: &TocNodePath) -> &TocTreeRow {
        &self.tree[n.rows[0]]
    }

    /// A merged section is in force when one of its members is.
    fn etat(&self, n: &TocNodePath) -> String {
        n.rows
            .iter()
            .map(|&r| &self.tree[r].etat)
            .find(|e| *e == "VIGUEUR")
            .unwrap_or(&self.row(n).etat)
            .clone()
    }

    fn toc_node(&self, n: &TocNodePath) -> TocNode {
        let row = self.row(n);
        TocNode {
            kind: row.child_kind.clone(),
            depth: n.depth,
            label: row.label.clone(),
            path: n.path.clone(),
            num_key: row.child_num_key.clone(),
            etat: self.etat(n),
        }
    }

    fn section_ref(&self, i: usize) -> Option<LawSectionRef> {
        let n = &self.paths.nodes[i];
        Some(LawSectionRef {
            path: n.path.clone()?,
            label: self.row(n).label.clone(),
        })
    }

    /// The address under `text` of the node holding the section `cid`: its
    /// path, the article a section without one holds, the text for an empty one.
    pub fn cid_href(&self, text: &str, cid: &str) -> Option<String> {
        let at = self.paths.nodes.iter().position(|n| {
            n.kind == TocKind::Section
                && n.rows
                    .iter()
                    .any(|&r| self.tree[r].child_cid.as_deref() == Some(cid))
        })?;
        let sub = self.paths.subtree(at);
        let tail = sub[0].path.clone().or_else(|| {
            sub.iter()
                .find(|n| n.kind == TocKind::Article)
                .and_then(|n| self.row(n).child_num_key.clone())
        });
        Some(match tail {
            Some(tail) => format!("{text}/{tail}"),
            None => text.to_string(),
        })
    }
}

/// The addressed tree of a text at `at`, from [`AppState::toc_address_cache`].
pub async fn toc_addresses(
    state: &AppState,
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    at: NaiveDate,
) -> Result<Arc<TocAddresses>> {
    let key = format!("{text_uid}@{at}");
    if let Some(hit) = state.toc_address_cache.get(&key).await {
        return Ok(hit);
    }
    let tree = repo.toc_tree(text_uid, at).await?;
    let keys = if tree.is_empty() {
        Vec::new()
    } else {
        repo.text_article_keys(text_uid).await?
    };
    let paths = {
        let rows: Vec<_> = tree.iter().map(TocTreeRow::toc_row).collect();
        toc_paths(&rows, &keys.iter().map(String::as_str).collect())
    };
    let toc = Arc::new(TocAddresses { tree, paths });
    state.toc_address_cache.insert(key, toc.clone()).await;
    Ok(toc)
}

/// A cross-reference of an article body as served: to the article cited, else
/// to a text that reads, dated when the reading is.
fn citation_span(
    s: lj_store::repository::ArticleCitationSpanRow,
    date: Option<NaiveDate>,
    placed: impl Fn(String) -> String,
) -> Option<CitationSpan> {
    let path = lj_vocab::norm_path(&s.ref_jurisdiction, &s.ref_slug?);
    let href = match s.ref_num_key.as_deref().filter(|k| !k.is_empty()) {
        Some(k) => match date {
            Some(d) => format!("{path}/{k}/{d}"),
            None => format!("{path}/{k}"),
        },
        None if s.ref_readable => match date {
            Some(d) => format!("{path}?date={d}"),
            None => path,
        },
        None => return None,
    };
    Some(CitationSpan {
        start: s.char_start.max(0) as usize,
        end: s.char_end.max(0) as usize,
        targets: vec![CitationTarget {
            href: Some(placed(href)),
            label: s.ref_title,
        }],
    })
}

/// Links the cross-references of the bodies a reading still carries, each
/// item's version named by `versions`, aligned on `items`.
async fn link_reading(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    items: &mut [LawSectionItem],
    versions: &[Option<(String, NaiveDate)>],
    date: Option<NaiveDate>,
) -> Result<()> {
    let owners: Vec<(String, NaiveDate)> = items
        .iter()
        .zip(versions)
        .filter(|(item, _)| item.texte.is_some())
        .filter_map(|(_, v)| v.clone())
        .collect();
    if owners.is_empty() {
        return Ok(());
    }
    let mut spans = repo.articles_citation_spans(text_uid, &owners).await?;
    for (item, version) in items.iter_mut().zip(versions) {
        if let (Some(_), Some(version)) = (&item.texte, version) {
            item.texte_spans = spans
                .remove(version)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|s| citation_span(s, date, |h| h))
                .collect();
        }
    }
    Ok(())
}

/// The reading of `nodes` in order, depths counted from `base`: headings and
/// article bodies with their cross-references, the bodies left out past the
/// reading bounds, whose count comes back.
async fn reading_items(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    toc: &TocAddresses,
    nodes: &[TocNodePath],
    base: i32,
    date: Option<NaiveDate>,
) -> Result<(Vec<LawSectionItem>, Option<usize>)> {
    let articles: Vec<&str> = nodes
        .iter()
        .filter(|n| n.kind == TocKind::Article)
        .map(|n| toc.row(n).child_uid.as_str())
        .collect();
    let mut bodies = if articles.len() <= INLINE_READING_MAX {
        repo.article_bodies(text_uid, &articles).await?
    } else {
        HashMap::new()
    };
    let mut versions = Vec::with_capacity(nodes.len());
    let mut items: Vec<LawSectionItem> = nodes
        .iter()
        .map(|n| {
            let row = toc.row(n);
            let body = bodies.remove(&row.child_uid);
            versions.push(body.as_ref().map(|b| (b.num_key.clone(), b.date_debut)));
            let body = body.unwrap_or_default();
            LawSectionItem {
                kind: row.child_kind.clone(),
                depth: n.depth - base,
                label: row.label.clone(),
                path: n.path.clone(),
                num_key: row.child_num_key.clone(),
                etat: toc.etat(n),
                texte: body.texte,
                texte_spans: Vec::new(),
                nota: body.nota,
            }
        })
        .collect();
    let omitted = borner_lecture(&mut items);
    link_reading(repo, text_uid, &mut items, &versions, date).await?;
    Ok((items, omitted))
}

/// Vue-lecture d'une section (`GET /api/norm/{code}/section/{path}`) : le
/// sous-arbre de la section à la date demandée, corps des articles joints.
/// 404 si le slug ou le chemin est inconnu.
#[instrument(skip(state), fields(db.system = "postgresql", code = %code, path = %path))]
pub async fn law_section(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    path: &str,
    date: Option<NaiveDate>,
) -> Result<LawSectionResponse> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);

    let text = resolve_text(state, &repo, code, juris).await?;
    let slug = text.slug.clone();
    let summary = repo
        .law_code_summary(&text.text_uid)
        .await?
        .ok_or(ApiError::NotFound)?;
    let at = date.unwrap_or_else(|| chrono::Utc::now().date_naive());
    let toc = toc_addresses(state, &repo, &summary.text_uid, at).await?;
    let index = toc.paths.find(path).ok_or(ApiError::NotFound)?;
    let subtree = toc.paths.subtree(index);
    let title = toc.row(&subtree[0]).label.clone();
    let (items, omitted) = reading_items(
        &repo,
        &summary.text_uid,
        &toc,
        &subtree[1..],
        subtree[0].depth,
        date,
    )
    .await?;
    let (ancestors, prev, next) = section_nav(&toc, index);
    let text_path = lj_vocab::norm_path(&summary.jurisdiction, &slug);
    let top_cited = section_top_cited(&repo, &summary.text_uid, &text_path, &items).await?;
    let citing_texts = section_citing_texts(&repo, &summary.text_uid, &items).await?;
    Ok(LawSectionResponse {
        code: slug,
        jurisdiction_label: jurisdiction_name(&summary.jurisdiction),
        jurisdiction: summary.jurisdiction,
        code_title: Some(summary.title),
        path: path.to_string(),
        title,
        ancestors,
        prev,
        next,
        items,
        omitted,
        top_cited,
        citing_texts,
    })
}

/// Where the section `cid` of a text lives today, `None` when the text holds
/// no such section.
pub async fn section_address(
    state: &AppState,
    code: &str,
    juris: Option<&str>,
    cid: &str,
) -> Result<Option<String>> {
    let conn = state.pool.get().await?;
    let repo = DecisionRepository::new(&conn);
    let text = resolve_text(state, &repo, code, juris).await?;
    let today = chrono::Utc::now().date_naive();
    let toc = toc_addresses(state, &repo, &text.text_uid, today).await?;
    Ok(toc.cid_href(&lj_vocab::norm_path(&text.jurisdiction, &text.slug), cid))
}

/// La même règle au cas racine, où le sommaire est le repli : au-delà d'une
/// borne, la vue-lecture est vide et l'arbre reprend la main.
fn lecture_bornee(rows: Vec<TocReadingRow>) -> Vec<LawSectionItem> {
    let mut items: Vec<LawSectionItem> = rows.into_iter().map(section_item).collect();
    if borner_lecture(&mut items).is_some() {
        return Vec::new();
    }
    items
}

/// La règle de lecture en place, à ses deux bornes. Rend le nombre d'articles
/// réduits à leur index, les items gardant numéro et lien.
fn borner_lecture(items: &mut [LawSectionItem]) -> Option<usize> {
    let articles = items.iter().filter(|i| i.kind == "article").count();
    let octets: usize = items
        .iter()
        .map(|i| i.texte.as_deref().map_or(0, str::len) + i.nota.as_deref().map_or(0, str::len))
        .sum();
    if articles <= INLINE_READING_MAX && octets <= INLINE_READING_MAX_BYTES {
        return None;
    }
    for item in items.iter_mut() {
        item.texte = None;
        item.nota = None;
    }
    Some(articles)
}

/// Les [`SECTION_TOP_CITED`] articles les plus cités du sous-arbre **déjà lu** :
/// une lecture de `citing_decision_counts` sur ses `num_key`, sans jointure ni
/// relecture de l'arbre. `label` = le numéro porté par l'arête, celui que la
/// vue-lecture affiche.
async fn section_top_cited(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    text_path: &str,
    items: &[LawSectionItem],
) -> Result<Vec<TopCitedArticle>> {
    let labels: std::collections::HashMap<&str, &str> = items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| Some((i.num_key.as_deref()?, i.label.as_str())))
        .collect();
    let num_keys: Vec<String> = labels.keys().map(|k| (*k).to_string()).collect();
    Ok(repo
        .citing_counts_for_articles(text_uid, &num_keys)
        .await?
        .into_iter()
        .take(SECTION_TOP_CITED)
        .map(|(num_key, count)| TopCitedArticle {
            label: labels[num_key.as_str()].to_string(),
            href: Some(format!("{text_path}/{num_key}")),
            num_key,
            count,
        })
        .collect())
}

/// Le même palmarès au périmètre du **texte entier** : le classement vient du
/// repo, qui lit la table de compteurs sans énumérer les articles du code, et
/// rend déjà le numéro affichable.
async fn top_cited_articles(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    text_path: &str,
) -> Result<Vec<TopCitedArticle>> {
    Ok(repo
        .top_cited_articles(
            text_uid,
            SECTION_TOP_CITED as i64,
            TOP_CITED_CANDIDATES as i64,
        )
        .await?
        .into_iter()
        .map(|(num_key, label, count)| TopCitedArticle {
            label,
            href: Some(format!("{text_path}/{num_key}")),
            num_key,
            count,
        })
        .collect())
}

/// Textes citant la division : agrégés au grain du texte citant, filtrés à
/// [`SECTION_CITING_TEXT_MIN_ARTICLES`] articles visés et plafonnés comme les
/// décisions du même bloc. La liste sort de la vue-lecture déjà lue — ses
/// `num_key` sont exactement le sous-arbre daté.
async fn section_citing_texts(
    repo: &DecisionRepository<'_>,
    text_uid: &str,
    items: &[LawSectionItem],
) -> Result<Vec<SectionCitingText>> {
    let num_keys: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| i.num_key.clone())
        .collect();
    let public_info: Vec<String> = lj_norm::referential_labels::PUBLIC_INFO_NATURES
        .iter()
        .map(|s| s.to_string())
        .collect();
    Ok(repo
        .section_citing_texts(
            text_uid,
            &num_keys,
            &public_info,
            SECTION_CITING_TEXT_MIN_ARTICLES,
            SECTION_CITING_TEXTS as i64,
        )
        .await?
        .into_iter()
        .map(|r| SectionCitingText {
            href: r
                .text_slug
                .map(|s| lj_vocab::norm_path(&r.text_jurisdiction, &s)),
            label: r.label,
            article_count: r.article_count,
        })
        .collect())
}

/// Places a section in the merged tree in reading order: enclosing divisions
/// (root to parent), previous and next block, the reading order neighbours outside
/// the subtree (the next of a Livre is the following Livre, not its first Titre).
/// A division without an address is not listed.
fn section_nav(
    toc: &TocAddresses,
    at: usize,
) -> (
    Vec<LawSectionRef>,
    Option<LawSectionRef>,
    Option<LawSectionRef>,
) {
    let nodes = &toc.paths.nodes;
    let depth = nodes[at].depth;
    // Pile des divisions ouvertes jusqu'à la section : ses ancêtres.
    let mut stack: Vec<usize> = Vec::new();
    for (i, n) in nodes[..at].iter().enumerate() {
        if n.kind == TocKind::Section {
            stack.truncate((n.depth - 1) as usize);
            stack.push(i);
        }
    }
    stack.truncate((depth - 1) as usize);
    let ancestors = stack.iter().filter_map(|&i| toc.section_ref(i)).collect();
    let end = at + toc.paths.subtree(at).len();
    let next = (end..nodes.len())
        .filter(|&i| nodes[i].kind == TocKind::Section && nodes[i].depth <= depth)
        .find_map(|i| toc.section_ref(i));
    // Inverse du suivant : le dernier bloc en ordre de lecture avant la
    // section, quelle que soit sa profondeur (hors ancêtres).
    let prev = (0..at)
        .rev()
        .filter(|&i| nodes[i].kind == TocKind::Section && !stack.contains(&i))
        .find_map(|i| toc.section_ref(i));
    (ancestors, prev, next)
}

fn section_item(row: TocReadingRow) -> LawSectionItem {
    LawSectionItem {
        kind: row.child_kind,
        depth: row.depth,
        label: row.label,
        path: None,
        num_key: row.child_num_key,
        etat: row.etat,
        texte: row.texte,
        texte_spans: Vec::new(),
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
        expired: row.expired,
    }
}

/// Construit les hits DTO en surlignant le corps (`snippets::highlight`, mêmes
/// tokenizers que l'index). Les docs sont indexés par position (id synthétique) ;
/// un doc qui ne matche pas (absent du retour `highlight`) retombe sur un corps
/// tronqué. `slug` est non-NULL (la requête filtre `t.slug IS NOT NULL`).
fn hits_with_snippets(
    rows: Vec<ArticleSearchRow>,
    q: &str,
    snippet_chars: usize,
) -> Vec<ArticleSearchHit> {
    let docs: Vec<(i64, String)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i as i64, r.texte.clone().unwrap_or_default()))
        .collect();
    let snippets = crate::snippets::highlight(&docs, q, snippet_chars);
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let snippet = snippets
                .get(&(i as i64))
                .cloned()
                .unwrap_or_else(|| truncate_plain(r.texte.as_deref().unwrap_or(""), snippet_chars));
            ArticleSearchHit {
                code: r.slug.unwrap_or_default(),
                jurisdiction: r.jurisdiction,
                code_title: r.code_title,
                num: r.num,
                num_key: r.num_key,
                titre_path: r.title_path,
                snippet,
                source: r.source,
                status: (!r.status.is_empty()).then_some(r.status),
                valid_from: r
                    .date_debut
                    .and_then(stated_bound)
                    .or_else(|| r.text_date.map(|d| d.to_string())),
                valid_to: r.date_fin.and_then(stated_bound),
                date_debut: r.date_debut.map(|d| d.to_string()),
            }
        })
        .collect()
}

/// A version bound the source states, not one of the sentinels standing for an
/// unknown start, an open end or a date to be determined.
fn stated_bound(d: NaiveDate) -> Option<String> {
    let day = d.to_string();
    (d.year() > 1 && d.year() < 2998 && day != lj_dtos::DATE_A_DETERMINER).then_some(day)
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

    #[test]
    fn a_text_placed_elsewhere_by_name_is_a_doubt_only_where_its_key_reaches_on_mention() {
        let text = |uid: &str, title: &str, keyed: bool| PlacedElsewhere {
            text_uid: uid.to_string(),
            jurisdiction: "FR".to_string(),
            title: title.to_string(),
            keyed,
            articles: true,
            body: false,
        };
        let polynesia = ["geo:PF".to_string(), "geo:FR".to_string()];
        let metropole = ["geo:FR-M".to_string(), "geo:FR".to_string()];
        let day = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let statute = text(
            "JORFTEXT000000000001",
            "LOI n° 2020-1 relative aux ports",
            false,
        );
        let urbanism = text("LEGITEXT000006074075", "Code de l'urbanisme", false);

        assert!(is_doubt(&statute, &polynesia, day));
        assert!(!is_doubt(&urbanism, &polynesia, day));
        assert!(!is_doubt(&statute, &metropole, day));
        assert!(!is_doubt(
            &PlacedElsewhere {
                keyed: true,
                ..statute
            },
            &polynesia,
            day
        ));
    }

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

    fn nav_toc() -> TocAddresses {
        let tree = nav_tree();
        let paths = {
            let rows: Vec<_> = tree.iter().map(TocTreeRow::toc_row).collect();
            let keys = rows.iter().filter_map(|r| r.num_key).collect();
            toc_paths(&rows, &keys)
        };
        TocAddresses { tree, paths }
    }

    #[test]
    fn section_nav_ancetres_et_voisins() {
        let toc = nav_toc();
        let nav = |path: &str| section_nav(&toc, toc.paths.find(path).expect(path));
        let path = |r: Option<LawSectionRef>| r.map(|r| r.path);

        let (anc, prev, next) = nav("l1/t1/c2");
        assert_eq!(
            anc.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["l1", "l1/t1"]
        );
        assert_eq!(path(prev).as_deref(), Some("l1/t1/c1"));
        assert_eq!(path(next).as_deref(), Some("l1/t2"));

        let (anc, prev, next) = nav("l1/t2");
        assert_eq!(anc.len(), 1);
        assert_eq!(path(prev).as_deref(), Some("l1/t1/c2"));
        assert_eq!(path(next).as_deref(), Some("l2"));

        let (anc, prev, next) = nav("l1");
        assert!(anc.is_empty() && prev.is_none());
        assert_eq!(path(next).as_deref(), Some("l2"));
        let (_, _, next) = nav("l2");
        assert!(next.is_none());
    }

    #[test]
    fn un_cid_mene_au_chemin_de_son_noeud() {
        let toc = nav_toc();
        let text = "/norm/fr/code";
        assert_eq!(
            toc.cid_href(text, "c2").as_deref(),
            Some("/norm/fr/code/l1/t1/c2")
        );
        assert_eq!(toc.cid_href(text, "zzz"), None);
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
            jurisdiction_code: Some("ce".to_string()),
            date_lecture: Some("2024-02-13".to_string()),
            docket_numbers: Some(vec!["123456".to_string()]),
            publication_codes: Some(vec!["A".to_string()]),
            summary: None,
            cited_num_keys: Vec::new(),
        };
        let refs = crate::referential::Referential::new(Vec::new(), Vec::new(), Vec::new());
        let hit = citing_dto(row, &refs);
        assert_eq!(hit.id, "CETATEXT000012345678");
        assert_eq!(hit.jurisdiction_type, JurisdictionType::Ce);
        assert_eq!(hit.title, "Conseil d'État, 13 février 2024, 123456");
        assert_eq!(hit.date_lecture.as_deref(), Some("2024-02-13"));
        assert_eq!(hit.publication_codes, vec!["A".to_string()]);
    }

    fn version(status: &str, debut: &str, fin: Option<&str>) -> LawVersionRow {
        LawVersionRow {
            source_uid: format!("LEGIARTI{status}"),
            status: status.to_string(),
            date_debut: Some(debut.to_string()),
            date_fin: fin.map(str::to_string),
        }
    }

    fn motif(versions: &[LawVersionRow], date: &str) -> String {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        match no_version_error(versions, Some(d)) {
            ApiError::NotFound => "not_found".to_string(),
            ApiError::NoVersionAtDate(m) => m,
            other => panic!("motif inattendu : {other}"),
        }
    }

    /// Quelle date déclenche le repli sur la dernière version : après la fin de
    /// la dernière fenêtre seulement. Une version encore ouverte, ou une date
    /// antérieure à tout, laissent `derniere_fin` sans borne à dépasser.
    #[test]
    fn le_repli_ne_s_ouvre_qu_apres_la_derniere_fenetre() {
        let jour = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();

        let abroge = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("ABROGE", "2019-05-24", Some("2021-05-01")),
        ];
        assert_eq!(derniere_fin(&abroge), Some(jour("2021-05-01")));

        let en_vigueur = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("VIGUEUR", "2019-05-24", None),
        ];
        assert_eq!(derniere_fin(&en_vigueur), None);

        assert_eq!(derniere_fin(&[]), None);
    }

    /// Three outcomes: only an article with no `VIGUEUR` version reads as repealed.
    #[test]
    fn le_motif_n_affirme_l_abrogation_que_sans_version_en_vigueur() {
        assert_eq!(motif(&[], "2026-08-17"), "not_found");

        let abroge = [
            version("MODIFIE", "2000-09-21", Some("2019-05-23")),
            version("ABROGE", "2019-05-24", Some("2025-10-01")),
        ];
        assert!(motif(&abroge, "2026-08-17").starts_with("law article repealed"));
        assert!(motif(&abroge, "2026-08-17").contains("2025-10-01"));

        let trou = [
            version("VIGUEUR", "2006-03-24", Some("2007-12-31")),
            version("MODIFIE_MORT_NE", "2008-01-01", Some("2006-03-24")),
        ];
        let m = motif(&trou, "2026-08-01");
        assert!(m.contains("no version covers that date"), "{m}");
        assert!(!m.contains("repealed"), "{m}");
        assert!(m.contains("2006-03-24") && m.contains("2007-12-31"), "{m}");
    }

    /// A gap between two live versions is named by the status that closed it.
    #[test]
    fn le_trou_entre_deux_versions_vivantes_dit_ce_qui_l_a_ouvert() {
        let abroge_puis_recree = [
            version("ABROGE", "2003-08-02", Some("2016-06-17")),
            version("MODIFIE", "2024-01-01", Some("2025-05-02")),
            version("VIGUEUR", "2025-05-03", None),
        ];
        let m = motif(&abroge_puis_recree, "2016-07-07");
        assert!(m.contains("repealed on 2016-06-17"), "{m}");
        assert!(m.contains("held again from 2024-01-01"), "{m}");

        let transfere = [
            version("MODIFIE", "2008-12-10", Some("2009-01-31")),
            version("TRANSFERE", "2009-02-01", Some("2009-05-14")),
            version("VIGUEUR", "2016-06-17", None),
        ];
        let m = motif(&transfere, "2012-12-05");
        assert!(m.contains("moved to another number on 2009-05-14"), "{m}");

        let trou_sans_statut = [
            version("MODIFIE", "1996-09-11", Some("2005-10-01")),
            version("VIGUEUR", "2019-01-01", None),
        ];
        let m = motif(&trou_sans_statut, "2010-06-01");
        assert!(m.contains("no version is held between"), "{m}");
        assert!(m.contains("2005-10-01") && m.contains("2019-01-01"), "{m}");

        // Outside the bounds there is no gap: the coverage answers whole.
        let m = motif(&trou_sans_statut, "1990-01-01");
        assert!(m.contains("span 1996-09-11 to now"), "{m}");
    }

    /// A version still open runs the coverage up to now.
    #[test]
    fn la_borne_haute_dit_maintenant_quand_une_version_court_toujours() {
        let ouverte = [
            version("MODIFIE", "2017-01-01", Some("2023-12-27")),
            version("VIGUEUR", "2023-12-28", None),
        ];
        let m = motif(&ouverte, "2010-01-01");
        assert!(m.contains("span 2017-01-01 to now"), "{m}");
        assert!(!m.contains("2023-12-27"), "{m}");

        let bornee = [
            version("VIGUEUR", "2006-03-24", Some("2007-12-31")),
            version("MODIFIE_MORT_NE", "2008-01-01", Some("2006-03-24")),
        ];
        assert!(
            motif(&bornee, "2026-08-01").contains("span 2006-03-24 to 2007-12-31"),
            "une couverture entièrement bornée garde sa fin"
        );
    }

    /// `VIGUEUR_DIFF` est une entrée en vigueur différée, pas une abrogation :
    /// le préfixe suffit à la ranger du bon côté.
    #[test]
    fn la_vigueur_differee_ne_vaut_pas_abrogation() {
        let differe = [
            version("MODIFIE", "1950-04-30", Some("2026-01-01")),
            version("VIGUEUR_DIFF", "2027-01-01", Some("2027-08-31")),
        ];
        assert!(!motif(&differe, "2026-08-17").contains("repealed"));
    }

    /// Two identical queries share an entry, and nothing that changes the
    /// response collides, a filter moved from one axis to another included.
    #[test]
    fn textes_cache_key_separe_ce_qui_change_la_reponse() {
        let key = |code, jur: &[&str]| {
            let jur: Vec<String> = jur.iter().map(|j| j.to_string()).collect();
            textes_cache_key(
                "nosocomiale",
                code,
                &[],
                &jur,
                None,
                None,
                None,
                None,
                lj_dtos::SearchMode::Auto,
                ARTICLE_SNIPPET_MAX,
            )
        };
        assert_eq!(key(Some("csp"), &[]), key(Some("csp"), &[]));
        assert_ne!(key(Some("csp"), &[]), key(None, &["csp"]));
        assert_ne!(key(Some("csp"), &[]), key(None, &[]));
        assert_ne!(key(None, &["BE"]), key(None, &["BE", "FR"]));
        // Two readers placed apart search two corpora: one entry each.
        let placed = |places: &[&str]| {
            let places: Vec<String> = places.iter().map(|p| p.to_string()).collect();
            textes_cache_key(
                "nosocomiale",
                None,
                &places,
                &[],
                None,
                None,
                None,
                None,
                lj_dtos::SearchMode::Auto,
                ARTICLE_SNIPPET_MAX,
            )
        };
        assert_ne!(placed(&["CH"]), placed(&[]));
        assert_ne!(placed(&["CH"]), placed(&["SN"]));
        assert_ne!(placed(&["CH"]), placed(&["CH", "SN"]));
        // A lower bound serves other versions.
        let bounded = |from: Option<NaiveDate>| {
            textes_cache_key(
                "nosocomiale",
                None,
                &[],
                &[],
                None,
                None,
                None,
                from,
                lj_dtos::SearchMode::Auto,
                ARTICLE_SNIPPET_MAX,
            )
        };
        assert_ne!(
            bounded(NaiveDate::from_ymd_opt(2025, 10, 11)),
            bounded(None)
        );
        // A reference entry must not be served to a lexical query.
        let mode = |m| {
            textes_cache_key(
                "1240 code civil",
                None,
                &[],
                &[],
                None,
                None,
                None,
                None,
                m,
                ARTICLE_SNIPPET_MAX,
            )
        };
        assert_ne!(
            mode(lj_dtos::SearchMode::Auto),
            mode(lj_dtos::SearchMode::Lexical)
        );
        assert_ne!(
            mode(lj_dtos::SearchMode::Reference),
            mode(lj_dtos::SearchMode::Lexical)
        );
        // The rail block's shorter snippets must not be served to the norm search.
        let cut = |chars| {
            textes_cache_key(
                "nosocomiale",
                None,
                &[],
                &[],
                None,
                None,
                None,
                None,
                lj_dtos::SearchMode::Lexical,
                chars,
            )
        };
        assert_ne!(
            cut(ARTICLE_SNIPPET_MAX),
            cut(crate::snippets::TEASER_MAX_CHARS)
        );
    }

    fn item(kind: &str, texte: Option<&str>) -> LawSectionItem {
        LawSectionItem {
            kind: kind.into(),
            depth: 1,
            label: "Article 1".into(),
            path: None,
            num_key: Some("1".into()),
            etat: "VIGUEUR".into(),
            texte: texte.map(str::to_string),
            texte_spans: Vec::new(),
            nota: None,
        }
    }

    /// Les deux bornes de la lecture en place mordent chacune de leur cote,
    /// et laissent l'index quand elles mordent.
    #[test]
    fn la_lecture_en_place_a_deux_bornes() {
        let mut court = vec![item("article", Some("corps")); 3];
        assert_eq!(borner_lecture(&mut court), None);
        assert!(court.iter().all(|i| i.texte.is_some()));

        let mut trop_d_articles = vec![item("article", Some("x")); INLINE_READING_MAX + 1];
        assert_eq!(
            borner_lecture(&mut trop_d_articles),
            Some(INLINE_READING_MAX + 1)
        );
        assert!(trop_d_articles.iter().all(|i| i.texte.is_none()));
        assert!(trop_d_articles.iter().all(|i| i.num_key.is_some()));

        // Few articles, many bytes: the byte bound cuts on its own.
        let gros = "a".repeat(INLINE_READING_MAX_BYTES / 2 + 1);
        let mut trop_lourd = vec![item("article", Some(&gros)); 2];
        assert_eq!(borner_lecture(&mut trop_lourd), Some(2));
        assert!(trop_lourd.iter().all(|i| i.texte.is_none()));
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    fn reader(place: &str, path: &[&str], date: &str) -> Reader {
        Reader {
            place: place.to_string(),
            path: path.iter().map(|p| p.to_string()).collect(),
            date: Some(date.parse().unwrap()),
        }
    }

    fn state(id: i64, places: &[&str], starts_on: &str, ends_on: Option<&str>) -> AssertionRow {
        AssertionRow {
            id,
            places: places.iter().map(|p| p.to_string()).collect(),
            starts_on: starts_on.parse().unwrap(),
            ends_on: ends_on.map(|d| d.parse().unwrap()),
            start_origin: "received".to_string(),
            end_origin: ends_on.map(|_| "received".to_string()),
            start_source: None,
            end_source: None,
            version_start: starts_on.parse().unwrap(),
        }
    }

    #[test]
    fn the_default_place_leaves_the_href_alone_and_another_rides_before_the_fragment() {
        let rd = reader("geo:FR-M", &["geo:FR-M", "geo:FR"], "2026-01-01");
        assert_eq!(
            rd.href("/norm/code-civil/1240".into()),
            "/norm/code-civil/1240"
        );
        let nc = reader("geo:NC", &["geo:NC", "geo:FR"], "2026-01-01");
        assert_eq!(
            nc.href("/norm/code-civil/1240".into()),
            "/norm/code-civil/1240?place=NC"
        );
        assert_eq!(
            nc.href("/norm/code-civil?date=2020-01-01".into()),
            "/norm/code-civil?date=2020-01-01&place=NC"
        );
        assert_eq!(
            nc.href("/norm/code-civil#LEGISCTA1".into()),
            "/norm/code-civil?place=NC#LEGISCTA1"
        );
    }

    #[test]
    fn a_state_covering_the_reader_is_served_and_never_listed_elsewhere() {
        let rows = [
            state(1, &["geo:FR-M", "geo:GP"], "2016-10-01", None),
            state(2, &["geo:NC"], "2024-10-03", None),
        ];
        let rd = reader("geo:FR-M", &["geo:FR-M", "geo:FR"], "2026-01-01");
        let st = reader_states(&rows, &rd);
        assert_eq!(st.served.map(|a| a.id), Some(1));
        assert!(st.competing.is_empty());
        assert!(st.elsewhere.is_empty());

        let dz = reader("geo:DZ", &["geo:DZ"], "2026-01-01");
        let st = reader_states(&rows, &dz);
        assert!(st.served.is_none());
        assert_eq!(
            st.elsewhere.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![1, 2]
        );

        let before = reader("geo:NC", &["geo:NC", "geo:FR"], "2020-01-01");
        let st = reader_states(&rows, &before);
        assert!(st.served.is_none());
        assert_eq!(
            st.elsewhere.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn two_states_covering_the_reader_serve_the_latest_start_and_show_the_other() {
        let rows = [
            state(1, &["geo:SN"], "2000-01-01", Some("2013-01-01")),
            state(2, &["geo:SN"], "2010-01-01", None),
        ];
        let rd = reader("geo:SN", &["geo:SN"], "2011-06-01");
        let st = reader_states(&rows, &rd);
        assert_eq!(st.served.map(|a| a.id), Some(2));
        assert_eq!(
            st.competing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![1]
        );
        assert!(st.elsewhere.is_empty());
    }
}
