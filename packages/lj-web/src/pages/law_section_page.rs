//! Page `/norm/{juris}/{code}/{path}` (LawSectionPage) : vue-lecture d'une
//! division d'un texte au gabarit Légifrance `section_lc` — articles rendus à
//! la suite avec intertitres, fil d'Ariane des divisions englobantes, plan
//! « Dans cette division », décisions citant la division, divisions voisines.
//! Contenu bloquant SSR (SEO), `Cache-Control` selon le statut, calqué sur
//! [`crate::pages::law_code_page`].
//!
//! Gabarit trois colonnes commun aux pages document, monté par
//! [`DecisionLayout`] : gauche = s'y repérer dans le temps et dans le plan,
//! centre = la division et ce qui la cite, droite = les rebonds du corpus.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use lj_dtos::{LawSectionItem, LawSectionRef, LawSectionResponse, TopCitedArticle};

use crate::components::decision::DecisionLayout;
use crate::components::layout::SHELL;
use crate::components::own_page_description;
use crate::components::search::compact_search::highlight::CitedBlock;
use crate::helpers::{decisions_href, format_article_num, group_thousands};
use crate::pages::law_page::cited_by::{section as cited_by_section, CitedBySubject, CitedText};
use crate::pages::law_page::data::{
    chrono_date, fetch_section, fetch_section_citing, section_key, sendable, CitingResult,
    PageError,
};
use crate::pages::law_page::top_cited::top_cited_block;
use crate::pages::law_page::{
    norm_trail, rail_block, rail_item, status_label, ChronoDatePicker, TrailDivision,
};

/// Articles portés par le « voir tout » de la division : la facette
/// `legalArticle` prend une valeur par article, et une URL n'en porte pas deux
/// mille — un Livre en compte autant. Même borne que l'agrégation servie par
/// l'API (`legi::section_citing`).
const FACET_VALUES: usize = 20;

#[component]
pub fn LawSectionPage() -> impl IntoView {
    // La page pose sa propre description (parfois depuis un `<Suspense>`) :
    // on la declare ici, synchroniquement, sinon la generique du shell sort aussi.
    own_page_description();

    let key = section_key();
    // Datée par `?date=` (Chronolégi) : sous-arbre et corps à la date demandée,
    // sinon en vigueur.
    let date = chrono_date();
    let section = Resource::new_blocking(
        move || (key.get(), date.get()),
        |((juris, code, path), date)| sendable(fetch_section(juris, code, path, date)),
    );
    // Décisions citant la division : non bloquantes, streamées sous la lecture —
    // la date choisit la composition du sous-arbre agrégé.
    let citing = Resource::new(
        move || (key.get(), date.get()),
        |((juris, code, path), date)| sendable(fetch_section_citing(juris, code, path, date)),
    );

    view! {
        <Suspense fallback=SectionSkeleton>
            {move || Suspend::new(async move {
                match section.await {
                    Ok(section) => {
                        set_cache_control(200);
                        Either::Left(
                            view! {
                                <LawSectionLoaded section=section citing=citing date=date.get() />
                            },
                        )
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <SectionError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

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
fn SectionError(err: PageError) -> impl IntoView {
    if err.status == 404 {
        crate::pages::not_found::signaler_lien_mort();
    }
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    view! {
        <Title text="Section introuvable - LibreJustice" />
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
fn SectionSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class=format!("{SHELL} flex flex-1 flex-col gap-6")>
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-4 w-full" />
            <Skeleton class="h-4 w-full" />
        </div>
    }
}

#[component]
pub(crate) fn LawSectionLoaded(
    section: LawSectionResponse,
    citing: Resource<CitingResult>,
    /// Date de consultation Chronolégi (`?date=`), propagée aux liens.
    #[prop(optional_no_strip)]
    date: Option<String>,
) -> impl IntoView {
    let code = section.code.clone();
    let text = lj_vocab::norm_path(&section.jurisdiction, &section.code);
    let code_title = section
        .code_title
        .clone()
        .unwrap_or_else(|| section.code.clone());
    let page_title = format!("{} - {} - LibreJustice", section.title, code_title);
    let description = format!(
        "{} — {}. Lecture à la suite des articles de la division, texte consolidé en vigueur.",
        section.title, code_title
    );
    let url = format!("https://librejustice.fr{text}/{}", section.path);
    #[cfg(feature = "hydrate")]
    {
        let document = url.clone();
        Effect::new(move |_| crate::reads::report(lj_dtos::ReadKind::Section, &document));
    }

    let en_vigueur = section
        .items
        .iter()
        .filter(|i| i.kind == "article" && i.etat == "VIGUEUR")
        .count();
    let abroges = section
        .items
        .iter()
        .filter(|i| i.kind == "article" && i.etat != "VIGUEUR")
        .count();
    let compte = match (en_vigueur, abroges) {
        (1, 0) => "1 article en vigueur".to_string(),
        (n, 0) => format!("{n} articles en vigueur"),
        (n, 1) => format!("{n} en vigueur, 1 abrogé"),
        (n, m) => format!("{n} en vigueur, {m} abrogés"),
    };

    // La date de consultation suit dans le fil (Chronolégi).
    let q = date
        .as_deref()
        .map(|d| format!("?date={d}"))
        .unwrap_or_default();
    let breadcrumb = norm_trail(
        section.jurisdiction_label.clone(),
        format!("{text}{q}"),
        code_title.clone(),
        section
            .ancestors
            .iter()
            .map(|a| TrailDivision {
                label: a.label.clone(),
                href: Some(format!("{text}/{}{q}", a.path)),
                whole: false,
            })
            .collect(),
    );
    let footer = nav_footer(
        &text,
        section.prev.clone(),
        section.next.clone(),
        date.as_deref(),
    );
    let neighbors = neighbor_rail(&text, section.prev, section.next, date.as_deref());

    let hide_abroges = RwSignal::new(false);
    // Contenu de la gouttière gauche, rendu deux fois (voir plus bas) : la
    // closure le rebâtit, une vue ne se rend pas à deux endroits.
    let rail_content = {
        let base = format!("{text}/{}", section.path);
        let date = date.clone();
        let items = section.items.clone();
        move || {
            let picker = view! {
                <ChronoDatePicker
                    base=base.clone()
                    date=Signal::derive({
                        let date = date.clone();
                        move || date.clone()
                    })
                />
            };
            let toggle = (abroges > 0).then(|| {
                view! {
                    <button
                        type="button"
                        on:click=move |_| hide_abroges.update(|v| *v = !*v)
                        class="inline-flex items-center gap-1 rounded-md border border-[var(--color-rule)] px-2.5 py-1 text-xs text-[var(--color-ink-subtle)] transition-colors hover:border-[var(--color-ink)] hover:text-[var(--color-ink)]"
                    >
                        {move || {
                            if hide_abroges.get() {
                                "Afficher les articles abrogés"
                            } else {
                                "Masquer les articles abrogés"
                            }
                        }}
                    </button>
                }
            });
            view! {
                {picker}
                {division_rail(&items)}
                {toggle}
            }
        }
    };

    // « Cité par » au centre : décisions citantes de tout le sous-arbre, puis
    // textes citants agrégés au grain du texte. Le « voir tout » compose la
    // facette `legalArticle` sur les articles de tête, le repli plein-texte prend
    // le titre de la division.
    let subject = CitedBySubject {
        name: "cette division",
        all_href: decisions_href(
            facet_values(&code, &section.items, &section.top_cited)
                .into_iter()
                .map(|value| ("legalArticle", value)),
        ),
        search_href: decisions_href([("q", section.title.clone())]),
    };
    let citing_texts: Vec<CitedText> = section
        .citing_texts
        .into_iter()
        .map(CitedText::from)
        .collect();
    // Articles de la division en ordre de lecture, pour que chaque ligne de
    // décision dise par quel article elle entre : l'API ne renvoie que les clés
    // canoniques, les numéros affichables sont dans la vue-lecture.
    let read_order = StoredValue::new(reading_articles(&section.items));
    let cited_by = view! {
        <Suspense fallback=move || {
            view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">"Chargement des décisions…"</p>
            }
        }>
            {move || {
                let subject = subject.clone();
                let texts = citing_texts.clone();
                Suspend::new(async move {
                    cited_by_section(citing.await, texts, &subject, &read_order.get_value())
                })
            }}
        </Suspense>
    };
    let top_cited = top_cited_block(section.top_cited);

    let trop_longue = section.omitted.map(|n| {
        view! {
            <p class="rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/50 px-3 py-2 text-sm text-[var(--color-ink-subtle)]">
                {format!(
                    "Division trop longue pour être lue d'un bloc : ses {} articles sont listés, \
                     chacun s'ouvre sur sa page.",
                    group_thousands(n as i64),
                )}
            </p>
        }
    });
    let items = section
        .items
        .into_iter()
        .map(|item| section_item_view(item, &text, date.as_deref()))
        .collect_view();
    let reading = view! {
        <div class="flex flex-col" class:abroges-masques=move || hide_abroges.get()>
            {trop_longue}
            {items}
        </div>
    };

    // Rail gauche : la dimension temporelle, le plan local, puis l'action de
    // lecture. Scrollable indépendamment — le plan d'un Livre dépasse la
    // fenêtre.
    let toc = view! {
        <div class="flex flex-col gap-6 lg:sticky lg:top-20 lg:max-h-[calc(100vh-6rem)] lg:self-start lg:overflow-y-auto">
            {rail_content()}
        </div>
    }
    .into_any();
    let main = view! {
        <div class="flex flex-col gap-6">
            {breadcrumb}
            <header class="flex flex-col gap-3">
                <h1 class="font-sans text-2xl text-[var(--color-ink)] sm:text-3xl">
                    {section.title}
                </h1>
                <p class="text-sm text-[var(--color-ink-subtle)]">{compte}</p>
            </header>
            // Sous `lg`, `DecisionLayout` efface les deux gouttières : le rail
            // gauche est ré-injecté ici, plan avant le corps, comme la page
            // décision ré-injecte son sommaire.
            <div class="flex flex-col gap-6 lg:hidden">{rail_content()}</div>
            {reading}
            {cited_by}
            {footer}
        </div>
    }
    .into_any();
    let similar = view! {
        <aside
            aria-label="Articles et divisions liés"
            class="flex flex-col gap-6 lg:sticky lg:top-20 lg:self-start"
        >
            {top_cited}
            {neighbors}
        </aside>
    }
    .into_any();

    view! {
        <Title text=page_title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />
        <DecisionLayout toc=toc main=main similar=similar />
    }
}

/// Articles de la division en ordre de lecture, `(clé canonique, numéro
/// affichable)` : la table dans laquelle une ligne de décision citante résout
/// les articles qu'elle vise.
fn reading_articles(items: &[LawSectionItem]) -> Vec<(String, String)> {
    items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| {
            let num_key = i.num_key.clone()?;
            Some((num_key, format_article_num(&i.label)))
        })
        .collect()
}

/// Valeurs de la facette `legalArticle` du « voir tout » : les articles les plus
/// cités d'abord, complétés dans l'ordre du document, bornées à
/// [`FACET_VALUES`].
fn facet_values(code: &str, items: &[LawSectionItem], top: &[TopCitedArticle]) -> Vec<String> {
    let doc_order = items
        .iter()
        .filter(|i| i.kind == "article")
        .filter_map(|i| i.num_key.as_deref());
    let mut keys: Vec<&str> = Vec::with_capacity(FACET_VALUES);
    for key in top.iter().map(|a| a.num_key.as_str()).chain(doc_order) {
        if keys.len() == FACET_VALUES {
            break;
        }
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.into_iter().map(|k| format!("{code}|{k}")).collect()
}

/// Rail « Dans cette division » : le plan local — les **enfants directs** de la
/// division, sous-divisions (ancre `#{path}`) et articles (ancre
/// `#art-{num_key}`, portée par les `<article>` de la vue-lecture). Une division
/// qui ne porte que des articles a donc un plan, elle aussi. Rien à lister ⇒ pas
/// de rail.
///
/// Le plan s'arrête au premier niveau : le sous-arbre entier d'une grosse
/// division est un dump, pas un plan.
fn division_rail(items: &[LawSectionItem]) -> Option<AnyView> {
    let mut entries: Vec<AnyView> = Vec::new();
    for item in items {
        if item.depth != 1 {
            continue;
        }
        let (href, label, cls) = if item.kind == "section" {
            let Some(path) = item.path.as_deref() else {
                continue;
            };
            (
                format!("#{path}"),
                item.label.clone(),
                "block text-sm leading-snug text-[var(--color-accent)] no-underline hover:underline",
            )
        } else {
            let Some(num_key) = item.num_key.as_deref() else {
                continue;
            };
            (
                format!("#art-{num_key}"),
                format!("Article {}", format_article_num(&item.label)),
                "block text-xs leading-snug text-[var(--color-ink-muted)] no-underline hover:text-[var(--color-accent)] hover:underline",
            )
        };
        let body = view! { <a href=href class=cls>{label}</a> }.into_any();
        entries.push(rail_item(false, body));
    }
    if entries.is_empty() {
        return None;
    }
    Some(
        view! {
            <nav aria-label="Dans cette division">
                {rail_block("Dans cette division", entries)}
            </nav>
        }
        .into_any(),
    )
}

/// Colonne droite : les divisions voisines en ordre de lecture, hors sous-arbre
/// de la division courante — la piste droite ne porte que des rebonds. Aucune
/// voisine ⇒ pas de bloc.
fn neighbor_rail(
    text: &str,
    prev: Option<LawSectionRef>,
    next: Option<LawSectionRef>,
    date: Option<&str>,
) -> AnyView {
    if prev.is_none() && next.is_none() {
        return ().into_any();
    }
    let q = date.map(|d| format!("?date={d}")).unwrap_or_default();
    let cards: Vec<AnyView> = [
        prev.map(|t| neighbor_card(text, t, "← Bloc précédent", &q, STACKED)),
        next.map(|t| neighbor_card(text, t, "Bloc suivant →", &q, STACKED)),
    ]
    .into_iter()
    .flatten()
    .collect();
    view! {
        <div class="flex flex-col gap-2">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Divisions voisines"</h2>
            <div class="flex flex-col gap-3">{cards}</div>
        </div>
    }
    .into_any()
}

/// Navigation bloc précédent / bloc suivant en pied de lecture, mobile
/// uniquement — sur desktop la piste droite porte les divisions voisines, mais
/// sous `lg` les gouttières disparaissent (même patron que les articles voisins
/// de la page article).
fn nav_footer(
    text: &str,
    prev: Option<LawSectionRef>,
    next: Option<LawSectionRef>,
    date: Option<&str>,
) -> AnyView {
    if prev.is_none() && next.is_none() {
        return ().into_any();
    }
    let q = date.map(|d| format!("?date={d}")).unwrap_or_default();
    let card =
        |target: Option<LawSectionRef>, eyebrow: &'static str, layout: &'static str| match target {
            Some(t) => neighbor_card(text, t, eyebrow, &q, layout),
            None => view! { <span class="flex-1" /> }.into_any(),
        };
    view! {
        <nav
            aria-label="Divisions voisines"
            class="mt-4 flex flex-col gap-3 border-t border-[var(--color-rule)] pt-6 sm:flex-row lg:hidden"
        >
            {card(prev, "← Bloc précédent", PAIRED_LEFT)}
            {card(next, "Bloc suivant →", PAIRED_RIGHT)}
        </nav>
    }
    .into_any()
}

/// Cartes voisines empilées, une par ligne (rail droit).
const STACKED: &str = "items-start text-left";
/// Cartes voisines côte à côte, largeur partagée (pied de lecture) : le bloc
/// précédent à gauche, le suivant aligné à droite.
const PAIRED_LEFT: &str = "flex-1 items-start text-left";
const PAIRED_RIGHT: &str = "flex-1 items-end text-right";

/// Carte de rebond vers une division voisine : œillard puis libellé, la
/// grammaire bordée des rebonds. `layout` place la carte dans son conteneur.
fn neighbor_card(
    text: &str,
    target: LawSectionRef,
    eyebrow: &'static str,
    q: &str,
    layout: &'static str,
) -> AnyView {
    let href = format!("{text}/{}{q}", target.path);
    view! {
        <A
            href=href
            attr:class=format!(
                "group flex flex-col gap-1 rounded-md border border-[var(--color-rule)] px-4 py-3 no-underline transition-colors hover:border-[var(--color-ink)] {layout}",
            )
        >
            <span class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                {eyebrow}
            </span>
            <span class="line-clamp-2 text-sm leading-snug text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                {target.label}
            </span>
        </A>
    }
    .into_any()
}

/// Rend un item de la vue-lecture : intertitre de sous-division (taille selon
/// la profondeur, ancré `id = path`, lié à sa vue-lecture) ou article complet (numéro lié à sa page,
/// corps, Nota). Les items abrogés portent `item-abroge` (masquables d'un
/// clic, règle CSS `abroges-masques`). Partagé avec la vue-lecture intégrale
/// des textes courts (`law_code_page`).
pub(crate) fn section_item_view(item: LawSectionItem, text: &str, date: Option<&str>) -> AnyView {
    let abroge = item.etat != "VIGUEUR";
    if item.kind == "section" {
        let cls = match item.depth {
            1 => "mt-10 scroll-mt-24 border-t border-[var(--color-rule)] pt-6 font-sans text-xl text-[var(--color-ink)]",
            2 => "mt-8 scroll-mt-24 font-sans text-lg text-[var(--color-ink)]",
            _ => "mt-6 scroll-mt-24 font-sans text-base font-medium text-[var(--color-ink)]",
        };
        let cls = if abroge {
            format!("item-abroge {cls} text-[var(--color-ink-subtle)]")
        } else {
            cls.to_string()
        };
        let badge = abroge.then(|| {
            view! {
                <span class="ml-2 align-middle text-xs uppercase tracking-wide text-[var(--color-ink-subtle)]">
                    {status_label(&item.etat)}
                </span>
            }
        });
        let q = date.map(|d| format!("?date={d}")).unwrap_or_default();
        let title = match item.path.as_deref() {
            Some(path) => view! {
                <A
                    href=format!("{text}/{path}{q}")
                    attr:class="no-underline underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    {item.label}
                </A>
            }
            .into_any(),
            None => item.label.into_any(),
        };
        return view! {
            <h2 id=item.path class=cls>
                {title}
                {badge}
            </h2>
        }
        .into_any();
    }
    let num_key = item.num_key.unwrap_or_default();
    // Chronolégi : le lien de l'article porte la date de consultation.
    let href = match date {
        Some(d) => format!("{text}/{num_key}/{d}"),
        None => format!("{text}/{num_key}"),
    };
    let id = format!("art-{num_key}");
    let label = format!("Article {}", format_article_num(&item.label));
    let etat_badge = abroge.then(|| {
        view! {
            <span class="text-xs uppercase tracking-wide text-[var(--color-ink-subtle)]">
                {status_label(&item.etat)}
            </span>
        }
    });
    let spans = item.texte_spans;
    let texte = item.texte.map(|t| {
        view! {
            <CitedBlock
                text=t
                spans=spans
                class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]"
            />
        }
    });
    let nota = item.nota.map(|n| {
        view! {
            <div class="border-l-2 border-[var(--color-rule)] pl-3">
                <p class="text-xs font-medium uppercase tracking-wide text-[var(--color-ink-subtle)]">
                    "Nota"
                </p>
                <div class="mt-1 whitespace-pre-line text-sm leading-relaxed text-[var(--color-ink-subtle)]">
                    {n}
                </div>
            </div>
        }
    });
    let cls = if abroge {
        "item-abroge mt-6 flex scroll-mt-24 flex-col gap-2 opacity-70"
    } else {
        "mt-6 flex scroll-mt-24 flex-col gap-2"
    };
    view! {
        <article id=id class=cls>
            <p class="flex items-baseline gap-2">
                <A
                    href=href
                    attr:class="font-sans text-base font-medium text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    {label}
                </A>
                {etat_badge}
            </p>
            {texte}
            {nota}
        </article>
    }
    .into_any()
}
