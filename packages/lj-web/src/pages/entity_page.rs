//! Page `/entite/{ns}/{id}` (fiche entité, ADR 0189). Identité registre
//! (`entity`/`entity_denomination`, ADR 0179) + agrégats contentieux dérivés du
//! reverse-lookup `decision_party`. Calquée sur [`crate::pages::decision_page`]
//! et [`crate::pages::law_code_page`] : en-tête + agrégats bloquants SSR (SEO
//! dans le document initial), liste de décisions citantes streamée
//! (`PartiallyBlocked`), paginée par `?page=`.
//!
//! Le rendu s'adapte au namespace (`siren` entreprise, `rna` association,
//! `cnb`/`oacc` avocat) : libellé de catégorie et forme lisible de l'uid registre.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::{
    EntityCounselDto, EntityDecisionHitDto, EntityDecisionsResponse, EntityHeaderDto,
    EntityKeyCountDto, EntityPageResponse, EntityRegistreResponse, EntityStatsDto,
    EntityYearCountDto, RegistreEntrepriseDto,
};
use serde::{Deserialize, Serialize};

use crate::api::ApiClient;
use crate::components::search::ResultCard;
use crate::helpers::{format_iso_date, group_thousands, split_entity_uid};
use crate::pages::decision_page::data::{sendable, PageError};
use crate::seo::CANONICAL_BASE;

/// Décisions citantes par page (offset-based, ADR 0189).
const PAGE_SIZE: i64 = 20;

fn client() -> ApiClient {
    ApiClient::from_context()
}

// ── Chargement ───────────────────────────────────────────────────────────────

/// Liste paginée résolue : réponse ou erreur repliée (jamais de rejet, parité
/// `CitingResult` de la page /texte).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDecisionsResult {
    pub response: Option<EntityDecisionsResponse>,
    pub error: Option<String>,
}

/// Fiche (identité + agrégats), bloquante SSR pour le SEO. Uid inconnu ⇒ 404.
pub async fn fetch_entity(ns: String, id: String) -> Result<EntityPageResponse, PageError> {
    if ns.trim().is_empty() || id.trim().is_empty() {
        return Err(PageError {
            status: 400,
            message: "Entité invalide".to_string(),
        });
    }
    let data = client().fetch_entity(&ns, &id).await?;
    Ok(data)
}

/// Volet registre (non bloquant, streamé — ADR 0199). `None` = namespace sans
/// volet ou amont indisponible : la section ne se rend pas, jamais d'erreur.
pub async fn fetch_entity_registre(ns: String, id: String) -> Option<EntityRegistreResponse> {
    if !matches!(ns.as_str(), "siren" | "rna") {
        return None;
    }
    client().fetch_entity_registre(&ns, &id).await.ok()
}

/// Décisions citantes (non bloquant, streamé). Erreur repliée en `error`.
pub async fn fetch_entity_decisions(ns: String, id: String, page: i64) -> EntityDecisionsResult {
    match client()
        .fetch_entity_decisions(&ns, &id, page, PAGE_SIZE)
        .await
    {
        Ok(response) => EntityDecisionsResult {
            response: Some(response),
            error: None,
        },
        Err(err) => EntityDecisionsResult {
            response: None,
            error: Some(err.message),
        },
    }
}

/// Segments `ns`/`id` de la route `/entite/:ns/:id`.
fn entity_params() -> (Signal<String>, Signal<String>) {
    let params = use_params_map();
    let ns = Signal::derive(move || params.read().get("ns").unwrap_or_default());
    let id = Signal::derive(move || params.read().get("id").unwrap_or_default());
    (ns, id)
}

/// Page courante depuis `?page=` (1 par défaut ; valeurs `< 1` ramenées à 1).
fn page_param() -> Signal<i64> {
    let query = use_query_map();
    Signal::derive(move || {
        query
            .read()
            .get("page")
            .and_then(|p| p.parse::<i64>().ok())
            .filter(|&p| p >= 1)
            .unwrap_or(1)
    })
}

#[component]
pub fn EntityPage() -> impl IntoView {
    let (ns, id) = entity_params();
    let page = page_param();

    // Fiche bloquante (SEO) ; décisions streamées et rechargées à chaque `?page=`.
    let entity = Resource::new_blocking(
        move || (ns.get(), id.get()),
        |(ns, id)| sendable(fetch_entity(ns, id)),
    );
    let decisions = Resource::new(
        move || (ns.get(), id.get(), page.get()),
        |(ns, id, page)| sendable(fetch_entity_decisions(ns, id, page)),
    );
    let registre = Resource::new(
        move || (ns.get(), id.get()),
        |(ns, id)| sendable(fetch_entity_registre(ns, id)),
    );

    view! {
        <Suspense fallback=EntitySkeleton>
            {move || Suspend::new(async move {
                match entity.await {
                    Ok(data) => {
                        set_cache_control(200);
                        Either::Left(
                            view! {
                                <EntityLoaded
                                    data=data
                                    decisions=decisions
                                    registre=registre
                                    page=page
                                />
                            },
                        )
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <EntityError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// `Cache-Control` de la réponse document (aligné sur decision/law : 200 → 7 j au
/// CDN, 404/400 → 5 min, 5xx → no-store). No-op côté hydrate.
#[cfg(feature = "ssr")]
fn set_cache_control(status: u16) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue, StatusCode};
    let value = match status {
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 | 422 => "public, max-age=0, s-maxage=300",
        _ => "no-store",
    };
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if status != 200 {
            if let Ok(code) = StatusCode::from_u16(status) {
                resp.set_status(code);
            }
        }
        if let Ok(hv) = HeaderValue::from_str(value) {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn set_cache_control(_status: u16) {}

#[component]
fn EntityError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    let title = "Entité introuvable - LibreJustice";
    view! {
        <Title text=title />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 py-16 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {eyebrow}
            </p>
            <h1 class="mt-2 font-sans text-3xl text-[var(--color-ink)]">{err.message}</h1>
        </div>
    }
}

#[component]
fn EntitySkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto w-full max-w-4xl flex-1 px-4 py-10 sm:px-6 lg:px-8">
            <div class="flex flex-col gap-4">
                <Skeleton class="h-4 w-24" />
                <Skeleton class="h-10 w-2/3" />
                <Skeleton class="h-4 w-1/3" />
            </div>
            <div class="mt-10 grid grid-cols-3 gap-4">
                {(0..3)
                    .map(|_| view! { <Skeleton class="h-20 w-full rounded-lg" /> })
                    .collect::<Vec<_>>()}
            </div>
            <div class="mt-10 flex flex-col gap-3">
                {(0..5)
                    .map(|_| view! { <Skeleton class="h-16 w-full rounded-md" /> })
                    .collect::<Vec<_>>()}
            </div>
        </div>
    }
}

// ── Page chargée ──────────────────────────────────────────────────────────────

#[component]
fn EntityLoaded(
    data: EntityPageResponse,
    decisions: Resource<EntityDecisionsResult>,
    registre: Resource<Option<EntityRegistreResponse>>,
    page: Signal<i64>,
) -> impl IntoView {
    let EntityPageResponse { header, stats } = data;
    let (ns, local) = split_entity_uid(&header.uid);
    let (ns, local) = (ns.to_string(), local.to_string());
    let base_path = format!("/entite/{ns}/{local}");

    let category = namespace_label(&ns);
    let title = header.denomination.clone();
    let page_title = format!("{title} - LibreJustice");
    let url = format!("{CANONICAL_BASE}{base_path}");
    let description = meta_description(&header, &stats, category);
    let jsonld = build_json_ld(&header, &url, category);

    let denominations = denominations_section(&header);
    let contentieux = contentieux_section(&stats, matches!(ns.as_str(), "cnb" | "oacc"));

    view! {
        <Title text=page_title />
        <Meta name="description" content=description.clone() />
        <Meta property="og:type" content="profile" />
        <Meta property="og:site_name" content="LibreJustice" />
        <Meta property="og:title" content=title.clone() />
        <Meta property="og:description" content=description.clone() />
        <Meta property="og:url" content=url.clone() />
        <Meta property="og:locale" content="fr_FR" />
        <Meta property="og:image" content=crate::seo::OG_IMAGE />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto w-full max-w-4xl flex-1 px-4 py-10 sm:px-6 lg:px-8">
            <EntityHeader header=header category=category />
            {denominations}
            {contentieux}
            <RegistreSection registre=registre />
            <DecisionsSection decisions=decisions base_path=base_path page=page />
        </div>
    }
}

/// Libellé de catégorie d'un namespace registre.
fn namespace_label(ns: &str) -> &'static str {
    match ns {
        "siren" => "Entreprise",
        "rna" => "Association",
        "cnb" => "Avocat",
        "oacc" => "Avocat aux Conseils",
        _ => "Entité",
    }
}

/// Forme lisible de l'uid registre (`SIREN 552 043 002`, `RNA W123456789`…).
/// `None` si le namespace n'a pas de forme d'affichage dédiée.
fn registry_label(ns: &str, local: &str) -> Option<String> {
    match ns {
        "siren" => Some(format!("SIREN {}", format_siren(local))),
        "rna" => Some(format!("RNA {}", local.to_uppercase())),
        "cnb" => Some(format!("CNB {local}")),
        "oacc" => Some(format!("Avocats aux Conseils · {local}")),
        _ => None,
    }
}

/// Groupe un SIREN en 3-3-3 (`552 043 002`). Non-conforme (≠ 9 chiffres) ⇒ brut.
fn format_siren(s: &str) -> String {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.len() == 9 {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else {
        s.to_string()
    }
}

fn meta_description(header: &EntityHeaderDto, stats: &EntityStatsDto, category: &str) -> String {
    let count = stats.decision_count;
    let decisions = match count {
        0 => "aucune décision référencée".to_string(),
        1 => "1 décision référencée".to_string(),
        n => format!("{} décisions référencées", group_thousands(n)),
    };
    format!(
        "{}, {}. {} sur LibreJustice.",
        header.denomination,
        category.to_lowercase(),
        decisions,
    )
}

/// JSON-LD schema.org : `Person` pour une personne physique, `Organization`
/// sinon. Un seul `@type` (parité `seo::decision`/`seo::law`).
fn build_json_ld(header: &EntityHeaderDto, url: &str, category: &str) -> String {
    use serde_json::{json, Map, Value};
    let type_ = if header.nature == "physique" {
        "Person"
    } else {
        "Organization"
    };
    let mut node = Map::new();
    node.insert("@context".into(), json!("https://schema.org"));
    node.insert("@type".into(), json!(type_));
    node.insert("name".into(), json!(header.denomination));
    node.insert("url".into(), json!(url));
    node.insert("identifier".into(), json!(header.uid));
    if let Some(sigle) = &header.sigle {
        node.insert("alternateName".into(), json!(sigle));
    }
    node.insert("description".into(), json!(category));
    serde_json::to_string(&Value::Object(node)).unwrap_or_else(|_| "{}".to_string())
}

// ── En-tête ─────────────────────────────────────────────────────────────────

#[component]
fn EntityHeader(header: EntityHeaderDto, category: &'static str) -> impl IntoView {
    let (ns, local) = split_entity_uid(&header.uid);
    let registry = registry_label(ns, local);

    // Badges : forme juridique, uid registre lisible, état actif/inactif.
    let forme_badge = header.forme.clone().map(neutral_badge);
    let registry_badge = registry.map(|r| {
        view! {
            <span class="inline-flex items-center rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-0.5 font-mono text-xs text-[var(--color-ink-muted)]">
                {r}
            </span>
        }
        .into_any()
    });
    let state_badge = if header.active {
        view! {
            <span class="inline-flex items-center rounded-full border border-[var(--color-accent)] px-2.5 py-0.5 text-xs text-[var(--color-accent)]">
                "En activité"
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="inline-flex items-center rounded-full border border-[var(--color-rule)] px-2.5 py-0.5 text-xs text-[var(--color-ink-subtle)]">
                "Cessée"
            </span>
        }
        .into_any()
    };

    // Sigle en sous-titre discret quand distinct de la dénomination.
    let sigle = header
        .sigle
        .clone()
        .filter(|s| !s.eq_ignore_ascii_case(&header.denomination))
        .map(|s| {
            view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">{format!("Sigle : {s}")}</p>
            }
        });

    view! {
        <header class="flex flex-col gap-3">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                {category}
            </p>
            <h1 class="font-sans text-3xl text-[var(--color-ink)] sm:text-4xl">
                {header.denomination.clone()}
            </h1>
            {sigle}
            <div class="mt-1 flex flex-wrap items-center gap-2">
                {forme_badge}
                {registry_badge}
                {state_badge}
            </div>
        </header>
    }
}

fn neutral_badge(text: String) -> AnyView {
    view! {
        <span class="inline-flex items-center rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-0.5 text-xs text-[var(--color-ink-muted)]">
            {text}
        </span>
    }
    .into_any()
}

// ── Chronologie des dénominations ─────────────────────────────────────────────

/// Timeline des dénominations (plus récente en tête) — rendue seulement à partir
/// de deux dénominations datées (une seule = déjà l'en-tête).
fn denominations_section(header: &EntityHeaderDto) -> Option<AnyView> {
    if header.denominations.len() < 2 {
        return None;
    }
    let rows = header
        .denominations
        .iter()
        .rev()
        .map(|d| {
            let period = denomination_period(d.date_debut.as_deref(), d.date_fin.as_deref());
            let period_view = period.map(|p| {
                view! { <span class="text-xs text-[var(--color-ink-subtle)]">{p}</span> }
            });
            view! {
                <li class="flex flex-col gap-0.5 border-l border-[var(--color-rule)] py-1 pl-4">
                    <span class="text-sm text-[var(--color-ink)]">{d.denomination.clone()}</span>
                    {period_view}
                </li>
            }
        })
        .collect_view();
    Some(
        view! {
            <section aria-label="Chronologie des dénominations" class="mt-10">
                <h2 class="font-sans text-base text-[var(--color-ink)]">
                    "Chronologie des dénominations"
                </h2>
                <ol class="mt-4 flex flex-col gap-2">{rows}</ol>
            </section>
        }
        .into_any(),
    )
}

fn denomination_period(debut: Option<&str>, fin: Option<&str>) -> Option<String> {
    match (debut, fin) {
        (Some(d), Some(f)) => Some(format!(
            "{} – {}",
            format_iso_date(Some(d)),
            format_iso_date(Some(f))
        )),
        (Some(d), None) => Some(format!("depuis {}", format_iso_date(Some(d)))),
        (None, Some(f)) => Some(format!("jusqu'au {}", format_iso_date(Some(f)))),
        (None, None) => None,
    }
}

// ── Contentieux (agrégats) ────────────────────────────────────────────────────

fn contentieux_section(stats: &EntityStatsDto, is_avocat: bool) -> AnyView {
    // Pour un avocat, `side` est le côté qu'il représente, pas le sien.
    let (applicant_label, defendant_label) = if is_avocat {
        ("En demande", "En défense")
    } else {
        ("Comme demandeur", "Comme défendeur")
    };
    let tiles = view! {
        <div class="grid grid-cols-3 gap-4">
            {stat_tile("Décisions", stats.decision_count)}
            {stat_tile(applicant_label, stats.as_applicant)}
            {stat_tile(defendant_label, stats.as_defendant)}
        </div>
    };

    // Corps masqué si l'entité n'apparaît dans aucune décision.
    if stats.decision_count == 0 {
        return view! {
            <section aria-label="Contentieux" class="mt-10">
                <h2 class="font-sans text-base text-[var(--color-ink)]">"Contentieux"</h2>
                {tiles}
                <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">
                    "Aucune décision ne référence cette entité pour l'instant."
                </p>
            </section>
        }
        .into_any();
    }

    let by_jurisdiction = jurisdiction_block(&stats.by_jurisdiction);
    let by_year = year_block(&stats.by_year);
    let counsel = counsel_block(&stats.top_counsel);

    view! {
        <section aria-label="Contentieux" class="mt-10">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Contentieux"</h2>
            {tiles}
            <div class="mt-6 grid grid-cols-1 gap-8 md:grid-cols-2">
                {by_jurisdiction}
                {by_year}
            </div>
            {counsel}
        </section>
    }
    .into_any()
}

fn stat_tile(label: &'static str, value: i64) -> AnyView {
    view! {
        <div class="rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-4">
            <p class="font-sans text-2xl text-[var(--color-ink)] tabular-nums">
                {group_thousands(value)}
            </p>
            <p class="mt-1 text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                {label}
            </p>
        </div>
    }
    .into_any()
}

/// Répartition par juridiction (barres CSS horizontales, décroissant).
/// Libellés abrégés (« CA Agen », « TJ Lille ») — le complet reste en tooltip.
fn jurisdiction_block(items: &[EntityKeyCountDto]) -> Option<AnyView> {
    if items.is_empty() {
        return None;
    }
    let max = items.iter().map(|i| i.count).max().unwrap_or(1).max(1);
    let rows = items
        .iter()
        .map(|i| bar_row(&abbreviate_jurisdiction(&i.label), &i.label, i.count, max))
        .collect_view();
    Some(sub_block("Par juridiction", rows))
}

/// Abréviation conventionnelle d'un libellé de juridiction : famille en sigle
/// (« Cour d'appel d'Agen » → « CA Agen »), hautes juridictions en forme
/// courte usuelle. Libellé inconnu ⇒ inchangé.
fn abbreviate_jurisdiction(label: &str) -> String {
    const EXACT: &[(&str, &str)] = &[
        ("Cour de cassation", "Cass."),
        ("Conseil d'État", "CE"),
        ("Conseil constitutionnel", "Cons. const."),
        ("Cour de justice de l'Union européenne", "CJUE"),
        ("Cour européenne des droits de l'homme", "CEDH"),
        ("Cour nationale du droit d'asile", "CNDA"),
        ("Tribunal des conflits", "T. confl."),
        (
            "Commission nationale de l'informatique et des libertés",
            "CNIL",
        ),
    ];
    if let Some((_, court)) = EXACT.iter().find(|(full, _)| *full == label) {
        return (*court).to_string();
    }
    const FAMILLES: &[(&str, &str)] = &[
        ("Cour administrative d'appel ", "CAA"),
        ("Cour d'appel ", "CA"),
        ("Tribunal judiciaire ", "TJ"),
        ("Tribunal administratif ", "TA"),
        ("Tribunal de commerce ", "T. com."),
        ("Tribunal des activités économiques ", "TAE"),
    ];
    for (famille, sigle) in FAMILLES {
        if let Some(rest) = label.strip_prefix(famille) {
            // Le connecteur saute, sauf article contracté qui se déplie
            // (« du Havre » → « Le Havre », « des Sables… » → « Les Sables… »).
            let ville = if let Some(v) = rest.strip_prefix("de ") {
                v.to_string()
            } else if let Some(v) = rest.strip_prefix("d'") {
                v.to_string()
            } else if let Some(v) = rest.strip_prefix("du ") {
                format!("Le {v}")
            } else if let Some(v) = rest.strip_prefix("des ") {
                format!("Les {v}")
            } else {
                rest.to_string()
            };
            return format!("{sigle} {ville}");
        }
    }
    label.to_string()
}

/// Répartition par année (barres CSS, ordre chronologique).
fn year_block(items: &[EntityYearCountDto]) -> Option<AnyView> {
    if items.is_empty() {
        return None;
    }
    let max = items.iter().map(|i| i.count).max().unwrap_or(1).max(1);
    let rows = items
        .iter()
        .map(|i| {
            let year = i.year.to_string();
            bar_row(&year, &year, i.count, max)
        })
        .collect_view();
    Some(sub_block("Par année", rows))
}

fn sub_block(title: &'static str, rows: impl IntoView + 'static) -> AnyView {
    view! {
        <div class="flex min-w-0 flex-col gap-2">
            <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {title}
            </h3>
            <ul class="flex flex-col gap-1.5">{rows}</ul>
        </div>
    }
    .into_any()
}

/// Une barre : libellé (complet en tooltip), jauge proportionnelle, compteur.
fn bar_row(label: &str, full: &str, count: i64, max: i64) -> AnyView {
    let pct = ((count as f64 / max as f64) * 100.0).round().max(3.0);
    view! {
        <li class="flex items-center gap-3">
            <span
                class="w-36 shrink-0 truncate text-sm text-[var(--color-ink-muted)]"
                title=full.to_string()
            >
                {label.to_string()}
            </span>
            <span class="relative h-2 min-w-0 flex-1 overflow-hidden rounded-full bg-[var(--color-rule)]/40">
                <span
                    class="absolute inset-y-0 left-0 rounded-full bg-[var(--color-accent)]"
                    style=format!("width:{pct}%")
                />
            </span>
            <span class="w-10 shrink-0 text-right text-xs tabular-nums text-[var(--color-ink-subtle)]">
                {group_thousands(count)}
            </span>
        </li>
    }
    .into_any()
}

/// Conseils observés aux côtés de l'entité (décroissant). Chaque conseil résolu
/// au registre lie vers sa propre fiche.
fn counsel_block(items: &[EntityCounselDto]) -> Option<AnyView> {
    if items.is_empty() {
        return None;
    }
    let rows = items.iter().cloned().map(counsel_row).collect_view();
    Some(
        view! {
            <div class="mt-6 flex flex-col gap-2">
                <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    "Conseils observés"
                </h3>
                <ul class="flex flex-col gap-1.5">{rows}</ul>
            </div>
        }
        .into_any(),
    )
}

fn counsel_row(item: EntityCounselDto) -> AnyView {
    let count = view! {
        <span class="shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">
            {format!("{} déc.", group_thousands(item.count))}
        </span>
    };
    let name = match &item.uid {
        Some(uid) => {
            let (ns, local) = split_entity_uid(uid);
            let href = format!("/entite/{ns}/{local}");
            view! {
                <A
                    href=href
                    attr:class="min-w-0 truncate text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    {item.name.clone()}
                </A>
            }
            .into_any()
        }
        None => view! {
            <span class="min-w-0 truncate text-sm text-[var(--color-ink-muted)]">
                {item.name.clone()}
            </span>
        }
        .into_any(),
    };
    view! { <li class="flex items-baseline justify-between gap-3">{name} {count}</li> }.into_any()
}

// ── Registre (APIs externes à l'affichage, ADR 0199) ─────────────────────────

/// Volet registre streamé : identité enrichie/dirigeants/finances
/// (recherche-entreprises) et annonces officielles (BODACC/JOAFE), servis par
/// les APIs publiques sans stock local. Rien à afficher (namespace sans volet,
/// amont indisponible) → la section n'existe pas dans le flux.
#[component]
fn RegistreSection(registre: Resource<Option<EntityRegistreResponse>>) -> impl IntoView {
    view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move { registre.await.and_then(registre_view) })}
        </Suspense>
    }
}

fn registre_view(data: EntityRegistreResponse) -> Option<AnyView> {
    if data.entreprise.is_none() && data.annonces.is_empty() && data.liens.is_empty() {
        return None;
    }
    let entreprise = data.entreprise.map(entreprise_block);
    let annonces = annonces_block(data.annonces, data.annonces_total);
    let liens = liens_block(data.liens);
    Some(
        view! {
            <section aria-label="Registre" class="mt-10">
                <h2 class="font-sans text-base text-[var(--color-ink)]">"Registre"</h2>
                {entreprise}
                {annonces}
                {liens}
            </section>
        }
        .into_any(),
    )
}

/// Identité enrichie + dirigeants + finances (entreprises `siren:`).
fn entreprise_block(e: RegistreEntrepriseDto) -> AnyView {
    let mut identity: Vec<(&'static str, String)> = Vec::new();
    if let Some(adresse) = e.siege_adresse {
        identity.push(("Siège", adresse));
    }
    if let Some(naf) = e.activite_naf {
        identity.push(("Activité (NAF)", naf));
    }
    if let Some(date) = e.date_creation {
        identity.push(("Création", format_iso_date(Some(&date))));
    }
    if let Some(effectif) = e.effectif {
        identity.push(("Effectif", effectif));
    }
    let identity_rows = (!identity.is_empty()).then(|| {
        let rows = identity
            .into_iter()
            .map(|(label, value)| {
                view! {
                    <div class="flex gap-3 text-sm">
                        <dt class="w-28 shrink-0 text-[var(--color-ink-subtle)]">{label}</dt>
                        <dd class="min-w-0 text-[var(--color-ink)]">{value}</dd>
                    </div>
                }
            })
            .collect_view();
        view! { <dl class="mt-4 flex flex-col gap-1.5">{rows}</dl> }
    });

    let dirigeants = (!e.dirigeants.is_empty()).then(|| {
        let rows = e
            .dirigeants
            .into_iter()
            .map(|d| {
                let qualite = d.qualite.map(|q| {
                    view! {
                        <span class="shrink-0 text-xs text-[var(--color-ink-subtle)]">{q}</span>
                    }
                });
                view! {
                    <li class="flex items-baseline justify-between gap-3">
                        <span class="min-w-0 truncate text-sm text-[var(--color-ink)]">
                            {d.nom}
                        </span>
                        {qualite}
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="mt-6 flex flex-col gap-2">
                <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    "Dirigeants"
                </h3>
                <ul class="flex flex-col gap-1.5">{rows}</ul>
            </div>
        }
    });

    let finances = (!e.finances.is_empty()).then(|| {
        let rows = e
            .finances
            .into_iter()
            .map(|f| {
                let ca = f
                    .chiffre_affaires
                    .map(|v| format!("CA {}", format_euros(v)));
                let resultat = f
                    .resultat_net
                    .map(|v| format!("résultat {}", format_euros(v)));
                let detail = [ca, resultat]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                view! {
                    <li class="flex items-baseline justify-between gap-3 text-sm">
                        <span class="text-[var(--color-ink-muted)]">{f.annee}</span>
                        <span class="tabular-nums text-[var(--color-ink)]">{detail}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="mt-6 flex flex-col gap-2">
                <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    "Comptes publiés"
                </h3>
                <ul class="flex flex-col gap-1.5">{rows}</ul>
            </div>
        }
    });

    view! {
        {identity_rows}
        {dirigeants}
        {finances}
    }
    .into_any()
}

/// Montant en euros, signe conservé, milliers groupés (`format_siren` du pauvre
/// pour la monnaie : `-2 469 630 €`).
fn format_euros(value: i64) -> String {
    format!("{} €", group_thousands(value))
}

/// Annonces officielles (BODACC/JOAFE), plus récentes d'abord. Le PDF JOAFE
/// est un lien direct vers l'hébergement DILA — jamais proxifié.
fn annonces_block(annonces: Vec<lj_dtos::RegistreAnnonceDto>, total: i64) -> Option<AnyView> {
    if annonces.is_empty() {
        return None;
    }
    let shown = annonces.len() as i64;
    let rows = annonces
        .into_iter()
        .map(|a| {
            let date = a.date.map(|d| {
                view! {
                    <span class="w-24 shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">
                        {format_iso_date(Some(&d))}
                    </span>
                }
            });
            let pdf = a.url_pdf.map(|url| {
                view! {
                    <a
                        href=url
                        target="_blank"
                        rel="noopener external"
                        class="shrink-0 text-xs text-[var(--color-ink)] underline underline-offset-4 hover:text-[var(--color-accent)]"
                    >
                        "PDF ↗"
                    </a>
                }
            });
            view! {
                <li class="flex items-baseline gap-3">
                    {date}
                    <span class="min-w-0 flex-1 truncate text-sm text-[var(--color-ink)]">
                        {a.famille}
                    </span>
                    {pdf}
                </li>
            }
        })
        .collect_view();
    let more = (total > shown).then(|| {
        view! {
            <p class="text-xs text-[var(--color-ink-subtle)]">
                {format!("{} annonces au total.", group_thousands(total))}
            </p>
        }
    });
    Some(
        view! {
            <div class="mt-6 flex flex-col gap-2">
                <h3 class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    "Annonces officielles"
                </h3>
                <ul class="flex flex-col gap-1.5">{rows}</ul>
                {more}
            </div>
        }
        .into_any(),
    )
}

/// Liens sortants vers les sources officielles (annuaire-entreprises, INPI).
fn liens_block(liens: Vec<lj_dtos::RegistreLienDto>) -> Option<AnyView> {
    if liens.is_empty() {
        return None;
    }
    let items = liens
        .into_iter()
        .map(|l| {
            view! {
                <a
                    href=l.url
                    target="_blank"
                    rel="noopener external"
                    class="inline-flex items-center gap-1 rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-1 text-xs text-[var(--color-ink-muted)] hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
                >
                    {l.label}
                    <span aria-hidden="true">"↗"</span>
                </a>
            }
        })
        .collect_view();
    Some(view! { <div class="mt-6 flex flex-wrap gap-2">{items}</div> }.into_any())
}

// ── Décisions (liste paginée) ──────────────────────────────────────────────────

#[component]
fn DecisionsSection(
    decisions: Resource<EntityDecisionsResult>,
    base_path: String,
    page: Signal<i64>,
) -> impl IntoView {
    let base_path = StoredValue::new(base_path);
    view! {
        <section aria-label="Décisions" class="mt-10">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Décisions"</h2>
            <Suspense fallback=move || {
                view! {
                    <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">
                        "Chargement des décisions…"
                    </p>
                }
            }>
                {move || Suspend::new(async move {
                    let resolved = decisions.await;
                    decisions_view(resolved, &base_path.get_value(), page.get())
                })}
            </Suspense>
        </section>
    }
}

fn decisions_view(resolved: EntityDecisionsResult, base_path: &str, page: i64) -> AnyView {
    if let Some(err) = resolved.error {
        return view! {
            <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">
                {format!("Décisions indisponibles ({err}).")}
            </p>
        }
        .into_any();
    }
    let Some(response) = resolved.response else {
        return ().into_any();
    };
    if response.items.is_empty() {
        return view! {
            <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">
                "Aucune décision sur cette page."
            </p>
        }
        .into_any();
    }
    let total = response.total;
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    // Même carte que les résultats de recherche (`ResultCard`, unité de rendu) :
    // hits `SearchHit` complets, badge de rôle de l'entité par carte, graine de
    // navigation prev/next bornée aux décisions de la page courante.
    let hit_ids: std::sync::Arc<Vec<String>> =
        std::sync::Arc::new(response.items.iter().map(|i| i.hit.id.clone()).collect());
    let cards = response
        .items
        .into_iter()
        .enumerate()
        .map(|(index, item)| decision_result_card(item, index, page, total, hit_ids.clone()))
        .collect_view();
    let pagination = pagination_view(base_path, page, total_pages);
    view! {
        <ol class="mt-2 flex flex-col">{cards}</ol>
        {pagination}
    }
    .into_any()
}

/// Une décision de la fiche en carte résultat de recherche, coiffée du badge de
/// rôle de l'entité (Demandeur / Défendeur / Conseil).
fn decision_result_card(
    item: EntityDecisionHitDto,
    index: usize,
    page: i64,
    total: i64,
    hit_ids: std::sync::Arc<Vec<String>>,
) -> AnyView {
    let role = role_label(item.side.as_deref(), &item.quality).map(str::to_string);
    view! {
        <li class="contents">
            <ResultCard
                hit=item.hit
                index=index
                page=page as u32
                total=total
                page_size=PAGE_SIZE as u32
                hit_ids=hit_ids
                auto_load_summary=false
                animate=false
                role_badge=role
            />
        </li>
    }
    .into_any()
}

/// Rôle de l'entité dans la décision : conseil > côté (demandeur/défendeur).
fn role_label(side: Option<&str>, quality: &str) -> Option<&'static str> {
    if matches!(quality, "law_firm" | "counsel_name") {
        return Some("Conseil");
    }
    match side {
        Some("applicant") => Some("Demandeur"),
        Some("defendant") => Some("Défendeur"),
        _ => None,
    }
}

/// Pagination `<A>` (précédent / page courante / suivant) — SSR-friendly, la
/// navigation se fait par `?page=`. Masquée s'il n'y a qu'une page.
fn pagination_view(base_path: &str, page: i64, total_pages: i64) -> Option<AnyView> {
    if total_pages <= 1 {
        return None;
    }
    let page = page.min(total_pages);
    let prev = (page > 1).then(|| {
        view! {
            <A
                href=format!("{base_path}?page={}", page - 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                <span aria-hidden="true">"←"</span>
                "Précédent"
            </A>
        }
        .into_any()
    });
    let next = (page < total_pages).then(|| {
        view! {
            <A
                href=format!("{base_path}?page={}", page + 1)
                attr:class="inline-flex items-center gap-1 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            >
                "Suivant"
                <span aria-hidden="true">"→"</span>
            </A>
        }
        .into_any()
    });
    Some(
        view! {
            <nav
                aria-label="Pagination"
                class="mt-6 flex items-center justify-between border-t border-[var(--color-rule)] pt-4"
            >
                <div class="min-w-0">{prev}</div>
                <span class="text-xs tabular-nums text-[var(--color-ink-subtle)]">
                    {format!("Page {page} sur {total_pages}")}
                </span>
                <div class="min-w-0 text-right">{next}</div>
            </nav>
        }
        .into_any(),
    )
}

#[cfg(test)]
mod tests {
    use super::abbreviate_jurisdiction;

    /// Fige la convention d'abréviation, dont le dépliage des articles
    /// contractés (« du Havre » → « Le Havre »).
    #[test]
    fn abbreviation_juridictions() {
        for (full, court) in [
            ("Cour d'appel d'Agen", "CA Agen"),
            ("Cour d'appel de Besançon", "CA Besançon"),
            ("Cour administrative d'appel de Lyon", "CAA Lyon"),
            (
                "Tribunal judiciaire d'Avesnes-sur-Helpe",
                "TJ Avesnes-sur-Helpe",
            ),
            (
                "Tribunal des activités économiques du Havre",
                "TAE Le Havre",
            ),
            (
                "Tribunal judiciaire des Sables-d'Olonne",
                "TJ Les Sables-d'Olonne",
            ),
            ("Tribunal de commerce de Paris", "T. com. Paris"),
            ("Cour de cassation", "Cass."),
            ("Conseil d'État", "CE"),
            ("Cour européenne des droits de l'homme", "CEDH"),
            ("Tribunal des conflits", "T. confl."),
            (
                "Juridiction inconnue de Trifouillis",
                "Juridiction inconnue de Trifouillis",
            ),
        ] {
            assert_eq!(abbreviate_jurisdiction(full), court, "libellé : {full}");
        }
    }
}
