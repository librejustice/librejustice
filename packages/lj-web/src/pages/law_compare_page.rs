//! Page `/texte/{code}/{num}/comparer/{de}/{a}` — comparateur de versions d'un
//! article (ADR 0193). Diff calculé côté serveur (`LawCompareResponse`), rendu
//! **inline** : suppressions barrées, insertions surlignées, une colonne de
//! lecture. Deux sélecteurs de versions naviguent vers la nouvelle URL.
//! `noindex` (combinatoire de paires) + canonique sur la page article.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;
use lj_dtos::{LawArticleVersion, LawCompareOp, LawCompareResponse};

use crate::helpers::{format_article_num, format_iso_date};
use crate::pages::law_page::data::{compare_key, fetch_compare, sendable, PageError};
use crate::pages::law_page::set_cache_control;

/// Clé d'URL d'une version pour le comparateur : sa `date_debut`, ou
/// `initiale` pour la borne ouverte (sentinelle absorbée côté API).
pub(crate) fn version_url_key(version: &LawArticleVersion) -> String {
    if version.date_debut.is_empty() {
        "initiale".to_string()
    } else {
        version.date_debut.clone()
    }
}

/// Libellé humain d'une version (même forme que la frise de la page article).
fn version_label(version: &LawArticleVersion) -> String {
    match (version.date_debut.is_empty(), version.date_fin.as_deref()) {
        (true, _) => "Version initiale".to_string(),
        (false, Some(fin)) => format!(
            "{} – {}",
            format_iso_date(Some(&version.date_debut)),
            format_iso_date(Some(fin))
        ),
        (false, None) => format!("depuis le {}", format_iso_date(Some(&version.date_debut))),
    }
}

#[component]
pub fn LawComparePage() -> impl IntoView {
    let key = compare_key();
    let cmp = Resource::new_blocking(move || key.get(), |key| sendable(fetch_compare(key)));

    view! {
        <Suspense fallback=CompareSkeleton>
            {move || Suspend::new(async move {
                match cmp.await {
                    Ok(cmp) => {
                        set_cache_control(200);
                        Either::Left(view! { <CompareLoaded cmp=cmp /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <CompareError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[component]
fn CompareError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    view! {
        <Title text="Comparaison introuvable - LibreJustice" />
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
fn CompareSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-10 w-full" />
            <div class="mt-4 flex flex-col gap-3">
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-full" />
                <Skeleton class="h-4 w-2/3" />
            </div>
        </div>
    }
}

#[component]
fn CompareLoaded(cmp: LawCompareResponse) -> impl IntoView {
    let LawCompareResponse {
        code,
        code_title,
        num,
        num_key,
        from,
        to,
        segments,
        versions,
    } = cmp;
    let code_label = code_title.unwrap_or_else(|| code.clone());
    let num_label = format_article_num(&num);
    let page_title = format!("Versions de l'article {num_label} du {code_label} - LibreJustice");
    let article_href = format!("/texte/{code}/{num_key}");
    let canonical = format!("https://librejustice.fr/texte/{code}/{num_key}");

    // Sélecteurs : navigation vers la nouvelle paire au change. Chaque select
    // garde l'autre borne de l'URL courante.
    let from_key = version_url_key(&from);
    let to_key = version_url_key(&to);
    let base = format!("/texte/{code}/{num_key}/comparer");

    let selector = |label: &'static str, selected: String, other: String, other_is_to: bool| {
        let navigate = use_navigate();
        let base = base.clone();
        let options = versions
            .iter()
            .map(|v| {
                let key = version_url_key(v);
                let is_selected = key == selected;
                view! {
                    <option value=key selected=is_selected>
                        {version_label(v)}
                    </option>
                }
            })
            .collect_view();
        view! {
            <label class="flex min-w-0 flex-1 flex-col gap-1">
                <span class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    {label}
                </span>
                <select
                    class="h-10 w-full rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2 text-sm text-[var(--color-ink)]"
                    on:change=move |ev| {
                        let picked = event_target_value(&ev);
                        let href = if other_is_to {
                            format!("{base}/{picked}/{other}")
                        } else {
                            format!("{base}/{other}/{picked}")
                        };
                        navigate(&href, Default::default());
                    }
                >
                    {options}
                </select>
            </label>
        }
    };

    let diff_view = segments
        .into_iter()
        .map(|s| match s.op {
            LawCompareOp::Equal => view! { <span>{s.text}</span> }.into_any(),
            LawCompareOp::Delete => view! {
                <del class="rounded-sm bg-rose-100/70 px-0.5 text-rose-900 decoration-rose-400/70">
                    {s.text}
                </del>
            }
            .into_any(),
            LawCompareOp::Insert => view! {
                <ins class="rounded-sm bg-emerald-100/80 px-0.5 text-emerald-900 no-underline">
                    {s.text}
                </ins>
            }
            .into_any(),
        })
        .collect_view();

    view! {
        <Title text=page_title />
        <Meta name="robots" content="noindex" />
        <Link rel="canonical" href=canonical />

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <nav class="text-sm text-[var(--color-ink-subtle)]">
                <A
                    href=article_href
                    attr:class="underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    {format!("← Article {num_label} du {code_label}")}
                </A>
            </nav>
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Comparateur de versions"
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">
                    {format!("Article {num_label}")}
                </h1>
                <p class="text-sm text-[var(--color-ink-muted)]">{code_label.clone()}</p>
            </header>

            <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
                {selector("Version de référence", from_key, to_key.clone(), true)}
                <span aria-hidden="true" class="hidden pb-2 text-[var(--color-ink-subtle)] sm:block">
                    "→"
                </span>
                {selector("Version comparée", to_key, version_url_key(&from), false)}
            </div>

            <p class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[var(--color-ink-subtle)]">
                <span>
                    <del class="rounded-sm bg-rose-100/70 px-1 text-rose-900 decoration-rose-400/70">
                        "texte supprimé"
                    </del>
                </span>
                <span>
                    <ins class="rounded-sm bg-emerald-100/80 px-1 text-emerald-900 no-underline">
                        "texte ajouté"
                    </ins>
                </span>
            </p>

            <div class="whitespace-pre-line rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-6 text-[15px] leading-relaxed text-[var(--color-ink)]">
                {diff_view}
            </div>
        </div>
    }
}
