//! `/norm-index/{juris}/in-force` and `/norm-index/{juris}/in-force/{date}`: what applies in a place at a date, grouped
//! by official journal then by nature. The place is a setting (`?place=`), as
//! on the article page; without one, the metropole. Every group announces its
//! whole count and shows only its first titles, the metropolitan fund not
//! fitting in one page.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};
use lj_dtos::PlaceCorpusResponse;

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::components::ui::DatePicker;
use crate::helpers::group_thousands;
use crate::helpers::{format_iso_date, today_iso};
use crate::pages::juridictions_page::{set_cache_control, HubError, HubSkeleton};
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
    let lang = crate::i18n::use_reader_lang().get();
    let places = Resource::new(|| (), move |_| sendable(fetch_places(lang)));
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
    let numbers = &crate::i18n::use_ui().numbers;
    let dates = &crate::i18n::use_ui().dates;
    let ui = &crate::i18n::use_ui().hubs;
    let norm = &crate::i18n::use_ui().norm;
    let lang = crate::i18n::use_reader_lang().get();
    let place_label = crate::i18n::place_name(lang, &response.place.uid, &response.place.label);
    let place = (crate::i18n::use_ui().shell.located)(&response.place.uid, &place_label);
    let title = (ui.corpus_title)(&place);
    let le_jour = format_iso_date(dates, Some(&response.date));
    let description = (ui.corpus_description)(&place, &le_jour);
    let count = (ui.n_texts)(&group_thousands(numbers, response.total), response.total);
    let empty = response.journals.is_empty();

    view! {
        <Title text=title />
        <Meta name="description" content=description />
        // The place and the date are a reader setting, not another document:
        // every variant canonicalises to the default page, as a dated article
        // does.
        <crate::seo::Canonical href="https://librejustice.fr/norm-index/fr/in-force" />

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {ui.applicability}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {(ui.corpus_heading)(&place)}
                </h1>
                <p class="max-w-prose text-[var(--color-ink-muted)]">
                    {(ui.corpus_intro)(&le_jour, &count)}
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
                            {ui.corpus_empty}
                        </p>
                    },
                )
            } else {
                Either::Right(
                    response
                        .journals
                        .into_iter()
                        .map(|journal| {
                            let heading = format!("{} · {}", (norm.journal_name)(&journal.label), group_thousands(numbers, journal.total));
                            let groups = journal
                                .groups
                                .into_iter()
                                .map(|group| {
                                    let shown = group.texts.len() as i64;
                                    let rest = group.total - shown;
                                    let label = (crate::i18n::use_ui().referential.nature)(
                                        &group.nature.to_ascii_uppercase(),
                                    )
                                    .map_or(group.label, str::to_string);
                                    let items = group
                                        .texts
                                        .into_iter()
                                        .map(|mut t| {
                                            crate::i18n::relabel_title(lang, &mut t.title);
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
                                                <span>{label}</span>
                                                <span class="tabular-nums text-xs text-[var(--color-ink-subtle)]">
                                                    {group_thousands(numbers, group.total)}
                                                </span>
                                            </h3>
                                            <ul class="flex flex-col divide-y divide-[var(--color-rule)]">
                                                {items}
                                            </ul>
                                            {(rest > 0)
                                                .then(|| {
                                                    view! {
                                                        <p class="text-xs text-[var(--color-ink-subtle)]">
                                                            {(ui.and_more)(&group_thousands(numbers, rest))}
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
    let ui = &crate::i18n::use_ui().hubs;
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
                {ui.on}
            </span>
            <DatePicker
                value=Signal::derive(move || date.get().unwrap_or_else(today_iso))
                on_change=on_change
                aria_label=ui.on
                first_year=NORM_FIRST_YEAR
                years_ahead=NORM_YEARS_AHEAD
            />
        </div>
    }
}
