//! Page `/loi/{code}` (LawCodePage, SEO-critique). Sommaire d'un code LEGI :
//! en-tête (titre, nature, dernière modification), nombre d'articles,
//! lien Légifrance. Calquée sur [`crate::pages::law_page`] : sommaire bloquant
//! SSR (SEO dans le document initial), JSON-LD `Legislation`.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use lj_dtos::{LawCodeSummary, TocEntry};

use crate::helpers::{format_article_num, format_iso_date};
use crate::pages::law_page::data::{
    code_param, fetch_code_summary, fetch_toc, sendable, PageError, TocResult,
};
use crate::seo::law::code_canonical_url;
use crate::seo::OG_IMAGE;

#[component]
pub fn LawCodePage() -> impl IntoView {
    let code = code_param();
    let summary = Resource::new_blocking(
        move || code.get(),
        |code| sendable(fetch_code_summary(code)),
    );
    // Table des matières non bloquante (streamée via `<Suspense>`) : hors du chemin
    // critique du rendu SEO, qui n'attend que le sommaire.
    let toc = Resource::new(move || code.get(), |code| sendable(fetch_toc(code)));

    view! {
        <Suspense fallback=CodeSkeleton>
            {move || Suspend::new(async move {
                match summary.await {
                    Ok(summary) => {
                        set_cache_control(200);
                        Either::Left(view! { <LawCodeLoaded summary=summary toc=toc /> })
                    }
                    Err(err) => {
                        set_cache_control(err.status);
                        Either::Right(view! { <CodeError err=err /> })
                    }
                }
            })}
        </Suspense>
    }
}

#[cfg(feature = "ssr")]
fn set_cache_control(status: u16) {
    use axum::http::{header::CACHE_CONTROL, HeaderValue};
    let value = match status {
        200 => "public, max-age=0, s-maxage=604800, stale-while-revalidate=86400",
        404 | 400 | 422 => "public, max-age=0, s-maxage=300",
        _ => "no-store",
    };
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        if let Ok(hv) = HeaderValue::from_str(value) {
            resp.insert_header(CACHE_CONTROL, hv);
        }
    }
}

#[cfg(not(feature = "ssr"))]
fn set_cache_control(_status: u16) {}

#[component]
fn CodeError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    let title = "Code introuvable — LibreJustice";
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
fn CodeSkeleton() -> impl IntoView {
    use crate::components::ui::Skeleton;
    view! {
        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-12 sm:px-6 lg:px-8">
            <Skeleton class="h-4 w-1/4" />
            <Skeleton class="h-9 w-2/3" />
            <Skeleton class="h-4 w-1/3" />
        </div>
    }
}

#[component]
fn LawCodeLoaded(summary: LawCodeSummary, toc: Resource<TocResult>) -> impl IntoView {
    let title = summary.titre.clone();
    let code = summary.code.clone();
    let description = format!(
        "{} — {} articles. Référentiel consolidé, versions à date.",
        summary.titre, summary.article_count
    );
    let url = code_canonical_url(&summary.code);
    let page_title = format!("{title} — LibreJustice");

    let json = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "Legislation",
        "name": summary.titre,
        "url": url,
        "inLanguage": "fr",
        "legislationType": summary.nature,
        "jurisdiction": "FR",
        "legislationIdentifier": summary.legitext,
    });
    let jsonld = serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string());

    let modified = summary
        .derniere_modification
        .as_deref()
        .map(|d| format_iso_date(Some(d)))
        .map(|d| {
            view! {
                <p class="text-sm text-[var(--color-ink-muted)]">
                    {format!("Dernière modification le {d}")}
                </p>
            }
        });

    view! {
        <Title text=page_title />
        <Meta name="description" content=description.clone() />
        <Meta property="og:type" content="website" />
        <Meta property="og:site_name" content="LibreJustice" />
        <Meta property="og:title" content=title.clone() />
        <Meta property="og:description" content=description.clone() />
        <Meta property="og:url" content=url.clone() />
        <Meta property="og:locale" content="fr_FR" />
        <Meta property="og:image" content=OG_IMAGE />
        <Link rel="canonical" href=url />
        <leptos_meta::Script type_="application/ld+json">{jsonld}</leptos_meta::Script>

        <div class="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-8 px-4 py-12 sm:px-6 lg:px-8">
            <header class="flex flex-col gap-2">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    {summary.nature}
                </p>
                <h1 class="font-sans text-3xl text-[var(--color-ink)]">{title}</h1>
                {modified}
                <p class="flex flex-wrap items-center gap-x-2 text-sm text-[var(--color-ink-subtle)]">
                    <span>{format!("{} articles", summary.article_count)}</span>
                    <span aria-hidden="true">"·"</span>
                    <A
                        href="/sources#dila"
                        attr:class="underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                    >
                        "Base LEGI · DILA"
                    </A>
                    <span aria-hidden="true">"·"</span>
                    <span class="font-mono text-xs">{summary.legitext}</span>
                </p>
            </header>

            <CodeTocSection toc=toc code=code />
        </div>
    }
}

/// Table des matières du code (streamée). Erreur ⇒ message discret ; vide ⇒
/// section masquée. L'arbre est reconstruit depuis `title_path` (segments séparés
/// par « > ») : divisions repliables (`<details>`), articles en feuilles.
#[component]
fn CodeTocSection(toc: Resource<TocResult>, code: String) -> impl IntoView {
    let code = StoredValue::new(code);
    view! {
        <Suspense fallback=move || {
            view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">"Chargement du sommaire…"</p>
            }
        }>
            {move || Suspend::new(async move {
                let resolved = toc.await;
                toc_view(resolved, code.get_value())
            })}
        </Suspense>
    }
}

fn toc_view(resolved: TocResult, code: String) -> AnyView {
    if let Some(err) = resolved.error {
        return view! {
            <p class="text-sm text-[var(--color-ink-subtle)]">
                {format!("Sommaire indisponible ({err}).")}
            </p>
        }
        .into_any();
    }
    if resolved.entries.is_empty() {
        return ().into_any();
    }
    let tree = build_toc_tree(resolved.entries);
    view! {
        <section aria-label="Table des matières" class="flex flex-col gap-3">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Table des matières"</h2>
            <div class="flex flex-col gap-1">{render_nodes(&tree, &code)}</div>
        </section>
    }
    .into_any()
}

/// Nœud de l'arbre de sommaire : une division (titre + enfants) ou un article.
#[derive(Debug, PartialEq, Eq)]
enum TocNode {
    Division {
        title: String,
        children: Vec<TocNode>,
    },
    Article(TocEntry),
}

/// Reconstruit l'arbre depuis les `title_path` (« A > B > C »). Les articles sans
/// chemin sont rattachés à la racine, dans l'ordre d'arrivée (qui suit l'ordre de
/// position du code).
fn build_toc_tree(entries: Vec<TocEntry>) -> Vec<TocNode> {
    let mut roots: Vec<TocNode> = Vec::new();
    for entry in entries {
        let segments: Vec<String> = entry
            .title_path
            .as_deref()
            .map(|p| {
                p.split(" > ")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        insert_entry(&mut roots, &segments, entry);
    }
    roots
}

/// Insère un article sous le chemin de divisions `segments`, créant les divisions
/// manquantes. Chemin vide ⇒ article en racine. Une division existante de même
/// titre est réutilisée où qu'elle soit parmi ses sœurs (pas seulement la dernière
/// insérée) : les articles d'une même division n'arrivent pas forcément groupés
/// (`position` NULL ⇒ tri `num_key` qui entrelace les divisions), sinon l'arbre se
/// fragmenterait en doublons.
fn insert_entry(level: &mut Vec<TocNode>, segments: &[String], entry: TocEntry) {
    let Some((head, rest)) = segments.split_first() else {
        level.push(TocNode::Article(entry));
        return;
    };
    if let Some(TocNode::Division { children, .. }) = level
        .iter_mut()
        .find(|n| matches!(n, TocNode::Division { title, .. } if title == head))
    {
        insert_entry(children, rest, entry);
        return;
    }
    let mut children = Vec::new();
    insert_entry(&mut children, rest, entry);
    level.push(TocNode::Division {
        title: head.clone(),
        children,
    });
}

/// Rend une liste de nœuds : divisions repliables, articles en feuilles. Les
/// articles consécutifs d'une même division coulent en ligne (flux
/// `flex-wrap`) — une entrée par ligne noierait les 1 500 articles d'un code.
fn render_nodes(nodes: &[TocNode], code: &str) -> AnyView {
    let mut views: Vec<AnyView> = Vec::new();
    let mut run: Vec<&TocEntry> = Vec::new();
    for node in nodes {
        match node {
            TocNode::Article(entry) => run.push(entry),
            TocNode::Division { title, children } => {
                if !run.is_empty() {
                    views.push(article_flow(std::mem::take(&mut run), code));
                }
                let inner = render_nodes(children, code);
                views.push(
                    view! {
                        <details class="border-l border-[var(--color-rule)] pl-4">
                            <summary class="cursor-pointer py-1.5 text-sm font-medium text-[var(--color-ink)] transition-colors marker:text-[var(--color-ink-subtle)] hover:text-[var(--color-accent)]">
                                {title.clone()}
                            </summary>
                            <div class="mt-1 flex flex-col gap-1 pb-2">{inner}</div>
                        </details>
                    }
                    .into_any(),
                );
            }
        }
    }
    if !run.is_empty() {
        views.push(article_flow(run, code));
    }
    view! { {views} }.into_any()
}

/// Flux d'articles d'une division : liens en ligne, séparés par l'espace.
fn article_flow(entries: Vec<&TocEntry>, code: &str) -> AnyView {
    let links = entries
        .into_iter()
        .map(|entry| {
            // Lien sur la clé canonique (`numKey`), résolue en lookup exact
            // (ADR 0123 §2).
            let href = format!("/loi/{code}/{}", entry.num_key);
            let label = format_article_num(&entry.num);
            let abrogated = entry.status != "VIGUEUR";
            let cls = if abrogated {
                "text-sm leading-relaxed text-[var(--color-ink-subtle)] line-through underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            } else {
                "text-sm leading-relaxed text-[var(--color-ink-muted)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
            };
            view! {
                <A href=href attr:class=cls>
                    {label}
                </A>
            }
        })
        .collect_view();
    view! { <div class="flex flex-wrap gap-x-4 gap-y-1 py-1">{links}</div> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(num: &str, path: Option<&str>) -> TocEntry {
        TocEntry {
            num: num.to_string(),
            num_key: num.to_string(),
            title_path: path.map(str::to_string),
            status: "VIGUEUR".to_string(),
        }
    }

    fn div(title: &str, children: Vec<TocNode>) -> TocNode {
        TocNode::Division {
            title: title.to_string(),
            children,
        }
    }

    /// Articles d'une même division arrivant **non contigus** (cas `position` NULL,
    /// tri `num_key` lexical entrelaçant les divisions) : l'arbre doit les regrouper
    /// sous une division unique, pas fragmenter en doublons.
    #[test]
    fn groups_non_contiguous_divisions() {
        let entries = vec![
            art("1", Some("Livre I > Titre I")),
            art("100", Some("Livre II")),
            art("2", Some("Livre I > Titre I")),
            art("10", Some("Livre I > Titre II")),
        ];
        let tree = build_toc_tree(entries);
        assert_eq!(
            tree,
            vec![
                div(
                    "Livre I",
                    vec![
                        div(
                            "Titre I",
                            vec![
                                TocNode::Article(art("1", Some("Livre I > Titre I"))),
                                TocNode::Article(art("2", Some("Livre I > Titre I")))
                            ]
                        ),
                        div(
                            "Titre II",
                            vec![TocNode::Article(art("10", Some("Livre I > Titre II")))]
                        ),
                    ],
                ),
                div(
                    "Livre II",
                    vec![TocNode::Article(art("100", Some("Livre II")))]
                ),
            ]
        );
    }

    /// Articles sans chemin (cas `title_path` NULL, codes étrangers ordonnés par
    /// `position`) restent à la racine, dans l'ordre d'arrivée.
    #[test]
    fn pathless_articles_stay_at_root() {
        let entries = vec![art("1", None), art("2", None)];
        let tree = build_toc_tree(entries);
        assert_eq!(
            tree,
            vec![
                TocNode::Article(art("1", None)),
                TocNode::Article(art("2", None)),
            ]
        );
    }
}
