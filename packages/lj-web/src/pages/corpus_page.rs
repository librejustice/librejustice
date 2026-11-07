//! `/norm-index/{juris}/in-force` and `/norm-index/{juris}/in-force/{date}`: what applies in a place at a date, grouped
//! by official journal then by nature. The place is a setting (`?place=`), as
//! on the article page; without one, the metropole. Every group announces its
//! whole count and shows only its first titles, the metropolitan fund not
//! fitting in one page.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::PlaceCorpusResponse;

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::components::ui::DatePicker;
use crate::helpers::{format_iso_date, today_iso};
use crate::pages::juridictions_page::{fmt_count, set_cache_control, HubError, HubSkeleton};
use crate::pages::law_page::data::{fetch_places, sendable, PageError};
use crate::pages::law_page::{place_groups, PlacePicker, NORM_FIRST_YEAR, NORM_YEARS_AHEAD};

#[component]
pub fn CorpusPage() -> impl IntoView {
    crate::pages::cache_public_5_min();
    own_page_description();

    let params = use_params_map();
    let query = use_query_map();
    let date = Signal::derive(move || params.read().get("date"));
    let place = Signal::derive(move || query.read().get("place"));
    let corpus = Resource::new_blocking(
        move || (place.get(), date.get()),
        |(place, date)| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_place_corpus(place.as_deref(), date.as_deref())
                    .await
                    .map_err(PageError::from)
            })
        },
    );

    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match corpus.await {
                    Ok(r) => {
                        set_cache_control(200);
                        Either::Left(view! { <CorpusLoaded response=r date=date /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <HubError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

/// « Lu depuis » over the whole vocabulary, which streams in after the corpus.
#[component]
fn CorpusPlacePicker(#[prop(into)] date: Signal<Option<String>>) -> impl IntoView {
    let places = Resource::new(|| (), |_| sendable(fetch_places()));
    view! {
        <Suspense fallback=|| ()>
            {move || Suspend::new(async move {
                let groups = place_groups(&places.await);
                view! { <PlacePicker base="/norm-index/fr/in-force".to_string() groups=groups date=date /> }
            })}
        </Suspense>
    }
}

#[component]
fn CorpusLoaded(
    response: PlaceCorpusResponse,
    #[prop(into)] date: Signal<Option<String>>,
) -> impl IntoView {
    let place_label = response.place.label.clone();
    let title = format!("Ce qui s'applique en {place_label} - LibreJustice");
    let le_jour = format_iso_date(Some(&response.date));
    let description = format!(
        "Les textes applicables en {place_label} au {le_jour}, par journal officiel puis par nature."
    );
    let count = format!(
        "{} texte{}",
        fmt_count(response.total),
        if response.total > 1 { "s" } else { "" },
    );
    let empty = response.journals.is_empty();

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        // The place and the date are a reader setting, not another document:
        // every variant canonicalises to the default page, as a dated article
        // does.
        <Link rel="canonical" href="https://librejustice.fr/norm-index/fr/in-force" />

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Applicabilité"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {format!("Ce qui s'applique en {place_label}")}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {format!("Au {le_jour}, {count} s'y appliquent. ")}
                    "Un texte compte dès qu'un de ses articles s'applique. Un groupe trop long n'affiche que quelques titres."
                </p>
                <div class="flex flex-wrap items-end gap-4">
                    <CorpusPlacePicker date=date />
                    <CorpusDate date=date place=response.place.uid.clone() />
                </div>
            </header>

            {if empty {
                Either::Left(
                    view! {
                        <p class="text-[var(--color-ink-muted)]">
                            "Aucun texte compilé ne nomme ce lieu à cette date."
                        </p>
                    },
                )
            } else {
                Either::Right(
                    response
                        .journals
                        .into_iter()
                        .map(|journal| {
                            let heading = format!("{} · {}", journal.label, fmt_count(journal.total));
                            let groups = journal
                                .groups
                                .into_iter()
                                .map(|group| {
                                    let shown = group.texts.len() as i64;
                                    let rest = group.total - shown;
                                    let items = group
                                        .texts
                                        .into_iter()
                                        .map(|t| {
                                            view! {
                                                <li class="py-1.5">
                                                    <A
                                                        href=lj_vocab::norm_path(&t.jurisdiction, &t.slug)
                                                        attr:class="text-sm text-[var(--color-ink)] hover:text-[var(--color-accent)]"
                                                    >
                                                        {t.title}
                                                    </A>
                                                </li>
                                            }
                                        })
                                        .collect_view();
                                    view! {
                                        <div class="flex flex-col gap-1">
                                            <h3 class="flex items-baseline justify-between gap-3 text-sm font-medium text-[var(--color-ink)]">
                                                <span>{group.label}</span>
                                                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                                                    {fmt_count(group.total)}
                                                </span>
                                            </h3>
                                            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                                                {items}
                                            </ul>
                                            {(rest > 0)
                                                .then(|| {
                                                    view! {
                                                        <p class="text-xs text-[var(--color-ink-subtle)]">
                                                            {format!("et {} autres", fmt_count(rest))}
                                                        </p>
                                                    }
                                                })}
                                        </div>
                                    }
                                })
                                .collect_view();
                            view! {
                                <section class="flex flex-col gap-4">
                                    <h2 class="border-b border-[var(--color-rule)] pb-1 font-sans text-lg text-[var(--color-ink)]">
                                        {heading}
                                    </h2>
                                    {groups}
                                </section>
                            }
                        })
                        .collect_view(),
                )
            }}
        </div>
    }
}

/// The day the corpus is read on; changing it navigates, the place riding along.
#[component]
fn CorpusDate(#[prop(into)] date: Signal<Option<String>>, place: String) -> impl IntoView {
    use leptos_router::hooks::use_navigate;
    let suffix = if place == lj_dtos::DEFAULT_READER_PLACE {
        String::new()
    } else {
        format!("?place={place}")
    };
    let navigate = use_navigate();
    let on_change = Callback::new(move |picked: String| {
        navigate(
            &format!("/norm-index/fr/in-force/{picked}{suffix}"),
            Default::default(),
        );
    });
    view! {
        <div class="flex flex-col gap-1">
            <span class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Au"
            </span>
            <DatePicker
                value=Signal::derive(move || date.get().unwrap_or_else(today_iso))
                on_change=on_change
                aria_label="Au"
                first_year=NORM_FIRST_YEAR
                years_ahead=NORM_YEARS_AHEAD
            />
        </div>
    }
}
