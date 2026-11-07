//! Page `/norm/{juris}/{code}/{num}/compare/{de}/{a}` : vis-à-vis de deux rédactions
//! d'un article, une colonne par version, alignées au bloc d'alinéas.
//! `noindex` (combinatoire de paires) + canonique sur la page article.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;
use lj_dtos::{
    LawArticleVersion, LawCompareOp, LawCompareResponse, LawCompareRow, LawCompareRowKind,
    LawCompareSegment,
};

use crate::components::ui::{DropdownSelect, SelectOption};
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
    crate::pages::cache_public_5_min();
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
    if err.status == 404 {
        crate::pages::not_found::signaler_lien_mort();
    }
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
        <div class="mx-auto flex w-full max-w-5xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/3" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-10 w-full" />
            <div class="mt-4 grid gap-4 sm:grid-cols-2">
                <Skeleton class="h-40 w-full" />
                <Skeleton class="h-40 w-full" />
            </div>
        </div>
    }
}

/// Segments d'une colonne : `Equal` nu, `Delete`/`Insert` marqués.
fn column(segments: Vec<LawCompareSegment>) -> impl IntoView {
    segments
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
        .collect_view()
}

/// Une cellule du vis-à-vis. Vide, elle nomme l'absence : du blanc se lirait
/// comme un défaut de mise en page.
fn cell(
    segments: Vec<LawCompareSegment>,
    side_label: &'static str,
    absent: &'static str,
) -> impl IntoView {
    let empty = segments.is_empty();
    view! {
        <div class="min-w-0 px-4 py-3">
            <p class="mb-1 text-[10px] uppercase tracking-[0.16em] text-[var(--color-ink-subtle)] sm:hidden">
                {side_label}
            </p>
            {if empty {
                Either::Left(
                    view! {
                        <p class="text-sm italic text-[var(--color-ink-subtle)]">{absent}</p>
                    },
                )
            } else {
                Either::Right(
                    view! {
                        <div class="whitespace-pre-line text-[15px] leading-relaxed text-[var(--color-ink)]">
                            {column(segments)}
                        </div>
                    },
                )
            }}
        </div>
    }
}

/// Une ligne alignée : deux cellules côte à côte au-delà de `sm`, empilées en
/// dessous. Une ligne modifiée porte une teinte, son numéro et son ancre.
fn compare_row(row: LawCompareRow, change_no: Option<usize>) -> impl IntoView {
    let tint = match row.kind {
        LawCompareRowKind::Equal => "",
        _ => "bg-[var(--color-vellum)]/60",
    };
    let anchor = change_no.map(|n| format!("m{n}"));
    let marker = change_no.map(|n| {
        view! {
            <p class="px-4 pt-3 text-[10px] uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                {format!("Modification {n}")}
            </p>
        }
    });
    view! {
        <div
            id=anchor
            class=format!(
                "scroll-mt-24 border-t border-[var(--color-rule)]/60 first:border-t-0 {tint}",
            )
        >
            {marker}
            <div class="grid sm:grid-cols-2 sm:divide-x sm:divide-[var(--color-rule)]/60">
                {cell(row.left, "Version de référence", "Alinéa absent de cette version")}
                {cell(row.right, "Version comparée", "Alinéa supprimé")}
            </div>
        </div>
    }
}

#[component]
fn CompareLoaded(cmp: LawCompareResponse) -> impl IntoView {
    let LawCompareResponse {
        code,
        jurisdiction,
        code_title,
        num,
        num_key,
        from,
        to,
        rows,
        versions,
    } = cmp;
    let code_label = code_title.unwrap_or_else(|| code.clone());
    let num_label = format_article_num(&num);
    let page_title = format!("Versions de l'article {num_label} du {code_label} - LibreJustice");
    let text = lj_vocab::norm_path(&jurisdiction, &code);
    let article_href = format!("{text}/{num_key}");
    let canonical = format!("https://librejustice.fr{text}/{num_key}");

    let from_label = version_label(&from);
    let to_label = version_label(&to);
    let changes = rows
        .iter()
        .filter(|r| r.kind != LawCompareRowKind::Equal)
        .count();

    // Sélecteurs : navigation vers la nouvelle paire au change. Chaque select
    // garde l'autre borne de l'URL courante.
    let from_key = version_url_key(&from);
    let to_key = version_url_key(&to);
    let base = format!("{text}/{num_key}/compare");

    let options: Vec<SelectOption> = versions
        .iter()
        .map(|v| SelectOption {
            value: version_url_key(v),
            label: version_label(v),
        })
        .collect();
    let selector = |label: &'static str, selected: String, other: String, other_is_to: bool| {
        let navigate = use_navigate();
        let base = base.clone();
        let on_change = Callback::new(move |picked: String| {
            let href = if other_is_to {
                format!("{base}/{picked}/{other}")
            } else {
                format!("{base}/{other}/{picked}")
            };
            navigate(&href, Default::default());
        });
        view! {
            <div class="flex min-w-0 flex-1 flex-col gap-1">
                <span class="text-xs uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                    {label}
                </span>
                <DropdownSelect
                    value=selected
                    on_change=on_change
                    options=options.clone()
                    aria_label=label
                    class="w-full"
                    field=true
                />
            </div>
        }
    };

    let mut change_no = 0usize;
    let body = rows
        .into_iter()
        .map(|row| {
            let no = if row.kind == LawCompareRowKind::Equal {
                None
            } else {
                change_no += 1;
                Some(change_no)
            };
            compare_row(row, no)
        })
        .collect_view();

    let verdict = if changes == 0 {
        Either::Left(view! {
            <p class="text-sm text-[var(--color-ink-muted)]">
                "Les deux rédactions sont identiques, mot pour mot."
            </p>
        })
    } else {
        let plural = if changes > 1 { "s" } else { "" };
        Either::Right(view! {
            <p class="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-[var(--color-ink-muted)]">
                <span>{format!("{changes} modification{plural}")}</span>
                <a
                    href="#m1"
                    class="text-xs underline underline-offset-4 hover:text-[var(--color-accent)]"
                >
                    "Aller à la première"
                </a>
            </p>
        })
    };

    view! {
        <Title text=page_title />
        <Meta name="robots" content="noindex" />
        <Link rel="canonical" href=canonical />

        <div class="mx-auto flex w-full max-w-5xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
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

            {verdict}

            <div class="overflow-hidden rounded-lg border border-[var(--color-rule)]">
                <div class="hidden grid-cols-2 divide-x divide-[var(--color-rule)]/60 border-b border-[var(--color-rule)] bg-[var(--color-vellum)]/40 sm:grid">
                    <p class="px-4 py-2 text-xs text-[var(--color-ink-muted)]">
                        <span class="uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                            "Référence"
                        </span>
                        " · "
                        {from_label}
                    </p>
                    <p class="px-4 py-2 text-xs text-[var(--color-ink-muted)]">
                        <span class="uppercase tracking-[0.16em] text-[var(--color-ink-subtle)]">
                            "Comparée"
                        </span>
                        " · "
                        {to_label}
                    </p>
                </div>
                {body}
            </div>

            <p class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[var(--color-ink-subtle)]">
                <span>
                    <del class="rounded-sm bg-rose-100/70 px-1 text-rose-900 decoration-rose-400/70">
                        "retiré"
                    </del>
                    " de la version de référence"
                </span>
                <span>
                    <ins class="rounded-sm bg-emerald-100/80 px-1 text-emerald-900 no-underline">
                        "ajouté"
                    </ins>
                    " dans la version comparée"
                </span>
            </p>
        </div>
    }
}
