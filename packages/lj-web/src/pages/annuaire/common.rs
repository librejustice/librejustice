//! Éléments partagés par l'annuaire des entités (ADR 0192) : catégories,
//! chargements de données, rail latéral des catégories et rendu d'une ligne
//! d'entité.
//!
//! Les fetchers passent par [`ApiClient`] (transport aiguillé par cible, comme
//! la fiche entité) : in-process `lj_api::entities` au SSR, HTTP
//! `/api/entities/*` côté hydrate.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::{
    AnnuaireCategorieStatsDto, AnnuaireStatsResponse, EntityDirectoryItemDto,
    EntityDirectoryResponse,
};

use crate::api::ApiClient;
use crate::helpers::{group_thousands, split_entity_uid};
use crate::pages::decision_page::data::{sendable, PageError};
use crate::pages::law_page::{rail_block, rail_item};

/// Une catégorie de l'annuaire (namespace × nature), telle qu'exposée dans l'URL
/// (`/annuaire/{slug}`) et transmise à l'API (`kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Entreprises,
    PersonnesPubliques,
    Associations,
    Avocats,
    Cabinets,
}

impl Kind {
    /// Toutes les catégories, dans l'ordre d'affichage de l'accueil.
    pub const ALL: [Kind; 5] = [
        Kind::Entreprises,
        Kind::PersonnesPubliques,
        Kind::Associations,
        Kind::Avocats,
        Kind::Cabinets,
    ];

    /// Catégorie depuis le slug d'URL (`kind`). Slug inconnu ⇒ `None` (404 doux).
    pub fn from_slug(slug: &str) -> Option<Kind> {
        match slug {
            "entreprises" => Some(Kind::Entreprises),
            "personnes-publiques" => Some(Kind::PersonnesPubliques),
            "associations" => Some(Kind::Associations),
            "avocats" => Some(Kind::Avocats),
            "cabinets" => Some(Kind::Cabinets),
            _ => None,
        }
    }

    /// Slug d'URL / paramètre `kind` de l'API.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Entreprises => "entreprises",
            Kind::PersonnesPubliques => "personnes-publiques",
            Kind::Associations => "associations",
            Kind::Avocats => "avocats",
            Kind::Cabinets => "cabinets",
        }
    }

    /// Libellé pluriel (titre de carte / de page).
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Entreprises => "Entreprises",
            Kind::PersonnesPubliques => "Personnes publiques",
            Kind::Associations => "Associations",
            Kind::Avocats => "Avocats",
            Kind::Cabinets => "Cabinets d'avocats",
        }
    }

    /// Baseline descriptive de la catégorie (carte accueil + méta).
    pub fn tagline(self) -> &'static str {
        match self {
            Kind::Entreprises => "Sociétés et entrepreneurs identifiés par leur SIREN.",
            Kind::PersonnesPubliques => "État, collectivités et établissements publics.",
            Kind::Associations => "Associations déclarées identifiées par leur RNA.",
            Kind::Avocats => "Avocats et avocats aux Conseils, par barreau.",
            Kind::Cabinets => "Structures d'exercice des avocats : SCP, SELARL, AARPI…",
        }
    }

    /// Compteurs de cette catégorie dans les stats de l'annuaire
    /// (registre chargé + entités avec contentieux).
    pub fn stats(self, stats: &AnnuaireStatsResponse) -> AnnuaireCategorieStatsDto {
        match self {
            Kind::Entreprises => stats.entreprises,
            Kind::PersonnesPubliques => stats.personnes_publiques,
            Kind::Associations => stats.associations,
            Kind::Avocats => stats.avocats,
            Kind::Cabinets => stats.cabinets,
        }
    }

    /// Participe « lié » accordé au pluriel de la catégorie (« dont N liées à
    /// des décisions de justice »).
    pub fn liees(self) -> &'static str {
        match self {
            Kind::Avocats | Kind::Cabinets => "liés",
            Kind::Entreprises | Kind::PersonnesPubliques | Kind::Associations => "liées",
        }
    }

    /// Seuls les avocats portent un filtre barreau.
    pub fn has_barreau_filter(self) -> bool {
        matches!(self, Kind::Avocats)
    }
}

// ── Chargements ─────────────────────────────────────────────────────────────

/// Compteurs par catégorie (rail + cartes), streamés (non bloquants). Erreur
/// repliée en `None` : rail et cartes restent rendus, compteurs absents.
pub async fn fetch_stats() -> Option<AnnuaireStatsResponse> {
    ApiClient::from_context().fetch_annuaire_stats().await.ok()
}

/// Ressource des compteurs par catégorie, partagée rail + contenu d'une page.
pub fn stats_resource() -> Resource<Option<AnnuaireStatsResponse>> {
    Resource::new(|| (), |_| sendable(fetch_stats()))
}

/// Recherche d'entités (accueil, `?q=`), bloquante SSR. Renvoie la liste d'items.
pub async fn fetch_search(q: String) -> Result<Vec<EntityDirectoryItemDto>, PageError> {
    ApiClient::from_context()
        .search_entities(&q, None, SEARCH_LIMIT)
        .await
        .map(|r| r.items)
        .map_err(PageError::from)
}

/// Listing paginé d'une catégorie (tri contentieux décroissant côté API),
/// bloquant SSR.
pub async fn fetch_directory(
    kind: &'static str,
    barreau: Option<String>,
    page: i64,
) -> Result<EntityDirectoryResponse, PageError> {
    ApiClient::from_context()
        .fetch_entities_directory(kind, barreau.as_deref(), page, PAGE_SIZE)
        .await
        .map_err(PageError::from)
}

/// Résultats max d'une recherche annuaire (≤ plafond API, 50).
const SEARCH_LIMIT: u32 = 25;
/// Entités par page de listing (offset-based, parité fiche entité).
pub const PAGE_SIZE: i64 = 20;
/// Pages max d'un listing — contrat de `/entities/directory`
/// (`ENTITY_DIRECTORY_MAX_DEPTH` = 10 000 lignes côté API, ADR 0239) : la
/// pagination s'arrête là, la recherche par préfixe couvre la traîne.
pub const MAX_PAGES: i64 = 10_000 / PAGE_SIZE;
/// Longueur minimale d'une recherche (codepoints) — contrat de
/// `/entities/search` (`ENTITY_SEARCH_MIN_QUERY` côté API).
pub const SEARCH_MIN_QUERY: usize = 2;

// ── Rail latéral des catégories ─────────────────────────────────────────────

/// Rail « Catégories » (gouttière gauche, sticky sur desktop) : lien « Toutes »
/// vers `/annuaire` puis les 4 catégories, point plein accent sur la catégorie
/// courante (idiome `rail_block`/`rail_item` des pages /texte). Compteurs et
/// barres de proportion streamés avec les stats ; en attendant (ou sur erreur),
/// le rail se rend sans chiffres.
#[component]
pub fn AnnuaireRail(
    #[prop(optional, into)] current: Option<Kind>,
    stats: Resource<Option<AnnuaireStatsResponse>>,
) -> impl IntoView {
    view! {
        <nav
            aria-label="Catégories de l'annuaire"
            class="lg:sticky lg:top-20 lg:self-start"
        >
            <Suspense fallback=move || rail_view(current, None)>
                {move || Suspend::new(async move { rail_view(current, stats.await) })}
            </Suspense>
        </nav>
    }
}

// Le rail compte les entités AVEC contentieux (parité avec les totaux des
// listings qu'il lie) ; les totaux registre vivent sur les cartes de l'accueil.
fn rail_view(current: Option<Kind>, stats: Option<AnnuaireStatsResponse>) -> AnyView {
    let max = stats.as_ref().map(|s| {
        Kind::ALL
            .into_iter()
            .map(|k| k.stats(s).contentieux)
            .max()
            .unwrap_or(1)
            .max(1)
    });
    let total = stats.as_ref().map(|s| {
        Kind::ALL
            .into_iter()
            .map(|k| k.stats(s).contentieux)
            .sum::<i64>()
    });

    let mut items: Vec<AnyView> = Vec::new();
    items.push(rail_item(
        current.is_none(),
        rail_entry(
            "/annuaire".to_string(),
            "Toutes",
            current.is_none(),
            total,
            None,
        ),
    ));
    for kind in Kind::ALL {
        let is_current = current == Some(kind);
        let count = stats.as_ref().map(|s| kind.stats(s).contentieux);
        let bar = count.zip(max).map(|(c, m)| mini_bar(c, m));
        items.push(rail_item(
            is_current,
            rail_entry(
                format!("/annuaire/{}", kind.slug()),
                kind.plural(),
                is_current,
                count,
                bar,
            ),
        ));
    }
    rail_block("Catégories", items)
}

/// Corps d'une entrée du rail : lien (accent, ou encre médium si courant) +
/// compteur tabulaire, barre de proportion en dessous quand les stats sont là.
fn rail_entry(
    href: String,
    label: &'static str,
    current: bool,
    count: Option<i64>,
    bar: Option<AnyView>,
) -> AnyView {
    let label_class = if current {
        "min-w-0 truncate text-sm font-medium text-[var(--color-ink)] no-underline"
    } else {
        "min-w-0 truncate text-sm text-[var(--color-accent)] no-underline hover:underline"
    };
    let count_view = count.map(|c| {
        view! {
            <span class="shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">
                {group_thousands(c)}
            </span>
        }
    });
    view! {
        <div class="flex flex-col gap-1.5">
            <span class="flex items-baseline justify-between gap-2">
                <A href=href attr:class=label_class>
                    {label}
                </A>
                {count_view}
            </span>
            {bar}
        </div>
    }
    .into_any()
}

/// Barre de proportion fine (jauge accent sur piste `rule`, parité des barres
/// de la fiche entité).
pub fn mini_bar(count: i64, max: i64) -> AnyView {
    let pct = ((count as f64 / max.max(1) as f64) * 100.0)
        .round()
        .max(4.0);
    view! {
        <span class="relative block h-1 w-full overflow-hidden rounded-full bg-[var(--color-rule)]/40">
            <span
                class="absolute inset-y-0 left-0 rounded-full bg-[var(--color-accent)]"
                style=format!("width:{pct}%")
            />
        </span>
    }
    .into_any()
}

/// Chips de catégories (mobile, remplace le rail sous `lg`) : « Toutes » +
/// les 4 catégories, chip accentuée sur la courante.
pub fn category_chips(current: Option<Kind>) -> AnyView {
    let chip = |href: String, label: &'static str, is_current: bool| {
        let class = if is_current {
            "rounded-full border border-[var(--color-accent)] px-3 py-1 text-xs text-[var(--color-accent)] no-underline"
        } else {
            "rounded-full border border-[var(--color-rule)] px-3 py-1 text-xs text-[var(--color-ink-muted)] no-underline transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
        };
        view! {
            <A href=href attr:class=class>
                {label}
            </A>
        }
        .into_any()
    };
    let mut chips: Vec<AnyView> = vec![chip("/annuaire".to_string(), "Toutes", current.is_none())];
    for kind in Kind::ALL {
        chips.push(chip(
            format!("/annuaire/{}", kind.slug()),
            kind.plural(),
            current == Some(kind),
        ));
    }
    view! {
        <div class="flex flex-wrap items-center gap-1.5 lg:hidden">{chips}</div>
    }
    .into_any()
}

// ── Rendu partagé ───────────────────────────────────────────────────────────

/// Une ligne d'entité (résultat de recherche ou listing) : dénomination liée à
/// la fiche + badges (forme juridique, barreau, état), volume de contentieux à
/// droite avec sa barre de proportion (relative au max de la liste).
pub fn entity_row(item: EntityDirectoryItemDto, max_count: i64) -> AnyView {
    let (ns, local) = split_entity_uid(&item.uid);
    let href = format!("/entite/{ns}/{local}");

    let decisions = match item.decision_count {
        1 => "1 déc.".to_string(),
        n => format!("{} déc.", group_thousands(n)),
    };

    let mut badges: Vec<AnyView> = Vec::new();
    if let Some(forme) = item.forme.clone().filter(|f| !f.trim().is_empty()) {
        badges.push(row_badge(forme));
    }
    if let Some(barreau) = item.barreau.clone().filter(|b| !b.trim().is_empty()) {
        badges.push(row_badge(barreau));
    }
    if !item.active {
        badges.push(
            view! {
                <span class="inline-flex items-center rounded-full border border-[var(--color-rule)] px-2 py-0.5 text-xs text-[var(--color-ink-subtle)]">
                    "Cessée"
                </span>
            }
            .into_any(),
        );
    }
    let badges_row = (!badges.is_empty())
        .then(|| view! { <div class="flex flex-wrap items-center gap-1.5">{badges}</div> });

    view! {
        <li class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] p-3">
            <div class="flex items-start justify-between gap-4">
                <div class="flex min-w-0 flex-col gap-1.5">
                    <A
                        href=href
                        attr:class="font-sans text-sm leading-snug text-[var(--color-ink)] no-underline transition-colors hover:text-[var(--color-accent)]"
                    >
                        {item.denomination.clone()}
                    </A>
                    {badges_row}
                </div>
                <div class="flex w-24 shrink-0 flex-col items-end gap-1.5 pt-0.5">
                    <span class="text-xs tabular-nums text-[var(--color-ink-subtle)]">
                        {decisions}
                    </span>
                    {mini_bar(item.decision_count, max_count)}
                </div>
            </div>
        </li>
    }
    .into_any()
}

/// Badge neutre d'une ligne (forme juridique, barreau) — parité des badges de
/// l'en-tête de la fiche entité, fond vellum léger sur la carte parchment.
fn row_badge(text: String) -> AnyView {
    view! {
        <span class="inline-flex items-center rounded-full border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 px-2 py-0.5 text-xs text-[var(--color-ink-muted)]">
            {text}
        </span>
    }
    .into_any()
}

/// Max de contentieux d'une liste d'items (division des barres de proportion).
pub fn max_decision_count(items: &[EntityDirectoryItemDto]) -> i64 {
    items
        .iter()
        .map(|i| i.decision_count)
        .max()
        .unwrap_or(1)
        .max(1)
}

/// Message d'état neutre (chargement / vide / erreur) sous une section annuaire.
pub fn status_note(text: impl Into<String>) -> AnyView {
    let text = text.into();
    view! { <p class="mt-4 text-sm text-[var(--color-ink-subtle)]">{text}</p> }.into_any()
}

/// Squelette d'une liste d'entités (fallback SSR / chargement).
pub fn list_skeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mt-4 flex flex-col gap-3">
            {(0..6).map(|_| view! { <Skeleton class="h-14 w-full rounded-md" /> }).collect::<Vec<_>>()}
        </div>
    }
}
