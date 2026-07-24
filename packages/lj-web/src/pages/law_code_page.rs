//! Page `/texte/{code}` (LawCodePage, SEO-critique). Sommaire d'un texte :
//! en-tête (titre, nature, méta), rail « Plan du texte » ancré sur les
//! divisions de premier niveau, puis — selon la taille servie par l'API —
//! vue-lecture intégrale (textes courts : BOFiP, décrets, arrêtés…) ou
//! recherche bornée + table des matières arborescente (chevrons, compteurs
//! d'articles, vue-lecture par division). Calquée sur
//! [`crate::pages::law_page`] : sommaire bloquant SSR (SEO dans le document
//! initial), JSON-LD `Legislation`.

use leptos::either::Either;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;
use lj_dtos::{LawCodeSummary, LawSectionItem, TocEntry, INLINE_READING_MAX};

use crate::helpers::{format_article_num, format_iso_date, group_thousands};
use crate::pages::law_page::data::{
    chrono_date, code_param, fetch_code_summary, fetch_toc, sendable, PageError, TocResult,
};
use crate::pages::law_page::{rail_block, rail_item, ChronoDatePicker};
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
    // critique du rendu SEO, qui n'attend que le sommaire. Datée par `?date=`
    // (Chronolégi, ADR 0193 §5).
    let date = chrono_date();
    let toc = Resource::new(
        move || (code.get(), date.get()),
        |(code, date)| sendable(fetch_toc(code, date)),
    );

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
fn CodeError(err: PageError) -> impl IntoView {
    let eyebrow = if err.status == 404 {
        "Introuvable"
    } else {
        "Erreur"
    };
    let title = "Code introuvable - LibreJustice";
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
    // Même grille rail + contenu que la page chargée : le squelette s'aligne sur
    // la colonne contenu, pas de saut latéral au remplacement.
    view! {
        <div class="mx-auto w-full max-w-[92rem] flex-1 px-4 py-8 sm:px-6 lg:px-8">
            <div class="grid gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                <div class="hidden lg:block"></div>
                <div class="flex w-full min-w-0 max-w-3xl flex-col gap-6">
                    <Skeleton class="h-4 w-1/6" />
                    <Skeleton class="h-10 w-1/2" />
                    <Skeleton class="h-4 w-1/3" />
                    <Skeleton class="mt-4 h-11 w-full max-w-2xl" />
                </div>
            </div>
        </div>
    }
}

#[component]
fn LawCodeLoaded(summary: LawCodeSummary, toc: Resource<TocResult>) -> impl IntoView {
    let title = summary.titre.clone();
    let code = summary.code.clone();
    let has_articles = summary.article_count > 0;
    // Corps monolithique (ADR 0196) : circulaires… — un texte peut porter un
    // corps, des articles, ou les deux (préambule BOFiP).
    let body = summary.body.clone().filter(|b| !b.trim().is_empty());
    let description = if has_articles {
        format!(
            "{} — {} articles. Référentiel consolidé, versions à date.",
            summary.titre, summary.article_count
        )
    } else {
        let excerpt: String = body
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(180)
            .collect();
        format!("{} — {excerpt}", summary.titre)
    };
    let url = code_canonical_url(&summary.code);
    let page_title = format!("{title} - LibreJustice");

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

    // Provenance : seuls les textes LEGI portent l'attribution DILA et leur
    // identifiant LEGITEXT ; les textes curés (étrangers, traduits) renvoient
    // à la page sources générale — leur uid technique n'apprend rien.
    let is_legi = summary.legitext.starts_with("LEGITEXT");
    let (source_href, source_label) = if is_legi {
        ("/sources#dila", "Base LEGI · DILA")
    } else {
        ("/sources", "Données & sources")
    };
    // Bloc méta (articles / dernière modif / source / LEGITEXT), un item par
    // ligne — dans la colonne étroite du rail, des séparateurs « · » se replient
    // en orphelins de bout/début de ligne. Bâti par une closure pour être rendu à
    // deux endroits : en tête de la gouttière gauche sur desktop (au-dessus du
    // « Plan du texte », équilibre la page) et dans l'en-tête sur mobile.
    let article_count = summary.article_count;
    let modif_date = summary
        .derniere_modification
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let signature_date = summary
        .date_texte
        .as_deref()
        .map(|d| format_iso_date(Some(d)));
    let nor = summary.nor.clone();
    let legitext_uid = is_legi.then(|| summary.legitext.clone());
    // Texte publié au JO (uid JORFTEXT) : lien composé vers sa page Légifrance
    // d'origine — version au JO, travaux préparatoires, dossiers législatifs
    // (ADR 0215, zéro ingest).
    let jorf_url = summary.legitext.starts_with("JORFTEXT").then(|| {
        format!(
            "https://www.legifrance.gouv.fr/jorf/id/{}",
            summary.legitext
        )
    });
    let meta_line = move || {
        let count = has_articles.then(
            || view! { <span>{format!("{} articles", group_thousands(article_count))}</span> },
        );
        let modified = modif_date
            .clone()
            .map(|d| view! { <span>{format!("Dernière modification le {d}")}</span> });
        let signed = signature_date
            .clone()
            .map(|d| view! { <span>{format!("Date de signature : {d}")}</span> });
        let nor = nor
            .clone()
            .map(|n| view! { <span class="font-mono text-xs">{format!("NOR : {n}")}</span> });
        let legitext = legitext_uid
            .clone()
            .map(|uid| view! { <span class="font-mono text-xs">{uid}</span> });
        let jorf = jorf_url.clone().map(|url| {
            view! {
                <a
                    href=url
                    rel="external noopener"
                    target="_blank"
                    class="self-start underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    "Texte au JO — Légifrance"
                </a>
            }
        });
        view! {
            <p class="flex flex-col gap-1 text-sm text-[var(--color-ink-subtle)]">
                {count}
                {signed}
                {modified}
                <A
                    href=source_href
                    attr:class="self-start underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                >
                    {source_label}
                </A>
                {jorf}
                {nor}
                {legitext}
            </p>
        }
    };
    // Bandeau temporel (ADR 0178) : prochaine version programmée du texte.
    let upcoming = summary
        .upcoming_versions
        .first()
        .map(|d| crate::pages::law_page::upcoming_banner("Ce texte sera modifié", d));
    // État de diffusion (ADR 0196) : les circulaires abrogées restent lisibles,
    // le bandeau signale l'abrogation.
    let abroge = (summary.status.as_deref() == Some("ABROGE")).then(|| {
        view! {
            <p class="mt-1 w-fit rounded-md border border-[var(--color-rule)] bg-[var(--color-vellum)]/60 px-3 py-1.5 text-sm text-[var(--color-ink-muted)]">
                "Ce texte est abrogé."
            </p>
        }
    });
    // Badge de portée (ADR 0196) : doctrine administrative — les normes n'en
    // portent pas.
    let scope_badge = summary.scope.clone().map(|label| {
        view! {
            <span class="w-fit rounded-full border border-[var(--color-rule)] px-2.5 py-0.5 text-xs normal-case tracking-normal text-[var(--color-ink-muted)]">
                {label}
            </span>
        }
    });
    // Corps monolithique (ADR 0196) : rendu en flux (retours à la ligne
    // préservés), coiffé d'un intertitre quand le texte a aussi des articles.
    let body_section = body.map(|b| {
        let heading = has_articles.then(|| {
            view! {
                <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                    "Texte"
                </h2>
            }
        });
        view! {
            <section class="flex min-w-0 flex-col gap-3">
                {heading}
                <div class="whitespace-pre-line text-base leading-relaxed text-[var(--color-ink)]">
                    {b}
                </div>
            </section>
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

        <div class="mx-auto w-full max-w-[92rem] flex-1 px-4 py-8 sm:px-6 lg:px-8">
            // Gabarit commun /decisions · /textes · /decision : conteneur 92rem,
            // gouttière 240px, colonne contenu bornée 3xl — le rail « Plan du
            // texte » et le contenu tombent au même x que sur les autres pages.
            <div class="grid gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
                // Gouttière gauche (desktop) : la ligne méta coiffe le rail « Plan
                // du texte » — elle remplit le haut de la colonne, ce qui équilibre
                // la page (sinon le rail flottait sous un grand vide).
                <div class="hidden lg:block">
                    <div class="mb-6 flex flex-col gap-5">
                        {meta_line()}
                        {has_articles.then(|| view! {
                            <ChronoDatePicker base=format!("/texte/{}", summary.code) date=chrono_date() />
                        })}
                    </div>
                    {has_articles.then(|| view! { <CodePlanRail toc=toc /> })}
                </div>
                <div class="flex w-full min-w-0 max-w-3xl flex-col gap-8">
                    <header class="flex flex-col gap-3">
                        // Nature en libellé lisible : les textes curés portent un type
                        // technique à underscores (« CODE_PROCEDURE_CIVILE »).
                        <p class="flex flex-wrap items-center gap-2 text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                            <span>{summary.nature.replace('_', " ")}</span>
                            {scope_badge}
                        </p>
                        <h1 class="font-sans text-3xl text-[var(--color-ink)] sm:text-4xl">
                            {title}
                        </h1>
                        // Méta + Chronolégi en mobile uniquement : sur desktop ils
                        // vivent dans la gouttière gauche, au-dessus du plan.
                        <div class="flex flex-col gap-4 lg:hidden">
                            {meta_line()}
                            {has_articles.then(|| view! {
                                <ChronoDatePicker base=format!("/texte/{}", summary.code) date=chrono_date() />
                            })}
                        </div>
                        {upcoming}
                        {abroge}
                    </header>

                    {body_section}
                    {has_articles.then(|| view! {
                        <CodeTocSection toc=toc code=code.clone() />
                    })}
                </div>
            </div>
        </div>
    }
}

/// Recherche bornée à ce code (gabarit Légifrance « Rechercher dans tout le
/// code ») : formulaire GET nu vers `/textes?q=…&code={slug}` — pas de signal,
/// la page textes lit `code` et borne le moteur au texte.
#[component]
fn CodeSearchForm(code: String) -> impl IntoView {
    use crate::components::search::compact_search::SearchIcon;
    view! {
        <form action="/textes" method="get" role="search" class="flex w-full max-w-2xl items-center gap-2">
            <input type="hidden" name="code" value=code />
            <div class="group flex h-11 w-full min-w-0 flex-1 items-center gap-2 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 transition-colors has-[:focus-visible]:border-[var(--color-ink)]">
                <span class="text-[var(--color-ink-subtle)]" aria-hidden="true">
                    <SearchIcon />
                </span>
                <input
                    name="q"
                    size="1"
                    aria-label="Rechercher dans ce code"
                    placeholder="Rechercher dans ce code…"
                    autocomplete="off"
                    class="h-full min-w-0 flex-1 bg-transparent text-[var(--color-ink)] outline-none placeholder:text-[var(--color-ink-subtle)]"
                />
            </div>
            <button
                type="submit"
                class="inline-flex h-11 shrink-0 items-center justify-center whitespace-nowrap rounded-md border border-[var(--color-rule)] bg-[var(--color-ink)] px-4 text-sm font-medium text-[var(--color-parchment)] transition-colors hover:bg-[var(--color-ink-muted)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)]"
            >
                "Rechercher"
            </button>
        </form>
    }
}

/// Table des matières du code (streamée). Erreur ⇒ message discret ; vide ⇒
/// section masquée. L'arbre est reconstruit depuis `title_path` (segments séparés
/// par « > ») : divisions repliables (`<details>`), articles en feuilles.
#[component]
fn CodeTocSection(toc: Resource<TocResult>, code: String) -> impl IntoView {
    let code = StoredValue::new(code);
    let date = chrono_date();
    view! {
        <Suspense fallback=move || {
            view! {
                <p class="text-sm text-[var(--color-ink-subtle)]">"Chargement du sommaire…"</p>
            }
        }>
            {move || Suspend::new(async move {
                let resolved = toc.await;
                toc_view(resolved, code.get_value(), date.get())
            })}
        </Suspense>
    }
}

fn toc_view(resolved: TocResult, code: String, date: Option<String>) -> AnyView {
    if let Some(err) = resolved.error {
        return view! {
            <p class="text-sm text-[var(--color-ink-subtle)]">
                {format!("Sommaire indisponible ({err}).")}
            </p>
        }
        .into_any();
    }
    // Vue-lecture intégrale (textes courts) : les articles se lisent à la
    // suite sur la page — pas de chips ni de recherche bornée à cette échelle.
    // Intertitres ancrés `#{cid}` (le rail « Plan du texte » y pointe).
    if !resolved.reading.is_empty() {
        let items: Vec<AnyView> = resolved
            .reading
            .into_iter()
            .map(|item| {
                crate::pages::law_section_page::section_item_view(item, &code, date.as_deref())
            })
            .collect();
        return view! {
            <section aria-label="Texte intégral" class="flex min-w-0 max-w-3xl flex-col">
                {items}
            </section>
        }
        .into_any();
    }
    // Arbre structurel réel daté (ADR 0207) : sections ancrées (`#{cid}`),
    // vue-lecture par division. Prime sur la reconstruction par `title_path`. Le
    // rail « Plan du texte » vit désormais dans la gouttière gauche de la page
    // (`CodePlanRail`), pas ici — cette vue ne rend que la table des matières.
    if !resolved.tree.is_empty() {
        let (views, _) = render_real_level(&resolved.tree, 0, 1, &code, date.as_deref());
        return with_code_search(&code, toc_section(views));
    }
    if resolved.entries.is_empty() {
        return ().into_any();
    }
    let tree = build_toc_tree(resolved.entries);
    let toc = toc_section(vec![render_nodes(&tree, &code, 0, date.as_deref())]);
    with_code_search(&code, toc)
}

/// Coiffe la table des matières du formulaire de recherche bornée au texte —
/// rendu avec elle (même Suspense) : il n'a de sens que face à un sommaire.
fn with_code_search(code: &str, toc: AnyView) -> AnyView {
    view! {
        <div class="flex min-w-0 flex-col gap-8">
            <CodeSearchForm code=code.to_string() />
            {toc}
        </div>
    }
    .into_any()
}

/// Rail « Plan du texte » de la gouttière gauche, streamé et sticky sur desktop.
/// La colonne gauche parente (`hidden lg:block`, toujours présente) stabilise la
/// grille pendant le streaming et garde l'en-tête aligné même quand le rail est
/// absent (parité /textes : la gouttière existe même sans contenu). Le rail vit
/// sous `lg` uniquement (aucun rail en mobile).
#[component]
fn CodePlanRail(toc: Resource<TocResult>) -> impl IntoView {
    // Pas de wrapper `hidden lg:block` ici : la colonne gauche parente le porte
    // déjà (et fournit le bloc englobant étiré dont le rail sticky a besoin).
    view! {
        <Suspense>
            {move || Suspend::new(async move {
                let resolved = toc.await;
                plan_rail_view(&resolved)
            })}
        </Suspense>
    }
}

/// Contenu du rail : seulement quand l'arbre réel est disponible (`plan_rail`
/// masque déjà les textes à moins de deux divisions). Rien sinon.
fn plan_rail_view(resolved: &TocResult) -> Option<AnyView> {
    if resolved.error.is_some() || resolved.tree.is_empty() {
        return None;
    }
    plan_rail(&resolved.tree)
}

/// Colonne « Table des matières » : en-tête petites capitales + arbre.
fn toc_section(views: Vec<AnyView>) -> AnyView {
    view! {
        <section aria-label="Table des matières" class="flex min-w-0 flex-col gap-3">
            <h2 class="text-xs uppercase tracking-[0.18em] text-[var(--color-ink-subtle)]">
                "Table des matières"
            </h2>
            <div class="flex flex-col">{views}</div>
        </section>
    }
    .into_any()
}

/// Rail « Plan du texte » (sticky, desktop) : les divisions de premier niveau,
/// ancrées `#{cid}` — orientation immédiate dans un code de plusieurs milliers
/// d'articles. Masqué s'il n'y a pas au moins deux divisions.
fn plan_rail(items: &[lj_dtos::TocNode]) -> Option<AnyView> {
    let entries: Vec<AnyView> = items
        .iter()
        .filter(|n| n.depth == 1 && n.kind == "section")
        .map(|n| {
            let href = format!("#{}", n.cid.clone().unwrap_or_default());
            let body = view! {
                <a
                    href=href
                    class="block text-sm leading-snug text-[var(--color-accent)] no-underline hover:underline"
                >
                    {n.label.clone()}
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
            <nav aria-label="Plan du texte" class="hidden lg:sticky lg:top-20 lg:block lg:self-start">
                {rail_block("Plan du texte", entries)}
            </nav>
        }
        .into_any(),
    )
}

/// Contexte de matérialisation en place d'une division (accordéon de lecture,
/// ADR 0214) : de quoi fetcher sa vue-lecture (`/api/texte/{code}/section/{cid}`)
/// à la date de consultation. `cid` n'est lu que par le fetch et l'alignement
/// du fragment — côté client uniquement.
#[derive(Clone)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
struct InlineRead {
    code: String,
    cid: String,
    date: Option<String>,
}

/// Une division du sommaire en `<details>` : chevron custom, libellé, compteur
/// d'articles, lien « Lire » vers la vue-lecture — le tout sur une ligne qui ne
/// casse plus. Tous les niveaux arrivent dépliés : le sommaire entier se lit
/// sans un clic (gabarit Légifrance), le rail « Plan » sert de navigation.
/// `top` (premier niveau) : filet séparateur, typo plus grande.
///
/// `inline` (divisions ≤ [`INLINE_READING_MAX`] articles, ADR 0214) : le lien
/// « Lire » matérialise la vue-lecture sur place — les chips s'effacent, le
/// re-clic (« Sommaire ») les rappelle. Amélioration progressive : sans JS le
/// lien navigue vers la page section (URL canonique) ; clic modifié idem.
/// Arrivée sur `#{cid}` : le scroll natif est complété par la matérialisation
/// de la division pointée (parité Légifrance — un lien copié rend la lecture,
/// pas seulement la position).
fn section_details(
    label: String,
    top: bool,
    article_count: usize,
    id: Option<String>,
    lire: Option<String>,
    inline: Option<InlineRead>,
    children: Vec<AnyView>,
) -> AnyView {
    let (details_cls, summary_cls, label_cls, chevron_top) = if top {
        (
            "scroll-mt-24 border-t border-[var(--color-rule)] py-1 first:border-t-0",
            "flex cursor-pointer list-none items-start gap-2.5 rounded-md px-2 py-3 transition-colors hover:bg-[var(--color-vellum)]/50 [&::-webkit-details-marker]:hidden",
            "min-w-0 flex-1 font-sans text-base font-medium leading-snug text-[var(--color-ink)]",
            "mt-[5px]",
        )
    } else {
        (
            "scroll-mt-24",
            "flex cursor-pointer list-none items-start gap-2.5 rounded-md px-2 py-2 transition-colors hover:bg-[var(--color-vellum)]/50 [&::-webkit-details-marker]:hidden",
            "min-w-0 flex-1 text-sm font-medium leading-snug text-[var(--color-ink)]",
            "mt-1",
        )
    };
    let count = (article_count > 0).then(|| {
        view! {
            <span class="mt-0.5 shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">
                {format!("{} art.", group_thousands(article_count as i64))}
            </span>
        }
    });
    // Accordéon (ADR 0214) : la vue-lecture fetchée remplace les chips tant
    // que la division est dépliée en lecture.
    let open_reading = RwSignal::new(false);
    let reading = RwSignal::new(None::<Vec<LawSectionItem>>);
    // Fetch en vol : un seul départ ; remis à faux sur échec (retente ensuite).
    let loading = StoredValue::new(false);
    let items_ctx = inline.clone();
    // Arrivée sur `#{cid}` (lien copié, rail « Plan ») : matérialise la
    // division pointée en plus du scroll natif du navigateur.
    #[cfg(feature = "hydrate")]
    if let Some(ctx) = inline.clone() {
        Effect::new(move |_| {
            let hash = window().location().hash().unwrap_or_default();
            if hash.trim_start_matches('#') == ctx.cid {
                materialize(ctx.clone(), open_reading, reading, loading);
            }
        });
    }
    let lire = match (lire, inline) {
        (Some(href), Some(ctx)) => {
            Some(inline_read_control(href, ctx, open_reading, reading, loading))
        }
        (Some(href), None) => Some(
            view! {
                <A
                    href=href
                    attr:class="mt-0.5 shrink-0 text-xs text-[var(--color-accent)] no-underline hover:underline"
                >
                    "Lire"
                </A>
            }
            .into_any(),
        ),
        (None, _) => None,
    };
    let reading_view = items_ctx.map(|ctx| {
        move || {
            open_reading.get().then(|| {
                let content: AnyView = match reading.get() {
                    Some(items) => items
                        .into_iter()
                        .map(|it| {
                            crate::pages::law_section_page::section_item_view(
                                it,
                                &ctx.code,
                                ctx.date.as_deref(),
                            )
                        })
                        .collect::<Vec<_>>()
                        .into_any(),
                    None => view! {
                        <p class="py-3 pl-2 text-sm text-[var(--color-ink-subtle)]">
                            "Chargement…"
                        </p>
                    }
                    .into_any(),
                };
                view! {
                    <div class="ml-[14px] flex flex-col border-l border-[var(--color-rule)] pb-2 pl-4">
                        {content}
                    </div>
                }
            })
        }
    });
    view! {
        <details open=true id=id.unwrap_or_default() class=details_cls>
            <summary class=summary_cls>
                <svg
                    aria-hidden="true"
                    viewBox="0 0 12 12"
                    class=format!("toc-chevron h-3 w-3 shrink-0 text-[var(--color-ink-subtle)] transition-transform {chevron_top}")
                >
                    <path
                        d="M4 2l4 4-4 4"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    />
                </svg>
                <span class=label_cls>{label}</span>
                {count}
                {lire}
            </summary>
            <div
                class="ml-[14px] flex flex-col border-l border-[var(--color-rule)] pb-2 pl-4"
                class:hidden=move || open_reading.get()
            >
                {children}
            </div>
            {reading_view}
        </details>
    }
    .into_any()
}

/// Matérialise une division : ouvre la lecture et fetch la vue-lecture au
/// premier dépliage (payload gardé en mémoire). Échec réseau : referme, le
/// prochain déclencheur retente.
#[cfg(feature = "hydrate")]
fn materialize(
    ctx: InlineRead,
    open_reading: RwSignal<bool>,
    reading: RwSignal<Option<Vec<LawSectionItem>>>,
    loading: StoredValue<bool>,
) {
    open_reading.set(true);
    if reading.get_untracked().is_some() || loading.get_value() {
        return;
    }
    loading.set_value(true);
    leptos::task::spawn_local(async move {
        let client = crate::api::client::ApiClient::from_context();
        match client
            .fetch_law_section(&ctx.code, &ctx.cid, ctx.date.as_deref())
            .await
        {
            Ok(resp) => reading.set(Some(resp.items)),
            Err(_) => {
                loading.set_value(false);
                open_reading.set(false);
            }
        }
    });
}

/// Lien « Lire » d'une division lisible sur place (ADR 0214) : au clic simple,
/// toggle de matérialisation, fragment aligné `#{cid}` sans entrée
/// d'historique ; ouvert, le lien devient « Sommaire » (retour aux chips — le
/// chevron de la ligne, lui, plie le sous-arbre). Clic modifié ou sans JS :
/// navigation vers la page section.
fn inline_read_control(
    href: String,
    ctx: InlineRead,
    open_reading: RwSignal<bool>,
    reading: RwSignal<Option<Vec<LawSectionItem>>>,
    loading: StoredValue<bool>,
) -> AnyView {
    #[cfg(feature = "hydrate")]
    let on_click = {
        let ctx = StoredValue::new(ctx);
        move |ev: leptos::ev::MouseEvent| {
            if ev.ctrl_key() || ev.meta_key() || ev.shift_key() || ev.alt_key() {
                return;
            }
            // Annule la navigation ET le toggle du `<details>` parent (le
            // `preventDefault` couvre l'action par défaut du `<summary>`).
            ev.prevent_default();
            if open_reading.get_untracked() {
                open_reading.set(false);
                return;
            }
            sync_hash(&ctx.get_value().cid);
            materialize(ctx.get_value(), open_reading, reading, loading);
        }
    };
    #[cfg(not(feature = "hydrate"))]
    let on_click = {
        let _ = (ctx, reading, loading);
        move |_: leptos::ev::MouseEvent| {}
    };
    let label = move || {
        if open_reading.get() {
            "Sommaire"
        } else {
            "Lire"
        }
    };
    view! {
        <a
            href=href
            on:click=on_click
            class="mt-0.5 shrink-0 text-xs text-[var(--color-accent)] no-underline hover:underline"
        >
            {label}
        </a>
    }
    .into_any()
}

/// Aligne le fragment de l'URL sur la division matérialisée (`#{cid}`) —
/// `replaceState` : pas d'entrée d'historique, pas de saut de scroll.
#[cfg(feature = "hydrate")]
fn sync_hash(cid: &str) {
    if let Ok(history) = window().history() {
        let _ = history.replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("#{cid}")),
        );
    }
}

/// Nombre d'articles du sous-arbre qui commence à `after` (liste aplatie par
/// `depth`, ADR 0207) : les items de profondeur strictement supérieure à `depth`.
fn subtree_article_count(items: &[lj_dtos::TocNode], after: usize, depth: i32) -> usize {
    items[after..]
        .iter()
        .take_while(|n| n.depth > depth)
        .filter(|n| n.kind == "article")
        .count()
}

/// Rend un niveau de l'arbre structurel réel (liste aplatie par `depth`,
/// ADR 0207) à partir de l'index `i` : articles consécutifs en flux, sections
/// en `<details>` ancrés (`id = cid`) avec lien « Lire » vers la vue-lecture.
/// Renvoie `(vues, index du premier item hors de ce niveau)`.
fn render_real_level(
    items: &[lj_dtos::TocNode],
    mut i: usize,
    depth: i32,
    code: &str,
    date: Option<&str>,
) -> (Vec<AnyView>, usize) {
    let mut views: Vec<AnyView> = Vec::new();
    let mut run: Vec<&lj_dtos::TocNode> = Vec::new();
    while i < items.len() && items[i].depth == depth {
        let item = &items[i];
        if item.kind == "article" {
            run.push(item);
            i += 1;
            continue;
        }
        if !run.is_empty() {
            views.push(real_article_flow(std::mem::take(&mut run), code, date));
        }
        let article_count = subtree_article_count(items, i + 1, depth);
        let (children, next) = render_real_level(items, i + 1, depth + 1, code, date);
        let cid = item.cid.clone().unwrap_or_default();
        // La date de consultation suit dans la vue-lecture (Chronolégi).
        let lire = match date {
            Some(d) => format!("/texte/{code}/section/{cid}?date={d}"),
            None => format!("/texte/{code}/section/{cid}"),
        };
        // Division lisible sur place (ADR 0214) : même borne que la
        // vue-lecture intégrale des textes courts.
        let inline =
            (article_count > 0 && article_count <= INLINE_READING_MAX).then(|| InlineRead {
                code: code.to_string(),
                cid: cid.clone(),
                date: date.map(str::to_string),
            });
        views.push(section_details(
            item.label.clone(),
            depth == 1,
            article_count,
            Some(cid),
            Some(lire),
            inline,
            children,
        ));
        i = next;
    }
    if !run.is_empty() {
        views.push(real_article_flow(run, code, date));
    }
    (views, i)
}

/// Flux d'articles consécutifs d'une division de l'arbre réel : chips liées
/// sur la clé canonique, articles abrogés barrés (parité `article_flow`).
fn real_article_flow(items: Vec<&lj_dtos::TocNode>, code: &str, date: Option<&str>) -> AnyView {
    let links = items
        .into_iter()
        .map(|item| {
            let num_key = item.num_key.clone().unwrap_or_default();
            let label = format_article_num(&item.label);
            article_chip(code, num_key, label, item.etat != "VIGUEUR", date)
        })
        .collect_view();
    view! { <div class="flex flex-wrap gap-1.5 py-2 pl-2">{links}</div> }.into_any()
}

/// Chip d'article du sommaire : numéro en petite pastille bordée, abrogé barré.
/// Hover card d'article (ADR 0168) : le texte se lit au survol, sans naviguer.
/// `<a>` natif plutôt que le `<A>` routeur : un gros code en aligne des
/// milliers, on épargne le tracking de route par lien.
fn article_chip(
    code: &str,
    num_key: String,
    label: String,
    abrogated: bool,
    date: Option<&str>,
) -> AnyView {
    use crate::components::hover_preview::{HoverPreview, PreviewKind};
    let cls = if abrogated {
        "rounded-md border border-[var(--color-rule)] px-2 py-1 text-[13px] leading-none tabular-nums text-[var(--color-ink-subtle)] line-through no-underline transition-colors hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
    } else {
        "rounded-md border border-[var(--color-rule)] px-2 py-1 text-[13px] leading-none tabular-nums text-[var(--color-ink-muted)] no-underline transition-colors hover:border-[var(--color-accent)] hover:text-[var(--color-accent)]"
    };
    // Chronolégi : le lien et la hover card portent la date de consultation.
    let href = match date {
        Some(d) => format!("/texte/{code}/{num_key}/{d}"),
        None => format!("/texte/{code}/{num_key}"),
    };
    let kind = PreviewKind::Article {
        code: code.to_string(),
        num: num_key,
        date: date.map(str::to_string),
    };
    view! {
        <HoverPreview kind=kind>
            <a href=href class=cls>
                {label}
            </a>
        </HoverPreview>
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

/// Nombre d'articles (récursif) d'une liste de nœuds du sommaire `title_path`.
fn division_article_count(nodes: &[TocNode]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            TocNode::Article(_) => 1,
            TocNode::Division { children, .. } => division_article_count(children),
        })
        .sum()
}

/// Rend une liste de nœuds : divisions repliables, articles en feuilles. Les
/// divisions racines (parties) sont dépliées d'office — refermées, la page
/// d'arrivée se réduisait à deux lignes mortes. Les articles consécutifs d'une
/// même division coulent en chips (une entrée par ligne noierait les 1 500
/// articles d'un code).
fn render_nodes(nodes: &[TocNode], code: &str, depth: usize, date: Option<&str>) -> AnyView {
    let mut views: Vec<AnyView> = Vec::new();
    let mut run: Vec<&TocEntry> = Vec::new();
    for node in nodes {
        match node {
            TocNode::Article(entry) => run.push(entry),
            TocNode::Division { title, children } => {
                if !run.is_empty() {
                    views.push(article_flow(std::mem::take(&mut run), code, date));
                }
                let inner = vec![render_nodes(children, code, depth + 1, date)];
                views.push(section_details(
                    title.clone(),
                    depth == 0,
                    division_article_count(children),
                    None,
                    None,
                    // Sommaire `title_path` : pas de cid, pas de vue-lecture.
                    None,
                    inner,
                ));
            }
        }
    }
    if !run.is_empty() {
        views.push(article_flow(run, code, date));
    }
    view! { {views} }.into_any()
}

/// Flux d'articles d'une division : chips liées, séparées par l'espace.
fn article_flow(entries: Vec<&TocEntry>, code: &str, date: Option<&str>) -> AnyView {
    let links = entries
        .into_iter()
        .map(|entry| {
            // Chip sur la clé canonique (`numKey`), résolue en lookup exact
            // (ADR 0123 §2).
            let label = format_article_num(&entry.num);
            article_chip(
                code,
                entry.num_key.clone(),
                label,
                entry.status != "VIGUEUR",
                date,
            )
        })
        .collect_view();
    view! { <div class="flex flex-wrap gap-1.5 py-2 pl-2">{links}</div> }.into_any()
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
