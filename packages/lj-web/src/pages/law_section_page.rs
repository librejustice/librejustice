//! Page `/texte/{code}/section/{cid}` (LawSectionPage, ADR 0207) : vue-lecture
//! d'une division d'un texte au gabarit Légifrance `section_lc` — articles
//! rendus à la suite avec intertitres, fil d'Ariane des divisions englobantes,
//! rail « Dans cette division », navigation bloc précédent/suivant, masquage
//! des articles abrogés. Contenu bloquant SSR (SEO), `Cache-Control` selon le
//! statut, calqué sur [`crate::pages::law_code_page`].

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use lj_dtos::{LawSectionItem, LawSectionRef, LawSectionResponse};

use crate::helpers::format_article_num;
use crate::pages::law_page::data::{chrono_date, fetch_section, section_key, sendable, PageError};
use crate::pages::law_page::{rail_block, rail_item, status_label, ChronoDatePicker};

#[component]
pub fn LawSectionPage() -> impl IntoView {
    let key = section_key();
    // Datée par `?date=` (Chronolégi, ADR 0193 §5) : sous-arbre et corps à la
    // date demandée, sinon en vigueur.
    let date = chrono_date();
    let section = Resource::new_blocking(
        move || (key.get(), date.get()),
        |((code, cid), date)| sendable(fetch_section(code, cid, date)),
    );

    view! {
        <Suspense fallback=SectionSkeleton>
            {move || Suspend::new(async move {
                match section.await {
                    Ok(section) => {
                        set_cache_control(200);
                        Either::Left(
                            view! { <LawSectionLoaded section=section date=date.get() /> },
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
        <div class="mx-auto flex w-full max-w-[92rem] flex-1 flex-col gap-6 px-4 py-8 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-4 w-full" />
            <Skeleton class="h-4 w-full" />
        </div>
    }
}

#[component]
fn LawSectionLoaded(
    section: LawSectionResponse,
    /// Date de consultation Chronolégi (`?date=`), propagée aux liens.
    #[prop(optional_no_strip)]
    date: Option<String>,
) -> impl IntoView {
    let code = section.code.clone();
    let code_title = section
        .code_title
        .clone()
        .unwrap_or_else(|| section.code.clone());
    let page_title = format!("{} - {} - LibreJustice", section.title, code_title);
    let description = format!(
        "{} — {}. Lecture à la suite des articles de la division, texte consolidé en vigueur.",
        section.title, code_title
    );
    let url = format!(
        "https://librejustice.fr/texte/{}/section/{}",
        section.code, section.cid
    );

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
    let compte = if abroges > 0 {
        format!("{en_vigueur} articles en vigueur, {abroges} abrogés")
    } else {
        format!("{en_vigueur} articles, lecture à la suite")
    };

    let breadcrumb = breadcrumb(
        &code,
        code_title.clone(),
        &section.ancestors,
        date.as_deref(),
    );
    let rail = division_rail(&section.items);
    let footer = nav_footer(&code, section.prev, section.next, date.as_deref());
    let picker = view! {
        <ChronoDatePicker
            base=format!("/texte/{}/section/{}", section.code, section.cid)
            date=Signal::derive({
                let date = date.clone();
                move || date.clone()
            })
        />
    };

    let hide_abroges = RwSignal::new(false);
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

    let items = section
        .items
        .into_iter()
        .map(|item| section_item_view(item, &code, date.as_deref()))
        .collect_view();
    let reading = view! {
        <div
            class="flex max-w-3xl flex-col"
            class:abroges-masques=move || hide_abroges.get()
        >
            {items}
        </div>
    };

    let main = view! {
        <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
            <header class="flex flex-col gap-3">
                <h1 class="font-sans text-2xl text-[var(--color-ink)] sm:text-3xl">
                    {section.title}
                </h1>
                <p class="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-[var(--color-ink-subtle)]">
                    <span>{compte}</span>
                    {toggle}
                </p>
                {picker}
            </header>
            {reading}
            {footer}
        </div>
    };

    view! {
        <Title text=page_title />
        <Meta name="description" content=description />
        <Link rel="canonical" href=url />

        <div class="mx-auto flex w-full max-w-[92rem] flex-1 flex-col gap-6 px-4 py-8 sm:px-6 lg:px-8">
            {breadcrumb}
            // Gabarit commun /decisions · /textes · /decision : gouttière 240px
            // toujours présente (vide sans rail) — le contenu tombe au même x que
            // sur les autres pages.
            <div class="grid items-start gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                <div class="hidden lg:block">{rail}</div>
                {main}
            </div>
        </div>
    }
}

/// Fil d'Ariane : code → divisions englobantes (chacune vers sa vue-lecture).
/// La division courante n'y figure pas — c'est le `<h1>`.
fn breadcrumb(
    code: &str,
    code_title: String,
    ancestors: &[LawSectionRef],
    date: Option<&str>,
) -> AnyView {
    let crumb_cls = "inline-block max-w-[22rem] truncate align-bottom underline-offset-4 hover:text-[var(--color-accent)] hover:underline";
    // La date de consultation suit dans le fil (Chronolégi).
    let q = date.map(|d| format!("?date={d}")).unwrap_or_default();
    let ancestors = ancestors
        .iter()
        .map(|a| {
            let href = format!("/texte/{code}/section/{}{q}", a.cid);
            let label = a.label.clone();
            view! {
                <li class="flex items-center gap-1.5">
                    <span aria-hidden="true">"›"</span>
                    <A href=href attr:class=crumb_cls>
                        {label}
                    </A>
                </li>
            }
        })
        .collect_view();
    view! {
        <nav aria-label="Fil d'Ariane" class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
            <ol class="flex flex-wrap items-center gap-x-1.5 gap-y-1">
                <li>
                    <A href=format!("/texte/{code}{q}") attr:class=crumb_cls>
                        {code_title}
                    </A>
                </li>
                {ancestors}
            </ol>
        </nav>
    }
    .into_any()
}

/// Rail sticky « Dans cette division » : les sous-divisions de premier niveau,
/// ancrées `#{cid}`. Masqué s'il n'y en a pas au moins deux.
fn division_rail(items: &[LawSectionItem]) -> Option<AnyView> {
    let entries: Vec<AnyView> = items
        .iter()
        .filter(|i| i.kind == "section" && i.depth == 1)
        .map(|i| {
            let href = format!("#{}", i.cid.clone().unwrap_or_default());
            let body = view! {
                <a
                    href=href
                    class="block text-sm leading-snug text-[var(--color-accent)] no-underline hover:underline"
                >
                    {i.label.clone()}
                </a>
            }
            .into_any();
            rail_item(false, body)
        })
        .collect();
    if entries.len() < 2 {
        return None;
    }
    Some(
        view! {
            <nav
                aria-label="Dans cette division"
                class="hidden lg:sticky lg:top-20 lg:block lg:self-start"
            >
                {rail_block("Dans cette division", entries)}
            </nav>
        }
        .into_any(),
    )
}

/// Navigation bloc précédent / bloc suivant en pied de lecture — l'ordre de
/// lecture du texte, hors sous-arbre de la division courante.
fn nav_footer(
    code: &str,
    prev: Option<LawSectionRef>,
    next: Option<LawSectionRef>,
    date: Option<&str>,
) -> AnyView {
    if prev.is_none() && next.is_none() {
        return ().into_any();
    }
    let q = date.map(|d| format!("?date={d}")).unwrap_or_default();
    let card = |target: Option<LawSectionRef>, eyebrow: &'static str, right: bool| {
        let Some(t) = target else {
            return view! { <span class="flex-1" /> }.into_any();
        };
        let href = format!("/texte/{code}/section/{}{q}", t.cid);
        let align = if right {
            "items-end text-right"
        } else {
            "items-start text-left"
        };
        view! {
            <A
                href=href
                attr:class=format!(
                    "group flex flex-1 flex-col gap-1 rounded-md border border-[var(--color-rule)] px-4 py-3 no-underline transition-colors hover:border-[var(--color-ink)] {align}",
                )
            >
                <span class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                    {eyebrow}
                </span>
                <span class="line-clamp-2 text-sm leading-snug text-[var(--color-ink)] group-hover:text-[var(--color-accent)]">
                    {t.label}
                </span>
            </A>
        }
        .into_any()
    };
    view! {
        <nav
            aria-label="Divisions voisines"
            class="mt-4 flex max-w-3xl flex-col gap-3 border-t border-[var(--color-rule)] pt-6 sm:flex-row"
        >
            {card(prev, "← Bloc précédent", false)}
            {card(next, "Bloc suivant →", true)}
        </nav>
    }
    .into_any()
}

/// Rend un item de la vue-lecture : intertitre de sous-division (taille selon
/// la profondeur, ancré `id = cid`) ou article complet (numéro lié à sa page,
/// corps, Nota). Les items abrogés portent `item-abroge` (masquables d'un
/// clic, règle CSS `abroges-masques`). Partagé avec la vue-lecture intégrale
/// des textes courts (`law_code_page`).
pub(crate) fn section_item_view(item: LawSectionItem, code: &str, date: Option<&str>) -> AnyView {
    let abroge = item.etat != "VIGUEUR";
    if item.kind == "section" {
        let cid = item.cid.unwrap_or_default();
        let cls = match item.depth {
            1 => "mt-10 border-t border-[var(--color-rule)] pt-6 font-sans text-xl text-[var(--color-ink)]",
            2 => "mt-8 font-sans text-lg text-[var(--color-ink)]",
            _ => "mt-6 font-sans text-base font-medium text-[var(--color-ink)]",
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
        return view! {
            <h2 id=cid class=cls>
                {item.label}
                {badge}
            </h2>
        }
        .into_any();
    }
    let num_key = item.num_key.unwrap_or_default();
    // Chronolégi : le lien de l'article porte la date de consultation.
    let href = match date {
        Some(d) => format!("/texte/{code}/{num_key}/{d}"),
        None => format!("/texte/{code}/{num_key}"),
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
    let texte = item.texte.map(|t| {
        view! {
            <div class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]">
                {t}
            </div>
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
