//! `/notice/{juris}/{*uid}`: what a journal printed, one page whatever the journal. A market keeps its own.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::hooks::use_params_map;
use lj_dtos::{NoticeField, PublicationNotice};

use crate::api::ApiClient;
use crate::components::own_page_description;
use crate::components::search::journal_results::numeric_date;
use crate::helpers::format_article_num;
use crate::pages::journals_page::{Fait, MarchePage};
use crate::pages::juridictions_page::{set_cache_control, HubError, HubSkeleton};
use crate::pages::law_code_page::{publication_block, publication_note};
use crate::pages::law_page::data::{sendable, PageError};

const BASE: &str = "https://librejustice.fr";

#[component]
pub fn NoticePage() -> impl IntoView {
    let params = use_params_map();
    move || match params
        .get()
        .get("uid")
        .is_some_and(|u| u.starts_with("marche:"))
    {
        true => Either::Left(view! { <MarchePage /> }),
        false => Either::Right(view! { <PublicationNoticePage /> }),
    }
}

#[component]
fn PublicationNoticePage() -> impl IntoView {
    own_page_description();
    let params = use_params_map();
    let notice = Resource::new_blocking(
        move || params.get().get("uid").unwrap_or_default(),
        |uid| {
            sendable(async move {
                ApiClient::from_context()
                    .fetch_publication_notice(&uid)
                    .await
                    .map_err(PageError::from)
            })
        },
    );
    view! {
        <Suspense fallback=HubSkeleton>
            {move || Suspend::new(async move {
                match notice.await {
                    Ok(n) => {
                        set_cache_control(200);
                        Either::Left(view! { <PublicationLoaded notice=n /> })
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

/// `Article 1`, `Article unique`; an annex keeps its own name.
fn article_label(num: &str, ui: &crate::i18n::NormUi) -> String {
    if num.starts_with("Annexe") {
        num.to_string()
    } else {
        (ui.article_n)(&format_article_num(num))
    }
}

#[component]
fn PublicationLoaded(notice: PublicationNotice) -> impl IntoView {
    let ui = &crate::i18n::use_ui().norm;
    let dates = &crate::i18n::use_ui().dates;
    let journal = notice
        .journal_label
        .clone()
        .unwrap_or_else(|| notice.journal.clone());
    let parution = notice
        .date_publi
        .as_deref()
        .and_then(|d| numeric_date(dates, d));
    let description = match &parution {
        Some(d) => (ui.notice_description_on)(&notice.title, &journal, d),
        None => (ui.notice_description)(&notice.title, &journal),
    };
    let journal = (ui.journal_name)(&journal).to_string();
    let canonical = match notice.norm.as_ref().and_then(|n| n.href.as_deref()) {
        Some(norm) => format!("{BASE}{norm}"),
        None => format!(
            "{BASE}{}",
            lj_vocab::notice_path(&notice.jurisdiction, &notice.uid)
        ),
    };
    let individual = notice.individual;
    let p = notice.publication;
    let issue = p.issue.clone().map(|i| {
        let label = match i.href {
            Some(href) => Either::Left(view! {
                <a href=href class="underline decoration-[var(--color-rule)] underline-offset-4">{i.label}</a>
            }),
            None => Either::Right(view! { <span>{i.label}</span> }),
        };
        view! { " · " {label} }
    });
    let fields = printed_fields(notice.fields);
    let entity = notice.entity.map(|e| {
        view! {
            <div class="flex flex-col gap-0.5">
                <dt class="text-xs uppercase tracking-[0.14em] text-[var(--color-ink-subtle)]">
                    {crate::i18n::use_ui().entity.entity}
                </dt>
                <dd class="text-sm text-[var(--color-ink)]">
                    <a href=e.href.unwrap_or_default() class="underline underline-offset-4">{e.label}</a>
                </dd>
            </div>
        }
    });
    let articles = (!notice.articles.is_empty()).then(|| {
        view! {
            <section class="flex min-w-0 flex-col gap-3">
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    {ui.pub_text}
                </h2>
                {notice
                    .articles
                    .into_iter()
                    .map(|a| view! {
                        <div class="flex flex-col gap-1">
                            {a.num.map(|n| view! {
                                <h3 class="font-sans text-sm text-[var(--color-ink-muted)]">{article_label(&n, ui)}</h3>
                            })}
                            <div class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]">{crate::components::search::compact_search::highlight::plain_text(a.text)}</div>
                        </div>
                    })
                    .collect_view()}
            </section>
        }
    });

    view! {
        <Title text=format!("{} - {journal} - LibreJustice", notice.title) />
        <Meta name="description" content=description />
        {individual.then(|| view! { <Meta name="robots" content="noindex" /> })}
        <crate::seo::Canonical href=canonical />

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-3">
                <p class="text-sm text-[var(--color-ink-muted)]">{journal.clone()} {issue}</p>
                <h1 class="font-sans text-2xl text-[var(--color-ink)]">{notice.title.clone()}</h1>
                {publication_note(p.corrigendum.clone())}
                {publication_note(p.repeal.clone())}
            </header>

            <dl class="grid grid-cols-1 gap-x-8 gap-y-3 sm:grid-cols-2">
                {parution.map(|d| view! { <Fait terme=ui.fact_published valeur=d /> })}
                {notice
                    .date_texte
                    .as_deref()
                    .and_then(|d| numeric_date(dates, d))
                    .map(|d| view! { <Fait terme=ui.fact_text_date valeur=d /> })}
                {notice.nature.map(|n| view! { <Fait terme=ui.fact_nature valeur=n /> })}
                {notice.nor.map(|n| view! { <Fait terme="NOR" valeur=n /> })}
                {notice.issuer.map(|i| view! { <Fait terme=ui.fact_issuer valeur=i /> })}
                {entity}
            </dl>

            {fields}

            {publication_block(ui.pub_notice, p.notice.clone())}
            {publication_block(ui.pub_summary, p.summary.clone())}
            {publication_block(ui.pub_visas, p.visas.clone())}
            {articles}
            {publication_block(ui.pub_text, notice.body)}
            {publication_block(ui.pub_signatories, p.signatories.clone())}
            {publication_block(ui.pub_preparatory, p.preparatory_work.clone())}

            <ul class="flex flex-col gap-2 text-sm">
                {notice.norm.map(|n| view! {
                    <li>
                        {ui.in_force_text}
                        <a href=n.href.unwrap_or_default() class="underline underline-offset-4">{n.label}</a>
                    </li>
                })}
                {notice.source_url.map(|href| view! {
                    <li>
                        <a href=href rel="noopener" class="underline underline-offset-4">{ui.see_official}</a>
                    </li>
                })}
            </ul>
        </div>
    }
}

/// A register's fields under their headings, consecutive fields of one part under one.
fn printed_fields(fields: Vec<NoticeField>) -> Option<impl IntoView> {
    let mut parts: Vec<(Option<String>, Vec<NoticeField>)> = Vec::new();
    for f in fields {
        match parts.last_mut() {
            Some((group, fs)) if *group == f.group => fs.push(f),
            _ => parts.push((f.group.clone(), vec![f])),
        }
    }
    (!parts.is_empty()).then(|| {
        parts
            .into_iter()
            .map(|(group, fields)| {
                view! {
                    <section class="flex min-w-0 flex-col gap-3">
                        {group.map(|g| view! {
                            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">{g}</h2>
                        })}
                        <dl class="grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-[12rem_1fr]">
                            {fields
                                .into_iter()
                                .map(|f| {
                                    let value = match f.href {
                                        Some(href) => Either::Left(view! {
                                            <a href=href class="underline underline-offset-4">{f.value}</a>
                                        }),
                                        None => Either::Right(f.value),
                                    };
                                    view! {
                                        <dt class="text-sm text-[var(--color-ink-muted)]">{f.label}</dt>
                                        <dd class="whitespace-pre-line text-sm text-[var(--color-ink)]">{value}</dd>
                                    }
                                })
                                .collect_view()}
                        </dl>
                    </section>
                }
            })
            .collect_view()
    })
}
